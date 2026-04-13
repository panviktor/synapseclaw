//! SurrealDB-backed session persistence for channel conversations.
//!
//! Tables: `channel_session`, `channel_session_meta`,
//! `channel_session_summary` (schema in `surrealdb_schema.surql`).

use async_trait::async_trait;
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

fn json_opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|val| val.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
}

#[allow(clippy::cast_sign_loss)]
fn json_u64(v: &serde_json::Value, key: &str) -> u64 {
    json_i64(v, key).max(0) as u64
}

impl SurrealSessionBackend {
    async fn ensure_metadata_row(&self, session_key: &str, now: &str) -> std::io::Result<()> {
        self.db
            .query(
                "IF (SELECT count() FROM channel_session_meta \
                   WHERE session_key = $key GROUP ALL)[0].count = 0 \
                 { \
                     CREATE channel_session_meta SET \
                         session_key = $key, \
                         created_at = $now, \
                         last_activity = $now, \
                         message_count = 0, \
                         input_tokens = 0, \
                         output_tokens = 0; \
                 };",
            )
            .bind(("key", session_key.to_string()))
            .bind(("now", now.to_string()))
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }
}

// ── SessionBackend impl ─────────────────────────────────────────────────

#[async_trait]
impl SessionBackend for SurrealSessionBackend {
    async fn load(&self, session_key: &str) -> Vec<ChatMessage> {
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
    }

    async fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()> {
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
                         message_count = 1, \
                         input_tokens = 0, \
                         output_tokens = 0; \
                 };",
            )
            .bind(("key", session_key.to_string()))
            .bind(("now", now))
            .await
            .map_err(std::io::Error::other)?;

        Ok(())
    }

    async fn remove_last(&self, session_key: &str) -> std::io::Result<bool> {
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
    }

    async fn replace(&self, session_key: &str, messages: &[ChatMessage]) -> std::io::Result<()> {
        self.db
            .query("DELETE FROM channel_session WHERE session_key = $key")
            .bind(("key", session_key.to_string()))
            .await
            .map_err(std::io::Error::other)?;

        for message in messages {
            let created_at = Utc::now().to_rfc3339();
            self.db
                .query(
                    "CREATE channel_session SET \
                     session_key = $key, role = $role, content = $content, created_at = $created_at",
                )
                .bind(("key", session_key.to_string()))
                .bind(("role", message.role.clone()))
                .bind(("content", message.content.clone()))
                .bind(("created_at", created_at))
                .await
                .map_err(std::io::Error::other)?;
        }

        if messages.is_empty() {
            self.db
                .query("DELETE FROM channel_session_meta WHERE session_key = $key")
                .bind(("key", session_key.to_string()))
                .await
                .map_err(std::io::Error::other)?;
        } else {
            let now = Utc::now().to_rfc3339();
            self.db
                .query(
                    "IF (SELECT count() FROM channel_session_meta \
                       WHERE session_key = $key GROUP ALL)[0].count > 0 \
                     { \
                         UPDATE channel_session_meta SET \
                             last_activity = $now, \
                             message_count = $mcount \
                         WHERE session_key = $key; \
                     } ELSE { \
                         CREATE channel_session_meta SET \
                             session_key = $key, \
                             created_at = $now, \
                             last_activity = $now, \
                             message_count = $mcount, \
                             input_tokens = 0, \
                             output_tokens = 0; \
                     };",
                )
                .bind(("key", session_key.to_string()))
                .bind(("now", now))
                .bind(("mcount", messages.len() as i64))
                .await
                .map_err(std::io::Error::other)?;
        }

        Ok(())
    }

    async fn list_sessions(&self) -> Vec<String> {
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
    }

    async fn list_sessions_with_metadata(&self) -> Vec<SessionMetadata> {
        let mut resp = match self
            .db
            .query(
                "SELECT session_key, label, current_goal, created_at, last_activity, message_count, \
                        input_tokens, output_tokens \
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
                    label: json_opt_str(v, "label"),
                    current_goal: json_opt_str(v, "current_goal"),
                    created_at: created,
                    last_activity: activity,
                    message_count: json_i64(v, "message_count") as usize,
                    input_tokens: json_u64(v, "input_tokens"),
                    output_tokens: json_u64(v, "output_tokens"),
                }
            })
            .collect()
    }

    async fn touch_session(&self, session_key: &str) -> std::io::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.ensure_metadata_row(session_key, &now).await?;
        self.db
            .query("UPDATE channel_session_meta SET last_activity = $now WHERE session_key = $key")
            .bind(("key", session_key.to_string()))
            .bind(("now", now))
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }

    async fn update_label(&self, session_key: &str, label: &str) -> std::io::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.ensure_metadata_row(session_key, &now).await?;
        self.db
            .query(
                "UPDATE channel_session_meta SET \
                    label = $label, last_activity = $now \
                 WHERE session_key = $key",
            )
            .bind(("key", session_key.to_string()))
            .bind(("label", label.to_string()))
            .bind(("now", now))
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }

    async fn update_goal(&self, session_key: &str, goal: &str) -> std::io::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.ensure_metadata_row(session_key, &now).await?;
        self.db
            .query(
                "UPDATE channel_session_meta SET \
                    current_goal = $goal, last_activity = $now \
                 WHERE session_key = $key",
            )
            .bind(("key", session_key.to_string()))
            .bind(("goal", goal.to_string()))
            .bind(("now", now))
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }

    async fn increment_message_count(&self, session_key: &str) -> std::io::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.ensure_metadata_row(session_key, &now).await?;
        self.db
            .query(
                "UPDATE channel_session_meta SET \
                    message_count = message_count + 1, last_activity = $now \
                 WHERE session_key = $key",
            )
            .bind(("key", session_key.to_string()))
            .bind(("now", now))
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }

    async fn add_token_usage(
        &self,
        session_key: &str,
        input: i64,
        output: i64,
    ) -> std::io::Result<()> {
        let now = Utc::now().to_rfc3339();
        self.ensure_metadata_row(session_key, &now).await?;
        let mut existing = self
            .db
            .query(
                "SELECT input_tokens, output_tokens \
                 FROM channel_session_meta WHERE session_key = $key LIMIT 1",
            )
            .bind(("key", session_key.to_string()))
            .await
            .map_err(std::io::Error::other)?;
        let rows: Vec<serde_json::Value> = existing.take(0).unwrap_or_default();
        let current = rows.first();
        let input_tokens = current
            .map(|row| json_i64(row, "input_tokens"))
            .unwrap_or(0)
            .saturating_add(input)
            .max(0);
        let output_tokens = current
            .map(|row| json_i64(row, "output_tokens"))
            .unwrap_or(0)
            .saturating_add(output)
            .max(0);
        self.db
            .query(
                "UPDATE channel_session_meta SET \
                    input_tokens = $input_tokens, \
                    output_tokens = $output_tokens, \
                    last_activity = $now \
                 WHERE session_key = $key",
            )
            .bind(("key", session_key.to_string()))
            .bind(("input_tokens", input_tokens))
            .bind(("output_tokens", output_tokens))
            .bind(("now", now))
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    }

    async fn cleanup_stale(&self, ttl_hours: u32) -> std::io::Result<usize> {
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
    }

    async fn search(&self, query: &SessionQuery) -> Vec<SessionMetadata> {
        let Some(keyword) = &query.keyword else {
            return self.list_sessions_with_metadata().await;
        };

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
                    "SELECT label, current_goal, created_at, last_activity, message_count, \
                            input_tokens, output_tokens \
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
                    label: json_opt_str(v, "label"),
                    current_goal: json_opt_str(v, "current_goal"),
                    created_at: created,
                    last_activity: activity,
                    message_count: json_i64(v, "message_count") as usize,
                    input_tokens: json_u64(v, "input_tokens"),
                    output_tokens: json_u64(v, "output_tokens"),
                });
            }
        }
        results
    }

    async fn load_summary(&self, session_key: &str) -> Option<ChannelSummary> {
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
    }

    async fn save_summary(
        &self,
        session_key: &str,
        summary: &ChannelSummary,
    ) -> std::io::Result<()> {
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
    }

    async fn delete(&self, session_key: &str) -> std::io::Result<bool> {
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
    }
}
