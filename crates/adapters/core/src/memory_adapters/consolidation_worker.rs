//! Background memory consolidation worker.
//!
//! Runs as a tokio task in the daemon, periodically:
//! 1. Importance decay on old episodes
//! 2. Garbage collection of low-importance entries
//! 3. Prompt optimization (Phase 4.4) — analyzes reflections, improves instructions

use std::sync::Arc;
use std::time::Duration;
use synapse_domain::application::services::learning_compaction_service::{
    compact_selected_near_duplicates, compact_selected_near_duplicates_with_failures,
    DuplicateCompactionThresholds,
};
use synapse_domain::application::services::learning_maintenance_service::{
    build_learning_maintenance_plan, LearningMaintenancePolicy, LearningMaintenanceSnapshot,
};
use synapse_domain::application::services::procedural_cluster_review_service::{
    representative_keys_for_action, review_failure_pattern_clusters, review_precedent_clusters,
    ProceduralClusterReviewAction,
};
use synapse_domain::application::services::procedural_cluster_service::{
    plan_recent_clusters, plan_recent_clusters_since, ProceduralClusterKind,
};
use synapse_domain::application::services::procedural_contradiction_service::find_recipe_failure_contradictions;
use synapse_domain::application::services::run_recipe_cluster_service::plan_recipe_clusters;
use synapse_domain::application::services::run_recipe_review_service::{
    review_run_recipes_with_failures, RunRecipeReviewThresholds,
};
use synapse_domain::application::services::skill_review_service::review_learned_skills_with_failures;
use synapse_domain::domain::memory::{MemoryCategory, MemoryEntry, SkillUpdate};
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;

/// Configuration for the consolidation worker.
#[derive(Debug, Clone)]
pub struct ConsolidationWorkerConfig {
    /// Interval between consolidation cycles (decay + GC).
    pub interval: Duration,
    /// How far back to look for recent learning activity.
    pub activity_window: Duration,
    /// How many recent entries per category to probe when building a cheap snapshot.
    pub activity_probe_limit: usize,
    /// Importance decay threshold.
    pub gc_importance_threshold: f32,
    /// Max age in days for GC candidates.
    pub gc_max_age_days: u32,
    /// Force a maintenance cycle after this many idle skips.
    pub max_idle_cycles_before_forced_maintenance: u32,
    /// Interval between prompt optimization cycles.
    pub optimization_interval: Duration,
    /// Minimum reflections needed before optimization runs.
    pub min_reflections_for_optimization: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RunRecipeReviewOutcome {
    removed: usize,
    blocked_clusters: usize,
}

impl Default for ConsolidationWorkerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3600),         // 1 hour
            activity_window: Duration::from_secs(21600), // 6 hours
            activity_probe_limit: 12,
            gc_importance_threshold: 0.05,
            gc_max_age_days: 30,
            max_idle_cycles_before_forced_maintenance: 6,
            optimization_interval: Duration::from_secs(21600), // 6 hours
            min_reflections_for_optimization: 3,
        }
    }
}

/// Spawn the consolidation worker as a background tokio task.
///
/// When `provider` is Some, prompt optimization runs every `optimization_interval`.
pub fn spawn_consolidation_worker(
    memory: Arc<dyn UnifiedMemoryPort>,
    run_recipe_store: Arc<dyn RunRecipeStorePort>,
    config: ConsolidationWorkerConfig,
    agent_id: String,
    provider: Option<(Arc<dyn synapse_providers::traits::Provider>, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.interval);
        interval.tick().await; // skip first immediate tick

        let mut last_optimization = std::time::Instant::now();
        let mut skipped_cycles_since_maintenance = 0_u32;
        let maintenance_policy = LearningMaintenancePolicy {
            min_recent_learning_activity: 1,
            max_skipped_cycles_before_forced_maintenance: config
                .max_idle_cycles_before_forced_maintenance,
            min_reflections_for_prompt_optimization: config.min_reflections_for_optimization,
            ..LearningMaintenancePolicy::default()
        };

        loop {
            interval.tick().await;
            let prompt_optimization_due =
                provider.is_some() && last_optimization.elapsed() >= config.optimization_interval;
            let snapshot = sample_learning_maintenance_snapshot(
                memory.as_ref(),
                run_recipe_store.as_ref(),
                &agent_id,
                config.activity_probe_limit,
                config.activity_window,
                skipped_cycles_since_maintenance,
                prompt_optimization_due,
            )
            .await;
            let plan = build_learning_maintenance_plan(&snapshot, &maintenance_policy);

            if !plan.should_run_any() {
                if plan.has_any_advisory_action() {
                    tracing::debug!(
                        reasons = ?plan.reasons,
                        "Memory consolidation advisory backlog present; no executable maintenance step registered yet"
                    );
                }
                skipped_cycles_since_maintenance =
                    skipped_cycles_since_maintenance.saturating_add(1);
                tracing::debug!(
                    skipped_cycles_since_maintenance,
                    "Memory consolidation cycle skipped: no fresh learning activity"
                );
                continue;
            }

            tracing::debug!(
                reasons = ?plan.reasons,
                "Memory consolidation cycle starting"
            );

            if plan.run_importance_decay {
                match memory.recalculate_importance(&agent_id).await {
                    Ok(_) => tracing::debug!("Importance decay applied"),
                    Err(e) => tracing::debug!("Importance decay failed: {e}"),
                }
            }

            if plan.run_gc {
                match memory
                    .gc_low_importance(config.gc_importance_threshold, config.gc_max_age_days)
                    .await
                {
                    Ok(count) => {
                        if count > 0 {
                            tracing::info!("Memory GC: {count} entries removed");
                        }
                    }
                    Err(e) => tracing::debug!("Memory GC failed: {e}"),
                }
            }

            if plan.run_run_recipe_review {
                match run_recipe_review(
                    memory.as_ref(),
                    run_recipe_store.as_ref(),
                    &agent_id,
                    config.activity_probe_limit * 4,
                    config.activity_window,
                )
                .await
                {
                    Ok(outcome) if outcome == RunRecipeReviewOutcome::default() => {
                        tracing::debug!("Run recipe review found no actionable clusters")
                    }
                    Ok(outcome) => {
                        if outcome.removed > 0 {
                            tracing::info!(
                                count = outcome.removed,
                                "Run recipe review removed duplicates"
                            );
                        }
                        if outcome.blocked_clusters > 0 {
                            tracing::info!(
                                count = outcome.blocked_clusters,
                                "Run recipe review found contradiction-blocked clusters"
                            );
                        }
                    }
                    Err(e) => tracing::debug!("Run recipe review failed: {e}"),
                }
            }

            if plan.run_precedent_compaction {
                let recent_failure_clusters = match plan_recent_clusters(
                    memory.as_ref(),
                    &agent_id,
                    ProceduralClusterKind::FailurePattern,
                    config.activity_probe_limit * 4,
                    6,
                    0.96,
                )
                .await
                {
                    Ok(clusters) => clusters,
                    Err(e) => {
                        tracing::debug!(
                            "Failed to load failure clusters for precedent compaction: {e}"
                        );
                        Vec::new()
                    }
                };
                let recent_precedent_clusters = match plan_recent_clusters(
                    memory.as_ref(),
                    &agent_id,
                    ProceduralClusterKind::Precedent,
                    config.activity_probe_limit * 4,
                    6,
                    0.95,
                )
                .await
                {
                    Ok(clusters) => clusters,
                    Err(e) => {
                        tracing::debug!(
                            "Failed to load precedent clusters for precedent compaction: {e}"
                        );
                        Vec::new()
                    }
                };
                let precedent_reviews =
                    review_precedent_clusters(&recent_precedent_clusters, &recent_failure_clusters);
                let compact_candidate_keys = representative_keys_for_action(
                    &precedent_reviews,
                    "precedent",
                    ProceduralClusterReviewAction::CompactCandidate,
                );
                match compact_selected_near_duplicates_with_failures(
                    memory.as_ref(),
                    &agent_id,
                    MemoryCategory::Custom("precedent".into()),
                    &compact_candidate_keys,
                    config.activity_probe_limit * 4,
                    &DuplicateCompactionThresholds::precedent_defaults(),
                    &recent_failure_clusters,
                )
                .await
                {
                    Ok(0) => tracing::debug!("Precedent compaction found no duplicates"),
                    Ok(count) => tracing::info!(count, "Precedent compaction removed duplicates"),
                    Err(e) => tracing::debug!("Precedent compaction failed: {e}"),
                }
            }

            if plan.run_failure_pattern_compaction {
                let recent_failure_clusters = match plan_recent_clusters(
                    memory.as_ref(),
                    &agent_id,
                    ProceduralClusterKind::FailurePattern,
                    config.activity_probe_limit * 4,
                    6,
                    0.96,
                )
                .await
                {
                    Ok(clusters) => clusters,
                    Err(e) => {
                        tracing::debug!(
                            "Failed to load failure clusters for failure-pattern compaction: {e}"
                        );
                        Vec::new()
                    }
                };
                let failure_pattern_reviews = review_failure_pattern_clusters(
                    &recent_failure_clusters,
                    &find_recipe_failure_contradictions(
                        &plan_recipe_clusters(
                            &run_recipe_store.list_recent(
                                &agent_id,
                                (chrono::Utc::now().timestamp().max(0) as u64)
                                    .saturating_sub(config.activity_window.as_secs()),
                            ),
                            0.9,
                        ),
                        &recent_failure_clusters,
                        0.75,
                    ),
                );
                let compact_candidate_keys = representative_keys_for_action(
                    &failure_pattern_reviews,
                    "failure_pattern",
                    ProceduralClusterReviewAction::CompactCandidate,
                );
                match compact_selected_near_duplicates(
                    memory.as_ref(),
                    &agent_id,
                    MemoryCategory::Custom("failure_pattern".into()),
                    &compact_candidate_keys,
                    config.activity_probe_limit * 4,
                    &DuplicateCompactionThresholds::failure_pattern_defaults(),
                )
                .await
                {
                    Ok(0) => tracing::debug!("Failure-pattern compaction found no duplicates"),
                    Ok(count) => {
                        tracing::info!(count, "Failure-pattern compaction removed duplicates")
                    }
                    Err(e) => tracing::debug!("Failure-pattern compaction failed: {e}"),
                }
            }

            if plan.run_skill_review {
                match run_skill_review(
                    memory.as_ref(),
                    run_recipe_store.as_ref(),
                    &agent_id,
                    config.activity_probe_limit * 4,
                    config.activity_window,
                )
                .await
                {
                    Ok(0) => tracing::debug!("Learning skill review found no changes"),
                    Ok(count) => {
                        tracing::info!(count, "Learning skill review applied updates")
                    }
                    Err(e) => tracing::debug!("Learning skill review failed: {e}"),
                }
            }

            if let Some((ref prov, ref model)) = provider {
                if plan.run_prompt_optimization {
                    match super::prompt_optimizer::optimize_prompt(
                        prov.as_ref(),
                        model,
                        memory.as_ref(),
                        &agent_id,
                        config.min_reflections_for_optimization,
                    )
                    .await
                    {
                        Ok(Some(opt)) => {
                            tracing::info!(
                                changes = opt.changes.len(),
                                reflections = opt.reflections_analyzed,
                                "prompt.optimization.cycle_complete"
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("prompt.optimization.failed: {e}");
                        }
                    }
                    last_optimization = std::time::Instant::now();
                }
            }

            skipped_cycles_since_maintenance = 0;

            tracing::debug!("Memory consolidation cycle complete");
        }
    })
}

pub(crate) async fn sample_learning_maintenance_snapshot(
    memory: &dyn UnifiedMemoryPort,
    run_recipe_store: &dyn RunRecipeStorePort,
    agent_id: &str,
    probe_limit: usize,
    activity_window: Duration,
    skipped_cycles_since_maintenance: u32,
    prompt_optimization_due: bool,
) -> LearningMaintenanceSnapshot {
    let recent_run_recipe_cutoff =
        (chrono::Utc::now().timestamp().max(0) as u64).saturating_sub(activity_window.as_secs());
    let recent_cutoff =
        chrono::Utc::now() - chrono::Duration::seconds(activity_window.as_secs() as i64);
    let recent_run_recipes = run_recipe_store.list_recent(agent_id, recent_run_recipe_cutoff);
    let recent_precedents = list_recent_category_entries(
        memory,
        MemoryCategory::Custom("precedent".into()),
        probe_limit,
        recent_cutoff,
    )
    .await;
    let recent_reflections = list_recent_category_entries(
        memory,
        MemoryCategory::Reflection,
        probe_limit,
        recent_cutoff,
    )
    .await;
    let recent_failure_patterns = list_recent_category_entries(
        memory,
        MemoryCategory::Custom("failure_pattern".into()),
        probe_limit,
        recent_cutoff,
    )
    .await;
    let precedent_clusters = plan_recent_clusters_since(
        memory,
        agent_id,
        ProceduralClusterKind::Precedent,
        probe_limit,
        6,
        0.95,
        Some(recent_cutoff),
    )
    .await
    .unwrap_or_default();
    let failure_pattern_clusters = plan_recent_clusters_since(
        memory,
        agent_id,
        ProceduralClusterKind::FailurePattern,
        probe_limit,
        6,
        0.96,
        Some(recent_cutoff),
    )
    .await
    .unwrap_or_default();
    let recent_skills = memory
        .list_recent_skills(&agent_id.to_string(), probe_limit, recent_cutoff)
        .await
        .unwrap_or_default();
    let recent_learned_skills = recent_skills
        .iter()
        .filter(|skill| skill.origin == synapse_domain::domain::memory::SkillOrigin::Learned)
        .collect::<Vec<_>>();
    let recipe_clusters = plan_recipe_clusters(&recent_run_recipes, 0.9);
    let procedural_contradictions =
        find_recipe_failure_contradictions(&recipe_clusters, &failure_pattern_clusters, 0.75);
    let precedent_cluster_reviews =
        review_precedent_clusters(&precedent_clusters, &failure_pattern_clusters);
    let failure_pattern_cluster_reviews =
        review_failure_pattern_clusters(&failure_pattern_clusters, &procedural_contradictions);

    LearningMaintenanceSnapshot {
        recent_run_recipe_count: recent_run_recipes.len(),
        run_recipe_cluster_count: recipe_clusters.len(),
        procedural_contradiction_count: procedural_contradictions.len(),
        recent_precedent_count: recent_precedents.len(),
        precedent_cluster_count: precedent_clusters.len(),
        precedent_compact_candidate_count: precedent_cluster_reviews
            .iter()
            .filter(|review| {
                review.action == ProceduralClusterReviewAction::CompactCandidate
                    && review.kind == "precedent"
            })
            .count(),
        precedent_preserve_branch_count: precedent_cluster_reviews
            .iter()
            .filter(|review| {
                review.action == ProceduralClusterReviewAction::PreserveBranch
                    && review.kind == "precedent"
            })
            .count(),
        recent_reflection_count: recent_reflections.len(),
        recent_failure_pattern_count: recent_failure_patterns.len(),
        failure_pattern_cluster_count: failure_pattern_clusters.len(),
        failure_pattern_compact_candidate_count: failure_pattern_cluster_reviews
            .iter()
            .filter(|review| {
                review.action == ProceduralClusterReviewAction::CompactCandidate
                    && review.kind == "failure_pattern"
            })
            .count(),
        failure_pattern_blocking_count: failure_pattern_cluster_reviews
            .iter()
            .filter(|review| {
                review.action == ProceduralClusterReviewAction::BlocksProceduralPaths
                    && review.kind == "failure_pattern"
            })
            .count(),
        recent_skill_count: recent_learned_skills.len(),
        candidate_skill_count: recent_learned_skills
            .iter()
            .filter(|skill| skill.status == synapse_domain::domain::memory::SkillStatus::Candidate)
            .count(),
        skipped_cycles_since_maintenance,
        prompt_optimization_due,
    }
}

async fn run_recipe_review(
    memory: &dyn UnifiedMemoryPort,
    store: &dyn RunRecipeStorePort,
    agent_id: &str,
    limit: usize,
    activity_window: Duration,
) -> anyhow::Result<RunRecipeReviewOutcome> {
    let failure_clusters = plan_recent_clusters(
        memory,
        agent_id,
        ProceduralClusterKind::FailurePattern,
        limit,
        6,
        0.96,
    )
    .await?;
    let recent_cutoff =
        (chrono::Utc::now().timestamp().max(0) as u64).saturating_sub(activity_window.as_secs());
    let decisions = review_run_recipes_with_failures(
        &store.list_recent(agent_id, recent_cutoff),
        &failure_clusters,
        &RunRecipeReviewThresholds::default(),
    );
    let mut removed = 0;
    let mut blocked_clusters = 0;

    for decision in decisions {
        if decision.promotion_blocked {
            blocked_clusters += 1;
        }
        store.upsert(decision.canonical_recipe)?;
        for task_family in decision.removed_task_families {
            store.remove(agent_id, &task_family)?;
            removed += 1;
        }
    }

    Ok(RunRecipeReviewOutcome {
        removed,
        blocked_clusters,
    })
}

async fn list_recent_category_entries(
    memory: &dyn UnifiedMemoryPort,
    category: MemoryCategory,
    limit: usize,
    recent_cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<MemoryEntry> {
    memory
        .list_recent_scoped(Some(&category), None, limit, false, recent_cutoff)
        .await
        .unwrap_or_default()
}

async fn run_skill_review(
    memory: &dyn UnifiedMemoryPort,
    run_recipe_store: &dyn RunRecipeStorePort,
    agent_id: &str,
    limit: usize,
    activity_window: Duration,
) -> Result<usize, synapse_domain::domain::memory::MemoryError> {
    let agent_id = agent_id.to_string();
    let recent_cutoff =
        chrono::Utc::now() - chrono::Duration::seconds(activity_window.as_secs() as i64);
    let skills = memory
        .list_recent_skills(&agent_id, limit, recent_cutoff)
        .await?;
    let recent_recipe_cutoff =
        (chrono::Utc::now().timestamp().max(0) as u64).saturating_sub(activity_window.as_secs());
    let recipes = run_recipe_store.list_recent(&agent_id, recent_recipe_cutoff);
    let failure_clusters = plan_recent_clusters(
        memory,
        &agent_id,
        ProceduralClusterKind::FailurePattern,
        limit,
        6,
        0.96,
    )
    .await?;
    let decisions = review_learned_skills_with_failures(&skills, &recipes, &failure_clusters);
    let mut applied = 0;

    for decision in decisions {
        memory
            .update_skill(
                &decision.skill_id,
                SkillUpdate {
                    increment_success: false,
                    increment_fail: false,
                    new_description: None,
                    new_content: None,
                    new_task_family: None,
                    new_tool_pattern: None,
                    new_status: Some(decision.target_status),
                },
                &agent_id,
            )
            .await?;
        applied += 1;
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ConsolidationWorkerConfig::default();
        assert_eq!(config.interval, Duration::from_secs(3600));
        assert_eq!(config.activity_window, Duration::from_secs(21600));
        assert_eq!(config.activity_probe_limit, 12);
        assert!((config.gc_importance_threshold - 0.05).abs() < 0.001);
        assert_eq!(config.gc_max_age_days, 30);
        assert_eq!(config.max_idle_cycles_before_forced_maintenance, 6);
        assert_eq!(config.optimization_interval, Duration::from_secs(21600));
        assert_eq!(config.min_reflections_for_optimization, 3);
    }
}
