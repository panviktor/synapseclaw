//! Provider-facing context budget policy for Phase 4.10.
//!
//! The domain owns the budget targets and pressure classification so adapters
//! can observe and enforce prompt economy without hard-coding magic numbers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderContextBudgetTier {
    Healthy,
    Caution,
    OverBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderContextTurnShape {
    Baseline,
    SimpleTool,
    HeavyTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderContextArtifact {
    Bootstrap,
    CoreMemory,
    RuntimeInterpretation,
    ScopedContext,
    Resolution,
    PriorChat,
    CurrentTurn,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderContextBudgetInput {
    pub total_chars: usize,
    pub prior_chat_messages: usize,
    pub current_turn_messages: usize,
    pub bootstrap_chars: usize,
    pub core_memory_chars: usize,
    pub runtime_interpretation_chars: usize,
    pub scoped_context_chars: usize,
    pub resolution_chars: usize,
    pub prior_chat_chars: usize,
    pub current_turn_chars: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudgetSnapshot {
    pub stable_system_chars: usize,
    pub dynamic_system_chars: usize,
    pub protected_chars: usize,
    pub removable_chars: usize,
    pub chars_over_target: usize,
    pub chars_over_ceiling: usize,
    pub estimated_total_tokens: usize,
    pub target_total_tokens: usize,
    pub ceiling_total_tokens: usize,
    pub protected_tokens: usize,
    pub removable_tokens: usize,
    pub tokens_headroom_to_target: usize,
    pub tokens_headroom_to_ceiling: usize,
    pub reserved_output_headroom_tokens: usize,
    pub primary_ballast_artifact: Option<ProviderContextArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderContextBudgetAssessment {
    pub turn_shape: ProviderContextTurnShape,
    pub target_total_chars: usize,
    pub ceiling_total_chars: usize,
    pub tier: ProviderContextBudgetTier,
    pub snapshot: ContextBudgetSnapshot,
}

pub fn assess_provider_context_budget(
    input: ProviderContextBudgetInput,
) -> ProviderContextBudgetAssessment {
    let turn_shape = if input.current_turn_messages >= 4 || input.prior_chat_messages >= 4 {
        ProviderContextTurnShape::HeavyTool
    } else if input.current_turn_messages >= 2 {
        ProviderContextTurnShape::SimpleTool
    } else {
        ProviderContextTurnShape::Baseline
    };

    let (target_total_chars, ceiling_total_chars) = match turn_shape {
        ProviderContextTurnShape::Baseline => (3_500, 5_500),
        ProviderContextTurnShape::SimpleTool => (5_500, 7_000),
        ProviderContextTurnShape::HeavyTool => (6_000, 8_500),
    };

    let chars_over_target = input.total_chars.saturating_sub(target_total_chars);
    let chars_over_ceiling = input.total_chars.saturating_sub(ceiling_total_chars);
    let estimated_total_tokens = estimate_tokens_from_chars(input.total_chars);
    let target_total_tokens = estimate_tokens_from_chars(target_total_chars);
    let ceiling_total_tokens = estimate_tokens_from_chars(ceiling_total_chars);
    let reserved_output_headroom_tokens = reserved_output_headroom_tokens(turn_shape);

    let tier = if estimated_total_tokens.saturating_add(reserved_output_headroom_tokens)
        > ceiling_total_tokens
        || input.total_chars > ceiling_total_chars
    {
        ProviderContextBudgetTier::OverBudget
    } else if estimated_total_tokens.saturating_add(reserved_output_headroom_tokens)
        > target_total_tokens
        || input.total_chars > target_total_chars
    {
        ProviderContextBudgetTier::Caution
    } else {
        ProviderContextBudgetTier::Healthy
    };

    let stable_system_chars = input.bootstrap_chars;
    let dynamic_system_chars = input
        .core_memory_chars
        .saturating_add(input.runtime_interpretation_chars)
        .saturating_add(input.scoped_context_chars)
        .saturating_add(input.resolution_chars);
    let protected_chars = input
        .bootstrap_chars
        .saturating_add(input.core_memory_chars)
        .saturating_add(input.current_turn_chars);
    let removable_chars = input
        .prior_chat_chars
        .saturating_add(input.scoped_context_chars)
        .saturating_add(input.resolution_chars)
        .saturating_add(input.runtime_interpretation_chars);
    let protected_tokens = estimate_tokens_from_chars(protected_chars);
    let removable_tokens = estimate_tokens_from_chars(removable_chars);
    let tokens_headroom_to_target = target_total_tokens.saturating_sub(estimated_total_tokens);
    let tokens_headroom_to_ceiling = ceiling_total_tokens.saturating_sub(estimated_total_tokens);

    let primary_ballast_artifact = removable_artifact_candidates(&input)
        .into_iter()
        .max_by_key(|(_, chars)| *chars)
        .and_then(|(artifact, chars)| (chars > 0).then_some(artifact));

    ProviderContextBudgetAssessment {
        turn_shape,
        target_total_chars,
        ceiling_total_chars,
        tier,
        snapshot: ContextBudgetSnapshot {
            stable_system_chars,
            dynamic_system_chars,
            protected_chars,
            removable_chars,
            chars_over_target,
            chars_over_ceiling,
            estimated_total_tokens,
            target_total_tokens,
            ceiling_total_tokens,
            protected_tokens,
            removable_tokens,
            tokens_headroom_to_target,
            tokens_headroom_to_ceiling,
            reserved_output_headroom_tokens,
            primary_ballast_artifact,
        },
    }
}

pub fn estimate_tokens_from_chars(chars: usize) -> usize {
    chars.div_ceil(4)
}

fn removable_artifact_candidates(
    input: &ProviderContextBudgetInput,
) -> [(ProviderContextArtifact, usize); 4] {
    [
        (ProviderContextArtifact::PriorChat, input.prior_chat_chars),
        (
            ProviderContextArtifact::ScopedContext,
            input.scoped_context_chars,
        ),
        (ProviderContextArtifact::Resolution, input.resolution_chars),
        (
            ProviderContextArtifact::RuntimeInterpretation,
            input.runtime_interpretation_chars,
        ),
    ]
}

fn reserved_output_headroom_tokens(shape: ProviderContextTurnShape) -> usize {
    match shape {
        ProviderContextTurnShape::Baseline => 96,
        ProviderContextTurnShape::SimpleTool => 96,
        ProviderContextTurnShape::HeavyTool => 128,
    }
}

pub fn provider_context_budget_tier_name(tier: ProviderContextBudgetTier) -> &'static str {
    match tier {
        ProviderContextBudgetTier::Healthy => "healthy",
        ProviderContextBudgetTier::Caution => "caution",
        ProviderContextBudgetTier::OverBudget => "over_budget",
    }
}

pub fn provider_context_turn_shape_name(shape: ProviderContextTurnShape) -> &'static str {
    match shape {
        ProviderContextTurnShape::Baseline => "baseline",
        ProviderContextTurnShape::SimpleTool => "simple_tool",
        ProviderContextTurnShape::HeavyTool => "heavy_tool",
    }
}

pub fn provider_context_artifact_name(artifact: ProviderContextArtifact) -> &'static str {
    match artifact {
        ProviderContextArtifact::Bootstrap => "bootstrap",
        ProviderContextArtifact::CoreMemory => "core_memory",
        ProviderContextArtifact::RuntimeInterpretation => "runtime_interpretation",
        ProviderContextArtifact::ScopedContext => "scoped_context",
        ProviderContextArtifact::Resolution => "resolution",
        ProviderContextArtifact::PriorChat => "prior_chat",
        ProviderContextArtifact::CurrentTurn => "current_turn",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_turn_above_target_enters_caution_band() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 4_800,
            prior_chat_messages: 0,
            current_turn_messages: 1,
            current_turn_chars: 400,
            ..Default::default()
        });
        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::Baseline);
        assert_eq!(assessment.target_total_chars, 3_500);
        assert_eq!(assessment.ceiling_total_chars, 5_500);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::Caution);
        assert_eq!(assessment.snapshot.chars_over_target, 1_300);
        assert_eq!(assessment.snapshot.estimated_total_tokens, 1_200);
    }

    #[test]
    fn simple_tool_turn_tracks_primary_ballast_artifact() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 6_600,
            prior_chat_messages: 1,
            current_turn_messages: 3,
            prior_chat_chars: 2_100,
            scoped_context_chars: 300,
            resolution_chars: 250,
            current_turn_chars: 900,
            ..Default::default()
        });
        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::SimpleTool);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::Caution);
        assert_eq!(
            assessment.snapshot.primary_ballast_artifact,
            Some(ProviderContextArtifact::PriorChat)
        );
        assert_eq!(assessment.snapshot.removable_chars, 2_650);
        assert!(assessment.snapshot.removable_tokens > 0);
    }

    #[test]
    fn heavy_tool_turn_can_be_flagged_over_budget() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 10_200,
            prior_chat_messages: 4,
            current_turn_messages: 5,
            prior_chat_chars: 2_000,
            current_turn_chars: 1_500,
            ..Default::default()
        });
        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::HeavyTool);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::OverBudget);
        assert_eq!(assessment.snapshot.chars_over_ceiling, 1_700);
        assert_eq!(assessment.snapshot.tokens_headroom_to_ceiling, 0);
    }

    #[test]
    fn reserved_output_headroom_can_push_turn_into_caution() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 3_200,
            prior_chat_messages: 0,
            current_turn_messages: 1,
            current_turn_chars: 300,
            ..Default::default()
        });

        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::Baseline);
        assert_eq!(assessment.snapshot.estimated_total_tokens, 800);
        assert_eq!(assessment.snapshot.target_total_tokens, 875);
        assert_eq!(assessment.snapshot.reserved_output_headroom_tokens, 96);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::Caution);
    }
}
