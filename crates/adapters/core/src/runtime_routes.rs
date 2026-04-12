use std::fmt::Write;
use std::path::{Path, PathBuf};

use synapse_domain::application::services::model_capability_support::profile_supports_lane_confidently;
use synapse_domain::application::services::model_lane_resolution::{
    model_lane_resolution_source_name, resolve_lane_candidates, resolve_route_selection_profile,
    resolved_model_profile_confidence_name, resolved_model_profile_freshness_name,
    resolved_model_profile_source_name,
};
use synapse_domain::application::services::model_preset_resolution::preset_title;
use synapse_domain::application::services::provider_native_context_policy::{
    resolve_provider_native_context_policy, ProviderNativeContextPolicyInput,
};
use synapse_domain::application::services::runtime_assumptions::{
    format_runtime_assumption, RuntimeAssumption,
};
use synapse_domain::application::services::runtime_calibration::{
    runtime_calibration_action_name, runtime_calibration_comparison_name,
    runtime_calibration_decision_kind_name, RuntimeCalibrationRecord,
};
use synapse_domain::application::services::runtime_trace_janitor::{
    append_runtime_watchdog_alerts, RuntimeHandoffArtifact,
};
use synapse_domain::application::services::runtime_watchdog::{
    build_runtime_watchdog_digest, runtime_watchdog_action_name, runtime_watchdog_reason_name,
    runtime_watchdog_severity_name, runtime_watchdog_subsystem_name, RuntimeWatchdogAlert,
    RuntimeWatchdogDigest, RuntimeWatchdogInput,
};
use synapse_domain::application::services::session_handoff::session_handoff_reason_name;
use synapse_domain::config::schema::{CapabilityLane, Config, ModelFeature};
use synapse_domain::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, ToolRepairAction, ToolRepairTrace,
};
use synapse_domain::domain::turn_admission::{
    admission_repair_hint_name, context_pressure_state_name, turn_admission_action_name,
    turn_intent_name, AdmissionRepairHint, CandidateAdmissionReason,
};
use synapse_domain::ports::model_profile_catalog::{
    CatalogModelProfile, CatalogModelProfileSource, ContextLimitProfileObservation,
    ModelProfileCatalogPort,
};
use synapse_domain::ports::route_selection::{RouteAdmissionState, RouteSelection};

const MODEL_CACHE_FILE: &str = "models_cache.json";
const MODEL_CACHE_PREVIEW_LIMIT: usize = 10;

pub(crate) fn resolve_provider_alias(name: &str) -> Option<String> {
    let candidate = name.trim();
    if candidate.is_empty() {
        return None;
    }

    synapse_providers::list_providers()
        .into_iter()
        .find(|provider| {
            provider.name.eq_ignore_ascii_case(candidate)
                || provider
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(candidate))
        })
        .map(|provider| provider.name.to_string())
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct ModelCacheState {
    entries: Vec<ModelCacheEntry>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct ModelCacheEntry {
    provider: String,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    fetched_at_unix: u64,
    models: Vec<String>,
    #[serde(default)]
    profiles: Vec<ModelProfileCacheEntry>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct ModelProfileCacheEntry {
    model: String,
    #[serde(default)]
    context_window_tokens: Option<usize>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    features: Vec<ModelFeature>,
    #[serde(default)]
    observed_at_unix: Option<u64>,
}

pub(crate) struct WorkspaceModelProfileCatalog {
    workspace_dir: PathBuf,
    provider_endpoint: Option<ProviderEndpointProfileLookup>,
}

struct ProviderEndpointProfileLookup {
    configured_provider: String,
    inferred_provider: Option<String>,
    endpoint: String,
}

impl WorkspaceModelProfileCatalog {
    pub(crate) fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            provider_endpoint: None,
        }
    }

    pub(crate) fn from_config(config: &Config) -> Self {
        Self::with_provider_endpoint(
            config.workspace_dir.clone(),
            config.default_provider.as_deref(),
            config.api_url.as_deref(),
        )
    }

    pub(crate) fn with_provider_endpoint(
        workspace_dir: impl Into<PathBuf>,
        provider: Option<&str>,
        endpoint: Option<&str>,
    ) -> Self {
        let provider_endpoint = provider.zip(endpoint).and_then(|(provider, endpoint)| {
            let endpoint = normalize_cache_endpoint(endpoint);
            if endpoint.is_empty() {
                return None;
            }
            let inferred_provider =
                synapse_domain::config::model_catalog::provider_for_api_base_url(&endpoint)
                    .filter(|inferred| !inferred.eq_ignore_ascii_case(provider))
                    .map(str::to_string);
            Some(ProviderEndpointProfileLookup {
                configured_provider: provider.to_string(),
                inferred_provider,
                endpoint,
            })
        });
        Self {
            workspace_dir: workspace_dir.into(),
            provider_endpoint,
        }
    }

    fn endpoint_for_provider(&self, provider: &str) -> Option<&str> {
        self.provider_endpoint.as_ref().and_then(|lookup| {
            (lookup.configured_provider.eq_ignore_ascii_case(provider)
                || lookup
                    .inferred_provider
                    .as_deref()
                    .is_some_and(|inferred| inferred.eq_ignore_ascii_case(provider)))
            .then_some(lookup.endpoint.as_str())
        })
    }

    fn provider_lookup_candidates<'a>(&'a self, provider: &'a str) -> Vec<&'a str> {
        let mut candidates = vec![provider];
        if let Some(lookup) = self.provider_endpoint.as_ref() {
            if lookup.configured_provider.eq_ignore_ascii_case(provider) {
                if let Some(inferred_provider) = lookup.inferred_provider.as_deref() {
                    if !inferred_provider.eq_ignore_ascii_case(provider) {
                        candidates.push(inferred_provider);
                    }
                }
            }
        }
        candidates
    }

    pub(crate) fn record_context_limit_observation(
        &self,
        provider: &str,
        model: &str,
        observation: ContextLimitProfileObservation,
    ) -> anyhow::Result<()> {
        let Some(observed_window) = observation
            .observed_context_window_tokens
            .filter(|tokens| *tokens > 0)
        else {
            return Ok(());
        };
        if observation
            .requested_context_tokens
            .is_some_and(|requested| requested <= observed_window)
        {
            return Ok(());
        }

        let endpoint = self.endpoint_for_provider(provider).map(str::to_string);
        let candidates = self.provider_lookup_candidates(provider);
        let cache_path = model_cache_path(self.workspace_dir.as_path());
        let mut state = load_model_cache_state_for_update(cache_path.as_path())?;
        let target_provider =
            select_observation_cache_provider(&state, candidates.as_slice(), endpoint.as_deref());
        let now = current_unix_secs();

        let entry_index = if let Some(index) = state.entries.iter().position(|entry| {
            model_cache_entry_matches(entry, target_provider.as_str(), endpoint.as_deref())
        }) {
            index
        } else {
            state.entries.push(ModelCacheEntry {
                provider: target_provider.clone(),
                endpoint: endpoint.clone(),
                fetched_at_unix: now,
                models: Vec::new(),
                profiles: Vec::new(),
            });
            state.entries.len() - 1
        };

        let entry = &mut state.entries[entry_index];
        let mut changed = false;
        if !entry
            .models
            .iter()
            .any(|cached_model| cached_model.eq_ignore_ascii_case(model))
        {
            entry.models.push(model.to_string());
            changed = true;
        }

        if let Some(profile) = entry
            .profiles
            .iter_mut()
            .find(|profile| profile.model.eq_ignore_ascii_case(model))
        {
            if profile
                .context_window_tokens
                .is_none_or(|existing| observed_window <= existing)
            {
                profile.context_window_tokens = Some(observed_window);
                profile.observed_at_unix = Some(now);
                changed = true;
            }
        } else {
            entry.profiles.push(ModelProfileCacheEntry {
                model: model.to_string(),
                context_window_tokens: Some(observed_window),
                max_output_tokens: None,
                features: Vec::new(),
                observed_at_unix: Some(now),
            });
            changed = true;
        }

        if changed {
            save_model_cache_state(cache_path.as_path(), &state)?;
        }
        Ok(())
    }
}

impl ModelProfileCatalogPort for WorkspaceModelProfileCatalog {
    fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile> {
        let endpoint = self.endpoint_for_provider(provider);
        let candidates = self.provider_lookup_candidates(provider);
        candidates
            .iter()
            .find_map(|candidate_provider| {
                load_cached_model_profile(
                    self.workspace_dir.as_path(),
                    candidate_provider,
                    model,
                    endpoint,
                )
            })
            .or_else(|| {
                candidates.iter().find_map(|candidate_provider| {
                    synapse_domain::config::model_catalog::model_profile(candidate_provider, model)
                })
            })
            .or_else(|| synapse_domain::config::model_catalog::model_profile(provider, model))
    }

    fn record_context_limit_observation(
        &self,
        provider: &str,
        model: &str,
        observation: ContextLimitProfileObservation,
    ) -> anyhow::Result<()> {
        WorkspaceModelProfileCatalog::record_context_limit_observation(
            self,
            provider,
            model,
            observation,
        )
    }
}

pub(crate) fn build_models_help_response(current: &RouteSelection, config: &Config) -> String {
    let workspace_dir = config.workspace_dir.as_path();
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    write_route_runtime_diagnostics(&mut response, current);
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

    let current_catalog = WorkspaceModelProfileCatalog::from_config(config);
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
    let native_context_policy =
        resolve_provider_native_context_policy(ProviderNativeContextPolicyInput {
            profile: &current_profile,
            provider_prompt_caching: false,
            operator_prompt_caching_enabled: config.agent.prompt_caching,
        });
    let _ = writeln!(
        response,
        "Native context policy: prompt_cache_supported=`{}` prompt_cache_enabled=`{}` server_continuation_supported=`{}`",
        native_context_policy.prompt_caching_supported,
        native_context_policy.prompt_caching_enabled,
        native_context_policy.server_continuation_supported,
    );
    if let Some(cache) = current.context_cache {
        let _ = writeln!(
            response,
            "History compaction cache: entries=`{}` hits=`{}` max=`{}` ttl_secs=`{}` loaded=`{}` threshold=`{}` target=`{}` protect=`{}/{}` summary=`{}` source_chars=`{}` summary_chars=`{}`",
            cache.entries, cache.hits, cache.max_entries, cache.ttl_secs, cache.loaded,
            format_basis_points(cache.threshold_basis_points),
            format_basis_points(cache.target_basis_points),
            cache.protect_first_n,
            cache.protect_last_n,
            format_basis_points(cache.summary_basis_points),
            cache.max_source_chars,
            cache.max_summary_chars,
        );
    }

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

    let catalog_aliases = synapse_domain::config::model_catalog::model_route_aliases();
    if !catalog_aliases.is_empty() {
        response.push_str("\nCatalog model aliases:\n");
        for route in catalog_aliases.iter().take(12) {
            let _ = writeln!(
                response,
                "  `{}` → {} ({})",
                route.hint, route.model, route.provider
            );
        }
    }

    let current_endpoint = current_catalog.endpoint_for_provider(&current.provider);
    let cached_models = current_catalog
        .provider_lookup_candidates(&current.provider)
        .into_iter()
        .find_map(|candidate_provider| {
            let preview =
                load_cached_model_preview(workspace_dir, candidate_provider, current_endpoint);
            (!preview.is_empty()).then_some(preview)
        })
        .unwrap_or_default();
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
    write_route_runtime_diagnostics(&mut response, current);
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

fn load_cached_model_preview(
    workspace_dir: &Path,
    provider_name: &str,
    endpoint: Option<&str>,
) -> Vec<String> {
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
        .find(|entry| model_cache_entry_matches(entry, provider_name, endpoint))
        .map(|entry| {
            entry
                .models
                .into_iter()
                .take(MODEL_CACHE_PREVIEW_LIMIT)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn normalize_cache_endpoint(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_string()
}

fn model_cache_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("state").join(MODEL_CACHE_FILE)
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn load_model_cache_state_for_update(cache_path: &Path) -> anyhow::Result<ModelCacheState> {
    if !cache_path.exists() {
        return Ok(ModelCacheState::default());
    }
    let raw = std::fs::read_to_string(cache_path)
        .map_err(|error| anyhow::anyhow!("failed to read model cache: {error}"))?;
    serde_json::from_str::<ModelCacheState>(&raw)
        .map_err(|error| anyhow::anyhow!("failed to parse model cache: {error}"))
}

fn save_model_cache_state(cache_path: &Path, state: &ModelCacheState) -> anyhow::Result<()> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| anyhow::anyhow!("failed to create model cache directory: {error}"))?;
    }
    let json = serde_json::to_vec_pretty(state)
        .map_err(|error| anyhow::anyhow!("failed to serialize model cache: {error}"))?;
    std::fs::write(cache_path, json)
        .map_err(|error| anyhow::anyhow!("failed to write model cache: {error}"))
}

fn select_observation_cache_provider(
    state: &ModelCacheState,
    candidates: &[&str],
    endpoint: Option<&str>,
) -> String {
    candidates
        .iter()
        .find(|candidate| {
            state
                .entries
                .iter()
                .any(|entry| model_cache_entry_matches(entry, candidate, endpoint))
        })
        .or_else(|| candidates.last())
        .copied()
        .unwrap_or("unknown")
        .to_string()
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
        CandidateAdmissionReason::CalibrationSuppressedRoute => {
            "calibration suppressed route".to_string()
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

fn write_route_runtime_diagnostics(response: &mut String, current: &RouteSelection) {
    write_route_admission_diagnostics(response, current);
    write_tool_repair_diagnostics(response, current);
    write_runtime_assumptions(response, &current.assumptions);
    write_runtime_calibrations(response, &current.calibrations);
    write_runtime_handoff_artifacts(response, &current.handoff_artifacts);
    write_runtime_watchdog_digest(response, current);
}

fn write_route_admission_diagnostics(response: &mut String, current: &RouteSelection) {
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
        write_recent_admissions(response, &current.recent_admissions);
    }
}

fn write_tool_repair_diagnostics(response: &mut String, current: &RouteSelection) {
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
        write_recent_tool_repairs(response, &current.recent_tool_repairs);
    }
}

fn write_runtime_assumptions(response: &mut String, assumptions: &[RuntimeAssumption]) {
    if assumptions.is_empty() {
        return;
    }
    let _ = writeln!(
        response,
        "Runtime assumptions retained: {}",
        assumptions.len()
    );
    for assumption in assumptions.iter().rev().take(3) {
        let _ = writeln!(
            response,
            "Runtime assumption: {}",
            format_runtime_assumption(assumption)
        );
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

fn write_runtime_calibrations(response: &mut String, calibrations: &[RuntimeCalibrationRecord]) {
    if calibrations.is_empty() {
        return;
    }
    let _ = writeln!(
        response,
        "Runtime calibrations retained: {}",
        calibrations.len()
    );
    for record in calibrations.iter().take(3) {
        let _ = writeln!(
            response,
            "Runtime calibration: {} / {} / confidence={} / action={}",
            runtime_calibration_decision_kind_name(record.decision_kind),
            runtime_calibration_comparison_name(record.comparison),
            record.confidence_basis_points,
            runtime_calibration_action_name(record.recommended_action),
        );
    }
}

fn write_runtime_handoff_artifacts(response: &mut String, artifacts: &[RuntimeHandoffArtifact]) {
    if artifacts.is_empty() {
        return;
    }
    let _ = writeln!(
        response,
        "Runtime handoff artifacts retained: {}",
        artifacts.len()
    );
    for artifact in artifacts.iter().take(2) {
        let packet = &artifact.packet;
        let _ = writeln!(
            response,
            "Runtime handoff: {}{}",
            session_handoff_reason_name(packet.reason),
            packet
                .recommended_action
                .as_deref()
                .map(|action| format!(" / action={action}"))
                .unwrap_or_default(),
        );
        if let Some(task) = packet.active_task.as_deref() {
            let _ = writeln!(response, "Runtime handoff task: {task}");
        }
    }
}

fn write_runtime_watchdog_digest(response: &mut String, current: &RouteSelection) {
    let now_unix = current_unix_seconds();
    let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
        last_admission: current.last_admission.as_ref(),
        recent_admissions: &current.recent_admissions,
        last_tool_repair: current.last_tool_repair.as_ref(),
        recent_tool_repairs: &current.recent_tool_repairs,
        context_cache: current.context_cache.as_ref(),
        assumptions: &current.assumptions,
        subsystem_observations: &[],
        now_unix,
    });
    let alerts = append_runtime_watchdog_alerts(&current.watchdog_alerts, &digest.alerts, now_unix);
    let digest = RuntimeWatchdogDigest {
        generated_at_unix: digest.generated_at_unix,
        alerts,
    };
    if !digest.has_alerts() {
        return;
    }

    let degraded = digest
        .degraded_subsystems()
        .into_iter()
        .map(runtime_watchdog_subsystem_name)
        .collect::<Vec<_>>();
    let _ = writeln!(response, "Runtime watchdog alerts: {}", digest.alerts.len());
    if !degraded.is_empty() {
        let _ = writeln!(
            response,
            "Runtime watchdog degraded: {}",
            degraded.join(", ")
        );
    }
    for alert in digest.alerts.iter().take(3) {
        write_runtime_watchdog_alert(response, alert);
    }
}

fn write_runtime_watchdog_alert(response: &mut String, alert: &RuntimeWatchdogAlert) {
    let _ = writeln!(
        response,
        "Runtime watchdog: {} / {} / {} / action={}",
        runtime_watchdog_severity_name(alert.severity),
        runtime_watchdog_subsystem_name(alert.subsystem),
        runtime_watchdog_reason_name(alert.reason),
        runtime_watchdog_action_name(alert.recommended_action)
    );
}

fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn capability_lane_name(lane: CapabilityLane) -> &'static str {
    lane.as_str()
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
    if profile_supports_lane_confidently(profile, CapabilityLane::MultimodalUnderstanding) {
        covered.push("multimodal_understanding".to_string());
    }
    if profile_supports_lane_confidently(profile, CapabilityLane::ImageGeneration) {
        covered.push("image_generation".to_string());
    }
    if profile_supports_lane_confidently(profile, CapabilityLane::AudioGeneration) {
        covered.push("audio_generation".to_string());
    }
    if profile_supports_lane_confidently(profile, CapabilityLane::VideoGeneration) {
        covered.push("video_generation".to_string());
    }
    if profile_supports_lane_confidently(profile, CapabilityLane::MusicGeneration) {
        covered.push("music_generation".to_string());
    }
    if profile_supports_lane_confidently(profile, CapabilityLane::Embedding) {
        covered.push("embedding".to_string());
    }

    format!(
        "{} ({}/{})",
        covered.join(", "),
        resolved_model_profile_source_name(profile.features_source),
        resolved_model_profile_confidence_name(profile.features_confidence())
    )
}

fn format_basis_points(value: u32) -> String {
    format!("{}.{:02}%", value / 100, value % 100)
}

fn load_cached_model_profile(
    workspace_dir: &Path,
    provider_name: &str,
    model_name: &str,
    endpoint: Option<&str>,
) -> Option<CatalogModelProfile> {
    let cache_path = workspace_dir.join("state").join(MODEL_CACHE_FILE);
    let raw = std::fs::read_to_string(cache_path).ok()?;
    let state = serde_json::from_str::<ModelCacheState>(&raw).ok()?;
    let entry = state
        .entries
        .into_iter()
        .find(|entry| model_cache_entry_matches(entry, provider_name, endpoint))?;
    let profile = entry
        .profiles
        .into_iter()
        .find(|profile| profile.model == model_name)?;
    Some(CatalogModelProfile {
        context_window_tokens: profile.context_window_tokens,
        max_output_tokens: profile.max_output_tokens,
        features: profile.features,
        source: Some(CatalogModelProfileSource::CachedProviderCatalog),
        observed_at_unix: profile.observed_at_unix.or(Some(entry.fetched_at_unix)),
    })
}

fn model_cache_entry_matches(
    entry: &ModelCacheEntry,
    provider_name: &str,
    endpoint: Option<&str>,
) -> bool {
    if !entry.provider.eq_ignore_ascii_case(provider_name) {
        return false;
    }
    match endpoint
        .map(normalize_cache_endpoint)
        .filter(|value| !value.is_empty())
    {
        Some(endpoint) => entry.endpoint.as_deref() == Some(endpoint.as_str()),
        None => entry.endpoint.as_deref().is_none_or(str::is_empty),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::application::services::model_lane_resolution::{
        ResolvedModelProfile, ResolvedModelProfileSource,
    };
    use synapse_domain::application::services::runtime_assumptions::{
        RuntimeAssumption, RuntimeAssumptionFreshness, RuntimeAssumptionInvalidation,
        RuntimeAssumptionKind, RuntimeAssumptionReplacementPath, RuntimeAssumptionSource,
    };
    use synapse_domain::application::services::runtime_calibration::{
        RuntimeCalibrationAction, RuntimeCalibrationComparison, RuntimeCalibrationDecisionKind,
        RuntimeCalibrationOutcome,
    };
    use synapse_domain::application::services::runtime_watchdog::{
        RuntimeWatchdogAction, RuntimeWatchdogReason, RuntimeWatchdogSeverity,
        RuntimeWatchdogSubsystem,
    };
    use synapse_domain::application::services::session_handoff::{
        SessionHandoffPacket, SessionHandoffReason,
    };
    use synapse_domain::config::schema::{
        Config, ModelCandidateProfileConfig, ModelLaneCandidateConfig, ModelLaneConfig,
    };
    use synapse_domain::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairTrace};
    use synapse_domain::domain::turn_admission::{
        CandidateAdmissionReason, ContextPressureState, TurnAdmissionAction, TurnAdmissionSnapshot,
        TurnIntentCategory,
    };
    use synapse_domain::ports::route_selection::{ContextCacheStats, RouteAdmissionState};

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
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
        });
        assert!(response.contains("Current provider: `openai-codex`"));
        assert!(response.contains("Switch provider with `/models <provider>`"));
    }

    #[test]
    fn provider_alias_resolver_canonicalizes_known_aliases() {
        assert_eq!(resolve_provider_alias("grok").as_deref(), Some("xai"));
        assert_eq!(
            resolve_provider_alias("  GOOGLE  ").as_deref(),
            Some("gemini")
        );
        assert_eq!(resolve_provider_alias("unknown-provider"), None);
    }

    #[test]
    fn model_feature_coverage_filters_low_confidence_features() {
        let stale_observed_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_secs()
            .saturating_sub(31 * 24 * 60 * 60);
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::Vision, ModelFeature::ImageGeneration],
            features_source: ResolvedModelProfileSource::CachedProviderCatalog,
            observed_at_unix: Some(stale_observed_at_unix),
            ..Default::default()
        };

        let coverage = format_profile_feature_coverage(&profile);

        assert!(coverage.contains("reasoning"));
        assert!(coverage.contains("cached_provider_catalog/low"));
        assert!(!coverage.contains("multimodal_understanding"));
        assert!(!coverage.contains("image_generation"));
    }

    #[test]
    fn model_feature_coverage_accepts_curated_multimodal_features() {
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::Vision],
            features_source: ResolvedModelProfileSource::BundledCatalog,
            ..Default::default()
        };

        let coverage = format_profile_feature_coverage(&profile);

        assert!(coverage.contains("multimodal_understanding"));
        assert!(coverage.contains("bundled_catalog/medium"));
    }

    #[test]
    fn models_help_includes_effective_lanes_and_catalog_aliases() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::CheapReasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "test-provider".into(),
                model: "test-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: Default::default(),
            }],
        }];
        let response = build_models_help_response(
            &RouteSelection {
                provider: "test-provider".into(),
                model: "test-model".into(),
                lane: Some(CapabilityLane::CheapReasoning),
                candidate_index: Some(0),
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
            },
            &config,
        );
        assert!(response.contains("`cheap_reasoning` → test-model (test-provider)"));
        assert!(response.contains("Profile sources:"));
        assert!(response.contains("Current route limits:"));
        assert!(response.contains("Catalog model aliases:"));
        assert!(response.contains("`gemma31b` → google/gemma-4-31b-it (openrouter)"));
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
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
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
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
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
    fn models_help_includes_runtime_assumption_ledger() {
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
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: None,
                assumptions: vec![RuntimeAssumption {
                    kind: RuntimeAssumptionKind::ContextWindow,
                    source: RuntimeAssumptionSource::RouteAdmission,
                    freshness: RuntimeAssumptionFreshness::Challenged,
                    confidence_basis_points: 3_500,
                    value: "context_limit_exceeded".into(),
                    invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
                    replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
                }],
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
            },
            &config,
        );

        assert!(response.contains("Runtime assumptions retained: 1"));
        assert!(response.contains("kind=context_window"));
        assert!(response.contains("freshness=challenged"));
        assert!(response.contains("replacement_path=compact_session"));
        assert!(response.contains("Runtime watchdog alerts: 1"));
        assert!(response.contains(
            "Runtime watchdog: caution / context_budget / challenged_assumption / action=compact_context"
        ));
    }

    #[test]
    fn models_help_includes_runtime_calibration_ledger() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();

        let calibration = RuntimeCalibrationRecord {
            decision_kind: RuntimeCalibrationDecisionKind::ToolChoice,
            decision_signature: "tool:message_send".into(),
            suppression_key: None,
            confidence_basis_points: 9_000,
            outcome: RuntimeCalibrationOutcome::Failed,
            comparison: RuntimeCalibrationComparison::OverconfidentFailure,
            recommended_action: RuntimeCalibrationAction::SuppressChoice,
            observed_at_unix: 1_744_243_250,
        };

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
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: vec![calibration],
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
            },
            &config,
        );

        assert!(response.contains("Runtime calibrations retained: 1"));
        assert!(response.contains(
            "Runtime calibration: tool_choice / overconfident_failure / confidence=9000 / action=suppress_choice"
        ));
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
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
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
    fn providers_help_uses_shared_runtime_watchdog_digest() {
        let observed_at_unix = current_unix_seconds();
        let response = build_providers_help_response(&RouteSelection {
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: Some(ToolRepairTrace {
                observed_at_unix,
                tool_name: "message_send".into(),
                failure_kind: ToolFailureKind::ReportedFailure,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                detail: Some("missing delivery target".into()),
            }),
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
        });

        assert!(response.contains("Runtime watchdog alerts: 1"));
        assert!(response.contains(
            "Runtime watchdog: caution / tool_execution / tool_failure / action=repair_tool_request"
        ));
    }

    #[test]
    fn providers_help_includes_retained_runtime_trace_artifacts() {
        let observed_at_unix = current_unix_seconds();
        let response = build_providers_help_response(&RouteSelection {
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
            watchdog_alerts: vec![RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::MemoryBackend,
                severity: RuntimeWatchdogSeverity::Degraded,
                reason: RuntimeWatchdogReason::SubsystemDegraded,
                recommended_action: RuntimeWatchdogAction::CheckMemoryBackend,
                observed_at_unix,
            }],
            handoff_artifacts: vec![RuntimeHandoffArtifact {
                observed_at_unix,
                packet: SessionHandoffPacket {
                    reason: SessionHandoffReason::ContextOverflow,
                    recommended_action: Some("start_fresh_handoff".into()),
                    active_task: Some("continue after compact handoff".into()),
                    current_defaults: Vec::new(),
                    anchors: Vec::new(),
                    unresolved_questions: Vec::new(),
                    assumptions: Vec::new(),
                },
            }],
        });

        assert!(response.contains("Runtime handoff artifacts retained: 1"));
        assert!(response.contains("Runtime handoff: context_overflow / action=start_fresh_handoff"));
        assert!(response.contains("Runtime watchdog degraded: memory_backend"));
        assert!(response.contains(
            "Runtime watchdog: degraded / memory_backend / subsystem_degraded / action=check_memory_backend"
        ));
    }

    #[test]
    fn providers_help_includes_runtime_calibration_ledger() {
        let response = build_providers_help_response(&RouteSelection {
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: vec![RuntimeCalibrationRecord {
                decision_kind: RuntimeCalibrationDecisionKind::RouteChoice,
                decision_signature: "route:openrouter:qwen/qwen3.6-plus".into(),
                suppression_key: None,
                confidence_basis_points: 9_000,
                outcome: RuntimeCalibrationOutcome::Failed,
                comparison: RuntimeCalibrationComparison::OverconfidentFailure,
                recommended_action: RuntimeCalibrationAction::SuppressChoice,
                observed_at_unix: 1_744_243_260,
            }],
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
        });

        assert!(response.contains("Runtime calibrations retained: 1"));
        assert!(response.contains(
            "Runtime calibration: route_choice / overconfident_failure / confidence=9000 / action=suppress_choice"
        ));
    }

    #[test]
    fn providers_help_includes_runtime_assumption_ledger() {
        let response = build_providers_help_response(&RouteSelection {
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: vec![RuntimeAssumption {
                kind: RuntimeAssumptionKind::ContextWindow,
                source: RuntimeAssumptionSource::RouteAdmission,
                freshness: RuntimeAssumptionFreshness::Challenged,
                confidence_basis_points: 3_500,
                value: "context_limit_exceeded".into(),
                invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
                replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
            }],
            calibrations: Vec::new(),
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
        });

        assert!(response.contains("Runtime assumptions retained: 1"));
        assert!(response.contains("kind=context_window"));
        assert!(response.contains("replacement_path=compact_session"));
        assert!(response.contains("Runtime watchdog alerts: 1"));
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
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
            },
            &config,
        );

        assert!(response.contains("Current route feature coverage: reasoning, multimodal_understanding, image_generation, audio_generation"));
    }

    #[test]
    fn models_help_includes_native_context_policy_and_cache_stats() {
        let workspace = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();
        config.agent.prompt_caching = true;
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "x-ai/grok-4.20".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(2_000_000),
                    max_output_tokens: Some(128_000),
                    features: vec![
                        ModelFeature::PromptCaching,
                        ModelFeature::ServerContinuation,
                    ],
                },
            }],
        }];

        let response = build_models_help_response(
            &RouteSelection {
                provider: "openrouter".into(),
                model: "x-ai/grok-4.20".into(),
                lane: Some(CapabilityLane::Reasoning),
                candidate_index: Some(0),
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: Some(ContextCacheStats {
                    entries: 7,
                    hits: 11,
                    max_entries: 256,
                    ttl_secs: 172_800,
                    loaded: true,
                    threshold_basis_points: 5_000,
                    target_basis_points: 2_500,
                    protect_first_n: 2,
                    protect_last_n: 6,
                    summary_basis_points: 2_000,
                    max_source_chars: 60_000,
                    max_summary_chars: 12_000,
                }),
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
            },
            &config,
        );

        assert!(response.contains(
            "Native context policy: prompt_cache_supported=`true` prompt_cache_enabled=`true` server_continuation_supported=`true`"
        ));
        assert!(response.contains(
            "History compaction cache: entries=`7` hits=`11` max=`256` ttl_secs=`172800` loaded=`true`"
        ));
        assert!(response.contains(
            "threshold=`50.00%` target=`25.00%` protect=`2/6` summary=`20.00%` source_chars=`60000` summary_chars=`12000`"
        ));
    }

    #[test]
    fn workspace_profile_catalog_falls_back_to_bundled_catalog() {
        let workspace = tempfile::tempdir().unwrap();
        let catalog = WorkspaceModelProfileCatalog::new(workspace.path());

        let profile = catalog
            .lookup_model_profile("openrouter", "google/gemma-4-31b-it")
            .expect("bundled Gemma profile should resolve without live cache");

        assert_eq!(profile.context_window_tokens, Some(262_144));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::BundledCatalog)
        );
        assert!(profile.features.contains(&ModelFeature::ToolCalling));
        assert!(profile
            .features
            .contains(&ModelFeature::MultimodalUnderstanding));
    }

    #[test]
    fn workspace_profile_catalog_scopes_cached_profiles_by_endpoint() {
        let workspace = tempfile::tempdir().unwrap();
        let cache_dir = workspace.path().join("state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(
            cache_dir.join(MODEL_CACHE_FILE),
            r#"{
              "entries": [
                {
                  "provider": "openai",
                  "endpoint": "https://api.openai.com/v1",
                  "fetched_at_unix": 100,
                  "models": ["shared-model"],
                  "profiles": [
                    {
                      "model": "shared-model",
                      "context_window_tokens": 111,
                      "max_output_tokens": 22,
                      "features": []
                    }
                  ]
                },
                {
                  "provider": "openai",
                  "endpoint": "https://openrouter.ai/api/v1",
                  "fetched_at_unix": 200,
                  "models": ["shared-model"],
                  "profiles": [
                    {
                      "model": "shared-model",
                      "context_window_tokens": 222,
                      "max_output_tokens": 44,
                      "features": []
                    }
                  ]
                }
              ]
            }"#,
        )
        .unwrap();

        let native_catalog = WorkspaceModelProfileCatalog::with_provider_endpoint(
            workspace.path(),
            Some("openai"),
            Some("https://api.openai.com/v1/"),
        );
        let router_catalog = WorkspaceModelProfileCatalog::with_provider_endpoint(
            workspace.path(),
            Some("openai"),
            Some("https://openrouter.ai/api/v1"),
        );
        let legacy_catalog = WorkspaceModelProfileCatalog::new(workspace.path());

        let native = native_catalog
            .lookup_model_profile("openai", "shared-model")
            .expect("native endpoint profile should resolve");
        let router = router_catalog
            .lookup_model_profile("openai", "shared-model")
            .expect("router endpoint profile should resolve");

        assert_eq!(native.context_window_tokens, Some(111));
        assert_eq!(native.max_output_tokens, Some(22));
        assert_eq!(native.observed_at_unix, Some(100));
        assert_eq!(router.context_window_tokens, Some(222));
        assert_eq!(router.max_output_tokens, Some(44));
        assert_eq!(router.observed_at_unix, Some(200));
        assert!(legacy_catalog
            .lookup_model_profile("openai", "shared-model")
            .is_none());
    }

    #[test]
    fn workspace_profile_catalog_uses_endpoint_inferred_provider_metadata() {
        let workspace = tempfile::tempdir().unwrap();
        let cache_dir = workspace.path().join("state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(
            cache_dir.join(MODEL_CACHE_FILE),
            r#"{
              "entries": [
                {
                  "provider": "deepseek",
                  "endpoint": "https://api.deepseek.com/v1",
                  "fetched_at_unix": 300,
                  "models": ["deepseek-chat"],
                  "profiles": [
                    {
                      "model": "deepseek-chat",
                      "context_window_tokens": 333,
                      "max_output_tokens": 55,
                      "features": []
                    }
                  ]
                }
              ]
            }"#,
        )
        .unwrap();
        let mut config = Config::default();
        config.workspace_dir = workspace.path().to_path_buf();
        config.default_provider = Some("openai".to_string());
        config.api_url = Some("https://api.deepseek.com/v1/".to_string());

        let catalog = WorkspaceModelProfileCatalog::from_config(&config);
        let profile = catalog
            .lookup_model_profile("openai", "deepseek-chat")
            .expect("endpoint-inferred provider cache should resolve");

        assert_eq!(profile.context_window_tokens, Some(333));
        assert_eq!(profile.max_output_tokens, Some(55));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::CachedProviderCatalog)
        );
        assert_eq!(profile.observed_at_unix, Some(300));
    }

    #[test]
    fn workspace_profile_catalog_records_lower_context_limit_observation() {
        let workspace = tempfile::tempdir().unwrap();
        let cache_dir = workspace.path().join("state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(
            cache_dir.join(MODEL_CACHE_FILE),
            r#"{
              "entries": [
                {
                  "provider": "openrouter",
                  "endpoint": "https://openrouter.ai/api/v1",
                  "fetched_at_unix": 100,
                  "models": ["x-ai/grok-4.20"],
                  "profiles": [
                    {
                      "model": "x-ai/grok-4.20",
                      "context_window_tokens": 2000000,
                      "max_output_tokens": 128000,
                      "features": []
                    }
                  ]
                }
              ]
            }"#,
        )
        .unwrap();
        let catalog = WorkspaceModelProfileCatalog::with_provider_endpoint(
            workspace.path(),
            Some("openrouter"),
            Some("https://openrouter.ai/api/v1/"),
        );

        catalog
            .record_context_limit_observation(
                "openrouter",
                "x-ai/grok-4.20",
                ContextLimitProfileObservation {
                    observed_context_window_tokens: Some(128_000),
                    requested_context_tokens: Some(140_000),
                },
            )
            .unwrap();

        let profile = catalog
            .lookup_model_profile("openrouter", "x-ai/grok-4.20")
            .expect("observed profile should resolve");

        assert_eq!(profile.context_window_tokens, Some(128_000));
        assert_eq!(profile.max_output_tokens, Some(128_000));
        assert!(profile.observed_at_unix.is_some_and(|value| value >= 100));
    }

    #[test]
    fn workspace_profile_catalog_does_not_raise_context_limit_observation() {
        let workspace = tempfile::tempdir().unwrap();
        let catalog = WorkspaceModelProfileCatalog::with_provider_endpoint(
            workspace.path(),
            Some("openai"),
            Some("https://api.deepseek.com/v1/"),
        );

        catalog
            .record_context_limit_observation(
                "openai",
                "deepseek-chat",
                ContextLimitProfileObservation {
                    observed_context_window_tokens: Some(64_000),
                    requested_context_tokens: Some(70_000),
                },
            )
            .unwrap();
        catalog
            .record_context_limit_observation(
                "openai",
                "deepseek-chat",
                ContextLimitProfileObservation {
                    observed_context_window_tokens: Some(128_000),
                    requested_context_tokens: Some(140_000),
                },
            )
            .unwrap();

        let profile = catalog
            .lookup_model_profile("openai", "deepseek-chat")
            .expect("endpoint-inferred observation should resolve");

        assert_eq!(profile.context_window_tokens, Some(64_000));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::CachedProviderCatalog)
        );
    }
}
