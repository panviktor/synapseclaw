//! Port: dead letter queue for failed pipeline steps.
//!
//! Phase 4.5: failed pipeline steps are enqueued for inspection,
//! manual retry, or dismissal by operators.

use crate::domain::pipeline_context::DeadLetter;
use async_trait::async_trait;

/// Port for managing the dead letter queue.
///
/// Implementations handle persistence (SQLite, SurrealDB, etc.).
/// The pipeline service enqueues entries after exhausting retries;
/// operators inspect, retry, or dismiss via CLI/API.
#[async_trait]
pub trait DeadLetterPort: Send + Sync {
    /// Enqueue a failed step into the dead letter queue.
    async fn enqueue(&self, letter: DeadLetter) -> anyhow::Result<()>;

    /// List pending dead letters, ordered by creation time (newest first).
    async fn list_pending(&self, limit: usize) -> anyhow::Result<Vec<DeadLetter>>;

    /// List all dead letters (any status), ordered by creation time (newest first).
    async fn list_all(&self, limit: usize) -> anyhow::Result<Vec<DeadLetter>>;

    /// Mark a dead letter as retried (sets status + retried_at timestamp).
    async fn mark_retried(&self, id: &str) -> anyhow::Result<()>;

    /// Dismiss a dead letter without retrying.
    async fn dismiss(&self, id: &str, by: &str) -> anyhow::Result<()>;

    /// Get a single dead letter by ID.
    async fn get(&self, id: &str) -> anyhow::Result<Option<DeadLetter>>;
}
