//! Phase 4.3: Specialized memory ports.
//!
//! Seven ports for the full agent memory architecture:
//!
//! - [`WorkingMemoryPort`] — core memory blocks (MemGPT), always in prompt
//! - [`EpisodicMemoryPort`] — interaction history, session-scoped
//! - [`SemanticMemoryPort`] — knowledge graph: entities + bitemporal facts
//! - [`SkillMemoryPort`] — procedural memory: learned skills
//! - [`ReflectionPort`] — self-improvement records
//! - [`ConsolidationPort`] — background extraction & GC
//! - [`UnifiedMemoryPort`] — facade for cross-tier hybrid search + convenience ops

use crate::domain::memory::{
    AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingProfile, Entity, HybridSearchResult,
    MemoryCategory, MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, SearchResult,
    SessionId, Skill, SkillUpdate, TemporalFact, Visibility,
};
use async_trait::async_trait;

// ── Working Memory (core blocks, always in prompt) ───────────────

/// Core memory blocks that are **always** included in the agent's
/// system prompt. Agents edit them via `core_memory_update` tool.
#[async_trait]
pub trait WorkingMemoryPort: Send + Sync {
    /// Get all core memory blocks for an agent.
    async fn get_core_blocks(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<CoreMemoryBlock>, MemoryError>;

    /// Replace the content of a core memory block.
    async fn update_core_block(
        &self,
        agent_id: &AgentId,
        label: &str,
        content: String,
    ) -> Result<(), MemoryError>;

    /// Append text to a core memory block.
    async fn append_core_block(
        &self,
        agent_id: &AgentId,
        label: &str,
        text: &str,
    ) -> Result<(), MemoryError>;
}

// ── Episodic Memory (interaction history) ────────────────────────

/// Raw interaction episodes — conversation turns, tool calls, etc.
#[async_trait]
pub trait EpisodicMemoryPort: Send + Sync {
    /// Store a new episode.
    async fn store_episode(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError>;

    /// Get the N most recent episodes for an agent.
    async fn get_recent(
        &self,
        agent_id: &AgentId,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Get all episodes for a session.
    async fn get_session(&self, session_id: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Hybrid search across episodes (vector + BM25).
    async fn search_episodes(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError>;
}

// ── Semantic Memory (knowledge graph) ────────────────────────────

/// Entities and bitemporal facts forming a knowledge graph.
#[async_trait]
pub trait SemanticMemoryPort: Send + Sync {
    /// Create or update an entity (entity resolution).
    async fn upsert_entity(&self, entity: Entity) -> Result<MemoryId, MemoryError>;

    /// Find entity by name (with fuzzy matching).
    async fn find_entity(&self, name: &str) -> Result<Option<Entity>, MemoryError>;

    /// Add a fact (graph edge) with bitemporal semantics.
    async fn add_fact(&self, fact: TemporalFact) -> Result<MemoryId, MemoryError>;

    /// Invalidate a fact (set `valid_to = now`).
    async fn invalidate_fact(&self, fact_id: &MemoryId) -> Result<(), MemoryError>;

    /// Get current (non-invalidated) facts about an entity.
    async fn get_current_facts(
        &self,
        entity_id: &MemoryId,
    ) -> Result<Vec<TemporalFact>, MemoryError>;

    /// Graph traversal: entities reachable within N hops.
    async fn traverse(
        &self,
        entity_id: &MemoryId,
        hops: usize,
    ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError>;

    /// Search entities (vector + BM25).
    async fn search_entities(&self, query: &MemoryQuery) -> Result<Vec<Entity>, MemoryError>;
}

// ── Skill Memory (procedural) ────────────────────────────────────

/// Learned procedures from successful (or failed) pipeline runs.
#[async_trait]
pub trait SkillMemoryPort: Send + Sync {
    /// Store a new skill.
    async fn store_skill(&self, skill: Skill) -> Result<MemoryId, MemoryError>;

    /// Find skills relevant to a task.
    async fn find_skills(&self, query: &MemoryQuery) -> Result<Vec<Skill>, MemoryError>;

    /// Update a skill (bump counters, new content, version++).
    async fn update_skill(
        &self,
        skill_id: &MemoryId,
        update: SkillUpdate,
        agent_id: &AgentId,
    ) -> Result<(), MemoryError>;

    /// Get skill by name, scoped to agent.
    async fn get_skill(&self, name: &str, agent_id: &AgentId)
        -> Result<Option<Skill>, MemoryError>;
}

// ── Reflection (self-improvement) ────────────────────────────────

/// Post-run introspection records.
#[async_trait]
pub trait ReflectionPort: Send + Sync {
    /// Store a reflection.
    async fn store_reflection(&self, reflection: Reflection) -> Result<MemoryId, MemoryError>;

    /// Get reflections relevant to a query.
    async fn get_relevant_reflections(
        &self,
        query: &MemoryQuery,
    ) -> Result<Vec<Reflection>, MemoryError>;

    /// Get recent failure patterns for an agent.
    async fn get_failure_patterns(
        &self,
        agent_id: &AgentId,
        limit: usize,
    ) -> Result<Vec<Reflection>, MemoryError>;
}

// ── Consolidation (background processing) ────────────────────────

/// Background memory maintenance: extraction, decay, GC.
#[async_trait]
pub trait ConsolidationPort: Send + Sync {
    /// Run a full consolidation cycle.
    async fn run_consolidation(
        &self,
        agent_id: &AgentId,
    ) -> Result<ConsolidationReport, MemoryError>;

    /// Recalculate importance scores across all entries.
    async fn recalculate_importance(&self, agent_id: &AgentId) -> Result<u32, MemoryError>;

    /// Garbage-collect entries below importance threshold and older than max_age_days.
    async fn gc_low_importance(
        &self,
        threshold: f32,
        max_age_days: u32,
    ) -> Result<u32, MemoryError>;
}

// ── Unified Memory (facade) ──────────────────────────────────────

/// Facade composing all memory subsystems.
///
/// Agents and services use this as the single memory interface.
/// Tool implementations hold `Arc<dyn UnifiedMemoryPort>`.
#[async_trait]
pub trait UnifiedMemoryPort:
    WorkingMemoryPort
    + EpisodicMemoryPort
    + SemanticMemoryPort
    + SkillMemoryPort
    + ReflectionPort
    + ConsolidationPort
    + Send
    + Sync
{
    /// Cross-tier hybrid search with RRF fusion.
    async fn hybrid_search(&self, query: &MemoryQuery) -> Result<HybridSearchResult, MemoryError>;

    /// Generate an embedding vector for text.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError>;

    /// Generate a query embedding, applying model-specific calibration if needed.
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embed(text).await
    }

    /// Generate a document embedding, applying model-specific calibration if needed.
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embed(text).await
    }

    /// Retrieval calibration metadata for the active embedding model.
    fn embedding_profile(&self) -> EmbeddingProfile {
        EmbeddingProfile::default()
    }

    // ── Convenience methods for inbound message flow ─────────────

    /// Quick store: persist a key-value entry (used by autosave, consolidation).
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<(), MemoryError>;

    /// Quick recall: keyword/vector search for prompt injection.
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Fire-and-forget consolidation after a conversation turn.
    async fn consolidate_turn(
        &self,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<(), MemoryError>;

    /// Forget (delete) a memory entry by key, scoped to agent. Returns true if found.
    async fn forget(&self, key: &str, agent_id: &AgentId) -> Result<bool, MemoryError>;

    /// Get a single memory entry by exact key, scoped to agent.
    async fn get(&self, key: &str, agent_id: &AgentId) -> Result<Option<MemoryEntry>, MemoryError>;

    /// List memory entries, optionally filtered by category and/or session.
    async fn list(
        &self,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Find facts similar to the given embedding vector. Returns (fact, similarity) pairs.
    /// Used by AUDN dedup cycle to prevent duplicate facts in memory.
    async fn find_similar_facts(
        &self,
        _embedding: &[f32],
        _limit: usize,
    ) -> Result<Vec<(TemporalFact, f32)>, MemoryError> {
        Ok(vec![]) // default no-op
    }

    /// Check if content should be skipped for auto-save (noise filter).
    fn should_skip_autosave(&self, content: &str) -> bool;

    /// Total entry count across all subsystems.
    async fn count(&self) -> Result<usize, MemoryError>;

    /// Backend name.
    fn name(&self) -> &str;

    /// Health check.
    async fn health_check(&self) -> bool;

    /// Promote the visibility of a memory entry (Private → SharedWith/Global).
    /// Only the owning agent can promote. Use `memory_sharing::validate_promotion`
    /// to check policy before calling this.
    async fn promote_visibility(
        &self,
        _entry_id: &MemoryId,
        _visibility: &Visibility,
        _shared_with: &[AgentId],
        _agent_id: &AgentId,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Storage(
            "promote_visibility not supported".into(),
        ))
    }

    /// Load learning signal patterns from storage.
    /// Returns empty vec if not supported (patterns fall back to built-in defaults).
    async fn list_signal_patterns(
        &self,
    ) -> Result<Vec<crate::application::services::learning_signals::SignalPattern>, MemoryError>
    {
        Ok(vec![])
    }

    /// Reflect on a conversation turn for skill learning. Fire-and-forget.
    /// `tools_used`: list of tool names called during this turn.
    async fn reflect_on_turn(
        &self,
        _user_message: &str,
        _assistant_response: &str,
        _tools_used: &[String],
    ) -> Result<(), MemoryError> {
        Ok(()) // default no-op
    }
}
