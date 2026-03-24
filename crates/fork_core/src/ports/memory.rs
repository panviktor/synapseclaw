//! Port: tier-aware memory operations.
//!
//! Phase 4.0: explicit three-tier memory model.
//!
//! Tier 1: Working memory — in-run only (RunContext, not in this port)
//! Tier 2: Session memory — conversation-scoped, durable
//! Tier 3: Long-term memory — cross-session knowledge

use crate::domain::memory::{MemoryCategory, MemoryEntry, SessionMemory};
use anyhow::Result;
use async_trait::async_trait;

/// Tier-aware memory port.
///
/// Callers specify which tier they're addressing. The adapter routes
/// to the appropriate backend.
#[async_trait]
pub trait MemoryTiersPort: Send + Sync {
    // ── Tier 2: Session memory ───────────────────────────────────

    /// Get session memory (goal + summary) for a conversation.
    async fn get_session_memory(&self, conversation_key: &str) -> Result<SessionMemory>;

    /// Update the current goal for a conversation.
    async fn set_session_goal(&self, conversation_key: &str, goal: &str) -> Result<()>;

    /// Update the rolling summary for a conversation.
    async fn set_session_summary(&self, conversation_key: &str, summary: &str) -> Result<()>;

    // ── Tier 3: Long-term memory ─────────────────────────────────

    /// Recall relevant long-term memories for a query.
    ///
    /// Searches across categories. Returns entries sorted by relevance.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;

    /// Store a memory entry with explicit category.
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<()>;

    /// Forget a memory entry by key.
    async fn forget(&self, key: &str) -> Result<bool>;

    /// List entries by category.
    async fn list(
        &self,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>>;

    // ── Consolidation ────────────────────────────────────────────

    /// Run LLM-driven memory consolidation for a turn.
    ///
    /// Extracts facts → Core tier, journal → Daily tier.
    async fn consolidate_turn(
        &self,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<()>;

    // ── Utility ──────────────────────────────────────────────────

    /// Check if content should be skipped for auto-save (noise filter).
    fn should_skip_autosave(&self, content: &str) -> bool;

    /// Total entry count across all tiers.
    async fn count(&self) -> Result<usize>;
}

/// Backward-compatible alias — callers that don't need tier awareness.
pub type MemoryPort = dyn MemoryTiersPort;
