//! Memory subsystem — Phase 4.3: SurrealDB embedded backend.
//!
//! Modules:
//! - `surrealdb_adapter` — SurrealDB adapter implementing all memory ports (feature-gated)
//! - `embeddings` — pluggable embedding providers (OpenAI, custom, noop)
//! - `vector` — cosine similarity, hybrid merge, byte serialization
//! - `chunker` — markdown document chunking
//! - `response_cache` — two-tier LLM response cache (in-memory + SQLite)

pub mod chunker;
pub mod embeddings;
pub mod response_cache;
pub mod surrealdb_adapter;
pub mod vector;

pub use response_cache::ResponseCache;
pub use surrealdb_adapter::SurrealMemoryAdapter;
// NoopUnifiedMemory is defined below and re-exported here for convenience.

// Re-export domain types for convenience.
pub use synapse_domain::domain::memory::{
    AgentId, ConsolidationReport, CoreMemoryBlock, Entity, HybridSearchResult, MemoryCategory,
    MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, SearchResult, SessionId, Skill,
    SkillUpdate, TemporalFact, Visibility,
};
pub use synapse_domain::ports::memory::{
    ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
    UnifiedMemoryPort, WorkingMemoryPort,
};

use std::path::Path;
use synapse_domain::config::schema::MemoryConfig;

// ── Utility functions (backend-agnostic) ─────────────────────────

/// Legacy auto-save key used for model-authored assistant summaries.
/// These entries are treated as untrusted context and should not be re-injected.
pub fn is_assistant_autosave_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    normalized == "assistant_resp" || normalized.starts_with("assistant_resp_")
}

/// Filter known synthetic autosave noise patterns that should not be
/// persisted as user conversation memories.
pub fn should_skip_autosave_content(content: &str) -> bool {
    let normalized = content.trim();
    if normalized.is_empty() {
        return true;
    }

    let lowered = normalized.to_ascii_lowercase();
    lowered.starts_with("[cron:")
        || lowered.starts_with("[distilled_")
        || lowered.contains("distilled_index_sig:")
}

// ── Response Cache Factory ───────────────────────────────────────

/// Factory: create an optional response cache from config.
pub fn create_response_cache(config: &MemoryConfig, workspace_dir: &Path) -> Option<ResponseCache> {
    if !config.response_cache_enabled {
        return None;
    }

    match ResponseCache::new(
        workspace_dir,
        config.response_cache_ttl_minutes,
        config.response_cache_max_entries,
    ) {
        Ok(cache) => {
            tracing::info!(
                "Response cache enabled (TTL: {}min, max: {} entries)",
                config.response_cache_ttl_minutes,
                config.response_cache_max_entries
            );
            Some(cache)
        }
        Err(e) => {
            tracing::warn!("Response cache disabled due to error: {e}");
            None
        }
    }
}

// ── NoopUnifiedMemory (for tests and "none" backend) ────────────

/// A no-op implementation of all memory ports.
/// Used when memory is disabled (`backend = "none"`) and in tests.
pub struct NoopUnifiedMemory;

#[async_trait::async_trait]
impl synapse_domain::ports::memory::WorkingMemoryPort for NoopUnifiedMemory {
    async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
        Ok(vec![])
    }
    async fn update_core_block(&self, _: &AgentId, _: &str, _: String) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn append_core_block(&self, _: &AgentId, _: &str, _: &str) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl synapse_domain::ports::memory::EpisodicMemoryPort for NoopUnifiedMemory {
    async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
        Ok(vec![])
    }
}

#[async_trait::async_trait]
impl synapse_domain::ports::memory::SemanticMemoryPort for NoopUnifiedMemory {
    async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
        Ok(None)
    }
    async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
        Ok(vec![])
    }
    async fn traverse(
        &self,
        _: &MemoryId,
        _: usize,
    ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
        Ok(vec![])
    }
    async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
        Ok(vec![])
    }
}

#[async_trait::async_trait]
impl synapse_domain::ports::memory::SkillMemoryPort for NoopUnifiedMemory {
    async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
        Ok(vec![])
    }
    async fn update_skill(&self, _: &MemoryId, _: SkillUpdate) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn get_skill(&self, _: &str) -> Result<Option<Skill>, MemoryError> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl synapse_domain::ports::memory::ReflectionPort for NoopUnifiedMemory {
    async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn get_relevant_reflections(
        &self,
        _: &MemoryQuery,
    ) -> Result<Vec<Reflection>, MemoryError> {
        Ok(vec![])
    }
    async fn get_failure_patterns(
        &self,
        _: &AgentId,
        _: usize,
    ) -> Result<Vec<Reflection>, MemoryError> {
        Ok(vec![])
    }
}

#[async_trait::async_trait]
impl synapse_domain::ports::memory::ConsolidationPort for NoopUnifiedMemory {
    async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
        Ok(ConsolidationReport::default())
    }
    async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
        Ok(0)
    }
    async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
        Ok(0)
    }
}

#[async_trait::async_trait]
impl synapse_domain::ports::memory::UnifiedMemoryPort for NoopUnifiedMemory {
    async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
        Ok(HybridSearchResult::default())
    }
    async fn embed(&self, _: &str) -> Result<Vec<f32>, MemoryError> {
        Ok(vec![])
    }
    async fn store(
        &self,
        _: &str,
        _: &str,
        _: &MemoryCategory,
        _: Option<&str>,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn recall(
        &self,
        _: &str,
        _: usize,
        _: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn forget(&self, _: &str) -> Result<bool, MemoryError> {
        Ok(false)
    }
    fn should_skip_autosave(&self, _: &str) -> bool {
        false
    }
    async fn count(&self) -> Result<usize, MemoryError> {
        Ok(0)
    }
    fn name(&self) -> &str {
        "noop"
    }
    async fn health_check(&self) -> bool {
        true
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_autosave_key_detection_matches_legacy_patterns() {
        assert!(is_assistant_autosave_key("assistant_resp"));
        assert!(is_assistant_autosave_key("assistant_resp_1234"));
        assert!(is_assistant_autosave_key("ASSISTANT_RESP_abcd"));
        assert!(!is_assistant_autosave_key("assistant_response"));
        assert!(!is_assistant_autosave_key("user_msg_1234"));
    }

    #[test]
    fn autosave_content_filter_drops_cron_and_distilled_noise() {
        assert!(should_skip_autosave_content("[cron:auto] patrol check"));
        assert!(should_skip_autosave_content(
            "[DISTILLED_MEMORY_CHUNK 1/2] DISTILLED_INDEX_SIG:abc123"
        ));
        assert!(!should_skip_autosave_content(
            "User prefers concise answers."
        ));
    }
}
