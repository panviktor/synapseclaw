//! SurrealDB persistence for web chat sessions.
//!
//! Phase 4.5: migrated from SQLite to shared SurrealDB instance.
//! Tables: chat_session, chat_message, run, run_event (schema in surrealdb_schema.surql).

use anyhow::Result;
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;

/// Persistent chat session database backed by SurrealDB.
pub struct ChatDb {
    db: Arc<Surreal<Db>>,
}

/// A row from `chat_session`.
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

/// A row from `chat_message`.
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

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|val| val.as_str())
        .unwrap_or_default()
        .to_string()
}

fn json_i64(v: &serde_json::Value, key: &str) -> i64 {
    v.get(key).and_then(|val| val.as_i64()).unwrap_or(0)
}

fn json_opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|val| val.as_str()).map(String::from)
}

fn json_opt_i64(v: &serde_json::Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|val| val.as_i64())
}

fn row_to_session(v: &serde_json::Value) -> ChatSessionRow {
    ChatSessionRow {
        key: json_str(v, "key"),
        label: json_opt_str(v, "label"),
        current_goal: json_opt_str(v, "current_goal"),
        session_summary: json_opt_str(v, "session_summary"),
        created_at: json_i64(v, "created_at"),
        last_active: json_i64(v, "last_active"),
        message_count: json_i64(v, "message_count"),
        input_tokens: json_i64(v, "input_tokens"),
        output_tokens: json_i64(v, "output_tokens"),
    }
}

fn row_to_message(v: &serde_json::Value) -> ChatMessageRow {
    ChatMessageRow {
        id: json_i64(v, "seq"),
        session_key: json_str(v, "session_key"),
        kind: json_str(v, "kind"),
        role: json_opt_str(v, "role"),
        content: json_str(v, "content"),
        tool_name: json_opt_str(v, "tool_name"),
        run_id: json_opt_str(v, "run_id"),
        input_tokens: json_opt_i64(v, "input_tokens"),
        output_tokens: json_opt_i64(v, "output_tokens"),
        timestamp: json_i64(v, "timestamp"),
    }
}

impl ChatDb {
    /// Create a new ChatDb backed by the shared SurrealDB instance.
    /// Schema is already applied via surrealdb_schema.surql.
    pub fn new(db: Arc<Surreal<Db>>) -> Self {
        Self { db }
    }

    // ── Session CRUD ──────────────────────────────────────────────────

    pub async fn upsert_session(&self, row: &ChatSessionRow) -> Result<()> {
        self.db
            .query(
                "IF (SELECT count() FROM chat_session WHERE key = $key GROUP ALL)[0].count > 0 {
                    UPDATE chat_session SET
                        label = $label, current_goal = $goal, session_summary = $summary,
                        last_active = $last_active, message_count = $msg_count,
                        input_tokens = $in_tok, output_tokens = $out_tok
                    WHERE key = $key;
                } ELSE {
                    CREATE chat_session SET
                        key = $key, label = $label, current_goal = $goal, session_summary = $summary,
                        created_at = $created_at, last_active = $last_active, message_count = $msg_count,
                        input_tokens = $in_tok, output_tokens = $out_tok;
                };",
            )
            .bind(("key", row.key.clone()))
            .bind(("label", row.label.clone()))
            .bind(("goal", row.current_goal.clone()))
            .bind(("summary", row.session_summary.clone()))
            .bind(("created_at", row.created_at))
            .bind(("last_active", row.last_active))
            .bind(("msg_count", row.message_count))
            .bind(("in_tok", row.input_tokens))
            .bind(("out_tok", row.output_tokens))
            .await
            .map_err(|e| anyhow::anyhow!("upsert_session: {e}"))?;
        Ok(())
    }

    pub async fn list_sessions(&self, key_prefix: &str) -> Result<Vec<ChatSessionRow>> {
        let mut resp = if key_prefix.is_empty() {
            self.db
                .query("SELECT * FROM chat_session ORDER BY last_active DESC")
                .await
                .map_err(|e| anyhow::anyhow!("list_sessions: {e}"))?
        } else {
            self.db
                .query(
                    "SELECT * FROM chat_session WHERE string::starts_with(key, $prefix) ORDER BY last_active DESC",
                )
                .bind(("prefix", key_prefix.to_string()))
                .await
                .map_err(|e| anyhow::anyhow!("list_sessions: {e}"))?
        };
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("list_sessions parse: {e}"))?;
        Ok(rows.iter().map(row_to_session).collect())
    }

    pub async fn get_session(&self, key: &str) -> Result<Option<ChatSessionRow>> {
        let mut resp = self
            .db
            .query("SELECT * FROM chat_session WHERE key = $key LIMIT 1")
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("get_session: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("get_session parse: {e}"))?;
        Ok(rows.first().map(row_to_session))
    }

    pub async fn delete_session(&self, key: &str) -> Result<()> {
        self.db
            .query("DELETE FROM chat_message WHERE session_key = $key")
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("delete messages: {e}"))?;
        self.db
            .query("DELETE FROM chat_session WHERE key = $key")
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("delete session: {e}"))?;
        Ok(())
    }

    pub async fn update_session_label(&self, key: &str, label: &str) -> Result<()> {
        self.db
            .query("UPDATE chat_session SET label = $label WHERE key = $key")
            .bind(("key", key.to_string()))
            .bind(("label", label.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("update_label: {e}"))?;
        Ok(())
    }

    pub async fn touch_session(&self, key: &str, now: i64) -> Result<()> {
        self.db
            .query("UPDATE chat_session SET last_active = $now WHERE key = $key")
            .bind(("key", key.to_string()))
            .bind(("now", now))
            .await
            .map_err(|e| anyhow::anyhow!("touch_session: {e}"))?;
        Ok(())
    }

    pub async fn update_session_summary(&self, key: &str, summary: &str) -> Result<()> {
        self.db
            .query("UPDATE chat_session SET session_summary = $summary WHERE key = $key")
            .bind(("key", key.to_string()))
            .bind(("summary", summary.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("update_summary: {e}"))?;
        Ok(())
    }

    pub async fn update_session_goal(&self, key: &str, goal: &str) -> Result<()> {
        self.db
            .query("UPDATE chat_session SET current_goal = $goal WHERE key = $key")
            .bind(("key", key.to_string()))
            .bind(("goal", goal.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("update_goal: {e}"))?;
        Ok(())
    }

    pub async fn increment_message_count(&self, key: &str) -> Result<()> {
        self.db
            .query("UPDATE chat_session SET message_count = message_count + 1 WHERE key = $key")
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("increment_message_count: {e}"))?;
        Ok(())
    }

    pub async fn add_token_usage(&self, key: &str, input: i64, output: i64) -> Result<()> {
        self.db
            .query(
                "UPDATE chat_session SET input_tokens = input_tokens + $input, output_tokens = output_tokens + $output WHERE key = $key",
            )
            .bind(("key", key.to_string()))
            .bind(("input", input))
            .bind(("output", output))
            .await
            .map_err(|e| anyhow::anyhow!("add_token_usage: {e}"))?;
        Ok(())
    }

    // ── Message CRUD ──────────────────────────────────────────────────

    pub async fn append_message(&self, msg: &ChatMessageRow) -> Result<i64> {
        // Generate a monotonic sequence number per session
        let seq = msg.timestamp * 1000 + (msg.id % 1000);
        self.db
            .query(
                "CREATE chat_message SET
                    session_key = $session_key, kind = $kind, role = $role,
                    content = $content, tool_name = $tool_name, run_id = $run_id,
                    input_tokens = $in_tok, output_tokens = $out_tok,
                    timestamp = $timestamp, seq = $seq",
            )
            .bind(("session_key", msg.session_key.clone()))
            .bind(("kind", msg.kind.clone()))
            .bind(("role", msg.role.clone()))
            .bind(("content", msg.content.clone()))
            .bind(("tool_name", msg.tool_name.clone()))
            .bind(("run_id", msg.run_id.clone()))
            .bind(("in_tok", msg.input_tokens))
            .bind(("out_tok", msg.output_tokens))
            .bind(("timestamp", msg.timestamp))
            .bind(("seq", seq))
            .await
            .map_err(|e| anyhow::anyhow!("append_message: {e}"))?;
        Ok(seq)
    }

    pub async fn get_messages(&self, session_key: &str, limit: i64) -> Result<Vec<ChatMessageRow>> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM chat_message WHERE session_key = $key ORDER BY seq DESC LIMIT $limit",
            )
            .bind(("key", session_key.to_string()))
            .bind(("limit", limit))
            .await
            .map_err(|e| anyhow::anyhow!("get_messages: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("get_messages parse: {e}"))?;
        let mut messages: Vec<ChatMessageRow> = rows.iter().map(row_to_message).collect();
        messages.reverse(); // chronological order
        Ok(messages)
    }

    pub async fn clear_messages(&self, session_key: &str) -> Result<()> {
        self.db
            .query("DELETE FROM chat_message WHERE session_key = $key")
            .bind(("key", session_key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("clear_messages: {e}"))?;
        self.db
            .query(
                "UPDATE chat_session SET message_count = 0, input_tokens = 0, output_tokens = 0 WHERE key = $key",
            )
            .bind(("key", session_key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("reset_counters: {e}"))?;
        Ok(())
    }

    /// Get the first user message for auto-labeling.
    pub async fn first_user_message(&self, session_key: &str) -> Result<Option<String>> {
        let mut resp = self
            .db
            .query(
                "SELECT content FROM chat_message WHERE session_key = $key AND kind = 'user' ORDER BY seq ASC LIMIT 1",
            )
            .bind(("key", session_key.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("first_user_message: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("first_user_message parse: {e}"))?;
        Ok(rows
            .first()
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_str())
            .map(String::from))
    }
}
