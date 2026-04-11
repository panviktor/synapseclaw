use crate::application::services::model_lane_resolution::{
    ResolvedModelProfile, ResolvedModelProfileConfidence,
};
use crate::config::schema::ModelFeature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderNativeContextPolicyInput<'a> {
    pub profile: &'a ResolvedModelProfile,
    pub provider_prompt_caching: bool,
    pub operator_prompt_caching_enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderNativeContextPolicy {
    pub prompt_caching_supported: bool,
    pub prompt_caching_enabled: bool,
    pub server_continuation_supported: bool,
}

pub fn resolve_provider_native_context_policy(
    input: ProviderNativeContextPolicyInput<'_>,
) -> ProviderNativeContextPolicy {
    let features_confident =
        input.profile.features_confidence() >= ResolvedModelProfileConfidence::Medium;
    let profile_prompt_caching = features_confident
        && input
            .profile
            .features
            .contains(&ModelFeature::PromptCaching);
    let server_continuation_supported = features_confident
        && input
            .profile
            .features
            .contains(&ModelFeature::ServerContinuation);
    let prompt_caching_supported = input.provider_prompt_caching || profile_prompt_caching;

    ProviderNativeContextPolicy {
        prompt_caching_supported,
        prompt_caching_enabled: input.operator_prompt_caching_enabled && prompt_caching_supported,
        server_continuation_supported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::model_lane_resolution::ResolvedModelProfileSource;

    #[test]
    fn enables_prompt_cache_only_when_operator_and_route_support_it() {
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::PromptCaching],
            features_source: ResolvedModelProfileSource::ManualConfig,
            ..Default::default()
        };

        let policy = resolve_provider_native_context_policy(ProviderNativeContextPolicyInput {
            profile: &profile,
            provider_prompt_caching: false,
            operator_prompt_caching_enabled: true,
        });

        assert!(policy.prompt_caching_supported);
        assert!(policy.prompt_caching_enabled);
        assert!(!policy.server_continuation_supported);
    }

    #[test]
    fn exposes_server_continuation_as_supported_not_globally_forced() {
        let profile = ResolvedModelProfile {
            features: vec![ModelFeature::ServerContinuation],
            features_source: ResolvedModelProfileSource::BundledCatalog,
            ..Default::default()
        };

        let policy = resolve_provider_native_context_policy(ProviderNativeContextPolicyInput {
            profile: &profile,
            provider_prompt_caching: false,
            operator_prompt_caching_enabled: false,
        });

        assert!(policy.server_continuation_supported);
        assert!(!policy.prompt_caching_supported);
        assert!(!policy.prompt_caching_enabled);
    }
}
