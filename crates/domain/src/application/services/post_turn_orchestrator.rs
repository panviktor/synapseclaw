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
use crate::application::services::runtime_decision_trace::{
    merge_runtime_decision_trace_update, runtime_auxiliary_decision,
    runtime_memory_decision_from_mutation, RuntimeDecisionTraceUpdate,
    RuntimeTraceAuxiliaryDecision, RuntimeTraceMemoryDecision,
};
use crate::application::services::runtime_trace_janitor::RUNTIME_TRACE_JANITOR_TTL_SECS;
use crate::application::services::skill_feedback_service;
use crate::application::services::skill_governance_service::{
    SkillPatchCandidate, SkillUseOutcome,
};
use crate::application::services::skill_patch_candidate_service::{
    self, build_skill_patch_candidates_from_repairs, SkillPatchCandidatePolicy,
};
use crate::application::services::skill_promotion_service::{self, SkillPromotionAssessment};
use crate::application::services::skill_trace_service::{
    build_skill_use_trace_from_live_turn, parse_skill_activation_trace_entry,
    skill_activation_trace_memory_category, skill_activation_trace_memory_key_prefix,
    skill_use_trace_memory_category, skill_use_trace_memory_key, skill_use_trace_to_memory_entry,
};
use crate::application::services::user_profile_service;
use crate::domain::memory::{MemoryCategory, Skill, SkillOrigin, SkillStatus, SkillUpdate};
use crate::domain::memory_mutation::{
    MutationCandidate, MutationDecision, MutationSource, MutationThresholds, MutationWriteClass,
};
use crate::domain::tool_fact::TypedToolFact;
use crate::domain::tool_repair::ToolRepairTrace;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::route_selection::RouteSelectionPort;
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
    /// Bounded current/recent repair traces from the live route. Used to propose
    /// reviewable skill patches without editing active skills directly.
    pub tool_repairs: Vec<ToolRepairTrace>,
    pub run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    pub user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
    pub user_profile_key: Option<String>,
    pub auto_save_enabled: bool,
    /// Optional SSE event sender for publishing reports to UI.
    /// Both web and channels should pass this if available.
    pub event_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
    /// Optional sink for merging post-turn memory/aux decisions into the turn trace.
    pub runtime_trace_sink: Option<PostTurnRuntimeTraceSink>,
}

#[derive(Clone)]
pub struct PostTurnRuntimeTraceSink {
    pub trace_id: String,
    pub conversation_key: String,
    pub routes: Arc<dyn RouteSelectionPort>,
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
    /// Count of compact live skill use traces written from typed turn evidence.
    pub skill_use_traces_recorded: usize,
    /// Count of learned skill success/failure counters updated from live traces.
    pub skill_use_feedback_updates: usize,
    /// Reviewable skill patch candidates generated from repeated repair traces.
    pub skill_patch_candidates: Vec<SkillPatchCandidate>,
    /// Count of generated skill patch candidates queued for operator review.
    pub skill_patch_candidates_queued: usize,
    /// Whether a dynamic user profile patch was applied.
    pub user_profile_updated: bool,
    /// Redacted runtime memory decisions for the matching turn trace.
    pub runtime_memory_decisions: Vec<RuntimeTraceMemoryDecision>,
    /// Redacted auxiliary learning decisions for the matching turn trace.
    pub runtime_auxiliary_decisions: Vec<RuntimeTraceAuxiliaryDecision>,
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
        skill_use_traces_recorded: 0,
        skill_use_feedback_updates: 0,
        skill_patch_candidates: Vec::new(),
        skill_patch_candidates_queued: 0,
        user_profile_updated: false,
        runtime_memory_decisions: Vec::new(),
        runtime_auxiliary_decisions: Vec::new(),
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
            write_class: Some(MutationWriteClass::FactAnchor),
        };
        let decision = mutation::evaluate_candidate(
            mem,
            candidate,
            &input.agent_id,
            &MutationThresholds::default(),
        )
        .await;
        let (event, trace) =
            apply_decision_with_runtime_trace(mem, &decision, &input.agent_id, None).await;
        report.runtime_memory_decisions.push(trace);
        if let Some(event) = event {
            tracing::debug!(
                target: "post_turn",
                kind = ?event.kind,
                agent_id = %input.agent_id,
                "Explicit learning event"
            );
            report.explicit_mutation = Some(event);
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
            let (event, trace) = apply_decision_with_runtime_trace(
                mem,
                &decision,
                &input.agent_id,
                Some("precedent_similarity"),
            )
            .await;
            report.runtime_memory_decisions.push(trace);
            if let Some(event) = event {
                report.candidate_mutations.push(event);
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
            let (event, trace) = apply_decision_with_runtime_trace(
                mem,
                &decision,
                &input.agent_id,
                Some("failure_similarity"),
            )
            .await;
            report.runtime_memory_decisions.push(trace);
            if let Some(event) = event {
                report.candidate_mutations.push(event);
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
                                new_tags: None,
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
            let (event, trace) =
                apply_decision_with_runtime_trace(mem, &decision, &input.agent_id, None).await;
            report.runtime_memory_decisions.push(trace);
            if let Some(event) = event {
                report.candidate_mutations.push(event);
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

    // ── 1e2. Repair trace -> skill patch candidate queue ──
    if !signal.is_explicit()
        && input.auto_save_enabled
        && allow_background_learning
        && !input.tool_repairs.is_empty()
    {
        let patch_candidates = build_repair_patch_candidates_for_live_skills(
            mem,
            &input.agent_id,
            &input.tool_repairs,
            &SkillPatchCandidatePolicy::default(),
        )
        .await;
        let queued = queue_skill_patch_candidates(mem, &patch_candidates, &input.agent_id).await;
        report.skill_patch_candidates_queued += queued.len();
        report.skill_patch_candidates.extend(queued);
    }

    // ── 1e3. Active skill use trace from typed live-turn evidence ──
    if !signal.is_explicit() && input.auto_save_enabled {
        let live_skill_use_report = record_live_skill_use_traces(
            mem,
            &input.agent_id,
            &input.tools_used,
            &input.tool_facts,
            &input.tool_repairs,
            chrono::Utc::now(),
        )
        .await;
        report.skill_use_traces_recorded = live_skill_use_report.traces_recorded;
        report.skill_use_feedback_updates = live_skill_use_report.skill_stats_updated;
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

    let trace_observed_at_unix = chrono::Utc::now().timestamp();
    report.runtime_auxiliary_decisions = build_runtime_auxiliary_decisions(
        &report,
        input.auto_save_enabled,
        allow_background_learning,
        should_consolidate,
        should_reflect,
        trace_observed_at_unix,
    );
    if let Some(sink) = input.runtime_trace_sink.as_ref() {
        publish_runtime_trace_fragments(sink, &report, trace_observed_at_unix);
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
            "skill_use_traces_recorded": report.skill_use_traces_recorded,
            "skill_use_feedback_updates": report.skill_use_feedback_updates,
            "skill_patch_candidate_count": report.skill_patch_candidates.len(),
            "skill_patch_candidates_queued": report.skill_patch_candidates_queued,
            "skill_patch_candidates": report.skill_patch_candidates,
            "user_profile_updated": report.user_profile_updated,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    report
}

async fn build_repair_patch_candidates_for_live_skills(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    tool_repairs: &[ToolRepairTrace],
    policy: &SkillPatchCandidatePolicy,
) -> Vec<SkillPatchCandidate> {
    let skills = match mem.list_skills(&agent_id.to_string(), 128).await {
        Ok(skills) => skills,
        Err(e) => {
            tracing::warn!(
                target: "post_turn",
                error = %e,
                "Skill lookup failed during repair patch candidate generation"
            );
            return Vec::new();
        }
    };

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for skill in skills
        .iter()
        .filter(|skill| skill_allows_patch_candidate(skill))
    {
        let relevant_repairs = relevant_repairs_for_skill(skill, tool_repairs);
        if relevant_repairs.is_empty() {
            continue;
        }
        for candidate in build_skill_patch_candidates_from_repairs(skill, &relevant_repairs, policy)
        {
            if seen.insert(candidate.id.clone()) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

async fn queue_skill_patch_candidates(
    mem: &dyn UnifiedMemoryPort,
    candidates: &[SkillPatchCandidate],
    agent_id: &str,
) -> Vec<SkillPatchCandidate> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let category = skill_patch_candidate_service::skill_patch_candidate_memory_category();
    let existing_keys = mem
        .list(Some(&category), None, 512)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.key)
        .collect::<HashSet<_>>();

    let mut queued = Vec::new();
    for candidate in candidates {
        let key = skill_patch_candidate_service::skill_patch_candidate_memory_key(candidate);
        if existing_keys.contains(&key) {
            continue;
        }
        let entry = match skill_patch_candidate_service::skill_patch_candidate_to_memory_entry(
            candidate,
            chrono::Utc::now(),
        ) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(
                    target: "post_turn",
                    error = %e,
                    candidate = %candidate.id,
                    "Skill patch candidate serialization failed"
                );
                continue;
            }
        };
        match mem.store_episode(entry).await {
            Ok(_) => queued.push(candidate.clone()),
            Err(e) => tracing::warn!(
                target: "post_turn",
                error = %e,
                agent_id = %agent_id,
                candidate = %candidate.id,
                "Skill patch candidate queue write failed"
            ),
        }
    }
    queued
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LiveSkillUseTraceReport {
    traces_recorded: usize,
    skill_stats_updated: usize,
}

async fn record_live_skill_use_traces(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    tools_used: &[String],
    tool_facts: &[TypedToolFact],
    tool_repairs: &[ToolRepairTrace],
    observed_at: chrono::DateTime<chrono::Utc>,
) -> LiveSkillUseTraceReport {
    if tools_used.is_empty() && tool_repairs.is_empty() {
        return LiveSkillUseTraceReport::default();
    }

    let activation_category = skill_activation_trace_memory_category();
    let activation_prefix = skill_activation_trace_memory_key_prefix(agent_id);
    let activation_entries = match mem.list(Some(&activation_category), None, 32).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                target: "post_turn",
                error = %e,
                agent_id = %agent_id,
                "Skill activation trace lookup failed"
            );
            return LiveSkillUseTraceReport::default();
        }
    };
    let activation_traces = activation_entries
        .iter()
        .filter(|entry| entry.key.starts_with(&activation_prefix))
        .filter_map(parse_skill_activation_trace_entry)
        .collect::<Vec<_>>();
    if activation_traces.is_empty() {
        return LiveSkillUseTraceReport::default();
    }

    let skills = match mem.list_skills(&agent_id.to_string(), 128).await {
        Ok(skills) => skills,
        Err(e) => {
            tracing::warn!(
                target: "post_turn",
                error = %e,
                agent_id = %agent_id,
                "Skill lookup failed during live use trace recording"
            );
            return LiveSkillUseTraceReport::default();
        }
    };
    if skills.is_empty() {
        return LiveSkillUseTraceReport::default();
    }

    let use_category = skill_use_trace_memory_category();
    let mut existing_keys = mem
        .list(Some(&use_category), None, 256)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.key)
        .collect::<HashSet<_>>();

    let observed_at_unix = observed_at.timestamp();
    let mut report = LiveSkillUseTraceReport::default();
    let mut seen_skill_ids = HashSet::new();

    for activation in activation_traces {
        for activation_id in &activation.loaded_skill_ids {
            let Some(skill) = find_memory_skill_by_activation_id(&skills, activation_id) else {
                continue;
            };
            if !seen_skill_ids.insert(skill.id.clone()) {
                continue;
            }
            if !skill_was_exercised_this_turn(skill, tools_used, tool_repairs) {
                continue;
            }

            let trace = build_skill_use_trace_from_live_turn(
                skill,
                &activation,
                tools_used,
                tool_facts,
                tool_repairs,
                observed_at_unix,
            );
            let key = skill_use_trace_memory_key(agent_id, &trace);
            if existing_keys.contains(&key) {
                continue;
            }
            let entry = match skill_use_trace_to_memory_entry(agent_id, &trace, observed_at, None) {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!(
                        target: "post_turn",
                        error = %e,
                        skill = %skill.name,
                        "Skill use trace serialization failed"
                    );
                    continue;
                }
            };
            match mem.store_episode(entry).await {
                Ok(_) => {
                    existing_keys.insert(key);
                    report.traces_recorded += 1;
                    if apply_live_skill_use_feedback(mem, agent_id, skill, trace.outcome).await {
                        report.skill_stats_updated += 1;
                    }
                }
                Err(e) => tracing::warn!(
                    target: "post_turn",
                    error = %e,
                    agent_id = %agent_id,
                    skill = %skill.name,
                    "Skill use trace write failed"
                ),
            }
        }
    }

    report
}

async fn apply_live_skill_use_feedback(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    skill: &Skill,
    outcome: SkillUseOutcome,
) -> bool {
    if skill.origin != SkillOrigin::Learned || skill.status != SkillStatus::Active {
        return false;
    }
    let (increment_success, increment_fail) = match outcome {
        SkillUseOutcome::Succeeded | SkillUseOutcome::Repaired => (true, false),
        SkillUseOutcome::Failed => (false, true),
    };
    match mem
        .update_skill(
            &skill.id,
            SkillUpdate {
                increment_success,
                increment_fail,
                new_description: None,
                new_content: None,
                new_task_family: None,
                new_tool_pattern: None,
                new_lineage_task_families: None,
                new_tags: None,
                new_status: None,
            },
            &agent_id.to_string(),
        )
        .await
    {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                target: "post_turn",
                error = %e,
                agent_id = %agent_id,
                skill = %skill.name,
                outcome = ?outcome,
                "Skill use feedback update failed"
            );
            false
        }
    }
}

fn skill_allows_patch_candidate(skill: &Skill) -> bool {
    skill.origin == SkillOrigin::Learned && skill.status != SkillStatus::Deprecated
}

fn relevant_repairs_for_skill(
    skill: &Skill,
    tool_repairs: &[ToolRepairTrace],
) -> Vec<ToolRepairTrace> {
    if skill.tool_pattern.is_empty() {
        return Vec::new();
    }
    let skill_tools = skill
        .tool_pattern
        .iter()
        .map(|tool| tool.trim().to_lowercase())
        .collect::<HashSet<_>>();
    tool_repairs
        .iter()
        .filter(|trace| skill_tools.contains(&trace.tool_name.trim().to_lowercase()))
        .cloned()
        .collect()
}

fn find_memory_skill_by_activation_id<'a>(
    skills: &'a [Skill],
    activation_id: &str,
) -> Option<&'a Skill> {
    let normalized_activation_id = normalize_tool_match_value(activation_id);
    skills.iter().find(|skill| {
        normalize_tool_match_value(&skill.id) == normalized_activation_id
            || (skill.id.trim().is_empty()
                && normalize_tool_match_value(&skill.name) == normalized_activation_id)
    })
}

fn skill_was_exercised_this_turn(
    skill: &Skill,
    tools_used: &[String],
    tool_repairs: &[ToolRepairTrace],
) -> bool {
    if skill.tool_pattern.is_empty() {
        return false;
    }
    let skill_tools = skill
        .tool_pattern
        .iter()
        .map(|tool| normalize_tool_match_value(tool))
        .filter(|tool| !tool.is_empty())
        .collect::<HashSet<_>>();
    tools_used
        .iter()
        .any(|tool| skill_tools.contains(&normalize_tool_match_value(tool)))
        || tool_repairs
            .iter()
            .any(|trace| skill_tools.contains(&normalize_tool_match_value(&trace.tool_name)))
}

fn normalize_tool_match_value(value: &str) -> String {
    value.trim().to_lowercase()
}

async fn apply_decision_with_runtime_trace(
    mem: &dyn UnifiedMemoryPort,
    decision: &MutationDecision,
    agent_id: &str,
    source_override: Option<&str>,
) -> (Option<LearningEvent>, RuntimeTraceMemoryDecision) {
    let observed_at_unix = chrono::Utc::now().timestamp();
    match mutation::apply_decision_with_event(mem, decision, agent_id).await {
        Ok(event) => {
            let applied = !decision.action.is_noop();
            let trace = runtime_memory_decision_from_mutation(
                decision,
                observed_at_unix,
                source_override,
                applied,
                event.entry_id.as_deref(),
                None,
            );
            (Some(event), trace)
        }
        Err(error) => {
            tracing::warn!(
                target: "post_turn",
                error = %error,
                action = ?decision.action,
                "Memory mutation failed"
            );
            let error = error.to_string();
            let trace = runtime_memory_decision_from_mutation(
                decision,
                observed_at_unix,
                source_override,
                false,
                None,
                Some(&error),
            );
            (None, trace)
        }
    }
}

fn build_runtime_auxiliary_decisions(
    report: &PostTurnReport,
    auto_save_enabled: bool,
    allow_background_learning: bool,
    should_consolidate: bool,
    should_reflect: bool,
    observed_at_unix: i64,
) -> Vec<RuntimeTraceAuxiliaryDecision> {
    let mut decisions = Vec::new();
    if should_consolidate || report.consolidation_started {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "consolidation",
            if report.consolidation_started {
                "started"
            } else {
                "attempted"
            },
            usize::from(report.consolidation_started),
            None,
        ));
    } else if auto_save_enabled && !allow_background_learning {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "consolidation",
            "suppressed",
            0,
            Some("background_learning_governor"),
        ));
    }
    if should_reflect || report.reflection_started {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "reflection",
            if report.reflection_started {
                "started"
            } else {
                "attempted"
            },
            usize::from(report.reflection_started),
            None,
        ));
    }
    if report.run_recipes_upserted > 0 {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "run_recipe",
            "upserted",
            report.run_recipes_upserted,
            None,
        ));
    }
    if report.run_recipes_removed > 0 {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "run_recipe",
            "removed_redundant",
            report.run_recipes_removed,
            None,
        ));
    }
    if !report.skill_promotion_assessments.is_empty() {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "skill_promotion",
            "assessed",
            report.skill_promotion_assessments.len(),
            None,
        ));
    }
    if report.skills_upserted > 0 {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "skill_promotion",
            "upserted",
            report.skills_upserted,
            None,
        ));
    }
    if report.skills_penalized > 0 {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "skill_feedback",
            "penalized",
            report.skills_penalized,
            None,
        ));
    }
    if report.skill_use_traces_recorded > 0 {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "skill_use_trace",
            "recorded",
            report.skill_use_traces_recorded,
            None,
        ));
    }
    if report.skill_use_feedback_updates > 0 {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "skill_use_feedback",
            "updated",
            report.skill_use_feedback_updates,
            None,
        ));
    }
    if report.user_profile_updated {
        decisions.push(runtime_auxiliary_decision(
            observed_at_unix,
            "user_profile",
            "updated",
            1,
            None,
        ));
    }
    decisions
}

fn publish_runtime_trace_fragments(
    sink: &PostTurnRuntimeTraceSink,
    report: &PostTurnReport,
    now_unix: i64,
) {
    if report.runtime_memory_decisions.is_empty() && report.runtime_auxiliary_decisions.is_empty() {
        return;
    }
    let mut route = sink.routes.get_route(&sink.conversation_key);
    let updated = merge_runtime_decision_trace_update(
        &route.runtime_decision_traces,
        &sink.trace_id,
        RuntimeDecisionTraceUpdate {
            memory: report.runtime_memory_decisions.clone(),
            auxiliary: report.runtime_auxiliary_decisions.clone(),
            ..Default::default()
        },
        now_unix,
        RUNTIME_TRACE_JANITOR_TTL_SECS,
    );
    if updated == route.runtime_decision_traces {
        tracing::debug!(
            target: "post_turn",
            trace_id = %sink.trace_id,
            "Post-turn runtime trace sink found no matching active trace"
        );
        return;
    }
    route.runtime_decision_traces = updated;
    sink.routes.set_route(&sink.conversation_key, route);
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
        MemoryQuery, Reflection, SearchResult, SessionId, Skill, SkillOrigin, SkillStatus,
        SkillUpdate, TemporalFact, Visibility,
    };
    use crate::domain::tool_fact::{
        OutcomeStatus, ProfileOperation, ResourceFact, ResourceKind, ResourceMetadata,
        ResourceOperation, SearchDomain, SearchFact, ToolFactPayload, TypedToolFact,
        UserProfileFact,
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
        async fn store_episode(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError> {
            let id = if entry.id.trim().is_empty() {
                entry.key.clone()
            } else {
                entry.id.clone()
            };
            self.entries.write().push(entry);
            Ok(id)
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
    async fn repeated_live_repair_traces_queue_skill_patch_candidate() {
        let memory = StubMemory::default();
        memory
            .store_skill(Skill {
                id: "skill-matrix-upgrade".into(),
                name: "Matrix Upgrade".into(),
                description: "Upgrade self-hosted Matrix safely".into(),
                content: "# Matrix Upgrade\n\nCheck the current version first.".into(),
                task_family: Some("matrix-upgrade".into()),
                tool_pattern: vec!["shell".into()],
                lineage_task_families: Vec::new(),
                tags: vec!["ops".into()],
                success_count: 3,
                fail_count: 0,
                version: 2,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Active,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let repair = |observed_at_unix| crate::domain::tool_repair::ToolRepairTrace {
            observed_at_unix,
            tool_name: "shell".into(),
            failure_kind: crate::domain::tool_repair::ToolFailureKind::MissingResource,
            suggested_action: crate::domain::tool_repair::ToolRepairAction::AdjustArgumentsOrTarget,
            repair_outcome: crate::domain::tool_repair::ToolRepairOutcome::Resolved,
            detail: Some("matrix repository path had moved before upgrade".into()),
            ..crate::domain::tool_repair::ToolRepairTrace::default()
        };

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message:
                    "Please remember the repeated Matrix upgrade repair path after this run".into(),
                assistant_response:
                    "The Matrix upgrade completed after adjusting the repository path.".into(),
                tools_used: vec!["shell".into()],
                tool_facts: Vec::new(),
                tool_repairs: vec![repair(100), repair(200)],
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
            },
        )
        .await;

        assert_eq!(report.skill_patch_candidates_queued, 1);
        assert_eq!(report.skill_patch_candidates.len(), 1);
        let category = skill_patch_candidate_service::skill_patch_candidate_memory_category();
        let entries = memory.list(Some(&category), None, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        let queued = skill_patch_candidate_service::parse_skill_patch_candidate_entry(&entries[0])
            .expect("queued patch candidate");
        assert_eq!(queued.status, SkillStatus::Candidate);
        assert_eq!(queued.target_skill_id, "skill-matrix-upgrade");
    }

    #[tokio::test]
    async fn active_skill_use_trace_records_live_typed_outcome() {
        let memory = StubMemory::default();
        memory
            .store_skill(Skill {
                id: "skill-matrix-upgrade".into(),
                name: "Matrix Upgrade".into(),
                description: "Upgrade self-hosted Matrix safely".into(),
                content: "# Matrix Upgrade\n\nCheck the current version first.".into(),
                task_family: Some("matrix-upgrade".into()),
                tool_pattern: vec!["shell".into()],
                lineage_task_families: Vec::new(),
                tags: vec!["ops".into()],
                success_count: 3,
                fail_count: 0,
                version: 2,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Active,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let activation =
            crate::application::services::skill_governance_service::SkillActivationTrace {
                selected_skill_ids: vec!["skill-matrix-upgrade".into()],
                loaded_skill_ids: vec!["skill-matrix-upgrade".into()],
                blocked_skill_ids: Vec::new(),
                blocked_reasons: Vec::new(),
                budget_catalog_entries: 1,
                budget_preloaded_skills: 0,
                route_model: Some("deepseek".into()),
                outcome: Some("loaded".into()),
            };
        let activation_entry =
            crate::application::services::skill_trace_service::skill_activation_trace_to_memory_entry(
                "agent",
                &activation,
                chrono::Utc::now(),
                None,
            )
            .unwrap();
        memory.store_episode(activation_entry).await.unwrap();

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message:
                    "Check the Matrix deployment version and report whether it needs an upgrade"
                        .into(),
                assistant_response:
                    "The version check completed after adjusting the repository path.".into(),
                tools_used: vec!["shell".into()],
                tool_facts: vec![TypedToolFact::outcome(
                    "shell",
                    OutcomeStatus::Succeeded,
                    Some(40),
                )],
                tool_repairs: vec![crate::domain::tool_repair::ToolRepairTrace {
                    observed_at_unix: 200,
                    tool_name: "shell".into(),
                    failure_kind: crate::domain::tool_repair::ToolFailureKind::MissingResource,
                    suggested_action:
                        crate::domain::tool_repair::ToolRepairAction::AdjustArgumentsOrTarget,
                    repair_outcome: crate::domain::tool_repair::ToolRepairOutcome::Resolved,
                    ..crate::domain::tool_repair::ToolRepairTrace::default()
                }],
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
            },
        )
        .await;

        assert_eq!(report.skill_use_traces_recorded, 1);
        assert_eq!(report.skill_use_feedback_updates, 1);
        assert!(report.runtime_auxiliary_decisions.iter().any(|decision| {
            decision.kind == "skill_use_trace"
                && decision.action == "recorded"
                && decision.count == 1
        }));
        assert!(report.runtime_auxiliary_decisions.iter().any(|decision| {
            decision.kind == "skill_use_feedback"
                && decision.action == "updated"
                && decision.count == 1
        }));
        assert_eq!(memory.skills.read()[0].success_count, 4);
        assert_eq!(memory.skills.read()[0].fail_count, 0);
        let category =
            crate::application::services::skill_trace_service::skill_use_trace_memory_category();
        let entries = memory.list(Some(&category), None, 10).await.unwrap();
        assert_eq!(entries.len(), 1);
        let parsed =
            crate::application::services::skill_trace_service::parse_skill_use_trace_entry(
                &entries[0],
            )
            .expect("skill use trace");
        assert_eq!(parsed.skill_id, "skill-matrix-upgrade");
        assert_eq!(
            parsed.outcome,
            crate::application::services::skill_governance_service::SkillUseOutcome::Repaired
        );
        assert_eq!(parsed.route_model.as_deref(), Some("deepseek"));
        assert!(!entries[0].content.contains("# Matrix Upgrade"));
    }

    #[tokio::test]
    async fn active_skill_use_trace_failure_updates_learned_skill_fail_count() {
        let memory = StubMemory::default();
        memory
            .store_skill(Skill {
                id: "skill-matrix-upgrade".into(),
                name: "Matrix Upgrade".into(),
                description: "Upgrade self-hosted Matrix safely".into(),
                content: "# Matrix Upgrade\n\nCheck the current version first.".into(),
                task_family: Some("matrix-upgrade".into()),
                tool_pattern: vec!["shell".into()],
                lineage_task_families: Vec::new(),
                tags: vec!["ops".into()],
                success_count: 3,
                fail_count: 0,
                version: 2,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Active,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let activation =
            crate::application::services::skill_governance_service::SkillActivationTrace {
                selected_skill_ids: vec!["skill-matrix-upgrade".into()],
                loaded_skill_ids: vec!["skill-matrix-upgrade".into()],
                blocked_skill_ids: Vec::new(),
                blocked_reasons: Vec::new(),
                budget_catalog_entries: 1,
                budget_preloaded_skills: 0,
                route_model: None,
                outcome: Some("loaded".into()),
            };
        let activation_entry =
            crate::application::services::skill_trace_service::skill_activation_trace_to_memory_entry(
                "agent",
                &activation,
                chrono::Utc::now(),
                None,
            )
            .unwrap();
        memory.store_episode(activation_entry).await.unwrap();

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Check the Matrix deployment version again".into(),
                assistant_response: "The typed shell outcome reported a runtime failure.".into(),
                tools_used: vec!["shell".into()],
                tool_facts: vec![TypedToolFact::outcome(
                    "shell",
                    OutcomeStatus::RuntimeError,
                    Some(40),
                )],
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
            },
        )
        .await;

        assert_eq!(report.skill_use_traces_recorded, 1);
        assert_eq!(report.skill_use_feedback_updates, 1);
        assert_eq!(memory.skills.read()[0].success_count, 3);
        assert_eq!(memory.skills.read()[0].fail_count, 1);
        let category =
            crate::application::services::skill_trace_service::skill_use_trace_memory_category();
        let entries = memory.list(Some(&category), None, 10).await.unwrap();
        let parsed =
            crate::application::services::skill_trace_service::parse_skill_use_trace_entry(
                &entries[0],
            )
            .expect("skill use trace");
        assert_eq!(
            parsed.outcome,
            crate::application::services::skill_governance_service::SkillUseOutcome::Failed
        );
        assert!(parsed
            .verification
            .as_deref()
            .is_some_and(|value| value.contains("failure_outcomes=1")));
    }

    #[tokio::test]
    async fn auto_updates_structured_user_profile_from_learning_candidates() {
        let memory = StubMemory::default();
        let store = Arc::new(InMemoryUserProfileStore::new());

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Remember my project alias".into(),
                assistant_response: "Saved your project alias.".into(),
                tools_used: vec!["user_profile".into()],
                tool_facts: vec![TypedToolFact {
                    tool_id: "user_profile".into(),
                    payload: ToolFactPayload::UserProfile(UserProfileFact {
                        key: "project_alias".into(),
                        operation: ProfileOperation::Set,
                        value: Some("Borealis".into()),
                    }),
                }],
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: Some(store.clone()),
                user_profile_key: Some("web:test".into()),
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
            },
        )
        .await;

        assert!(report.user_profile_updated);
        assert_eq!(
            store
                .load("web:test")
                .and_then(|profile| profile.get_text("project_alias")),
            Some("Borealis".into())
        );
    }

    #[tokio::test]
    async fn skips_user_profile_write_when_learning_patch_changes_nothing() {
        let memory = StubMemory::default();
        let store = Arc::new(InMemoryUserProfileStore::new());
        store
            .upsert("web:test", {
                let mut profile = UserProfile::default();
                profile.set("project_alias", serde_json::json!("Borealis"));
                profile
            })
            .unwrap();

        let report = execute_post_turn_learning(
            &memory,
            PostTurnInput {
                agent_id: "agent".into(),
                user_message: "Remember my project alias".into(),
                assistant_response: "Saved your project alias.".into(),
                tools_used: vec!["user_profile".into()],
                tool_facts: vec![TypedToolFact {
                    tool_id: "user_profile".into(),
                    payload: ToolFactPayload::UserProfile(UserProfileFact {
                        key: "project_alias".into(),
                        operation: ProfileOperation::Set,
                        value: Some("Borealis".into()),
                    }),
                }],
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: Some(store),
                user_profile_key: Some("web:test".into()),
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                user_message: "Remember both project aliases".into(),
                assistant_response: "Captured conflicting project aliases.".into(),
                tools_used: vec!["user_profile".into()],
                tool_facts: vec![
                    TypedToolFact {
                        tool_id: "user_profile".into(),
                        payload: ToolFactPayload::UserProfile(UserProfileFact {
                            key: "project_alias".into(),
                            operation: ProfileOperation::Set,
                            value: Some("Borealis".into()),
                        }),
                    },
                    TypedToolFact {
                        tool_id: "user_profile".into(),
                        payload: ToolFactPayload::UserProfile(UserProfileFact {
                            key: "project_alias".into(),
                            operation: ProfileOperation::Set,
                            value: Some("Atlas".into()),
                        }),
                    },
                ],
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: Some(store.clone()),
                user_profile_key: Some("web:test".into()),
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                user_message: "Let's continue the reflective memory-only discussion"
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
                            query: Some("reflective_memory_topic".into()),
                            result_count: Some(3),
                            primary_locator: Some("daily_123".into()),
                        }),
                    },
                    TypedToolFact::focus(
                        "memory_recall",
                        vec![FocusEntity {
                            kind: "topic".into(),
                            name: "reflective_memory_topic".into(),
                            metadata: None,
                        }],
                        Vec::new(),
                    ),
                ],
                tool_repairs: Vec::new(),
                run_recipe_store: Some(run_recipe_store),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                user_message: "I want to keep exploring a reflective memory topic through responsibility, memory, and how a person changes over time.".into(),
                assistant_response:
                    "We can treat the topic as something partially discovered and partially constructed through repeated commitments."
                        .into(),
                tools_used: vec![],
                tool_facts: vec![],
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                tool_repairs: Vec::new(),
                run_recipe_store: Some(run_recipe_store),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                tool_repairs: Vec::new(),
                run_recipe_store: None,
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                tool_repairs: Vec::new(),
                run_recipe_store: Some(run_recipe_store),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                tool_repairs: Vec::new(),
                run_recipe_store: Some(run_recipe_store.clone()),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
                tool_repairs: Vec::new(),
                run_recipe_store: Some(run_recipe_store),
                user_profile_store: None,
                user_profile_key: None,
                auto_save_enabled: true,
                event_tx: None,
                runtime_trace_sink: None,
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
