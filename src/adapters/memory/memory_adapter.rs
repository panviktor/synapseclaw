//! Adapter: wraps existing `dyn Memory` + `ConversationStorePort` as MemoryTiersPort.

use crate::adapters::providers::Provider;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_core::domain::memory::{MemoryCategory, MemoryEntry, SessionMemory};
use synapse_core::ports::conversation_store::ConversationStorePort;
use synapse_core::ports::memory::MemoryTiersPort;
use synapse_core::ports::memory_backend::Memory;

pub struct MemoryTiersAdapter {
    /// Long-term memory backend (sqlite/qdrant/markdown).
    memory: Arc<dyn Memory>,
    /// Conversation store for session memory (goal/summary).
    conversation_store: Option<Arc<dyn ConversationStorePort>>,
    /// Provider for consolidation LLM calls.
    provider: Arc<dyn Provider>,
    /// Model for consolidation.
    model: String,
}

impl MemoryTiersAdapter {
    pub fn new(
        memory: Arc<dyn Memory>,
        conversation_store: Option<Arc<dyn ConversationStorePort>>,
        provider: Arc<dyn Provider>,
        model: String,
    ) -> Self {
        Self {
            memory,
            conversation_store,
            provider,
            model,
        }
    }
}

// MemoryCategory and MemoryEntry are now the same type in both
// fork_core and crate::memory (re-exported from fork_core).
// No conversion functions needed.

/// Alias for readability — identity function since types are unified.
fn to_upstream_category(cat: &MemoryCategory) -> MemoryCategory {
    cat.clone()
}

/// Alias for readability — identity function since types are unified.
fn to_domain_entry(e: synapse_core::domain::memory::MemoryEntry) -> MemoryEntry {
    e
}

#[async_trait]
impl MemoryTiersPort for MemoryTiersAdapter {
    // ── Tier 2: Session memory ───────────────────────────────────

    async fn get_session_memory(&self, conversation_key: &str) -> Result<SessionMemory> {
        if let Some(ref store) = self.conversation_store {
            let session = store.get_session(conversation_key).await;
            Ok(SessionMemory {
                conversation_key: conversation_key.to_string(),
                goal: session.as_ref().and_then(|s| s.current_goal.clone()),
                summary: session.as_ref().and_then(|s| s.summary.clone()),
            })
        } else {
            Ok(SessionMemory {
                conversation_key: conversation_key.to_string(),
                ..Default::default()
            })
        }
    }

    async fn set_session_goal(&self, conversation_key: &str, goal: &str) -> Result<()> {
        if let Some(ref store) = self.conversation_store {
            store.update_goal(conversation_key, goal).await?;
        }
        Ok(())
    }

    async fn set_session_summary(&self, conversation_key: &str, summary: &str) -> Result<()> {
        if let Some(ref store) = self.conversation_store {
            store.set_summary(conversation_key, summary).await?;
        }
        Ok(())
    }

    // ── Tier 3: Long-term memory ─────────────────────────────────

    async fn recall(
        &self,
        query: &str,
        limit: usize,
        _category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        // Upstream Memory::recall doesn't filter by category — recall all, let caller filter
        let entries = self.memory.recall(query, limit, session_id).await?;
        Ok(entries.into_iter().map(to_domain_entry).collect())
    }

    async fn store(
        &self,
        key: &str,
        content: &str,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<()> {
        self.memory
            .store(key, content, to_upstream_category(category), session_id)
            .await
    }

    async fn forget(&self, key: &str) -> Result<bool> {
        self.memory.forget(key).await
    }

    async fn list(
        &self,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let entries = self
            .memory
            .list(Some(&to_upstream_category(category)), session_id)
            .await?;
        Ok(entries.into_iter().map(to_domain_entry).collect())
    }

    // ── Consolidation ────────────────────────────────────────────

    async fn consolidate_turn(&self, user_message: &str, assistant_response: &str) -> Result<()> {
        crate::memory::consolidation::consolidate_turn(
            self.provider.as_ref(),
            &self.model,
            self.memory.as_ref(),
            user_message,
            assistant_response,
        )
        .await
    }

    // ── Utility ──────────────────────────────────────────────────

    fn should_skip_autosave(&self, content: &str) -> bool {
        synapse_core::domain::util::should_skip_autosave_content(content)
    }

    async fn count(&self) -> Result<usize> {
        self.memory.count().await
    }
}
