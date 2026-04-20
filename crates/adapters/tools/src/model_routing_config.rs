use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use synapse_domain::application::services::model_preset_resolution::{
    known_model_presets, normalize_model_preset_id, preset_description, preset_reasoning_seed,
    preset_title, resolve_effective_model_lanes,
};
use synapse_domain::config::schema::{
    CapabilityLane, ClassificationRule, Config, DelegateAgentConfig, ModelCandidateProfileConfig,
    ModelLaneCandidateConfig, ModelLaneConfig,
};
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    RoutingAction, RoutingFact, ToolFactPayload, TypedToolFact,
};
use synapse_domain::domain::util::MaybeSet;
use synapse_domain::ports::tool::{
    ToolArgumentPolicy, ToolContract, ToolNonReplayableReason, ToolRuntimeRole,
};
use synapse_infra::config_io::ConfigIO;

const DEFAULT_AGENT_MAX_DEPTH: u32 = 3;
const DEFAULT_AGENT_MAX_ITERATIONS: usize = 10;

pub struct ModelRoutingConfigTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
}

impl ModelRoutingConfigTool {
    pub fn new(config: Arc<Config>, security: Arc<SecurityPolicy>) -> Self {
        Self { config, security }
    }

    fn load_config_without_env(&self) -> anyhow::Result<Config> {
        let contents = fs::read_to_string(&self.config.config_path).map_err(|error| {
            anyhow::anyhow!(
                "Failed to read config file {}: {error}",
                self.config.config_path.display()
            )
        })?;

        let mut parsed: Config = toml::from_str(&contents).map_err(|error| {
            anyhow::anyhow!(
                "Failed to parse config file {}: {error}",
                self.config.config_path.display()
            )
        })?;
        parsed.config_path = self.config.config_path.clone();
        parsed.workspace_dir = self.config.workspace_dir.clone();
        Ok(parsed)
    }

    fn require_write_access(&self) -> Option<ToolResult> {
        if !self.security.can_act() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: autonomy is read-only".into()),
            });
        }

        if !self.security.record_action() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: rate limit exceeded".into()),
            });
        }

        None
    }

    fn parse_string_list(raw: &Value, field: &str) -> anyhow::Result<Vec<String>> {
        if let Some(raw_string) = raw.as_str() {
            return Ok(raw_string
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(ToOwned::to_owned)
                .collect());
        }

        if let Some(array) = raw.as_array() {
            let mut out = Vec::new();
            for item in array {
                let value = item
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("'{field}' array must only contain strings"))?;
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            return Ok(out);
        }

        anyhow::bail!("'{field}' must be a string or string[]")
    }

    fn parse_non_empty_string(args: &Value, field: &str) -> anyhow::Result<String> {
        let value = args
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing '{field}'"))?
            .trim();

        if value.is_empty() {
            anyhow::bail!("'{field}' must not be empty");
        }

        Ok(value.to_string())
    }

    fn parse_optional_string_update(args: &Value, field: &str) -> anyhow::Result<MaybeSet<String>> {
        let Some(raw) = args.get(field) else {
            return Ok(MaybeSet::Unset);
        };

        if raw.is_null() {
            return Ok(MaybeSet::Null);
        }

        let value = raw
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("'{field}' must be a string or null"))?
            .trim()
            .to_string();

        let output = if value.is_empty() {
            MaybeSet::Null
        } else {
            MaybeSet::Set(value)
        };
        Ok(output)
    }

    fn parse_optional_f64_update(args: &Value, field: &str) -> anyhow::Result<MaybeSet<f64>> {
        let Some(raw) = args.get(field) else {
            return Ok(MaybeSet::Unset);
        };

        if raw.is_null() {
            return Ok(MaybeSet::Null);
        }

        let value = raw
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("'{field}' must be a number or null"))?;
        Ok(MaybeSet::Set(value))
    }

    fn parse_optional_usize_update(args: &Value, field: &str) -> anyhow::Result<MaybeSet<usize>> {
        let Some(raw) = args.get(field) else {
            return Ok(MaybeSet::Unset);
        };

        if raw.is_null() {
            return Ok(MaybeSet::Null);
        }

        let raw_value = raw
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("'{field}' must be a non-negative integer or null"))?;
        let value = usize::try_from(raw_value)
            .map_err(|_| anyhow::anyhow!("'{field}' is too large for this platform"))?;
        Ok(MaybeSet::Set(value))
    }

    fn parse_optional_u32_update(args: &Value, field: &str) -> anyhow::Result<MaybeSet<u32>> {
        let Some(raw) = args.get(field) else {
            return Ok(MaybeSet::Unset);
        };

        if raw.is_null() {
            return Ok(MaybeSet::Null);
        }

        let raw_value = raw
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("'{field}' must be a non-negative integer or null"))?;
        let value =
            u32::try_from(raw_value).map_err(|_| anyhow::anyhow!("'{field}' must fit in u32"))?;
        Ok(MaybeSet::Set(value))
    }

    fn parse_optional_i32_update(args: &Value, field: &str) -> anyhow::Result<MaybeSet<i32>> {
        let Some(raw) = args.get(field) else {
            return Ok(MaybeSet::Unset);
        };

        if raw.is_null() {
            return Ok(MaybeSet::Null);
        }

        let raw_value = raw
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("'{field}' must be an integer or null"))?;
        let value =
            i32::try_from(raw_value).map_err(|_| anyhow::anyhow!("'{field}' must fit in i32"))?;
        Ok(MaybeSet::Set(value))
    }

    fn parse_optional_bool(args: &Value, field: &str) -> anyhow::Result<Option<bool>> {
        let Some(raw) = args.get(field) else {
            return Ok(None);
        };

        let value = raw
            .as_bool()
            .ok_or_else(|| anyhow::anyhow!("'{field}' must be a boolean"))?;
        Ok(Some(value))
    }

    fn scenario_row(rule: &ClassificationRule, cfg: &Config) -> Value {
        let resolved =
            synapse_domain::application::services::inbound_message_service::resolve_model_command_route(
                &rule.hint,
                cfg,
            );
        json!({
            "selector": rule.hint,
            "resolved_route": resolved.map(|route| json!({
                "provider": route.provider,
                "model": route.model,
                "lane": route.lane.map(Self::lane_name),
                "candidate_index": route.candidate_index,
            })),
            "classification": {
                "keywords": rule.keywords,
                "patterns": rule.patterns,
                "min_length": rule.min_length,
                "max_length": rule.max_length,
                "priority": rule.priority,
            },
        })
    }

    fn lane_name(lane: CapabilityLane) -> &'static str {
        match lane {
            CapabilityLane::Reasoning => "reasoning",
            CapabilityLane::CheapReasoning => "cheap_reasoning",
            CapabilityLane::Compaction => "compaction",
            CapabilityLane::Embedding => "embedding",
            CapabilityLane::WebExtraction => "web_extraction",
            CapabilityLane::ToolValidator => "tool_validator",
            CapabilityLane::ImageGeneration => "image_generation",
            CapabilityLane::AudioGeneration => "audio_generation",
            CapabilityLane::VideoGeneration => "video_generation",
            CapabilityLane::MusicGeneration => "music_generation",
            CapabilityLane::MultimodalUnderstanding => "multimodal_understanding",
        }
    }

    fn effective_lane_rows(cfg: &Config) -> Vec<Value> {
        resolve_effective_model_lanes(cfg)
            .into_iter()
            .map(|lane| {
                json!({
                    "lane": Self::lane_name(lane.lane),
                    "candidates": lane.candidates.into_iter().map(|candidate| {
                        json!({
                            "provider": candidate.provider,
                            "model": candidate.model,
                            "api_key_env": candidate.api_key_env,
                            "dimensions": candidate.dimensions,
                            "profile": {
                                "context_window_tokens": candidate.profile.context_window_tokens,
                                "max_output_tokens": candidate.profile.max_output_tokens,
                                "features": candidate.profile.features,
                            }
                        })
                    }).collect::<Vec<_>>(),
                })
            })
            .collect()
    }

    fn snapshot(cfg: &Config) -> Value {
        let mut rules = cfg.query_classification.rules.clone();
        rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.hint.cmp(&b.hint))
        });

        let scenarios = rules
            .iter()
            .map(|rule| Self::scenario_row(rule, cfg))
            .collect::<Vec<_>>();

        let unresolved_classification_rules: Vec<Value> = rules
            .iter()
            .filter(|rule| {
                synapse_domain::application::services::inbound_message_service::resolve_model_command_route(
                    &rule.hint,
                    cfg,
                )
                .is_none()
            })
            .map(|rule| {
                json!({
                    "selector": rule.hint,
                    "keywords": rule.keywords,
                    "patterns": rule.patterns,
                    "min_length": rule.min_length,
                    "max_length": rule.max_length,
                    "priority": rule.priority,
                })
            })
            .collect();

        let mut agents: BTreeMap<String, Value> = BTreeMap::new();
        for (name, agent) in &cfg.agents {
            agents.insert(
                name.clone(),
                json!({
                    "provider": agent.provider,
                    "model": agent.model,
                    "system_prompt": agent.system_prompt,
                    "api_key_configured": agent
                        .api_key
                        .as_ref()
                        .is_some_and(|value| !value.trim().is_empty()),
                    "temperature": agent.temperature,
                    "max_depth": agent.max_depth,
                    "agentic": agent.agentic,
                    "allowed_tools": agent.allowed_tools,
                    "max_iterations": agent.max_iterations,
                }),
            );
        }

        json!({
            "default": {
                "provider": cfg.default_provider,
                "model": cfg.default_model,
                "temperature": cfg.default_temperature,
            },
            "routing": {
                "preset": cfg.model_preset,
                "preset_title": cfg.model_preset.as_deref().and_then(preset_title),
                "preset_description": cfg.model_preset.as_deref().and_then(preset_description),
                "effective_lanes": Self::effective_lane_rows(cfg),
            },
            "query_classification": {
                "enabled": cfg.query_classification.enabled,
                "rules_count": cfg.query_classification.rules.len(),
            },
            "scenarios": scenarios,
            "unresolved_classification_rules": unresolved_classification_rules,
            "agents": agents,
        })
    }

    fn normalize_and_sort_rules(rules: &mut Vec<ClassificationRule>) {
        rules.retain(|rule| !rule.hint.trim().is_empty());
        rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.hint.cmp(&b.hint))
        });
    }

    fn has_rule_matcher(rule: &ClassificationRule) -> bool {
        !rule.keywords.is_empty()
            || !rule.patterns.is_empty()
            || rule.min_length.is_some()
            || rule.max_length.is_some()
    }

    fn ensure_rule_defaults(rule: &mut ClassificationRule, hint: &str) {
        if !Self::has_rule_matcher(rule) {
            rule.keywords = vec![hint.to_string()];
        }
    }

    fn parse_lane_selector(args: &Value) -> anyhow::Result<CapabilityLane> {
        let raw = args
            .get("lane")
            .or_else(|| args.get("hint"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Missing 'lane'"))?
            .trim();
        raw.parse::<CapabilityLane>().map_err(|_| {
            anyhow::anyhow!(
                "'lane' must be one of: {}",
                CapabilityLane::ALL
                    .into_iter()
                    .map(Self::lane_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    fn upsert_lane_candidate(
        cfg: &mut Config,
        lane: CapabilityLane,
        provider: String,
        model: String,
        api_key_update: MaybeSet<String>,
    ) {
        let existing = cfg
            .model_lanes
            .iter()
            .find(|entry| entry.lane == lane)
            .and_then(|entry| {
                entry
                    .candidates
                    .iter()
                    .find(|candidate| candidate.provider == provider && candidate.model == model)
                    .cloned()
            });
        let mut candidate = existing.unwrap_or_else(|| ModelLaneCandidateConfig {
            provider: provider.clone(),
            model: model.clone(),
            api_key: None,
            api_key_env: None,
            dimensions: None,
            profile: ModelCandidateProfileConfig::default(),
        });

        candidate.provider = provider;
        candidate.model = model;
        match api_key_update {
            MaybeSet::Set(api_key) => candidate.api_key = Some(api_key),
            MaybeSet::Null => candidate.api_key = None,
            MaybeSet::Unset => {}
        }

        if let Some(entry) = cfg.model_lanes.iter_mut().find(|entry| entry.lane == lane) {
            entry.candidates.retain(|existing| {
                existing.provider != candidate.provider || existing.model != candidate.model
            });
            entry.candidates.insert(0, candidate);
        } else {
            cfg.model_lanes.push(ModelLaneConfig {
                lane,
                candidates: vec![candidate],
            });
        }
        cfg.model_lanes
            .sort_by(|left, right| left.lane.as_str().cmp(right.lane.as_str()));
    }

    fn handle_get(&self) -> anyhow::Result<ToolResult> {
        let cfg = self.load_config_without_env()?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&Self::snapshot(&cfg))?,
            error: None,
        })
    }

    fn handle_list_hints(&self) -> anyhow::Result<ToolResult> {
        let cfg = self.load_config_without_env()?;
        let lane_selectors: Vec<&'static str> = CapabilityLane::ALL
            .into_iter()
            .map(Self::lane_name)
            .collect();
        let mut catalog_aliases: Vec<String> =
            synapse_domain::config::model_catalog::route_aliases()
                .into_iter()
                .map(|alias| alias.hint)
                .collect();
        catalog_aliases.sort();
        catalog_aliases.dedup();

        let mut classification_hints: Vec<String> = cfg
            .query_classification
            .rules
            .iter()
            .map(|r| r.hint.clone())
            .collect();
        classification_hints.sort();
        classification_hints.dedup();

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "lane_selectors": lane_selectors,
                "catalog_aliases": catalog_aliases,
                "classification_hints": classification_hints,
                "example": {
                    "reasoning_lane": {
                        "action": "upsert_scenario",
                        "hint": "reasoning",
                        "provider": "example-provider",
                        "model": "example-reasoning-model",
                        "classification_enabled": false
                    },
                    "cheap_lane": {
                        "action": "upsert_scenario",
                        "hint": "cheap_reasoning",
                        "provider": "test-provider",
                        "model": "test-fast-model",
                        "classification_enabled": true,
                        "keywords": ["summarize", "short"],
                        "priority": 50
                    }
                }
            }))?,
            error: None,
        })
    }

    fn handle_list_presets(&self) -> anyhow::Result<ToolResult> {
        let cfg = self.load_config_without_env()?;
        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "current_preset": cfg.model_preset,
                "presets": known_model_presets().iter().map(|preset| {
                    json!({
                        "id": preset.id,
                        "title": preset.title,
                        "description": preset.description,
                        "default_reasoning_seed": preset_reasoning_seed(&preset.id).map(|(provider, model)| {
                            json!({
                                "provider": provider,
                                "model": model,
                            })
                        }),
                    })
                }).collect::<Vec<_>>(),
            }))?,
            error: None,
        })
    }

    async fn handle_set_preset(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let preset_update = Self::parse_optional_string_update(args, "preset")?;
        let sync_defaults = args
            .get("sync_defaults")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let mut cfg = self.load_config_without_env()?;

        match preset_update {
            MaybeSet::Unset => anyhow::bail!("set_preset requires 'preset' (string or null)"),
            MaybeSet::Null => {
                cfg.model_preset = None;
            }
            MaybeSet::Set(preset) => {
                let normalized = normalize_model_preset_id(&preset)
                    .ok_or_else(|| anyhow::anyhow!("Unknown preset '{preset}'"))?;
                cfg.model_preset = Some(normalized.to_string());
                if sync_defaults {
                    if let Some((provider, model)) = preset_reasoning_seed(normalized) {
                        cfg.default_provider = Some(provider.to_string());
                        cfg.default_model = Some(model.to_string());
                    }
                }
            }
        }

        cfg.save().await?;

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "message": "Model preset updated",
                "config": Self::snapshot(&cfg),
            }))?,
            error: None,
        })
    }

    async fn handle_set_default(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let provider_update = Self::parse_optional_string_update(args, "provider")?;
        let model_update = Self::parse_optional_string_update(args, "model")?;
        let temperature_update = Self::parse_optional_f64_update(args, "temperature")?;

        let any_update = !matches!(provider_update, MaybeSet::Unset)
            || !matches!(model_update, MaybeSet::Unset)
            || !matches!(temperature_update, MaybeSet::Unset);

        if !any_update {
            anyhow::bail!("set_default requires at least one of: provider, model, temperature");
        }

        let mut cfg = self.load_config_without_env()?;

        // Capture previous values for rollback on probe failure.
        let previous_provider = cfg.default_provider.clone();
        let previous_model = cfg.default_model.clone();
        let previous_temperature = cfg.default_temperature;

        match provider_update {
            MaybeSet::Set(provider) => cfg.default_provider = Some(provider),
            MaybeSet::Null => cfg.default_provider = None,
            MaybeSet::Unset => {}
        }

        match model_update {
            MaybeSet::Set(model) => cfg.default_model = Some(model),
            MaybeSet::Null => cfg.default_model = None,
            MaybeSet::Unset => {}
        }

        match temperature_update {
            MaybeSet::Set(temperature) => {
                if !(0.0..=2.0).contains(&temperature) {
                    anyhow::bail!("'temperature' must be between 0.0 and 2.0");
                }
                cfg.default_temperature = temperature;
            }
            MaybeSet::Null => {
                cfg.default_temperature = Config::default().default_temperature;
            }
            MaybeSet::Unset => {}
        }

        cfg.save().await?;

        // Probe the new model with a minimal API call to catch invalid model IDs
        // before the channel hot-reload picks up the change.
        if let (Some(provider_name), Some(model_name)) =
            (cfg.default_provider.clone(), cfg.default_model.clone())
        {
            if let Err(probe_err) = self.probe_model(&provider_name, &model_name).await {
                if synapse_providers::reliable::is_non_retryable(&probe_err) {
                    let reverted_model = previous_model.as_deref().unwrap_or("(none)").to_string();

                    // Rollback to previous config.
                    cfg.default_provider = previous_provider;
                    cfg.default_model = previous_model;
                    cfg.default_temperature = previous_temperature;
                    cfg.save().await?;

                    return Ok(ToolResult {
                        success: false,
                        output: format!(
                            "Model '{model_name}' is not available: {probe_err}. Reverted to '{reverted_model}'.",
                        ),
                        error: None,
                    });
                }
                // Retryable errors (e.g. transient network issues) — keep the
                // new config and let the resilient wrapper handle retries.
                tracing::warn!(
                    model = %model_name,
                    "Model probe returned retryable error (keeping new config): {probe_err}"
                );
            }
        }

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "message": "Default provider/model settings updated",
                "config": Self::snapshot(&cfg),
            }))?,
            error: None,
        })
    }

    /// Send a minimal 1-token chat request to verify the model is accessible.
    /// Returns `Ok(())` if the probe succeeds **or** if no API key is available
    /// (the probe would fail with an auth error unrelated to model validity).
    /// Provider construction failures are also treated as non-fatal.
    async fn probe_model(&self, provider_name: &str, model: &str) -> anyhow::Result<()> {
        // Use the runtime config's API key (which includes env-sourced keys),
        // not the on-disk config (which may have no key at all).
        let api_key = self.config.api_key.as_deref();
        if api_key.is_none_or(|k| k.trim().is_empty()) {
            return Ok(());
        }

        let provider_runtime_options =
            synapse_providers::provider_runtime_options_from_config(&self.config);
        let provider = match synapse_providers::create_provider_with_url_and_options(
            provider_name,
            api_key,
            self.config.api_url.as_deref(),
            &provider_runtime_options,
        ) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };

        provider
            .chat_with_system(Some("Respond with OK."), "ping", model, 0.0)
            .await?;

        Ok(())
    }

    async fn handle_upsert_scenario(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let lane = Self::parse_lane_selector(args)?;
        let selector = Self::lane_name(lane).to_string();
        let provider = Self::parse_non_empty_string(args, "provider")?;
        let model = Self::parse_non_empty_string(args, "model")?;
        let api_key_update = Self::parse_optional_string_update(args, "api_key")?;

        let keywords_update = if let Some(raw) = args.get("keywords") {
            Some(Self::parse_string_list(raw, "keywords")?)
        } else {
            None
        };
        let patterns_update = if let Some(raw) = args.get("patterns") {
            Some(Self::parse_string_list(raw, "patterns")?)
        } else {
            None
        };
        let min_length_update = Self::parse_optional_usize_update(args, "min_length")?;
        let max_length_update = Self::parse_optional_usize_update(args, "max_length")?;
        let priority_update = Self::parse_optional_i32_update(args, "priority")?;
        let classification_enabled = Self::parse_optional_bool(args, "classification_enabled")?;

        let should_touch_rule = classification_enabled.is_some()
            || keywords_update.is_some()
            || patterns_update.is_some()
            || !matches!(min_length_update, MaybeSet::Unset)
            || !matches!(max_length_update, MaybeSet::Unset)
            || !matches!(priority_update, MaybeSet::Unset);

        let mut cfg = self.load_config_without_env()?;
        Self::upsert_lane_candidate(&mut cfg, lane, provider, model, api_key_update);

        if should_touch_rule {
            if matches!(classification_enabled, Some(false)) {
                cfg.query_classification
                    .rules
                    .retain(|rule| rule.hint != selector);
            } else {
                let existing_rule = cfg
                    .query_classification
                    .rules
                    .iter()
                    .find(|rule| rule.hint == selector)
                    .cloned();

                let mut next_rule = existing_rule.unwrap_or_else(|| ClassificationRule {
                    hint: selector.clone(),
                    ..ClassificationRule::default()
                });
                next_rule.hint = selector.clone();

                if let Some(keywords) = keywords_update {
                    next_rule.keywords = keywords;
                }
                if let Some(patterns) = patterns_update {
                    next_rule.patterns = patterns;
                }

                match min_length_update {
                    MaybeSet::Set(value) => next_rule.min_length = Some(value),
                    MaybeSet::Null => next_rule.min_length = None,
                    MaybeSet::Unset => {}
                }

                match max_length_update {
                    MaybeSet::Set(value) => next_rule.max_length = Some(value),
                    MaybeSet::Null => next_rule.max_length = None,
                    MaybeSet::Unset => {}
                }

                match priority_update {
                    MaybeSet::Set(value) => next_rule.priority = value,
                    MaybeSet::Null => next_rule.priority = 0,
                    MaybeSet::Unset => {}
                }

                if matches!(classification_enabled, Some(true)) {
                    Self::ensure_rule_defaults(&mut next_rule, &selector);
                }

                if !Self::has_rule_matcher(&next_rule) {
                    anyhow::bail!(
                        "Classification rule for selector '{selector}' has no matching criteria. Provide keywords/patterns or set min_length/max_length."
                    );
                }

                cfg.query_classification
                    .rules
                    .retain(|rule| rule.hint != selector);
                cfg.query_classification.rules.push(next_rule);
            }
        }

        Self::normalize_and_sort_rules(&mut cfg.query_classification.rules);
        cfg.query_classification.enabled = !cfg.query_classification.rules.is_empty();

        cfg.save().await?;

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "message": "Scenario lane candidate upserted",
                "selector": selector,
                "lane": Self::lane_name(lane),
                "config": Self::snapshot(&cfg),
            }))?,
            error: None,
        })
    }

    async fn handle_remove_scenario(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let lane = Self::parse_lane_selector(args)?;
        let selector = Self::lane_name(lane).to_string();
        let remove_classification = args
            .get("remove_classification")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let mut cfg = self.load_config_without_env()?;

        let before_lanes = cfg.model_lanes.len();
        cfg.model_lanes.retain(|entry| entry.lane != lane);
        let lanes_removed = before_lanes.saturating_sub(cfg.model_lanes.len());

        let mut rules_removed = 0usize;
        if remove_classification {
            let before_rules = cfg.query_classification.rules.len();
            cfg.query_classification
                .rules
                .retain(|rule| rule.hint != selector);
            rules_removed = before_rules.saturating_sub(cfg.query_classification.rules.len());
        }

        if lanes_removed == 0 && rules_removed == 0 {
            anyhow::bail!("No scenario found for selector '{selector}'");
        }

        Self::normalize_and_sort_rules(&mut cfg.query_classification.rules);
        cfg.query_classification.enabled = !cfg.query_classification.rules.is_empty();

        cfg.save().await?;

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "message": "Scenario removed",
                "selector": selector,
                "lane": Self::lane_name(lane),
                "lanes_removed": lanes_removed,
                "classification_rules_removed": rules_removed,
                "config": Self::snapshot(&cfg),
            }))?,
            error: None,
        })
    }

    async fn handle_upsert_agent(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let name = Self::parse_non_empty_string(args, "name")?;
        let provider = Self::parse_non_empty_string(args, "provider")?;
        let model = Self::parse_non_empty_string(args, "model")?;

        let system_prompt_update = Self::parse_optional_string_update(args, "system_prompt")?;
        let api_key_update = Self::parse_optional_string_update(args, "api_key")?;
        let temperature_update = Self::parse_optional_f64_update(args, "temperature")?;
        let max_depth_update = Self::parse_optional_u32_update(args, "max_depth")?;
        let max_iterations_update = Self::parse_optional_usize_update(args, "max_iterations")?;
        let agentic_update = Self::parse_optional_bool(args, "agentic")?;

        let allowed_tools_update = if let Some(raw) = args.get("allowed_tools") {
            Some(Self::parse_string_list(raw, "allowed_tools")?)
        } else {
            None
        };

        let mut cfg = self.load_config_without_env()?;

        let mut next_agent = cfg
            .agents
            .get(&name)
            .cloned()
            .unwrap_or(DelegateAgentConfig {
                provider: provider.clone(),
                model: model.clone(),
                system_prompt: None,
                api_key: None,
                temperature: None,
                max_depth: DEFAULT_AGENT_MAX_DEPTH,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: DEFAULT_AGENT_MAX_ITERATIONS,
            });

        next_agent.provider = provider;
        next_agent.model = model;

        match system_prompt_update {
            MaybeSet::Set(value) => next_agent.system_prompt = Some(value),
            MaybeSet::Null => next_agent.system_prompt = None,
            MaybeSet::Unset => {}
        }

        match api_key_update {
            MaybeSet::Set(value) => next_agent.api_key = Some(value),
            MaybeSet::Null => next_agent.api_key = None,
            MaybeSet::Unset => {}
        }

        match temperature_update {
            MaybeSet::Set(value) => {
                if !(0.0..=2.0).contains(&value) {
                    anyhow::bail!("'temperature' must be between 0.0 and 2.0");
                }
                next_agent.temperature = Some(value);
            }
            MaybeSet::Null => next_agent.temperature = None,
            MaybeSet::Unset => {}
        }

        match max_depth_update {
            MaybeSet::Set(value) => next_agent.max_depth = value,
            MaybeSet::Null => next_agent.max_depth = DEFAULT_AGENT_MAX_DEPTH,
            MaybeSet::Unset => {}
        }

        match max_iterations_update {
            MaybeSet::Set(value) => next_agent.max_iterations = value,
            MaybeSet::Null => next_agent.max_iterations = DEFAULT_AGENT_MAX_ITERATIONS,
            MaybeSet::Unset => {}
        }

        if let Some(agentic) = agentic_update {
            next_agent.agentic = agentic;
        }

        if let Some(allowed_tools) = allowed_tools_update {
            next_agent.allowed_tools = allowed_tools;
        }

        if next_agent.max_depth == 0 {
            anyhow::bail!("'max_depth' must be greater than 0");
        }

        if next_agent.max_iterations == 0 {
            anyhow::bail!("'max_iterations' must be greater than 0");
        }

        if next_agent.agentic && next_agent.allowed_tools.is_empty() {
            anyhow::bail!(
                "Agent '{name}' has agentic=true but allowed_tools is empty. Set allowed_tools or disable agentic mode."
            );
        }

        cfg.agents.insert(name.clone(), next_agent);
        cfg.save().await?;

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "message": "Delegate agent upserted",
                "name": name,
                "config": Self::snapshot(&cfg),
            }))?,
            error: None,
        })
    }

    async fn handle_remove_agent(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let name = Self::parse_non_empty_string(args, "name")?;

        let mut cfg = self.load_config_without_env()?;
        if cfg.agents.remove(&name).is_none() {
            anyhow::bail!("No delegate agent found with name '{name}'");
        }

        cfg.save().await?;

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "message": "Delegate agent removed",
                "name": name,
                "config": Self::snapshot(&cfg),
            }))?,
            error: None,
        })
    }
}

#[async_trait]
impl Tool for ModelRoutingConfigTool {
    fn name(&self) -> &str {
        "model_routing_config"
    }

    fn description(&self) -> &str {
        "Manage default model settings, capability-lane candidates, classification rules, and delegate sub-agent profiles"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "get",
                        "list_hints",
                        "list_presets",
                        "set_preset",
                        "set_default",
                        "upsert_scenario",
                        "remove_scenario",
                        "upsert_agent",
                        "remove_agent"
                    ],
                    "default": "get"
                },
                "hint": {
                    "type": "string",
                    "description": "Capability-lane selector for scenario routing (for example: reasoning, cheap_reasoning, image_generation). Prefer 'lane' for new calls."
                },
                "lane": {
                    "type": "string",
                    "description": "Capability lane for upsert_scenario/remove_scenario (reasoning, cheap_reasoning, embedding, multimodal_understanding, image_generation, audio_generation, video_generation, music_generation)"
                },
                "preset": {
                    "type": ["string", "null"],
                    "description": "Preset id for set_preset (for example: chatgpt, claude, openrouter, gemini, local)"
                },
                "sync_defaults": {
                    "type": "boolean",
                    "description": "When set_preset, also sync default provider/model to the preset's reasoning seed (default true)"
                },
                "provider": {
                    "type": "string",
                    "description": "Provider for set_default/upsert_scenario/upsert_agent"
                },
                "model": {
                    "type": "string",
                    "description": "Model for set_default/upsert_scenario/upsert_agent"
                },
                "temperature": {
                    "type": ["number", "null"],
                    "description": "Optional temperature override (0.0-2.0)"
                },
                "api_key": {
                    "type": ["string", "null"],
                    "description": "Optional API key override for lane candidate or delegate agent"
                },
                "keywords": {
                    "description": "Classification keywords for upsert_scenario (string or string array)",
                    "oneOf": [
                        {"type": "string"},
                        {"type": "array", "items": {"type": "string"}}
                    ]
                },
                "patterns": {
                    "description": "Classification literal patterns for upsert_scenario (string or string array)",
                    "oneOf": [
                        {"type": "string"},
                        {"type": "array", "items": {"type": "string"}}
                    ]
                },
                "min_length": {
                    "type": ["integer", "null"],
                    "minimum": 0,
                    "description": "Optional minimum message length matcher"
                },
                "max_length": {
                    "type": ["integer", "null"],
                    "minimum": 0,
                    "description": "Optional maximum message length matcher"
                },
                "priority": {
                    "type": ["integer", "null"],
                    "description": "Classification priority (higher runs first)"
                },
                "classification_enabled": {
                    "type": "boolean",
                    "description": "When true, upsert classification rule for this lane selector; false removes it"
                },
                "remove_classification": {
                    "type": "boolean",
                    "description": "When remove_scenario, whether to remove matching classification rule (default true)"
                },
                "name": {
                    "type": "string",
                    "description": "Delegate sub-agent name for upsert_agent/remove_agent"
                },
                "system_prompt": {
                    "type": ["string", "null"],
                    "description": "Optional system prompt override for delegate agent"
                },
                "max_depth": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "description": "Delegate max recursion depth"
                },
                "agentic": {
                    "type": "boolean",
                    "description": "Enable tool-call loop mode for delegate agent"
                },
                "allowed_tools": {
                    "description": "Allowed tools for agentic delegate mode (string or string array)",
                    "oneOf": [
                        {"type": "string"},
                        {"type": "array", "items": {"type": "string"}}
                    ]
                },
                "max_iterations": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "description": "Maximum tool-call iterations for agentic delegate mode"
                }
            },
            "additionalProperties": false
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::RuntimeStateInspection)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(self.runtime_role(), ToolNonReplayableReason::MutatesState)
            .with_arguments(vec![
                ToolArgumentPolicy::replayable("action"),
                ToolArgumentPolicy::replayable("hint"),
                ToolArgumentPolicy::replayable("lane"),
                ToolArgumentPolicy::replayable("preset"),
                ToolArgumentPolicy::replayable("sync_defaults"),
                ToolArgumentPolicy::sensitive("provider").user_private(),
                ToolArgumentPolicy::sensitive("model").user_private(),
                ToolArgumentPolicy::replayable("temperature"),
                ToolArgumentPolicy::sensitive("api_key").secret(),
                ToolArgumentPolicy::sensitive("keywords").user_private(),
                ToolArgumentPolicy::sensitive("patterns").user_private(),
                ToolArgumentPolicy::replayable("min_length"),
                ToolArgumentPolicy::replayable("max_length"),
                ToolArgumentPolicy::replayable("priority"),
                ToolArgumentPolicy::replayable("classification_enabled"),
                ToolArgumentPolicy::replayable("remove_classification"),
                ToolArgumentPolicy::sensitive("name").user_private(),
                ToolArgumentPolicy::sensitive("system_prompt").user_private(),
                ToolArgumentPolicy::replayable("max_depth"),
                ToolArgumentPolicy::replayable("agentic"),
                ToolArgumentPolicy::sensitive("allowed_tools").user_private(),
                ToolArgumentPolicy::replayable("max_iterations"),
            ])
    }

    fn extract_facts(&self, args: &Value, result: Option<&ToolResult>) -> Vec<TypedToolFact> {
        if matches!(result, Some(result) if !result.success) {
            return Vec::new();
        }

        let action = match args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("get")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "get" => RoutingAction::Get,
            "list_hints" => RoutingAction::ListHints,
            "list_presets" => RoutingAction::ListPresets,
            "set_preset" => RoutingAction::SetPreset,
            "set_default" => RoutingAction::SetDefault,
            "upsert_scenario" => RoutingAction::UpsertScenario,
            "remove_scenario" => RoutingAction::RemoveScenario,
            "upsert_agent" => RoutingAction::UpsertAgent,
            "remove_agent" => RoutingAction::RemoveAgent,
            _ => return Vec::new(),
        };

        vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Routing(RoutingFact {
                action,
                preset: args
                    .get("preset")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                hint: args
                    .get("lane")
                    .or_else(|| args.get("hint"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                agent_name: args
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                provider: args
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                model: args
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                matcher_count: args.get("patterns").and_then(Value::as_array).map(Vec::len),
                allowed_tool_count: match args.get("allowed_tools") {
                    Some(Value::Array(values)) => Some(values.len()),
                    Some(Value::String(value)) if !value.trim().is_empty() => Some(1),
                    _ => None,
                },
            }),
        }]
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("get")
            .to_ascii_lowercase();

        let result = match action.as_str() {
            "get" => self.handle_get(),
            "list_hints" => self.handle_list_hints(),
            "list_presets" => self.handle_list_presets(),
            "set_default"
            | "set_preset"
            | "upsert_scenario"
            | "remove_scenario"
            | "upsert_agent"
            | "remove_agent" => {
                if let Some(blocked) = self.require_write_access() {
                    return Ok(blocked);
                }

                match action.as_str() {
                    "set_preset" => Box::pin(self.handle_set_preset(&args)).await,
                    "set_default" => Box::pin(self.handle_set_default(&args)).await,
                    "upsert_scenario" => Box::pin(self.handle_upsert_scenario(&args)).await,
                    "remove_scenario" => Box::pin(self.handle_remove_scenario(&args)).await,
                    "upsert_agent" => Box::pin(self.handle_upsert_agent(&args)).await,
                    "remove_agent" => Box::pin(self.handle_remove_agent(&args)).await,
                    _ => unreachable!("validated above"),
                }
            }
            _ => anyhow::bail!(
                "Unknown action '{action}'. Valid: get, list_hints, list_presets, set_preset, set_default, upsert_scenario, remove_scenario, upsert_agent, remove_agent"
            ),
        };

        match result {
            Ok(outcome) => Ok(outcome),
            Err(error) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::config::AutonomyLevel;
    use synapse_domain::domain::security_policy::SecurityPolicy;
    use tempfile::TempDir;

    fn test_security() -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    fn readonly_security() -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::ReadOnly,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    async fn test_config(tmp: &TempDir) -> Arc<Config> {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        config.save().await.unwrap();
        Arc::new(config)
    }

    #[tokio::test]
    async fn set_default_updates_provider_model_and_temperature() {
        let tmp = TempDir::new().unwrap();
        let tool = ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, test_security());

        let result = tool
            .execute(json!({
                "action": "set_default",
                "provider": "test-provider",
                "model": "test-default-model",
                "temperature": 0.2
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(
            output["config"]["default"]["provider"].as_str(),
            Some("test-provider")
        );
        assert_eq!(
            output["config"]["default"]["model"].as_str(),
            Some("test-default-model")
        );
        assert_eq!(
            output["config"]["default"]["temperature"].as_f64(),
            Some(0.2)
        );
    }

    #[tokio::test]
    async fn upsert_scenario_creates_lane_candidate_and_rule() {
        let tmp = TempDir::new().unwrap();
        let tool = ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, test_security());

        let result = tool
            .execute(json!({
                "action": "upsert_scenario",
                "lane": "reasoning",
                "provider": "test-provider",
                "model": "test-reasoning-model",
                "classification_enabled": true,
                "keywords": ["code", "bug", "refactor"],
                "patterns": ["```"],
                "priority": 50
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);

        let get_result = tool.execute(json!({"action": "get"})).await.unwrap();
        assert!(get_result.success);
        let output: Value = serde_json::from_str(&get_result.output).unwrap();

        assert_eq!(output["query_classification"]["enabled"], json!(true));

        let scenarios = output["scenarios"].as_array().unwrap();
        assert!(scenarios.iter().any(|item| {
            item["selector"] == json!("reasoning")
                && item["resolved_route"]["provider"] == json!("test-provider")
                && item["resolved_route"]["model"] == json!("test-reasoning-model")
                && item["resolved_route"]["lane"] == json!("reasoning")
        }));
    }

    #[tokio::test]
    async fn remove_scenario_also_removes_rule() {
        let tmp = TempDir::new().unwrap();
        let tool = ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, test_security());

        let _ = tool
            .execute(json!({
                "action": "upsert_scenario",
                "lane": "reasoning",
                "provider": "test-provider",
                "model": "test-reasoning-model",
                "classification_enabled": true,
                "keywords": ["code"]
            }))
            .await
            .unwrap();

        let removed = tool
            .execute(json!({
                "action": "remove_scenario",
                "lane": "reasoning"
            }))
            .await
            .unwrap();
        assert!(removed.success, "{:?}", removed.error);

        let get_result = tool.execute(json!({"action": "get"})).await.unwrap();
        let output: Value = serde_json::from_str(&get_result.output).unwrap();
        assert_eq!(output["query_classification"]["enabled"], json!(false));
        assert!(output["scenarios"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn upsert_and_remove_delegate_agent() {
        let tmp = TempDir::new().unwrap();
        let tool = ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, test_security());

        let upsert = tool
            .execute(json!({
                "action": "upsert_agent",
                "name": "coder",
                "provider": "test-provider",
                "model": "test-agent-model",
                "agentic": true,
                "allowed_tools": ["file_read", "file_write", "shell"],
                "max_iterations": 6
            }))
            .await
            .unwrap();
        assert!(upsert.success, "{:?}", upsert.error);

        let get_result = tool.execute(json!({"action": "get"})).await.unwrap();
        let output: Value = serde_json::from_str(&get_result.output).unwrap();
        assert_eq!(
            output["agents"]["coder"]["provider"],
            json!("test-provider")
        );
        assert_eq!(
            output["agents"]["coder"]["model"],
            json!("test-agent-model")
        );
        assert_eq!(output["agents"]["coder"]["agentic"], json!(true));

        let remove = tool
            .execute(json!({
                "action": "remove_agent",
                "name": "coder"
            }))
            .await
            .unwrap();
        assert!(remove.success, "{:?}", remove.error);

        let get_result = tool.execute(json!({"action": "get"})).await.unwrap();
        let output: Value = serde_json::from_str(&get_result.output).unwrap();
        assert!(output["agents"]["coder"].is_null());
    }

    #[tokio::test]
    async fn read_only_mode_blocks_mutating_actions() {
        let tmp = TempDir::new().unwrap();
        let tool =
            ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, readonly_security());

        let result = tool
            .execute(json!({
                "action": "set_default",
                "provider": "test-provider"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("read-only"));
    }

    #[tokio::test]
    async fn set_default_skips_probe_without_api_key() {
        // When no API key is configured (test_config has none), the probe is
        // skipped and any model string is accepted. This verifies the probe-
        // skip path doesn't accidentally reject valid config changes.
        let tmp = TempDir::new().unwrap();
        let tool = ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, test_security());

        let result = tool
            .execute(json!({
                "action": "set_default",
                "provider": "test-provider",
                "model": "totally-fake-model-12345"
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(
            output["config"]["default"]["model"].as_str(),
            Some("totally-fake-model-12345")
        );
    }

    #[tokio::test]
    async fn set_default_temperature_only_skips_probe() {
        // Temperature-only changes don't set a new model, so the probe should
        // not fire at all (no provider/model to probe).
        let tmp = TempDir::new().unwrap();
        let tool = ModelRoutingConfigTool::new(Box::pin(test_config(&tmp)).await, test_security());

        let result = tool
            .execute(json!({
                "action": "set_default",
                "temperature": 1.5
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        let output: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(
            output["config"]["default"]["temperature"].as_f64(),
            Some(1.5)
        );
    }
}
