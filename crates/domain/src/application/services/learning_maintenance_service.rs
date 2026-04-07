//! Background learning and maintenance gating.
//!
//! Keeps deferred learning work cheap by deciding when background maintenance
//! should actually run, instead of blindly executing every cycle.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningMaintenanceSnapshot {
    pub recent_precedent_count: usize,
    pub recent_reflection_count: usize,
    pub recent_failure_pattern_count: usize,
    pub recent_skill_count: usize,
    pub candidate_skill_count: usize,
    pub skipped_cycles_since_maintenance: u32,
    pub prompt_optimization_due: bool,
}

impl LearningMaintenanceSnapshot {
    pub fn recent_learning_activity_count(&self) -> usize {
        self.recent_precedent_count
            + self.recent_reflection_count
            + self.recent_failure_pattern_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearningMaintenanceReason {
    RecentLearningActivity,
    PrecedentBacklog,
    FailurePatternBacklog,
    CandidateSkillBacklog,
    PromptOptimizationDue,
    ForcedMaintenanceInterval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningMaintenancePlan {
    pub run_importance_decay: bool,
    pub run_gc: bool,
    pub run_precedent_compaction: bool,
    pub run_failure_pattern_compaction: bool,
    pub run_skill_review: bool,
    pub run_prompt_optimization: bool,
    pub reasons: Vec<LearningMaintenanceReason>,
}

impl LearningMaintenancePlan {
    pub fn should_run_any(&self) -> bool {
        self.run_importance_decay
            || self.run_gc
            || self.run_skill_review
            || self.run_prompt_optimization
    }

    pub fn has_any_advisory_action(&self) -> bool {
        self.run_precedent_compaction
            || self.run_failure_pattern_compaction
            || self.should_run_any()
    }
}

#[derive(Debug, Clone)]
pub struct LearningMaintenancePolicy {
    pub min_recent_learning_activity: usize,
    pub min_precedents_for_compaction: usize,
    pub min_failure_patterns_for_compaction: usize,
    pub min_candidate_skills_for_review: usize,
    pub max_skipped_cycles_before_forced_maintenance: u32,
    pub min_reflections_for_prompt_optimization: usize,
}

impl Default for LearningMaintenancePolicy {
    fn default() -> Self {
        Self {
            min_recent_learning_activity: 1,
            min_precedents_for_compaction: 3,
            min_failure_patterns_for_compaction: 2,
            min_candidate_skills_for_review: 2,
            max_skipped_cycles_before_forced_maintenance: 6,
            min_reflections_for_prompt_optimization: 3,
        }
    }
}

pub fn build_learning_maintenance_plan(
    snapshot: &LearningMaintenanceSnapshot,
    policy: &LearningMaintenancePolicy,
) -> LearningMaintenancePlan {
    let recent_learning_activity =
        snapshot.recent_learning_activity_count() >= policy.min_recent_learning_activity;
    let precedent_backlog = snapshot.recent_precedent_count >= policy.min_precedents_for_compaction;
    let failure_backlog =
        snapshot.recent_failure_pattern_count >= policy.min_failure_patterns_for_compaction;
    let candidate_skill_backlog =
        snapshot.candidate_skill_count >= policy.min_candidate_skills_for_review;
    let forced_maintenance = snapshot.skipped_cycles_since_maintenance
        >= policy.max_skipped_cycles_before_forced_maintenance;
    let prompt_optimization = snapshot.prompt_optimization_due
        && snapshot.recent_reflection_count >= policy.min_reflections_for_prompt_optimization;

    let run_importance_decay = recent_learning_activity || forced_maintenance;
    let run_gc = run_importance_decay;
    let run_precedent_compaction = precedent_backlog;
    let run_failure_pattern_compaction = failure_backlog;
    let run_skill_review = candidate_skill_backlog;

    let mut reasons = Vec::new();
    if recent_learning_activity {
        reasons.push(LearningMaintenanceReason::RecentLearningActivity);
    }
    if precedent_backlog {
        reasons.push(LearningMaintenanceReason::PrecedentBacklog);
    }
    if failure_backlog {
        reasons.push(LearningMaintenanceReason::FailurePatternBacklog);
    }
    if candidate_skill_backlog {
        reasons.push(LearningMaintenanceReason::CandidateSkillBacklog);
    }
    if prompt_optimization {
        reasons.push(LearningMaintenanceReason::PromptOptimizationDue);
    }
    if forced_maintenance {
        reasons.push(LearningMaintenanceReason::ForcedMaintenanceInterval);
    }

    LearningMaintenancePlan {
        run_importance_decay,
        run_gc,
        run_precedent_compaction,
        run_failure_pattern_compaction,
        run_skill_review,
        run_prompt_optimization: prompt_optimization,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_learning_activity_runs_maintenance() {
        let plan = build_learning_maintenance_plan(
            &LearningMaintenanceSnapshot {
                recent_precedent_count: 1,
                recent_reflection_count: 0,
                recent_failure_pattern_count: 0,
                recent_skill_count: 0,
                candidate_skill_count: 0,
                skipped_cycles_since_maintenance: 0,
                prompt_optimization_due: false,
            },
            &LearningMaintenancePolicy::default(),
        );

        assert!(plan.run_importance_decay);
        assert!(plan.run_gc);
        assert!(!plan.run_precedent_compaction);
        assert!(!plan.run_prompt_optimization);
        assert!(plan
            .reasons
            .contains(&LearningMaintenanceReason::RecentLearningActivity));
    }

    #[test]
    fn prompt_optimization_needs_due_signal_and_reflections() {
        let plan = build_learning_maintenance_plan(
            &LearningMaintenanceSnapshot {
                recent_precedent_count: 0,
                recent_reflection_count: 3,
                recent_failure_pattern_count: 0,
                recent_skill_count: 0,
                candidate_skill_count: 0,
                skipped_cycles_since_maintenance: 0,
                prompt_optimization_due: true,
            },
            &LearningMaintenancePolicy::default(),
        );

        assert!(plan.run_prompt_optimization);
        assert!(plan
            .reasons
            .contains(&LearningMaintenanceReason::PromptOptimizationDue));
    }

    #[test]
    fn forced_interval_runs_decay_even_without_recent_activity() {
        let plan = build_learning_maintenance_plan(
            &LearningMaintenanceSnapshot {
                recent_precedent_count: 0,
                recent_reflection_count: 0,
                recent_failure_pattern_count: 0,
                recent_skill_count: 0,
                candidate_skill_count: 0,
                skipped_cycles_since_maintenance: 6,
                prompt_optimization_due: false,
            },
            &LearningMaintenancePolicy::default(),
        );

        assert!(plan.run_importance_decay);
        assert!(plan.run_gc);
        assert!(!plan.run_prompt_optimization);
        assert!(plan
            .reasons
            .contains(&LearningMaintenanceReason::ForcedMaintenanceInterval));
    }

    #[test]
    fn category_backlogs_trigger_targeted_reviews() {
        let plan = build_learning_maintenance_plan(
            &LearningMaintenanceSnapshot {
                recent_precedent_count: 3,
                recent_reflection_count: 0,
                recent_failure_pattern_count: 2,
                recent_skill_count: 4,
                candidate_skill_count: 2,
                skipped_cycles_since_maintenance: 0,
                prompt_optimization_due: false,
            },
            &LearningMaintenancePolicy::default(),
        );

        assert!(plan.run_precedent_compaction);
        assert!(plan.run_failure_pattern_compaction);
        assert!(plan.run_skill_review);
        assert!(plan.should_run_any());
        assert!(plan
            .reasons
            .contains(&LearningMaintenanceReason::PrecedentBacklog));
        assert!(plan
            .reasons
            .contains(&LearningMaintenanceReason::FailurePatternBacklog));
        assert!(plan
            .reasons
            .contains(&LearningMaintenanceReason::CandidateSkillBacklog));
    }
}
