//! SurrealDB persistence for heartbeat task execution history.
//!
//! Phase 4.5: migrated from SQLite to shared SurrealDB instance.
//! Schema defined in `surrealdb_schema.surql` (heartbeat_run table).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use surrealdb::engine::local::Db;
use surrealdb::Surreal;

const MAX_OUTPUT_BYTES: usize = 16 * 1024;
const TRUNCATED_MARKER: &str = "\n...[truncated]";

/// A single heartbeat task execution record.
#[derive(Debug, Clone)]
pub struct HeartbeatRun {
    pub id: String,
    pub task_text: String,
    pub task_priority: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub status: String, // "ok" or "error"
    pub output: Option<String>,
    pub duration_ms: i64,
}

/// Record a heartbeat task execution and prune old entries.
#[allow(clippy::too_many_arguments)]
pub async fn record_run(
    db: &Surreal<Db>,
    task_text: &str,
    task_priority: &str,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    status: &str,
    output: Option<&str>,
    duration_ms: i64,
    max_history: u32,
) -> Result<()> {
    let bounded_output = output.map(truncate_output);

    db.query(
        "CREATE heartbeat_run SET
            task_text = $task_text,
            task_priority = $task_priority,
            started_at = $started_at,
            finished_at = $finished_at,
            status = $status,
            output = $output,
            duration_ms = $duration_ms",
    )
    .bind(("task_text", task_text.to_string()))
    .bind(("task_priority", task_priority.to_string()))
    .bind(("started_at", started_at.to_rfc3339()))
    .bind(("finished_at", finished_at.to_rfc3339()))
    .bind(("status", status.to_string()))
    .bind(("output", bounded_output))
    .bind(("duration_ms", duration_ms))
    .await
    .context("Failed to insert heartbeat run")?;

    // Prune oldest entries beyond max_history
    let keep = i64::from(max_history.max(1));
    db.query(
        "DELETE heartbeat_run WHERE id NOT IN (
            SELECT VALUE id FROM heartbeat_run
            ORDER BY started_at DESC
            LIMIT $keep
        )",
    )
    .bind(("keep", keep))
    .await
    .context("Failed to prune heartbeat run history")?;

    Ok(())
}

/// List the most recent heartbeat runs.
pub async fn list_runs(db: &Surreal<Db>, limit: usize) -> Result<Vec<HeartbeatRun>> {
    let lim = i64::try_from(limit.max(1)).context("Run history limit overflow")?;

    let mut resp = db
        .query(
            "SELECT * FROM heartbeat_run
             ORDER BY started_at DESC
             LIMIT $limit",
        )
        .bind(("limit", lim))
        .await
        .context("Failed to query heartbeat runs")?;

    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    rows.iter().map(row_to_run).collect()
}

/// Get aggregate stats: (total_runs, total_ok, total_error).
pub async fn run_stats(db: &Surreal<Db>) -> Result<(u64, u64, u64)> {
    let mut resp = db
        .query(
            "SELECT
                count() AS total,
                count(status = 'ok' OR NULL) AS ok,
                count(status = 'error' OR NULL) AS err
             FROM heartbeat_run GROUP ALL",
        )
        .await
        .context("Failed to query heartbeat run stats")?;

    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    if let Some(row) = rows.first() {
        let total = row.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
        let ok = row.get("ok").and_then(|v| v.as_u64()).unwrap_or(0);
        let err = row.get("err").and_then(|v| v.as_u64()).unwrap_or(0);
        Ok((total, ok, err))
    } else {
        // No rows means empty table
        Ok((0, 0, 0))
    }
}

fn row_to_run(v: &serde_json::Value) -> Result<HeartbeatRun> {
    let id = v
        .get("id")
        .and_then(|val| val.as_str().or_else(|| None))
        .map(String::from)
        .unwrap_or_else(|| {
            // SurrealDB Thing IDs can be objects like {"tb":"heartbeat_run","id":{"String":"..."}}
            v.get("id").map(|val| val.to_string()).unwrap_or_default()
        });

    let started_at = parse_rfc3339(
        v.get("started_at")
            .and_then(|val| val.as_str())
            .unwrap_or_default(),
    )?;
    let finished_at = parse_rfc3339(
        v.get("finished_at")
            .and_then(|val| val.as_str())
            .unwrap_or_default(),
    )?;

    Ok(HeartbeatRun {
        id,
        task_text: json_str(v, "task_text"),
        task_priority: json_str(v, "task_priority"),
        started_at,
        finished_at,
        status: json_str(v, "status"),
        output: json_opt_str(v, "output"),
        duration_ms: json_i64(v, "duration_ms"),
    })
}

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

fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_BYTES {
        return output.to_string();
    }

    if MAX_OUTPUT_BYTES <= TRUNCATED_MARKER.len() {
        return TRUNCATED_MARKER.to_string();
    }

    let mut cutoff = MAX_OUTPUT_BYTES - TRUNCATED_MARKER.len();
    while cutoff > 0 && !output.is_char_boundary(cutoff) {
        cutoff -= 1;
    }

    let mut truncated = output[..cutoff].to_string();
    truncated.push_str(TRUNCATED_MARKER);
    truncated
}

fn parse_rfc3339(raw: &str) -> Result<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(raw)
        .with_context(|| format!("Invalid RFC3339 timestamp in heartbeat DB: {raw}"))?;
    Ok(parsed.with_timezone(&Utc))
}
