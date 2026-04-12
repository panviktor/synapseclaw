use crate::application::services::model_capability_support::profile_supports_lane_confidently;
use crate::application::services::model_preset_resolution::resolve_effective_model_lanes;
use crate::config::schema::{
    CapabilityLane, Config, EmbeddingRouteConfig, ModelCandidateProfileConfig, ModelFeature,
    ModelRouteConfig,
};
use crate::ports::model_profile_catalog::{
    CatalogModelProfile, CatalogModelProfileSource, ModelProfileCatalogPort,
};
use crate::ports::route_selection::RouteSelection;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLaneResolutionSource {
    ExplicitLaneConfig,
    ImplicitReasoningLane,
    LegacyModelRoute,
    LegacyEmbeddingRoute,
    DefaultRoute,
}

pub fn model_lane_resolution_source_name(source: ModelLaneResolutionSource) -> &'static str {
    match source {
        ModelLaneResolutionSource::ExplicitLaneConfig => "explicit_lane_config",
        ModelLaneResolutionSource::ImplicitReasoningLane => "implicit_reasoning_lane",
        ModelLaneResolutionSource::LegacyModelRoute => "legacy_model_route",
        ModelLaneResolutionSource::LegacyEmbeddingRoute => "legacy_embedding_route",
        ModelLaneResolutionSource::DefaultRoute => "default_route",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedModelProfile {
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub features: Vec<ModelFeature>,
    pub context_window_source: ResolvedModelProfileSource,
    pub max_output_source: ResolvedModelProfileSource,
    pub features_source: ResolvedModelProfileSource,
    pub observed_at_unix: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResolvedModelProfileSource {
    #[default]
    Unknown,
    ManualConfig,
    CachedProviderCatalog,
    BundledCatalog,
    LocalOverrideCatalog,
    AdapterFallback,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResolvedModelProfileFreshness {
    #[default]
    Unknown,
    Explicit,
    Curated,
    Fresh,
    Aging,
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolvedModelProfileConfidence {
    #[default]
    Unknown,
    Low,
    Medium,
    High,
}

pub fn resolved_model_profile_source_name(source: ResolvedModelProfileSource) -> &'static str {
    match source {
        ResolvedModelProfileSource::Unknown => "unknown",
        ResolvedModelProfileSource::ManualConfig => "manual_config",
        ResolvedModelProfileSource::CachedProviderCatalog => "cached_provider_catalog",
        ResolvedModelProfileSource::BundledCatalog => "bundled_catalog",
        ResolvedModelProfileSource::LocalOverrideCatalog => "local_override_catalog",
        ResolvedModelProfileSource::AdapterFallback => "adapter_fallback",
    }
}

pub fn resolved_model_profile_freshness_name(
    freshness: ResolvedModelProfileFreshness,
) -> &'static str {
    match freshness {
        ResolvedModelProfileFreshness::Unknown => "unknown",
        ResolvedModelProfileFreshness::Explicit => "explicit",
        ResolvedModelProfileFreshness::Curated => "curated",
        ResolvedModelProfileFreshness::Fresh => "fresh",
        ResolvedModelProfileFreshness::Aging => "aging",
        ResolvedModelProfileFreshness::Stale => "stale",
    }
}

pub fn resolved_model_profile_confidence_name(
    confidence: ResolvedModelProfileConfidence,
) -> &'static str {
    match confidence {
        ResolvedModelProfileConfidence::Unknown => "unknown",
        ResolvedModelProfileConfidence::Low => "low",
        ResolvedModelProfileConfidence::Medium => "medium",
        ResolvedModelProfileConfidence::High => "high",
    }
}

impl ResolvedModelProfile {
    pub fn context_window_known(&self) -> bool {
        !matches!(
            self.context_window_source,
            ResolvedModelProfileSource::Unknown
        )
    }

    pub fn max_output_known(&self) -> bool {
        !matches!(self.max_output_source, ResolvedModelProfileSource::Unknown)
    }

    pub fn features_known(&self) -> bool {
        !matches!(self.features_source, ResolvedModelProfileSource::Unknown)
    }

    pub fn context_window_freshness(&self) -> ResolvedModelProfileFreshness {
        classify_profile_field_freshness(self.context_window_source, self.observed_at_unix)
    }

    pub fn max_output_freshness(&self) -> ResolvedModelProfileFreshness {
        classify_profile_field_freshness(self.max_output_source, self.observed_at_unix)
    }

    pub fn features_freshness(&self) -> ResolvedModelProfileFreshness {
        classify_profile_field_freshness(self.features_source, self.observed_at_unix)
    }

    pub fn context_window_confidence(&self) -> ResolvedModelProfileConfidence {
        classify_profile_field_confidence(
            self.context_window_source,
            self.context_window_freshness(),
        )
    }

    pub fn max_output_confidence(&self) -> ResolvedModelProfileConfidence {
        classify_profile_field_confidence(self.max_output_source, self.max_output_freshness())
    }

    pub fn features_confidence(&self) -> ResolvedModelProfileConfidence {
        classify_profile_field_confidence(self.features_source, self.features_freshness())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelCandidate {
    pub source: ModelLaneResolutionSource,
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub dimensions: Option<usize>,
    pub profile: ResolvedModelProfile,
}

pub fn resolve_lane_candidates(
    config: &Config,
    lane: CapabilityLane,
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> Vec<ResolvedModelCandidate> {
    let effective_lanes = resolve_effective_model_lanes(config);

    if let Some(explicit) = effective_lanes.iter().find(|entry| entry.lane == lane) {
        if !explicit.candidates.is_empty() {
            return explicit
                .candidates
                .iter()
                .map(|candidate| {
                    resolve_candidate(
                        ModelLaneResolutionSource::ExplicitLaneConfig,
                        candidate.provider.as_str(),
                        candidate.model.as_str(),
                        candidate.api_key.clone(),
                        candidate.api_key_env.clone(),
                        candidate.dimensions,
                        &candidate.profile,
                        catalog,
                    )
                })
                .collect();
        }
    }

    let legacy_model_routes = config
        .model_routes
        .iter()
        .filter(|route| route.capability == Some(lane) || legacy_route_matches_lane(route, lane))
        .map(|route| {
            resolve_candidate(
                ModelLaneResolutionSource::LegacyModelRoute,
                route.provider.as_str(),
                route.model.as_str(),
                route.api_key.clone(),
                None,
                None,
                &route.profile,
                catalog,
            )
        });

    let legacy_embedding_routes = config
        .embedding_routes
        .iter()
        .filter(|route| {
            route.capability == Some(lane) || legacy_embedding_route_matches_lane(route, lane)
        })
        .map(|route| {
            resolve_candidate(
                ModelLaneResolutionSource::LegacyEmbeddingRoute,
                route.provider.as_str(),
                route.model.as_str(),
                route.api_key.clone(),
                None,
                route.dimensions,
                &route.profile,
                catalog,
            )
        });

    let mut resolved = legacy_model_routes
        .chain(legacy_embedding_routes)
        .collect::<Vec<_>>();

    if resolved.is_empty() && lane_allows_implicit_reasoning_candidates(lane) {
        resolved = resolve_lane_candidates(config, CapabilityLane::Reasoning, catalog)
            .into_iter()
            .filter(|candidate| profile_supports_lane_confidently(&candidate.profile, lane))
            .map(|mut candidate| {
                candidate.source = ModelLaneResolutionSource::ImplicitReasoningLane;
                candidate
            })
            .collect();
    }

    if resolved.is_empty() && lane == CapabilityLane::Reasoning {
        if let Some(default_model) = config.default_model.clone() {
            resolved.push(resolve_candidate(
                ModelLaneResolutionSource::DefaultRoute,
                config.default_provider.as_deref().unwrap_or("openrouter"),
                &default_model,
                config.api_key.clone(),
                None,
                None,
                &ModelCandidateProfileConfig::default(),
                catalog,
            ));
        }
    }

    resolved
}

pub fn resolve_candidate_profile(
    provider: &str,
    model: &str,
    manual: &ModelCandidateProfileConfig,
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> ResolvedModelProfile {
    let auto = catalog.and_then(|catalog| catalog.lookup_model_profile(provider, model));
    merge_profile(manual, auto)
}

pub fn resolve_route_selection_profile(
    config: &Config,
    route: &RouteSelection,
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> ResolvedModelProfile {
    if let Some(lane) = route.lane {
        let candidates = resolve_lane_candidates(config, lane, catalog);
        if let Some(index) = route.candidate_index {
            if let Some(candidate) = candidates.get(index) {
                if candidate.provider == route.provider && candidate.model == route.model {
                    return candidate.profile.clone();
                }
            }
        }
        if let Some(candidate) = candidates.iter().find(|candidate| {
            candidate.provider == route.provider && candidate.model == route.model
        }) {
            return candidate.profile.clone();
        }
    }

    if let Some(route_match) = config
        .model_routes
        .iter()
        .find(|candidate| candidate.provider == route.provider && candidate.model == route.model)
    {
        return resolve_candidate_profile(
            route_match.provider.as_str(),
            route_match.model.as_str(),
            &route_match.profile,
            catalog,
        );
    }

    if let Some(route_match) = config
        .embedding_routes
        .iter()
        .find(|candidate| candidate.provider == route.provider && candidate.model == route.model)
    {
        return resolve_candidate_profile(
            route_match.provider.as_str(),
            route_match.model.as_str(),
            &route_match.profile,
            catalog,
        );
    }

    resolve_candidate_profile(
        route.provider.as_str(),
        route.model.as_str(),
        &ModelCandidateProfileConfig::default(),
        catalog,
    )
}

fn resolve_candidate(
    source: ModelLaneResolutionSource,
    provider: &str,
    model: &str,
    api_key: Option<String>,
    api_key_env: Option<String>,
    dimensions: Option<usize>,
    manual: &ModelCandidateProfileConfig,
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> ResolvedModelCandidate {
    ResolvedModelCandidate {
        source,
        provider: provider.to_string(),
        model: model.to_string(),
        api_key,
        api_key_env,
        dimensions,
        profile: resolve_candidate_profile(provider, model, manual, catalog),
    }
}

fn merge_profile(
    manual: &ModelCandidateProfileConfig,
    auto: Option<CatalogModelProfile>,
) -> ResolvedModelProfile {
    let auto = auto.unwrap_or_default();
    let auto_features = auto.features.clone();
    let auto_observed_at_unix = auto.observed_at_unix;
    let auto_source = auto
        .source
        .map(map_catalog_source)
        .unwrap_or(ResolvedModelProfileSource::Unknown);

    let context_window_source = if manual.context_window_tokens.is_some() {
        ResolvedModelProfileSource::ManualConfig
    } else if auto.context_window_tokens.is_some() {
        auto_source
    } else {
        ResolvedModelProfileSource::Unknown
    };

    let max_output_source = if manual.max_output_tokens.is_some() {
        ResolvedModelProfileSource::ManualConfig
    } else if auto.max_output_tokens.is_some() {
        auto_source
    } else {
        ResolvedModelProfileSource::Unknown
    };

    let features_source = if !manual.features.is_empty() {
        ResolvedModelProfileSource::ManualConfig
    } else if !auto.features.is_empty() {
        auto_source
    } else {
        ResolvedModelProfileSource::Unknown
    };

    ResolvedModelProfile {
        context_window_tokens: manual.context_window_tokens.or(auto.context_window_tokens),
        max_output_tokens: manual.max_output_tokens.or(auto.max_output_tokens),
        features: if manual.features.is_empty() {
            auto_features
        } else {
            manual.features.clone()
        },
        context_window_source,
        max_output_source,
        features_source,
        observed_at_unix: if matches!(
            auto_source,
            ResolvedModelProfileSource::CachedProviderCatalog
                | ResolvedModelProfileSource::BundledCatalog
                | ResolvedModelProfileSource::LocalOverrideCatalog
                | ResolvedModelProfileSource::AdapterFallback
        ) {
            auto_observed_at_unix
        } else {
            None
        },
    }
}

fn map_catalog_source(source: CatalogModelProfileSource) -> ResolvedModelProfileSource {
    match source {
        CatalogModelProfileSource::CachedProviderCatalog => {
            ResolvedModelProfileSource::CachedProviderCatalog
        }
        CatalogModelProfileSource::BundledCatalog => ResolvedModelProfileSource::BundledCatalog,
        CatalogModelProfileSource::LocalOverrideCatalog => {
            ResolvedModelProfileSource::LocalOverrideCatalog
        }
        CatalogModelProfileSource::AdapterFallback => ResolvedModelProfileSource::AdapterFallback,
    }
}

fn classify_profile_field_freshness(
    source: ResolvedModelProfileSource,
    observed_at_unix: Option<u64>,
) -> ResolvedModelProfileFreshness {
    match source {
        ResolvedModelProfileSource::Unknown => ResolvedModelProfileFreshness::Unknown,
        ResolvedModelProfileSource::ManualConfig
        | ResolvedModelProfileSource::LocalOverrideCatalog => {
            ResolvedModelProfileFreshness::Explicit
        }
        ResolvedModelProfileSource::BundledCatalog => ResolvedModelProfileFreshness::Curated,
        ResolvedModelProfileSource::CachedProviderCatalog
        | ResolvedModelProfileSource::AdapterFallback => {
            let Some(observed_at_unix) = observed_at_unix else {
                return ResolvedModelProfileFreshness::Unknown;
            };
            let Some(now_unix) = current_unix_time() else {
                return ResolvedModelProfileFreshness::Unknown;
            };
            let age_secs = now_unix.saturating_sub(observed_at_unix);
            if age_secs <= 7 * 24 * 60 * 60 {
                ResolvedModelProfileFreshness::Fresh
            } else if age_secs <= 30 * 24 * 60 * 60 {
                ResolvedModelProfileFreshness::Aging
            } else {
                ResolvedModelProfileFreshness::Stale
            }
        }
    }
}

fn classify_profile_field_confidence(
    source: ResolvedModelProfileSource,
    freshness: ResolvedModelProfileFreshness,
) -> ResolvedModelProfileConfidence {
    match source {
        ResolvedModelProfileSource::Unknown => ResolvedModelProfileConfidence::Unknown,
        ResolvedModelProfileSource::ManualConfig
        | ResolvedModelProfileSource::LocalOverrideCatalog => ResolvedModelProfileConfidence::High,
        ResolvedModelProfileSource::BundledCatalog => ResolvedModelProfileConfidence::Medium,
        ResolvedModelProfileSource::CachedProviderCatalog => match freshness {
            ResolvedModelProfileFreshness::Fresh => ResolvedModelProfileConfidence::High,
            ResolvedModelProfileFreshness::Aging => ResolvedModelProfileConfidence::Medium,
            ResolvedModelProfileFreshness::Stale | ResolvedModelProfileFreshness::Unknown => {
                ResolvedModelProfileConfidence::Low
            }
            ResolvedModelProfileFreshness::Explicit | ResolvedModelProfileFreshness::Curated => {
                ResolvedModelProfileConfidence::Medium
            }
        },
        ResolvedModelProfileSource::AdapterFallback => match freshness {
            ResolvedModelProfileFreshness::Fresh => ResolvedModelProfileConfidence::Medium,
            ResolvedModelProfileFreshness::Aging
            | ResolvedModelProfileFreshness::Stale
            | ResolvedModelProfileFreshness::Unknown => ResolvedModelProfileConfidence::Low,
            ResolvedModelProfileFreshness::Explicit | ResolvedModelProfileFreshness::Curated => {
                ResolvedModelProfileConfidence::Medium
            }
        },
    }
}

fn current_unix_time() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn legacy_route_matches_lane(route: &ModelRouteConfig, lane: CapabilityLane) -> bool {
    matches!(
        (lane, route.hint.as_str()),
        (CapabilityLane::CheapReasoning, "cheap")
            | (CapabilityLane::Reasoning, "reasoning")
            | (CapabilityLane::Reasoning, "main")
    )
}

fn legacy_embedding_route_matches_lane(route: &EmbeddingRouteConfig, lane: CapabilityLane) -> bool {
    matches!(
        (lane, route.hint.as_str()),
        (CapabilityLane::Embedding, "semantic")
    )
}

fn lane_allows_implicit_reasoning_candidates(lane: CapabilityLane) -> bool {
    matches!(
        lane,
        CapabilityLane::MultimodalUnderstanding
            | CapabilityLane::ImageGeneration
            | CapabilityLane::AudioGeneration
            | CapabilityLane::VideoGeneration
            | CapabilityLane::MusicGeneration
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        Config, ModelCandidateProfileConfig, ModelFeature, ModelLaneCandidateConfig,
        ModelLaneConfig, SummaryConfig,
    };

    struct StubCatalog;

    impl ModelProfileCatalogPort for StubCatalog {
        fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile> {
            if provider == "openrouter" && model == "qwen/qwen3.6-plus" {
                return Some(CatalogModelProfile {
                    context_window_tokens: Some(200_000),
                    max_output_tokens: Some(32_000),
                    features: vec![ModelFeature::ToolCalling],
                    source: Some(CatalogModelProfileSource::CachedProviderCatalog),
                    observed_at_unix: Some(1_712_345_678),
                });
            }
            None
        }
    }

    fn base_config() -> Config {
        let mut config = Config::default();
        config.default_provider = Some("openai-codex".into());
        config.default_model = Some("gpt-5.4".into());
        config.summary = SummaryConfig::default();
        config
    }

    #[test]
    fn explicit_lane_candidates_win_over_legacy_routes() {
        let mut config = base_config();
        config.model_routes.push(ModelRouteConfig {
            hint: "cheap".into(),
            capability: Some(CapabilityLane::CheapReasoning),
            provider: "openrouter".into(),
            model: "legacy-qwen".into(),
            api_key: None,
            profile: ModelCandidateProfileConfig::default(),
        });
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::CheapReasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "qwen/qwen3.6-plus".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        });

        let resolved =
            resolve_lane_candidates(&config, CapabilityLane::CheapReasoning, Some(&StubCatalog));

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].source,
            ModelLaneResolutionSource::ExplicitLaneConfig
        );
        assert_eq!(resolved[0].profile.context_window_tokens, Some(200_000));
    }

    #[test]
    fn manual_profile_overrides_auto_catalog() {
        let manual = ModelCandidateProfileConfig {
            context_window_tokens: Some(64_000),
            max_output_tokens: None,
            features: vec![ModelFeature::Vision],
        };

        let resolved = resolve_candidate_profile(
            "openrouter",
            "qwen/qwen3.6-plus",
            &manual,
            Some(&StubCatalog),
        );

        assert_eq!(resolved.context_window_tokens, Some(64_000));
        assert_eq!(resolved.max_output_tokens, Some(32_000));
        assert_eq!(resolved.features, vec![ModelFeature::Vision]);
        assert_eq!(
            resolved.context_window_source,
            ResolvedModelProfileSource::ManualConfig
        );
        assert_eq!(
            resolved.max_output_source,
            ResolvedModelProfileSource::CachedProviderCatalog
        );
        assert_eq!(
            resolved.features_source,
            ResolvedModelProfileSource::ManualConfig
        );
    }

    #[test]
    fn falls_back_to_default_reasoning_route() {
        let config = base_config();
        let resolved = resolve_lane_candidates(&config, CapabilityLane::Reasoning, None);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].source, ModelLaneResolutionSource::DefaultRoute);
        assert_eq!(resolved[0].provider, "openai-codex");
        assert_eq!(resolved[0].model, "gpt-5.4");
    }

    #[test]
    fn route_selection_profile_prefers_lane_candidate_metadata() {
        let mut config = base_config();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::CheapReasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "qwen/qwen3.6-plus".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        });

        let route = RouteSelection {
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            lane: Some(CapabilityLane::CheapReasoning),
            candidate_index: Some(0),
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
        };

        let resolved = resolve_route_selection_profile(&config, &route, Some(&StubCatalog));
        assert_eq!(resolved.context_window_tokens, Some(200_000));
        assert_eq!(
            resolved.context_window_source,
            ResolvedModelProfileSource::CachedProviderCatalog
        );
        assert_eq!(resolved.observed_at_unix, Some(1_712_345_678));
    }

    #[test]
    fn manual_profile_metadata_is_high_confidence_and_explicit() {
        let resolved = ResolvedModelProfile {
            context_window_tokens: Some(64_000),
            context_window_source: ResolvedModelProfileSource::ManualConfig,
            features: vec![ModelFeature::Vision],
            features_source: ResolvedModelProfileSource::ManualConfig,
            ..Default::default()
        };

        assert_eq!(
            resolved.context_window_freshness(),
            ResolvedModelProfileFreshness::Explicit
        );
        assert_eq!(
            resolved.context_window_confidence(),
            ResolvedModelProfileConfidence::High
        );
        assert_eq!(
            resolved.features_confidence(),
            ResolvedModelProfileConfidence::High
        );
    }

    #[test]
    fn stale_cached_catalog_metadata_downgrades_confidence() {
        let stale_observed_at = 1;
        let resolved = ResolvedModelProfile {
            context_window_tokens: Some(200_000),
            context_window_source: ResolvedModelProfileSource::CachedProviderCatalog,
            features: vec![ModelFeature::Vision],
            features_source: ResolvedModelProfileSource::CachedProviderCatalog,
            observed_at_unix: Some(stale_observed_at),
            ..Default::default()
        };

        assert_eq!(
            resolved.features_confidence(),
            ResolvedModelProfileConfidence::Low
        );
    }

    #[test]
    fn specialized_lane_can_implicitly_reuse_reasoning_candidate() {
        let mut config = base_config();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "universal-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(256_000),
                    max_output_tokens: Some(16_000),
                    features: vec![
                        ModelFeature::ToolCalling,
                        ModelFeature::Vision,
                        ModelFeature::ImageGeneration,
                        ModelFeature::AudioGeneration,
                    ],
                },
            }],
        });

        let resolved = resolve_lane_candidates(&config, CapabilityLane::ImageGeneration, None);

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].source,
            ModelLaneResolutionSource::ImplicitReasoningLane
        );
        assert_eq!(resolved[0].provider, "openrouter");
        assert_eq!(resolved[0].model, "universal-model");
    }

    #[test]
    fn implicit_reasoning_candidate_requires_confident_capability_support() {
        let mut config = base_config();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openrouter".into(),
                model: "plain-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        });

        let resolved = resolve_lane_candidates(&config, CapabilityLane::ImageGeneration, None);

        assert!(resolved.is_empty());
    }
}
