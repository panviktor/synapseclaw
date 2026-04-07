//! Background learning and maintenance gating.
//!
//! Keeps deferred learning work cheap by deciding when background maintenance
//! should actually run, instead of blindly executing every cycle.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningMaintenanceSnapshot {
    pub recent_precedent_count: usize,
    pub recent_reflection_count: usize,
    pub recent_failure_pattern_count: usize,
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
    PromptOptimizationDue,
    ForcedMaintenanceInterval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearningMaintenancePlan {
    pub run_importance_decay: bool,
    pub run_gc: bool,
    pub run_prompt_optimization: bool,
    pub reasons: Vec<LearningMaintenanceReason>,
}

impl LearningMaintenancePlan {
    pub fn should_run_any(&self) -> bool {
        self.run_importance_decay || self.run_gc || self.run_prompt_optimization
    }
}

#[derive(Debug, Clone)]
pub struct LearningMaintenancePolicy {
    pub min_recent_learning_activity: usize,
    pub max_skipped_cycles_before_forced_maintenance: u32,
    pub min_reflections_for_prompt_optimization: usize,
}

impl Default for LearningMaintenancePolicy {
    fn default() -> Self {
        Self {
            min_recent_learning_activity: 1,
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
    let forced_maintenance = snapshot.skipped_cycles_since_maintenance
        >= policy.max_skipped_cycles_before_forced_maintenance;
    let prompt_optimization = snapshot.prompt_optimization_due
        && snapshot.recent_reflection_count >= policy.min_reflections_for_prompt_optimization;

    let run_importance_decay = recent_learning_activity || forced_maintenance;
    let run_gc = run_importance_decay;

    let mut reasons = Vec::new();
    if recent_learning_activity {
        reasons.push(LearningMaintenanceReason::RecentLearningActivity);
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
                skipped_cycles_since_maintenance: 0,
                prompt_optimization_due: false,
            },
            &LearningMaintenancePolicy::default(),
        );

        assert!(plan.run_importance_decay);
        assert!(plan.run_gc);
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
}
