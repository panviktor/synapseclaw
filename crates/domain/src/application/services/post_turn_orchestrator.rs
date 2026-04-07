//! Post-turn learning orchestrator — single source of truth for learning policy.
//!
//! Both web and channel paths call `execute_post_turn_learning()` instead of
//! implementing their own spawn/decide/mutate logic. This eliminates policy
//! divergence between transport adapters.

use crate::application::services::learning_candidate_service::{self, LearningCandidate};
use crate::application::services::learning_events::LearningEvent;
use crate::application::services::learning_evidence_service::{self, LearningEvidenceEnvelope};
use crate::application::services::learning_quality_service::{
    self, LearningCandidateAssessment,
};
use crate::application::services::recipe_evolution_service;
use crate::application::services::learning_signals::{self, LearningSignal};
use crate::application::services::memory_mutation as mutation;
use crate::application::services::user_profile_service;
use crate::domain::memory::MemoryCategory;
use crate::domain::memory_mutation::{MutationCandidate, MutationSource, MutationThresholds};
use crate::domain::tool_fact::TypedToolFact;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::run_recipe_store::RunRecipeStorePort;
use crate::ports::user_profile_store::UserProfileStorePort;
use std::sync::Arc;

// ── Gate constants ───────────────────────────────────────────────

/// Minimum user message length (chars) for background consolidation.
const CONSOLIDATE_MIN_CHARS: usize = 20;

/// Minimum user message length (chars) for reflection.
const REFLECT_MIN_USER_CHARS: usize = 30;

/// Minimum response length (bytes) for reflection.
const REFLECT_MIN_RESPONSE_LEN: usize = 200;

// ── Input / Output ───────────────────────────────────────────────

/// Everything the orchestrator needs to decide and execute post-turn learning.
pub struct PostTurnInput {
    pub agent_id: String,
    pub user_message: String,
    pub assistant_response: String,
    pub tools_used: Vec<String>,
    pub tool_facts: Vec<TypedToolFact>,
    pub run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    pub user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
    pub user_profile_key: Option<String>,
    pub auto_save_enabled: bool,
    /// Optional SSE event sender for publishing reports to UI.
    /// Both web and channels should pass this if available.
    pub event_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
}

/// What the orchestrator did — returned to the transport adapter for logging/UI.
#[derive(Debug)]
pub struct PostTurnReport {
    /// Detected learning signal from user message.
    pub signal: LearningSignal,
    /// Learning event from explicit AUDN mutation (if any).
    pub explicit_mutation: Option<LearningEvent>,
    /// Whether background consolidation was started.
    pub consolidation_started: bool,
    /// Whether skill reflection was started.
    pub reflection_started: bool,
    /// Cheap typed evidence from this turn.
    pub learning_evidence: LearningEvidenceEnvelope,
    /// Structured low-cost candidates for downstream learning.
    pub learning_candidates: Vec<LearningCandidate>,
    /// Quality-gated assessments for those candidates.
    pub learning_assessments: Vec<LearningCandidateAssessment>,
    /// Applied mutation events derived from low-cost learning candidates.
    pub candidate_mutations: Vec<LearningEvent>,
    /// Count of run recipes upserted from low-cost candidates.
    pub run_recipes_upserted: usize,
    /// Whether a structured user profile patch was applied.
    pub user_profile_updated: bool,
}

// ── Orchestrator ─────────────────────────────────────────────────

/// Execute all post-turn learning in one place.
///
/// This is the **single source of truth** for learning policy.
/// Web and channels are pure transport — they call this and log the report.
pub async fn execute_post_turn_learning(
    mem: &dyn UnifiedMemoryPort,
    input: PostTurnInput,
) -> PostTurnReport {
    // Load signal patterns from memory port — unified for all transports.
    let patterns = mem.list_signal_patterns().await.unwrap_or_default();
    let signal = learning_signals::classify_signal_with_patterns(&input.user_message, &patterns);
    let user_chars = input.user_message.chars().count();
    let learning_evidence = learning_evidence_service::build_learning_evidence(&input.tool_facts);
    let learning_candidates = learning_candidate_service::build_learning_candidates(
        &input.user_message,
        &input.assistant_response,
        &input.tool_facts,
        &learning_evidence,
    );
    let existing_recipes = input
        .run_recipe_store
        .as_ref()
        .map(|store| store.list(&input.agent_id))
        .unwrap_or_default();
    let learning_assessments = learning_quality_service::assess_learning_candidates(
        &learning_candidates,
        &learning_evidence,
        &existing_recipes,
    );
    let mutation_candidates =
        learning_candidate_service::build_mutation_candidates_from_assessments(
            &learning_assessments,
        );

    let mut report = PostTurnReport {
        signal: signal.clone(),
        explicit_mutation: None,
        consolidation_started: false,
        reflection_started: false,
        learning_evidence: learning_evidence.clone(),
        learning_candidates: learning_candidates.clone(),
        learning_assessments: learning_assessments.clone(),
        candidate_mutations: Vec::new(),
        run_recipes_upserted: 0,
        user_profile_updated: false,
    };

    // ── 1. Explicit hot-path: direct AUDN mutation ──
    if signal.is_explicit() {
        let candidate = MutationCandidate {
            category: MemoryCategory::Core,
            text: input.user_message.clone(),
            confidence: signal.confidence(),
            source: MutationSource::ExplicitUser,
        };
        let decision = mutation::evaluate_candidate(
            mem,
            candidate,
            &input.agent_id,
            &MutationThresholds::default(),
        )
        .await;
        match mutation::apply_decision_with_event(mem, &decision, &input.agent_id).await {
            Ok(event) => {
                tracing::debug!(
                    target: "post_turn",
                    kind = ?event.kind,
                    agent_id = %input.agent_id,
                    "Explicit learning event"
                );
                report.explicit_mutation = Some(event);
            }
            Err(e) => {
                tracing::warn!(
                    target: "post_turn",
                    error = %e,
                    "Explicit mutation failed"
                );
            }
        }
    }

    // ── 1b. Cheap typed candidate mutation path ──
    if !signal.is_explicit() && input.auto_save_enabled && !mutation_candidates.is_empty() {
        let decisions = mutation::evaluate_candidates(
            mem,
            mutation_candidates,
            &input.agent_id,
            &MutationThresholds::default(),
        )
        .await;
        for decision in decisions {
            match mutation::apply_decision_with_event(mem, &decision, &input.agent_id).await {
                Ok(event) => report.candidate_mutations.push(event),
                Err(e) => {
                    tracing::warn!(
                        target: "post_turn",
                        error = %e,
                        "Typed candidate mutation failed"
                    );
                }
            }
        }
    }

    // ── 1c. Cheap procedural candidate path ──
    if !signal.is_explicit() && input.auto_save_enabled {
        if let Some(store) = input.run_recipe_store.as_ref() {
            for assessment in &learning_assessments {
                if !assessment.accepted {
                    continue;
                }
                let LearningCandidate::RunRecipe(recipe_candidate) = &assessment.candidate else {
                    continue;
                };
                let updated_at = chrono::Utc::now().timestamp().max(0) as u64;
                let existing = store.get(&input.agent_id, &recipe_candidate.task_family_hint);
                let recipe = if assessment.reason == "merge_existing_recipe" {
                    existing
                        .as_ref()
                        .map(|existing_recipe| {
                            recipe_evolution_service::merge_existing_recipe(
                                existing_recipe,
                                recipe_candidate,
                                updated_at,
                            )
                        })
                        .unwrap_or_else(|| {
                            recipe_evolution_service::build_new_recipe(
                                &input.agent_id,
                                recipe_candidate,
                                updated_at,
                            )
                        })
                } else {
                    recipe_evolution_service::build_new_recipe(
                        &input.agent_id,
                        recipe_candidate,
                        updated_at,
                    )
                };
                match store.upsert(recipe) {
                    Ok(()) => report.run_recipes_upserted += 1,
                    Err(e) => {
                        tracing::warn!(
                            target: "post_turn",
                            error = %e,
                            "Run recipe upsert failed"
                        );
                    }
                }
            }
        }
    }

    // ── 1d. Cheap structured profile path ──
    if !signal.is_explicit() && input.auto_save_enabled {
        if let (Some(store), Some(user_key)) = (
            input.user_profile_store.as_ref(),
            input.user_profile_key.as_deref(),
        ) {
            let current = store.load(user_key);
            let patch = learning_candidate_service::build_user_profile_patch(
                &learning_candidates,
                current.as_ref(),
            );
            if !patch.is_noop() {
                let updated = user_profile_service::apply_patch(current.clone(), &patch);
                if updated != current {
                    let result = if let Some(profile) = updated {
                        store.upsert(user_key, profile)
                    } else {
                        store.remove(user_key).map(|_| ())
                    };
                    match result {
                        Ok(()) => report.user_profile_updated = true,
                        Err(e) => {
                            tracing::warn!(
                                target: "post_turn",
                                error = %e,
                                "User profile auto-update failed"
                            );
                        }
                    }
                }
            }
        }
    }

    // ── 2. Background consolidation (only for non-explicit turns) ──
    let should_consolidate = !signal.is_explicit()
        && input.auto_save_enabled
        && (user_chars >= CONSOLIDATE_MIN_CHARS || learning_evidence.has_actionable_evidence());

    if should_consolidate {
        if let Err(e) = mem
            .consolidate_turn(&input.user_message, &input.assistant_response)
            .await
        {
            tracing::warn!(target: "post_turn", error = %e, "Consolidation failed");
        }
        report.consolidation_started = true;
    }

    // ── 3. Skill reflection ──
    let resp_lower = input.assistant_response.to_lowercase();
    let has_errors = resp_lower.contains("error") || resp_lower.contains("failed");
    let should_reflect = input.assistant_response.len() > REFLECT_MIN_RESPONSE_LEN
        && user_chars >= REFLECT_MIN_USER_CHARS
        && (!input.tools_used.is_empty()
            || learning_evidence.has_actionable_evidence()
            || has_errors);

    if should_reflect {
        if let Err(e) = mem
            .reflect_on_turn(
                &input.user_message,
                &input.assistant_response,
                &input.tools_used,
            )
            .await
        {
            tracing::warn!(target: "post_turn", error = %e, "Reflection failed");
        }
        report.reflection_started = true;
    }

    tracing::debug!(
        target: "post_turn",
        signal = ?report.signal,
        explicit = report.explicit_mutation.is_some(),
        consolidation = report.consolidation_started,
        reflection = report.reflection_started,
        "Post-turn learning complete"
    );

    // Publish to SSE event stream (if available) — unified for web + channels.
    if let Some(ref tx) = input.event_tx {
        let _ = tx.send(serde_json::json!({
            "type": "post_turn_report",
            "agent_id": input.agent_id,
            "signal": report.signal.as_str(),
            "explicit_mutation": report.explicit_mutation.is_some(),
            "explicit_kind": report.explicit_mutation.as_ref().map(|event| format!("{:?}", event.kind)),
            "consolidation_started": report.consolidation_started,
            "reflection_started": report.reflection_started,
            "typed_fact_count": report.learning_evidence.typed_fact_count,
            "learning_facets": report.learning_evidence.facets,
            "learning_candidate_count": report.learning_candidates.len(),
            "learning_candidates": report.learning_candidates,
            "learning_assessment_count": report.learning_assessments.len(),
            "learning_assessments": report.learning_assessments,
            "candidate_mutation_count": report.candidate_mutations.len(),
            "run_recipes_upserted": report.run_recipes_upserted,
            "user_profile_updated": report.user_profile_updated,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingDistanceMetric, EmbeddingProfile,
        Entity, HybridSearchResult, MemoryCategory, MemoryEntry, MemoryError, MemoryId,
        MemoryQuery, Reflection, SearchResult, SessionId, Skill, SkillUpdate, TemporalFact,
        Visibility,
    };
    use crate::domain::tool_fact::{
        ProfileOperation, ToolFactPayload, TypedToolFact, UserProfileFact, UserProfileField,
    };
    use crate::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort,
        SkillMemoryPort, UnifiedMemoryPort, WorkingMemoryPort,
    };
    use crate::ports::user_profile_store::{InMemoryUserProfileStore, UserProfileStorePort};
    use crate::domain::user_profile::UserProfile;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[test]
    fn consolidation_gate_constants() {
        assert_eq!(CONSOLIDATE_MIN_CHARS, 20);
        assert_eq!(REFLECT_MIN_USER_CHARS, 30);
        assert_eq!(REFLECT_MIN_RESPONSE_LEN, 200);
    }

    struct StubMemory;

    #[async_trait]
    impl WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(
            &self,
            _: &AgentId,
        ) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
            Ok(vec![])
        }

        async fn update_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: String,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn append_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Err(MemoryError::Storage("not used in test".into()))
        }

        async fn get_recent(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }

        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }

        async fn search_episodes(
            &self,
            _: &MemoryQuery,
        ) -> Result<Vec<SearchResult>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SemanticMemoryPort for StubMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
            Err(MemoryError::Storage("not used in test".into()))
        }

        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
            Ok(None)
        }

        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
            Err(MemoryError::Storage("not used in test".into()))
        }

        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn get_current_facts(
            &self,
            _: &MemoryId,
        ) -> Result<Vec<TemporalFact>, MemoryError> {
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

    #[async_trait]
    impl SkillMemoryPort for StubMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
            Err(MemoryError::Storage("not used in test".into()))
        }

        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(vec![])
        }

        async fn update_skill(
            &self,
            _: &MemoryId,
            _: SkillUpdate,
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl ReflectionPort for StubMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
            Err(MemoryError::Storage("not used in test".into()))
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

    #[async_trait]
    impl ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
            Ok(ConsolidationReport {
                episodes_processed: 0,
                entities_extracted: 0,
                facts_created: 0,
                facts_invalidated: 0,
                skills_generated: 0,
                entries_garbage_collected: 0,
            })
        }

        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
            Ok(0)
        }

        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(HybridSearchResult::default())
        }

        async fn embed(&self, _: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(vec![0.0; 8])
        }

        fn embedding_profile(&self) -> EmbeddingProfile {
            EmbeddingProfile {
                profile_id: "post_turn_test".into(),
                provider_family: "test".into(),
                model_id: "post_turn_test".into(),
                distance_metric: EmbeddingDistanceMetric::Cosine,
                dimensions: 8,
                normalize_output: true,
                supports_multilingual: true,
                supports_code: false,
                query_prefix: None,
                document_prefix: None,
                recommended_chunk_chars: 512,
                recommended_top_k: 6,
            }
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

        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> {
            Ok(false)
        }

        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> {
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
            "post_turn_test"
        }

        async fn health_check(&self) -> bool {
            true
        }

        async fn promote_visibility(
            &self,
            _: &MemoryId,
            _: &Visibility,
            _: &[AgentId],
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn auto_updates_structured_user_profile_from_learning_candidates() {
        let memory = StubMemory;
        let store = Arc::new(InMemoryUserProfileStore::new());

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Remember my timezone".into(),
                assistant_response: "Saved your timezone.".into(),
                tools_used: vec!["user_profile".into()],
                tool_facts: vec![TypedToolFact {
                    tool_id: "user_profile".into(),
                    payload: ToolFactPayload::UserProfile(UserProfileFact {
                        field: UserProfileField::Timezone,
                        operation: ProfileOperation::Set,
                        value: Some("Europe/Berlin".into()),
                    }),
                }],
                run_recipe_store: None,
                user_profile_store: Some(store.clone()),
                user_profile_key: Some("web:test".into()),
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert!(report.user_profile_updated);
        assert_eq!(
            store.load("web:test").and_then(|profile| profile.timezone),
            Some("Europe/Berlin".into())
        );
    }

    #[tokio::test]
    async fn skips_user_profile_write_when_learning_patch_changes_nothing() {
        let memory = StubMemory;
        let store = Arc::new(InMemoryUserProfileStore::new());
        store
            .upsert(
                "web:test",
                UserProfile {
                    timezone: Some("Europe/Berlin".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Remember my timezone".into(),
                assistant_response: "Saved your timezone.".into(),
                tools_used: vec!["user_profile".into()],
                tool_facts: vec![TypedToolFact {
                    tool_id: "user_profile".into(),
                    payload: ToolFactPayload::UserProfile(UserProfileFact {
                        field: UserProfileField::Timezone,
                        operation: ProfileOperation::Set,
                        value: Some("Europe/Berlin".into()),
                    }),
                }],
                run_recipe_store: None,
                user_profile_store: Some(store),
                user_profile_key: Some("web:test".into()),
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert!(!report.user_profile_updated);
    }
}
