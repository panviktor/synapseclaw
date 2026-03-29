//! Trait abstraction for session persistence backends.
//!
//! Backends store per-sender conversation histories. The trait is intentionally
//! minimal — load, append, remove_last, list — so that JSONL and SQLite (and
//! future backends) share a common interface.

use crate::adapters::providers::traits::ChatMessage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata about a persisted session.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// Session key (e.g. `telegram_user123`).
    pub key: String,
    /// When the session was first created.
    pub created_at: DateTime<Utc>,
    /// When the last message was appended.
    pub last_activity: DateTime<Utc>,
    /// Total number of messages in the session.
    pub message_count: usize,
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
/// Implementations must be `Send + Sync` for sharing across async tasks.
pub trait SessionBackend: Send + Sync {
    /// Load all messages for a session. Returns empty vec if session doesn't exist.
    fn load(&self, session_key: &str) -> Vec<ChatMessage>;

    /// Append a single message to a session.
    fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()>;

    /// Remove the last message from a session. Returns `true` if a message was removed.
    fn remove_last(&self, session_key: &str) -> std::io::Result<bool>;

    /// List all session keys.
    fn list_sessions(&self) -> Vec<String>;

    /// List sessions with metadata.
    fn list_sessions_with_metadata(&self) -> Vec<SessionMetadata> {
        // Default: construct metadata from messages (backends can override for efficiency)
        self.list_sessions()
            .into_iter()
            .map(|key| {
                let messages = self.load(&key);
                SessionMetadata {
                    key,
                    created_at: Utc::now(),
                    last_activity: Utc::now(),
                    message_count: messages.len(),
                }
            })
            .collect()
    }

    /// Compact a session file (remove duplicates/corruption). No-op by default.
    fn compact(&self, _session_key: &str) -> std::io::Result<()> {
        Ok(())
    }

    /// Remove sessions that haven't been active within the given TTL hours.
    fn cleanup_stale(&self, _ttl_hours: u32) -> std::io::Result<usize> {
        Ok(0)
    }

    /// Search sessions by keyword. Default returns empty (backends with FTS override).
    fn search(&self, _query: &SessionQuery) -> Vec<SessionMetadata> {
        Vec::new()
    }

    /// Load the rolling summary for a session. Returns `None` if no summary exists.
    fn load_summary(&self, _session_key: &str) -> Option<ChannelSummary> {
        None
    }

    /// Persist a rolling summary for a session.
    fn save_summary(&self, _session_key: &str, _summary: &ChannelSummary) -> std::io::Result<()> {
        Ok(())
    }

    /// Delete a session and its summary. Returns `true` if the session existed.
    fn delete(&self, _session_key: &str) -> std::io::Result<bool> {
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
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 5,
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
