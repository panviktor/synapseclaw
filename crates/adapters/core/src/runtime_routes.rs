use std::fmt::Write;
use std::path::{Path, PathBuf};

use synapse_domain::application::services::model_lane_resolution::{
    model_lane_resolution_source_name, resolve_lane_candidates, resolve_route_selection_profile,
    resolved_model_profile_confidence_name, resolved_model_profile_freshness_name,
    resolved_model_profile_source_name,
};
use synapse_domain::application::services::model_preset_resolution::preset_title;
use synapse_domain::config::schema::{CapabilityLane, Config, ModelFeature};
use synapse_domain::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, ToolRepairAction, ToolRepairTrace,
};
use synapse_domain::domain::turn_admission::{
    admission_repair_hint_name, context_pressure_state_name, turn_admission_action_name,
    turn_intent_name, AdmissionRepairHint, CandidateAdmissionReason,
};
use synapse_domain::ports::model_profile_catalog::{
    CatalogModelProfile, CatalogModelProfileSource, ModelProfileCatalogPort,
};
use synapse_domain::ports::route_selection::{RouteAdmissionState, RouteSelection};

const MODEL_CACHE_FILE: &str = "models_cache.json";
const MODEL_CACHE_PREVIEW_LIMIT: usize = 10;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ModelCacheState {
    entries: Vec<ModelCacheEntry>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ModelCacheEntry {
    provider: String,
    #[serde(default)]
    fetched_at_unix: u64,
    models: Vec<String>,
    #[serde(default)]
    profiles: Vec<ModelProfileCacheEntry>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ModelProfileCacheEntry {
    model: String,
    #[serde(default)]
    context_window_tokens: Option<usize>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    features: Vec<ModelFeature>,
}

pub(crate) struct WorkspaceModelProfileCatalog {
    workspace_dir: PathBuf,
}

impl WorkspaceModelProfileCatalog {
    pub(crate) fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
        }
    }
}

impl ModelProfileCatalogPort for WorkspaceModelProfileCatalog {
    fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile> {
        load_cached_model_profile(self.workspace_dir.as_path(), provider, model)
    }
}

pub(crate) fn build_models_help_response(current: &RouteSelection, config: &Config) -> String {
    let workspace_dir = config.workspace_dir.as_path();
    let model_routes = &config.model_routes;
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    if let Some(admission) = current.last_admission.as_ref() {
        let _ = writeln!(
            response,
            "Last admission: `{}` / `{}` / `{}`",
            turn_intent_name(admission.snapshot.intent),
            context_pressure_state_name(admission.snapshot.pressure_state),
            turn_admission_action_name(admission.snapshot.action),
        );
        if !admission.reasons.is_empty() {
            let _ = writeln!(
                response,
                "Last admission reasons: {}",
                admission
                    .reasons
                    .iter()
                    .map(format_candidate_admission_reason)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if let Some(repair) = admission.recommended_action {
            let _ = writeln!(
                response,
                "Suggested next action: {}",
                format_admission_repair_hint(repair)
            );
        }
    }
    if !current.recent_admissions.is_empty() {
        let _ = writeln!(
            response,
            "Recent admissions retained: {}",
            current.recent_admissions.len()
        );
        write_recent_admissions(&mut response, &current.recent_admissions);
    }
    if let Some(repair) = current.last_tool_repair.as_ref() {
        let _ = writeln!(
            response,
            "Last tool repair: {} / {}",
            tool_failure_kind_name(repair.failure_kind),
            format_tool_repair_action(repair)
        );
        if let Some(detail) = repair.detail.as_deref() {
            let _ = writeln!(response, "Last tool repair detail: {}", detail);
        }
    }
    if !current.recent_tool_repairs.is_empty() {
        let _ = writeln!(
            response,
            "Recent tool repairs retained: {}",
            current.recent_tool_repairs.len()
        );
        write_recent_tool_repairs(&mut response, &current.recent_tool_repairs);
    }
    if let Some(lane) = current.lane {
        let _ = writeln!(
            response,
            "Current lane: `{}`{}",
            capability_lane_name(lane),
            current
                .candidate_index
                .map(|index| format!(" (candidate #{index})"))
                .unwrap_or_default()
        );
    }
    response.push_str("\nSwitch model with `/model <model-id>` or `/model <hint>`.\n");

    let current_catalog = WorkspaceModelProfileCatalog::new(workspace_dir.to_path_buf());
    let current_profile = resolve_route_selection_profile(config, current, Some(&current_catalog));
    let _ = writeln!(
        response,
        "Profile sources: ctx=`{}`, output=`{}`, features=`{}`",
        resolved_model_profile_source_name(current_profile.context_window_source),
        resolved_model_profile_source_name(current_profile.max_output_source),
        resolved_model_profile_source_name(current_profile.features_source),
    );
    let _ = writeln!(
        response,
        "Profile quality: ctx=`{}/{}`, output=`{}/{}`, features=`{}/{}`",
        resolved_model_profile_freshness_name(current_profile.context_window_freshness()),
        resolved_model_profile_confidence_name(current_profile.context_window_confidence()),
        resolved_model_profile_freshness_name(current_profile.max_output_freshness()),
        resolved_model_profile_confidence_name(current_profile.max_output_confidence()),
        resolved_model_profile_freshness_name(current_profile.features_freshness()),
        resolved_model_profile_confidence_name(current_profile.features_confidence()),
    );
    let _ = writeln!(
        response,
        "Current route limits: ctx=`{}` output=`{}`",
        current_profile
            .context_window_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "?".into()),
        current_profile
            .max_output_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "?".into())
    );
    if let Some(observed_at) = current_profile.observed_at_unix {
        let _ = writeln!(response, "Profile observed_at_unix: `{observed_at}`");
    }
    let _ = writeln!(
        response,
        "Current route feature coverage: {}",
        format_profile_feature_coverage(&current_profile)
    );

    if let Some(preset) = config.model_preset.as_deref() {
        let _ = writeln!(
            response,
            "\nActive preset: `{}` ({})",
            preset,
            preset_title(preset).unwrap_or("custom preset")
        );
    }

    let resolved_lanes = [
        CapabilityLane::Reasoning,
        CapabilityLane::CheapReasoning,
        CapabilityLane::Embedding,
        CapabilityLane::MultimodalUnderstanding,
        CapabilityLane::ImageGeneration,
        CapabilityLane::AudioGeneration,
        CapabilityLane::VideoGeneration,
        CapabilityLane::MusicGeneration,
    ]
    .into_iter()
    .filter_map(|lane| {
        let candidates = resolve_lane_candidates(config, lane, Some(&current_catalog));
        (!candidates.is_empty()).then_some((lane, candidates))
    })
    .collect::<Vec<_>>();
    if !resolved_lanes.is_empty() {
        response.push_str("\nEffective capability lanes:\n");
        for (lane, candidates) in resolved_lanes {
            let lane_name = capability_lane_name(lane);
            let candidate_preview = candidates
                .iter()
                .take(3)
                .map(|candidate| {
                    let profile = &candidate.profile;
                    let mut suffix = String::new();
                    let _ = write!(
                        suffix,
                        ", {}",
                        model_lane_resolution_source_name(candidate.source)
                    );
                    if let Some(tokens) = profile.context_window_tokens {
                        let _ = write!(
                            suffix,
                            ", {}k ctx/{}/{}",
                            tokens / 1000,
                            resolved_model_profile_source_name(profile.context_window_source),
                            resolved_model_profile_confidence_name(
                                profile.context_window_confidence()
                            )
                        );
                    } else {
                        let _ = write!(suffix, ", ctx?");
                    }
                    if !profile.features.is_empty() {
                        let _ = write!(
                            suffix,
                            ", {}/{}/{}",
                            profile
                                .features
                                .iter()
                                .map(model_feature_name)
                                .collect::<Vec<_>>()
                                .join("+"),
                            resolved_model_profile_source_name(profile.features_source),
                            resolved_model_profile_confidence_name(profile.features_confidence())
                        );
                    } else {
                        let _ = write!(suffix, ", features?");
                    }
                    format!("{} ({}){}", candidate.model, candidate.provider, suffix)
                })
                .collect::<Vec<_>>()
                .join(" | ");
            let _ = writeln!(response, "  `{lane_name}` → {candidate_preview}");
        }
    }

    if !model_routes.is_empty() {
        response.push_str("\nConfigured model routes:\n");
        for route in model_routes {
            let _ = writeln!(
                response,
                "  `{}` → {} ({})",
                route.hint, route.model, route.provider
            );
        }
    }

    let cached_models = load_cached_model_preview(workspace_dir, &current.provider);
    if cached_models.is_empty() {
        let _ = writeln!(
            response,
            "\nNo cached model list found for `{}`. Ask the operator to run `synapseclaw models refresh --provider {}`.",
            current.provider, current.provider
        );
    } else {
        let _ = writeln!(
            response,
            "\nCached model IDs (top {}):",
            cached_models.len()
        );
        for model in cached_models {
            let _ = writeln!(response, "- `{model}`");
        }
    }

    response
}

pub(crate) fn build_providers_help_response(current: &RouteSelection) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    if let Some(admission) = current.last_admission.as_ref() {
        let _ = writeln!(
            response,
            "Last admission: `{}` / `{}` / `{}`",
            turn_intent_name(admission.snapshot.intent),
            context_pressure_state_name(admission.snapshot.pressure_state),
            turn_admission_action_name(admission.snapshot.action),
        );
        if !admission.reasons.is_empty() {
            let _ = writeln!(
                response,
                "Last admission reasons: {}",
                admission
                    .reasons
                    .iter()
                    .map(format_candidate_admission_reason)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if let Some(repair) = admission.recommended_action {
            let _ = writeln!(
                response,
                "Suggested next action: {}",
                format_admission_repair_hint(repair)
            );
        }
    }
    if !current.recent_admissions.is_empty() {
        let _ = writeln!(
            response,
            "Recent admissions retained: {}",
            current.recent_admissions.len()
        );
        write_recent_admissions(&mut response, &current.recent_admissions);
    }
    if let Some(repair) = current.last_tool_repair.as_ref() {
        let _ = writeln!(
            response,
            "Last tool repair: {} / {}",
            tool_failure_kind_name(repair.failure_kind),
            format_tool_repair_action(repair)
        );
        if let Some(detail) = repair.detail.as_deref() {
            let _ = writeln!(response, "Last tool repair detail: {}", detail);
        }
    }
    if !current.recent_tool_repairs.is_empty() {
        let _ = writeln!(
            response,
            "Recent tool repairs retained: {}",
            current.recent_tool_repairs.len()
        );
        write_recent_tool_repairs(&mut response, &current.recent_tool_repairs);
    }
    response.push_str("\nSwitch provider with `/models <provider>`.\n");
    response.push_str("Switch model with `/model <model-id>`.\n\n");
    response.push_str("Available providers:\n");
    for provider in synapse_providers::list_providers() {
        if provider.aliases.is_empty() {
            let _ = writeln!(response, "- {}", provider.name);
        } else {
            let _ = writeln!(
                response,
                "- {} (aliases: {})",
                provider.name,
                provider.aliases.join(", ")
            );
        }
    }
    response
}

fn load_cached_model_preview(workspace_dir: &Path, provider_name: &str) -> Vec<String> {
    let cache_path = workspace_dir.join("state").join(MODEL_CACHE_FILE);
    let Ok(raw) = std::fs::read_to_string(cache_path) else {
        return Vec::new();
    };
    let Ok(state) = serde_json::from_str::<ModelCacheState>(&raw) else {
        return Vec::new();
    };

    state
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider_name)
        .map(|entry| {
            entry
                .models
                .into_iter()
                .take(MODEL_CACHE_PREVIEW_LIMIT)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn format_candidate_admission_reason(reason: &CandidateAdmissionReason) -> String {
    match reason {
        CandidateAdmissionReason::RequiresLane(lane) => {
            format!("requires {}", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::MissingFeature(feature) => {
            format!("missing {}", model_feature_name(feature))
        }
        CandidateAdmissionReason::CapabilityMetadataUnknown(lane) => {
            format!("{} metadata unknown", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::CapabilityMetadataStale(lane) => {
            format!("{} metadata stale", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::CapabilityMetadataLowConfidence(lane) => {
            format!("{} metadata low confidence", capability_lane_name(*lane))
        }
        CandidateAdmissionReason::SpecializedLaneMismatch(lane) => {
            format!(
                "specialized lane mismatch ({})",
                capability_lane_name(*lane)
            )
        }
        CandidateAdmissionReason::CandidateWindowMetadataUnknown => {
            "window metadata unknown".to_string()
        }
        CandidateAdmissionReason::CandidateWindowNearLimit => "window near limit".to_string(),
        CandidateAdmissionReason::CandidateWindowExceeded => "window exceeded".to_string(),
        CandidateAdmissionReason::ProviderContextWarning => "context warning".to_string(),
        CandidateAdmissionReason::ProviderContextCritical => "context critical".to_string(),
        CandidateAdmissionReason::ProviderContextOverflowRisk => {
            "context overflow risk".to_string()
        }
    }
}

fn format_admission_repair_hint(hint: AdmissionRepairHint) -> String {
    match hint {
        AdmissionRepairHint::SwitchToLane(lane) => {
            format!(
                "{} ({})",
                admission_repair_hint_name(hint),
                capability_lane_name(lane)
            )
        }
        AdmissionRepairHint::RefreshCapabilityMetadata(lane) => {
            format!(
                "{} ({})",
                admission_repair_hint_name(hint),
                capability_lane_name(lane)
            )
        }
        AdmissionRepairHint::SwitchToToolCapableReasoning
        | AdmissionRepairHint::CompactSession
        | AdmissionRepairHint::StartFreshHandoff => admission_repair_hint_name(hint).to_string(),
    }
}

fn format_tool_repair_action(trace: &ToolRepairTrace) -> String {
    match trace.suggested_action {
        ToolRepairAction::SwitchRouteLane(lane) => format!(
            "{} ({})",
            tool_repair_action_name(trace.suggested_action),
            capability_lane_name(lane)
        ),
        _ => tool_repair_action_name(trace.suggested_action).to_string(),
    }
}

fn write_recent_tool_repairs(response: &mut String, recent_tool_repairs: &[ToolRepairTrace]) {
    for repair in recent_tool_repairs.iter().rev().take(3) {
        let _ = writeln!(
            response,
            "Recent tool repair: {} / {} / {}",
            repair.tool_name,
            tool_failure_kind_name(repair.failure_kind),
            format_tool_repair_action(repair)
        );
        if let Some(detail) = repair.detail.as_deref() {
            let _ = writeln!(response, "Recent tool repair detail: {}", detail);
        }
    }
}

fn write_recent_admissions(response: &mut String, recent_admissions: &[RouteAdmissionState]) {
    for admission in recent_admissions.iter().rev().take(3) {
        let _ = writeln!(
            response,
            "Recent admission: {} / {} / {}",
            turn_intent_name(admission.snapshot.intent),
            context_pressure_state_name(admission.snapshot.pressure_state),
            turn_admission_action_name(admission.snapshot.action),
        );
        if !admission.reasons.is_empty() {
            let _ = writeln!(
                response,
                "Recent admission reasons: {}",
                admission
                    .reasons
                    .iter()
                    .map(format_candidate_admission_reason)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if let Some(repair) = admission.recommended_action {
            let _ = writeln!(
                response,
                "Recent admission suggested action: {}",
                format_admission_repair_hint(repair)
            );
        }
    }
}

fn capability_lane_name(lane: CapabilityLane) -> &'static str {
    match lane {
        CapabilityLane::Reasoning => "reasoning",
        CapabilityLane::CheapReasoning => "cheap_reasoning",
        CapabilityLane::Embedding => "embedding",
        CapabilityLane::ImageGeneration => "image_generation",
        CapabilityLane::AudioGeneration => "audio_generation",
        CapabilityLane::VideoGeneration => "video_generation",
        CapabilityLane::MusicGeneration => "music_generation",
        CapabilityLane::MultimodalUnderstanding => "multimodal_understanding",
    }
}

fn model_feature_name(feature: &ModelFeature) -> &'static str {
    match feature {
        ModelFeature::ToolCalling => "tools",
        ModelFeature::Vision => "vision",
        ModelFeature::ImageGeneration => "image",
        ModelFeature::AudioGeneration => "audio",
        ModelFeature::VideoGeneration => "video",
        ModelFeature::MusicGeneration => "music",
        ModelFeature::Embedding => "embedding",
        ModelFeature::MultimodalUnderstanding => "multimodal",
        ModelFeature::ServerContinuation => "continuation",
        ModelFeature::PromptCaching => "prompt_cache",
    }
}

fn format_profile_feature_coverage(
    profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
) -> String {
    if profile.features.is_empty() {
        return "unknown".to_string();
    }

    let mut covered = vec!["reasoning".to_string()];
    if profile.features.contains(&ModelFeature::Vision)
        || profile
            .features
            .contains(&ModelFeature::MultimodalUnderstanding)
    {
        covered.push("multimodal_understanding".to_string());
    }
    if profile.features.contains(&ModelFeature::ImageGeneration) {
        covered.push("image_generation".to_string());
    }
    if profile.features.contains(&ModelFeature::AudioGeneration) {
        covered.push("audio_generation".to_string());
    }
    if profile.features.contains(&ModelFeature::VideoGeneration) {
        covered.push("video_generation".to_string());
    }
    if profile.features.contains(&ModelFeature::MusicGeneration) {
        covered.push("music_generation".to_string());
    }
    if profile.features.contains(&ModelFeature::Embedding) {
        covered.push("embedding".to_string());
    }

    format!(
        "{} ({}/{})",
        covered.join(", "),
        resolved_model_profile_source_name(profile.features_source),
        resolved_model_profile_confidence_name(profile.features_confidence())
    )
}

fn load_cached_model_profile(
    workspace_dir: &Path,
    provider_name: &str,
    model_name: &str,
) -> Option<CatalogModelProfile> {
    let cache_path = workspace_dir.join("state").join(MODEL_CACHE_FILE);
    let raw = std::fs::read_to_string(cache_path).ok()?;
    let state = serde_json::from_str::<ModelCacheState>(&raw).ok()?;
    let entry = state
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider_name)?;
    let profile = entry
        .profiles
        .into_iter()
        .find(|profile| profile.model == model_name)?;
    Some(CatalogModelProfile {
        context_window_tokens: profile.context_window_tokens,
        max_output_tokens: profile.max_output_tokens,
        features: profile.features,
        source: Some(CatalogModelProfileSource::CachedProviderCatalog),
        observed_at_unix: Some(entry.fetched_at_unix),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::config::schema::{Config, ModelRouteConfig};
    use synapse_domain::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairTrace};
    use synapse_domain::domain::turn_admission::{
        CandidateAdmissionReason, ContextPressureState, TurnAdmissionAction, TurnAdmissionSnapshot,
        TurnIntentCategory,
    };
    use synapse_domain::ports::route_selection::RouteAdmissionState;

    #[test]
    fn providers_help_includes_current_route() {
        let response = build_providers_help_response(&RouteSelection {
            provider: "openai-codex".into(),
            model: "gpt-5.4".into(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
        });
        assert!(response.contains("Current provider: `openai-codex`"));
        assert!(response.contains("Switch provider with `/models <provider>`"));
    }

    #[test]
    fn models_help_includes_configured_routes() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();
        config.model_routes = vec![ModelRouteConfig {
            hint: "cheap".into(),
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            api_key: None,
            capability: None,
            profile: Default::default(),
        }];
        let response = build_models_help_response(
            &RouteSelection {
                provider: "openrouter".into(),
                model: "qwen/qwen3.6-plus".into(),
                lane: None,
                candidate_index: None,
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
            },
            &config,
        );
        assert!(response.contains("`cheap` → qwen/qwen3.6-plus (openrouter)"));
        assert!(response.contains("Profile sources:"));
        assert!(response.contains("Current route limits:"));
        assert!(response.contains("No cached model list found"));
    }

    #[test]
    fn models_help_includes_last_admission_reasons() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();

        let response = build_models_help_response(
            &RouteSelection {
                provider: "openrouter".into(),
                model: "qwen/qwen3.6-plus".into(),
                lane: None,
                candidate_index: None,
                last_admission: Some(RouteAdmissionState {
                    observed_at_unix: 1_744_243_100,
                    snapshot: TurnAdmissionSnapshot {
                        intent: TurnIntentCategory::MultimodalUnderstanding,
                        pressure_state: ContextPressureState::Warning,
                        action: TurnAdmissionAction::Reroute,
                    },
                    reasons: vec![
                        CandidateAdmissionReason::RequiresLane(
                            CapabilityLane::MultimodalUnderstanding,
                        ),
                        CandidateAdmissionReason::ProviderContextWarning,
                    ],
                    recommended_action: Some(AdmissionRepairHint::SwitchToLane(
                        CapabilityLane::MultimodalUnderstanding,
                    )),
                }),
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
            },
            &config,
        );

        assert!(
            response.contains("Last admission: `multimodal_understanding` / `warning` / `reroute`")
        );
        assert!(response.contains(
            "Last admission reasons: requires multimodal_understanding, context warning"
        ));
        assert!(response.contains("Suggested next action: switch_lane (multimodal_understanding)"));
    }

    #[test]
    fn models_help_includes_recent_admission_history() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();

        let recent = RouteAdmissionState {
            observed_at_unix: 1_744_243_090,
            snapshot: TurnAdmissionSnapshot {
                intent: TurnIntentCategory::ImageGeneration,
                pressure_state: ContextPressureState::Critical,
                action: TurnAdmissionAction::Compact,
            },
            reasons: vec![
                CandidateAdmissionReason::RequiresLane(CapabilityLane::ImageGeneration),
                CandidateAdmissionReason::CandidateWindowNearLimit,
            ],
            recommended_action: Some(AdmissionRepairHint::CompactSession),
        };

        let response = build_models_help_response(
            &RouteSelection {
                provider: "openrouter".into(),
                model: "qwen/qwen3.6-plus".into(),
                lane: None,
                candidate_index: None,
                last_admission: None,
                recent_admissions: vec![recent],
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
            },
            &config,
        );

        assert!(response.contains("Recent admissions retained: 1"));
        assert!(response.contains("Recent admission: image_generation / critical / compact"));
        assert!(response
            .contains("Recent admission reasons: requires image_generation, window near limit"));
        assert!(response.contains("Recent admission suggested action: compact_session"));
    }

    #[test]
    fn models_help_includes_last_tool_repair_trace() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();

        let response = build_models_help_response(
            &RouteSelection {
                provider: "openrouter".into(),
                model: "qwen/qwen3.6-plus".into(),
                lane: None,
                candidate_index: None,
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: Some(ToolRepairTrace {
                    observed_at_unix: 1_744_243_200,
                    tool_name: "message_send".into(),
                    failure_kind: ToolFailureKind::ReportedFailure,
                    suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                    detail: Some("missing delivery target".into()),
                }),
                recent_tool_repairs: vec![ToolRepairTrace {
                    observed_at_unix: 1_744_243_200,
                    tool_name: "message_send".into(),
                    failure_kind: ToolFailureKind::ReportedFailure,
                    suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                    detail: Some("missing delivery target".into()),
                }],
            },
            &config,
        );

        assert!(
            response.contains("Last tool repair: reported_failure / adjust_arguments_or_target")
        );
        assert!(response.contains("Last tool repair detail: missing delivery target"));
        assert!(response.contains("Recent tool repairs retained: 1"));
        assert!(response.contains(
            "Recent tool repair: message_send / reported_failure / adjust_arguments_or_target"
        ));
        assert!(response.contains("Recent tool repair detail: missing delivery target"));
    }

    #[test]
    fn models_help_includes_current_route_feature_coverage() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();
        config.model_lanes = vec![synapse_domain::config::schema::ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![synapse_domain::config::schema::ModelLaneCandidateConfig {
                provider: "openai-codex".into(),
                model: "gpt-5.4".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: synapse_domain::config::schema::ModelCandidateProfileConfig {
                    context_window_tokens: Some(200_000),
                    max_output_tokens: None,
                    features: vec![
                        ModelFeature::ToolCalling,
                        ModelFeature::Vision,
                        ModelFeature::ImageGeneration,
                        ModelFeature::AudioGeneration,
                    ],
                },
            }],
        }];

        let response = build_models_help_response(
            &RouteSelection {
                provider: "openai-codex".into(),
                model: "gpt-5.4".into(),
                lane: Some(CapabilityLane::Reasoning),
                candidate_index: Some(0),
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
            },
            &config,
        );

        assert!(response.contains("Current route feature coverage: reasoning, multimodal_understanding, image_generation, audio_generation"));
    }
}
