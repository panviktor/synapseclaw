use std::collections::HashSet;

use crate::application::services::model_capability_support::lane_required_feature;
use crate::config::model_catalog::{
    apply_default_api_key, known_model_presets as catalog_known_model_presets,
    model_profile as catalog_model_profile,
    normalize_model_preset_id as catalog_normalize_model_preset_id,
    preset_description as catalog_preset_description, preset_extra_lanes,
    preset_reasoning_seed as catalog_preset_reasoning_seed, preset_seed_multimodal_from_reasoning,
    preset_title as catalog_preset_title,
    recommended_preset_for_provider as catalog_recommended_preset_for_provider,
    route_aliases as catalog_route_aliases, KnownModelPreset,
};
use crate::config::schema::{
    CapabilityLane, Config, ModelCandidateProfileConfig, ModelFeature, ModelLaneCandidateConfig,
    ModelLaneConfig, ModelRouteConfig,
};

pub fn known_model_presets() -> &'static [KnownModelPreset] {
    catalog_known_model_presets()
}

pub fn recommended_model_preset_for_provider(provider: &str) -> Option<&'static str> {
    catalog_recommended_preset_for_provider(provider)
}

pub fn normalize_model_preset_id(value: &str) -> Option<&'static str> {
    catalog_normalize_model_preset_id(value)
}

pub fn preset_title(preset_id: &str) -> Option<&'static str> {
    let normalized = normalize_model_preset_id(preset_id)?;
    catalog_preset_title(normalized)
}

pub fn preset_description(preset_id: &str) -> Option<&'static str> {
    let normalized = normalize_model_preset_id(preset_id)?;
    catalog_preset_description(normalized)
}

pub fn preset_reasoning_seed(preset_id: &str) -> Option<(&'static str, &'static str)> {
    let normalized = normalize_model_preset_id(preset_id)?;
    catalog_preset_reasoning_seed(normalized)
}

pub fn resolve_effective_model_lanes(config: &Config) -> Vec<ModelLaneConfig> {
    let mut lanes = config
        .model_preset
        .as_deref()
        .and_then(|preset| build_preset_model_lanes(config, preset))
        .unwrap_or_default();

    for explicit in &config.model_lanes {
        if let Some(existing) = lanes.iter_mut().find(|lane| lane.lane == explicit.lane) {
            *existing = explicit.clone();
        } else {
            lanes.push(explicit.clone());
        }
    }

    lanes
}

pub fn provider_router_routes(config: &Config) -> Vec<ModelRouteConfig> {
    let mut routes = Vec::new();
    let mut seen_hints = HashSet::new();

    for lane in resolve_effective_model_lanes(config) {
        for (index, candidate) in lane.candidates.iter().enumerate() {
            if index == 0 {
                push_provider_router_route(
                    &mut routes,
                    &mut seen_hints,
                    ModelRouteConfig {
                        hint: lane.lane.as_str().to_string(),
                        capability: Some(lane.lane),
                        provider: candidate.provider.clone(),
                        model: candidate.model.clone(),
                        api_key: candidate.api_key.clone(),
                        profile: candidate.profile.clone(),
                    },
                );
            }
            push_provider_router_route(
                &mut routes,
                &mut seen_hints,
                ModelRouteConfig {
                    hint: provider_model_selector(&candidate.provider, &candidate.model),
                    capability: Some(lane.lane),
                    provider: candidate.provider.clone(),
                    model: candidate.model.clone(),
                    api_key: candidate.api_key.clone(),
                    profile: candidate.profile.clone(),
                },
            );
        }
    }

    for alias in catalog_route_aliases() {
        push_provider_router_route(&mut routes, &mut seen_hints, alias);
    }

    routes
}

fn push_provider_router_route(
    routes: &mut Vec<ModelRouteConfig>,
    seen_hints: &mut HashSet<String>,
    route: ModelRouteConfig,
) {
    let hint = route.hint.trim();
    if hint.is_empty() || !seen_hints.insert(hint.to_ascii_lowercase()) {
        return;
    }
    routes.push(route);
}

fn provider_model_selector(provider: &str, model: &str) -> String {
    format!("{}:{}", provider.trim(), model.trim())
}

fn build_preset_model_lanes(config: &Config, preset: &str) -> Option<Vec<ModelLaneConfig>> {
    let preset = normalize_model_preset_id(preset)?;
    let (default_reasoning_provider, default_reasoning_model) =
        catalog_preset_reasoning_seed(preset)?;
    let reasoning_provider = config
        .default_provider
        .clone()
        .unwrap_or_else(|| default_reasoning_provider.to_string());
    let reasoning_model = config
        .default_model
        .clone()
        .unwrap_or_else(|| default_reasoning_model.to_string());
    let uses_preset_reasoning_seed = config
        .default_provider
        .as_deref()
        .unwrap_or(default_reasoning_provider)
        == default_reasoning_provider
        && config
            .default_model
            .as_deref()
            .unwrap_or(default_reasoning_model)
            == default_reasoning_model;

    let reasoning_candidate = ModelLaneCandidateConfig {
        provider: reasoning_provider.clone(),
        model: reasoning_model.clone(),
        api_key: config.api_key.clone(),
        api_key_env: None,
        dimensions: None,
        profile: ModelCandidateProfileConfig::default(),
    };

    let mut lanes = vec![ModelLaneConfig {
        lane: CapabilityLane::Reasoning,
        candidates: vec![reasoning_candidate.clone()],
    }];

    if uses_preset_reasoning_seed && preset_seed_multimodal_from_reasoning(preset) {
        lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MultimodalUnderstanding,
            candidates: vec![ModelLaneCandidateConfig {
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: None,
                    max_output_tokens: None,
                    features: vec![
                        ModelFeature::ToolCalling,
                        ModelFeature::Vision,
                        ModelFeature::MultimodalUnderstanding,
                    ],
                },
                ..reasoning_candidate.clone()
            }],
        });
    }

    if let Some(mut extra_lanes) = preset_extra_lanes(preset) {
        apply_default_api_key(&mut extra_lanes, config.api_key.as_ref());
        lanes.extend(extra_lanes);
    }
    seed_declared_media_lanes_from_reasoning_profile(&mut lanes, &reasoning_candidate);

    Some(lanes)
}

fn seed_declared_media_lanes_from_reasoning_profile(
    lanes: &mut Vec<ModelLaneConfig>,
    reasoning_candidate: &ModelLaneCandidateConfig,
) {
    let Some(profile) =
        catalog_model_profile(&reasoning_candidate.provider, &reasoning_candidate.model)
    else {
        return;
    };
    seed_declared_media_lanes_from_profile(
        lanes,
        reasoning_candidate,
        ModelCandidateProfileConfig {
            context_window_tokens: profile.context_window_tokens,
            max_output_tokens: profile.max_output_tokens,
            features: profile.features,
        },
    );
}

fn seed_declared_media_lanes_from_profile(
    lanes: &mut Vec<ModelLaneConfig>,
    reasoning_candidate: &ModelLaneCandidateConfig,
    profile: ModelCandidateProfileConfig,
) {
    for lane in [
        CapabilityLane::MultimodalUnderstanding,
        CapabilityLane::ImageGeneration,
        CapabilityLane::AudioGeneration,
        CapabilityLane::VideoGeneration,
        CapabilityLane::MusicGeneration,
    ] {
        if lanes.iter().any(|existing| existing.lane == lane) {
            continue;
        }
        if !profile_supports_seeded_lane(&profile, lane) {
            continue;
        }
        lanes.push(ModelLaneConfig {
            lane,
            candidates: vec![ModelLaneCandidateConfig {
                profile: profile.clone(),
                ..reasoning_candidate.clone()
            }],
        });
    }
}

fn profile_supports_seeded_lane(
    profile: &ModelCandidateProfileConfig,
    lane: CapabilityLane,
) -> bool {
    match lane_required_feature(lane) {
        Some(ModelFeature::MultimodalUnderstanding) => {
            profile
                .features
                .contains(&ModelFeature::MultimodalUnderstanding)
                || profile.features.contains(&ModelFeature::Vision)
        }
        Some(feature) => profile.features.contains(&feature),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_preset_expands_into_reasoning_helper_and_media_lanes() {
        let mut config = Config::default();
        config.default_provider = Some("openrouter".into());
        config.default_model =
            crate::config::model_catalog::provider_default_model("openrouter").map(str::to_string);
        config.model_preset = Some("openrouter".into());

        let lanes = resolve_effective_model_lanes(&config);

        assert_eq!(lanes.len(), 7);
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::Reasoning));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::CheapReasoning));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::Embedding));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::ImageGeneration));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::AudioGeneration));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::MusicGeneration));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::VideoGeneration));
    }

    #[test]
    fn provider_router_routes_follow_effective_lanes_and_catalog_aliases() {
        let mut config = Config::default();
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::CheapReasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "test-provider".into(),
                model: "test-cheap-model".into(),
                api_key: Some("test-key".into()),
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        }];

        let routes = provider_router_routes(&config);

        let cheap = routes
            .iter()
            .find(|route| route.hint == "cheap_reasoning")
            .expect("cheap lane route should exist");
        assert_eq!(cheap.provider, "test-provider");
        assert_eq!(cheap.model, "test-cheap-model");
        assert_eq!(cheap.capability, Some(CapabilityLane::CheapReasoning));
        assert!(routes
            .iter()
            .any(|route| route.hint == "test-provider:test-cheap-model"));
        assert!(routes.iter().any(|route| route.hint == "qwen36"));
    }

    #[test]
    fn chatgpt_preset_seeds_multimodal_lane_when_using_default_reasoning_seed() {
        let mut config = Config::default();
        config.model_preset = Some("chatgpt".into());
        config.default_provider = Some("openai-codex".into());
        config.default_model = crate::config::model_catalog::provider_default_model("openai-codex")
            .map(str::to_string);

        let lanes = resolve_effective_model_lanes(&config);

        let multimodal = lanes
            .iter()
            .find(|lane| lane.lane == CapabilityLane::MultimodalUnderstanding)
            .expect("chatgpt preset should seed multimodal lane");
        assert_eq!(multimodal.candidates.len(), 1);
        assert!(multimodal.candidates[0]
            .profile
            .features
            .contains(&ModelFeature::Vision));
    }

    #[test]
    fn declared_reasoning_profile_features_seed_media_lanes() {
        let reasoning_candidate = ModelLaneCandidateConfig {
            provider: "test-provider".into(),
            model: "test-universal-model".into(),
            api_key: Some("test-key".into()),
            api_key_env: None,
            dimensions: None,
            profile: ModelCandidateProfileConfig::default(),
        };
        let mut lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![reasoning_candidate.clone()],
        }];

        seed_declared_media_lanes_from_profile(
            &mut lanes,
            &reasoning_candidate,
            ModelCandidateProfileConfig {
                context_window_tokens: Some(1_000_000),
                max_output_tokens: Some(64_000),
                features: vec![
                    ModelFeature::ToolCalling,
                    ModelFeature::Vision,
                    ModelFeature::ImageGeneration,
                    ModelFeature::VideoGeneration,
                    ModelFeature::MusicGeneration,
                ],
            },
        );

        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::MultimodalUnderstanding));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::ImageGeneration));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::VideoGeneration));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::MusicGeneration));
        assert!(!lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::AudioGeneration));
        let seeded = lanes
            .iter()
            .find(|lane| lane.lane == CapabilityLane::VideoGeneration)
            .expect("video lane should be seeded");
        assert_eq!(seeded.candidates[0].provider, "test-provider");
        assert_eq!(seeded.candidates[0].model, "test-universal-model");
        assert_eq!(seeded.candidates[0].api_key.as_deref(), Some("test-key"));
        assert_eq!(
            seeded.candidates[0].profile.context_window_tokens,
            Some(1_000_000)
        );
    }

    #[test]
    fn declared_reasoning_profile_features_do_not_override_existing_lanes() {
        let reasoning_candidate = ModelLaneCandidateConfig {
            provider: "test-provider".into(),
            model: "test-universal-model".into(),
            api_key: None,
            api_key_env: None,
            dimensions: None,
            profile: ModelCandidateProfileConfig::default(),
        };
        let mut lanes = vec![
            ModelLaneConfig {
                lane: CapabilityLane::Reasoning,
                candidates: vec![reasoning_candidate.clone()],
            },
            ModelLaneConfig {
                lane: CapabilityLane::ImageGeneration,
                candidates: vec![ModelLaneCandidateConfig {
                    provider: "explicit-provider".into(),
                    model: "explicit-image-model".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig {
                        context_window_tokens: Some(128_000),
                        max_output_tokens: None,
                        features: vec![ModelFeature::ImageGeneration],
                    },
                }],
            },
        ];

        seed_declared_media_lanes_from_profile(
            &mut lanes,
            &reasoning_candidate,
            ModelCandidateProfileConfig {
                context_window_tokens: Some(1_000_000),
                max_output_tokens: None,
                features: vec![ModelFeature::ImageGeneration, ModelFeature::AudioGeneration],
            },
        );

        let image = lanes
            .iter()
            .find(|lane| lane.lane == CapabilityLane::ImageGeneration)
            .expect("explicit image lane should remain");
        assert_eq!(image.candidates[0].provider, "explicit-provider");
        assert_eq!(
            lanes
                .iter()
                .filter(|lane| lane.lane == CapabilityLane::ImageGeneration)
                .count(),
            1
        );
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::AudioGeneration));
    }

    #[test]
    fn explicit_lane_overrides_preset_lane() {
        let mut config = Config::default();
        config.model_preset = Some("openrouter".into());
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::CheapReasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "anthropic".into(),
                model: "claude-haiku-4-5-20251001".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        }];

        let lanes = resolve_effective_model_lanes(&config);
        let cheap = lanes
            .iter()
            .find(|lane| lane.lane == CapabilityLane::CheapReasoning)
            .unwrap();

        assert_eq!(cheap.candidates.len(), 1);
        assert_eq!(cheap.candidates[0].provider, "anthropic");
    }

    #[test]
    fn provider_recommendations_match_expected_presets() {
        assert_eq!(
            recommended_model_preset_for_provider("openai-codex"),
            Some("chatgpt")
        );
        assert_eq!(
            recommended_model_preset_for_provider("anthropic"),
            Some("claude")
        );
        assert_eq!(
            recommended_model_preset_for_provider("openrouter"),
            Some("openrouter")
        );
        assert_eq!(
            recommended_model_preset_for_provider("ollama"),
            Some("local")
        );
    }
}
