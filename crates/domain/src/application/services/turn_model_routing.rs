use crate::application::services::model_capability_support::profile_supports_lane_confidently;
use crate::application::services::model_lane_resolution::{
    resolve_lane_candidates, ResolvedModelCandidate, ResolvedModelProfile,
};
use crate::application::services::turn_markup::{
    contains_image_attachment_marker, detect_generation_marker, StructuredGenerationMarker,
};
use crate::config::schema::{CapabilityLane, Config};
use crate::ports::model_profile_catalog::ModelProfileCatalogPort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnCapabilityRequirement {
    MultimodalUnderstanding,
    ImageGeneration,
    AudioGeneration,
    VideoGeneration,
    MusicGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRouteOverride {
    pub lane: CapabilityLane,
    pub provider: String,
    pub model: String,
    pub candidate_index: Option<usize>,
}

pub fn infer_turn_capability_requirement(user_message: &str) -> Option<TurnCapabilityRequirement> {
    if contains_image_attachment_marker(user_message) {
        return Some(TurnCapabilityRequirement::MultimodalUnderstanding);
    }

    match detect_generation_marker(user_message) {
        Some(StructuredGenerationMarker::Image) => Some(TurnCapabilityRequirement::ImageGeneration),
        Some(StructuredGenerationMarker::Audio) => Some(TurnCapabilityRequirement::AudioGeneration),
        Some(StructuredGenerationMarker::Video) => Some(TurnCapabilityRequirement::VideoGeneration),
        Some(StructuredGenerationMarker::Music) => Some(TurnCapabilityRequirement::MusicGeneration),
        None => None,
    }
}

pub fn resolve_turn_route_override(
    config: &Config,
    user_message: &str,
    current_provider: &str,
    current_model: &str,
    current_profile: &ResolvedModelProfile,
    current_supports_vision: bool,
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> Option<TurnRouteOverride> {
    let requirement = infer_turn_capability_requirement(user_message)?;
    let lane = requirement_lane(requirement);

    if current_route_supports_requirement(requirement, current_profile, current_supports_vision) {
        return None;
    }

    let candidates = resolve_lane_candidates(config, lane, catalog);
    let (selected, candidate_index) = select_lane_candidate(lane, &candidates)?;
    if selected.provider == current_provider && selected.model == current_model {
        return None;
    }

    Some(TurnRouteOverride {
        lane,
        provider: selected.provider.clone(),
        model: selected.model.clone(),
        candidate_index: Some(candidate_index),
    })
}

fn requirement_lane(requirement: TurnCapabilityRequirement) -> CapabilityLane {
    match requirement {
        TurnCapabilityRequirement::MultimodalUnderstanding => {
            CapabilityLane::MultimodalUnderstanding
        }
        TurnCapabilityRequirement::ImageGeneration => CapabilityLane::ImageGeneration,
        TurnCapabilityRequirement::AudioGeneration => CapabilityLane::AudioGeneration,
        TurnCapabilityRequirement::VideoGeneration => CapabilityLane::VideoGeneration,
        TurnCapabilityRequirement::MusicGeneration => CapabilityLane::MusicGeneration,
    }
}

fn current_route_supports_requirement(
    requirement: TurnCapabilityRequirement,
    current_profile: &ResolvedModelProfile,
    current_supports_vision: bool,
) -> bool {
    match requirement {
        TurnCapabilityRequirement::MultimodalUnderstanding => {
            current_supports_vision
                || profile_supports_lane_confidently(
                    current_profile,
                    CapabilityLane::MultimodalUnderstanding,
                )
        }
        TurnCapabilityRequirement::ImageGeneration => {
            profile_supports_lane_confidently(current_profile, CapabilityLane::ImageGeneration)
        }
        TurnCapabilityRequirement::AudioGeneration => {
            profile_supports_lane_confidently(current_profile, CapabilityLane::AudioGeneration)
        }
        TurnCapabilityRequirement::VideoGeneration => {
            profile_supports_lane_confidently(current_profile, CapabilityLane::VideoGeneration)
        }
        TurnCapabilityRequirement::MusicGeneration => {
            profile_supports_lane_confidently(current_profile, CapabilityLane::MusicGeneration)
        }
    }
}

fn select_lane_candidate<'a>(
    lane: CapabilityLane,
    candidates: &'a [ResolvedModelCandidate],
) -> Option<(&'a ResolvedModelCandidate, usize)> {
    candidates
        .iter()
        .enumerate()
        .find(|(_, candidate)| candidate_supports_lane(candidate, lane))
        .map(|(index, candidate)| (candidate, index))
}

fn candidate_supports_lane(candidate: &ResolvedModelCandidate, lane: CapabilityLane) -> bool {
    profile_supports_lane_confidently(&candidate.profile, lane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        Config, ModelCandidateProfileConfig, ModelFeature, ModelLaneCandidateConfig,
        ModelLaneConfig,
    };
    use crate::ports::model_profile_catalog::{
        CatalogModelProfile, CatalogModelProfileSource, ModelProfileCatalogPort,
    };

    struct StaleVisionCatalog;

    impl ModelProfileCatalogPort for StaleVisionCatalog {
        fn lookup_model_profile(&self, provider: &str, model: &str) -> Option<CatalogModelProfile> {
            if provider == "provider-b" && model == "vision-model" {
                return Some(CatalogModelProfile {
                    context_window_tokens: Some(128_000),
                    max_output_tokens: None,
                    features: vec![ModelFeature::Vision],
                    source: Some(CatalogModelProfileSource::CachedProviderCatalog),
                    observed_at_unix: Some(1),
                });
            }
            None
        }

        fn record_model_profile_observation(
            &self,
            _provider: &str,
            _model: &str,
            _observation: crate::ports::model_profile_catalog::ModelProfileObservation,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn no_override_for_plain_text_turn() {
        let config = Config::default();
        let override_result = resolve_turn_route_override(
            &config,
            "hello",
            "openrouter",
            "qwen",
            &ResolvedModelProfile::default(),
            false,
            None,
        );

        assert!(override_result.is_none());
    }

    #[test]
    fn no_override_when_current_route_already_supports_multimodal() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MultimodalUnderstanding,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "openai-codex".into(),
                model: "gpt-5.4".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: None,
                    max_output_tokens: None,
                    features: vec![ModelFeature::Vision],
                },
            }],
        });

        let override_result = resolve_turn_route_override(
            &config,
            "Describe this [IMAGE:/tmp/cat.png]",
            "openai-codex",
            "gpt-5.4",
            &ResolvedModelProfile::default(),
            true,
            None,
        );

        assert!(override_result.is_none());
    }

    #[test]
    fn multimodal_turn_prefers_candidate_with_matching_features() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MultimodalUnderstanding,
            candidates: vec![
                ModelLaneCandidateConfig {
                    provider: "provider-a".into(),
                    model: "plain-model".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig::default(),
                },
                ModelLaneCandidateConfig {
                    provider: "provider-b".into(),
                    model: "vision-model".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig {
                        context_window_tokens: None,
                        max_output_tokens: None,
                        features: vec![ModelFeature::Vision],
                    },
                },
            ],
        });

        let override_result = resolve_turn_route_override(
            &config,
            "Describe this [IMAGE:/tmp/cat.png]",
            "openrouter",
            "qwen/qwen3.6-plus",
            &ResolvedModelProfile::default(),
            false,
            None,
        )
        .expect("multimodal lane should override the non-vision route");

        assert_eq!(
            override_result,
            TurnRouteOverride {
                lane: CapabilityLane::MultimodalUnderstanding,
                provider: "provider-b".into(),
                model: "vision-model".into(),
                candidate_index: Some(1),
            }
        );
    }

    #[test]
    fn multimodal_turn_does_not_fallback_to_candidate_without_known_support() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MultimodalUnderstanding,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "provider-a".into(),
                model: "plain-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        });

        let override_result = resolve_turn_route_override(
            &config,
            "Describe this [IMAGE:/tmp/cat.png]",
            "openrouter",
            "qwen/qwen3.6-plus",
            &ResolvedModelProfile::default(),
            false,
            None,
        );

        assert!(override_result.is_none());
    }

    #[test]
    fn multimodal_turn_rejects_stale_cached_capability_metadata() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::MultimodalUnderstanding,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "provider-b".into(),
                model: "vision-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        });

        let override_result = resolve_turn_route_override(
            &config,
            "Describe this [IMAGE:/tmp/cat.png]",
            "openrouter",
            "qwen/qwen3.6-plus",
            &ResolvedModelProfile::default(),
            false,
            Some(&StaleVisionCatalog),
        );

        assert!(override_result.is_none());
    }

    #[test]
    fn infers_media_generation_requirements_from_structured_markers() {
        assert_eq!(
            infer_turn_capability_requirement("[GENERATE:VIDEO] short trailer"),
            Some(TurnCapabilityRequirement::VideoGeneration)
        );
        assert_eq!(
            infer_turn_capability_requirement("[GENERATE:MUSIC] menu theme"),
            Some(TurnCapabilityRequirement::MusicGeneration)
        );
        assert_eq!(
            infer_turn_capability_requirement("[GENERATE:IMAGE] album cover"),
            Some(TurnCapabilityRequirement::ImageGeneration)
        );
        assert_eq!(
            infer_turn_capability_requirement("[GENERATE:AUDIO] narration"),
            Some(TurnCapabilityRequirement::AudioGeneration)
        );
    }

    #[test]
    fn image_generation_turn_prefers_image_lane() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::ImageGeneration,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "provider-image".into(),
                model: "image-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: None,
                    max_output_tokens: None,
                    features: vec![ModelFeature::ImageGeneration],
                },
            }],
        });

        let override_result = resolve_turn_route_override(
            &config,
            "[GENERATE:IMAGE] red fox reading a book",
            "openrouter",
            "qwen/qwen3.6-plus",
            &ResolvedModelProfile::default(),
            false,
            None,
        )
        .expect("image generation should route to the image lane");

        assert_eq!(
            override_result,
            TurnRouteOverride {
                lane: CapabilityLane::ImageGeneration,
                provider: "provider-image".into(),
                model: "image-model".into(),
                candidate_index: Some(0),
            }
        );
    }

    #[test]
    fn image_generation_turn_stays_on_current_route_when_profile_already_supports_it() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::ImageGeneration,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "provider-image".into(),
                model: "image-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: None,
                    max_output_tokens: None,
                    features: vec![ModelFeature::ImageGeneration],
                },
            }],
        });

        let override_result = resolve_turn_route_override(
            &config,
            "[GENERATE:IMAGE] red fox reading a book",
            "openrouter",
            "current-image-model",
            &ResolvedModelProfile {
                features: vec![ModelFeature::ImageGeneration],
                features_source:
                    crate::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            },
            false,
            None,
        );

        assert!(override_result.is_none());
    }

    #[test]
    fn universal_reasoning_candidate_can_supply_media_generation_lanes() {
        let mut config = Config::default();
        config.model_lanes.push(ModelLaneConfig {
            lane: CapabilityLane::Reasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "provider-universal".into(),
                model: "universal-media-model".into(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: None,
                    max_output_tokens: None,
                    features: vec![
                        ModelFeature::ToolCalling,
                        ModelFeature::ImageGeneration,
                        ModelFeature::AudioGeneration,
                        ModelFeature::VideoGeneration,
                        ModelFeature::MusicGeneration,
                    ],
                },
            }],
        });

        let video_override = resolve_turn_route_override(
            &config,
            "[GENERATE:VIDEO] launch teaser",
            "provider-text",
            "plain-model",
            &ResolvedModelProfile::default(),
            false,
            None,
        )
        .expect("video should reuse the universal reasoning candidate");
        let music_override = resolve_turn_route_override(
            &config,
            "[GENERATE:MUSIC] title theme",
            "provider-text",
            "plain-model",
            &ResolvedModelProfile::default(),
            false,
            None,
        )
        .expect("music should reuse the universal reasoning candidate");

        assert_eq!(video_override.lane, CapabilityLane::VideoGeneration);
        assert_eq!(music_override.lane, CapabilityLane::MusicGeneration);
        assert_eq!(video_override.provider, "provider-universal");
        assert_eq!(music_override.provider, "provider-universal");
    }
}
