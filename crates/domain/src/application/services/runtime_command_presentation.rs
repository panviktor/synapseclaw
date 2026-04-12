use crate::application::services::inbound_message_service::CommandEffect;
use crate::application::services::route_switch_preflight::RouteSwitchPreflight;
use crate::config::schema::CapabilityLane;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextBudgetPresentation {
    Compact,
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCommandPresentationOptions {
    pub default_provider: String,
    pub provider_switch_hint: String,
    pub show_lane: bool,
    pub context_budget: ContextBudgetPresentation,
}

impl RuntimeCommandPresentationOptions {
    pub fn new(default_provider: impl Into<String>) -> Self {
        Self {
            default_provider: default_provider.into(),
            provider_switch_hint: "Use `/model <model-id>` or `/model <hint>` to choose a model."
                .to_string(),
            show_lane: true,
            context_budget: ContextBudgetPresentation::Detailed,
        }
    }

    pub fn with_provider_switch_hint(mut self, hint: impl Into<String>) -> Self {
        self.provider_switch_hint = hint.into();
        self
    }

    pub fn with_context_budget(mut self, context_budget: ContextBudgetPresentation) -> Self {
        self.context_budget = context_budget;
        self
    }

    pub fn without_lane(mut self) -> Self {
        self.show_lane = false;
        self
    }
}

pub fn format_common_command_effect(
    effect: &CommandEffect,
    options: &RuntimeCommandPresentationOptions,
) -> Option<String> {
    match effect {
        CommandEffect::ShowProviders | CommandEffect::ShowModel => None,
        CommandEffect::SwitchProvider { provider } => {
            Some(format_switch_provider_success(provider, options))
        }
        CommandEffect::SwitchModel {
            model,
            inferred_provider,
            lane,
            candidate_index: _,
            compacted,
        } => Some(format_switch_model_success(
            model,
            inferred_provider
                .as_deref()
                .unwrap_or(&options.default_provider),
            *lane,
            *compacted,
            options,
        )),
        CommandEffect::SwitchModelBlocked {
            model,
            provider,
            lane,
            preflight,
            compacted,
        } => Some(format_switch_model_blocked(
            model, provider, *lane, preflight, *compacted, options,
        )),
        CommandEffect::ClearSession => Some(format_clear_session_response()),
    }
}

pub fn format_switch_provider_success(
    provider: &str,
    options: &RuntimeCommandPresentationOptions,
) -> String {
    format!(
        "Provider switched to `{provider}`. {}",
        options.provider_switch_hint
    )
}

pub fn format_unknown_provider(provider: &str) -> String {
    format!("Unknown provider `{provider}`. Use `/models` to list valid providers.")
}

pub fn format_provider_initialization_failure(provider: &str, safe_error: &str) -> String {
    format!("Failed to initialize provider `{provider}`: {safe_error}")
}

pub fn format_switch_model_failure(model: &str, provider: &str, safe_error: &str) -> String {
    format!("Model switch to `{model}` (provider: `{provider}`) blocked: {safe_error}")
}

pub fn format_switch_model_success(
    model: &str,
    provider: &str,
    lane: Option<CapabilityLane>,
    compacted: bool,
    options: &RuntimeCommandPresentationOptions,
) -> String {
    if model.is_empty() {
        return "Model ID cannot be empty. Use `/model <model-id>`.".to_string();
    }

    let context_note = if compacted {
        "Context compacted before switching."
    } else {
        "Context preserved."
    };

    if let Some(lane) = visible_lane(lane, options) {
        return format!(
            "Lane `{}` switched to `{provider}:{model}`. {context_note}",
            lane.as_str()
        );
    }

    format!("Model switched to `{model}` (provider: `{provider}`). {context_note}")
}

pub fn format_switch_model_blocked(
    model: &str,
    provider: &str,
    lane: Option<CapabilityLane>,
    preflight: &RouteSwitchPreflight,
    compacted: bool,
    options: &RuntimeCommandPresentationOptions,
) -> String {
    let budget_note = format_context_budget_note(preflight, options.context_budget);
    let compacted_note = if compacted {
        " Compaction ran first, but the context is still too large."
    } else {
        ""
    };

    if let Some(lane) = visible_lane(lane, options) {
        return format!(
            "Lane `{}` route switch to `{provider}:{model}` blocked. {budget_note}{compacted_note}",
            lane.as_str()
        );
    }

    format!(
        "Model switch to `{model}` (provider: `{provider}`) blocked. {budget_note}{compacted_note}"
    )
}

pub fn format_clear_session_response() -> String {
    "Conversation history cleared. Starting fresh.".to_string()
}

fn visible_lane(
    lane: Option<CapabilityLane>,
    options: &RuntimeCommandPresentationOptions,
) -> Option<CapabilityLane> {
    if !options.show_lane {
        return None;
    }

    lane
}

fn format_context_budget_note(
    preflight: &RouteSwitchPreflight,
    presentation: ContextBudgetPresentation,
) -> String {
    let safe_context_budget = preflight
        .safe_context_budget_tokens
        .unwrap_or_else(|| preflight.target_context_window_tokens.unwrap_or(0));
    let mut note = format!(
        "Target safe context budget is ~{safe_context_budget} tokens, current provider-facing context is ~{} tokens.",
        preflight.estimated_context_tokens
    );

    if presentation == ContextBudgetPresentation::Detailed {
        if let Some(target_window) = preflight.target_context_window_tokens {
            note.push_str(&format!(" Target window: ~{target_window} tokens."));
        }
        if let Some(reserved_output) = preflight.reserved_output_headroom_tokens {
            note.push_str(&format!(" Reserved output: ~{reserved_output} tokens."));
        }
    }

    note
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::route_switch_preflight::{
        RouteSwitchPreflight, RouteSwitchStatus,
    };

    #[test]
    fn formats_lane_aware_model_switch() {
        let options = RuntimeCommandPresentationOptions::new("openrouter");

        let response = format_switch_model_success(
            "vision-model",
            "openrouter",
            Some(CapabilityLane::MultimodalUnderstanding),
            true,
            &options,
        );

        assert!(response
            .starts_with("Lane `multimodal_understanding` switched to `openrouter:vision-model`."));
        assert!(response.contains("Context compacted before switching"));
        assert!(
            response.find("Lane `").expect("lane first")
                < response
                    .find("openrouter:vision-model")
                    .expect("route target")
        );
    }

    #[test]
    fn formats_blocked_model_switch_with_budget_details() {
        let options = RuntimeCommandPresentationOptions::new("openrouter");
        let response = format_switch_model_blocked(
            "tiny-model",
            "openrouter",
            Some(CapabilityLane::Reasoning),
            &RouteSwitchPreflight {
                estimated_context_tokens: 8_000,
                target_context_window_tokens: Some(4_000),
                safe_context_budget_tokens: Some(3_000),
                reserved_output_headroom_tokens: Some(1_000),
                recommended_compaction_threshold_tokens: Some(1_500),
                recommended_condensation: None,
                status: RouteSwitchStatus::TooLarge,
            },
            true,
            &options,
        );

        assert!(response
            .starts_with("Lane `reasoning` route switch to `openrouter:tiny-model` blocked."));
        assert!(response.contains("safe context budget is ~3000 tokens"));
        assert!(response.contains("Target window: ~4000 tokens"));
        assert!(response.contains("Reserved output: ~1000 tokens"));
        assert!(response.contains("Compaction ran first"));
    }

    #[test]
    fn common_formatter_skips_adapter_specific_help() {
        let options = RuntimeCommandPresentationOptions::new("openrouter");

        assert_eq!(
            format_common_command_effect(&CommandEffect::ShowModel, &options),
            None
        );
        assert_eq!(
            format_common_command_effect(&CommandEffect::ClearSession, &options),
            Some("Conversation history cleared. Starting fresh.".to_string())
        );
    }

    #[test]
    fn formats_model_switch_failure_with_provider_context() {
        assert_eq!(
            format_switch_model_failure("small-model", "openrouter", "provider unavailable"),
            "Model switch to `small-model` (provider: `openrouter`) blocked: provider unavailable"
        );
    }
}
