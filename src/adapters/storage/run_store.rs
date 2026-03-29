//! Adapter: wraps `ChatDb` to implement `RunStorePort`.
//!
//! Uses the `runs` + `run_events` tables added to ChatDb in Phase 4.0.

use crate::adapters::gateway::chat_db::ChatDb;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_core::domain::run::{Run, RunEvent, RunEventType, RunOrigin, RunState};
use synapse_core::ports::run_store::RunStorePort;

/// Wraps `ChatDb` to implement `RunStorePort`.
pub struct ChatDbRunStore {
    db: Arc<ChatDb>,
}

impl ChatDbRunStore {
    pub fn new(db: Arc<ChatDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RunStorePort for ChatDbRunStore {
    async fn create_run(&self, run: &Run) -> anyhow::Result<()> {
        let conn = self.db.conn().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.execute(
            "INSERT OR IGNORE INTO runs (run_id, conversation_key, origin, state, started_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                run.run_id,
                run.conversation_key,
                run.origin.to_string(),
                run.state.to_string(),
                run.started_at as i64,
                chrono::Utc::now().timestamp(),
            ],
        )?;
        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> Option<Run> {
        let conn = self.db.conn().ok()?;
        conn.query_row(
            "SELECT run_id, conversation_key, origin, state, started_at, finished_at
             FROM runs WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok(Run {
                    run_id: row.get(0)?,
                    conversation_key: row.get(1)?,
                    origin: run_origin_from_str(&row.get::<_, String>(2)?),
                    state: RunState::from_str_lossy(&row.get::<_, String>(3)?),
                    #[allow(clippy::cast_sign_loss)]
                    started_at: row.get::<_, i64>(4)? as u64,
                    #[allow(clippy::cast_sign_loss)]
                    finished_at: row.get::<_, Option<i64>>(5)?.map(|t| t as u64),
                })
            },
        )
        .ok()
    }

    async fn update_state(
        &self,
        run_id: &str,
        state: RunState,
        finished_at: Option<u64>,
    ) -> anyhow::Result<()> {
        let conn = self.db.conn().map_err(|e| anyhow::anyhow!("{e}"))?;
        #[allow(clippy::cast_possible_wrap)]
        conn.execute(
            "UPDATE runs SET state = ?1, finished_at = ?2 WHERE run_id = ?3",
            rusqlite::params![state.to_string(), finished_at.map(|t| t as i64), run_id],
        )?;
        Ok(())
    }

    async fn list_runs(&self, conversation_key: &str, limit: usize) -> Vec<Run> {
        let conn = match self.db.conn() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT run_id, conversation_key, origin, state, started_at, finished_at
             FROM runs WHERE conversation_key = ?1 ORDER BY started_at DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        #[allow(clippy::cast_possible_wrap)]
        stmt.query_map(rusqlite::params![conversation_key, limit as i64], |row| {
            Ok(Run {
                run_id: row.get(0)?,
                conversation_key: row.get(1)?,
                origin: run_origin_from_str(&row.get::<_, String>(2)?),
                state: RunState::from_str_lossy(&row.get::<_, String>(3)?),
                #[allow(clippy::cast_sign_loss)]
                started_at: row.get::<_, i64>(4)? as u64,
                #[allow(clippy::cast_sign_loss)]
                finished_at: row.get::<_, Option<i64>>(5)?.map(|t| t as u64),
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    async fn list_all_runs(&self, limit: usize) -> Vec<Run> {
        let conn = match self.db.conn() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT run_id, conversation_key, origin, state, started_at, finished_at
             FROM runs ORDER BY started_at DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        #[allow(clippy::cast_possible_wrap)]
        stmt.query_map(rusqlite::params![limit as i64], |row| {
            Ok(Run {
                run_id: row.get(0)?,
                conversation_key: row.get(1)?,
                origin: run_origin_from_str(&row.get::<_, String>(2)?),
                state: RunState::from_str_lossy(&row.get::<_, String>(3)?),
                #[allow(clippy::cast_sign_loss)]
                started_at: row.get::<_, i64>(4)? as u64,
                #[allow(clippy::cast_sign_loss)]
                finished_at: row.get::<_, Option<i64>>(5)?.map(|t| t as u64),
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    async fn append_event(&self, event: &RunEvent) -> anyhow::Result<()> {
        let conn = self.db.conn().map_err(|e| anyhow::anyhow!("{e}"))?;
        #[allow(clippy::cast_possible_wrap)]
        conn.execute(
            "INSERT INTO run_events (run_id, event_type, content, tool_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                event.run_id,
                event.event_type.to_string(),
                event.content,
                event.tool_name,
                event.created_at as i64,
            ],
        )?;
        Ok(())
    }

    async fn get_events(&self, run_id: &str, limit: usize) -> Vec<RunEvent> {
        let conn = match self.db.conn() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT run_id, event_type, content, tool_name, created_at
             FROM run_events WHERE run_id = ?1 ORDER BY created_at ASC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        #[allow(clippy::cast_possible_wrap)]
        stmt.query_map(rusqlite::params![run_id, limit as i64], |row| {
            Ok(RunEvent {
                run_id: row.get(0)?,
                event_type: RunEventType::from_str_lossy(&row.get::<_, String>(1)?),
                content: row.get(2)?,
                tool_name: row.get(3)?,
                #[allow(clippy::cast_sign_loss)]
                created_at: row.get::<_, i64>(4)? as u64,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }
}

#[allow(clippy::match_same_arms)] // explicit arms document known DB values
fn run_origin_from_str(s: &str) -> RunOrigin {
    match s {
        "web" => RunOrigin::Web,
        "channel" => RunOrigin::Channel,
        "ipc" => RunOrigin::Ipc,
        "spawn" => RunOrigin::Spawn,
        "cron" => RunOrigin::Cron,
        _ => RunOrigin::Channel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (TempDir, ChatDbRunStore) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_runs.db");
        let db = Arc::new(ChatDb::open(&db_path).unwrap());
        (tmp, ChatDbRunStore::new(db))
    }

    #[tokio::test]
    async fn create_and_get_run() {
        let (_tmp, store) = make_store();
        let run = Run {
            run_id: "run-1".into(),
            conversation_key: Some("web:abc:1".into()),
            origin: RunOrigin::Web,
            state: RunState::Running,
            started_at: 1000,
            finished_at: None,
        };
        store.create_run(&run).await.unwrap();
        let loaded = store.get_run("run-1").await.unwrap();
        assert_eq!(loaded.run_id, "run-1");
        assert_eq!(loaded.origin, RunOrigin::Web);
        assert_eq!(loaded.state, RunState::Running);
        assert!(loaded.finished_at.is_none());
    }

    #[tokio::test]
    async fn update_state_to_completed() {
        let (_tmp, store) = make_store();
        store
            .create_run(&Run {
                run_id: "run-2".into(),
                conversation_key: Some("web:abc:1".into()),
                origin: RunOrigin::Channel,
                state: RunState::Running,
                started_at: 1000,
                finished_at: None,
            })
            .await
            .unwrap();

        store
            .update_state("run-2", RunState::Completed, Some(2000))
            .await
            .unwrap();

        let loaded = store.get_run("run-2").await.unwrap();
        assert_eq!(loaded.state, RunState::Completed);
        assert_eq!(loaded.finished_at, Some(2000));
    }

    #[tokio::test]
    async fn list_runs_by_conversation() {
        let (_tmp, store) = make_store();
        for i in 0..3 {
            store
                .create_run(&Run {
                    run_id: format!("run-{i}"),
                    conversation_key: Some("conv-1".into()),
                    origin: RunOrigin::Web,
                    state: RunState::Completed,
                    started_at: 1000 + i,
                    finished_at: Some(2000 + i),
                })
                .await
                .unwrap();
        }
        let runs = store.list_runs("conv-1", 10).await;
        assert_eq!(runs.len(), 3);
        // Newest first
        assert_eq!(runs[0].run_id, "run-2");
    }

    #[tokio::test]
    async fn append_and_get_events() {
        let (_tmp, store) = make_store();
        store
            .create_run(&Run {
                run_id: "run-ev".into(),
                conversation_key: None,
                origin: RunOrigin::Cron,
                state: RunState::Running,
                started_at: 1000,
                finished_at: None,
            })
            .await
            .unwrap();

        store
            .append_event(&RunEvent {
                run_id: "run-ev".into(),
                event_type: RunEventType::ToolCall,
                content: "shell: ls".into(),
                tool_name: Some("shell".into()),
                created_at: 1001,
            })
            .await
            .unwrap();
        store
            .append_event(&RunEvent {
                run_id: "run-ev".into(),
                event_type: RunEventType::Result,
                content: "done".into(),
                tool_name: None,
                created_at: 1002,
            })
            .await
            .unwrap();

        let events = store.get_events("run-ev", 10).await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, RunEventType::ToolCall);
        assert_eq!(events[0].tool_name, Some("shell".into()));
        assert_eq!(events[1].event_type, RunEventType::Result);
    }

    #[tokio::test]
    async fn get_nonexistent_run() {
        let (_tmp, store) = make_store();
        assert!(store.get_run("nope").await.is_none());
    }

    #[tokio::test]
    async fn list_all_runs_across_conversations() {
        let (_tmp, store) = make_store();
        // Create runs in different conversations
        for (i, conv) in ["conv-a", "conv-b"].iter().enumerate() {
            store
                .create_run(&Run {
                    run_id: format!("all-{i}"),
                    conversation_key: Some((*conv).to_string()),
                    origin: RunOrigin::Web,
                    state: RunState::Completed,
                    started_at: 1000 + i as u64,
                    finished_at: Some(2000 + i as u64),
                })
                .await
                .unwrap();
        }
        // list_runs filters by conversation
        assert_eq!(store.list_runs("conv-a", 10).await.len(), 1);
        // list_all_runs returns all
        let all = store.list_all_runs(10).await;
        assert_eq!(all.len(), 2);
        // Newest first
        assert_eq!(all[0].run_id, "all-1");
    }
}
