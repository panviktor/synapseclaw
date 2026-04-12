//! Turn budget and execution-gating scaffold for Phase 4.8 / 4.9.
//!
//! This module is a design anchor, not the final integrated runtime path yet.
//! It exists so Phase 4.8 does not drift into "run all models on every turn"
//! behavior, and so Phase 4.9 learning work inherits the same economic model.

/// Why a turn should leave the cheap path and invoke a bounded interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterGateReason {
    ContinuationLikely,
    ReferenceLikeTurn,
    DirectTypedReference,
    KnownFactResolutionLikely,
    AmbiguityDetected,
    PostToolFollowUp,
}

/// How much extra interpretation work the runtime should do for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterpreterMode {
    #[default]
    Skip,
    Lightweight,
    Required,
}

/// Retrieval limits for a single turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrievalBudget {
    pub max_session_candidates: usize,
    pub max_precedent_candidates: usize,
    pub max_memory_candidates: usize,
    pub max_projection_lines: usize,
}

impl Default for RetrievalBudget {
    fn default() -> Self {
        Self {
            max_session_candidates: 2,
            max_precedent_candidates: 2,
            max_memory_candidates: 5,
            max_projection_lines: 24,
        }
    }
}

/// Signals available before the main response call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnExecutionSignals {
    pub has_working_state: bool,
    pub has_profile_facts: bool,
    pub has_reference_candidates: bool,
    pub direct_reference_count: usize,
    pub structured_resolution_fact_count: usize,
    pub ambiguity_candidate_count: usize,
    pub recent_tool_fact_count: usize,
    pub explicit_user_correction: bool,
}

/// Canonical budget decision for a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnExecutionBudget {
    pub use_embedding_shortlist: bool,
    pub interpreter_mode: InterpreterMode,
    pub gate_reasons: Vec<InterpreterGateReason>,
    pub retrieval_budget: RetrievalBudget,
    pub allow_background_learning: bool,
    pub allow_heavy_background_learning: bool,
}

impl Default for TurnExecutionBudget {
    fn default() -> Self {
        Self {
            use_embedding_shortlist: true,
            interpreter_mode: InterpreterMode::Skip,
            gate_reasons: Vec::new(),
            retrieval_budget: RetrievalBudget::default(),
            allow_background_learning: true,
            allow_heavy_background_learning: false,
        }
    }
}

/// Build a bounded execution budget from cheap pre-turn signals.
pub fn build_turn_execution_budget(signals: TurnExecutionSignals) -> TurnExecutionBudget {
    let mut budget = TurnExecutionBudget::default();

    if signals.has_reference_candidates {
        budget
            .gate_reasons
            .push(InterpreterGateReason::ReferenceLikeTurn);
    }
    if signals.direct_reference_count > 0 {
        budget
            .gate_reasons
            .push(InterpreterGateReason::DirectTypedReference);
    }
    if signals.has_working_state {
        budget
            .gate_reasons
            .push(InterpreterGateReason::ContinuationLikely);
    }
    if signals.has_profile_facts {
        budget
            .gate_reasons
            .push(InterpreterGateReason::KnownFactResolutionLikely);
    }
    if signals.ambiguity_candidate_count >= 2 {
        budget
            .gate_reasons
            .push(InterpreterGateReason::AmbiguityDetected);
    }
    if signals.recent_tool_fact_count > 0 {
        budget
            .gate_reasons
            .push(InterpreterGateReason::PostToolFollowUp);
    }

    budget.interpreter_mode =
        if signals.explicit_user_correction || signals.ambiguity_candidate_count >= 3 {
            InterpreterMode::Required
        } else if !budget.gate_reasons.is_empty() {
            InterpreterMode::Lightweight
        } else {
            InterpreterMode::Skip
        };

    if budget.interpreter_mode == InterpreterMode::Required {
        budget.retrieval_budget.max_session_candidates = 3;
        budget.retrieval_budget.max_precedent_candidates = 3;
        budget.retrieval_budget.max_memory_candidates = 6;
    }

    if (signals.direct_reference_count > 0 || signals.structured_resolution_fact_count > 0)
        && signals.ambiguity_candidate_count == 0
    {
        budget.retrieval_budget.max_session_candidates = 0;
        budget.retrieval_budget.max_precedent_candidates = 0;
        budget.retrieval_budget.max_memory_candidates =
            budget.retrieval_budget.max_memory_candidates.min(3);
        budget.retrieval_budget.max_projection_lines =
            budget.retrieval_budget.max_projection_lines.min(16);
    }

    if signals.explicit_user_correction {
        budget.allow_heavy_background_learning = true;
    }

    budget
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheap_path_skips_interpreter() {
        let budget = build_turn_execution_budget(TurnExecutionSignals::default());
        assert_eq!(budget.interpreter_mode, InterpreterMode::Skip);
        assert!(budget.gate_reasons.is_empty());
        assert!(budget.use_embedding_shortlist);
    }

    #[test]
    fn reference_follow_up_uses_lightweight_mode() {
        let budget = build_turn_execution_budget(TurnExecutionSignals {
            has_reference_candidates: true,
            recent_tool_fact_count: 1,
            ..TurnExecutionSignals::default()
        });
        assert_eq!(budget.interpreter_mode, InterpreterMode::Lightweight);
        assert!(budget
            .gate_reasons
            .contains(&InterpreterGateReason::ReferenceLikeTurn));
    }

    #[test]
    fn direct_typed_reference_trims_historical_retrieval_budget() {
        let budget = build_turn_execution_budget(TurnExecutionSignals {
            has_reference_candidates: true,
            direct_reference_count: 1,
            recent_tool_fact_count: 1,
            ..TurnExecutionSignals::default()
        });
        assert_eq!(budget.interpreter_mode, InterpreterMode::Lightweight);
        assert!(budget
            .gate_reasons
            .contains(&InterpreterGateReason::DirectTypedReference));
        assert_eq!(budget.retrieval_budget.max_session_candidates, 0);
        assert_eq!(budget.retrieval_budget.max_precedent_candidates, 0);
        assert_eq!(budget.retrieval_budget.max_memory_candidates, 3);
    }

    #[test]
    fn structured_resolution_facts_trim_historical_retrieval_budget() {
        let budget = build_turn_execution_budget(TurnExecutionSignals {
            has_profile_facts: true,
            structured_resolution_fact_count: 1,
            ..TurnExecutionSignals::default()
        });
        assert_eq!(budget.interpreter_mode, InterpreterMode::Lightweight);
        assert_eq!(budget.retrieval_budget.max_session_candidates, 0);
        assert_eq!(budget.retrieval_budget.max_precedent_candidates, 0);
        assert_eq!(budget.retrieval_budget.max_memory_candidates, 3);
    }

    #[test]
    fn explicit_correction_requires_interpreter_and_heavy_learning() {
        let budget = build_turn_execution_budget(TurnExecutionSignals {
            explicit_user_correction: true,
            ambiguity_candidate_count: 3,
            ..TurnExecutionSignals::default()
        });
        assert_eq!(budget.interpreter_mode, InterpreterMode::Required);
        assert!(budget.allow_heavy_background_learning);
        assert_eq!(budget.retrieval_budget.max_session_candidates, 3);
    }
}
