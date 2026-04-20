use std::fmt::Write;
use std::path::{Path, PathBuf};

use synapse_domain::application::services::capability_doctor::{
    build_capability_doctor_report, CapabilityDoctorAdapterStatus, CapabilityDoctorBackendStatus,
    CapabilityDoctorChannelStatus, CapabilityDoctorInput, CapabilityDoctorProviderKeyStatus,
    CapabilityDoctorReadiness, CapabilityDoctorReport, CapabilityDoctorSeverity,
    CapabilityDoctorSubsystem,
};
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
use synapse_domain::application::services::runtime_decision_trace::{
    RuntimeDecisionTrace, RuntimeTraceRouteRef,
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
use synapse_domain::domain::memory::EmbeddingProfile;
use synapse_domain::domain::tool_repair::{
    tool_argument_shape_kind_name, tool_failure_kind_name, tool_repair_action_name,
    tool_repair_outcome_name, ToolRepairAction, ToolRepairTrace,
};
use synapse_domain::domain::turn_admission::{
    admission_repair_hint_name, context_pressure_state_name, turn_admission_action_name,
    turn_intent_name, AdmissionRepairHint, CandidateAdmissionReason,
};
use synapse_domain::ports::model_profile_catalog::{
    CatalogModelProfile, CatalogModelProfileSource, ContextLimitProfileObservation,
    ModelProfileCatalogPort, ModelProfileObservation,
};
use synapse_domain::ports::provider::ProviderCapabilities;
use synapse_domain::ports::route_selection::{RouteAdmissionState, RouteSelection};
use synapse_domain::ports::tool::tool_runtime_role_name;

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

        let entry_index = ensure_model_cache_entry_index(
            &mut state,
            target_provider.as_str(),
            endpoint.as_deref(),
            now,
        );
        let entry = &mut state.entries[entry_index];
        let mut changed = ensure_model_cache_entry_model(entry, model);
        let (profile_index, created) = ensure_model_profile_cache_entry_index(entry, model);
        changed |= created;
        let profile = &mut entry.profiles[profile_index];
        if profile
            .context_window_tokens
            .is_none_or(|existing| observed_window <= existing)
        {
            profile.context_window_tokens = Some(observed_window);
            profile.observed_at_unix = Some(now);
            changed = true;
        }

        if changed {
            save_model_cache_state(cache_path.as_path(), &state)?;
        }
        Ok(())
    }

    pub(crate) fn record_model_profile_observation(
        &self,
        provider: &str,
        model: &str,
        observation: ModelProfileObservation,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() {
            anyhow::bail!("model profile observation provider cannot be empty");
        }
        if model.is_empty() {
            anyhow::bail!("model profile observation model cannot be empty");
        }

        let context_window_tokens = observation
            .context_window_tokens
            .filter(|tokens| *tokens > 0);
        let max_output_tokens = observation.max_output_tokens.filter(|tokens| *tokens > 0);
        let mut features = observation.features;
        dedupe_model_features(&mut features);
        if context_window_tokens.is_none() && max_output_tokens.is_none() && features.is_empty() {
            return Ok(());
        }

        let endpoint = self.endpoint_for_provider(provider).map(str::to_string);
        let candidates = self.provider_lookup_candidates(provider);
        let cache_path = model_cache_path(self.workspace_dir.as_path());
        let mut state = load_model_cache_state_for_update(cache_path.as_path())?;
        let target_provider =
            select_observation_cache_provider(&state, candidates.as_slice(), endpoint.as_deref());
        let now = current_unix_secs();

        let entry_index = ensure_model_cache_entry_index(
            &mut state,
            target_provider.as_str(),
            endpoint.as_deref(),
            now,
        );
        let entry = &mut state.entries[entry_index];
        let mut changed = ensure_model_cache_entry_model(entry, model);
        let (profile_index, created) = ensure_model_profile_cache_entry_index(entry, model);
        changed |= created;

        {
            let profile = &mut entry.profiles[profile_index];
            if let Some(context_window_tokens) = context_window_tokens {
                if profile.context_window_tokens != Some(context_window_tokens) {
                    profile.context_window_tokens = Some(context_window_tokens);
                    changed = true;
                }
            }
            if let Some(max_output_tokens) = max_output_tokens {
                if profile.max_output_tokens != Some(max_output_tokens) {
                    profile.max_output_tokens = Some(max_output_tokens);
                    changed = true;
                }
            }
            if !features.is_empty() && profile.features != features {
                profile.features = features;
                changed = true;
            }
            if changed {
                profile.observed_at_unix = Some(now);
            }
        }

        if changed {
            entry.fetched_at_unix = now;
            save_model_cache_state(cache_path.as_path(), &state)?;
        }
        Ok(())
    }
}

impl ModelProfileCatalogPort for WorkspaceModelProfileCatalog {
    fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile> {
        let endpoint = self.endpoint_for_provider(provider);
        let candidates = self.provider_lookup_candidates(provider);
        let cached = candidates.iter().find_map(|candidate_provider| {
            load_cached_model_profile(
                self.workspace_dir.as_path(),
                candidate_provider,
                model,
                endpoint,
            )
        });
        let bundled = candidates
            .iter()
            .find_map(|candidate_provider| {
                synapse_domain::config::model_catalog::model_profile(candidate_provider, model)
            })
            .or_else(|| synapse_domain::config::model_catalog::model_profile(provider, model));

        merge_cached_and_bundled_profile(cached, bundled)
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

    fn record_model_profile_observation(
        &self,
        provider: &str,
        model: &str,
        observation: ModelProfileObservation,
    ) -> anyhow::Result<()> {
        WorkspaceModelProfileCatalog::record_model_profile_observation(
            self,
            provider,
            model,
            observation,
        )
    }
}

fn merge_cached_and_bundled_profile(
    cached: Option<CatalogModelProfile>,
    bundled: Option<CatalogModelProfile>,
) -> Option<CatalogModelProfile> {
    let Some(mut cached) = cached else {
        return bundled;
    };
    let Some(bundled) = bundled else {
        return Some(cached);
    };
    if cached.context_window_tokens.is_none()
        && cached.max_output_tokens.is_none()
        && cached.features.is_empty()
    {
        return Some(bundled);
    }

    if cached.context_window_tokens.is_none() {
        cached.context_window_tokens = bundled.context_window_tokens;
    }
    if cached.max_output_tokens.is_none() {
        cached.max_output_tokens = bundled.max_output_tokens;
    }
    if cached.features.is_empty() {
        cached.features = bundled.features;
    }
    Some(cached)
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
        CapabilityLane::Compaction,
        CapabilityLane::Embedding,
        CapabilityLane::WebExtraction,
        CapabilityLane::ToolValidator,
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

    let catalog_aliases = synapse_domain::config::model_catalog::route_aliases();
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

pub(crate) struct RuntimeCapabilityDoctorInput<'a> {
    pub(crate) route: &'a RouteSelection,
    pub(crate) config: &'a Config,
    pub(crate) provider_capabilities: ProviderCapabilities,
    pub(crate) provider_plan_denial: Option<&'a str>,
    pub(crate) tool_registry_count: usize,
    pub(crate) memory_backend_name: Option<&'a str>,
    pub(crate) memory_backend_healthy: Option<bool>,
    pub(crate) memory_backend_configured: bool,
    pub(crate) embedding_profile: Option<&'a EmbeddingProfile>,
    pub(crate) channel_name: Option<&'a str>,
    pub(crate) channel_available: Option<bool>,
}

pub(crate) fn build_runtime_capability_doctor_report(
    input: RuntimeCapabilityDoctorInput<'_>,
) -> CapabilityDoctorReport {
    let catalog = WorkspaceModelProfileCatalog::from_config(input.config);
    let provider_adapter = provider_adapter_status(input.route.provider.as_str());
    let provider_key = provider_key_status_for_route(
        input.config,
        input.route,
        provider_is_local(input.route.provider.as_str()),
        &catalog,
    );

    build_capability_doctor_report(CapabilityDoctorInput {
        config: input.config,
        route: input.route,
        catalog: Some(&catalog),
        provider_adapter,
        provider_key,
        provider_capabilities: input.provider_capabilities,
        provider_plan_denial: input.provider_plan_denial,
        tool_registry_count: input.tool_registry_count,
        memory_backend: CapabilityDoctorBackendStatus {
            configured: input.memory_backend_configured,
            healthy: input.memory_backend_healthy,
            name: input.memory_backend_name,
        },
        embedding_profile: input.embedding_profile,
        channel_delivery: CapabilityDoctorChannelStatus {
            surface: input.channel_name,
            available: input.channel_available,
        },
        generated_at_unix: current_unix_seconds(),
    })
}

pub(crate) fn build_capability_doctor_response(report: &CapabilityDoctorReport) -> String {
    let mut response = String::new();
    let _ = writeln!(response, "Capability doctor");
    let _ = writeln!(
        response,
        "Route: `{}` / `{}` / lane=`{}`",
        report.route.provider,
        report.route.model,
        report.route.lane.as_deref().unwrap_or("default")
    );
    let _ = writeln!(
        response,
        "Summary: ok=`{}` warn=`{}` error=`{}`",
        report.summary.ok, report.summary.warn, report.summary.error
    );
    let _ = writeln!(
        response,
        "Profile: ctx=`{}` output=`{}` features=`{}`",
        report
            .model_profile
            .context_window_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "?".to_string()),
        report
            .model_profile
            .max_output_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "?".to_string()),
        if report.model_profile.features.is_empty() {
            "?".to_string()
        } else {
            report.model_profile.features.join("+")
        }
    );
    let _ = writeln!(
        response,
        "Profile quality: ctx=`{}/{}/{}` output=`{}/{}/{}` features=`{}/{}/{}`",
        report.model_profile.context_window_source,
        report.model_profile.context_window_freshness,
        report.model_profile.context_window_confidence,
        report.model_profile.max_output_source,
        report.model_profile.max_output_freshness,
        report.model_profile.max_output_confidence,
        report.model_profile.features_source,
        report.model_profile.features_freshness,
        report.model_profile.features_confidence,
    );
    if let Some(observed_at) = report.model_profile.observed_at_unix {
        let _ = writeln!(response, "Profile observed_at_unix: `{observed_at}`");
    }
    response.push_str("\nReadiness graph:\n");
    for node in &report.nodes {
        let _ = writeln!(
            response,
            "- `{}` / `{}` / `{}`: `{}`",
            capability_doctor_severity_name(node.severity),
            capability_doctor_subsystem_name(node.subsystem),
            node.subject,
            capability_doctor_readiness_name(node.readiness)
        );
        if !node.evidence.is_empty() {
            let _ = writeln!(response, "  evidence: {}", node.evidence.join("; "));
        }
        if let Some(recommendation) = node.recommendation.as_deref() {
            let _ = writeln!(response, "  recommendation: {recommendation}");
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

fn ensure_model_cache_entry_index(
    state: &mut ModelCacheState,
    target_provider: &str,
    endpoint: Option<&str>,
    now: u64,
) -> usize {
    if let Some(index) = state
        .entries
        .iter()
        .position(|entry| model_cache_entry_matches(entry, target_provider, endpoint))
    {
        index
    } else {
        state.entries.push(ModelCacheEntry {
            provider: target_provider.to_string(),
            endpoint: endpoint.map(str::to_string),
            fetched_at_unix: now,
            models: Vec::new(),
            profiles: Vec::new(),
        });
        state.entries.len() - 1
    }
}

fn ensure_model_cache_entry_model(entry: &mut ModelCacheEntry, model: &str) -> bool {
    if entry
        .models
        .iter()
        .any(|cached_model| cached_model.eq_ignore_ascii_case(model))
    {
        false
    } else {
        entry.models.push(model.to_string());
        true
    }
}

fn ensure_model_profile_cache_entry_index(
    entry: &mut ModelCacheEntry,
    model: &str,
) -> (usize, bool) {
    if let Some(index) = entry
        .profiles
        .iter()
        .position(|profile| profile.model.eq_ignore_ascii_case(model))
    {
        (index, false)
    } else {
        entry.profiles.push(ModelProfileCacheEntry {
            model: model.to_string(),
            ..Default::default()
        });
        (entry.profiles.len() - 1, true)
    }
}

fn dedupe_model_features(features: &mut Vec<ModelFeature>) {
    let mut deduped = Vec::with_capacity(features.len());
    for feature in features.drain(..) {
        if !deduped.contains(&feature) {
            deduped.push(feature);
        }
    }
    *features = deduped;
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

fn admission_required_lane(admission: &RouteAdmissionState) -> Option<CapabilityLane> {
    admission.required_lane
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
    write_runtime_decision_trace_diagnostics(response, &current.runtime_decision_traces);
    write_tool_repair_diagnostics(response, current);
    write_runtime_assumptions(response, &current.assumptions);
    write_runtime_calibrations(response, &current.calibrations);
    write_runtime_handoff_artifacts(response, &current.handoff_artifacts);
    write_runtime_watchdog_digest(response, current);
}

fn write_route_admission_diagnostics(response: &mut String, current: &RouteSelection) {
    if let Some(admission) = current.last_admission.as_ref() {
        if let Some(lane) = admission_required_lane(admission) {
            let _ = writeln!(
                response,
                "Last admission required lane: `{}`",
                capability_lane_name(lane)
            );
        }
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

fn write_runtime_decision_trace_diagnostics(
    response: &mut String,
    traces: &[RuntimeDecisionTrace],
) {
    if traces.is_empty() {
        return;
    }
    let _ = writeln!(
        response,
        "Runtime decision traces retained: {}",
        traces.len()
    );
    for trace in traces.iter().rev().take(3) {
        let _ = writeln!(
            response,
            "Runtime decision trace: {} -> {} / intent={} / pressure={} / action={}",
            format_trace_route_ref(&trace.route.before),
            format_trace_route_ref(&trace.route.after),
            trace.route.intent,
            trace.route.pressure_state,
            trace.route.action
        );
        if !trace.route.reasons.is_empty() {
            let _ = writeln!(
                response,
                "Runtime decision reasons: {}",
                trace.route.reasons.join(", ")
            );
        }
        if let Some(repair) = trace.route.recommended_action.as_deref() {
            let _ = writeln!(response, "Runtime decision suggested action: {repair}");
        }
        let _ = writeln!(
            response,
            "Runtime decision profile: context={} / output={} / features={}",
            trace.model_profile.context_window_confidence,
            trace.model_profile.max_output_confidence,
            trace.model_profile.features_confidence,
        );
        let _ = writeln!(
            response,
            "Runtime decision context: tier={} / estimated_tokens={} / target_tokens={} / ceiling_tokens={} / compact={}",
            trace.context.budget_tier,
            trace.context.estimated_total_tokens,
            trace.context.target_total_tokens,
            trace.context.ceiling_total_tokens,
            trace.context.requires_compaction,
        );
        if let Some(mode) = trace.context.condensation_mode.as_deref() {
            let _ = writeln!(
                response,
                "Runtime decision condensation: mode={} / target={} / min_reclaim_chars={}",
                mode,
                trace
                    .context
                    .condensation_target
                    .as_deref()
                    .unwrap_or("none"),
                trace
                    .context
                    .condensation_minimum_reclaim_chars
                    .unwrap_or(0)
            );
        }
        if let Some(cache) = trace.context.cache {
            let _ = writeln!(
                response,
                "Runtime decision context cache: entries={} / hits={} / loaded={}",
                cache.entries, cache.hits, cache.loaded
            );
        }
        if !trace.tools.is_empty() {
            let summary = trace
                .tools
                .iter()
                .rev()
                .take(3)
                .map(|tool| {
                    format!(
                        "{}:{}->{} outcome={} repeats={} role={}",
                        tool.tool_name,
                        tool.failure_kind,
                        tool.suggested_action,
                        tool.repair_outcome,
                        tool.repeat_count.max(1),
                        tool.tool_role.as_deref().unwrap_or("unknown")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(response, "Runtime decision tools: {summary}");
            for tool in trace.tools.iter().rev().take(3) {
                if let Some(shape) = tool.argument_shape.as_deref() {
                    let _ = writeln!(
                        response,
                        "Runtime decision tool args: {} / {}",
                        tool.tool_name, shape
                    );
                }
            }
        }
        if !trace.memory.is_empty() {
            let summary = trace
                .memory
                .iter()
                .rev()
                .take(3)
                .map(|memory| {
                    format!(
                        "{}:{}:{} applied={}",
                        memory.source, memory.category, memory.action, memory.applied
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(response, "Runtime decision memory: {summary}");
        }
        if !trace.auxiliary.is_empty() {
            let summary = trace
                .auxiliary
                .iter()
                .rev()
                .take(3)
                .map(|aux| format!("{}:{} count={}", aux.kind, aux.action, aux.count))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(response, "Runtime decision auxiliary: {summary}");
        }
    }
}

fn format_trace_route_ref(route: &RuntimeTraceRouteRef) -> String {
    let lane = route.lane.as_deref().unwrap_or("none");
    let candidate = route
        .candidate_index
        .map(|index| index.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "{}/{} lane={} candidate={}",
        route.provider, route.model, lane, candidate
    )
}

fn write_tool_repair_diagnostics(response: &mut String, current: &RouteSelection) {
    if let Some(repair) = current.last_tool_repair.as_ref() {
        let _ = writeln!(
            response,
            "Last tool repair: {} / {} / outcome={} / repeats={} / role={}",
            tool_failure_kind_name(repair.failure_kind),
            format_tool_repair_action(repair),
            tool_repair_outcome_name(repair.repair_outcome),
            repair.repeat_count.max(1),
            format_tool_repair_role(repair)
        );
        if let Some(route) = format_tool_repair_route(repair) {
            let _ = writeln!(response, "Last tool repair route: {route}");
        }
        if let Some(shape) = format_tool_repair_argument_shape(repair) {
            let _ = writeln!(response, "Last tool repair args: {shape}");
        }
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
            "Recent tool repair: {} / {} / {} / outcome={} / repeats={} / role={}",
            repair.tool_name,
            tool_failure_kind_name(repair.failure_kind),
            format_tool_repair_action(repair),
            tool_repair_outcome_name(repair.repair_outcome),
            repair.repeat_count.max(1),
            format_tool_repair_role(repair)
        );
        if let Some(route) = format_tool_repair_route(repair) {
            let _ = writeln!(response, "Recent tool repair route: {route}");
        }
        if let Some(shape) = format_tool_repair_argument_shape(repair) {
            let _ = writeln!(response, "Recent tool repair args: {shape}");
        }
        if let Some(detail) = repair.detail.as_deref() {
            let _ = writeln!(response, "Recent tool repair detail: {}", detail);
        }
    }
}

fn format_tool_repair_role(repair: &ToolRepairTrace) -> &'static str {
    repair
        .tool_role
        .map(tool_runtime_role_name)
        .unwrap_or("unknown")
}

fn format_tool_repair_route(repair: &ToolRepairTrace) -> Option<String> {
    repair.route.as_ref().map(|route| {
        format!(
            "{}/{} lane={} candidate={}",
            route.provider,
            route.model,
            route.lane.map(|lane| lane.as_str()).unwrap_or("none"),
            route
                .candidate_index
                .map(|index| index.to_string())
                .unwrap_or_else(|| "none".to_string())
        )
    })
}

fn format_tool_repair_argument_shape(repair: &ToolRepairTrace) -> Option<String> {
    repair.argument_shape.as_ref().map(|shape| {
        let keys = if shape.top_level_keys.is_empty() {
            "none".to_string()
        } else {
            shape.top_level_keys.join(",")
        };
        let missing = if shape.missing_required_keys.is_empty() {
            "none".to_string()
        } else {
            shape.missing_required_keys.join(",")
        };
        format!(
            "root={} keys={} missing_required={} approx_chars={}",
            tool_argument_shape_kind_name(shape.root_kind),
            keys,
            missing,
            shape.approximate_chars
        )
    })
}

fn write_recent_admissions(response: &mut String, recent_admissions: &[RouteAdmissionState]) {
    for admission in recent_admissions.iter().rev().take(3) {
        if let Some(lane) = admission_required_lane(admission) {
            let _ = writeln!(
                response,
                "Recent admission required lane: `{}`",
                capability_lane_name(lane)
            );
        }
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

fn provider_adapter_status(provider: &str) -> CapabilityDoctorAdapterStatus {
    let provider = provider.trim();
    if provider.is_empty() {
        return CapabilityDoctorAdapterStatus::Missing;
    }
    if provider.starts_with("custom:") || provider.starts_with("anthropic-custom:") {
        return CapabilityDoctorAdapterStatus::Available;
    }
    if synapse_providers::list_providers().into_iter().any(|info| {
        info.name.eq_ignore_ascii_case(provider)
            || info
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(provider))
    }) {
        CapabilityDoctorAdapterStatus::Available
    } else {
        CapabilityDoctorAdapterStatus::Missing
    }
}

fn provider_is_local(provider: &str) -> bool {
    synapse_providers::list_providers().into_iter().any(|info| {
        (info.name.eq_ignore_ascii_case(provider)
            || info
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(provider)))
            && info.local
    })
}

fn provider_key_status_for_route(
    config: &Config,
    route: &RouteSelection,
    provider_is_local: bool,
    catalog: &WorkspaceModelProfileCatalog,
) -> CapabilityDoctorProviderKeyStatus {
    if provider_is_local && !local_provider_route_requires_key(route) {
        return CapabilityDoctorProviderKeyStatus::NotRequired;
    }
    if route_candidate_has_key(config, route, catalog) {
        return CapabilityDoctorProviderKeyStatus::Present;
    }
    if config
        .default_provider
        .as_deref()
        .is_some_and(|provider| provider.eq_ignore_ascii_case(&route.provider))
        && provider_uses_config_api_key(config, route.provider.as_str())
        && (config
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
            || !config.reliability.api_keys.is_empty())
    {
        return CapabilityDoctorProviderKeyStatus::Present;
    }
    if common_provider_env_key_present(route.provider.as_str()) {
        return CapabilityDoctorProviderKeyStatus::Present;
    }
    if let Some(status) = special_provider_auth_status(config, route.provider.as_str()) {
        return status;
    }
    CapabilityDoctorProviderKeyStatus::Missing
}

fn provider_uses_config_api_key(config: &Config, provider: &str) -> bool {
    let normalized = provider.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "bedrock" | "aws-bedrock" => false,
        "openai-codex" | "openai_codex" | "codex" => {
            config
                .api_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty())
                || env_key_present("SYNAPSECLAW_CODEX_RESPONSES_URL")
                || env_key_present("SYNAPSECLAW_CODEX_BASE_URL")
        }
        _ => true,
    }
}

fn local_provider_route_requires_key(route: &RouteSelection) -> bool {
    route.provider.eq_ignore_ascii_case("ollama") && route.model.trim().ends_with(":cloud")
}

fn route_candidate_has_key(
    config: &Config,
    route: &RouteSelection,
    catalog: &WorkspaceModelProfileCatalog,
) -> bool {
    let Some(lane) = route.lane else {
        return false;
    };
    let candidates = resolve_lane_candidates(config, lane, Some(catalog));
    candidates
        .iter()
        .enumerate()
        .find(|(index, candidate)| {
            route
                .candidate_index
                .is_some_and(|candidate_index| candidate_index == *index)
                || (candidate.provider.eq_ignore_ascii_case(&route.provider)
                    && candidate.model.eq_ignore_ascii_case(&route.model))
        })
        .is_some_and(|(_, candidate)| {
            candidate
                .api_key
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty())
                || candidate
                    .api_key_env
                    .as_deref()
                    .is_some_and(env_key_present)
        })
}

fn common_provider_env_key_present(provider: &str) -> bool {
    let normalized = provider.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "bedrock" | "aws-bedrock") {
        return env_key_present("AWS_ACCESS_KEY_ID") && env_key_present("AWS_SECRET_ACCESS_KEY");
    }

    provider_api_key_env_names(provider)
        .into_iter()
        .any(env_key_present)
}

fn provider_api_key_env_names(provider: &str) -> Vec<&'static str> {
    let normalized = provider.trim().to_ascii_lowercase();
    let mut names = Vec::new();
    match normalized.as_str() {
        "openrouter" => names.push("OPENROUTER_API_KEY"),
        "openai" => names.push("OPENAI_API_KEY"),
        "openai-codex" | "openai_codex" | "codex" => {
            names.push("SYNAPSECLAW_OPENAI_CODEX_ACCESS_TOKEN");
            names.push("SYNAPSECLAW_OPENAI_CODEX_REFRESH_TOKEN");
        }
        "anthropic" | "claude" | "claude-code" => {
            names.push("ANTHROPIC_OAUTH_TOKEN");
            names.push("ANTHROPIC_API_KEY");
        }
        "ollama" => names.push("OLLAMA_API_KEY"),
        "gemini" | "google" | "google-gemini" => {
            names.push("GEMINI_API_KEY");
            names.push("GOOGLE_API_KEY");
        }
        "deepseek" => names.push("DEEPSEEK_API_KEY"),
        "xai" | "grok" => names.push("XAI_API_KEY"),
        "groq" => names.push("GROQ_API_KEY"),
        "mistral" => names.push("MISTRAL_API_KEY"),
        "cohere" => names.push("COHERE_API_KEY"),
        "perplexity" => names.push("PERPLEXITY_API_KEY"),
        "together" | "together-ai" => names.push("TOGETHER_API_KEY"),
        "fireworks" | "fireworks-ai" => names.push("FIREWORKS_API_KEY"),
        "novita" => names.push("NOVITA_API_KEY"),
        "telnyx" => names.push("TELNYX_API_KEY"),
        "venice" => names.push("VENICE_API_KEY"),
        "vercel" | "vercel-ai" => {
            names.push("VERCEL_API_KEY");
            names.push("AI_GATEWAY_API_KEY");
        }
        "nvidia" | "nvidia-nim" | "build.nvidia.com" => names.push("NVIDIA_API_KEY"),
        "synthetic" => names.push("SYNTHETIC_API_KEY"),
        "opencode" | "opencode-zen" => names.push("OPENCODE_API_KEY"),
        "opencode-go" => names.push("OPENCODE_GO_API_KEY"),
        "cloudflare" | "cloudflare-ai" => names.push("CLOUDFLARE_API_KEY"),
        "ovhcloud" | "ovh" => names.push("OVH_AI_ENDPOINTS_ACCESS_TOKEN"),
        "astrai" => names.push("ASTRAI_API_KEY"),
        "llamacpp" | "llama.cpp" => names.push("LLAMACPP_API_KEY"),
        "sglang" => names.push("SGLANG_API_KEY"),
        "vllm" => names.push("VLLM_API_KEY"),
        "aihubmix" => names.push("AIHUBMIX_API_KEY"),
        "siliconflow" | "silicon-flow" => names.push("SILICONFLOW_API_KEY"),
        "osaurus" => names.push("OSAURUS_API_KEY"),
        "azure" | "azure-openai" | "azure_openai" => names.push("AZURE_OPENAI_API_KEY"),
        _ => {}
    }

    if synapse_providers::is_moonshot_alias(normalized.as_str()) {
        names.push("MOONSHOT_API_KEY");
    }
    if matches!(
        normalized.as_str(),
        "kimi-code" | "kimi_coding" | "kimi_for_coding"
    ) {
        names.push("KIMI_CODE_API_KEY");
        names.push("MOONSHOT_API_KEY");
    }
    if synapse_providers::is_glm_alias(normalized.as_str()) {
        names.push("GLM_API_KEY");
    }
    if synapse_providers::is_zai_alias(normalized.as_str()) {
        names.push("ZAI_API_KEY");
    }
    if synapse_providers::is_minimax_alias(normalized.as_str()) {
        names.push("MINIMAX_OAUTH_TOKEN");
        names.push("MINIMAX_API_KEY");
        names.push("MINIMAX_OAUTH_REFRESH_TOKEN");
    }
    if synapse_providers::is_qianfan_alias(normalized.as_str()) {
        names.push("QIANFAN_API_KEY");
    }
    if synapse_providers::is_doubao_alias(normalized.as_str()) {
        names.push("ARK_API_KEY");
        names.push("VOLCENGINE_API_KEY");
        names.push("DOUBAO_API_KEY");
    }
    if synapse_providers::is_qwen_alias(normalized.as_str()) {
        names.push("DASHSCOPE_API_KEY");
        if synapse_providers::is_qwen_oauth_alias(normalized.as_str()) {
            names.push("QWEN_OAUTH_TOKEN");
            names.push("QWEN_OAUTH_REFRESH_TOKEN");
        }
    }
    if !matches!(
        normalized.as_str(),
        "openai-codex" | "openai_codex" | "codex" | "bedrock" | "aws-bedrock"
    ) {
        names.push("SYNAPSECLAW_API_KEY");
        names.push("API_KEY");
    }

    names.sort_unstable();
    names.dedup();
    names
}

fn special_provider_auth_status(
    config: &Config,
    provider: &str,
) -> Option<CapabilityDoctorProviderKeyStatus> {
    let normalized = provider.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "openai-codex" | "openai_codex" | "codex" => {
            if codex_cli_auth_file_present()
                || synapseclaw_auth_profile_present(config, "openai-codex")
            {
                Some(CapabilityDoctorProviderKeyStatus::Present)
            } else {
                Some(CapabilityDoctorProviderKeyStatus::Unknown)
            }
        }
        "gemini" | "google" | "google-gemini" => {
            if synapse_providers::gemini::GeminiProvider::has_cli_credentials()
                || synapseclaw_auth_profile_present(config, "gemini")
            {
                Some(CapabilityDoctorProviderKeyStatus::Present)
            } else {
                None
            }
        }
        "bedrock" | "aws-bedrock" => Some(CapabilityDoctorProviderKeyStatus::Unknown),
        _ => None,
    }
}

fn synapseclaw_auth_profile_present(config: &Config, provider: &str) -> bool {
    let path = synapse_providers::auth::state_dir_from_config(config).join("auth-profiles.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let active_profile_matches = value
        .get("active_profiles")
        .and_then(|active| active.get(provider))
        .and_then(serde_json::Value::as_str)
        .is_some();
    let stored_profile_matches = value
        .get("profiles")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|profiles| {
            profiles.values().any(|profile| {
                profile
                    .get("provider")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|profile_provider| profile_provider.eq_ignore_ascii_case(provider))
            })
        });
    active_profile_matches || stored_profile_matches
}

fn codex_cli_auth_file_present() -> bool {
    codex_cli_home()
        .map(|home| codex_cli_auth_file_has_tokens(home.join("auth.json").as_path()))
        .unwrap_or(false)
}

fn codex_cli_auth_file_has_tokens(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let auth_mode_matches = value
        .get("auth_mode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|mode| mode == "chatgpt");
    let has_token = value.get("tokens").is_some_and(|tokens| {
        tokens
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())
            || tokens
                .get("refresh_token")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|token| !token.trim().is_empty())
    });
    auth_mode_matches && has_token
}

fn codex_cli_home() -> Option<PathBuf> {
    match std::env::var("CODEX_HOME")
        .ok()
        .and_then(|value| first_nonempty_env_value(value.as_str()))
    {
        Some(configured) if configured == "~" => {
            directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
        }
        Some(configured) if configured.starts_with("~/") => directories::UserDirs::new()
            .map(|dirs| dirs.home_dir().join(configured.trim_start_matches("~/"))),
        Some(configured) => Some(PathBuf::from(configured)),
        None => directories::UserDirs::new().map(|dirs| dirs.home_dir().join(".codex")),
    }
}

fn first_nonempty_env_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn env_key_present(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn capability_doctor_severity_name(severity: CapabilityDoctorSeverity) -> &'static str {
    match severity {
        CapabilityDoctorSeverity::Ok => "ok",
        CapabilityDoctorSeverity::Warn => "warn",
        CapabilityDoctorSeverity::Error => "error",
    }
}

pub(crate) fn capability_doctor_subsystem_name(
    subsystem: CapabilityDoctorSubsystem,
) -> &'static str {
    match subsystem {
        CapabilityDoctorSubsystem::ProviderKey => "provider_key",
        CapabilityDoctorSubsystem::ProviderAdapter => "provider_adapter",
        CapabilityDoctorSubsystem::ProviderPlan => "provider_plan",
        CapabilityDoctorSubsystem::ModelProfile => "model_profile",
        CapabilityDoctorSubsystem::Route => "route",
        CapabilityDoctorSubsystem::Lane => "lane",
        CapabilityDoctorSubsystem::ToolRegistry => "tool_registry",
        CapabilityDoctorSubsystem::MemoryBackend => "memory_backend",
        CapabilityDoctorSubsystem::EmbeddingBackend => "embedding_backend",
        CapabilityDoctorSubsystem::ChannelDelivery => "channel_delivery",
        CapabilityDoctorSubsystem::ReasoningControls => "reasoning_controls",
        CapabilityDoctorSubsystem::NativeContinuation => "native_continuation",
    }
}

pub(crate) fn capability_doctor_readiness_name(
    readiness: CapabilityDoctorReadiness,
) -> &'static str {
    match readiness {
        CapabilityDoctorReadiness::Ready => "ready",
        CapabilityDoctorReadiness::MissingKey => "missing_key",
        CapabilityDoctorReadiness::MissingAdapter => "missing_adapter",
        CapabilityDoctorReadiness::MissingModelProfile => "missing_model_profile",
        CapabilityDoctorReadiness::UnknownContextWindow => "unknown_context_window",
        CapabilityDoctorReadiness::StaleCatalog => "stale_catalog",
        CapabilityDoctorReadiness::LowConfidenceMetadata => "low_confidence_metadata",
        CapabilityDoctorReadiness::UnsupportedModality => "unsupported_modality",
        CapabilityDoctorReadiness::UnsupportedToolCapability => "unsupported_tool_capability",
        CapabilityDoctorReadiness::IgnoredReasoningControls => "ignored_reasoning_controls",
        CapabilityDoctorReadiness::UnsupportedNativeContinuation => {
            "unsupported_native_continuation"
        }
        CapabilityDoctorReadiness::ProviderPlanDenied => "provider_plan_denied",
        CapabilityDoctorReadiness::DegradedBackend => "degraded_backend",
        CapabilityDoctorReadiness::NotConfigured => "not_configured",
        CapabilityDoctorReadiness::Unknown => "unknown",
    }
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
        ModelFeature::SpeechTranscription => "speech_transcription",
        ModelFeature::SpeechSynthesis => "speech_synthesis",
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
    use synapse_domain::application::services::runtime_decision_trace::{
        RuntimeDecisionTrace, RuntimeTraceAuxiliaryDecision, RuntimeTraceContextSnapshot,
        RuntimeTraceMemoryDecision, RuntimeTraceModelProfileSnapshot, RuntimeTraceRouteDecision,
        RuntimeTraceRouteRef, RuntimeTraceToolDecision,
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
            runtime_decision_traces: Vec::new(),
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
                runtime_decision_traces: Vec::new(),
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
                    required_lane: Some(CapabilityLane::MultimodalUnderstanding),
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
                runtime_decision_traces: Vec::new(),
            },
            &config,
        );

        assert!(
            response.contains("Last admission: `multimodal_understanding` / `warning` / `reroute`")
        );
        assert!(response.contains("Last admission required lane: `multimodal_understanding`"));
        assert!(response.contains(
            "Last admission reasons: requires multimodal_understanding, context warning"
        ));
        assert!(response.contains("Suggested next action: switch_lane (multimodal_understanding)"));
    }

    #[test]
    fn providers_help_includes_runtime_decision_trace_diagnostics() {
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
            runtime_decision_traces: vec![RuntimeDecisionTrace {
                trace_id: "trace-1".into(),
                observed_at_unix: 100,
                route: RuntimeTraceRouteDecision {
                    before: RuntimeTraceRouteRef {
                        provider: "openai-codex".into(),
                        model: "gpt-5.4".into(),
                        lane: None,
                        candidate_index: None,
                    },
                    after: RuntimeTraceRouteRef {
                        provider: "openai-codex".into(),
                        model: "gpt-5.4".into(),
                        lane: None,
                        candidate_index: None,
                    },
                    reroute_applied: false,
                    intent: "tool_heavy".into(),
                    pressure_state: "critical".into(),
                    action: "compact".into(),
                    reasons: vec!["context_critical".into()],
                    recommended_action: Some("compact_session".into()),
                },
                model_profile: RuntimeTraceModelProfileSnapshot {
                    context_window_tokens: Some(200_000),
                    max_output_tokens: Some(8_192),
                    features: vec!["tool_calling".into()],
                    context_window_source: "manual_config".into(),
                    context_window_freshness: "explicit".into(),
                    context_window_confidence: "high".into(),
                    max_output_source: "manual_config".into(),
                    max_output_freshness: "explicit".into(),
                    max_output_confidence: "high".into(),
                    features_source: "manual_config".into(),
                    features_freshness: "explicit".into(),
                    features_confidence: "high".into(),
                },
                context: RuntimeTraceContextSnapshot {
                    total_chars: 50_000,
                    estimated_total_tokens: 12_500,
                    target_total_tokens: 10_000,
                    ceiling_total_tokens: 12_000,
                    protected_chars: 1_000,
                    removable_chars: 49_000,
                    chars_over_target: 10_000,
                    chars_over_ceiling: 2_000,
                    tokens_headroom_to_target: 0,
                    tokens_headroom_to_ceiling: 0,
                    turn_shape: "baseline".into(),
                    budget_tier: "over_budget".into(),
                    requires_compaction: true,
                    condensation_mode: Some("summarize".into()),
                    condensation_target: Some("prior_chat".into()),
                    condensation_minimum_reclaim_chars: Some(5_000),
                    condensation_prefers_cached_artifact: true,
                    cache: None,
                },
                tools: vec![RuntimeTraceToolDecision {
                    observed_at_unix: 101,
                    tool_name: "web_fetch".into(),
                    tool_role: Some("external_lookup".into()),
                    failure_kind: "timeout".into(),
                    suggested_action: "retry_with_simpler_request".into(),
                    route: Some("openai/gpt-test lane=none candidate=none".into()),
                    attempt_reason: "model_tool_call".into(),
                    argument_shape: Some(
                        "root=object keys=url missing_required=none approx_chars=42".into(),
                    ),
                    admission: Some("action=proceed pressure=healthy reasons=".into()),
                    repair_outcome: "failed".into(),
                    expires_at_unix: 101 + 48 * 60 * 60,
                    repeat_count: 1,
                    suppression_key: Some("tool:web_fetch".into()),
                    detail: Some("timed out".into()),
                }],
                memory: vec![RuntimeTraceMemoryDecision {
                    observed_at_unix: 102,
                    source: "explicit_user".into(),
                    category: "core".into(),
                    write_class: Some("fact_anchor".into()),
                    action: "noop".into(),
                    applied: false,
                    entry_id_present: false,
                    reason: "secret memory text that must stay out of diagnostics".into(),
                    similarity_basis_points: Some(9_500),
                    failure: None,
                }],
                auxiliary: vec![RuntimeTraceAuxiliaryDecision {
                    observed_at_unix: 103,
                    kind: "reflection".into(),
                    action: "started".into(),
                    count: 1,
                    reason: None,
                    lane: None,
                    selected_provider: None,
                    selected_model: None,
                    selected_candidate_index: None,
                    candidate_order: Vec::new(),
                }],
                notes: Vec::new(),
            }],
        });

        assert!(response.contains("Runtime decision traces retained: 1"));
        assert!(response.contains("Runtime decision memory: explicit_user:core:noop applied=false"));
        assert!(response.contains("Runtime decision auxiliary: reflection:started count=1"));
        assert!(!response.contains("secret memory text"));
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
            required_lane: Some(CapabilityLane::ImageGeneration),
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
                runtime_decision_traces: Vec::new(),
            },
            &config,
        );

        assert!(response.contains("Recent admissions retained: 1"));
        assert!(response.contains("Recent admission required lane: `image_generation`"));
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
                runtime_decision_traces: Vec::new(),
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
                runtime_decision_traces: Vec::new(),
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
                    ..ToolRepairTrace::default()
                }),
                recent_tool_repairs: vec![ToolRepairTrace {
                    observed_at_unix: 1_744_243_200,
                    tool_name: "message_send".into(),
                    failure_kind: ToolFailureKind::ReportedFailure,
                    suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                    detail: Some("missing delivery target".into()),
                    ..ToolRepairTrace::default()
                }],
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
                runtime_decision_traces: Vec::new(),
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
                ..ToolRepairTrace::default()
            }),
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
            runtime_decision_traces: Vec::new(),
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
                    recent_repairs: Vec::new(),
                    assumptions: Vec::new(),
                },
            }],
            runtime_decision_traces: Vec::new(),
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
            runtime_decision_traces: Vec::new(),
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
            runtime_decision_traces: Vec::new(),
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
                runtime_decision_traces: Vec::new(),
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
                runtime_decision_traces: Vec::new(),
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
    fn capability_doctor_provider_key_env_names_follow_provider_auth_paths() {
        let codex = provider_api_key_env_names("openai-codex");
        assert!(codex.contains(&"SYNAPSECLAW_OPENAI_CODEX_ACCESS_TOKEN"));
        assert!(codex.contains(&"SYNAPSECLAW_OPENAI_CODEX_REFRESH_TOKEN"));
        assert!(!codex.contains(&"API_KEY"));

        let qwen_oauth = provider_api_key_env_names("qwen-code");
        assert!(qwen_oauth.contains(&"QWEN_OAUTH_TOKEN"));
        assert!(qwen_oauth.contains(&"QWEN_OAUTH_REFRESH_TOKEN"));
        assert!(qwen_oauth.contains(&"DASHSCOPE_API_KEY"));

        let bedrock = provider_api_key_env_names("bedrock");
        assert!(!bedrock.contains(&"API_KEY"));
        assert!(!bedrock.contains(&"SYNAPSECLAW_API_KEY"));
    }

    #[test]
    fn capability_doctor_does_not_treat_bedrock_config_api_key_as_auth() {
        let mut config = Config::default();
        config.api_key = Some("not-used-by-bedrock".into());

        assert!(!provider_uses_config_api_key(&config, "bedrock"));
    }

    #[test]
    fn capability_doctor_codex_cli_auth_requires_chatgpt_tokens() {
        let workspace = tempfile::tempdir().unwrap();
        let auth_path = workspace.path().join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{"auth_mode":"chatgpt","tokens":{"refresh_token":" rt "}}"#,
        )
        .unwrap();
        assert!(codex_cli_auth_file_has_tokens(auth_path.as_path()));

        std::fs::write(
            &auth_path,
            r#"{"auth_mode":"api-key","tokens":{"refresh_token":" rt "}}"#,
        )
        .unwrap();
        assert!(!codex_cli_auth_file_has_tokens(auth_path.as_path()));
    }

    #[test]
    fn capability_doctor_keeps_ollama_cloud_model_auth_checked() {
        let route = RouteSelection {
            provider: "ollama".into(),
            model: "qwen3:cloud".into(),
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
            runtime_decision_traces: Vec::new(),
        };

        assert!(local_provider_route_requires_key(&route));
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
    fn workspace_profile_catalog_does_not_let_empty_cache_shadow_bundled_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let cache_dir = workspace.path().join("state");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(
            cache_dir.join(MODEL_CACHE_FILE),
            r#"{
              "entries": [
                {
                  "provider": "deepseek",
                  "fetched_at_unix": 100,
                  "models": ["deepseek-reasoner"],
                  "profiles": [
                    {
                      "model": "deepseek-reasoner",
                      "features": []
                    }
                  ]
                }
              ]
            }"#,
        )
        .unwrap();
        let catalog = WorkspaceModelProfileCatalog::new(workspace.path());

        let profile = catalog
            .lookup_model_profile("deepseek", "deepseek-reasoner")
            .expect("bundled DeepSeek profile should fill empty cache profile");

        assert_eq!(profile.context_window_tokens, Some(128_000));
        assert_eq!(profile.max_output_tokens, Some(65_536));
        assert!(profile.features.contains(&ModelFeature::ToolCalling));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::BundledCatalog)
        );
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
    fn workspace_profile_catalog_records_endpoint_scoped_profile_observation() {
        let workspace = tempfile::tempdir().unwrap();
        let catalog = WorkspaceModelProfileCatalog::with_provider_endpoint(
            workspace.path(),
            Some("openai"),
            Some("https://api.deepseek.com/v1/"),
        );

        catalog
            .record_model_profile_observation(
                "openai",
                "deepseek-chat",
                ModelProfileObservation {
                    context_window_tokens: Some(256_000),
                    max_output_tokens: Some(32_000),
                    features: vec![ModelFeature::ToolCalling, ModelFeature::ToolCalling],
                },
            )
            .unwrap();

        let profile = catalog
            .lookup_model_profile("openai", "deepseek-chat")
            .expect("endpoint-scoped observation should resolve through inferred provider");

        assert_eq!(profile.context_window_tokens, Some(256_000));
        assert_eq!(profile.max_output_tokens, Some(32_000));
        assert_eq!(profile.features, vec![ModelFeature::ToolCalling]);
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::CachedProviderCatalog)
        );
        assert!(profile.observed_at_unix.is_some());
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
