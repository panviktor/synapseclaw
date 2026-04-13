//! Shared inbound runtime configuration assembly.
//!
//! Web and channel transports provide different concrete values, but the
//! mapping into the domain use-case config must stay identical.

use std::sync::Arc;

use synapse_domain::application::services::channel_presentation::ChannelPresentationMode;
use synapse_domain::application::use_cases::handle_inbound_message::InboundMessageConfig;
use synapse_domain::config::schema::{
    ModelLaneConfig, PromptBudgetConfig, QueryClassificationConfig,
};

const THREAD_ROOT_MAX_CHARS: usize = 500;
const THREAD_PARENT_RECENT_TURNS: usize = 3;
const THREAD_PARENT_MAX_CHARS: usize = 2000;

pub(crate) struct InboundRuntimeConfigInput {
    pub system_prompt: String,
    pub default_provider: String,
    pub default_model: String,
    pub temperature: f64,
    pub max_tool_iterations: usize,
    pub auto_save_memory: bool,
    pub model_lanes: Vec<ModelLaneConfig>,
    pub model_preset: Option<String>,
    pub query_classification: QueryClassificationConfig,
    pub message_timeout_secs: u64,
    pub min_relevance_score: f64,
    pub ack_reactions: bool,
    pub agent_id: String,
    pub prompt_budget_config: PromptBudgetConfig,
    pub presentation_mode: ChannelPresentationMode,
}

pub(crate) struct InboundRuntimeConfigFactory;

impl InboundRuntimeConfigFactory {
    pub(crate) fn build(input: InboundRuntimeConfigInput) -> InboundMessageConfig {
        let mut prompt_budget = input.prompt_budget_config.to_prompt_budget();
        prompt_budget.recall_min_relevance = input.min_relevance_score;
        let continuation_policy = input.prompt_budget_config.to_continuation_policy();
        let query_classifier = if input.query_classification.enabled {
            let query_classification = input.query_classification;
            Some(Arc::new(move |msg: &str| {
                crate::agent::classifier::classify(&query_classification, msg)
            })
                as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>)
        } else {
            None
        };

        InboundMessageConfig {
            system_prompt: input.system_prompt,
            default_provider: input.default_provider,
            default_model: input.default_model,
            temperature: input.temperature,
            max_tool_iterations: input.max_tool_iterations,
            auto_save_memory: input.auto_save_memory,
            model_lanes: input.model_lanes,
            model_preset: input.model_preset,
            thread_root_max_chars: THREAD_ROOT_MAX_CHARS,
            thread_parent_recent_turns: THREAD_PARENT_RECENT_TURNS,
            thread_parent_max_chars: THREAD_PARENT_MAX_CHARS,
            query_classifier,
            message_timeout_secs: input.message_timeout_secs,
            min_relevance_score: input.min_relevance_score,
            ack_reactions: input.ack_reactions,
            agent_id: input.agent_id,
            prompt_budget,
            continuation_policy,
            presentation_mode: input.presentation_mode,
        }
    }
}
