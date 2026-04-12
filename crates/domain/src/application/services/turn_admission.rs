use crate::application::services::execution_guidance::{ExecutionCapability, ExecutionGuidance};
use crate::application::services::model_capability_support::{
    assess_lane_capability_support, profile_supports_lane_confidently, supports_multimodal_input,
    LaneCapabilitySupport,
};
use crate::application::services::model_lane_resolution::{
    resolve_lane_candidates, ResolvedModelCandidate, ResolvedModelProfile,
    ResolvedModelProfileConfidence,
};
use crate::application::services::provider_context_budget::{
    assess_provider_context_budget, provider_context_condensation_plan, ProviderContextBudgetInput,
    ProviderContextBudgetTier, ProviderContextCondensationMode, ProviderContextCondensationPlan,
};
use crate::application::services::route_switch_preflight::{
    assess_route_switch_preflight_for_budget, RouteSwitchStatus,
};
use crate::application::services::runtime_calibration::{
    should_suppress_route_choice, RuntimeCalibrationRecord,
};
use crate::application::services::turn_model_routing::{
    infer_turn_capability_requirement, TurnCapabilityRequirement, TurnRouteOverride,
};
use crate::config::schema::{CapabilityLane, Config, ModelFeature};
use crate::domain::turn_admission::{
    AdmissionRepairHint, CandidateAdmissionReason, ContextPressureState, TurnAdmissionAction,
    TurnAdmissionSnapshot, TurnIntentCategory,
};
use crate::ports::model_profile_catalog::ModelProfileCatalogPort;
use crate::ports::provider::ProviderCapabilities;
use crate::ports::tool::{ToolRuntimeRole, ToolSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateAdmissionDecision {
    pub snapshot: TurnAdmissionSnapshot,
    pub required_lane: Option<CapabilityLane>,
    pub route_override: Option<TurnRouteOverride>,
    pub reasons: Vec<CandidateAdmissionReason>,
    pub recommended_action: Option<AdmissionRepairHint>,
    pub condensation_plan: Option<ProviderContextCondensationPlan>,
    pub requires_compaction: bool,
}

pub struct TurnAdmissionInput<'a> {
    pub config: Option<&'a Config>,
    pub user_message: &'a str,
    pub execution_guidance: Option<&'a ExecutionGuidance>,
    pub tool_specs: &'a [ToolSpec],
    pub current_provider: &'a str,
    pub current_model: &'a str,
    pub current_lane: Option<CapabilityLane>,
    pub current_profile: &'a ResolvedModelProfile,
    pub provider_capabilities: &'a ProviderCapabilities,
    pub provider_context: ProviderContextBudgetInput,
    pub calibration_records: &'a [RuntimeCalibrationRecord],
    pub catalog: Option<&'a dyn ModelProfileCatalogPort>,
}

pub fn assess_turn_admission(input: TurnAdmissionInput<'_>) -> CandidateAdmissionDecision {
    let intent = classify_turn_intent(
        input.user_message,
        input.execution_guidance,
        input.tool_specs,
    );
    let required_lane = required_lane_for_intent(intent);
    let mut reasons = Vec::new();
    let mut route_override = None;
    let mut requires_compaction = false;
    let mut required_lane_unsatisfied = false;

    let budget_assessment = assess_provider_context_budget(input.provider_context);
    let condensation_plan = provider_context_condensation_plan(&budget_assessment);
    let window_preflight =
        assess_route_switch_preflight_for_budget(&budget_assessment, input.current_profile);
    let pressure_state = classify_pressure_state(&budget_assessment.tier, window_preflight.status);
    push_pressure_reason(&mut reasons, pressure_state);
    push_window_metadata_reason(
        &mut reasons,
        pressure_state,
        window_preflight.status,
        input.current_profile,
    );

    if let Some(required_lane) = required_lane {
        reasons.push(CandidateAdmissionReason::RequiresLane(required_lane));
        if !current_candidate_supports_lane(
            required_lane,
            input.current_profile,
            input.provider_capabilities,
        ) {
            if let Some(config) = input.config {
                route_override = resolve_required_lane_override(
                    config,
                    required_lane,
                    input.current_provider,
                    input.current_model,
                    input.calibration_records,
                    input.catalog,
                );
            }
            if route_override.is_none() {
                required_lane_unsatisfied = true;
                push_lane_support_reason(&mut reasons, required_lane, input.current_profile);
            }
        }
    }

    if let Some(current_lane) = input.current_lane {
        if lane_is_specialized(current_lane) && intent_uses_reasoning_lane(intent) {
            reasons.push(CandidateAdmissionReason::SpecializedLaneMismatch(
                current_lane,
            ));
            if let Some(config) = input.config {
                route_override = route_override.or_else(|| {
                    resolve_required_lane_override(
                        config,
                        CapabilityLane::Reasoning,
                        input.current_provider,
                        input.current_model,
                        input.calibration_records,
                        input.catalog,
                    )
                });
            }
        }
    }

    if intent == TurnIntentCategory::ToolHeavy
        && current_candidate_explicitly_lacks_tool_support(
            input.current_lane,
            input.current_profile,
            input.provider_capabilities,
        )
    {
        reasons.push(CandidateAdmissionReason::MissingFeature(
            ModelFeature::ToolCalling,
        ));
        if let Some(config) = input.config {
            route_override = route_override.or_else(|| {
                resolve_tool_capable_reasoning_override(
                    config,
                    input.current_provider,
                    input.current_model,
                    input.calibration_records,
                    input.catalog,
                )
            });
        }
    }

    if should_suppress_route_choice(
        input.calibration_records,
        input.current_provider,
        input.current_model,
    ) {
        reasons.push(CandidateAdmissionReason::CalibrationSuppressedRoute);
        if let Some(config) = input.config {
            let lane = required_lane
                .or(input.current_lane)
                .unwrap_or(CapabilityLane::Reasoning);
            route_override = route_override.or_else(|| {
                resolve_required_lane_override(
                    config,
                    lane,
                    input.current_provider,
                    input.current_model,
                    input.calibration_records,
                    input.catalog,
                )
            });
        }
    }

    if matches!(pressure_state, ContextPressureState::Critical) {
        requires_compaction = true;
    }

    let action = if (matches!(pressure_state, ContextPressureState::OverflowRisk)
        || required_lane_unsatisfied)
        && route_override.is_none()
    {
        TurnAdmissionAction::Block
    } else if route_override.is_some() {
        TurnAdmissionAction::Reroute
    } else if requires_compaction {
        TurnAdmissionAction::Compact
    } else {
        TurnAdmissionAction::Proceed
    };
    let recommended_action = derive_recommended_action(
        &route_override,
        required_lane,
        &reasons,
        condensation_plan,
        requires_compaction,
        action,
    );

    CandidateAdmissionDecision {
        snapshot: TurnAdmissionSnapshot {
            intent,
            pressure_state,
            action,
        },
        required_lane,
        route_override,
        reasons,
        recommended_action,
        condensation_plan,
        requires_compaction,
    }
}

fn derive_recommended_action(
    route_override: &Option<TurnRouteOverride>,
    required_lane: Option<CapabilityLane>,
    reasons: &[CandidateAdmissionReason],
    condensation_plan: Option<ProviderContextCondensationPlan>,
    requires_compaction: bool,
    action: TurnAdmissionAction,
) -> Option<AdmissionRepairHint> {
    if let Some(override_route) = route_override {
        return Some(AdmissionRepairHint::SwitchToLane(override_route.lane));
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            CandidateAdmissionReason::MissingFeature(ModelFeature::ToolCalling)
        )
    }) {
        return Some(AdmissionRepairHint::SwitchToToolCapableReasoning);
    }
    if let Some(lane) = reasons.iter().find_map(|reason| match reason {
        CandidateAdmissionReason::CapabilityMetadataUnknown(lane)
        | CandidateAdmissionReason::CapabilityMetadataStale(lane)
        | CandidateAdmissionReason::CapabilityMetadataLowConfidence(lane) => Some(*lane),
        _ => None,
    }) {
        return Some(AdmissionRepairHint::RefreshCapabilityMetadata(lane));
    }
    if matches!(action, TurnAdmissionAction::Block) {
        if let Some(lane) = required_lane {
            return Some(AdmissionRepairHint::SwitchToLane(lane));
        }
    }
    if matches!(action, TurnAdmissionAction::Block)
        && reasons
            .iter()
            .any(|reason| matches!(reason, CandidateAdmissionReason::CandidateWindowExceeded))
    {
        return Some(AdmissionRepairHint::StartFreshHandoff);
    }
    if condensation_plan.is_some_and(|plan| plan.mode == ProviderContextCondensationMode::Handoff) {
        return Some(AdmissionRepairHint::StartFreshHandoff);
    }
    if requires_compaction
        || reasons.iter().any(|reason| {
            matches!(
                reason,
                CandidateAdmissionReason::ProviderContextCritical
                    | CandidateAdmissionReason::ProviderContextOverflowRisk
                    | CandidateAdmissionReason::CandidateWindowNearLimit
            )
        })
    {
        return Some(AdmissionRepairHint::CompactSession);
    }
    None
}

fn classify_turn_intent(
    user_message: &str,
    guidance: Option<&ExecutionGuidance>,
    tool_specs: &[ToolSpec],
) -> TurnIntentCategory {
    if let Some(requirement) = infer_turn_capability_requirement(user_message) {
        return match requirement {
            TurnCapabilityRequirement::MultimodalUnderstanding => {
                TurnIntentCategory::MultimodalUnderstanding
            }
            TurnCapabilityRequirement::ImageGeneration => TurnIntentCategory::ImageGeneration,
            TurnCapabilityRequirement::AudioGeneration => TurnIntentCategory::AudioGeneration,
            TurnCapabilityRequirement::VideoGeneration => TurnIntentCategory::VideoGeneration,
            TurnCapabilityRequirement::MusicGeneration => TurnIntentCategory::MusicGeneration,
        };
    }

    if guidance.is_some_and(|guidance| {
        guidance.direct_resolution_ready
            && guidance
                .preferred_capabilities
                .contains(&ExecutionCapability::Delivery)
    }) {
        return TurnIntentCategory::Deliver;
    }

    if tool_specs.iter().any(|spec| {
        matches!(
            spec.runtime_role,
            Some(ToolRuntimeRole::MemoryMutation) | Some(ToolRuntimeRole::ProfileMutation)
        )
    }) && tool_specs.iter().all(|spec| {
        matches!(
            spec.runtime_role,
            Some(ToolRuntimeRole::MemoryMutation)
                | Some(ToolRuntimeRole::ProfileMutation)
                | Some(ToolRuntimeRole::RuntimeStateInspection)
        )
    }) {
        return TurnIntentCategory::Mutate;
    }

    if guidance.is_some_and(|guidance| guidance.prefer_answer_from_resolved_state) {
        return TurnIntentCategory::Recall;
    }

    if !tool_specs.is_empty() {
        return TurnIntentCategory::ToolHeavy;
    }

    TurnIntentCategory::Reply
}

fn required_lane_for_intent(intent: TurnIntentCategory) -> Option<CapabilityLane> {
    match intent {
        TurnIntentCategory::MultimodalUnderstanding => {
            Some(CapabilityLane::MultimodalUnderstanding)
        }
        TurnIntentCategory::ImageGeneration => Some(CapabilityLane::ImageGeneration),
        TurnIntentCategory::AudioGeneration => Some(CapabilityLane::AudioGeneration),
        TurnIntentCategory::VideoGeneration => Some(CapabilityLane::VideoGeneration),
        TurnIntentCategory::MusicGeneration => Some(CapabilityLane::MusicGeneration),
        _ => None,
    }
}

fn classify_pressure_state(
    tier: &ProviderContextBudgetTier,
    route_status: RouteSwitchStatus,
) -> ContextPressureState {
    match route_status {
        RouteSwitchStatus::TooLarge => ContextPressureState::OverflowRisk,
        RouteSwitchStatus::CompactRecommended => ContextPressureState::Critical,
        RouteSwitchStatus::Safe | RouteSwitchStatus::Unknown => match tier {
            ProviderContextBudgetTier::Healthy => ContextPressureState::Healthy,
            ProviderContextBudgetTier::Caution => ContextPressureState::Warning,
            ProviderContextBudgetTier::OverBudget => ContextPressureState::Critical,
        },
    }
}

fn push_pressure_reason(
    reasons: &mut Vec<CandidateAdmissionReason>,
    pressure_state: ContextPressureState,
) {
    match pressure_state {
        ContextPressureState::Healthy => {}
        ContextPressureState::Warning => {
            reasons.push(CandidateAdmissionReason::ProviderContextWarning);
        }
        ContextPressureState::Critical => {
            reasons.push(CandidateAdmissionReason::ProviderContextCritical);
            reasons.push(CandidateAdmissionReason::CandidateWindowNearLimit);
        }
        ContextPressureState::OverflowRisk => {
            reasons.push(CandidateAdmissionReason::ProviderContextOverflowRisk);
            reasons.push(CandidateAdmissionReason::CandidateWindowExceeded);
        }
    }
}

fn push_window_metadata_reason(
    reasons: &mut Vec<CandidateAdmissionReason>,
    pressure_state: ContextPressureState,
    route_status: RouteSwitchStatus,
    profile: &ResolvedModelProfile,
) {
    if !matches!(route_status, RouteSwitchStatus::Unknown)
        || matches!(pressure_state, ContextPressureState::Healthy)
    {
        return;
    }
    if profile.context_window_confidence() < ResolvedModelProfileConfidence::Medium {
        reasons.push(CandidateAdmissionReason::CandidateWindowMetadataUnknown);
    }
}

fn current_candidate_supports_lane(
    lane: CapabilityLane,
    profile: &ResolvedModelProfile,
    provider_capabilities: &ProviderCapabilities,
) -> bool {
    match lane {
        CapabilityLane::Reasoning | CapabilityLane::CheapReasoning => true,
        CapabilityLane::Embedding
        | CapabilityLane::ImageGeneration
        | CapabilityLane::AudioGeneration
        | CapabilityLane::VideoGeneration
        | CapabilityLane::MusicGeneration => profile_supports_lane_confidently(profile, lane),
        CapabilityLane::MultimodalUnderstanding => {
            supports_multimodal_input(provider_capabilities, profile)
        }
    }
}

fn push_lane_support_reason(
    reasons: &mut Vec<CandidateAdmissionReason>,
    lane: CapabilityLane,
    profile: &ResolvedModelProfile,
) {
    match assess_lane_capability_support(profile, lane) {
        LaneCapabilitySupport::Supported => {}
        LaneCapabilitySupport::MetadataUnknown => {
            reasons.push(CandidateAdmissionReason::CapabilityMetadataUnknown(lane));
        }
        LaneCapabilitySupport::MetadataStale => {
            reasons.push(CandidateAdmissionReason::CapabilityMetadataStale(lane));
        }
        LaneCapabilitySupport::MetadataLowConfidence => {
            reasons.push(CandidateAdmissionReason::CapabilityMetadataLowConfidence(
                lane,
            ));
        }
        LaneCapabilitySupport::MissingFeature(feature) => {
            reasons.push(CandidateAdmissionReason::MissingFeature(feature));
        }
    }
}

fn lane_is_specialized(lane: CapabilityLane) -> bool {
    matches!(
        lane,
        CapabilityLane::Embedding
            | CapabilityLane::ImageGeneration
            | CapabilityLane::AudioGeneration
            | CapabilityLane::VideoGeneration
            | CapabilityLane::MusicGeneration
    )
}

fn intent_uses_reasoning_lane(intent: TurnIntentCategory) -> bool {
    matches!(
        intent,
        TurnIntentCategory::Reply
            | TurnIntentCategory::Recall
            | TurnIntentCategory::Mutate
            | TurnIntentCategory::Deliver
            | TurnIntentCategory::ToolHeavy
    )
}

fn current_candidate_explicitly_lacks_tool_support(
    current_lane: Option<CapabilityLane>,
    profile: &ResolvedModelProfile,
    provider_capabilities: &ProviderCapabilities,
) -> bool {
    if provider_capabilities.native_tool_calling
        || profile.features.contains(&ModelFeature::ToolCalling)
    {
        return false;
    }

    matches!(
        current_lane,
        Some(
            CapabilityLane::Embedding
                | CapabilityLane::ImageGeneration
                | CapabilityLane::AudioGeneration
                | CapabilityLane::VideoGeneration
                | CapabilityLane::MusicGeneration
        )
    )
}

fn resolve_required_lane_override(
    config: &Config,
    lane: CapabilityLane,
    current_provider: &str,
    current_model: &str,
    calibration_records: &[RuntimeCalibrationRecord],
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> Option<TurnRouteOverride> {
    let candidates = resolve_lane_candidates(config, lane, catalog);
    select_override_candidate(
        lane,
        &candidates,
        current_provider,
        current_model,
        calibration_records,
    )
}

fn resolve_tool_capable_reasoning_override(
    config: &Config,
    current_provider: &str,
    current_model: &str,
    calibration_records: &[RuntimeCalibrationRecord],
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> Option<TurnRouteOverride> {
    let candidates = resolve_lane_candidates(config, CapabilityLane::Reasoning, catalog);
    candidates
        .iter()
        .enumerate()
        .find(|(_, candidate)| {
            (candidate.provider != current_provider || candidate.model != current_model)
                && !should_suppress_route_choice(
                    calibration_records,
                    &candidate.provider,
                    &candidate.model,
                )
                && candidate
                    .profile
                    .features
                    .contains(&ModelFeature::ToolCalling)
        })
        .map(|(index, candidate)| TurnRouteOverride {
            lane: CapabilityLane::Reasoning,
            provider: candidate.provider.clone(),
            model: candidate.model.clone(),
            candidate_index: Some(index),
        })
}

fn select_override_candidate(
    lane: CapabilityLane,
    candidates: &[ResolvedModelCandidate],
    current_provider: &str,
    current_model: &str,
    calibration_records: &[RuntimeCalibrationRecord],
) -> Option<TurnRouteOverride> {
    candidates
        .iter()
        .enumerate()
        .find(|(_, candidate)| {
            (candidate.provider != current_provider || candidate.model != current_model)
                && !should_suppress_route_choice(
                    calibration_records,
                    &candidate.provider,
                    &candidate.model,
                )
                && profile_supports_lane_confidently(&candidate.profile, lane)
        })
        .map(|(index, candidate)| TurnRouteOverride {
            lane,
            provider: candidate.provider.clone(),
            model: candidate.model.clone(),
            candidate_index: Some(index),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::model_lane_resolution::ResolvedModelProfileSource;
    use crate::application::services::runtime_calibration::{
        RuntimeCalibrationAction, RuntimeCalibrationComparison, RuntimeCalibrationDecisionKind,
        RuntimeCalibrationOutcome, RuntimeCalibrationRecord, RuntimeCalibrationSuppressionKey,
    };
    use crate::config::schema::{
        Config, ModelCandidateProfileConfig, ModelLaneCandidateConfig, ModelLaneConfig,
    };
    use crate::ports::tool::ToolRuntimeRole;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn direct_delivery_spec() -> ToolSpec {
        ToolSpec {
            name: "message_send".into(),
            description: "send".into(),
            parameters: serde_json::json!({"type": "object"}),
            runtime_role: Some(ToolRuntimeRole::DirectDelivery),
        }
    }

    fn suppressed_route(provider: &str, model: &str) -> RuntimeCalibrationRecord {
        RuntimeCalibrationRecord {
            decision_kind: RuntimeCalibrationDecisionKind::RouteChoice,
            decision_signature: format!("route:{provider}:{model}"),
            suppression_key: Some(RuntimeCalibrationSuppressionKey::Route {
                provider: provider.into(),
                model: model.into(),
            }),
            confidence_basis_points: 9_000,
            outcome: RuntimeCalibrationOutcome::Failed,
            comparison: RuntimeCalibrationComparison::OverconfidentFailure,
            recommended_action: RuntimeCalibrationAction::SuppressChoice,
            observed_at_unix: 100,
        }
    }

    #[test]
    fn multimodal_turn_reroutes_to_multimodal_lane() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MultimodalUnderstanding,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openai".into(),
                model: "vision".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(128_000),
                    max_output_tokens: None,
                    features: vec![ModelFeature::Vision],
                },
            }],
        });

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: Some(&config),
            user_message: "Describe [IMAGE:/tmp/cat.png]",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "qwen",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile::default(),
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.intent,
            TurnIntentCategory::MultimodalUnderstanding
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Reroute);
        assert_eq!(
            decision.route_override.expect("override").lane,
            CapabilityLane::MultimodalUnderstanding
        );
        assert_eq!(
            decision.recommended_action,
            Some(AdmissionRepairHint::SwitchToLane(
                CapabilityLane::MultimodalUnderstanding
            ))
        );
    }

    #[test]
    fn oversized_context_blocks_without_reroute() {
        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "hello",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openai",
            current_model: "gpt",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                context_window_tokens: Some(8_000),
                context_window_source: ResolvedModelProfileSource::ManualConfig,
                max_output_tokens: None,
                features: vec![],
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 50_000,
                prior_chat_messages: 5,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.pressure_state,
            ContextPressureState::OverflowRisk
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Block);
        assert_eq!(
            decision.recommended_action,
            Some(AdmissionRepairHint::StartFreshHandoff)
        );
    }

    #[test]
    fn oversized_context_with_low_confidence_window_reports_unknown_window_metadata() {
        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "continue",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "fallback-model",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                context_window_tokens: Some(8_000),
                context_window_source: ResolvedModelProfileSource::AdapterFallback,
                max_output_tokens: None,
                features: vec![],
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 50_000,
                prior_chat_messages: 5,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.pressure_state,
            ContextPressureState::Critical
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Compact);
        assert!(decision
            .reasons
            .contains(&CandidateAdmissionReason::CandidateWindowMetadataUnknown));
    }

    #[test]
    fn critical_context_includes_artifact_specific_condensation_plan() {
        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "continue",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "long-context-model",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                context_window_tokens: Some(8_000),
                context_window_source: ResolvedModelProfileSource::ManualConfig,
                max_output_tokens: None,
                features: vec![],
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 16_000,
                prior_chat_messages: 4,
                current_turn_messages: 1,
                prior_chat_chars: 12_000,
                scoped_context_chars: 700,
                current_turn_chars: 500,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        let plan = decision.condensation_plan.expect("condensation plan");
        assert_eq!(plan.mode, ProviderContextCondensationMode::Summarize);
        assert_eq!(
            plan.target_artifact,
            Some(
                crate::application::services::provider_context_budget::ProviderContextArtifact::PriorChat
            )
        );
        assert!(plan.prefer_cached_artifact);
    }

    #[test]
    fn specialized_embedding_lane_reroutes_reply_to_reasoning() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openai".into(),
                model: "gpt-5.4".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(200_000),
                    max_output_tokens: None,
                    features: vec![ModelFeature::ToolCalling],
                },
            }],
        });

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: Some(&config),
            user_message: "Explain this briefly",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "qwen/qwen3-embedding-8b",
            current_lane: Some(CapabilityLane::Embedding),
            current_profile: &ResolvedModelProfile {
                context_window_tokens: Some(32_000),
                context_window_source: ResolvedModelProfileSource::ManualConfig,
                max_output_tokens: None,
                features: vec![ModelFeature::Embedding],
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Reroute);
        assert_eq!(
            decision.route_override.expect("override").lane,
            CapabilityLane::Reasoning
        );
    }

    #[test]
    fn calibration_suppressed_current_route_reroutes_to_unsuppressed_candidate() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![
                ModelLaneCandidateConfig {
                    provider: "openrouter".into(),
                    model: "current".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig {
                        context_window_tokens: Some(128_000),
                        max_output_tokens: None,
                        features: vec![ModelFeature::ToolCalling],
                    },
                },
                ModelLaneCandidateConfig {
                    provider: "openrouter".into(),
                    model: "fallback".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig {
                        context_window_tokens: Some(128_000),
                        max_output_tokens: None,
                        features: vec![ModelFeature::ToolCalling],
                    },
                },
            ],
        });
        let calibrations = vec![suppressed_route("openrouter", "current")];

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: Some(&config),
            user_message: "Explain this briefly",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "current",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                context_window_tokens: Some(128_000),
                context_window_source: ResolvedModelProfileSource::ManualConfig,
                features: vec![ModelFeature::ToolCalling],
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities {
                native_tool_calling: true,
                vision: false,
                prompt_caching: false,
            },
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &calibrations,
            catalog: None,
        });

        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Reroute);
        assert!(decision
            .reasons
            .contains(&CandidateAdmissionReason::CalibrationSuppressedRoute));
        assert_eq!(decision.route_override.expect("override").model, "fallback");
    }

    #[test]
    fn delivery_turn_stays_structural_without_tool_heavy_classification() {
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "Send it there",
            execution_guidance: Some(&guidance),
            tool_specs: &[direct_delivery_spec()],
            current_provider: "openai",
            current_model: "gpt-5.4",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile::default(),
            provider_capabilities: &ProviderCapabilities {
                native_tool_calling: true,
                vision: false,
                prompt_caching: false,
            },
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_500,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(decision.snapshot.intent, TurnIntentCategory::Deliver);
    }

    #[test]
    fn stale_capability_metadata_is_reported_explicitly() {
        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "Describe [IMAGE:/tmp/cat.png]",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "vision-model",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                features: vec![ModelFeature::Vision],
                features_source: ResolvedModelProfileSource::CachedProviderCatalog,
                observed_at_unix: Some(1),
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert!(decision
            .reasons
            .contains(&CandidateAdmissionReason::CapabilityMetadataStale(
                CapabilityLane::MultimodalUnderstanding
            )));
    }

    #[test]
    fn low_confidence_capability_metadata_is_reported_explicitly() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();
        let observed_at = now.saturating_sub(8 * 24 * 60 * 60);

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "[GENERATE:IMAGE] poster concept",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "fallback-image-model",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                features: vec![ModelFeature::ImageGeneration],
                features_source: ResolvedModelProfileSource::AdapterFallback,
                observed_at_unix: Some(observed_at),
                ..Default::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert!(decision.reasons.contains(
            &CandidateAdmissionReason::CapabilityMetadataLowConfidence(
                CapabilityLane::ImageGeneration
            )
        ));
        assert_eq!(
            decision.recommended_action,
            Some(AdmissionRepairHint::RefreshCapabilityMetadata(
                CapabilityLane::ImageGeneration
            ))
        );
    }

    #[test]
    fn image_generation_without_compatible_candidate_blocks() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::ImageGeneration,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "plain-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(128_000),
                    max_output_tokens: None,
                    features: vec![ModelFeature::ToolCalling],
                },
            }],
        });

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: Some(&config),
            user_message: "[GENERATE:IMAGE] cover art",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "qwen/qwen3.6-plus",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                features: vec![ModelFeature::ToolCalling],
                features_source: ResolvedModelProfileSource::ManualConfig,
                ..ResolvedModelProfile::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.intent,
            TurnIntentCategory::ImageGeneration
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Block);
        assert!(decision
            .reasons
            .contains(&CandidateAdmissionReason::MissingFeature(
                ModelFeature::ImageGeneration
            )));
        assert_eq!(
            decision.recommended_action,
            Some(AdmissionRepairHint::SwitchToLane(
                CapabilityLane::ImageGeneration
            ))
        );
    }

    #[test]
    fn music_generation_turn_requires_music_lane() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MusicGeneration,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "music-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(128_000),
                    max_output_tokens: None,
                    features: vec![ModelFeature::MusicGeneration],
                },
            }],
        });

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: Some(&config),
            user_message: "[GENERATE:MUSIC] short intro theme",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "qwen/qwen3.6-plus",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile::default(),
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.intent,
            TurnIntentCategory::MusicGeneration
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Reroute);
        assert_eq!(
            decision.route_override.expect("override").lane,
            CapabilityLane::MusicGeneration
        );
    }

    #[test]
    fn audio_generation_turn_requires_audio_lane() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::AudioGeneration,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "audio-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(128_000),
                    max_output_tokens: None,
                    features: vec![ModelFeature::AudioGeneration],
                },
            }],
        });

        let decision = assess_turn_admission(TurnAdmissionInput {
            config: Some(&config),
            user_message: "[GENERATE:AUDIO] short narration",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "qwen/qwen3.6-plus",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile::default(),
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.intent,
            TurnIntentCategory::AudioGeneration
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Reroute);
        assert_eq!(
            decision.route_override.expect("override").lane,
            CapabilityLane::AudioGeneration
        );
    }

    #[test]
    fn universal_reasoning_candidate_can_admit_video_generation() {
        let decision = assess_turn_admission(TurnAdmissionInput {
            config: None,
            user_message: "[GENERATE:VIDEO] launch teaser",
            execution_guidance: None,
            tool_specs: &[],
            current_provider: "openrouter",
            current_model: "universal-media-model",
            current_lane: Some(CapabilityLane::Reasoning),
            current_profile: &ResolvedModelProfile {
                context_window_tokens: Some(256_000),
                context_window_source: ResolvedModelProfileSource::ManualConfig,
                features: vec![
                    ModelFeature::ToolCalling,
                    ModelFeature::ImageGeneration,
                    ModelFeature::AudioGeneration,
                    ModelFeature::VideoGeneration,
                    ModelFeature::MusicGeneration,
                ],
                features_source: ResolvedModelProfileSource::ManualConfig,
                ..ResolvedModelProfile::default()
            },
            provider_capabilities: &ProviderCapabilities::default(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 3_000,
                prior_chat_messages: 0,
                current_turn_messages: 1,
                ..Default::default()
            },
            calibration_records: &[],
            catalog: None,
        });

        assert_eq!(
            decision.snapshot.intent,
            TurnIntentCategory::VideoGeneration
        );
        assert_eq!(
            decision.required_lane,
            Some(CapabilityLane::VideoGeneration)
        );
        assert_eq!(decision.snapshot.action, TurnAdmissionAction::Proceed);
        assert!(decision.route_override.is_none());
        assert!(decision
            .reasons
            .contains(&CandidateAdmissionReason::RequiresLane(
                CapabilityLane::VideoGeneration
            )));
        assert!(!decision.reasons.iter().any(|reason| matches!(
            reason,
            CandidateAdmissionReason::MissingFeature(ModelFeature::VideoGeneration)
                | CandidateAdmissionReason::CapabilityMetadataUnknown(
                    CapabilityLane::VideoGeneration
                )
                | CandidateAdmissionReason::CapabilityMetadataStale(
                    CapabilityLane::VideoGeneration
                )
                | CandidateAdmissionReason::CapabilityMetadataLowConfidence(
                    CapabilityLane::VideoGeneration
                )
        )));
    }
}
