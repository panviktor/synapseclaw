//! Port: memory operations — recall, store, consolidate.
//!
//! Abstracts the Memory trait + consolidation so the orchestrator
//! can manage memory without depending on concrete backends.

use anyhow::Result;
use async_trait::async_trait;

/// A recalled memory entry.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub score: Option<f64>,
}

/// Port for memory operations.
#[async_trait]
pub trait MemoryPort: Send + Sync {
    /// Recall relevant memories for a query.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;

    /// Store a memory entry.
    async fn store(
        &self,
        key: &str,
        content: &str,
        session_id: Option<&str>,
    ) -> Result<()>;

    /// Run LLM-driven memory consolidation for a turn (fire-and-forget).
    async fn consolidate_turn(
        &self,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<()>;

    /// Check if content should be skipped for auto-save.
    fn should_skip_autosave(&self, content: &str) -> bool;
}
