use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::schema::{
    ModelFeature, ModelLaneCandidateConfig, ModelLaneConfig, ModelPricing, ModelRouteConfig,
};
use crate::domain::memory::{EmbeddingDistanceMetric, EmbeddingProfile};
use crate::ports::model_profile_catalog::{CatalogModelProfile, CatalogModelProfileSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownModelPreset {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CuratedModelDefinition {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelCatalogData {
    #[serde(default)]
    presets: Vec<ModelPresetCatalogEntry>,
    #[serde(default)]
    providers: Vec<ProviderModelCatalogEntry>,
    #[serde(default)]
    pricing: Vec<ModelPricingCatalogEntry>,
    #[serde(default, alias = "model_profiles")]
    profiles: Vec<ModelProfileCatalogEntry>,
    #[serde(default)]
    embedding_profiles: Vec<EmbeddingProfileCatalogEntry>,
    #[serde(default, alias = "model_routes")]
    route_aliases: Vec<ModelRouteConfig>,
    #[serde(default)]
    request_policies: Vec<ModelRequestPolicyCatalogEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelPresetCatalogEntry {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    title: String,
    description: String,
    default_provider: String,
    default_model: String,
    #[serde(default)]
    seed_multimodal_from_reasoning: bool,
    #[serde(default)]
    provider_aliases: Vec<String>,
    #[serde(default)]
    extra_lanes: Vec<ModelLaneConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderModelCatalogEntry {
    provider: String,
    default_model: String,
    #[serde(default)]
    api_base_urls: Vec<String>,
    #[serde(default)]
    curated_models: Vec<CuratedModelDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelPricingCatalogEntry {
    model: String,
    input: f64,
    output: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelProfileCatalogEntry {
    provider: String,
    model: String,
    #[serde(default)]
    context_window_tokens: Option<usize>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    features: Vec<ModelFeature>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelRequestPolicyCatalogEntry {
    provider: String,
    model: String,
    #[serde(default)]
    fixed_temperature: Option<f64>,
    #[serde(default)]
    reasoning_efforts: Vec<String>,
    #[serde(default)]
    default_reasoning_effort: Option<String>,
    #[serde(default)]
    reasoning_effort_aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequestPolicy {
    pub fixed_temperature: Option<f64>,
    pub reasoning_efforts: Vec<String>,
    pub default_reasoning_effort: Option<String>,
    pub reasoning_effort_aliases: HashMap<String, String>,
}

impl ModelRequestPolicy {
    pub fn resolve_reasoning_effort(&self, requested: &str) -> Option<String> {
        let requested = requested.trim().to_ascii_lowercase();
        if requested.is_empty() {
            return self.default_reasoning_effort.clone();
        }

        let resolved = self
            .reasoning_effort_aliases
            .get(&requested)
            .cloned()
            .unwrap_or(requested);

        if self.reasoning_efforts.is_empty()
            || self
                .reasoning_efforts
                .iter()
                .any(|effort| effort.eq_ignore_ascii_case(&resolved))
        {
            return Some(resolved);
        }

        self.default_reasoning_effort.clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct EmbeddingProfileCatalogEntry {
    provider: String,
    model: String,
    dimensions: usize,
    distance_metric: EmbeddingDistanceMetric,
    normalize_output: bool,
    #[serde(default)]
    query_prefix: Option<String>,
    #[serde(default)]
    document_prefix: Option<String>,
    supports_multilingual: bool,
    supports_code: bool,
    recommended_chunk_chars: usize,
    recommended_top_k: usize,
}

#[derive(Debug)]
struct ParsedModelCatalog {
    data: ModelCatalogData,
    presets_view: Vec<KnownModelPreset>,
}

const BUNDLED_MODEL_CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/data/model_catalog.json"
));

fn bundled_model_catalog() -> &'static ParsedModelCatalog {
    static CATALOG: OnceLock<ParsedModelCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let data = parse_model_catalog_data(BUNDLED_MODEL_CATALOG_JSON)
            .expect("built-in model catalog must parse");
        parsed_catalog_from_data(data)
    })
}

fn runtime_override_slot() -> &'static RwLock<Option<&'static ParsedModelCatalog>> {
    static RUNTIME_OVERRIDE: OnceLock<RwLock<Option<&'static ParsedModelCatalog>>> =
        OnceLock::new();
    RUNTIME_OVERRIDE.get_or_init(|| RwLock::new(None))
}

fn active_model_catalog() -> &'static ParsedModelCatalog {
    runtime_override_slot()
        .read()
        .expect("runtime model catalog override lock poisoned")
        .unwrap_or_else(bundled_model_catalog)
}

fn parse_model_catalog_data(payload: &str) -> Result<ModelCatalogData> {
    serde_json::from_str(payload).context("failed to parse model catalog JSON")
}

fn parsed_catalog_from_data(data: ModelCatalogData) -> ParsedModelCatalog {
    let presets_view = data
        .presets
        .iter()
        .map(|preset| KnownModelPreset {
            id: preset.id.clone(),
            title: preset.title.clone(),
            description: preset.description.clone(),
        })
        .collect();
    ParsedModelCatalog { data, presets_view }
}

fn merge_catalog_data(
    base: &ModelCatalogData,
    override_data: ModelCatalogData,
) -> ModelCatalogData {
    let mut merged = base.clone();

    for preset in override_data.presets {
        if let Some(existing) = merged.presets.iter_mut().find(|item| item.id == preset.id) {
            *existing = preset;
        } else {
            merged.presets.push(preset);
        }
    }

    for provider in override_data.providers {
        if let Some(existing) = merged
            .providers
            .iter_mut()
            .find(|item| item.provider == provider.provider)
        {
            *existing = provider;
        } else {
            merged.providers.push(provider);
        }
    }

    for pricing in override_data.pricing {
        if let Some(existing) = merged
            .pricing
            .iter_mut()
            .find(|item| item.model == pricing.model)
        {
            *existing = pricing;
        } else {
            merged.pricing.push(pricing);
        }
    }

    for profile in override_data.profiles {
        if let Some(existing) = merged.profiles.iter_mut().find(|item| {
            item.provider.eq_ignore_ascii_case(&profile.provider)
                && item.model.eq_ignore_ascii_case(&profile.model)
        }) {
            *existing = profile;
        } else {
            merged.profiles.push(profile);
        }
    }

    for profile in override_data.embedding_profiles {
        if let Some(existing) = merged
            .embedding_profiles
            .iter_mut()
            .find(|item| embedding_profile_catalog_key_matches(item, &profile))
        {
            *existing = profile;
        } else {
            merged.embedding_profiles.push(profile);
        }
    }

    for route in override_data.route_aliases {
        if let Some(existing) = merged
            .route_aliases
            .iter_mut()
            .find(|item| item.hint.eq_ignore_ascii_case(&route.hint))
        {
            *existing = route;
        } else {
            merged.route_aliases.push(route);
        }
    }

    for policy in override_data.request_policies {
        if let Some(existing) = merged.request_policies.iter_mut().find(|item| {
            item.provider.eq_ignore_ascii_case(&policy.provider)
                && item.model.eq_ignore_ascii_case(&policy.model)
        }) {
            *existing = policy;
        } else {
            merged.request_policies.push(policy);
        }
    }

    merged
}

pub fn bundled_model_catalog_json() -> &'static str {
    BUNDLED_MODEL_CATALOG_JSON
}

pub fn install_runtime_model_catalog_override_json(payload: &str) -> Result<()> {
    let override_data =
        parse_model_catalog_data(payload).context("failed to parse user model catalog override")?;
    let merged = merge_catalog_data(&bundled_model_catalog().data, override_data);
    let leaked = Box::leak(Box::new(parsed_catalog_from_data(merged)));
    *runtime_override_slot()
        .write()
        .expect("runtime model catalog override lock poisoned") = Some(leaked);
    Ok(())
}

pub fn runtime_model_catalog_override_active() -> bool {
    runtime_override_slot()
        .read()
        .expect("runtime model catalog override lock poisoned")
        .is_some()
}

pub fn known_model_presets() -> &'static [KnownModelPreset] {
    active_model_catalog().presets_view.as_slice()
}

pub fn preset_title(preset_id: &str) -> Option<&'static str> {
    active_model_catalog()
        .presets_view
        .iter()
        .find(|preset| preset.id == preset_id)
        .map(|preset| preset.title.as_str())
}

pub fn preset_description(preset_id: &str) -> Option<&'static str> {
    active_model_catalog()
        .presets_view
        .iter()
        .find(|preset| preset.id == preset_id)
        .map(|preset| preset.description.as_str())
}

pub fn normalize_model_preset_id(value: &str) -> Option<&'static str> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    active_model_catalog()
        .data
        .presets
        .iter()
        .find(|preset| {
            preset.id.eq_ignore_ascii_case(value)
                || preset
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(value))
        })
        .map(|preset| preset.id.as_str())
}

pub fn recommended_preset_for_provider(provider: &str) -> Option<&'static str> {
    let provider = provider.trim();
    if provider.is_empty() {
        return None;
    }

    active_model_catalog()
        .data
        .presets
        .iter()
        .find(|preset| {
            preset.default_provider.eq_ignore_ascii_case(provider)
                || preset
                    .provider_aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(provider))
        })
        .map(|preset| preset.id.as_str())
}

pub fn preset_reasoning_seed(preset_id: &str) -> Option<(&'static str, &'static str)> {
    active_model_catalog()
        .data
        .presets
        .iter()
        .find(|preset| preset.id == preset_id)
        .map(|preset| {
            (
                preset.default_provider.as_str(),
                preset.default_model.as_str(),
            )
        })
}

pub fn preset_seed_multimodal_from_reasoning(preset_id: &str) -> bool {
    active_model_catalog()
        .data
        .presets
        .iter()
        .find(|preset| preset.id == preset_id)
        .is_some_and(|preset| preset.seed_multimodal_from_reasoning)
}

pub fn preset_extra_lanes(preset_id: &str) -> Option<Vec<ModelLaneConfig>> {
    active_model_catalog()
        .data
        .presets
        .iter()
        .find(|preset| preset.id == preset_id)
        .map(|preset| preset.extra_lanes.clone())
}

pub fn provider_default_model(provider: &str) -> Option<&'static str> {
    active_model_catalog()
        .data
        .providers
        .iter()
        .find(|entry| entry.provider == provider)
        .map(|entry| entry.default_model.as_str())
}

pub fn provider_curated_models(provider: &str) -> Option<Vec<(String, String)>> {
    active_model_catalog()
        .data
        .providers
        .iter()
        .find(|entry| entry.provider == provider)
        .map(|entry| {
            entry
                .curated_models
                .iter()
                .map(|model| (model.id.clone(), model.label.clone()))
                .collect()
        })
}

pub fn provider_for_api_base_url(endpoint: &str) -> Option<&'static str> {
    let endpoint = normalize_api_base_url(endpoint)?;
    active_model_catalog()
        .data
        .providers
        .iter()
        .find(|entry| {
            entry.api_base_urls.iter().any(|base_url| {
                let Some(base_url) = normalize_api_base_url(base_url) else {
                    return false;
                };
                endpoint == base_url || endpoint.starts_with(&format!("{base_url}/"))
            })
        })
        .map(|entry| entry.provider.as_str())
}

fn normalize_api_base_url(value: &str) -> Option<String> {
    let normalized = value.trim().trim_end_matches('/').to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

pub fn default_pricing_table() -> HashMap<String, ModelPricing> {
    active_model_catalog()
        .data
        .pricing
        .iter()
        .map(|entry| {
            (
                entry.model.clone(),
                ModelPricing {
                    input: entry.input,
                    output: entry.output,
                },
            )
        })
        .collect()
}

pub fn model_profile(provider: &str, model: &str) -> Option<CatalogModelProfile> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return None;
    }

    active_model_catalog()
        .data
        .profiles
        .iter()
        .find(|entry| {
            entry.provider.eq_ignore_ascii_case(provider) && entry.model.eq_ignore_ascii_case(model)
        })
        .map(|entry| CatalogModelProfile {
            context_window_tokens: entry.context_window_tokens,
            max_output_tokens: entry.max_output_tokens,
            features: entry.features.clone(),
            source: Some(CatalogModelProfileSource::BundledCatalog),
            observed_at_unix: None,
        })
}

pub fn model_request_policy(provider: &str, model: &str) -> Option<ModelRequestPolicy> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return None;
    }

    let normalized_model = model.rsplit('/').next().unwrap_or(model);

    active_model_catalog()
        .data
        .request_policies
        .iter()
        .find(|entry| {
            entry.provider.eq_ignore_ascii_case(provider)
                && (entry.model.eq_ignore_ascii_case(model)
                    || entry.model.eq_ignore_ascii_case(normalized_model))
        })
        .map(|entry| ModelRequestPolicy {
            fixed_temperature: entry.fixed_temperature,
            reasoning_efforts: entry.reasoning_efforts.clone(),
            default_reasoning_effort: entry.default_reasoning_effort.clone(),
            reasoning_effort_aliases: entry.reasoning_effort_aliases.clone(),
        })
}

pub fn embedding_profile(
    provider: &str,
    model: &str,
    dimensions: usize,
) -> Option<EmbeddingProfile> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let provider_family = embedding_provider_family(provider);

    active_model_catalog()
        .data
        .embedding_profiles
        .iter()
        .find(|entry| {
            embedding_profile_matches(entry, provider, provider_family, model, dimensions)
        })
        .map(|entry| {
            embedding_profile_from_catalog_entry(entry, provider_family, model, dimensions)
        })
}

fn embedding_provider_family(provider: &str) -> &str {
    match provider.split(':').next().unwrap_or(provider) {
        "custom" => "custom",
        other => other,
    }
}

fn embedding_profile_matches(
    entry: &EmbeddingProfileCatalogEntry,
    provider: &str,
    provider_family: &str,
    model: &str,
    dimensions: usize,
) -> bool {
    (entry.provider.eq_ignore_ascii_case(provider)
        || entry.provider.eq_ignore_ascii_case(provider_family))
        && entry.model.eq_ignore_ascii_case(model)
        && entry.dimensions == dimensions
}

fn embedding_profile_from_catalog_entry(
    entry: &EmbeddingProfileCatalogEntry,
    provider_family: &str,
    model: &str,
    dimensions: usize,
) -> EmbeddingProfile {
    EmbeddingProfile {
        profile_id: format!("{provider_family}:{model}:{dimensions}"),
        provider_family: provider_family.to_string(),
        model_id: model.to_string(),
        dimensions,
        distance_metric: entry.distance_metric.clone(),
        normalize_output: entry.normalize_output,
        query_prefix: entry.query_prefix.clone(),
        document_prefix: entry.document_prefix.clone(),
        supports_multilingual: entry.supports_multilingual,
        supports_code: entry.supports_code,
        recommended_chunk_chars: entry.recommended_chunk_chars,
        recommended_top_k: entry.recommended_top_k,
    }
}

fn embedding_profile_catalog_key_matches(
    left: &EmbeddingProfileCatalogEntry,
    right: &EmbeddingProfileCatalogEntry,
) -> bool {
    left.provider.eq_ignore_ascii_case(&right.provider)
        && left.model.eq_ignore_ascii_case(&right.model)
        && left.dimensions == right.dimensions
}

pub fn model_route_aliases() -> Vec<ModelRouteConfig> {
    active_model_catalog().data.route_aliases.clone()
}

pub fn model_route_alias(value: &str) -> Option<ModelRouteConfig> {
    let value = value.trim().trim_matches('`');
    if value.is_empty() {
        return None;
    }

    active_model_catalog()
        .data
        .route_aliases
        .iter()
        .find(|route| {
            route.hint.eq_ignore_ascii_case(value) || route.model.eq_ignore_ascii_case(value)
        })
        .cloned()
}

pub fn apply_default_api_key(lanes: &mut [ModelLaneConfig], api_key: Option<&String>) {
    for lane in lanes {
        for candidate in &mut lane.candidates {
            hydrate_candidate_api_key(candidate, api_key);
        }
    }
}

fn hydrate_candidate_api_key(candidate: &mut ModelLaneCandidateConfig, api_key: Option<&String>) {
    if candidate.api_key.is_none() && candidate.api_key_env.is_none() {
        candidate.api_key = api_key.cloned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::CapabilityLane;

    #[test]
    fn bundled_catalog_exposes_known_presets() {
        let presets = known_model_presets();
        assert!(presets.iter().any(|preset| preset.id == "chatgpt"));
        assert!(presets.iter().any(|preset| preset.id == "openrouter"));
    }

    #[test]
    fn preset_aliases_come_from_catalog_data() {
        assert_eq!(normalize_model_preset_id("codex"), Some("chatgpt"));
        assert_eq!(
            recommended_preset_for_provider("openai-codex"),
            Some("chatgpt")
        );
        assert_eq!(recommended_preset_for_provider("llamacpp"), Some("local"));
    }

    #[test]
    fn openrouter_preset_has_auxiliary_lanes() {
        let lanes = preset_extra_lanes("openrouter").expect("preset should exist");
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::CheapReasoning));
        assert!(lanes
            .iter()
            .any(|lane| lane.lane == CapabilityLane::Embedding));
    }

    #[test]
    fn openrouter_catalog_exposes_gemma_standard_profiles_pricing_and_aliases() {
        let curated = provider_curated_models("openrouter").expect("provider should exist");
        assert!(curated
            .iter()
            .any(|(model, _)| model == "google/gemma-4-31b-it"));
        assert!(curated
            .iter()
            .any(|(model, _)| model == "google/gemma-4-26b-a4b-it"));
        assert!(curated.iter().any(|(model, _)| model == "x-ai/grok-4.20"));

        let pricing = default_pricing_table();
        let price = pricing
            .get("google/gemma-4-31b-it")
            .expect("pricing should exist");
        assert_eq!(price.input, 0.14);
        assert_eq!(price.output, 0.40);
        let price = pricing
            .get("google/gemma-4-26b-a4b-it")
            .expect("pricing should exist");
        assert_eq!(price.input, 0.12);
        assert_eq!(price.output, 0.40);
        let price = pricing.get("x-ai/grok-4.20").expect("pricing should exist");
        assert_eq!(price.input, 2.0);
        assert_eq!(price.output, 6.0);

        let profile =
            model_profile("openrouter", "google/gemma-4-31b-it").expect("profile should exist");
        assert_eq!(profile.context_window_tokens, Some(262_144));
        assert_eq!(profile.max_output_tokens, Some(131_072));
        assert!(profile.features.contains(&ModelFeature::ToolCalling));
        assert!(profile.features.contains(&ModelFeature::Vision));
        assert!(profile
            .features
            .contains(&ModelFeature::MultimodalUnderstanding));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::BundledCatalog)
        );

        let profile =
            model_profile("openrouter", "google/gemma-4-26b-a4b-it").expect("profile should exist");
        assert_eq!(profile.context_window_tokens, Some(262_144));
        assert_eq!(profile.max_output_tokens, Some(262_144));
        assert!(profile.features.contains(&ModelFeature::ToolCalling));
        assert!(profile.features.contains(&ModelFeature::Vision));
        assert!(profile
            .features
            .contains(&ModelFeature::MultimodalUnderstanding));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::BundledCatalog)
        );

        let profile = model_profile("openrouter", "x-ai/grok-4.20").expect("profile should exist");
        assert_eq!(profile.context_window_tokens, Some(2_000_000));
        assert_eq!(profile.max_output_tokens, Some(66_000));
        assert!(profile.features.contains(&ModelFeature::ToolCalling));
        assert_eq!(
            profile.source,
            Some(CatalogModelProfileSource::BundledCatalog)
        );

        let aliases = model_route_aliases();
        assert!(aliases.iter().any(|route| route.hint == "gemma31b"));
        assert!(aliases.iter().any(|route| route.hint == "gemma26b"));
        assert!(aliases.iter().any(|route| route.hint == "grok420"));
        let alias = model_route_alias("gemma31b").expect("alias should exist");
        assert_eq!(alias.provider, "openrouter");
        assert_eq!(alias.model, "google/gemma-4-31b-it");
        let alias = model_route_alias("grok-4.20").expect("alias should exist");
        assert_eq!(alias.provider, "openrouter");
        assert_eq!(alias.model, "x-ai/grok-4.20");
        let alias = model_route_alias("qwen36").expect("alias should exist");
        assert_eq!(alias.provider, "openrouter");
        assert_eq!(alias.model, "qwen/qwen3.6-plus");
    }

    #[test]
    fn catalog_exposes_embedding_calibration_profiles() {
        let profile = embedding_profile("openrouter", "qwen/qwen3-embedding-8b", 4096)
            .expect("embedding profile should exist");
        assert_eq!(profile.provider_family, "openrouter");
        assert_eq!(profile.dimensions, 4096);
        assert_eq!(profile.distance_metric, EmbeddingDistanceMetric::Cosine);
        assert!(profile.normalize_output);
        assert!(profile.supports_multilingual);
        assert!(profile.supports_code);
        assert_eq!(profile.recommended_chunk_chars, 1200);
        assert_eq!(profile.recommended_top_k, 10);

        let profile = embedding_profile("llama.cpp", "multilingual-e5-small", 384)
            .expect("e5 profile should exist");
        assert_eq!(profile.query_prefix.as_deref(), Some("query: "));
        assert_eq!(profile.document_prefix.as_deref(), Some("passage: "));

        assert!(embedding_profile("openrouter", "qwen/qwen3-embedding-8b", 1536).is_none());
    }

    #[test]
    fn provider_defaults_come_from_catalog_data() {
        assert_eq!(provider_default_model("openai-codex"), Some("gpt-5.4"));
        assert_eq!(provider_default_model("ollama"), Some("llama4-scout"));
    }

    #[test]
    fn provider_can_be_inferred_from_catalog_base_url() {
        assert_eq!(
            provider_for_api_base_url("https://api.deepseek.com/v1"),
            Some("deepseek")
        );
        assert_eq!(
            provider_for_api_base_url("https://openrouter.ai/api/v1/"),
            Some("openrouter")
        );
        assert_eq!(
            provider_for_api_base_url("https://unknown.example.com/v1"),
            None
        );
    }

    #[test]
    fn override_catalog_can_replace_provider_defaults() {
        let override_data = parse_model_catalog_data(
            r#"{
              "providers": [
                {
                  "provider": "openai-codex",
                  "default_model": "example-main",
                  "curated_models": [{ "id": "example-main", "label": "Example Main" }]
                }
              ],
              "profiles": [
                {
                  "provider": "openrouter",
                  "model": "google/gemma-4-31b-it",
                  "context_window_tokens": 128000,
                  "features": ["tool_calling"]
                }
              ],
              "embedding_profiles": [
                {
                  "provider": "openrouter",
                  "model": "qwen/qwen3-embedding-8b",
                  "dimensions": 4096,
                  "distance_metric": "cosine",
                  "normalize_output": true,
                  "supports_multilingual": false,
                  "supports_code": false,
                  "recommended_chunk_chars": 900,
                  "recommended_top_k": 6
                }
              ],
              "route_aliases": [
                {
                  "hint": "gemma31b",
                  "provider": "openrouter",
                  "model": "google/gemma-4-31b-preview"
                }
              ],
              "request_policies": [
                {
                  "provider": "openai-codex",
                  "model": "gpt-5.4",
                  "default_reasoning_effort": "high",
                  "reasoning_efforts": ["low", "medium", "high"],
                  "reasoning_effort_aliases": { "minimal": "low", "xhigh": "high" }
                }
              ]
            }"#,
        )
        .expect("override should parse");

        let merged = merge_catalog_data(&bundled_model_catalog().data, override_data);
        let openai = merged
            .providers
            .iter()
            .find(|provider| provider.provider == "openai-codex")
            .expect("provider should exist");
        assert_eq!(openai.default_model, "example-main");
        assert_eq!(openai.curated_models.len(), 1);
        let gemma = merged
            .profiles
            .iter()
            .find(|profile| {
                profile.provider == "openrouter" && profile.model == "google/gemma-4-31b-it"
            })
            .expect("profile should merge by provider/model key");
        assert_eq!(gemma.context_window_tokens, Some(128_000));
        assert_eq!(gemma.features, vec![ModelFeature::ToolCalling]);
        let embedding = merged
            .embedding_profiles
            .iter()
            .find(|profile| {
                profile.provider == "openrouter" && profile.model == "qwen/qwen3-embedding-8b"
            })
            .expect("embedding profile should merge by provider/model/dimensions key");
        assert_eq!(embedding.recommended_top_k, 6);
        assert!(!embedding.supports_multilingual);
        let alias = merged
            .route_aliases
            .iter()
            .find(|route| route.hint == "gemma31b")
            .expect("route alias should merge by hint");
        assert_eq!(alias.model, "google/gemma-4-31b-preview");
        let policy = merged
            .request_policies
            .iter()
            .find(|policy| policy.provider == "openai-codex" && policy.model == "gpt-5.4")
            .expect("request policy should merge by provider/model key");
        assert_eq!(policy.default_reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn request_policy_resolves_temperature_and_reasoning_aliases() {
        let policy =
            model_request_policy("openai-codex", "gpt-5.4").expect("policy should resolve");
        assert_eq!(
            policy.resolve_reasoning_effort("xhigh"),
            Some("high".into())
        );
        assert_eq!(
            policy.resolve_reasoning_effort("minimal"),
            Some("low".into())
        );

        let policy = model_request_policy("openai", "o3").expect("policy should resolve");
        assert_eq!(policy.fixed_temperature, Some(1.0));

        let policy =
            model_request_policy("openrouter", "x-ai/grok-4.20").expect("policy should resolve");
        assert_eq!(
            policy.resolve_reasoning_effort("xhigh"),
            Some("high".into())
        );
    }
}
