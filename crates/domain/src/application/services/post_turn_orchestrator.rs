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
use crate::application::services::memory_quality_governor;
use crate::application::services::precedent_similarity_service;
use crate::application::services::procedural_cluster_service::{
    plan_recent_clusters, ProceduralClusterKind,
};
use crate::application::services::recipe_evolution_service;
use crate::application::services::run_recipe_review_service;
use crate::application::services::skill_feedback_service;
use crate::application::services::skill_promotion_service::{self, SkillPromotionAssessment};
use crate::application::services::user_profile_service;
use crate::domain::memory::MemoryCategory;
use crate::domain::memory_mutation::{MutationCandidate, MutationSource, MutationThresholds};
use crate::domain::tool_fact::TypedToolFact;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::run_recipe_store::RunRecipeStorePort;
use crate::ports::user_profile_store::UserProfileStorePort;
use std::collections::HashSet;
use std::sync::Arc;

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
    /// Count of redundant run recipes removed after deterministic cluster review.
    pub run_recipes_removed: usize,
    /// Candidate/active skill assessments derived from repeated recipes.
    pub skill_promotion_assessments: Vec<SkillPromotionAssessment>,
    /// Count of learned skills created or refreshed from recipe promotion.
    pub skills_upserted: usize,
    /// Count of learned skills cooled down by accepted failure evidence.
    pub skills_penalized: usize,
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
    let learning_assessments = memory_quality_governor::govern_learning_assessments(
        &learning_assessments,
        &learning_evidence,
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
        run_recipes_removed: 0,
        skill_promotion_assessments: Vec::new(),
        skills_upserted: 0,
        skills_penalized: 0,
        user_profile_updated: false,
    };
    let allow_background_learning = matches!(
        memory_quality_governor::assess_background_learning_input(&input.user_message),
        memory_quality_governor::BackgroundLearningInputVerdict::Allow
    );

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
    if !signal.is_explicit() && input.auto_save_enabled && allow_background_learning {
        let recent_failure_clusters = if precedent_assessments.is_empty() {
            Vec::new()
        } else {
            plan_recent_clusters(
                mem,
                &input.agent_id,
                ProceduralClusterKind::FailurePattern,
                24,
                6,
                0.96,
            )
            .await
            .unwrap_or_default()
        };
        for assessment in &precedent_assessments {
            let Some(candidate) =
                learning_candidate_service::build_mutation_candidate_from_assessment(assessment)
            else {
                continue;
            };
            let decision =
                precedent_similarity_service::evaluate_precedent_candidate_with_failures(
                    mem,
                    candidate,
                    &input.agent_id,
                    &precedent_similarity_service::PrecedentSimilarityThresholds::default(),
                    &recent_failure_clusters,
                )
                .await;
            let decision = merge_precedent_update_decision(mem, &input.agent_id, decision).await;
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
    if !signal.is_explicit() && input.auto_save_enabled && allow_background_learning {
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
            let decision = merge_failure_update_decision(mem, &input.agent_id, decision).await;
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

    // ── 1c2. Failure feedback into learned skills ──
    if !signal.is_explicit() && input.auto_save_enabled && !failure_assessments.is_empty() {
        if let Ok(existing_skills) = mem.list_skills(&input.agent_id, 64).await {
            for assessment in &failure_assessments {
                if !assessment.accepted {
                    continue;
                }
                let LearningCandidate::FailurePattern(failure) = &assessment.candidate else {
                    continue;
                };
                for feedback in
                    skill_feedback_service::assess_failure_feedback(failure, &existing_skills)
                {
                    match mem
                        .update_skill(
                            &feedback.skill_id,
                            crate::domain::memory::SkillUpdate {
                                increment_success: false,
                                increment_fail: true,
                                new_description: None,
                                new_content: None,
                                new_task_family: None,
                                new_tool_pattern: None,
                                new_lineage_task_families: None,
                                new_status: None,
                            },
                            &input.agent_id,
                        )
                        .await
                    {
                        Ok(()) => report.skills_penalized += 1,
                        Err(e) => {
                            tracing::warn!(
                                target: "post_turn",
                                error = %e,
                                skill = %feedback.skill_name,
                                "Skill failure feedback update failed"
                            );
                        }
                    }
                }
            }
        }
    }

    // ── 1d. Cheap typed candidate mutation path ──
    if !signal.is_explicit()
        && input.auto_save_enabled
        && allow_background_learning
        && !mutation_candidates.is_empty()
    {
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
    if !signal.is_explicit() && input.auto_save_enabled && allow_background_learning {
        if let Some(store) = input.run_recipe_store.as_ref() {
            let existing_skills = mem
                .list_skills(&input.agent_id, 128)
                .await
                .unwrap_or_default();
            let recent_failure_clusters = match plan_recent_clusters(
                mem,
                &input.agent_id,
                ProceduralClusterKind::FailurePattern,
                16,
                6,
                0.96,
            )
            .await
            {
                Ok(clusters) => clusters,
                Err(e) => {
                    tracing::warn!(
                        target: "post_turn",
                        error = %e,
                        "Failure cluster lookup failed during skill promotion"
                    );
                    Vec::new()
                }
            };
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

            let mut recipes_for_skill_promotion = promoted_recipes.clone();
            if !promoted_recipes.is_empty() {
                let touched_families = promoted_recipes
                    .iter()
                    .map(|recipe| recipe.task_family.clone())
                    .collect::<HashSet<_>>();
                let review_decisions = run_recipe_review_service::review_run_recipes_with_failures(
                    &store.list(&input.agent_id),
                    &recent_failure_clusters,
                    &run_recipe_review_service::RunRecipeReviewThresholds::default(),
                );
                let mut blocked_canonicals = HashSet::new();
                let mut reviewed_families = HashSet::new();
                let mut reviewed_canonicals = Vec::new();

                for decision in review_decisions {
                    let touches_promoted_cluster = decision
                        .cluster_task_families
                        .iter()
                        .any(|task_family| touched_families.contains(task_family));
                    match store.upsert(decision.canonical_recipe.clone()) {
                        Ok(()) => {
                            if touches_promoted_cluster {
                                if decision.promotion_blocked {
                                    blocked_canonicals
                                        .insert(decision.canonical_recipe.task_family.clone());
                                    report.skill_promotion_assessments.push(
                                        SkillPromotionAssessment {
                                            skill_name: skill_promotion_service::build_skill_name(
                                                &decision.canonical_recipe,
                                            ),
                                            lineage_task_families: decision
                                                .canonical_recipe
                                                .lineage_task_families
                                                .clone(),
                                            accepted: false,
                                            reason: decision
                                                .promotion_block_reason
                                                .unwrap_or("blocked_recipe_cluster"),
                                            target_status:
                                                crate::domain::memory::SkillStatus::Candidate,
                                        },
                                    );
                                }
                                reviewed_canonicals.push(decision.canonical_recipe.clone());
                                reviewed_families
                                    .extend(decision.cluster_task_families.iter().cloned());
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "post_turn",
                                error = %e,
                                task_family = %decision.canonical_recipe.task_family,
                                "Run recipe review canonical upsert failed"
                            );
                            continue;
                        }
                    }

                    for task_family in &decision.removed_task_families {
                        match store.remove(&input.agent_id, task_family) {
                            Ok(()) => report.run_recipes_removed += 1,
                            Err(e) => {
                                tracing::warn!(
                                    target: "post_turn",
                                    error = %e,
                                    task_family = %task_family,
                                    "Run recipe review removal failed"
                                );
                            }
                        }
                    }
                }

                if !reviewed_canonicals.is_empty() {
                    recipes_for_skill_promotion
                        .retain(|recipe| !reviewed_families.contains(&recipe.task_family));
                    for canonical in reviewed_canonicals {
                        if !recipes_for_skill_promotion
                            .iter()
                            .any(|existing| existing.task_family == canonical.task_family)
                        {
                            recipes_for_skill_promotion.push(canonical);
                        }
                    }
                }
                if !blocked_canonicals.is_empty() {
                    recipes_for_skill_promotion
                        .retain(|recipe| !blocked_canonicals.contains(&recipe.task_family));
                }
            }

            for recipe in recipes_for_skill_promotion {
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
                let promotion =
                    skill_promotion_service::assess_recipe_for_skill_promotion_with_failures(
                        &recipe,
                        existing_skill.as_ref(),
                        &existing_skills,
                        &recent_failure_clusters,
                    );
                report.skill_promotion_assessments.push(promotion.clone());
                if !promotion.accepted {
                    continue;
                }

                let result = if let Some(existing_skill) = existing_skill {
                    mem.update_skill(
                        &existing_skill.id,
                        skill_promotion_service::build_skill_update(
                            Some(&existing_skill),
                            &recipe,
                            &promotion,
                        ),
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
    let consolidation_verdict = memory_quality_governor::assess_consolidation_start(
        &learning_evidence,
        &input.user_message,
        memory_quality_governor::CONSOLIDATION_MIN_USER_CHARS,
    );
    let should_consolidate = !signal.is_explicit()
        && input.auto_save_enabled
        && matches!(
            consolidation_verdict,
            memory_quality_governor::ConsolidationStartVerdict::Start
        );

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
    let reflection_verdict = memory_quality_governor::assess_reflection_start(
        &learning_evidence,
        user_chars,
        input.assistant_response.len(),
        memory_quality_governor::REFLECTION_MIN_USER_CHARS,
        memory_quality_governor::REFLECTION_MIN_RESPONSE_CHARS,
    );
    let should_reflect = matches!(
        reflection_verdict,
        memory_quality_governor::ReflectionStartVerdict::Start
    );

    if should_reflect {
        let reflection_outcome = memory_quality_governor::derive_reflection_outcome(
            &learning_evidence,
            &input.tools_used,
        );
        if let Err(e) = mem
            .reflect_on_turn(&input.user_message, &input.tools_used, &reflection_outcome)
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
            "run_recipes_removed": report.run_recipes_removed,
            "skill_promotion_count": report.skill_promotion_assessments.len(),
            "skill_promotion_assessments": report.skill_promotion_assessments,
            "skills_upserted": report.skills_upserted,
            "skills_penalized": report.skills_penalized,
            "user_profile_updated": report.user_profile_updated,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    report
}

async fn merge_precedent_update_decision(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    mut decision: crate::domain::memory_mutation::MutationDecision,
) -> crate::domain::memory_mutation::MutationDecision {
    let crate::domain::memory_mutation::MutationAction::Update { target_id } = &decision.action
    else {
        return decision;
    };
    if let Ok(Some(existing)) = mem.get(target_id, &agent_id.to_string()).await {
        decision.candidate.text = precedent_similarity_service::merge_precedent_text(
            &existing.content,
            &decision.candidate.text,
        );
    }
    decision
}

async fn merge_failure_update_decision(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    mut decision: crate::domain::memory_mutation::MutationDecision,
) -> crate::domain::memory_mutation::MutationDecision {
    let crate::domain::memory_mutation::MutationAction::Update { target_id } = &decision.action
    else {
        return decision;
    };
    if let Ok(Some(existing)) = mem.get(target_id, &agent_id.to_string()).await {
        decision.candidate.text = failure_similarity_service::merge_failure_text(
            &existing.content,
            &decision.candidate.text,
        );
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dialogue_state::FocusEntity;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingDistanceMetric, EmbeddingProfile,
        Entity, HybridSearchResult, MemoryCategory, MemoryEntry, MemoryError, MemoryId,
        MemoryQuery, Reflection, SearchResult, SessionId, Skill, SkillUpdate, TemporalFact,
        Visibility,
    };
    use crate::domain::tool_fact::{
        OutcomeStatus, ProfileOperation, ResourceFact, ResourceKind, ResourceMetadata,
        ResourceOperation, SearchDomain, SearchFact, ToolFactPayload, TypedToolFact,
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
        assert_eq!(memory_quality_governor::CONSOLIDATION_MIN_USER_CHARS, 20);
        assert_eq!(memory_quality_governor::REFLECTION_MIN_USER_CHARS, 30);
        assert_eq!(memory_quality_governor::REFLECTION_MIN_RESPONSE_CHARS, 200);
    }

    #[derive(Default)]
    struct StubMemory {
        skills: parking_lot::RwLock<Vec<Skill>>,
        entries: parking_lot::RwLock<Vec<MemoryEntry>>,
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
            category: Option<&MemoryCategory>,
            _: Option<&str>,
            limit: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            let mut entries = self.entries.read().clone();
            if let Some(category) = category {
                entries.retain(|entry| &entry.category == category);
            }
            entries.truncate(limit);
            Ok(entries)
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
    async fn internal_only_memory_turn_does_not_promote_procedural_learning_or_reflection() {
        let memory = StubMemory::default();
        let run_recipe_store =
            Arc::new(crate::ports::run_recipe_store::InMemoryRunRecipeStore::new());

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Let's continue the philosophical discussion about the meaning of life"
                    .into(),
                assistant_response:
                    "I revisited a few earlier notes and compared them to the current thread, but this was still a reflective discussion rather than an external procedure worth learning."
                        .into(),
                tools_used: vec!["memory_recall".into()],
                tool_facts: vec![
                    TypedToolFact {
                        tool_id: "memory_recall".into(),
                        payload: ToolFactPayload::Search(SearchFact {
                            domain: SearchDomain::Memory,
                            query: Some("meaning of life".into()),
                            result_count: Some(3),
                            primary_locator: Some("daily_123".into()),
                        }),
                    },
                    TypedToolFact::focus(
                        "memory_recall",
                        vec![FocusEntity {
                            kind: "topic".into(),
                            name: "meaning of life".into(),
                            metadata: None,
                        }],
                        Vec::new(),
                    ),
                ],
                run_recipe_store: Some(run_recipe_store),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert!(report
            .learning_assessments
            .iter()
            .all(|assessment| !assessment.accepted));
        assert_eq!(report.run_recipes_upserted, 0);
        assert_eq!(report.skills_upserted, 0);
        assert!(!report.consolidation_started);
        assert!(!report.reflection_started);
    }

    #[tokio::test]
    async fn long_semantic_turn_without_tool_facts_still_consolidates() {
        let memory = StubMemory::default();
        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "I want to keep exploring the meaning of life through responsibility, memory, and how a person changes over time.".into(),
                assistant_response:
                    "We can treat meaning as something partially discovered and partially constructed through repeated commitments."
                        .into(),
                tools_used: vec![],
                tool_facts: vec![],
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert!(report.consolidation_started);
    }

    #[tokio::test]
    async fn low_information_repetition_skips_background_candidate_learning() {
        let memory = StubMemory::default();
        let run_recipe_store =
            Arc::new(crate::ports::run_recipe_store::InMemoryRunRecipeStore::new());

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message:
                    "again again again again again again again again again again again again"
                        .into(),
                assistant_response: "Fetched and sent the update.".into(),
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

        assert!(report.candidate_mutations.is_empty());
        assert_eq!(report.run_recipes_upserted, 0);
        assert_eq!(report.skills_upserted, 0);
    }

    #[tokio::test]
    async fn accepted_failure_pattern_penalizes_overlapping_learned_skill() {
        let memory = StubMemory::default();
        memory.skills.write().push(Skill {
            id: "skill_1".into(),
            name: "search_delivery".into(),
            description: "Promoted skill".into(),
            content: "content".into(),
            task_family: Some("search_delivery".into()),
            lineage_task_families: vec!["search_delivery".into()],
            tool_pattern: vec!["web_fetch".into(), "message_send".into()],
            tags: vec!["recipe-promotion".into()],
            success_count: 4,
            fail_count: 0,
            version: 1,
            origin: crate::domain::memory::SkillOrigin::Learned,
            status: crate::domain::memory::SkillStatus::Active,
            created_by: "agent".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        });

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Fetch the page and send it".into(),
                assistant_response: "The fetch failed.".into(),
                tools_used: vec!["web_fetch".into(), "message_send".into()],
                tool_facts: vec![
                    TypedToolFact::outcome("web_fetch", OutcomeStatus::RuntimeError, Some(220)),
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
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert_eq!(report.skills_penalized, 1);
        assert_eq!(memory.skills.read()[0].fail_count, 1);
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
                lineage_task_families: vec!["search_delivery".into()],
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
        assert_eq!(report.run_recipes_removed, 0);
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

    #[tokio::test]
    async fn recipe_review_removes_redundant_cross_family_duplicates() {
        let memory = StubMemory::default();
        let run_recipe_store =
            Arc::new(crate::ports::run_recipe_store::InMemoryRunRecipeStore::new());
        run_recipe_store
            .upsert(crate::domain::run_recipe::RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_resource_delivery".into(),
                lineage_task_families: vec!["search_resource_delivery".into()],
                sample_request: "find the status page, fetch it, open it and send it again".into(),
                summary: "Use search, resource tools and deliver the result.".into(),
                tool_pattern: vec![
                    "web_search".into(),
                    "web_fetch".into(),
                    "browser".into(),
                    "http_request".into(),
                    "shell".into(),
                    "file_read".into(),
                    "file_edit".into(),
                    "message_send".into(),
                ],
                success_count: 2,
                updated_at: 1,
            })
            .unwrap();
        run_recipe_store
            .upsert(crate::domain::run_recipe::RunRecipe {
                agent_id: "agent".into(),
                task_family: "delivery_search_resource".into(),
                lineage_task_families: vec!["delivery_search_resource".into()],
                sample_request: "find the status page, fetch it, open it and send it again".into(),
                summary: "Use search, resource tools and deliver the result.".into(),
                tool_pattern: vec![
                    "web_search".into(),
                    "web_fetch".into(),
                    "browser".into(),
                    "http_request".into(),
                    "shell".into(),
                    "file_read".into(),
                    "message_send".into(),
                ],
                success_count: 4,
                updated_at: 2,
            })
            .unwrap();

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Find the status page, fetch it, open it and send it again".into(),
                assistant_response: "Fetched the page, opened it and sent it again.".into(),
                tools_used: vec![
                    "web_search".into(),
                    "web_fetch".into(),
                    "browser".into(),
                    "http_request".into(),
                    "shell".into(),
                    "file_read".into(),
                    "file_edit".into(),
                    "message_send".into(),
                ],
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
                        tool_id: "web_fetch".into(),
                        payload: ToolFactPayload::Resource(ResourceFact {
                            kind: ResourceKind::WebPage,
                            operation: ResourceOperation::Fetch,
                            locator: "https://status.example.com".into(),
                            host: Some("status.example.com".into()),
                            metadata: ResourceMetadata::default(),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "browser".into(),
                        payload: ToolFactPayload::Resource(ResourceFact {
                            kind: ResourceKind::BrowserPage,
                            operation: ResourceOperation::Open,
                            locator: "https://status.example.com".into(),
                            host: Some("status.example.com".into()),
                            metadata: ResourceMetadata::default(),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "http_request".into(),
                        payload: ToolFactPayload::Resource(ResourceFact {
                            kind: ResourceKind::WebResource,
                            operation: ResourceOperation::Fetch,
                            locator: "https://status.example.com".into(),
                            host: Some("status.example.com".into()),
                            metadata: ResourceMetadata::default(),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "shell".into(),
                        payload: ToolFactPayload::Resource(ResourceFact {
                            kind: ResourceKind::File,
                            operation: ResourceOperation::Inspect,
                            locator: "status.log".into(),
                            host: None,
                            metadata: ResourceMetadata::default(),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "file_read".into(),
                        payload: ToolFactPayload::Resource(ResourceFact {
                            kind: ResourceKind::File,
                            operation: ResourceOperation::Read,
                            locator: "status.log".into(),
                            host: None,
                            metadata: ResourceMetadata::default(),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "file_edit".into(),
                        payload: ToolFactPayload::Resource(ResourceFact {
                            kind: ResourceKind::File,
                            operation: ResourceOperation::Edit,
                            locator: "status.md".into(),
                            host: None,
                            metadata: ResourceMetadata::default(),
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
                run_recipe_store: Some(run_recipe_store.clone()),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
            },
        )
        .await;

        assert_eq!(report.run_recipes_upserted, 1);
        assert_eq!(report.run_recipes_removed, 1);
        let recipes = run_recipe_store.list("agent");
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].task_family, "delivery_search_resource");
        assert_eq!(recipes[0].success_count, 7);
    }

    #[tokio::test]
    async fn contradicted_recipe_cluster_blocks_skill_promotion() {
        let memory = StubMemory::default();
        memory.entries.write().push(MemoryEntry {
            id: "f1".into(),
            key: "f1".into(),
            content: "failed_tools=web_search -> message_send | outcomes=runtime_error".into(),
            category: MemoryCategory::Custom("failure_pattern".into()),
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: None,
            score: None,
        });
        let run_recipe_store =
            Arc::new(crate::ports::run_recipe_store::InMemoryRunRecipeStore::new());
        run_recipe_store
            .upsert(crate::domain::run_recipe::RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                lineage_task_families: vec!["search_delivery".into()],
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
        assert_eq!(report.skills_upserted, 0);
        assert!(report
            .skill_promotion_assessments
            .iter()
            .any(|assessment| assessment.reason == "contradicted_by_failure_clusters"));
    }
}
