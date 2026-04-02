//! SurrealDB-backed session persistence for channel conversations.
//!
//! Phase 4.5: migrated from SQLite (`session_sqlite.rs`) to the shared
//! SurrealDB instance.  Tables: `channel_session`, `channel_session_meta`,
//! `channel_session_summary` (schema in `surrealdb_schema.surql`).
//!
//! The `SessionBackend` trait is synchronous, so every async SurrealDB call
//! is bridged via `tokio::runtime::Handle::current().block_on()`.

use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use synapse_channels::session_backend::{
    ChannelSummary, SessionBackend, SessionMetadata, SessionQuery,
};
use synapse_providers::traits::ChatMessage;

/// SurrealDB-backed channel session store.
pub struct SurrealSessionBackend {
    db: Arc<Surreal<Db>>,
}

impl SurrealSessionBackend {
    /// Wrap an existing shared SurrealDB handle.
    /// Schema is already applied via `surrealdb_schema.surql` at startup.
    pub fn new(db: Arc<Surreal<Db>>) -> Self {
        Self { db }
    }

    /// Run a future on the current Tokio runtime (sync bridge).
    fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::runtime::Handle::current().block_on(fut)
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|val| val.as_str())
        .unwrap_or_default()
        .to_string()
}

fn json_i64(v: &serde_json::Value, key: &str) -> i64 {
    v.get(key).and_then(|val| val.as_i64()).unwrap_or(0)
}

// ── SessionBackend impl ─────────────────────────────────────────────────

impl SessionBackend for SurrealSessionBackend {
    fn load(&self, session_key: &str) -> Vec<ChatMessage> {
        self.block_on(async {
            let mut resp = match self
                .db
                .query(
                    "SELECT role, content FROM channel_session \
                     WHERE session_key = $key ORDER BY created_at ASC",
                )
                .bind(("key", session_key.to_string()))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("surreal session load: {e}");
                    return Vec::new();
                }
            };
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            rows.iter()
                .map(|v| ChatMessage {
                    role: json_str(v, "role"),
                    content: json_str(v, "content"),
                })
                .collect()
        })
    }

    fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()> {
        self.block_on(async {
            let now = Utc::now().to_rfc3339();

            // Insert message
            self.db
                .query(
                    "CREATE channel_session SET \
                     session_key = $key, role = $role, content = $content, created_at = $now",
                )
                .bind(("key", session_key.to_string()))
                .bind(("role", message.role.clone()))
                .bind(("content", message.content.clone()))
                .bind(("now", now.clone()))
                .await
                .map_err(std::io::Error::other)?;

            // Upsert metadata
            self.db
                .query(
                    "IF (SELECT count() FROM channel_session_meta \
                       WHERE session_key = $key GROUP ALL)[0].count > 0 \
                     { \
                         UPDATE channel_session_meta SET \
                             last_activity = $now, \
                             message_count = message_count + 1 \
                         WHERE session_key = $key; \
                     } ELSE { \
                         CREATE channel_session_meta SET \
                             session_key = $key, \
                             created_at = $now, \
                             last_activity = $now, \
                             message_count = 1; \
                     };",
                )
                .bind(("key", session_key.to_string()))
                .bind(("now", now))
                .await
                .map_err(std::io::Error::other)?;

            Ok(())
        })
    }

    fn remove_last(&self, session_key: &str) -> std::io::Result<bool> {
        self.block_on(async {
            // Check if any messages exist for this session
            let mut resp = self
                .db
                .query(
                    "SELECT count() FROM channel_session \
                     WHERE session_key = $key GROUP ALL",
                )
                .bind(("key", session_key.to_string()))
                .await
                .map_err(std::io::Error::other)?;
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let count = rows
                .first()
                .and_then(|v| v.get("count"))
                .and_then(|c| c.as_i64())
                .unwrap_or(0);
            if count == 0 {
                return Ok(false);
            }

            // Delete the most recent message using a subquery
            self.db
                .query(
                    "DELETE channel_session WHERE id IN \
                     (SELECT id FROM channel_session \
                      WHERE session_key = $key \
                      ORDER BY created_at DESC LIMIT 1)",
                )
                .bind(("key", session_key.to_string()))
                .await
                .map_err(std::io::Error::other)?;

            // Decrement metadata count
            self.db
                .query(
                    "UPDATE channel_session_meta SET \
                     message_count = math::max(0, message_count - 1) \
                     WHERE session_key = $key",
                )
                .bind(("key", session_key.to_string()))
                .await
                .map_err(std::io::Error::other)?;

            Ok(true)
        })
    }

    fn list_sessions(&self) -> Vec<String> {
        self.block_on(async {
            let mut resp = match self
                .db
                .query(
                    "SELECT session_key FROM channel_session_meta \
                     ORDER BY last_activity DESC",
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("surreal session list: {e}");
                    return Vec::new();
                }
            };
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            rows.iter().map(|v| json_str(v, "session_key")).collect()
        })
    }

    fn list_sessions_with_metadata(&self) -> Vec<SessionMetadata> {
        self.block_on(async {
            let mut resp = match self
                .db
                .query(
                    "SELECT session_key, created_at, last_activity, message_count \
                     FROM channel_session_meta ORDER BY last_activity DESC",
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("surreal session list_meta: {e}");
                    return Vec::new();
                }
            };
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            rows.iter()
                .map(|v| {
                    let created = DateTime::parse_from_rfc3339(&json_str(v, "created_at"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    let activity = DateTime::parse_from_rfc3339(&json_str(v, "last_activity"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    SessionMetadata {
                        key: json_str(v, "session_key"),
                        created_at: created,
                        last_activity: activity,
                        message_count: json_i64(v, "message_count") as usize,
                    }
                })
                .collect()
        })
    }

    fn cleanup_stale(&self, ttl_hours: u32) -> std::io::Result<usize> {
        self.block_on(async {
            let cutoff = (Utc::now() - Duration::hours(i64::from(ttl_hours))).to_rfc3339();

            // Find stale session keys
            let mut resp = self
                .db
                .query(
                    "SELECT session_key FROM channel_session_meta \
                     WHERE last_activity < $cutoff",
                )
                .bind(("cutoff", cutoff))
                .await
                .map_err(std::io::Error::other)?;
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let stale_keys: Vec<String> = rows.iter().map(|v| json_str(v, "session_key")).collect();

            let count = stale_keys.len();
            for key in &stale_keys {
                let _ = self
                    .db
                    .query("DELETE FROM channel_session WHERE session_key = $key")
                    .bind(("key", key.clone()))
                    .await;
                let _ = self
                    .db
                    .query("DELETE FROM channel_session_meta WHERE session_key = $key")
                    .bind(("key", key.clone()))
                    .await;
                let _ = self
                    .db
                    .query("DELETE FROM channel_session_summary WHERE session_key = $key")
                    .bind(("key", key.clone()))
                    .await;
            }

            Ok(count)
        })
    }

    fn search(&self, query: &SessionQuery) -> Vec<SessionMetadata> {
        let Some(keyword) = &query.keyword else {
            return self.list_sessions_with_metadata();
        };

        self.block_on(async {
            #[allow(clippy::cast_possible_wrap)]
            let limit = query.limit.unwrap_or(50) as i64;

            // SurrealDB doesn't have FTS5, so use string::contains on content
            let mut resp = match self
                .db
                .query(
                    "SELECT DISTINCT session_key FROM channel_session \
                     WHERE string::contains(string::lowercase(content), string::lowercase($kw)) \
                     LIMIT $lim",
                )
                .bind(("kw", keyword.clone()))
                .bind(("lim", limit))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("surreal session search: {e}");
                    return Vec::new();
                }
            };
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let keys: Vec<String> = rows.iter().map(|v| json_str(v, "session_key")).collect();

            // Look up metadata for matched sessions
            let mut results = Vec::new();
            for key in &keys {
                let mut meta_resp = match self
                    .db
                    .query(
                        "SELECT created_at, last_activity, message_count \
                         FROM channel_session_meta WHERE session_key = $key",
                    )
                    .bind(("key", key.clone()))
                    .await
                {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let meta_rows: Vec<serde_json::Value> = meta_resp.take(0).unwrap_or_default();
                if let Some(v) = meta_rows.first() {
                    let created = DateTime::parse_from_rfc3339(&json_str(v, "created_at"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    let activity = DateTime::parse_from_rfc3339(&json_str(v, "last_activity"))
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    results.push(SessionMetadata {
                        key: key.clone(),
                        created_at: created,
                        last_activity: activity,
                        message_count: json_i64(v, "message_count") as usize,
                    });
                }
            }
            results
        })
    }

    fn load_summary(&self, session_key: &str) -> Option<ChannelSummary> {
        self.block_on(async {
            let mut resp = self
                .db
                .query(
                    "SELECT summary, message_count_at_summary, updated_at \
                     FROM channel_session_summary WHERE session_key = $key LIMIT 1",
                )
                .bind(("key", session_key.to_string()))
                .await
                .ok()?;
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let v = rows.first()?;
            let updated_at = DateTime::parse_from_rfc3339(&json_str(v, "updated_at"))
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            Some(ChannelSummary {
                summary: json_str(v, "summary"),
                message_count_at_summary: json_i64(v, "message_count_at_summary") as usize,
                updated_at,
            })
        })
    }

    fn save_summary(&self, session_key: &str, summary: &ChannelSummary) -> std::io::Result<()> {
        self.block_on(async {
            self.db
                .query(
                    "IF (SELECT count() FROM channel_session_summary \
                       WHERE session_key = $key GROUP ALL)[0].count > 0 \
                     { \
                         UPDATE channel_session_summary SET \
                             summary = $summary, \
                             message_count_at_summary = $mcount, \
                             updated_at = $updated \
                         WHERE session_key = $key; \
                     } ELSE { \
                         CREATE channel_session_summary SET \
                             session_key = $key, \
                             summary = $summary, \
                             message_count_at_summary = $mcount, \
                             updated_at = $updated; \
                     };",
                )
                .bind(("key", session_key.to_string()))
                .bind(("summary", summary.summary.clone()))
                .bind(("mcount", summary.message_count_at_summary as i64))
                .bind(("updated", summary.updated_at.to_rfc3339()))
                .await
                .map_err(std::io::Error::other)?;
            Ok(())
        })
    }

    fn delete(&self, session_key: &str) -> std::io::Result<bool> {
        self.block_on(async {
            // Check existence first
            let mut resp = self
                .db
                .query(
                    "SELECT count() FROM channel_session \
                     WHERE session_key = $key GROUP ALL",
                )
                .bind(("key", session_key.to_string()))
                .await
                .map_err(std::io::Error::other)?;
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let existed = rows
                .first()
                .and_then(|v| v.get("count"))
                .and_then(|c| c.as_i64())
                .unwrap_or(0)
                > 0;

            let _ = self
                .db
                .query("DELETE FROM channel_session WHERE session_key = $key")
                .bind(("key", session_key.to_string()))
                .await;
            let _ = self
                .db
                .query("DELETE FROM channel_session_meta WHERE session_key = $key")
                .bind(("key", session_key.to_string()))
                .await;
            let _ = self
                .db
                .query("DELETE FROM channel_session_summary WHERE session_key = $key")
                .bind(("key", session_key.to_string()))
                .await;

            Ok(existed)
        })
    }
}
