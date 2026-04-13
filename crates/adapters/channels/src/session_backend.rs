//! Trait abstraction for session persistence backends.
//!
//! Backends store per-sender conversation histories. The trait keeps transcript
//! writes and session metadata behind one channel-facing port so web/channel
//! runtime code does not grow separate persistence semantics.
//!
//! Phase 4.5: made async to support SurrealDB without `block_on()`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use synapse_providers::traits::ChatMessage;

/// Metadata about a persisted session.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// Session key (e.g. `telegram_user123`).
    pub key: String,
    /// Optional human-readable label.
    pub label: Option<String>,
    /// Optional current goal extracted for the session.
    pub current_goal: Option<String>,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the last message was appended.
    pub last_activity: DateTime<Utc>,
    /// Total number of messages in the session.
    pub message_count: usize,
    /// Provider input tokens accumulated for this session.
    pub input_tokens: u64,
    /// Provider output tokens accumulated for this session.
    pub output_tokens: u64,
}

/// Rolling summary of a channel conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSummary {
    /// Semantic summary text (2-3 sentences, max 300 chars).
    pub summary: String,
    /// Message count at the time this summary was generated.
    pub message_count_at_summary: usize,
    /// When this summary was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Query parameters for listing sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionQuery {
    /// Keyword to search in session messages (FTS5 if available).
    pub keyword: Option<String>,
    /// Maximum number of sessions to return.
    pub limit: Option<usize>,
}

/// Trait for session persistence backends.
///
/// All methods are async to support SurrealDB and other async backends.
/// SQLite implementations can use synchronous I/O internally since async-trait
/// wraps them in `Box::pin(async { ... })`.
#[async_trait]
pub trait SessionBackend: Send + Sync {
    /// Load all messages for a session. Returns empty vec if session doesn't exist.
    async fn load(&self, session_key: &str) -> Vec<ChatMessage>;

    /// Append a single message to a session.
    async fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()>;

    /// Remove the last message from a session. Returns `true` if a message was removed.
    async fn remove_last(&self, session_key: &str) -> std::io::Result<bool>;

    /// Replace only the message history with already-compacted provider history.
    /// Rolling summaries must be preserved.
    async fn replace(&self, session_key: &str, messages: &[ChatMessage]) -> std::io::Result<()>;

    /// List all session keys.
    async fn list_sessions(&self) -> Vec<String>;

    /// List sessions with metadata.
    async fn list_sessions_with_metadata(&self) -> Vec<SessionMetadata> {
        // Default: construct metadata from messages (backends can override for efficiency)
        let keys = self.list_sessions().await;
        let mut result = Vec::with_capacity(keys.len());
        for key in keys {
            let messages = self.load(&key).await;
            result.push(SessionMetadata {
                key,
                label: None,
                current_goal: None,
                created_at: Utc::now(),
                last_activity: Utc::now(),
                message_count: messages.len(),
                input_tokens: 0,
                output_tokens: 0,
            });
        }
        result
    }

    /// Touch a session's last activity without changing transcript content.
    async fn touch_session(&self, session_key: &str) -> std::io::Result<()> {
        let messages = self.load(session_key).await;
        self.replace(session_key, &messages).await
    }

    /// Update the durable user-facing label for a session.
    async fn update_label(&self, _session_key: &str, _label: &str) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "session backend does not support durable labels",
        ))
    }

    /// Update the durable current goal for a session.
    async fn update_goal(&self, _session_key: &str, _goal: &str) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "session backend does not support durable goals",
        ))
    }

    /// Increment the durable message count independently from transcript append.
    async fn increment_message_count(&self, _session_key: &str) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "session backend derives message count from transcript writes",
        ))
    }

    /// Add provider token accounting to durable session metadata.
    async fn add_token_usage(
        &self,
        _session_key: &str,
        _input: i64,
        _output: i64,
    ) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "session backend does not support durable token usage",
        ))
    }

    /// Compact a session file (remove duplicates/corruption). No-op by default.
    async fn compact(&self, _session_key: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Remove sessions that haven't been active within the given TTL hours.
    async fn cleanup_stale(&self, _ttl_hours: u32) -> std::io::Result<usize> {
        Ok(0)
    }

    /// Search sessions by keyword. Default returns empty (backends with FTS override).
    async fn search(&self, _query: &SessionQuery) -> Vec<SessionMetadata> {
        Vec::new()
    }

    /// Load the rolling summary for a session. Returns `None` if no summary exists.
    async fn load_summary(&self, _session_key: &str) -> Option<ChannelSummary> {
        None
    }

    /// Persist a rolling summary for a session.
    async fn save_summary(
        &self,
        _session_key: &str,
        _summary: &ChannelSummary,
    ) -> std::io::Result<()> {
        Ok(())
    }

    /// Delete a session and its summary. Returns `true` if the session existed.
    async fn delete(&self, _session_key: &str) -> std::io::Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_metadata_is_constructible() {
        let meta = SessionMetadata {
            key: "test".into(),
            label: None,
            current_goal: None,
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 5,
            input_tokens: 0,
            output_tokens: 0,
        };
        assert_eq!(meta.key, "test");
        assert_eq!(meta.message_count, 5);
    }

    #[test]
    fn session_query_defaults() {
        let q = SessionQuery::default();
        assert!(q.keyword.is_none());
        assert!(q.limit.is_none());
    }

    #[test]
    fn channel_summary_serde_roundtrip() {
        let summary = ChannelSummary {
            summary: "User discussed project timeline and assigned tasks.".into(),
            message_count_at_summary: 20,
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: ChannelSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.summary, summary.summary);
        assert_eq!(
            deserialized.message_count_at_summary,
            summary.message_count_at_summary
        );
    }
}
