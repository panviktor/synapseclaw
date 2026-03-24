//! Adapter: wraps existing `dyn Memory` as MemoryPort.

use crate::fork_core::ports::memory::{MemoryEntry, MemoryPort};
use crate::memory::Memory;
use crate::providers::Provider;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub struct MemoryAdapter {
    memory: Arc<dyn Memory>,
    provider: Arc<dyn Provider>,
    model: String,
}

impl MemoryAdapter {
    pub fn new(memory: Arc<dyn Memory>, provider: Arc<dyn Provider>, model: String) -> Self {
        Self {
            memory,
            provider,
            model,
        }
    }
}

#[async_trait]
impl MemoryPort for MemoryAdapter {
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>> {
        let entries = self.memory.recall(query, limit, session_id).await?;
        Ok(entries
            .into_iter()
            .map(|e| MemoryEntry {
                key: e.key,
                content: e.content,
                score: e.score,
            })
            .collect())
    }

    async fn store(
        &self,
        key: &str,
        content: &str,
        session_id: Option<&str>,
    ) -> Result<()> {
        self.memory
            .store(
                key,
                content,
                crate::memory::MemoryCategory::Conversation,
                session_id,
            )
            .await
    }

    async fn consolidate_turn(
        &self,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<()> {
        crate::memory::consolidation::consolidate_turn(
            self.provider.as_ref(),
            &self.model,
            self.memory.as_ref(),
            user_message,
            assistant_response,
        )
        .await
    }

    fn should_skip_autosave(&self, content: &str) -> bool {
        crate::memory::should_skip_autosave_content(content)
    }
}
