//! Adapter: SurrealDB-backed `RunStorePort`.
//!
//! Phase 4.5: migrated from SQLite ChatDb to shared SurrealDB.
//! Uses `run` + `run_event` tables (schema in surrealdb_schema.surql).

use async_trait::async_trait;
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use synapse_domain::domain::run::{Run, RunEvent, RunEventType, RunOrigin, RunState};
use synapse_domain::ports::run_store::RunStorePort;

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|val| val.as_str())
        .unwrap_or_default()
        .to_string()
}

fn json_opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|val| val.as_str()).map(String::from)
}

fn json_i64(v: &serde_json::Value, key: &str) -> i64 {
    v.get(key).and_then(|val| val.as_i64()).unwrap_or(0)
}

/// SurrealDB-backed RunStore.
pub struct SurrealRunStore {
    db: Arc<Surreal<Db>>,
}

impl SurrealRunStore {
    pub fn new(db: Arc<Surreal<Db>>) -> Self {
        Self { db }
    }
}

fn row_to_run(v: &serde_json::Value) -> Run {
    Run {
        run_id: json_str(v, "run_id"),
        conversation_key: json_opt_str(v, "conversation_key"),
        origin: run_origin_from_str(&json_str(v, "origin")),
        state: RunState::from_str_lossy(&json_str(v, "state")),
        #[allow(clippy::cast_sign_loss)]
        started_at: json_i64(v, "started_at") as u64,
        #[allow(clippy::cast_sign_loss)]
        finished_at: v
            .get("finished_at")
            .and_then(|v| v.as_i64())
            .map(|t| t as u64),
    }
}

fn row_to_event(v: &serde_json::Value) -> RunEvent {
    RunEvent {
        run_id: json_str(v, "run_id"),
        event_type: RunEventType::from_str_lossy(&json_str(v, "event_type")),
        content: json_str(v, "content"),
        tool_name: json_opt_str(v, "tool_name"),
        #[allow(clippy::cast_sign_loss)]
        created_at: json_i64(v, "created_at") as u64,
    }
}

#[async_trait]
impl RunStorePort for SurrealRunStore {
    async fn create_run(&self, run: &Run) -> anyhow::Result<()> {
        #[allow(clippy::cast_possible_wrap)]
        self.db
            .query(
                "CREATE run SET
                    run_id = $run_id, conversation_key = $conv_key, origin = $origin,
                    state = $state, started_at = $started_at, created_at = $created_at",
            )
            .bind(("run_id", run.run_id.clone()))
            .bind(("conv_key", run.conversation_key.clone()))
            .bind(("origin", run.origin.to_string()))
            .bind(("state", run.state.to_string()))
            .bind(("started_at", run.started_at as i64))
            .bind(("created_at", chrono::Utc::now().timestamp()))
            .await
            .map_err(|e| anyhow::anyhow!("create_run: {e}"))?;
        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> Option<Run> {
        let mut resp = self
            .db
            .query("SELECT * FROM run WHERE run_id = $run_id LIMIT 1")
            .bind(("run_id", run_id.to_string()))
            .await
            .ok()?;
        let rows: Vec<serde_json::Value> = resp.take(0).ok()?;
        rows.first().map(row_to_run)
    }

    async fn update_state(
        &self,
        run_id: &str,
        state: RunState,
        finished_at: Option<u64>,
    ) -> anyhow::Result<()> {
        #[allow(clippy::cast_possible_wrap)]
        self.db
            .query(
                "UPDATE run SET state = $state, finished_at = $finished_at WHERE run_id = $run_id",
            )
            .bind(("run_id", run_id.to_string()))
            .bind(("state", state.to_string()))
            .bind(("finished_at", finished_at.map(|t| t as i64)))
            .await
            .map_err(|e| anyhow::anyhow!("update_state: {e}"))?;
        Ok(())
    }

    async fn list_runs(&self, conversation_key: &str, limit: usize) -> Vec<Run> {
        let mut resp = match self
            .db
            .query(
                "SELECT * FROM run WHERE conversation_key = $conv ORDER BY started_at DESC LIMIT $limit",
            )
            .bind(("conv", conversation_key.to_string()))
            .bind(("limit", limit as i64))
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        rows.iter().map(row_to_run).collect()
    }

    async fn list_all_runs(&self, limit: usize) -> Vec<Run> {
        let mut resp = match self
            .db
            .query("SELECT * FROM run ORDER BY started_at DESC LIMIT $limit")
            .bind(("limit", limit as i64))
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        rows.iter().map(row_to_run).collect()
    }

    async fn append_event(&self, event: &RunEvent) -> anyhow::Result<()> {
        #[allow(clippy::cast_possible_wrap)]
        self.db
            .query(
                "CREATE run_event SET
                    run_id = $run_id, event_type = $event_type, content = $content,
                    tool_name = $tool_name, created_at = $created_at",
            )
            .bind(("run_id", event.run_id.clone()))
            .bind(("event_type", event.event_type.to_string()))
            .bind(("content", event.content.clone()))
            .bind(("tool_name", event.tool_name.clone()))
            .bind(("created_at", event.created_at as i64))
            .await
            .map_err(|e| anyhow::anyhow!("append_event: {e}"))?;
        Ok(())
    }

    async fn get_events(&self, run_id: &str, limit: usize) -> Vec<RunEvent> {
        let mut resp = match self
            .db
            .query(
                "SELECT * FROM run_event WHERE run_id = $run_id ORDER BY created_at ASC LIMIT $limit",
            )
            .bind(("run_id", run_id.to_string()))
            .bind(("limit", limit as i64))
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        rows.iter().map(row_to_event).collect()
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
