//! Post-turn learning orchestrator — single source of truth for learning policy.
//!
//! Both web and channel paths call `execute_post_turn_learning()` instead of
//! implementing their own spawn/decide/mutate logic. This eliminates policy
//! divergence between transport adapters.

use crate::application::services::failure_similarity_service;
use crate::application::services::learning_candidate_service::{self, LearningCandidate};
use crate::application::services::learning_conflict_service;
use crate::application::services::learning_events::LearningEvent;
use crate::application::services::learning_evidence_service::{self, LearningEvidenceEnvelope};
use crate::application::services::learning_quality_service::{self, LearningCandidateAssessment};
use crate::application::services::learning_signals::{self, LearningSignal};
use crate::application::services::learning_strength_service;
use crate::application::services::memory_mutation as mutation;
use crate::application::services::precedent_similarity_service;
use crate::application::services::recipe_evolution_service;
use crate::application::services::skill_promotion_service::{self, SkillPromotionAssessment};
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
    /// Candidate/active skill assessments derived from repeated recipes.
    pub skill_promotion_assessments: Vec<SkillPromotionAssessment>,
    /// Count of learned skills created or refreshed from recipe promotion.
    pub skills_upserted: usize,
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
    let current_profile = if let (Some(store), Some(user_key)) = (
        input.user_profile_store.as_ref(),
        input.user_profile_key.as_deref(),
    ) {
        store.load(user_key)
    } else {
        None
    };
    let existing_recipes = input
        .run_recipe_store
        .as_ref()
        .map(|store| store.list(&input.agent_id))
        .unwrap_or_default();
    let learning_assessments = learning_conflict_service::resolve_learning_conflicts(
        &learning_strength_service::strengthen_learning_assessments(
            &learning_quality_service::assess_learning_candidates(
                &learning_candidates,
                &learning_evidence,
                &existing_recipes,
            ),
            current_profile.as_ref(),
            &existing_recipes,
        ),
    );
    let precedent_assessments = learning_assessments
        .iter()
        .filter(|assessment| matches!(assessment.candidate, LearningCandidate::Precedent(_)))
        .cloned()
        .collect::<Vec<_>>();
    let failure_assessments = learning_assessments
        .iter()
        .filter(|assessment| matches!(assessment.candidate, LearningCandidate::FailurePattern(_)))
        .cloned()
        .collect::<Vec<_>>();
    let mutation_candidates =
        learning_candidate_service::build_mutation_candidates_from_assessments(
            &learning_assessments
                .iter()
                .filter(|assessment| {
                    !matches!(
                        assessment.candidate,
                        LearningCandidate::Precedent(_) | LearningCandidate::FailurePattern(_)
                    )
                })
                .cloned()
                .collect::<Vec<_>>(),
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
        skill_promotion_assessments: Vec::new(),
        skills_upserted: 0,
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

    // ── 1b. Cheap precedent mutation path (category-aware semantic merge) ──
    if !signal.is_explicit() && input.auto_save_enabled {
        for assessment in &precedent_assessments {
            let Some(candidate) =
                learning_candidate_service::build_mutation_candidate_from_assessment(assessment)
            else {
                continue;
            };
            let decision = precedent_similarity_service::evaluate_precedent_candidate(
                mem,
                candidate,
                &input.agent_id,
                &precedent_similarity_service::PrecedentSimilarityThresholds::default(),
            )
            .await;
            match mutation::apply_decision_with_event(mem, &decision, &input.agent_id).await {
                Ok(event) => report.candidate_mutations.push(event),
                Err(e) => {
                    tracing::warn!(
                        target: "post_turn",
                        error = %e,
                        "Precedent mutation failed"
                    );
                }
            }
        }
    }

    // ── 1c. Cheap failure-pattern mutation path ──
    if !signal.is_explicit() && input.auto_save_enabled {
        for assessment in &failure_assessments {
            let Some(candidate) =
                learning_candidate_service::build_mutation_candidate_from_assessment(assessment)
            else {
                continue;
            };
            let decision = failure_similarity_service::evaluate_failure_candidate(
                mem,
                candidate,
                &input.agent_id,
                &failure_similarity_service::FailureSimilarityThresholds::default(),
            )
            .await;
            match mutation::apply_decision_with_event(mem, &decision, &input.agent_id).await {
                Ok(event) => report.candidate_mutations.push(event),
                Err(e) => {
                    tracing::warn!(
                        target: "post_turn",
                        error = %e,
                        "Failure-pattern mutation failed"
                    );
                }
            }
        }
    }

    // ── 1d. Cheap typed candidate mutation path ──
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

    // ── 1e. Cheap procedural candidate path ──
    if !signal.is_explicit() && input.auto_save_enabled {
        if let Some(store) = input.run_recipe_store.as_ref() {
            let mut promoted_recipes = Vec::new();
            for assessment in &learning_assessments {
                if !assessment.accepted {
                    continue;
                }
                let LearningCandidate::RunRecipe(recipe_candidate) = &assessment.candidate else {
                    continue;
                };
                let updated_at = chrono::Utc::now().timestamp().max(0) as u64;
                let existing = store.get(&input.agent_id, &recipe_candidate.task_family_hint);
                let recipe = if assessment.merge_with_existing {
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
                match store.upsert(recipe.clone()) {
                    Ok(()) => {
                        report.run_recipes_upserted += 1;
                        promoted_recipes.push(recipe);
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "post_turn",
                            error = %e,
                            "Run recipe upsert failed"
                        );
                    }
                }
            }

            for recipe in promoted_recipes {
                let skill_name = skill_promotion_service::build_skill_name(&recipe);
                let existing_skill = match mem.get_skill(&skill_name, &input.agent_id).await {
                    Ok(skill) => skill,
                    Err(e) => {
                        tracing::warn!(
                            target: "post_turn",
                            error = %e,
                            skill = %skill_name,
                            "Skill lookup failed during recipe promotion"
                        );
                        None
                    }
                };
                let promotion = skill_promotion_service::assess_recipe_for_skill_promotion(
                    &recipe,
                    existing_skill.as_ref(),
                );
                report.skill_promotion_assessments.push(promotion.clone());
                if !promotion.accepted {
                    continue;
                }

                let result = if let Some(existing_skill) = existing_skill {
                    mem.update_skill(
                        &existing_skill.id,
                        skill_promotion_service::build_skill_update(&recipe, &promotion),
                        &input.agent_id,
                    )
                    .await
                } else {
                    mem.store_skill(skill_promotion_service::build_new_skill(
                        &input.agent_id,
                        &recipe,
                        &promotion,
                    ))
                    .await
                    .map(|_| ())
                };

                match result {
                    Ok(()) => report.skills_upserted += 1,
                    Err(e) => {
                        tracing::warn!(
                            target: "post_turn",
                            error = %e,
                            skill = %promotion.skill_name,
                            "Skill promotion failed"
                        );
                    }
                }
            }
        }
    }

    // ── 1f. Cheap structured profile path ──
    if !signal.is_explicit() && input.auto_save_enabled {
        if let (Some(store), Some(user_key)) = (
            input.user_profile_store.as_ref(),
            input.user_profile_key.as_deref(),
        ) {
            let patch = learning_candidate_service::build_user_profile_patch_from_assessments(
                &learning_assessments,
                current_profile.as_ref(),
            );
            if !patch.is_noop() {
                let updated = user_profile_service::apply_patch(current_profile.clone(), &patch);
                if updated != current_profile {
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
    let has_failures = learning_evidence.has_failure_outcomes();
    let has_reflection_signal =
        !input.tools_used.is_empty() || learning_evidence.has_actionable_evidence() || has_failures;
    let should_reflect = user_chars >= REFLECT_MIN_USER_CHARS
        && has_reflection_signal
        && (input.assistant_response.len() > REFLECT_MIN_RESPONSE_LEN || has_failures);

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
            "failure_outcome_count": report.learning_evidence.failure_outcome_count,
            "learning_facets": report.learning_evidence.facets,
            "learning_candidate_count": report.learning_candidates.len(),
            "learning_candidates": report.learning_candidates,
            "learning_assessment_count": report.learning_assessments.len(),
            "learning_assessments": report.learning_assessments,
            "candidate_mutation_count": report.candidate_mutations.len(),
            "run_recipes_upserted": report.run_recipes_upserted,
            "skill_promotion_count": report.skill_promotion_assessments.len(),
            "skill_promotion_assessments": report.skill_promotion_assessments,
            "skills_upserted": report.skills_upserted,
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
        OutcomeStatus, ProfileOperation, SearchDomain, SearchFact, ToolFactPayload, TypedToolFact,
        UserProfileFact, UserProfileField,
    };
    use crate::domain::user_profile::UserProfile;
    use crate::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        UnifiedMemoryPort, WorkingMemoryPort,
    };
    use crate::ports::user_profile_store::{InMemoryUserProfileStore, UserProfileStorePort};
    use async_trait::async_trait;
    use std::sync::Arc;

    #[test]
    fn consolidation_gate_constants() {
        assert_eq!(CONSOLIDATE_MIN_CHARS, 20);
        assert_eq!(REFLECT_MIN_USER_CHARS, 30);
        assert_eq!(REFLECT_MIN_RESPONSE_LEN, 200);
    }

    #[derive(Default)]
    struct StubMemory {
        skills: parking_lot::RwLock<Vec<Skill>>,
    }

    #[async_trait]
    impl WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
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

    #[async_trait]
    impl SkillMemoryPort for StubMemory {
        async fn store_skill(&self, mut skill: Skill) -> Result<MemoryId, MemoryError> {
            if skill.id.is_empty() {
                skill.id = format!("skill_{}", self.skills.read().len() + 1);
            }
            let id = skill.id.clone();
            self.skills.write().push(skill);
            Ok(id)
        }

        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(self.skills.read().clone())
        }

        async fn update_skill(
            &self,
            skill_id: &MemoryId,
            update: SkillUpdate,
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            if let Some(existing) = self
                .skills
                .write()
                .iter_mut()
                .find(|skill| skill.id == *skill_id)
            {
                if update.increment_success {
                    existing.success_count = existing.success_count.saturating_add(1);
                }
                if update.increment_fail {
                    existing.fail_count = existing.fail_count.saturating_add(1);
                }
                let should_bump_version = update.new_description.is_some()
                    || update.new_content.is_some()
                    || update.new_status.is_some();
                if let Some(description) = update.new_description {
                    existing.description = description;
                }
                if let Some(content) = update.new_content {
                    existing.content = content;
                }
                if let Some(status) = update.new_status {
                    existing.status = status;
                }
                if should_bump_version {
                    existing.version = existing.version.saturating_add(1);
                }
            }
            Ok(())
        }

        async fn get_skill(&self, name: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> {
            Ok(self
                .skills
                .read()
                .iter()
                .find(|skill| skill.name == name)
                .cloned())
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
        let memory = StubMemory::default();
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
        let memory = StubMemory::default();
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

    #[tokio::test]
    async fn skips_user_profile_write_for_conflicting_profile_candidates() {
        let memory = StubMemory::default();
        let store = Arc::new(InMemoryUserProfileStore::new());

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Remember both timezone options".into(),
                assistant_response: "Captured conflicting timezone facts.".into(),
                tools_used: vec!["user_profile".into()],
                tool_facts: vec![
                    TypedToolFact {
                        tool_id: "user_profile".into(),
                        payload: ToolFactPayload::UserProfile(UserProfileFact {
                            field: UserProfileField::Timezone,
                            operation: ProfileOperation::Set,
                            value: Some("Europe/Berlin".into()),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "user_profile".into(),
                        payload: ToolFactPayload::UserProfile(UserProfileFact {
                            field: UserProfileField::Timezone,
                            operation: ProfileOperation::Set,
                            value: Some("Europe/Paris".into()),
                        }),
                    },
                ],
                run_recipe_store: None,
                user_profile_store: Some(store.clone()),
                user_profile_key: Some("web:test".into()),
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert!(!report.user_profile_updated);
        assert!(report
            .learning_assessments
            .iter()
            .all(|assessment| !assessment.accepted));
        assert!(store.load("web:test").is_none());
    }

    #[tokio::test]
    async fn typed_failure_outcomes_trigger_reflection_without_string_matching() {
        let memory = StubMemory::default();
        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message:
                    "Please fetch the status page and summarize what it says for the team".into(),
                assistant_response: "I could not complete the request in time, but I captured structured outcome context for follow-up handling without relying on literal error keywords in the reflection gate.".into(),
                tools_used: vec!["web_fetch".into()],
                tool_facts: vec![TypedToolFact::outcome(
                    "web_fetch",
                    OutcomeStatus::RuntimeError,
                    Some(220),
                )],
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert!(report.reflection_started);
    }

    #[tokio::test]
    async fn repeated_recipe_promotes_skill_candidate() {
        let memory = StubMemory::default();
        let run_recipe_store =
            Arc::new(crate::ports::run_recipe_store::InMemoryRunRecipeStore::new());
        run_recipe_store
            .upsert(crate::domain::run_recipe::RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                sample_request: "find the status page and send it".into(),
                summary: "Use web search and deliver the result.".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                success_count: 2,
                updated_at: 1,
            })
            .unwrap();

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Find the status page and send it again".into(),
                assistant_response: "Fetched the page and sent it again.".into(),
                tools_used: vec!["web_search".into(), "message_send".into()],
                tool_facts: vec![
                    TypedToolFact {
                        tool_id: "web_search".into(),
                        payload: ToolFactPayload::Search(SearchFact {
                            domain: SearchDomain::Web,
                            query: Some("status page".into()),
                            result_count: Some(2),
                            primary_locator: Some("https://status.example.com".into()),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "message_send".into(),
                        payload: ToolFactPayload::Delivery(
                            crate::domain::tool_fact::DeliveryFact {
                                target: crate::domain::tool_fact::DeliveryTargetKind::CurrentConversation,
                                content_bytes: Some(24),
                            },
                        ),
                    },
                ],
                run_recipe_store: Some(run_recipe_store),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert_eq!(report.run_recipes_upserted, 1);
        assert_eq!(report.skills_upserted, 1);
        assert!(report
            .skill_promotion_assessments
            .iter()
            .any(|assessment| assessment.reason == "create_candidate_skill"));
        let stored_skills = memory.skills.read().clone();
        assert_eq!(stored_skills.len(), 1);
        assert_eq!(
            stored_skills[0].status,
            crate::domain::memory::SkillStatus::Candidate
        );
        assert_eq!(
            stored_skills[0].origin,
            crate::domain::memory::SkillOrigin::Learned
        );
    }
}
