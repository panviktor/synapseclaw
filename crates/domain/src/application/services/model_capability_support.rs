use crate::application::services::model_lane_resolution::{
    ResolvedModelProfile, ResolvedModelProfileConfidence, ResolvedModelProfileFreshness,
};
use crate::config::schema::{CapabilityLane, ModelFeature};
use crate::ports::provider::ProviderCapabilities;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneCapabilitySupport {
    Supported,
    MissingFeature(ModelFeature),
    MetadataUnknown,
    MetadataStale,
    MetadataLowConfidence,
}

pub fn assess_lane_capability_support(
    profile: &ResolvedModelProfile,
    lane: CapabilityLane,
) -> LaneCapabilitySupport {
    if matches!(
        lane,
        CapabilityLane::Reasoning | CapabilityLane::CheapReasoning
    ) {
        return LaneCapabilitySupport::Supported;
    }

    if profile.features_freshness() == ResolvedModelProfileFreshness::Unknown {
        return LaneCapabilitySupport::MetadataUnknown;
    }
    if profile.features_freshness() == ResolvedModelProfileFreshness::Stale {
        return LaneCapabilitySupport::MetadataStale;
    }
    if profile.features_confidence() < ResolvedModelProfileConfidence::Medium {
        return LaneCapabilitySupport::MetadataLowConfidence;
    }

    lane_required_feature(lane)
        .filter(|feature| profile_supports_feature(profile, feature.clone()))
        .map(|_| LaneCapabilitySupport::Supported)
        .unwrap_or_else(|| {
            LaneCapabilitySupport::MissingFeature(
                lane_required_feature(lane).expect("specialized lanes should map to a feature"),
            )
        })
}

pub fn profile_supports_lane_confidently(
    profile: &ResolvedModelProfile,
    lane: CapabilityLane,
) -> bool {
    assess_lane_capability_support(profile, lane) == LaneCapabilitySupport::Supported
}

pub fn supports_multimodal_input(
    provider_capabilities: &ProviderCapabilities,
    profile: &ResolvedModelProfile,
) -> bool {
    provider_capabilities.vision
        || profile_supports_lane_confidently(profile, CapabilityLane::MultimodalUnderstanding)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCallCapabilityIssue {
    MissingVisionInput { image_marker_count: usize },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCallCapabilityAssessment {
    pub issues: Vec<ProviderCallCapabilityIssue>,
}

impl ProviderCallCapabilityAssessment {
    pub fn allows_provider_call(&self) -> bool {
        self.issues.is_empty()
    }
}

pub struct ProviderCallCapabilityInput<'a> {
    pub image_marker_count: usize,
    pub provider_capabilities: &'a ProviderCapabilities,
    pub route_profile: &'a ResolvedModelProfile,
}

pub fn assess_provider_call_capabilities(
    input: ProviderCallCapabilityInput<'_>,
) -> ProviderCallCapabilityAssessment {
    let mut issues = Vec::new();
    if input.image_marker_count > 0
        && !supports_multimodal_input(input.provider_capabilities, input.route_profile)
    {
        issues.push(ProviderCallCapabilityIssue::MissingVisionInput {
            image_marker_count: input.image_marker_count,
        });
    }

    ProviderCallCapabilityAssessment { issues }
}

pub fn lane_required_feature(lane: CapabilityLane) -> Option<ModelFeature> {
    match lane {
        CapabilityLane::Reasoning | CapabilityLane::CheapReasoning => None,
        CapabilityLane::Embedding => Some(ModelFeature::Embedding),
        CapabilityLane::ImageGeneration => Some(ModelFeature::ImageGeneration),
        CapabilityLane::AudioGeneration => Some(ModelFeature::AudioGeneration),
        CapabilityLane::VideoGeneration => Some(ModelFeature::VideoGeneration),
        CapabilityLane::MusicGeneration => Some(ModelFeature::MusicGeneration),
        CapabilityLane::MultimodalUnderstanding => Some(ModelFeature::MultimodalUnderstanding),
    }
}

fn profile_supports_feature(profile: &ResolvedModelProfile, feature: ModelFeature) -> bool {
    match feature {
        ModelFeature::MultimodalUnderstanding => {
            profile
                .features
                .contains(&ModelFeature::MultimodalUnderstanding)
                || profile.features.contains(&ModelFeature::Vision)
        }
        other => profile.features.contains(&other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::model_lane_resolution::ResolvedModelProfileSource;

    #[test]
    fn reports_supported_for_confident_feature() {
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::ImageGeneration],
            features_source: ResolvedModelProfileSource::ManualConfig,
            ..Default::default()
        };

        assert_eq!(
            assess_lane_capability_support(&profile, CapabilityLane::ImageGeneration),
            LaneCapabilitySupport::Supported
        );
    }

    #[test]
    fn reports_metadata_low_confidence_before_missing_feature() {
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::ImageGeneration],
            features_source: ResolvedModelProfileSource::AdapterFallback,
            observed_at_unix: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("system time should be after unix epoch")
                    .as_secs()
                    .saturating_sub(8 * 24 * 60 * 60),
            ),
            ..Default::default()
        };

        assert_eq!(
            assess_lane_capability_support(&profile, CapabilityLane::ImageGeneration),
            LaneCapabilitySupport::MetadataLowConfidence
        );
    }

    #[test]
    fn provider_call_capability_accepts_route_profile_multimodal_support() {
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::Vision],
            features_source: ResolvedModelProfileSource::ManualConfig,
            ..Default::default()
        };

        let assessment = assess_provider_call_capabilities(ProviderCallCapabilityInput {
            image_marker_count: 1,
            provider_capabilities: &ProviderCapabilities::default(),
            route_profile: &profile,
        });

        assert!(assessment.allows_provider_call());
    }

    #[test]
    fn provider_call_capability_rejects_image_input_without_route_support() {
        let assessment = assess_provider_call_capabilities(ProviderCallCapabilityInput {
            image_marker_count: 1,
            provider_capabilities: &ProviderCapabilities::default(),
            route_profile: &ResolvedModelProfile::default(),
        });

        assert_eq!(
            assessment.issues,
            vec![ProviderCallCapabilityIssue::MissingVisionInput {
                image_marker_count: 1
            }]
        );
    }
}
