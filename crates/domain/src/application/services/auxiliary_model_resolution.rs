use std::fmt;

use crate::application::services::model_capability_support::{
    assess_lane_capability_support, LaneCapabilitySupport,
};
use crate::application::services::model_lane_resolution::{
    model_lane_resolution_source_name, resolve_explicit_lane_candidates,
    resolved_model_profile_confidence_name, resolved_model_profile_freshness_name,
    resolved_model_profile_source_name, ResolvedModelCandidate,
};
use crate::config::schema::{CapabilityLane, Config, ModelFeature};
use crate::ports::model_profile_catalog::ModelProfileCatalogPort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxiliaryLane {
    Compaction,
    Embedding,
    MultimodalUnderstanding,
    ImageGeneration,
    AudioGeneration,
    VideoGeneration,
    MusicGeneration,
    SpeechTranscription,
    SpeechSynthesis,
    WebExtraction,
    ToolValidator,
    CheapReasoning,
}

impl AuxiliaryLane {
    pub fn as_str(self) -> &'static str {
        match self {
            AuxiliaryLane::Compaction => "compaction",
            AuxiliaryLane::Embedding => "embedding",
            AuxiliaryLane::MultimodalUnderstanding => "multimodal_understanding",
            AuxiliaryLane::ImageGeneration => "image_generation",
            AuxiliaryLane::AudioGeneration => "audio_generation",
            AuxiliaryLane::VideoGeneration => "video_generation",
            AuxiliaryLane::MusicGeneration => "music_generation",
            AuxiliaryLane::SpeechTranscription => "speech_transcription",
            AuxiliaryLane::SpeechSynthesis => "speech_synthesis",
            AuxiliaryLane::WebExtraction => "web_extraction",
            AuxiliaryLane::ToolValidator => "tool_validator",
            AuxiliaryLane::CheapReasoning => "cheap_reasoning",
        }
    }

    pub fn capability_lane(self) -> CapabilityLane {
        match self {
            AuxiliaryLane::Compaction => CapabilityLane::Compaction,
            AuxiliaryLane::Embedding => CapabilityLane::Embedding,
            AuxiliaryLane::MultimodalUnderstanding => CapabilityLane::MultimodalUnderstanding,
            AuxiliaryLane::ImageGeneration => CapabilityLane::ImageGeneration,
            AuxiliaryLane::AudioGeneration => CapabilityLane::AudioGeneration,
            AuxiliaryLane::VideoGeneration => CapabilityLane::VideoGeneration,
            AuxiliaryLane::MusicGeneration => CapabilityLane::MusicGeneration,
            AuxiliaryLane::SpeechTranscription => CapabilityLane::SpeechTranscription,
            AuxiliaryLane::SpeechSynthesis => CapabilityLane::SpeechSynthesis,
            AuxiliaryLane::WebExtraction => CapabilityLane::WebExtraction,
            AuxiliaryLane::ToolValidator => CapabilityLane::ToolValidator,
            AuxiliaryLane::CheapReasoning => CapabilityLane::CheapReasoning,
        }
    }
}

impl fmt::Display for AuxiliaryLane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuxiliaryCandidateSkipReason {
    MissingCapabilityMetadata,
    StaleCapabilityMetadata,
    LowConfidenceCapabilityMetadata,
    MissingFeature(ModelFeature),
}

impl AuxiliaryCandidateSkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuxiliaryCandidateSkipReason::MissingCapabilityMetadata => {
                "missing_capability_metadata"
            }
            AuxiliaryCandidateSkipReason::StaleCapabilityMetadata => "stale_capability_metadata",
            AuxiliaryCandidateSkipReason::LowConfidenceCapabilityMetadata => {
                "low_confidence_capability_metadata"
            }
            AuxiliaryCandidateSkipReason::MissingFeature(_) => "missing_feature",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuxiliaryModelResolutionError {
    LaneNotConfigured {
        lane: AuxiliaryLane,
    },
    NoSupportedCandidate {
        lane: AuxiliaryLane,
        candidates: Vec<AuxiliaryModelCandidateDecision>,
    },
}

impl fmt::Display for AuxiliaryModelResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuxiliaryModelResolutionError::LaneNotConfigured { lane } => {
                write!(f, "auxiliary model lane '{lane}' is not configured")
            }
            AuxiliaryModelResolutionError::NoSupportedCandidate { lane, .. } => {
                write!(
                    f,
                    "auxiliary model lane '{lane}' has no supported candidate"
                )
            }
        }
    }
}

impl std::error::Error for AuxiliaryModelResolutionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxiliaryModelCandidateDecision {
    pub index: usize,
    pub provider: String,
    pub model: String,
    pub source: String,
    pub selected: bool,
    pub skip_reason: Option<AuxiliaryCandidateSkipReason>,
    pub profile_context_window_source: String,
    pub profile_context_window_freshness: String,
    pub profile_context_window_confidence: String,
    pub profile_features_source: String,
    pub profile_features_freshness: String,
    pub profile_features_confidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxiliaryModelSupportedCandidate {
    pub index: usize,
    pub candidate: ResolvedModelCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxiliaryModelResolution {
    pub lane: AuxiliaryLane,
    pub selected_index: usize,
    pub selected: ResolvedModelCandidate,
    pub candidates: Vec<AuxiliaryModelCandidateDecision>,
    pub supported_candidates: Vec<AuxiliaryModelSupportedCandidate>,
}

pub fn resolve_auxiliary_model(
    config: &Config,
    lane: AuxiliaryLane,
    catalog: Option<&dyn ModelProfileCatalogPort>,
) -> Result<AuxiliaryModelResolution, AuxiliaryModelResolutionError> {
    let candidates = resolve_explicit_lane_candidates(config, lane.capability_lane(), catalog);
    if candidates.is_empty() {
        return Err(AuxiliaryModelResolutionError::LaneNotConfigured { lane });
    }

    let mut decisions = Vec::with_capacity(candidates.len());
    let mut selected: Option<(usize, ResolvedModelCandidate)> = None;
    let mut supported_candidates = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        let skip_reason = auxiliary_candidate_skip_reason(lane, candidate);
        let can_select = skip_reason.is_none() && selected.is_none();
        if skip_reason.is_none() {
            supported_candidates.push(AuxiliaryModelSupportedCandidate {
                index,
                candidate: candidate.clone(),
            });
        }
        decisions.push(candidate_decision(
            index,
            candidate,
            can_select,
            skip_reason,
        ));
        if can_select {
            selected = Some((index, candidate.clone()));
        }
    }

    if let Some((selected_index, selected)) = selected {
        return Ok(AuxiliaryModelResolution {
            lane,
            selected_index,
            selected,
            candidates: decisions,
            supported_candidates,
        });
    }

    Err(AuxiliaryModelResolutionError::NoSupportedCandidate {
        lane,
        candidates: decisions,
    })
}

fn auxiliary_candidate_skip_reason(
    lane: AuxiliaryLane,
    candidate: &ResolvedModelCandidate,
) -> Option<AuxiliaryCandidateSkipReason> {
    match lane.capability_lane() {
        CapabilityLane::Reasoning
        | CapabilityLane::CheapReasoning
        | CapabilityLane::Compaction
        | CapabilityLane::Embedding
        | CapabilityLane::WebExtraction
        | CapabilityLane::ToolValidator => None,
        lane => match assess_lane_capability_support(&candidate.profile, lane) {
            LaneCapabilitySupport::Supported => None,
            LaneCapabilitySupport::MetadataUnknown => {
                Some(AuxiliaryCandidateSkipReason::MissingCapabilityMetadata)
            }
            LaneCapabilitySupport::MetadataStale => {
                Some(AuxiliaryCandidateSkipReason::StaleCapabilityMetadata)
            }
            LaneCapabilitySupport::MetadataLowConfidence => {
                Some(AuxiliaryCandidateSkipReason::LowConfidenceCapabilityMetadata)
            }
            LaneCapabilitySupport::MissingFeature(feature) => {
                Some(AuxiliaryCandidateSkipReason::MissingFeature(feature))
            }
        },
    }
}

fn candidate_decision(
    index: usize,
    candidate: &ResolvedModelCandidate,
    selected: bool,
    skip_reason: Option<AuxiliaryCandidateSkipReason>,
) -> AuxiliaryModelCandidateDecision {
    AuxiliaryModelCandidateDecision {
        index,
        provider: candidate.provider.clone(),
        model: candidate.model.clone(),
        source: model_lane_resolution_source_name(candidate.source).to_string(),
        selected,
        skip_reason,
        profile_context_window_source: resolved_model_profile_source_name(
            candidate.profile.context_window_source,
        )
        .to_string(),
        profile_context_window_freshness: resolved_model_profile_freshness_name(
            candidate.profile.context_window_freshness(),
        )
        .to_string(),
        profile_context_window_confidence: resolved_model_profile_confidence_name(
            candidate.profile.context_window_confidence(),
        )
        .to_string(),
        profile_features_source: resolved_model_profile_source_name(
            candidate.profile.features_source,
        )
        .to_string(),
        profile_features_freshness: resolved_model_profile_freshness_name(
            candidate.profile.features_freshness(),
        )
        .to_string(),
        profile_features_confidence: resolved_model_profile_confidence_name(
            candidate.profile.features_confidence(),
        )
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        Config, ModelCandidateProfileConfig, ModelLaneCandidateConfig, ModelLaneConfig,
    };

    fn candidate(provider: &str, model: &str) -> ModelLaneCandidateConfig {
        ModelLaneCandidateConfig {
            provider: provider.into(),
            model: model.into(),
            api_key: None,
            api_key_env: None,
            dimensions: None,
            profile: ModelCandidateProfileConfig::default(),
        }
    }

    #[test]
    fn explicit_compaction_lane_wins_without_default_route() {
        let mut config = Config::default();
        config.default_provider = Some("primary-provider".into());
        config.default_model = Some("primary-model".into());
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::Compaction,
            candidates: vec![candidate("summary-provider", "summary-model")],
        }];

        let resolved = resolve_auxiliary_model(&config, AuxiliaryLane::Compaction, None).unwrap();

        assert_eq!(resolved.selected.provider, "summary-provider");
        assert_eq!(resolved.selected.model, "summary-model");
        assert_eq!(resolved.selected_index, 0);
        assert_eq!(resolved.candidates.len(), 1);
        assert_eq!(resolved.supported_candidates.len(), 1);
        assert_eq!(resolved.supported_candidates[0].index, 0);
        assert!(resolved.candidates[0].selected);
    }

    #[test]
    fn missing_compaction_lane_is_typed_error() {
        let mut config = Config::default();
        config.default_provider = Some("primary-provider".into());
        config.default_model = Some("primary-model".into());

        let err = resolve_auxiliary_model(&config, AuxiliaryLane::Compaction, None).unwrap_err();

        assert_eq!(
            err,
            AuxiliaryModelResolutionError::LaneNotConfigured {
                lane: AuxiliaryLane::Compaction
            }
        );
    }

    #[test]
    fn explicit_embedding_lane_is_required() {
        let config = Config::default();

        let err = resolve_auxiliary_model(&config, AuxiliaryLane::Embedding, None).unwrap_err();

        assert!(matches!(
            err,
            AuxiliaryModelResolutionError::LaneNotConfigured {
                lane: AuxiliaryLane::Embedding
            }
        ));
    }

    #[test]
    fn bundled_preset_auxiliary_lane_is_configured_without_primary_fallback() {
        let mut config = Config::default();
        config.model_preset = Some("openrouter".into());
        config.default_provider = Some("openrouter".into());
        config.default_model = Some("anthropic/claude-sonnet-4-6".into());

        let compaction = resolve_auxiliary_model(&config, AuxiliaryLane::Compaction, None).unwrap();
        let embedding = resolve_auxiliary_model(&config, AuxiliaryLane::Embedding, None).unwrap();

        assert_eq!(compaction.selected.provider, "openrouter");
        assert_ne!(compaction.selected.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(compaction.candidates.len(), 2);
        assert_eq!(embedding.selected.provider, "openrouter");
        assert_eq!(embedding.selected.model, "qwen/qwen3-embedding-8b");
        assert_eq!(embedding.selected.dimensions, Some(4096));
    }

    #[test]
    fn unsupported_modality_candidate_is_skipped_before_selection() {
        let mut unsupported = candidate("provider-a", "plain-model");
        unsupported.profile = ModelCandidateProfileConfig {
            context_window_tokens: None,
            max_output_tokens: None,
            features: vec![ModelFeature::ToolCalling],
        };
        let mut supported = candidate("provider-b", "image-model");
        supported.profile = ModelCandidateProfileConfig {
            context_window_tokens: None,
            max_output_tokens: None,
            features: vec![ModelFeature::ImageGeneration],
        };

        let mut config = Config::default();
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::ImageGeneration,
            candidates: vec![unsupported, supported],
        }];

        let resolved =
            resolve_auxiliary_model(&config, AuxiliaryLane::ImageGeneration, None).unwrap();

        assert_eq!(resolved.selected_index, 1);
        assert_eq!(resolved.selected.provider, "provider-b");
        assert_eq!(
            resolved.candidates[0].skip_reason,
            Some(AuxiliaryCandidateSkipReason::MissingFeature(
                ModelFeature::ImageGeneration
            ))
        );
        assert!(!resolved.candidates[0].selected);
        assert!(resolved.candidates[1].selected);
        assert_eq!(resolved.supported_candidates.len(), 1);
        assert_eq!(resolved.supported_candidates[0].index, 1);
    }

    #[test]
    fn decision_trace_material_records_order_and_profile_sources() {
        let mut config = Config::default();
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::Compaction,
            candidates: vec![
                candidate("provider-a", "model-a"),
                candidate("provider-b", "model-b"),
            ],
        }];

        let resolved = resolve_auxiliary_model(&config, AuxiliaryLane::Compaction, None).unwrap();

        assert_eq!(
            resolved
                .candidates
                .iter()
                .map(|candidate| candidate.provider.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-a", "provider-b"]
        );
        assert!(resolved.candidates[0].selected);
        assert!(!resolved.candidates[1].selected);
        assert_eq!(resolved.candidates[0].source, "explicit_lane_config");
        assert_eq!(resolved.candidates[0].profile_features_source, "unknown");
    }
}
