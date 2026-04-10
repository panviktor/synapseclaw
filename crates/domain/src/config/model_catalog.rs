use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::schema::{ModelLaneCandidateConfig, ModelLaneConfig, ModelPricing};

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
}

#[derive(Debug, Clone, Deserialize)]
struct ModelPresetCatalogEntry {
    id: String,
    title: String,
    description: String,
    default_provider: String,
    default_model: String,
    #[serde(default)]
    seed_multimodal_from_reasoning: bool,
    #[serde(default)]
    extra_lanes: Vec<ModelLaneConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProviderModelCatalogEntry {
    provider: String,
    default_model: String,
    #[serde(default)]
    curated_models: Vec<CuratedModelDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelPricingCatalogEntry {
    model: String,
    input: f64,
    output: f64,
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
    fn provider_defaults_come_from_catalog_data() {
        assert_eq!(provider_default_model("openai-codex"), Some("gpt-5.4"));
        assert_eq!(provider_default_model("ollama"), Some("llama3.2"));
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
    }
}
