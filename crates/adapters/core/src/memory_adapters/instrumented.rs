//! Instrumented memory wrapper — adds latency tracking to all memory operations.
//!
//! Wraps any `UnifiedMemoryPort` and logs operation duration via tracing.
//! Slow operations (>50ms) are logged at INFO level; normal at DEBUG.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use synapse_domain::domain::memory::*;
use synapse_domain::ports::memory::*;

/// Wraps UnifiedMemoryPort with latency instrumentation.
pub struct InstrumentedMemory {
    inner: Arc<dyn UnifiedMemoryPort>,
}

impl InstrumentedMemory {
    pub fn new(inner: Arc<dyn UnifiedMemoryPort>) -> Self {
        Self { inner }
    }
}

fn log_op(op: &str, start: Instant, count: usize) {
    let ms = start.elapsed().as_millis() as u64;
    if ms > 50 {
        tracing::info!(op, latency_ms = ms, results = count, "memory.slow_op");
    } else {
        tracing::debug!(op, latency_ms = ms, results = count, "memory.op");
    }
}

#[async_trait]
impl WorkingMemoryPort for InstrumentedMemory {
    async fn get_core_blocks(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_core_blocks(agent_id).await;
        log_op(
            "get_core_blocks",
            t,
            r.as_ref().map(|v| v.len()).unwrap_or(0),
        );
        r
    }
    async fn update_core_block(&self, a: &AgentId, l: &str, c: String) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self.inner.update_core_block(a, l, c).await;
        log_op("update_core_block", t, 0);
        r
    }
    async fn append_core_block(&self, a: &AgentId, l: &str, text: &str) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self.inner.append_core_block(a, l, text).await;
        log_op("append_core_block", t, 0);
        r
    }
}

#[async_trait]
impl EpisodicMemoryPort for InstrumentedMemory {
    async fn store_episode(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError> {
        let t = Instant::now();
        let r = self.inner.store_episode(entry).await;
        log_op("store_episode", t, 0);
        r
    }
    async fn get_recent(&self, a: &AgentId, limit: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_recent(a, limit).await;
        log_op("get_recent", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
    }
    async fn get_session(&self, sid: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_session(sid).await;
        log_op("get_session", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
    }
    async fn search_episodes(&self, q: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.search_episodes(q).await;
        log_op(
            "search_episodes",
            t,
            r.as_ref().map(|v| v.len()).unwrap_or(0),
        );
        r
    }
}

#[async_trait]
impl SemanticMemoryPort for InstrumentedMemory {
    async fn upsert_entity(&self, e: Entity) -> Result<MemoryId, MemoryError> {
        let t = Instant::now();
        let r = self.inner.upsert_entity(e).await;
        log_op("upsert_entity", t, 0);
        r
    }
    async fn find_entity(&self, name: &str) -> Result<Option<Entity>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.find_entity(name).await;
        log_op(
            "find_entity",
            t,
            r.as_ref().map(|o| usize::from(o.is_some())).unwrap_or(0),
        );
        r
    }
    async fn add_fact(&self, f: TemporalFact) -> Result<MemoryId, MemoryError> {
        let t = Instant::now();
        let r = self.inner.add_fact(f).await;
        log_op("add_fact", t, 0);
        r
    }
    async fn invalidate_fact(&self, id: &MemoryId) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self.inner.invalidate_fact(id).await;
        log_op("invalidate_fact", t, 0);
        r
    }
    async fn get_current_facts(&self, id: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_current_facts(id).await;
        log_op(
            "get_current_facts",
            t,
            r.as_ref().map(|v| v.len()).unwrap_or(0),
        );
        r
    }
    async fn traverse(
        &self,
        id: &MemoryId,
        hops: usize,
    ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.traverse(id, hops).await;
        log_op("traverse", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
    }
    async fn search_entities(&self, q: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.search_entities(q).await;
        log_op(
            "search_entities",
            t,
            r.as_ref().map(|v| v.len()).unwrap_or(0),
        );
        r
    }
}

#[async_trait]
impl SkillMemoryPort for InstrumentedMemory {
    async fn store_skill(&self, s: Skill) -> Result<MemoryId, MemoryError> {
        let t = Instant::now();
        let r = self.inner.store_skill(s).await;
        log_op("store_skill", t, 0);
        r
    }
    async fn find_skills(&self, q: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.find_skills(q).await;
        log_op("find_skills", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
    }
    async fn update_skill(&self, id: &MemoryId, u: SkillUpdate) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self.inner.update_skill(id, u).await;
        log_op("update_skill", t, 0);
        r
    }
    async fn get_skill(&self, name: &str) -> Result<Option<Skill>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_skill(name).await;
        log_op(
            "get_skill",
            t,
            r.as_ref().map(|o| usize::from(o.is_some())).unwrap_or(0),
        );
        r
    }
}

#[async_trait]
impl ReflectionPort for InstrumentedMemory {
    async fn store_reflection(&self, refl: Reflection) -> Result<MemoryId, MemoryError> {
        let t = Instant::now();
        let r = self.inner.store_reflection(refl).await;
        log_op("store_reflection", t, 0);
        r
    }
    async fn get_relevant_reflections(
        &self,
        q: &MemoryQuery,
    ) -> Result<Vec<Reflection>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_relevant_reflections(q).await;
        log_op(
            "get_relevant_reflections",
            t,
            r.as_ref().map(|v| v.len()).unwrap_or(0),
        );
        r
    }
    async fn get_failure_patterns(
        &self,
        a: &AgentId,
        limit: usize,
    ) -> Result<Vec<Reflection>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get_failure_patterns(a, limit).await;
        log_op(
            "get_failure_patterns",
            t,
            r.as_ref().map(|v| v.len()).unwrap_or(0),
        );
        r
    }
}

#[async_trait]
impl ConsolidationPort for InstrumentedMemory {
    async fn run_consolidation(&self, a: &AgentId) -> Result<ConsolidationReport, MemoryError> {
        let t = Instant::now();
        let r = self.inner.run_consolidation(a).await;
        log_op("run_consolidation", t, 0);
        r
    }
    async fn recalculate_importance(&self, a: &AgentId) -> Result<u32, MemoryError> {
        let t = Instant::now();
        let r = self.inner.recalculate_importance(a).await;
        log_op(
            "recalculate_importance",
            t,
            *r.as_ref().unwrap_or(&0) as usize,
        );
        r
    }
    async fn gc_low_importance(&self, threshold: f32, max_age: u32) -> Result<u32, MemoryError> {
        let t = Instant::now();
        let r = self.inner.gc_low_importance(threshold, max_age).await;
        log_op("gc_low_importance", t, *r.as_ref().unwrap_or(&0) as usize);
        r
    }
}

#[async_trait]
impl UnifiedMemoryPort for InstrumentedMemory {
    async fn hybrid_search(&self, q: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
        let t = Instant::now();
        let r = self.inner.hybrid_search(q).await;
        log_op(
            "hybrid_search",
            t,
            r.as_ref().map(|h| h.episodes.len()).unwrap_or(0),
        );
        r
    }
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.embed(text).await;
        log_op("embed", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
    }
    async fn store(
        &self,
        key: &str,
        content: &str,
        cat: &MemoryCategory,
        sid: Option<&str>,
    ) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self.inner.store(key, content, cat, sid).await;
        log_op("store", t, 0);
        r
    }
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        sid: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.recall(query, limit, sid).await;
        log_op("recall", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
    }
    async fn consolidate_turn(&self, user: &str, asst: &str) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self.inner.consolidate_turn(user, asst).await;
        log_op("consolidate_turn", t, 0);
        r
    }
    async fn forget(&self, key: &str) -> Result<bool, MemoryError> {
        let t = Instant::now();
        let r = self.inner.forget(key).await;
        log_op("forget", t, 0);
        r
    }
    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.get(key).await;
        log_op(
            "get",
            t,
            r.as_ref().map(|o| usize::from(o.is_some())).unwrap_or(0),
        );
        r
    }
    async fn list(
        &self,
        cat: Option<&MemoryCategory>,
        sid: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let t = Instant::now();
        let r = self.inner.list(cat, sid, limit).await;
        log_op("list", t, r.as_ref().map(|v| v.len()).unwrap_or(0));
        r
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
    async fn reflect_on_turn(
        &self,
        user_message: &str,
        assistant_response: &str,
        tools_used: &[String],
    ) -> Result<(), MemoryError> {
        let t = Instant::now();
        let r = self
            .inner
            .reflect_on_turn(user_message, assistant_response, tools_used)
            .await;
        log_op("reflect_on_turn", t, 0);
        r
    }
}
