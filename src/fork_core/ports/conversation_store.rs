//! Port: conversation/session store for durable chat state.
//!
//! Abstracts web chat (`ChatDb`) and channel session persistence behind
//! a single contract.  Phase 4.0 Step 3.

use crate::fork_core::domain::conversation::{ConversationEvent, ConversationSession};
use async_trait::async_trait;

/// Port for storing and retrieving conversation sessions and transcript events.
///
/// Implementations: `ChatDbConversationStore` (wraps existing `ChatDb`).
/// Future: channel sessions can be migrated to the same port.
#[async_trait]
pub trait ConversationStorePort: Send + Sync {
    // ── Session CRUD ────────────────────────────────────────────

    /// Get a session by its unique key.
    async fn get_session(&self, key: &str) -> Option<ConversationSession>;

    /// List sessions, optionally filtered by key prefix.
    async fn list_sessions(&self, prefix: Option<&str>) -> Vec<ConversationSession>;

    /// Create or update a session.
    async fn upsert_session(&self, session: &ConversationSession) -> anyhow::Result<()>;

    /// Delete a session and all its events. Returns true if found.
    async fn delete_session(&self, key: &str) -> anyhow::Result<bool>;

    /// Update last_active timestamp to now.
    async fn touch_session(&self, key: &str) -> anyhow::Result<()>;

    // ── Transcript events ───────────────────────────────────────

    /// Append an event to a session's transcript.
    async fn append_event(
        &self,
        session_key: &str,
        event: &ConversationEvent,
    ) -> anyhow::Result<()>;

    /// Get recent events for a session (newest first up to `limit`, returned chronological).
    async fn get_events(&self, session_key: &str, limit: usize) -> Vec<ConversationEvent>;

    /// Delete all events for a session (reset transcript).
    async fn clear_events(&self, session_key: &str) -> anyhow::Result<()>;

    // ── Summary ─────────────────────────────────────────────────

    /// Get the rolling summary for a session.
    async fn get_summary(&self, key: &str) -> Option<String>;

    /// Set or update the rolling summary.
    async fn set_summary(&self, key: &str, summary: &str) -> anyhow::Result<()>;
}
