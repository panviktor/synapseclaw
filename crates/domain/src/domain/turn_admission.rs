use crate::config::schema::{CapabilityLane, ModelFeature};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnIntentCategory {
    Reply,
    Recall,
    Mutate,
    Deliver,
    ToolHeavy,
    MultimodalUnderstanding,
    ImageGeneration,
    AudioGeneration,
    VideoGeneration,
    MusicGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextPressureState {
    Healthy,
    Warning,
    Critical,
    OverflowRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnAdmissionAction {
    Proceed,
    Reroute,
    Compact,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRepairHint {
    SwitchToLane(CapabilityLane),
    SwitchToToolCapableReasoning,
    RefreshCapabilityMetadata(CapabilityLane),
    CompactSession,
    StartFreshHandoff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateAdmissionReason {
    RequiresLane(CapabilityLane),
    MissingFeature(ModelFeature),
    CapabilityMetadataUnknown(CapabilityLane),
    CapabilityMetadataStale(CapabilityLane),
    CapabilityMetadataLowConfidence(CapabilityLane),
    SpecializedLaneMismatch(CapabilityLane),
    CandidateWindowNearLimit,
    CandidateWindowExceeded,
    ProviderContextWarning,
    ProviderContextCritical,
    ProviderContextOverflowRisk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAdmissionSnapshot {
    pub intent: TurnIntentCategory,
    pub pressure_state: ContextPressureState,
    pub action: TurnAdmissionAction,
}

pub fn turn_intent_name(intent: TurnIntentCategory) -> &'static str {
    match intent {
        TurnIntentCategory::Reply => "reply",
        TurnIntentCategory::Recall => "recall",
        TurnIntentCategory::Mutate => "mutate",
        TurnIntentCategory::Deliver => "deliver",
        TurnIntentCategory::ToolHeavy => "tool_heavy",
        TurnIntentCategory::MultimodalUnderstanding => "multimodal_understanding",
        TurnIntentCategory::ImageGeneration => "image_generation",
        TurnIntentCategory::AudioGeneration => "audio_generation",
        TurnIntentCategory::VideoGeneration => "video_generation",
        TurnIntentCategory::MusicGeneration => "music_generation",
    }
}

pub fn context_pressure_state_name(state: ContextPressureState) -> &'static str {
    match state {
        ContextPressureState::Healthy => "healthy",
        ContextPressureState::Warning => "warning",
        ContextPressureState::Critical => "critical",
        ContextPressureState::OverflowRisk => "overflow_risk",
    }
}

pub fn turn_admission_action_name(action: TurnAdmissionAction) -> &'static str {
    match action {
        TurnAdmissionAction::Proceed => "proceed",
        TurnAdmissionAction::Reroute => "reroute",
        TurnAdmissionAction::Compact => "compact",
        TurnAdmissionAction::Block => "block",
    }
}

pub fn admission_repair_hint_name(hint: AdmissionRepairHint) -> &'static str {
    match hint {
        AdmissionRepairHint::SwitchToLane(_) => "switch_lane",
        AdmissionRepairHint::SwitchToToolCapableReasoning => "switch_tool_capable_reasoning",
        AdmissionRepairHint::RefreshCapabilityMetadata(_) => "refresh_capability_metadata",
        AdmissionRepairHint::CompactSession => "compact_session",
        AdmissionRepairHint::StartFreshHandoff => "start_fresh_handoff",
    }
}

pub fn candidate_admission_reason_label(reason: &CandidateAdmissionReason) -> String {
    match reason {
        CandidateAdmissionReason::RequiresLane(lane) => {
            format!("requires_{}", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::MissingFeature(feature) => {
            format!("missing_{}", model_feature_name(feature.clone()))
        }
        CandidateAdmissionReason::CapabilityMetadataUnknown(lane) => {
            format!("metadata_unknown_{}", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::CapabilityMetadataStale(lane) => {
            format!("metadata_stale_{}", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::CapabilityMetadataLowConfidence(lane) => {
            format!("metadata_low_confidence_{}", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::SpecializedLaneMismatch(lane) => {
            format!("lane_mismatch_{}", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::CandidateWindowNearLimit => "window_near_limit".to_string(),
        CandidateAdmissionReason::CandidateWindowExceeded => "window_exceeded".to_string(),
        CandidateAdmissionReason::ProviderContextWarning => "context_warning".to_string(),
        CandidateAdmissionReason::ProviderContextCritical => "context_critical".to_string(),
        CandidateAdmissionReason::ProviderContextOverflowRisk => {
            "context_overflow_risk".to_string()
        }
    }
}

pub fn admission_repair_hint_label(hint: AdmissionRepairHint) -> String {
    match hint {
        AdmissionRepairHint::SwitchToLane(lane) => {
            format!(
                "{}:{}",
                admission_repair_hint_name(hint),
                capability_lane_name(lane)
            )
        }
        AdmissionRepairHint::RefreshCapabilityMetadata(lane) => {
            format!(
                "{}:{}",
                admission_repair_hint_name(hint),
                capability_lane_name(lane)
            )
        }
        AdmissionRepairHint::SwitchToToolCapableReasoning
        | AdmissionRepairHint::CompactSession
        | AdmissionRepairHint::StartFreshHandoff => admission_repair_hint_name(hint).to_string(),
    }
}

fn capability_lane_name(lane: CapabilityLane) -> &'static str {
    match lane {
        CapabilityLane::Reasoning => "reasoning",
        CapabilityLane::CheapReasoning => "cheap_reasoning",
        CapabilityLane::Embedding => "embedding",
        CapabilityLane::MultimodalUnderstanding => "multimodal_understanding",
        CapabilityLane::ImageGeneration => "image_generation",
        CapabilityLane::AudioGeneration => "audio_generation",
        CapabilityLane::VideoGeneration => "video_generation",
        CapabilityLane::MusicGeneration => "music_generation",
    }
}

fn model_feature_name(feature: ModelFeature) -> &'static str {
    match feature {
        ModelFeature::ToolCalling => "tool_calling",
        ModelFeature::Vision => "vision",
        ModelFeature::Embedding => "embedding",
        ModelFeature::MultimodalUnderstanding => "multimodal_understanding",
        ModelFeature::ImageGeneration => "image_generation",
        ModelFeature::AudioGeneration => "audio_generation",
        ModelFeature::VideoGeneration => "video_generation",
        ModelFeature::MusicGeneration => "music_generation",
        ModelFeature::ServerContinuation => "server_continuation",
        ModelFeature::PromptCaching => "prompt_caching",
    }
}
