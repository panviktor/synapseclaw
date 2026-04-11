use crate::application::services::model_lane_resolution::{
    ResolvedModelProfile, ResolvedModelProfileConfidence,
};
use crate::application::services::provider_context_budget::{
    estimate_tokens_from_chars, provider_context_compression_threshold_tokens,
    provider_context_condensation_plan, provider_context_reserved_output_headroom_tokens,
    ProviderContextBudgetAssessment, ProviderContextCondensationPlan,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSwitchStatus {
    Unknown,
    Safe,
    CompactRecommended,
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSwitchPreflight {
    pub estimated_context_tokens: usize,
    pub target_context_window_tokens: Option<usize>,
    pub safe_context_budget_tokens: Option<usize>,
    pub reserved_output_headroom_tokens: Option<usize>,
    pub recommended_compaction_threshold_tokens: Option<usize>,
    pub recommended_condensation: Option<ProviderContextCondensationPlan>,
    pub status: RouteSwitchStatus,
}

pub fn assess_route_switch_preflight(
    provider_context_chars: usize,
    target_profile: &ResolvedModelProfile,
) -> RouteSwitchPreflight {
    assess_route_switch_preflight_for_estimated_tokens(
        estimate_tokens_from_chars(provider_context_chars),
        target_profile,
        None,
    )
}

pub fn assess_route_switch_preflight_for_budget(
    budget_assessment: &ProviderContextBudgetAssessment,
    target_profile: &ResolvedModelProfile,
) -> RouteSwitchPreflight {
    assess_route_switch_preflight_for_estimated_tokens(
        budget_assessment.snapshot.estimated_total_tokens,
        target_profile,
        provider_context_condensation_plan(budget_assessment),
    )
}

fn assess_route_switch_preflight_for_estimated_tokens(
    estimated_context_tokens: usize,
    target_profile: &ResolvedModelProfile,
    recommended_condensation: Option<ProviderContextCondensationPlan>,
) -> RouteSwitchPreflight {
    let context_window_confidence = target_profile.context_window_confidence();
    let target_context_window_tokens =
        if context_window_confidence >= ResolvedModelProfileConfidence::Medium {
            target_profile.context_window_tokens
        } else {
            None
        };
    let reserved_output_headroom_tokens = target_context_window_tokens
        .map(|limit| compute_reserved_output_headroom_tokens(limit, target_profile));
    let safe_context_budget_tokens = target_context_window_tokens.map(|limit| {
        limit.saturating_sub(
            reserved_output_headroom_tokens
                .expect("reserved headroom should exist when context window exists"),
        )
    });
    let recommended_compaction_threshold_tokens =
        safe_context_budget_tokens.map(provider_context_compression_threshold_tokens);

    let status = match safe_context_budget_tokens {
        None => RouteSwitchStatus::Unknown,
        Some(safe_budget) if estimated_context_tokens > safe_budget => RouteSwitchStatus::TooLarge,
        Some(_)
            if recommended_compaction_threshold_tokens
                .is_some_and(|threshold| estimated_context_tokens > threshold) =>
        {
            RouteSwitchStatus::CompactRecommended
        }
        Some(_) => RouteSwitchStatus::Safe,
    };

    RouteSwitchPreflight {
        estimated_context_tokens,
        target_context_window_tokens,
        safe_context_budget_tokens,
        reserved_output_headroom_tokens,
        recommended_compaction_threshold_tokens,
        recommended_condensation,
        status,
    }
}

fn compute_reserved_output_headroom_tokens(
    context_window_tokens: usize,
    target_profile: &ResolvedModelProfile,
) -> usize {
    provider_context_reserved_output_headroom_tokens(
        Some(context_window_tokens),
        target_profile.max_output_tokens,
        512,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::provider_context_budget::{
        assess_provider_context_budget, ProviderContextArtifact, ProviderContextBudgetInput,
        ProviderContextCondensationMode,
    };

    #[test]
    fn unknown_when_target_window_missing() {
        let result = assess_route_switch_preflight(8_000, &ResolvedModelProfile::default());
        assert_eq!(result.status, RouteSwitchStatus::Unknown);
        assert_eq!(result.safe_context_budget_tokens, None);
    }

    #[test]
    fn recommends_compaction_before_downshift() {
        let result = assess_route_switch_preflight(
            90_000,
            &ResolvedModelProfile {
                context_window_tokens: Some(30_000),
                max_output_tokens: None,
                features: Vec::new(),
                context_window_source:
                    crate::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            },
        );
        assert_eq!(result.estimated_context_tokens, 22_500);
        assert_eq!(result.status, RouteSwitchStatus::CompactRecommended);
        assert_eq!(result.reserved_output_headroom_tokens, Some(3_750));
        assert_eq!(result.safe_context_budget_tokens, Some(26_250));
        assert_eq!(result.recommended_compaction_threshold_tokens, Some(13_125));
    }

    #[test]
    fn blocks_when_context_exceeds_target_window() {
        let result = assess_route_switch_preflight(
            300_000,
            &ResolvedModelProfile {
                context_window_tokens: Some(60_000),
                max_output_tokens: None,
                features: Vec::new(),
                context_window_source:
                    crate::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            },
        );
        assert_eq!(result.status, RouteSwitchStatus::TooLarge);
        assert_eq!(result.safe_context_budget_tokens, Some(52_500));
    }

    #[test]
    fn honors_explicit_max_output_tokens_when_reserving_headroom() {
        let result = assess_route_switch_preflight(
            40_000,
            &ResolvedModelProfile {
                context_window_tokens: Some(32_000),
                max_output_tokens: Some(6_000),
                features: Vec::new(),
                context_window_source:
                    crate::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            },
        );

        assert_eq!(result.reserved_output_headroom_tokens, Some(6_000));
        assert_eq!(result.safe_context_budget_tokens, Some(26_000));
    }

    #[test]
    fn ignores_low_confidence_context_window_metadata() {
        let result = assess_route_switch_preflight(
            300_000,
            &ResolvedModelProfile {
                context_window_tokens: Some(60_000),
                context_window_source:
                    crate::application::services::model_lane_resolution::ResolvedModelProfileSource::AdapterFallback,
                observed_at_unix: Some(1),
                ..Default::default()
            },
        );

        assert_eq!(result.status, RouteSwitchStatus::Unknown);
        assert_eq!(result.target_context_window_tokens, None);
        assert_eq!(result.safe_context_budget_tokens, None);
    }

    #[test]
    fn budget_preflight_carries_condensation_recommendation() {
        let budget = assess_provider_context_budget(ProviderContextBudgetInput {
            total_chars: 6_600,
            current_turn_messages: 1,
            prior_chat_chars: 2_400,
            current_turn_chars: 500,
            ..Default::default()
        });
        let result = assess_route_switch_preflight_for_budget(
            &budget,
            &ResolvedModelProfile {
                context_window_tokens: Some(8_000),
                context_window_source:
                    crate::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            },
        );

        let plan = result.recommended_condensation.expect("plan");
        assert_eq!(plan.mode, ProviderContextCondensationMode::Summarize);
        assert_eq!(
            plan.target_artifact,
            Some(ProviderContextArtifact::PriorChat)
        );
    }
}
