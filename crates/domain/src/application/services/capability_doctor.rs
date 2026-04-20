use crate::application::services::model_capability_support::{
    assess_lane_capability_support, LaneCapabilitySupport,
};
use crate::application::services::model_lane_resolution::{
    resolve_lane_candidates, resolve_route_selection_profile,
    resolved_model_profile_confidence_name, resolved_model_profile_freshness_name,
    resolved_model_profile_source_name, ResolvedModelProfile, ResolvedModelProfileConfidence,
    ResolvedModelProfileFreshness,
};
use crate::application::services::model_preset_resolution::resolve_effective_model_lanes;
use crate::application::services::provider_native_context_policy::{
    resolve_provider_native_context_policy, ProviderNativeContextPolicyInput,
};
use crate::config::schema::{CapabilityLane, Config, ModelFeature};
use crate::domain::memory::EmbeddingProfile;
use crate::ports::model_profile_catalog::ModelProfileCatalogPort;
use crate::ports::provider::ProviderCapabilities;
use crate::ports::route_selection::RouteSelection;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDoctorSeverity {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDoctorSubsystem {
    ProviderKey,
    ProviderAdapter,
    ProviderPlan,
    ModelProfile,
    Route,
    Lane,
    ToolRegistry,
    MemoryBackend,
    EmbeddingBackend,
    ChannelDelivery,
    ReasoningControls,
    NativeContinuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDoctorReadiness {
    Ready,
    MissingKey,
    MissingAdapter,
    MissingModelProfile,
    UnknownContextWindow,
    StaleCatalog,
    LowConfidenceMetadata,
    UnsupportedModality,
    UnsupportedToolCapability,
    IgnoredReasoningControls,
    UnsupportedNativeContinuation,
    ProviderPlanDenied,
    DegradedBackend,
    NotConfigured,
    Unknown,
}

impl CapabilityDoctorReadiness {
    pub fn severity(self) -> CapabilityDoctorSeverity {
        match self {
            Self::Ready => CapabilityDoctorSeverity::Ok,
            Self::MissingKey
            | Self::MissingAdapter
            | Self::ProviderPlanDenied
            | Self::DegradedBackend => CapabilityDoctorSeverity::Error,
            Self::MissingModelProfile
            | Self::UnknownContextWindow
            | Self::StaleCatalog
            | Self::LowConfidenceMetadata
            | Self::UnsupportedModality
            | Self::UnsupportedToolCapability
            | Self::IgnoredReasoningControls
            | Self::UnsupportedNativeContinuation
            | Self::NotConfigured
            | Self::Unknown => CapabilityDoctorSeverity::Warn,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDoctorProviderKeyStatus {
    Present,
    Missing,
    NotRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDoctorAdapterStatus {
    Available,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapabilityDoctorBackendStatus<'a> {
    pub configured: bool,
    pub healthy: Option<bool>,
    pub name: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapabilityDoctorChannelStatus<'a> {
    pub surface: Option<&'a str>,
    pub available: Option<bool>,
}

pub struct CapabilityDoctorInput<'a> {
    pub config: &'a Config,
    pub route: &'a RouteSelection,
    pub catalog: Option<&'a dyn ModelProfileCatalogPort>,
    pub provider_adapter: CapabilityDoctorAdapterStatus,
    pub provider_key: CapabilityDoctorProviderKeyStatus,
    pub provider_capabilities: ProviderCapabilities,
    pub provider_plan_denial: Option<&'a str>,
    pub tool_registry_count: usize,
    pub memory_backend: CapabilityDoctorBackendStatus<'a>,
    pub embedding_profile: Option<&'a EmbeddingProfile>,
    pub channel_delivery: CapabilityDoctorChannelStatus<'a>,
    pub generated_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDoctorRouteSnapshot {
    pub provider: String,
    pub model: String,
    pub lane: Option<String>,
    pub candidate_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDoctorModelProfileSnapshot {
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub features: Vec<String>,
    pub context_window_source: String,
    pub context_window_freshness: String,
    pub context_window_confidence: String,
    pub max_output_source: String,
    pub max_output_freshness: String,
    pub max_output_confidence: String,
    pub features_source: String,
    pub features_freshness: String,
    pub features_confidence: String,
    pub observed_at_unix: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDoctorSummary {
    pub ok: usize,
    pub warn: usize,
    pub error: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDoctorNode {
    pub subsystem: CapabilityDoctorSubsystem,
    pub subject: String,
    pub readiness: CapabilityDoctorReadiness,
    pub severity: CapabilityDoctorSeverity,
    pub evidence: Vec<String>,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDoctorReport {
    pub generated_at_unix: i64,
    pub route: CapabilityDoctorRouteSnapshot,
    pub model_profile: CapabilityDoctorModelProfileSnapshot,
    pub summary: CapabilityDoctorSummary,
    pub nodes: Vec<CapabilityDoctorNode>,
}

pub fn build_capability_doctor_report(input: CapabilityDoctorInput<'_>) -> CapabilityDoctorReport {
    let profile = resolve_route_selection_profile(input.config, input.route, input.catalog);
    let mut nodes = Vec::new();

    push_provider_key_node(&mut nodes, &input);
    push_provider_adapter_node(&mut nodes, &input);
    push_provider_plan_node(&mut nodes, &input);
    push_model_profile_node(&mut nodes, input.route, &profile);
    push_route_node(&mut nodes, input.route, input.provider_adapter, &profile);
    push_current_lane_node(&mut nodes, input.route, &profile);
    push_configured_lane_nodes(&mut nodes, input.config, input.catalog, input.route);
    push_tool_registry_node(&mut nodes, input.tool_registry_count, &profile, &input);
    push_memory_backend_node(&mut nodes, &input);
    push_embedding_backend_node(&mut nodes, &input);
    push_channel_delivery_node(&mut nodes, &input);
    push_reasoning_controls_node(&mut nodes, &input);
    push_native_continuation_node(&mut nodes, &profile, &input);

    let summary = summarize_nodes(&nodes);

    CapabilityDoctorReport {
        generated_at_unix: input.generated_at_unix,
        route: CapabilityDoctorRouteSnapshot {
            provider: input.route.provider.clone(),
            model: input.route.model.clone(),
            lane: input.route.lane.map(|lane| lane.as_str().to_string()),
            candidate_index: input.route.candidate_index,
        },
        model_profile: profile_snapshot(&profile),
        summary,
        nodes,
    }
}

fn push_provider_key_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let (readiness, evidence, recommendation) = match input.provider_key {
        CapabilityDoctorProviderKeyStatus::Present => (
            CapabilityDoctorReadiness::Ready,
            vec!["provider key is configured".to_string()],
            None,
        ),
        CapabilityDoctorProviderKeyStatus::NotRequired => (
            CapabilityDoctorReadiness::Ready,
            vec!["provider does not require an API key".to_string()],
            None,
        ),
        CapabilityDoctorProviderKeyStatus::Missing => (
            CapabilityDoctorReadiness::MissingKey,
            vec!["no configured API key or provider key env var was found".to_string()],
            Some(format!(
                "configure an API key for provider `{}` or choose a local/provider-auth route",
                input.route.provider
            )),
        ),
        CapabilityDoctorProviderKeyStatus::Unknown => (
            CapabilityDoctorReadiness::Unknown,
            vec!["provider key availability is unknown".to_string()],
            Some("verify provider auth before relying on this route".to_string()),
        ),
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ProviderKey,
        input.route.provider.as_str(),
        readiness,
        evidence,
        recommendation,
    );
}

fn push_provider_adapter_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let (readiness, evidence, recommendation) = match input.provider_adapter {
        CapabilityDoctorAdapterStatus::Available => (
            CapabilityDoctorReadiness::Ready,
            vec!["provider adapter is registered".to_string()],
            None,
        ),
        CapabilityDoctorAdapterStatus::Missing => (
            CapabilityDoctorReadiness::MissingAdapter,
            vec!["provider adapter is not registered".to_string()],
            Some("choose a known provider or add an adapter registration".to_string()),
        ),
        CapabilityDoctorAdapterStatus::Unknown => (
            CapabilityDoctorReadiness::Unknown,
            vec!["provider adapter availability is unknown".to_string()],
            Some("inspect provider adapter registration".to_string()),
        ),
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ProviderAdapter,
        input.route.provider.as_str(),
        readiness,
        evidence,
        recommendation,
    );
}

fn push_provider_plan_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let Some(denial) = input.provider_plan_denial else {
        return;
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ProviderPlan,
        input.route.provider.as_str(),
        CapabilityDoctorReadiness::ProviderPlanDenied,
        vec![denial.to_string()],
        Some("choose another route or update provider plan access".to_string()),
    );
}

fn push_model_profile_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    route: &RouteSelection,
    profile: &ResolvedModelProfile,
) {
    let readiness = model_profile_readiness(profile);
    let evidence = vec![
        format!(
            "ctx={}/{}/{}",
            resolved_model_profile_source_name(profile.context_window_source),
            resolved_model_profile_freshness_name(profile.context_window_freshness()),
            resolved_model_profile_confidence_name(profile.context_window_confidence()),
        ),
        format!(
            "output={}/{}/{}",
            resolved_model_profile_source_name(profile.max_output_source),
            resolved_model_profile_freshness_name(profile.max_output_freshness()),
            resolved_model_profile_confidence_name(profile.max_output_confidence()),
        ),
        format!(
            "features={}/{}/{}",
            resolved_model_profile_source_name(profile.features_source),
            resolved_model_profile_freshness_name(profile.features_freshness()),
            resolved_model_profile_confidence_name(profile.features_confidence()),
        ),
    ];
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ModelProfile,
        format!("{}:{}", route.provider, route.model),
        readiness,
        evidence,
        recommendation_for_readiness(readiness, route),
    );
}

fn push_route_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    route: &RouteSelection,
    adapter: CapabilityDoctorAdapterStatus,
    profile: &ResolvedModelProfile,
) {
    let readiness = if route.provider.trim().is_empty() || route.model.trim().is_empty() {
        CapabilityDoctorReadiness::MissingModelProfile
    } else if adapter == CapabilityDoctorAdapterStatus::Missing {
        CapabilityDoctorReadiness::MissingAdapter
    } else if !profile.context_window_known() {
        CapabilityDoctorReadiness::UnknownContextWindow
    } else {
        CapabilityDoctorReadiness::Ready
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::Route,
        format!("{}:{}", route.provider, route.model),
        readiness,
        vec![format!(
            "lane={}",
            route.lane.map(|lane| lane.as_str()).unwrap_or("default")
        )],
        recommendation_for_readiness(readiness, route),
    );
}

fn push_current_lane_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    route: &RouteSelection,
    profile: &ResolvedModelProfile,
) {
    let Some(lane) = route.lane else {
        push_node(
            nodes,
            CapabilityDoctorSubsystem::Lane,
            "current:default",
            CapabilityDoctorReadiness::Ready,
            vec!["default reasoning route".to_string()],
            None,
        );
        return;
    };
    push_lane_node(
        nodes,
        "current",
        lane,
        &route.provider,
        &route.model,
        profile,
    );
}

fn push_configured_lane_nodes(
    nodes: &mut Vec<CapabilityDoctorNode>,
    config: &Config,
    catalog: Option<&dyn ModelProfileCatalogPort>,
    route: &RouteSelection,
) {
    for lane_config in resolve_effective_model_lanes(config) {
        if Some(lane_config.lane) == route.lane {
            continue;
        }
        let candidates = resolve_lane_candidates(config, lane_config.lane, catalog);
        let Some(candidate) = candidates.first() else {
            continue;
        };
        push_lane_node(
            nodes,
            "configured",
            lane_config.lane,
            candidate.provider.as_str(),
            candidate.model.as_str(),
            &candidate.profile,
        );
    }
}

fn push_lane_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    prefix: &str,
    lane: CapabilityLane,
    provider: &str,
    model: &str,
    profile: &ResolvedModelProfile,
) {
    let readiness = lane_readiness(lane, profile);
    push_node(
        nodes,
        CapabilityDoctorSubsystem::Lane,
        format!("{prefix}:{}", lane.as_str()),
        readiness,
        vec![format!("{provider}:{model}")],
        recommendation_for_lane_readiness(readiness, lane),
    );
}

fn push_tool_registry_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    tool_registry_count: usize,
    profile: &ResolvedModelProfile,
    input: &CapabilityDoctorInput<'_>,
) {
    let tool_calling_from_profile = profile.features_confidence()
        >= ResolvedModelProfileConfidence::Medium
        && profile.features.contains(&ModelFeature::ToolCalling);
    let readiness = if tool_registry_count == 0 {
        CapabilityDoctorReadiness::NotConfigured
    } else if input.provider_capabilities.native_tool_calling || tool_calling_from_profile {
        CapabilityDoctorReadiness::Ready
    } else if profile.features_known() {
        CapabilityDoctorReadiness::UnsupportedToolCapability
    } else {
        CapabilityDoctorReadiness::LowConfidenceMetadata
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ToolRegistry,
        "runtime_tools",
        readiness,
        vec![
            format!("registered_tools={tool_registry_count}"),
            format!(
                "provider_native_tools={}",
                input.provider_capabilities.native_tool_calling
            ),
        ],
        match readiness {
            CapabilityDoctorReadiness::UnsupportedToolCapability => Some(
                "switch to a tool-capable route or disable tool-dependent workflows".to_string(),
            ),
            CapabilityDoctorReadiness::LowConfidenceMetadata => {
                Some("refresh model capability metadata before tool-heavy runs".to_string())
            }
            _ => None,
        },
    );
}

fn push_memory_backend_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let readiness = if !input.memory_backend.configured {
        CapabilityDoctorReadiness::NotConfigured
    } else {
        match input.memory_backend.healthy {
            Some(true) => CapabilityDoctorReadiness::Ready,
            Some(false) => CapabilityDoctorReadiness::DegradedBackend,
            None => CapabilityDoctorReadiness::Unknown,
        }
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::MemoryBackend,
        input.memory_backend.name.unwrap_or("memory"),
        readiness,
        vec![format!("healthy={:?}", input.memory_backend.healthy)],
        match readiness {
            CapabilityDoctorReadiness::DegradedBackend => {
                Some("check memory backend connectivity and schema".to_string())
            }
            CapabilityDoctorReadiness::NotConfigured => {
                Some("configure a memory backend if durable memory is required".to_string())
            }
            CapabilityDoctorReadiness::Unknown => {
                Some("run a memory health check before relying on durable recall".to_string())
            }
            _ => None,
        },
    );
}

fn push_embedding_backend_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let (subject, readiness, evidence, recommendation) = match input.embedding_profile {
        Some(profile) if profile.dimensions > 0 => (
            format!("{}:{}", profile.provider_family, profile.model_id),
            CapabilityDoctorReadiness::Ready,
            vec![format!("dimensions={}", profile.dimensions)],
            None,
        ),
        Some(profile) => (
            format!("{}:{}", profile.provider_family, profile.model_id),
            CapabilityDoctorReadiness::DegradedBackend,
            vec!["dimensions=0".to_string()],
            Some("configure a non-zero embedding profile before vector recall".to_string()),
        ),
        None => (
            "embedding".to_string(),
            CapabilityDoctorReadiness::NotConfigured,
            vec!["no embedding profile available".to_string()],
            Some("configure an embedding backend if vector recall is required".to_string()),
        ),
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::EmbeddingBackend,
        subject,
        readiness,
        evidence,
        recommendation,
    );
}

fn push_channel_delivery_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let readiness = match input.channel_delivery.available {
        Some(true) => CapabilityDoctorReadiness::Ready,
        Some(false) => CapabilityDoctorReadiness::DegradedBackend,
        None => CapabilityDoctorReadiness::NotConfigured,
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ChannelDelivery,
        input.channel_delivery.surface.unwrap_or("web"),
        readiness,
        vec![format!("available={:?}", input.channel_delivery.available)],
        match readiness {
            CapabilityDoctorReadiness::DegradedBackend => {
                Some("check channel registry and delivery credentials".to_string())
            }
            CapabilityDoctorReadiness::NotConfigured => {
                Some("no channel delivery surface is active for this runtime command".to_string())
            }
            _ => None,
        },
    );
}

fn push_reasoning_controls_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    input: &CapabilityDoctorInput<'_>,
) {
    let requested = input.config.runtime.reasoning_enabled == Some(true)
        || input.config.runtime.reasoning_effort.is_some();
    if !requested {
        return;
    }

    let policy = crate::config::model_catalog::model_request_policy(
        &input.route.provider,
        &input.route.model,
    );
    let readiness = match (
        policy.as_ref(),
        input.config.runtime.reasoning_effort.as_deref(),
    ) {
        (Some(policy), Some(effort)) if policy.resolve_reasoning_effort(effort).is_some() => {
            CapabilityDoctorReadiness::Ready
        }
        (Some(_), None) => CapabilityDoctorReadiness::Ready,
        _ => CapabilityDoctorReadiness::IgnoredReasoningControls,
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::ReasoningControls,
        format!("{}:{}", input.route.provider, input.route.model),
        readiness,
        vec![format!(
            "reasoning_enabled={:?} reasoning_effort={:?}",
            input.config.runtime.reasoning_enabled, input.config.runtime.reasoning_effort
        )],
        match readiness {
            CapabilityDoctorReadiness::IgnoredReasoningControls => Some(
                "use a model with catalog request policy support or remove reasoning controls"
                    .to_string(),
            ),
            _ => None,
        },
    );
}

fn push_native_continuation_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    profile: &ResolvedModelProfile,
    input: &CapabilityDoctorInput<'_>,
) {
    let native_policy = resolve_provider_native_context_policy(ProviderNativeContextPolicyInput {
        profile,
        provider_prompt_caching: input.provider_capabilities.prompt_caching,
        operator_prompt_caching_enabled: input.config.agent.prompt_caching,
    });
    let readiness = if native_policy.server_continuation_supported {
        CapabilityDoctorReadiness::Ready
    } else if profile.features_known()
        && profile.features_confidence() >= ResolvedModelProfileConfidence::Medium
    {
        CapabilityDoctorReadiness::UnsupportedNativeContinuation
    } else {
        CapabilityDoctorReadiness::LowConfidenceMetadata
    };
    push_node(
        nodes,
        CapabilityDoctorSubsystem::NativeContinuation,
        format!("{}:{}", input.route.provider, input.route.model),
        readiness,
        vec![format!(
            "server_continuation_supported={}",
            native_policy.server_continuation_supported
        )],
        match readiness {
            CapabilityDoctorReadiness::UnsupportedNativeContinuation => {
                Some("do not rely on provider-native continuation for this route".to_string())
            }
            CapabilityDoctorReadiness::LowConfidenceMetadata => Some(
                "refresh feature metadata before assuming native continuation support".to_string(),
            ),
            _ => None,
        },
    );
}

fn model_profile_readiness(profile: &ResolvedModelProfile) -> CapabilityDoctorReadiness {
    if !profile.context_window_known() {
        return CapabilityDoctorReadiness::UnknownContextWindow;
    }
    if profile.context_window_freshness() == ResolvedModelProfileFreshness::Stale
        || profile.max_output_freshness() == ResolvedModelProfileFreshness::Stale
        || profile.features_freshness() == ResolvedModelProfileFreshness::Stale
    {
        return CapabilityDoctorReadiness::StaleCatalog;
    }
    if profile.context_window_confidence() <= ResolvedModelProfileConfidence::Low
        || profile.max_output_confidence() <= ResolvedModelProfileConfidence::Low
        || profile.features_confidence() <= ResolvedModelProfileConfidence::Low
    {
        return CapabilityDoctorReadiness::LowConfidenceMetadata;
    }
    CapabilityDoctorReadiness::Ready
}

fn lane_readiness(
    lane: CapabilityLane,
    profile: &ResolvedModelProfile,
) -> CapabilityDoctorReadiness {
    match assess_lane_capability_support(profile, lane) {
        LaneCapabilitySupport::Supported => CapabilityDoctorReadiness::Ready,
        LaneCapabilitySupport::MissingFeature(_) => CapabilityDoctorReadiness::UnsupportedModality,
        LaneCapabilitySupport::MetadataUnknown => CapabilityDoctorReadiness::LowConfidenceMetadata,
        LaneCapabilitySupport::MetadataStale => CapabilityDoctorReadiness::StaleCatalog,
        LaneCapabilitySupport::MetadataLowConfidence => {
            CapabilityDoctorReadiness::LowConfidenceMetadata
        }
    }
}

fn recommendation_for_readiness(
    readiness: CapabilityDoctorReadiness,
    route: &RouteSelection,
) -> Option<String> {
    match readiness {
        CapabilityDoctorReadiness::UnknownContextWindow
        | CapabilityDoctorReadiness::MissingModelProfile
        | CapabilityDoctorReadiness::LowConfidenceMetadata => Some(format!(
            "refresh model metadata for `{}` with `synapseclaw doctor models --provider {}`",
            route.model, route.provider
        )),
        CapabilityDoctorReadiness::StaleCatalog => Some(format!(
            "refresh stale model metadata with `synapseclaw doctor models --provider {}`",
            route.provider
        )),
        CapabilityDoctorReadiness::MissingAdapter => {
            Some("choose a known provider or add an adapter registration".to_string())
        }
        _ => None,
    }
}

fn recommendation_for_lane_readiness(
    readiness: CapabilityDoctorReadiness,
    lane: CapabilityLane,
) -> Option<String> {
    match readiness {
        CapabilityDoctorReadiness::UnsupportedModality => Some(format!(
            "configure a model that supports `{}` or route this turn to another lane",
            lane.as_str()
        )),
        CapabilityDoctorReadiness::StaleCatalog => {
            Some("refresh capability metadata before using this lane".to_string())
        }
        CapabilityDoctorReadiness::LowConfidenceMetadata => Some(
            "raise model capability confidence with catalog metadata or manual profile".to_string(),
        ),
        _ => None,
    }
}

fn push_node(
    nodes: &mut Vec<CapabilityDoctorNode>,
    subsystem: CapabilityDoctorSubsystem,
    subject: impl Into<String>,
    readiness: CapabilityDoctorReadiness,
    evidence: Vec<String>,
    recommendation: Option<String>,
) {
    nodes.push(CapabilityDoctorNode {
        subsystem,
        subject: subject.into(),
        readiness,
        severity: readiness.severity(),
        evidence,
        recommendation,
    });
}

fn summarize_nodes(nodes: &[CapabilityDoctorNode]) -> CapabilityDoctorSummary {
    CapabilityDoctorSummary {
        ok: nodes
            .iter()
            .filter(|node| node.severity == CapabilityDoctorSeverity::Ok)
            .count(),
        warn: nodes
            .iter()
            .filter(|node| node.severity == CapabilityDoctorSeverity::Warn)
            .count(),
        error: nodes
            .iter()
            .filter(|node| node.severity == CapabilityDoctorSeverity::Error)
            .count(),
    }
}

fn profile_snapshot(profile: &ResolvedModelProfile) -> CapabilityDoctorModelProfileSnapshot {
    CapabilityDoctorModelProfileSnapshot {
        context_window_tokens: profile.context_window_tokens,
        max_output_tokens: profile.max_output_tokens,
        features: profile.features.iter().map(model_feature_name).collect(),
        context_window_source: resolved_model_profile_source_name(profile.context_window_source)
            .to_string(),
        context_window_freshness: resolved_model_profile_freshness_name(
            profile.context_window_freshness(),
        )
        .to_string(),
        context_window_confidence: resolved_model_profile_confidence_name(
            profile.context_window_confidence(),
        )
        .to_string(),
        max_output_source: resolved_model_profile_source_name(profile.max_output_source)
            .to_string(),
        max_output_freshness: resolved_model_profile_freshness_name(profile.max_output_freshness())
            .to_string(),
        max_output_confidence: resolved_model_profile_confidence_name(
            profile.max_output_confidence(),
        )
        .to_string(),
        features_source: resolved_model_profile_source_name(profile.features_source).to_string(),
        features_freshness: resolved_model_profile_freshness_name(profile.features_freshness())
            .to_string(),
        features_confidence: resolved_model_profile_confidence_name(profile.features_confidence())
            .to_string(),
        observed_at_unix: profile.observed_at_unix,
    }
}

fn model_feature_name(feature: &ModelFeature) -> String {
    match feature {
        ModelFeature::ToolCalling => "tool_calling",
        ModelFeature::Vision => "vision",
        ModelFeature::ImageGeneration => "image_generation",
        ModelFeature::AudioGeneration => "audio_generation",
        ModelFeature::VideoGeneration => "video_generation",
        ModelFeature::MusicGeneration => "music_generation",
        ModelFeature::SpeechTranscription => "speech_transcription",
        ModelFeature::SpeechSynthesis => "speech_synthesis",
        ModelFeature::Embedding => "embedding",
        ModelFeature::MultimodalUnderstanding => "multimodal_understanding",
        ModelFeature::ServerContinuation => "server_continuation",
        ModelFeature::PromptCaching => "prompt_caching",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::model_lane_resolution::ResolvedModelProfileSource;
    use crate::ports::model_profile_catalog::{
        CatalogModelProfile, CatalogModelProfileSource, ContextLimitProfileObservation,
        ModelProfileObservation,
    };

    #[derive(Default)]
    struct StubCatalog {
        profile: Option<CatalogModelProfile>,
    }

    impl ModelProfileCatalogPort for StubCatalog {
        fn lookup_model_profile(
            &self,
            _provider: &str,
            _model: &str,
        ) -> Option<CatalogModelProfile> {
            self.profile.clone()
        }

        fn record_model_profile_observation(
            &self,
            _provider: &str,
            _model: &str,
            _observation: ModelProfileObservation,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn record_context_limit_observation(
            &self,
            _provider: &str,
            _model: &str,
            _observation: ContextLimitProfileObservation,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn test_route(lane: Option<CapabilityLane>) -> RouteSelection {
        RouteSelection {
            provider: "openrouter".into(),
            model: "test-model".into(),
            lane,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
            runtime_decision_traces: Vec::new(),
        }
    }

    fn base_config() -> Config {
        let mut config = Config::default();
        config.default_provider = Some("openrouter".into());
        config.default_model = Some("test-model".into());
        config
    }

    fn base_input<'a>(
        config: &'a Config,
        route: &'a RouteSelection,
        catalog: Option<&'a dyn ModelProfileCatalogPort>,
    ) -> CapabilityDoctorInput<'a> {
        CapabilityDoctorInput {
            config,
            route,
            catalog,
            provider_adapter: CapabilityDoctorAdapterStatus::Available,
            provider_key: CapabilityDoctorProviderKeyStatus::Present,
            provider_capabilities: ProviderCapabilities::default(),
            provider_plan_denial: None,
            tool_registry_count: 0,
            memory_backend: CapabilityDoctorBackendStatus {
                configured: true,
                healthy: Some(true),
                name: Some("stub"),
            },
            embedding_profile: None,
            channel_delivery: CapabilityDoctorChannelStatus {
                surface: Some("matrix"),
                available: Some(true),
            },
            generated_at_unix: 100,
        }
    }

    fn readiness(
        report: &CapabilityDoctorReport,
        subsystem: CapabilityDoctorSubsystem,
    ) -> Vec<CapabilityDoctorReadiness> {
        report
            .nodes
            .iter()
            .filter(|node| node.subsystem == subsystem)
            .map(|node| node.readiness)
            .collect()
    }

    #[test]
    fn reports_missing_api_key_as_missing_key() {
        let config = base_config();
        let route = test_route(None);
        let mut input = base_input(&config, &route, None);
        input.provider_key = CapabilityDoctorProviderKeyStatus::Missing;

        let report = build_capability_doctor_report(input);

        assert!(readiness(&report, CapabilityDoctorSubsystem::ProviderKey)
            .contains(&CapabilityDoctorReadiness::MissingKey));
    }

    #[test]
    fn reports_unsupported_image_lane_as_unsupported_modality() {
        let config = base_config();
        let route = test_route(Some(CapabilityLane::ImageGeneration));
        let catalog = StubCatalog {
            profile: Some(CatalogModelProfile {
                context_window_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                features: vec![ModelFeature::ToolCalling],
                source: Some(CatalogModelProfileSource::LocalOverrideCatalog),
                observed_at_unix: None,
            }),
        };

        let report = build_capability_doctor_report(base_input(&config, &route, Some(&catalog)));

        assert!(readiness(&report, CapabilityDoctorSubsystem::Lane)
            .contains(&CapabilityDoctorReadiness::UnsupportedModality));
    }

    #[test]
    fn reports_stale_cached_metadata_with_refresh_recommendation() {
        let config = base_config();
        let route = test_route(None);
        let catalog = StubCatalog {
            profile: Some(CatalogModelProfile {
                context_window_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                features: vec![ModelFeature::ToolCalling],
                source: Some(CatalogModelProfileSource::CachedProviderCatalog),
                observed_at_unix: Some(1),
            }),
        };

        let report = build_capability_doctor_report(base_input(&config, &route, Some(&catalog)));
        let profile_node = report
            .nodes
            .iter()
            .find(|node| node.subsystem == CapabilityDoctorSubsystem::ModelProfile)
            .expect("model profile node should exist");

        assert_eq!(
            profile_node.readiness,
            CapabilityDoctorReadiness::StaleCatalog
        );
        assert!(profile_node
            .recommendation
            .as_deref()
            .is_some_and(|text| text.contains("doctor models")));
    }

    #[test]
    fn reports_tool_registry_without_native_tools_as_unsupported_tool_capability() {
        let config = base_config();
        let route = test_route(None);
        let catalog = StubCatalog {
            profile: Some(CatalogModelProfile {
                context_window_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                features: vec![ModelFeature::Vision],
                source: Some(CatalogModelProfileSource::LocalOverrideCatalog),
                observed_at_unix: None,
            }),
        };
        let mut input = base_input(&config, &route, Some(&catalog));
        input.tool_registry_count = 3;

        let report = build_capability_doctor_report(input);

        assert!(readiness(&report, CapabilityDoctorSubsystem::ToolRegistry)
            .contains(&CapabilityDoctorReadiness::UnsupportedToolCapability));
    }

    #[test]
    fn reports_ignored_reasoning_controls_for_unknown_policy() {
        let mut config = base_config();
        config.runtime.reasoning_effort = Some("high".into());
        let route = test_route(None);

        let report = build_capability_doctor_report(base_input(&config, &route, None));

        assert!(
            readiness(&report, CapabilityDoctorSubsystem::ReasoningControls)
                .contains(&CapabilityDoctorReadiness::IgnoredReasoningControls)
        );
    }

    #[test]
    fn disabled_reasoning_control_does_not_report_ignored_control() {
        let mut config = base_config();
        config.runtime.reasoning_enabled = Some(false);
        let route = test_route(None);

        let report = build_capability_doctor_report(base_input(&config, &route, None));

        assert!(readiness(&report, CapabilityDoctorSubsystem::ReasoningControls).is_empty());
    }

    #[test]
    fn reports_native_continuation_when_profile_supports_it() {
        let config = base_config();
        let route = test_route(None);
        let catalog = StubCatalog {
            profile: Some(CatalogModelProfile {
                context_window_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                features: vec![ModelFeature::ServerContinuation],
                source: Some(CatalogModelProfileSource::BundledCatalog),
                observed_at_unix: None,
            }),
        };

        let report = build_capability_doctor_report(base_input(&config, &route, Some(&catalog)));

        assert!(
            readiness(&report, CapabilityDoctorSubsystem::NativeContinuation)
                .contains(&CapabilityDoctorReadiness::Ready)
        );
    }

    #[test]
    fn local_override_empty_features_are_known_for_missing_lane_support() {
        let profile = ResolvedModelProfile {
            context_window_tokens: Some(8_000),
            max_output_tokens: Some(1_000),
            features: Vec::new(),
            context_window_source: ResolvedModelProfileSource::ManualConfig,
            max_output_source: ResolvedModelProfileSource::ManualConfig,
            features_source: ResolvedModelProfileSource::ManualConfig,
            observed_at_unix: None,
        };

        assert_eq!(
            lane_readiness(CapabilityLane::AudioGeneration, &profile),
            CapabilityDoctorReadiness::UnsupportedModality
        );
    }
}
