//! Phase 4.3: ConsolidatingMemory wrapper.
//!
//! Wraps a `dyn UnifiedMemoryPort` and adds LLM-driven consolidation
//! (which requires a Provider — not available in the memory crate).

use std::sync::Arc;

use async_trait::async_trait;
use synapse_domain::domain::memory::*;
use synapse_domain::ports::memory::*;
use synapse_providers::traits::Provider;

/// Wraps UnifiedMemoryPort, adding LLM consolidation via Provider.
pub struct ConsolidatingMemory {
    inner: Arc<dyn UnifiedMemoryPort>,
    provider: Arc<dyn Provider>,
    model: String,
    agent_id: String,
    ipc_client: Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
}

impl ConsolidatingMemory {
    pub fn new(
        inner: Arc<dyn UnifiedMemoryPort>,
        provider: Arc<dyn Provider>,
        model: String,
        agent_id: String,
        ipc_client: Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
    ) -> Self {
        Self {
            inner,
            provider,
            model,
            agent_id,
            ipc_client,
        }
    }
}

// Delegate all port methods to inner, except consolidate_turn.

#[async_trait]
impl WorkingMemoryPort for ConsolidatingMemory {
    async fn get_core_blocks(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
        self.inner.get_core_blocks(agent_id).await
    }
    async fn update_core_block(
        &self,
        agent_id: &AgentId,
        label: &str,
        content: String,
    ) -> Result<(), MemoryError> {
        self.inner.update_core_block(agent_id, label, content).await
    }
    async fn append_core_block(
        &self,
        agent_id: &AgentId,
        label: &str,
        text: &str,
    ) -> Result<(), MemoryError> {
        self.inner.append_core_block(agent_id, label, text).await
    }
}

#[async_trait]
impl EpisodicMemoryPort for ConsolidatingMemory {
    async fn store_episode(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError> {
        self.inner.store_episode(entry).await
    }
    async fn get_recent(
        &self,
        agent_id: &AgentId,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner.get_recent(agent_id, limit).await
    }
    async fn get_session(&self, session_id: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner.get_session(session_id).await
    }
    async fn search_episodes(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
        self.inner.search_episodes(query).await
    }
}

#[async_trait]
impl SemanticMemoryPort for ConsolidatingMemory {
    async fn upsert_entity(&self, entity: Entity) -> Result<MemoryId, MemoryError> {
        self.inner.upsert_entity(entity).await
    }
    async fn find_entity(&self, name: &str) -> Result<Option<Entity>, MemoryError> {
        self.inner.find_entity(name).await
    }
    async fn add_fact(&self, fact: TemporalFact) -> Result<MemoryId, MemoryError> {
        self.inner.add_fact(fact).await
    }
    async fn invalidate_fact(&self, fact_id: &MemoryId) -> Result<(), MemoryError> {
        self.inner.invalidate_fact(fact_id).await
    }
    async fn get_current_facts(
        &self,
        entity_id: &MemoryId,
    ) -> Result<Vec<TemporalFact>, MemoryError> {
        self.inner.get_current_facts(entity_id).await
    }
    async fn traverse(
        &self,
        entity_id: &MemoryId,
        hops: usize,
    ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
        self.inner.traverse(entity_id, hops).await
    }
    async fn search_entities(&self, query: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
        self.inner.search_entities(query).await
    }
}

#[async_trait]
impl SkillMemoryPort for ConsolidatingMemory {
    async fn store_skill(&self, skill: Skill) -> Result<MemoryId, MemoryError> {
        self.inner.store_skill(skill).await
    }
    async fn find_skills(&self, query: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
        self.inner.find_skills(query).await
    }
    async fn update_skill(
        &self,
        skill_id: &MemoryId,
        update: SkillUpdate,
    ) -> Result<(), MemoryError> {
        self.inner.update_skill(skill_id, update).await
    }
    async fn get_skill(&self, name: &str) -> Result<Option<Skill>, MemoryError> {
        self.inner.get_skill(name).await
    }
}

#[async_trait]
impl ReflectionPort for ConsolidatingMemory {
    async fn store_reflection(&self, reflection: Reflection) -> Result<MemoryId, MemoryError> {
        self.inner.store_reflection(reflection).await
    }
    async fn get_relevant_reflections(
        &self,
        query: &MemoryQuery,
    ) -> Result<Vec<Reflection>, MemoryError> {
        self.inner.get_relevant_reflections(query).await
    }
    async fn get_failure_patterns(
        &self,
        agent_id: &AgentId,
        limit: usize,
    ) -> Result<Vec<Reflection>, MemoryError> {
        self.inner.get_failure_patterns(agent_id, limit).await
    }
}

#[async_trait]
impl ConsolidationPort for ConsolidatingMemory {
    async fn run_consolidation(
        &self,
        agent_id: &AgentId,
    ) -> Result<ConsolidationReport, MemoryError> {
        self.inner.run_consolidation(agent_id).await
    }
    async fn recalculate_importance(&self, agent_id: &AgentId) -> Result<u32, MemoryError> {
        self.inner.recalculate_importance(agent_id).await
    }
    async fn gc_low_importance(
        &self,
        threshold: f32,
        max_age_days: u32,
    ) -> Result<u32, MemoryError> {
        self.inner.gc_low_importance(threshold, max_age_days).await
    }
}

#[async_trait]
impl UnifiedMemoryPort for ConsolidatingMemory {
    async fn hybrid_search(&self, query: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
        self.inner.hybrid_search(query).await
    }
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.inner.embed(text).await
    }
    async fn store(
        &self,
        key: &str,
        content: &str,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        self.inner.store(key, content, category, session_id).await
    }
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner.recall(query, limit, session_id).await
    }
    async fn forget(&self, key: &str) -> Result<bool, MemoryError> {
        self.inner.forget(key).await
    }
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        self.inner.get(key).await
    }
    async fn list(
        &self,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner.list(category, session_id, limit).await
    }
    fn should_skip_autosave(&self, content: &str) -> bool {
        self.inner.should_skip_autosave(content)
    }
    async fn count(&self) -> Result<usize, MemoryError> {
        self.inner.count().await
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }

    /// Real LLM-driven consolidation — delegates to consolidation module.
    async fn consolidate_turn(
        &self,
        user_message: &str,
        assistant_response: &str,
    ) -> Result<(), MemoryError> {
        let outcome = super::consolidation::consolidate_turn(
            self.provider.as_ref(),
            &self.model,
            self.inner.as_ref(),
            user_message,
            assistant_response,
            &self.agent_id,
        )
        .await
        .map_err(|e| MemoryError::Storage(format!("Consolidation failed: {e}")))?;

        // IPC broadcast: notify fleet about discovered entities.
        if let Some(ref ipc) = self.ipc_client {
            if outcome.entities_extracted > 0 {
                let event = synapse_domain::domain::memory::MemoryEvent {
                    event_type: synapse_domain::domain::memory::MemoryEventType::EntityDiscovered,
                    source_agent: self.agent_id.clone(),
                    entry_id: String::new(),
                    summary: format!("{} entities extracted", outcome.entities_extracted),
                };
                let payload = serde_json::to_string(&event).unwrap_or_default();
                let body = serde_json::json!({
                    "to": "*",
                    "msg_type": "memory_event",
                    "payload": payload,
                });
                if let Err(e) = ipc.send_message(&body).await {
                    tracing::warn!("MemoryEvent IPC broadcast failed: {e}");
                }
            }
        }

        Ok(())
    }

    /// Skill reflection — analyze tool usage patterns.
    async fn reflect_on_turn(
        &self,
        user_message: &str,
        _assistant_response: &str,
        tools_used: &[String],
    ) -> Result<(), MemoryError> {
        if tools_used.is_empty() {
            return Ok(());
        }
        let summary = super::skill_learner::PipelineRunSummary {
            run_id: uuid::Uuid::new_v4().to_string(),
            task: user_message.chars().take(200).collect(),
            outcome: synapse_domain::domain::memory::ReflectionOutcome::Success,
            steps: tools_used.to_vec(),
            errors: vec![],
        };
        super::skill_learner::reflect_on_run(
            self.provider.as_ref(),
            &self.model,
            self.inner.as_ref(),
            &self.agent_id,
            &summary,
        )
        .await
        .map_err(|e| MemoryError::Storage(format!("Skill reflection: {e}")))?;
        Ok(())
    }
}
