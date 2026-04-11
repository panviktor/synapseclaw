use crate::application::services::model_lane_resolution::resolve_lane_candidates;
use crate::config::schema::{CapabilityLane, Config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryRouteSource {
    ExplicitSummaryConfig,
    ExplicitSummaryModel,
    CheapRoute,
    CurrentRoute,
}

impl SummaryRouteSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitSummaryConfig => "explicit_summary_config",
            Self::ExplicitSummaryModel => "explicit_summary_model",
            Self::CheapRoute => "cheap_route",
            Self::CurrentRoute => "current_route",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SummaryRouteResolution {
    pub source: SummaryRouteSource,
    pub provider: Option<String>,
    pub model: String,
    pub temperature: f64,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
}

pub fn resolve_summary_route(config: &Config, current_model: &str) -> SummaryRouteResolution {
    let summary = &config.summary;
    let summary_model = config.summary_model.as_deref();
    let model_routes = &config.model_routes;
    let explicit_summary_config =
        summary.provider.is_some() || summary.model.is_some() || summary.api_key_env.is_some();

    if explicit_summary_config {
        return SummaryRouteResolution {
            source: SummaryRouteSource::ExplicitSummaryConfig,
            provider: summary.provider.clone(),
            model: summary
                .model
                .clone()
                .or_else(|| summary_model.map(str::to_owned))
                .unwrap_or_else(|| current_model.to_string()),
            temperature: summary.temperature,
            api_key: None,
            api_key_env: summary.api_key_env.clone(),
        };
    }

    if let Some(model) = summary_model {
        return SummaryRouteResolution {
            source: SummaryRouteSource::ExplicitSummaryModel,
            provider: None,
            model: model.to_string(),
            temperature: summary.temperature,
            api_key: None,
            api_key_env: None,
        };
    }

    let cheap_candidates = resolve_lane_candidates(config, CapabilityLane::CheapReasoning, None);
    if let Some(candidate) = cheap_candidates.first() {
        return SummaryRouteResolution {
            source: SummaryRouteSource::CheapRoute,
            provider: Some(candidate.provider.clone()),
            model: candidate.model.clone(),
            temperature: summary.temperature,
            api_key: candidate.api_key.clone(),
            api_key_env: candidate.api_key_env.clone(),
        };
    }

    if let Some(route) = model_routes.iter().find(|route| route.hint == "cheap") {
        return SummaryRouteResolution {
            source: SummaryRouteSource::CheapRoute,
            provider: Some(route.provider.clone()),
            model: route.model.clone(),
            temperature: summary.temperature,
            api_key: route.api_key.clone(),
            api_key_env: None,
        };
    }

    SummaryRouteResolution {
        source: SummaryRouteSource::CurrentRoute,
        provider: None,
        model: current_model.to_string(),
        temperature: summary.temperature,
        api_key: None,
        api_key_env: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        Config, ModelCandidateProfileConfig, ModelLaneCandidateConfig, ModelLaneConfig,
        ModelRouteConfig, SummaryConfig,
    };

    fn cheap_route() -> ModelRouteConfig {
        ModelRouteConfig {
            hint: "cheap".into(),
            capability: Some(CapabilityLane::CheapReasoning),
            provider: "openrouter".into(),
            model: "qwen/qwen3.6-plus".into(),
            api_key: Some("test-key".into()),
            profile: ModelCandidateProfileConfig::default(),
        }
    }

    fn base_config() -> Config {
        let mut config = Config::default();
        config.default_model = Some("gpt-5.4".into());
        config
    }

    #[test]
    fn explicit_summary_config_wins_over_cheap_route() {
        let summary = SummaryConfig {
            provider: Some("anthropic".into()),
            model: Some("claude-haiku".into()),
            temperature: 0.2,
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
        };

        let mut config = base_config();
        config.summary = summary;
        config.model_routes = vec![cheap_route()];

        let resolved = resolve_summary_route(&config, "gpt-5.4");

        assert_eq!(resolved.source, SummaryRouteSource::ExplicitSummaryConfig);
        assert_eq!(resolved.provider.as_deref(), Some("anthropic"));
        assert_eq!(resolved.model, "claude-haiku");
        assert_eq!(resolved.temperature, 0.2);
        assert_eq!(resolved.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn summary_model_wins_over_cheap_route() {
        let mut config = base_config();
        config.summary_model = Some("gpt-5.4-mini".into());
        config.model_routes = vec![cheap_route()];
        let resolved = resolve_summary_route(&config, "gpt-5.4");

        assert_eq!(resolved.source, SummaryRouteSource::ExplicitSummaryModel);
        assert_eq!(resolved.provider, None);
        assert_eq!(resolved.model, "gpt-5.4-mini");
    }

    #[test]
    fn cheap_route_becomes_default_summary_lane() {
        let mut config = base_config();
        config.model_routes = vec![cheap_route()];
        let resolved = resolve_summary_route(&config, "gpt-5.4");

        assert_eq!(resolved.source, SummaryRouteSource::CheapRoute);
        assert_eq!(resolved.provider.as_deref(), Some("openrouter"));
        assert_eq!(resolved.model, "qwen/qwen3.6-plus");
        assert_eq!(resolved.api_key.as_deref(), Some("test-key"));
    }

    #[test]
    fn current_route_is_final_fallback() {
        let config = base_config();
        let resolved = resolve_summary_route(&config, "gpt-5.4");

        assert_eq!(resolved.source, SummaryRouteSource::CurrentRoute);
        assert_eq!(resolved.provider, None);
        assert_eq!(resolved.model, "gpt-5.4");
    }

    #[test]
    fn capability_lane_candidates_beat_legacy_cheap_route() {
        let mut config = base_config();
        config.model_routes = vec![cheap_route()];
        config.model_lanes = vec![ModelLaneConfig {
            lane: CapabilityLane::CheapReasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: "anthropic".into(),
                model: "claude-haiku".into(),
                api_key: None,
                api_key_env: Some("ANTHROPIC_API_KEY".into()),
                dimensions: None,
                profile: ModelCandidateProfileConfig::default(),
            }],
        }];

        let resolved = resolve_summary_route(&config, "gpt-5.4");

        assert_eq!(resolved.source, SummaryRouteSource::CheapRoute);
        assert_eq!(resolved.provider.as_deref(), Some("anthropic"));
        assert_eq!(resolved.model, "claude-haiku");
        assert_eq!(resolved.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }
}
