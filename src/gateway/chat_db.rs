//! SQLite persistence for web chat sessions.
//!
//! Stores session metadata and message transcripts in `workspace/chat/sessions.db`.
//! WAL mode for concurrent reads during active conversations.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

/// Persistent chat session database.
pub struct ChatDb {
    conn: Mutex<Connection>,
}

/// A row from `chat_sessions`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatSessionRow {
    pub key: String,
    pub label: Option<String>,
    pub current_goal: Option<String>,
    pub session_summary: Option<String>,
    pub created_at: i64,
    pub last_active: i64,
    pub message_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// A row from `chat_messages`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessageRow {
    pub id: i64,
    pub session_key: String,
    pub kind: String,
    pub role: Option<String>,
    pub content: String,
    pub tool_name: Option<String>,
    pub run_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub timestamp: i64,
}

impl ChatDb {
    /// Acquire a lock on the database connection.
    pub fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Open (or create) the chat database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chat_sessions (
                key             TEXT PRIMARY KEY,
                label           TEXT,
                current_goal    TEXT,
                session_summary TEXT,
                created_at      INTEGER NOT NULL,
                last_active     INTEGER NOT NULL,
                message_count   INTEGER DEFAULT 0,
                input_tokens    INTEGER DEFAULT 0,
                output_tokens   INTEGER DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS chat_messages (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key   TEXT NOT NULL REFERENCES chat_sessions(key) ON DELETE CASCADE,
                kind          TEXT NOT NULL,
                role          TEXT,
                content       TEXT NOT NULL,
                tool_name     TEXT,
                run_id        TEXT,
                input_tokens  INTEGER,
                output_tokens INTEGER,
                timestamp     INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_session
                ON chat_messages(session_key, timestamp);

            -- Phase 4.0: unified run execution tracking
            CREATE TABLE IF NOT EXISTS runs (
                run_id           TEXT PRIMARY KEY,
                conversation_key TEXT,
                origin           TEXT NOT NULL,
                state            TEXT NOT NULL DEFAULT 'running',
                started_at       INTEGER NOT NULL,
                finished_at      INTEGER,
                created_at       INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_runs_conversation
                ON runs(conversation_key);

            CREATE TABLE IF NOT EXISTS run_events (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id     TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                content    TEXT NOT NULL,
                tool_name  TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_run_events_run
                ON run_events(run_id, created_at);",
        )?;
        Ok(())
    }

    // ── Session CRUD ──────────────────────────────────────────────────

    pub fn upsert_session(&self, row: &ChatSessionRow) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "INSERT INTO chat_sessions (key, label, current_goal, session_summary, created_at, last_active, message_count, input_tokens, output_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(key) DO UPDATE SET
                label = ?2, current_goal = ?3, session_summary = ?4,
                last_active = ?6, message_count = ?7, input_tokens = ?8, output_tokens = ?9",
            params![
                row.key,
                row.label,
                row.current_goal,
                row.session_summary,
                row.created_at,
                row.last_active,
                row.message_count,
                row.input_tokens,
                row.output_tokens,
            ],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self, key_prefix: &str) -> Result<Vec<ChatSessionRow>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT key, label, current_goal, session_summary, created_at, last_active, message_count, input_tokens, output_tokens
             FROM chat_sessions WHERE key LIKE ?1 ORDER BY last_active DESC",
        )?;
        let prefix = format!("{key_prefix}%");
        let rows = stmt
            .query_map(params![prefix], |row| {
                Ok(ChatSessionRow {
                    key: row.get(0)?,
                    label: row.get(1)?,
                    current_goal: row.get(2)?,
                    session_summary: row.get(3)?,
                    created_at: row.get(4)?,
                    last_active: row.get(5)?,
                    message_count: row.get(6)?,
                    input_tokens: row.get(7)?,
                    output_tokens: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_session(&self, key: &str) -> Result<Option<ChatSessionRow>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT key, label, current_goal, session_summary, created_at, last_active, message_count, input_tokens, output_tokens
             FROM chat_sessions WHERE key = ?1",
        )?;
        let mut rows = stmt.query_map(params![key], |row| {
            Ok(ChatSessionRow {
                key: row.get(0)?,
                label: row.get(1)?,
                current_goal: row.get(2)?,
                session_summary: row.get(3)?,
                created_at: row.get(4)?,
                last_active: row.get(5)?,
                message_count: row.get(6)?,
                input_tokens: row.get(7)?,
                output_tokens: row.get(8)?,
            })
        })?;
        match rows.next() {
            Some(Ok(r)) => Ok(Some(r)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn delete_session(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "DELETE FROM chat_messages WHERE session_key = ?1",
            params![key],
        )?;
        conn.execute("DELETE FROM chat_sessions WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn update_session_label(&self, key: &str, label: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE chat_sessions SET label = ?2 WHERE key = ?1",
            params![key, label],
        )?;
        Ok(())
    }

    pub fn touch_session(&self, key: &str, now: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE chat_sessions SET last_active = ?2 WHERE key = ?1",
            params![key, now],
        )?;
        Ok(())
    }

    pub fn update_session_summary(&self, key: &str, summary: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE chat_sessions SET session_summary = ?2 WHERE key = ?1",
            params![key, summary],
        )?;
        Ok(())
    }

    pub fn update_session_goal(&self, key: &str, goal: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE chat_sessions SET current_goal = ?2 WHERE key = ?1",
            params![key, goal],
        )?;
        Ok(())
    }

    pub fn increment_message_count(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE chat_sessions SET message_count = message_count + 1 WHERE key = ?1",
            params![key],
        )?;
        Ok(())
    }

    pub fn add_token_usage(&self, key: &str, input: i64, output: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "UPDATE chat_sessions SET input_tokens = input_tokens + ?2, output_tokens = output_tokens + ?3 WHERE key = ?1",
            params![key, input, output],
        )?;
        Ok(())
    }

    // ── Message CRUD ──────────────────────────────────────────────────

    pub fn append_message(&self, msg: &ChatMessageRow) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "INSERT INTO chat_messages (session_key, kind, role, content, tool_name, run_id, input_tokens, output_tokens, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                msg.session_key,
                msg.kind,
                msg.role,
                msg.content,
                msg.tool_name,
                msg.run_id,
                msg.input_tokens,
                msg.output_tokens,
                msg.timestamp,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_messages(&self, session_key: &str, limit: i64) -> Result<Vec<ChatMessageRow>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT id, session_key, kind, role, content, tool_name, run_id, input_tokens, output_tokens, timestamp
             FROM chat_messages WHERE session_key = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![session_key, limit], |row| {
                Ok(ChatMessageRow {
                    id: row.get(0)?,
                    session_key: row.get(1)?,
                    kind: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    tool_name: row.get(5)?,
                    run_id: row.get(6)?,
                    input_tokens: row.get(7)?,
                    output_tokens: row.get(8)?,
                    timestamp: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        // Reverse to chronological order
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }

    pub fn clear_messages(&self, session_key: &str) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "DELETE FROM chat_messages WHERE session_key = ?1",
            params![session_key],
        )?;
        conn.execute(
            "UPDATE chat_sessions SET message_count = 0, input_tokens = 0, output_tokens = 0 WHERE key = ?1",
            params![session_key],
        )?;
        Ok(())
    }

    /// Get the first user message for auto-labeling.
    pub fn first_user_message(&self, session_key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT content FROM chat_messages WHERE session_key = ?1 AND kind = 'user' ORDER BY id ASC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![session_key], |row| row.get::<_, String>(0))?;
        match rows.next() {
            Some(Ok(c)) => Ok(Some(c)),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn roundtrip_session_and_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db = ChatDb::open(&dir.path().join("test.db")).unwrap();
        let now = now_secs();

        let session = ChatSessionRow {
            key: "web:abc123:default".into(),
            label: Some("Test".into()),
            current_goal: None,
            session_summary: None,
            created_at: now,
            last_active: now,
            message_count: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        db.upsert_session(&session).unwrap();

        let msg = ChatMessageRow {
            id: 0,
            session_key: "web:abc123:default".into(),
            kind: "user".into(),
            role: Some("user".into()),
            content: "Hello".into(),
            tool_name: None,
            run_id: None,
            input_tokens: None,
            output_tokens: None,
            timestamp: now,
        };
        let id = db.append_message(&msg).unwrap();
        assert!(id > 0);

        db.increment_message_count("web:abc123:default").unwrap();

        let messages = db.get_messages("web:abc123:default", 50).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello");

        let sessions = db.list_sessions("web:abc123:").unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 1);

        let first = db.first_user_message("web:abc123:default").unwrap();
        assert_eq!(first.as_deref(), Some("Hello"));

        db.delete_session("web:abc123:default").unwrap();
        let sessions = db.list_sessions("web:abc123:").unwrap();
        assert!(sessions.is_empty());
    }
}
