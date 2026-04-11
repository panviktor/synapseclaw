//! Provider-facing context budget policy for Phase 4.10.
//!
//! The domain owns the budget targets and pressure classification so adapters
//! can observe and enforce prompt economy through one shared policy.

use crate::application::services::model_lane_resolution::{
    ResolvedModelProfile, ResolvedModelProfileConfidence,
};
use crate::domain::message::ChatMessage;

pub const CONTEXT_COMPRESSION_THRESHOLD_NUMERATOR: usize = 1;
pub const CONTEXT_COMPRESSION_THRESHOLD_DENOMINATOR: usize = 2;
pub const CONTEXT_SAFETY_CEILING_NUMERATOR: usize = 17;
pub const CONTEXT_SAFETY_CEILING_DENOMINATOR: usize = 20;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderContextCondensationMode {
    Trim,
    Summarize,
    Handoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderContextCondensationPlan {
    pub mode: ProviderContextCondensationMode,
    pub target_artifact: Option<ProviderContextArtifact>,
    pub minimum_reclaim_chars: usize,
    pub prefer_cached_artifact: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderContextBudgetInput {
    pub total_chars: usize,
    pub prior_chat_messages: usize,
    pub current_turn_messages: usize,
    pub target_context_window_tokens: Option<usize>,
    pub target_max_output_tokens: Option<usize>,
    pub bootstrap_chars: usize,
    pub core_memory_chars: usize,
    pub runtime_interpretation_chars: usize,
    pub scoped_context_chars: usize,
    pub resolution_chars: usize,
    pub prior_chat_chars: usize,
    pub current_turn_chars: usize,
}

impl ProviderContextBudgetInput {
    pub fn with_target_model_profile(mut self, profile: &ResolvedModelProfile) -> Self {
        if profile.context_window_confidence() >= ResolvedModelProfileConfidence::Medium {
            self.target_context_window_tokens = profile.context_window_tokens;
            self.target_max_output_tokens = (profile.max_output_confidence()
                >= ResolvedModelProfileConfidence::Medium)
                .then_some(profile.max_output_tokens)
                .flatten();
        }
        self
    }
}

pub fn provider_context_input_for_history(history: &[ChatMessage]) -> ProviderContextBudgetInput {
    let non_system_messages = history.iter().filter(|msg| msg.role != "system").count();
    let prior_chat_messages = non_system_messages.saturating_sub(1);
    let total_chars: usize = history.iter().map(|msg| msg.content.chars().count()).sum();
    let system_breakdown = provider_history_system_breakdown(history);
    let system_chars: usize = history
        .iter()
        .filter(|msg| msg.role == "system")
        .map(|msg| msg.content.chars().count())
        .sum();
    let current_turn_chars = history
        .iter()
        .rev()
        .find(|msg| msg.role != "system")
        .map(|msg| msg.content.chars().count())
        .unwrap_or(0);
    let prior_chat_chars = total_chars
        .saturating_sub(system_chars)
        .saturating_sub(current_turn_chars);

    ProviderContextBudgetInput {
        total_chars,
        prior_chat_messages,
        current_turn_messages: usize::from(non_system_messages > 0),
        bootstrap_chars: lookup_system_section_chars(&system_breakdown, "bootstrap"),
        core_memory_chars: lookup_system_section_chars(&system_breakdown, "core_memory"),
        runtime_interpretation_chars: lookup_system_section_chars(
            &system_breakdown,
            "runtime_interpretation",
        ),
        scoped_context_chars: lookup_system_section_chars(&system_breakdown, "scoped_context"),
        resolution_chars: lookup_system_section_chars(&system_breakdown, "resolution"),
        prior_chat_chars,
        current_turn_chars,
        ..Default::default()
    }
}

fn provider_history_system_breakdown(history: &[ChatMessage]) -> Vec<(String, usize)> {
    let mut breakdown = std::collections::BTreeMap::<String, usize>::new();
    for message in history.iter().filter(|msg| msg.role == "system") {
        for (name, chars) in classify_system_message_sections(&message.content) {
            *breakdown.entry(name.to_string()).or_default() += chars;
        }
    }
    breakdown.into_iter().collect()
}

fn classify_system_message_sections(content: &str) -> Vec<(&'static str, usize)> {
    let markers = [
        ("[core-memory]\n", "core_memory"),
        ("[runtime-interpretation]\n", "runtime_interpretation"),
        ("[scoped-context]\n", "scoped_context"),
        ("[resolution-plan]\n", "resolution"),
        ("[clarification-policy]\n", "resolution"),
        ("[execution-guidance]\n", "resolution"),
    ];

    let mut ranges = markers
        .iter()
        .filter_map(|(marker, name)| content.find(marker).map(|start| (start, *marker, *name)))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|(start, _, _)| *start);

    if ranges.is_empty() {
        return vec![("bootstrap", content.chars().count())];
    }

    let mut sections = Vec::new();
    if let Some((first_start, _, _)) = ranges.first().copied() {
        if first_start > 0 {
            sections.push(("bootstrap", content[..first_start].chars().count()));
        }
    }

    for (index, (start, marker, name)) in ranges.iter().enumerate() {
        let end = ranges
            .get(index + 1)
            .map(|(next_start, _, _)| *next_start)
            .unwrap_or(content.len());
        let slice = &content[*start..end];
        if !slice.is_empty() {
            let marker_chars = marker.chars().count();
            sections.push((*name, slice.chars().count().max(marker_chars)));
        }
    }

    sections
}

fn lookup_system_section_chars(breakdown: &[(String, usize)], section: &str) -> usize {
    breakdown
        .iter()
        .find_map(|(name, chars)| (name == section).then_some(*chars))
        .unwrap_or(0)
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderContextPrunePolicy {
    pub drop_scoped_context: bool,
    pub max_runtime_interpretation_chars: Option<usize>,
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

    let (target_total_chars, ceiling_total_chars, reserved_output_headroom_tokens) =
        budget_limits_for_shape(turn_shape, &input);

    let chars_over_target = input.total_chars.saturating_sub(target_total_chars);
    let chars_over_ceiling = input.total_chars.saturating_sub(ceiling_total_chars);
    let estimated_total_tokens = estimate_tokens_from_chars(input.total_chars);
    let target_total_tokens = estimate_tokens_from_chars(target_total_chars);
    let ceiling_total_tokens = estimate_tokens_from_chars(ceiling_total_chars);

    let pressure_tokens = if input.target_context_window_tokens.is_some() {
        estimated_total_tokens
    } else {
        estimated_total_tokens.saturating_add(reserved_output_headroom_tokens)
    };
    let tier = if pressure_tokens > ceiling_total_tokens || input.total_chars > ceiling_total_chars
    {
        ProviderContextBudgetTier::OverBudget
    } else if pressure_tokens > target_total_tokens || input.total_chars > target_total_chars {
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

pub fn provider_context_prune_policy(
    input: ProviderContextBudgetInput,
) -> ProviderContextPrunePolicy {
    let assessment = assess_provider_context_budget(input);

    match assessment.tier {
        ProviderContextBudgetTier::Healthy => ProviderContextPrunePolicy::default(),
        ProviderContextBudgetTier::Caution => ProviderContextPrunePolicy {
            drop_scoped_context: matches!(
                assessment.snapshot.primary_ballast_artifact,
                Some(ProviderContextArtifact::ScopedContext)
            ) && input.scoped_context_chars > 0,
            max_runtime_interpretation_chars: (matches!(
                assessment.snapshot.primary_ballast_artifact,
                Some(ProviderContextArtifact::RuntimeInterpretation)
            ) && input.runtime_interpretation_chars > 640)
                .then_some(640),
        },
        ProviderContextBudgetTier::OverBudget => ProviderContextPrunePolicy {
            drop_scoped_context: input.scoped_context_chars > 0,
            max_runtime_interpretation_chars: (input.runtime_interpretation_chars > 420)
                .then_some(420),
        },
    }
}

pub fn provider_context_condensation_plan(
    assessment: &ProviderContextBudgetAssessment,
) -> Option<ProviderContextCondensationPlan> {
    if assessment.tier == ProviderContextBudgetTier::Healthy {
        return None;
    }

    let minimum_reclaim_chars = assessment
        .snapshot
        .chars_over_target
        .max(assessment.snapshot.chars_over_ceiling);
    let minimum_reclaim_chars = minimum_reclaim_chars.max(1);

    let Some(artifact) = assessment.snapshot.primary_ballast_artifact else {
        return (assessment.tier == ProviderContextBudgetTier::OverBudget).then_some(
            ProviderContextCondensationPlan {
                mode: ProviderContextCondensationMode::Handoff,
                target_artifact: None,
                minimum_reclaim_chars,
                prefer_cached_artifact: false,
            },
        );
    };

    let (mode, prefer_cached_artifact) = match artifact {
        ProviderContextArtifact::PriorChat => (ProviderContextCondensationMode::Summarize, true),
        ProviderContextArtifact::ScopedContext
        | ProviderContextArtifact::Resolution
        | ProviderContextArtifact::RuntimeInterpretation => {
            (ProviderContextCondensationMode::Trim, false)
        }
        ProviderContextArtifact::Bootstrap
        | ProviderContextArtifact::CoreMemory
        | ProviderContextArtifact::CurrentTurn => {
            return (assessment.tier == ProviderContextBudgetTier::OverBudget).then_some(
                ProviderContextCondensationPlan {
                    mode: ProviderContextCondensationMode::Handoff,
                    target_artifact: Some(artifact),
                    minimum_reclaim_chars,
                    prefer_cached_artifact: false,
                },
            );
        }
    };

    Some(ProviderContextCondensationPlan {
        mode,
        target_artifact: Some(artifact),
        minimum_reclaim_chars,
        prefer_cached_artifact,
    })
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

fn budget_limits_for_shape(
    shape: ProviderContextTurnShape,
    input: &ProviderContextBudgetInput,
) -> (usize, usize, usize) {
    let (base_target_chars, base_ceiling_chars) = base_budget_chars_for_shape(shape);
    let reserved_output_headroom_tokens = reserved_output_headroom_tokens(
        shape,
        input.target_context_window_tokens,
        input.target_max_output_tokens,
    );
    let Some(context_window_tokens) = input.target_context_window_tokens else {
        return (
            base_target_chars,
            base_ceiling_chars,
            reserved_output_headroom_tokens,
        );
    };

    let safe_input_tokens = context_window_tokens.saturating_sub(reserved_output_headroom_tokens);
    if safe_input_tokens == 0 {
        return (
            base_target_chars,
            base_ceiling_chars,
            reserved_output_headroom_tokens,
        );
    }

    let base_target_tokens = estimate_tokens_from_chars(base_target_chars);
    let base_ceiling_tokens = estimate_tokens_from_chars(base_ceiling_chars);
    let target_tokens = provider_context_compression_threshold_tokens(safe_input_tokens)
        .max(base_target_tokens)
        .min(safe_input_tokens);
    let ceiling_tokens = provider_context_safety_ceiling_tokens(safe_input_tokens)
        .max(base_ceiling_tokens)
        .min(safe_input_tokens)
        .max(target_tokens);

    (
        chars_from_tokens(target_tokens),
        chars_from_tokens(ceiling_tokens),
        reserved_output_headroom_tokens,
    )
}

fn base_budget_chars_for_shape(shape: ProviderContextTurnShape) -> (usize, usize) {
    match shape {
        ProviderContextTurnShape::Baseline => (3_500, 5_500),
        ProviderContextTurnShape::SimpleTool => (5_500, 7_000),
        ProviderContextTurnShape::HeavyTool => (6_000, 8_500),
    }
}

pub fn provider_context_compression_threshold_tokens(safe_input_tokens: usize) -> usize {
    safe_input_tokens.saturating_mul(CONTEXT_COMPRESSION_THRESHOLD_NUMERATOR)
        / CONTEXT_COMPRESSION_THRESHOLD_DENOMINATOR
}

pub fn provider_context_safety_ceiling_tokens(safe_input_tokens: usize) -> usize {
    safe_input_tokens.saturating_mul(CONTEXT_SAFETY_CEILING_NUMERATOR)
        / CONTEXT_SAFETY_CEILING_DENOMINATOR
}

pub fn provider_context_reserved_output_headroom_tokens(
    context_window_tokens: Option<usize>,
    target_max_output_tokens: Option<usize>,
    minimum_headroom_tokens: usize,
) -> usize {
    let Some(context_window_tokens) = context_window_tokens else {
        return minimum_headroom_tokens;
    };

    let heuristic = (context_window_tokens / 8).max(minimum_headroom_tokens);
    let requested = target_max_output_tokens.unwrap_or(heuristic);
    let max_safe_reserve = (context_window_tokens / 4).max(minimum_headroom_tokens);
    requested.clamp(minimum_headroom_tokens, max_safe_reserve)
}

fn chars_from_tokens(tokens: usize) -> usize {
    tokens.saturating_mul(4)
}

fn reserved_output_headroom_tokens(
    shape: ProviderContextTurnShape,
    context_window_tokens: Option<usize>,
    target_max_output_tokens: Option<usize>,
) -> usize {
    let base = match shape {
        ProviderContextTurnShape::Baseline => 96,
        ProviderContextTurnShape::SimpleTool => 96,
        ProviderContextTurnShape::HeavyTool => 128,
    };
    provider_context_reserved_output_headroom_tokens(
        context_window_tokens,
        target_max_output_tokens,
        base,
    )
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

pub fn provider_context_condensation_mode_name(
    mode: ProviderContextCondensationMode,
) -> &'static str {
    match mode {
        ProviderContextCondensationMode::Trim => "trim",
        ProviderContextCondensationMode::Summarize => "summarize",
        ProviderContextCondensationMode::Handoff => "handoff",
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

    #[test]
    fn large_window_profile_scales_budget_without_triggering_pressure() {
        let profile = ResolvedModelProfile {
            context_window_tokens: Some(262_144),
            max_output_tokens: Some(131_072),
            context_window_source:
                crate::application::services::model_lane_resolution::ResolvedModelProfileSource::CachedProviderCatalog,
            max_output_source:
                crate::application::services::model_lane_resolution::ResolvedModelProfileSource::CachedProviderCatalog,
            observed_at_unix: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be after unix epoch")
                    .as_secs(),
            ),
            ..Default::default()
        };
        let assessment = assess_provider_context_budget(
            ProviderContextBudgetInput {
                total_chars: 7_800,
                prior_chat_messages: 4,
                current_turn_messages: 1,
                prior_chat_chars: 5_000,
                current_turn_chars: 500,
                ..Default::default()
            }
            .with_target_model_profile(&profile),
        );

        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::HeavyTool);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::Healthy);
        assert_eq!(assessment.target_total_chars, 393_216);
        assert_eq!(assessment.ceiling_total_chars, 668_464);
        assert_eq!(assessment.snapshot.reserved_output_headroom_tokens, 65_536);
        assert_eq!(assessment.snapshot.chars_over_target, 0);
    }

    #[test]
    fn very_large_window_policy_uses_safe_input_ratios_instead_of_fixed_caps() {
        let profile = ResolvedModelProfile {
            context_window_tokens: Some(2_000_000),
            context_window_source:
                crate::application::services::model_lane_resolution::ResolvedModelProfileSource::CachedProviderCatalog,
            observed_at_unix: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be after unix epoch")
                    .as_secs(),
            ),
            ..Default::default()
        };
        let assessment = assess_provider_context_budget(
            ProviderContextBudgetInput {
                total_chars: 200_000,
                prior_chat_messages: 6,
                current_turn_messages: 1,
                prior_chat_chars: 160_000,
                current_turn_chars: 4_000,
                ..Default::default()
            }
            .with_target_model_profile(&profile),
        );

        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::HeavyTool);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::Healthy);
        assert_eq!(assessment.snapshot.reserved_output_headroom_tokens, 250_000);
        assert_eq!(assessment.snapshot.target_total_tokens, 875_000);
        assert_eq!(assessment.snapshot.ceiling_total_tokens, 1_487_500);
    }

    #[test]
    fn different_provider_windows_shift_pressure_boundaries() {
        let source =
            crate::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig;
        let deepseek_profile = ResolvedModelProfile {
            context_window_tokens: Some(128_000),
            context_window_source: source,
            max_output_tokens: Some(8_000),
            max_output_source: source,
            ..Default::default()
        };
        let grok_profile = ResolvedModelProfile {
            context_window_tokens: Some(2_000_000),
            context_window_source: source,
            max_output_tokens: Some(128_000),
            max_output_source: source,
            ..Default::default()
        };
        let input = ProviderContextBudgetInput {
            total_chars: 300_000,
            prior_chat_messages: 6,
            current_turn_messages: 1,
            prior_chat_chars: 250_000,
            current_turn_chars: 4_000,
            ..Default::default()
        };

        let deepseek =
            assess_provider_context_budget(input.with_target_model_profile(&deepseek_profile));
        let grok = assess_provider_context_budget(input.with_target_model_profile(&grok_profile));

        assert_eq!(deepseek.tier, ProviderContextBudgetTier::Caution);
        assert_eq!(deepseek.target_total_chars, 240_000);
        assert_eq!(deepseek.snapshot.reserved_output_headroom_tokens, 8_000);
        assert_eq!(grok.tier, ProviderContextBudgetTier::Healthy);
        assert_eq!(grok.target_total_chars, 3_744_000);
        assert_eq!(grok.snapshot.reserved_output_headroom_tokens, 128_000);
    }

    #[test]
    fn low_confidence_window_metadata_keeps_legacy_compact_budget() {
        let profile = ResolvedModelProfile {
            context_window_tokens: Some(262_144),
            context_window_source:
                crate::application::services::model_lane_resolution::ResolvedModelProfileSource::AdapterFallback,
            observed_at_unix: Some(1),
            ..Default::default()
        };
        let assessment = assess_provider_context_budget(
            ProviderContextBudgetInput {
                total_chars: 7_800,
                prior_chat_messages: 4,
                current_turn_messages: 1,
                prior_chat_chars: 5_000,
                current_turn_chars: 500,
                ..Default::default()
            }
            .with_target_model_profile(&profile),
        );

        assert_eq!(assessment.turn_shape, ProviderContextTurnShape::HeavyTool);
        assert_eq!(assessment.target_total_chars, 6_000);
        assert_eq!(assessment.ceiling_total_chars, 8_500);
        assert_eq!(assessment.snapshot.reserved_output_headroom_tokens, 128);
        assert_eq!(assessment.tier, ProviderContextBudgetTier::Caution);
    }

    #[test]
    fn prune_policy_drops_scoped_context_when_it_is_caution_ballast() {
        let policy = provider_context_prune_policy(ProviderContextBudgetInput {
            total_chars: 4_400,
            current_turn_messages: 1,
            scoped_context_chars: 1_600,
            runtime_interpretation_chars: 300,
            prior_chat_chars: 400,
            current_turn_chars: 500,
            ..Default::default()
        });

        assert!(policy.drop_scoped_context);
        assert_eq!(policy.max_runtime_interpretation_chars, None);
    }

    #[test]
    fn prune_policy_compacts_runtime_interpretation_under_over_budget_pressure() {
        let policy = provider_context_prune_policy(ProviderContextBudgetInput {
            total_chars: 7_200,
            current_turn_messages: 1,
            runtime_interpretation_chars: 1_500,
            scoped_context_chars: 800,
            current_turn_chars: 500,
            ..Default::default()
        });

        assert!(policy.drop_scoped_context);
        assert_eq!(policy.max_runtime_interpretation_chars, Some(420));
    }

    #[test]
    fn condensation_plan_prefers_cached_summary_for_prior_chat_ballast() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 6_600,
            current_turn_messages: 1,
            prior_chat_chars: 2_400,
            scoped_context_chars: 400,
            current_turn_chars: 500,
            ..Default::default()
        });

        let plan = provider_context_condensation_plan(&assessment).expect("plan");

        assert_eq!(plan.mode, ProviderContextCondensationMode::Summarize);
        assert_eq!(
            plan.target_artifact,
            Some(ProviderContextArtifact::PriorChat)
        );
        assert!(plan.prefer_cached_artifact);
        assert_eq!(plan.minimum_reclaim_chars, 3_100);
    }

    #[test]
    fn condensation_plan_trims_scoped_context_when_it_is_ballast() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 4_400,
            current_turn_messages: 1,
            scoped_context_chars: 1_600,
            runtime_interpretation_chars: 300,
            prior_chat_chars: 400,
            current_turn_chars: 500,
            ..Default::default()
        });

        let plan = provider_context_condensation_plan(&assessment).expect("plan");

        assert_eq!(plan.mode, ProviderContextCondensationMode::Trim);
        assert_eq!(
            plan.target_artifact,
            Some(ProviderContextArtifact::ScopedContext)
        );
        assert!(!plan.prefer_cached_artifact);
    }

    #[test]
    fn condensation_plan_handoff_when_over_budget_has_no_removable_ballast() {
        let assessment = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 8_000,
            current_turn_messages: 1,
            bootstrap_chars: 7_500,
            current_turn_chars: 500,
            ..Default::default()
        });

        let plan = provider_context_condensation_plan(&assessment).expect("plan");

        assert_eq!(plan.mode, ProviderContextCondensationMode::Handoff);
        assert_eq!(plan.target_artifact, None);
        assert!(!plan.prefer_cached_artifact);
    }
}
