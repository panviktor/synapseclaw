//! Port: Memory backend trait — the upstream-compatible storage interface.
//!
//! This is the simpler Memory trait used by the upstream codebase.
//! Adapters implement this for concrete backends (SQLite, Markdown, etc.).

use crate::domain::memory::{MemoryCategory, MemoryEntry};
use anyhow::Result;
use async_trait::async_trait;

/// Core memory trait — implement for any persistence backend.
#[async_trait]
pub trait Memory: Send + Sync {
    /// Backend name
    fn name(&self) -> &str;

    /// Store a memory entry, optionally scoped to a session
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<()>;

    /// Recall memories matching a query (keyword search), optionally scoped to a session
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;

    /// Get a specific memory by key
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>>;

    /// List all memory keys, optionally filtered by category and/or session
    async fn list(
        &self,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;

    /// Remove a memory by key
    async fn forget(&self, key: &str) -> Result<bool>;

    /// Count total memories
    async fn count(&self) -> Result<usize>;

    /// Health check
    async fn health_check(&self) -> bool;
}
