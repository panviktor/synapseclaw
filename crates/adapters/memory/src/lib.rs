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
use std::sync::Arc;
use synapse_domain::config::schema::MemoryConfig;

// ── Memory Factory ───────────────────────────────────────────────

/// Create the memory backend from config.
///
/// Returns `Arc<dyn UnifiedMemoryPort>` — either SurrealDB or Noop.
/// This is async because SurrealDB initialization requires await.
pub async fn create_memory(
    config: &MemoryConfig,
    workspace_dir: &Path,
    agent_id: &str,
    api_key: Option<&str>,
) -> anyhow::Result<Arc<dyn UnifiedMemoryPort>> {
    // If backend is "none" or memory is explicitly disabled, use noop.
    if config.backend == "none" {
        tracing::info!("Memory backend: none (disabled)");
        return Ok(Arc::new(NoopUnifiedMemory));
    }

    // Create embedding provider
    let embedder = create_embedding_provider(config, api_key);

    // SurrealDB data directory: workspace_dir/memory/brain.surreal
    let data_dir = workspace_dir.join("memory").join("brain.surreal");
    if let Some(parent) = data_dir.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    let data_dir_str = data_dir.to_string_lossy().to_string();

    match SurrealMemoryAdapter::new(&data_dir_str, embedder, agent_id.to_string()).await {
        Ok(adapter) => {
            tracing::info!("Memory backend: surrealdb ({})", data_dir_str);
            Ok(Arc::new(adapter))
        }
        Err(e) => {
            tracing::error!("SurrealDB init failed: {e}, falling back to noop");
            Ok(Arc::new(NoopUnifiedMemory))
        }
    }
}

/// Embedding cache size (entries). Avoids redundant API calls.
const EMBEDDING_CACHE_SIZE: usize = 10_000;

/// Create the embedding provider from config, wrapped in LRU cache.
fn create_embedding_provider(
    config: &MemoryConfig,
    api_key: Option<&str>,
) -> Arc<dyn embeddings::EmbeddingProvider> {
    if config.embedding_provider == "none" || config.embedding_provider.is_empty() {
        return Arc::new(embeddings::NoopEmbedding);
    }

    let provider_name = config.embedding_provider.as_str();

    // llama.cpp: no API key needed
    if provider_name == "llama.cpp" || provider_name.starts_with("llama.cpp:") {
        let url = provider_name
            .strip_prefix("llama.cpp:")
            .unwrap_or("http://127.0.0.1:8081");
        let inner = Box::new(embeddings::LlamaCppEmbedding::new(
            url,
            &config.embedding_model,
            config.embedding_dimensions,
        ));
        return Arc::new(embeddings::CachedEmbeddingProvider::new(
            inner,
            EMBEDDING_CACHE_SIZE,
        ));
    }

    // Resolve API key: provider-specific env var > caller-supplied key
    let resolved_key = embedding_provider_env_key(provider_name)
        .and_then(|var| std::env::var(var).ok())
        .or_else(|| api_key.map(String::from));

    if let Some(key) = resolved_key {
        let base_url = if provider_name.starts_with("custom:") {
            provider_name.trim_start_matches("custom:").to_string()
        } else {
            embeddings::default_base_url_for_provider(provider_name)
        };

        let inner = Box::new(embeddings::OpenAiEmbedding::new(
            &base_url,
            &key,
            &config.embedding_model,
            config.embedding_dimensions,
        ));
        Arc::new(embeddings::CachedEmbeddingProvider::new(
            inner,
            EMBEDDING_CACHE_SIZE,
        ))
    } else {
        tracing::warn!(
            "No API key for embedding provider '{}', using noop embeddings",
            config.embedding_provider
        );
        Arc::new(embeddings::NoopEmbedding)
    }
}

/// Look up the provider-specific environment variable for embedding API keys.
fn embedding_provider_env_key(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "openai" => Some("OPENAI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "cohere" => Some("COHERE_API_KEY"),
        "voyageai" | "voyage" => Some("VOYAGE_API_KEY"),
        "gemini" | "google" => Some("GEMINI_API_KEY"),
        _ => None,
    }
}

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
    async fn get(&self, _: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        Ok(None)
    }
    async fn list(
        &self,
        _: Option<&MemoryCategory>,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
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
    async fn reflect_on_turn(
        &self,
        _user_message: &str,
        _assistant_response: &str,
        _tools_used: &[String],
    ) -> Result<(), MemoryError> {
        Ok(())
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
