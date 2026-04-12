use crate::agent::autosave_memory_key;
use crate::agent::context_engine::{
    build_provider_prompt_snapshot, system_message_breakdown, total_message_chars,
    ProviderPromptSnapshot,
};
use crate::agent::dispatcher::{
    NativeToolDispatcher, ParsedToolCall, ToolDispatcher, ToolExecutionResult, XmlToolDispatcher,
};
use crate::agent::execute_one_tool;
use crate::agent::prompt::{PromptContext, SystemPromptBuilder};
use crate::agent::tool_repair_classification::classify_tool_execution_error;
use crate::agent::turn_context_fmt;
use crate::runtime;
use crate::tools::{self, Tool, ToolSpec};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use synapse_domain::application::services::dialogue_state_service::DialogueStateStore;
use synapse_domain::application::services::dialogue_state_service::{self};
use synapse_domain::application::services::history_compaction;
use synapse_domain::application::services::history_compaction::HistoryCompressionPolicy;
use synapse_domain::application::services::model_capability_support::{
    assess_provider_call_capabilities, ProviderCallCapabilityInput, ProviderCallCapabilityIssue,
};
#[cfg(test)]
use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile;
use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileConfidence;
use synapse_domain::application::services::provider_context_budget::{
    assess_provider_context_budget, provider_context_artifact_name,
    provider_context_budget_tier_name, provider_context_condensation_mode_name,
    provider_context_reserved_output_headroom_tokens, provider_context_turn_shape_name,
    ProviderContextBudgetInput, CONTEXT_SAFETY_CEILING_DENOMINATOR,
    CONTEXT_SAFETY_CEILING_NUMERATOR,
};
use synapse_domain::application::services::provider_native_context_policy::{
    resolve_provider_native_context_policy, ProviderNativeContextPolicyInput,
};
use synapse_domain::application::services::route_switch_preflight::{
    assess_route_switch_preflight_for_budget, RouteSwitchPreflightResolution,
};
use synapse_domain::application::services::runtime_assumptions::{
    apply_tool_repair_assumption_challenges, build_runtime_assumptions,
    challenge_runtime_assumption_ledger, merge_runtime_assumption_ledger, RuntimeAssumption,
    RuntimeAssumptionChallenge, RuntimeAssumptionInput, RuntimeAssumptionInvalidation,
    RuntimeAssumptionKind, RuntimeAssumptionReplacementPath,
};
use synapse_domain::application::services::runtime_calibration::{
    append_runtime_calibration_observation, RuntimeCalibrationDecisionKind,
    RuntimeCalibrationObservation, RuntimeCalibrationOutcome, RuntimeCalibrationRecord,
};
use synapse_domain::application::services::runtime_trace_janitor::{
    run_runtime_trace_janitor, RuntimeTraceJanitorInput,
};
use synapse_domain::application::services::scoped_instruction_resolution::{
    adjust_scoped_instruction_plan_for_context_pressure, build_scoped_instruction_plan,
    format_scoped_instruction_block,
};
use synapse_domain::application::services::summary_route_resolution::resolve_summary_route;
use synapse_domain::application::services::turn_admission::{
    assess_turn_admission, CandidateAdmissionDecision, TurnAdmissionInput,
};
use synapse_domain::application::services::turn_context::{
    self as tc, ContinuationPolicy, PromptBudget,
};
use synapse_domain::application::services::turn_interpretation;
use synapse_domain::application::services::turn_model_routing::resolve_turn_route_override;
use synapse_domain::config::schema::{CapabilityLane, Config, ContextCompressionConfig};
use synapse_domain::domain::tool_fact::TypedToolFact;
use synapse_domain::domain::tool_repair::{tool_failure_kind_name, ToolRepairTrace};
use synapse_domain::domain::turn_admission::{
    context_pressure_state_name, turn_admission_action_name, turn_intent_name, TurnAdmissionAction,
};
use synapse_domain::ports::channel_registry::ChannelRegistryPort;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::conversation_store::ConversationStorePort;
use synapse_domain::ports::history_compaction_cache::HistoryCompactionCachePort;
use synapse_domain::ports::route_selection::{ContextCacheStats, RouteAdmissionState};
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;
use synapse_domain::ports::scoped_instruction_context::{
    ScopedInstructionContextPort, ScopedInstructionRequest,
};
use synapse_domain::ports::summary::SummaryGeneratorPort;
use synapse_domain::ports::turn_defaults_context::{
    InMemoryTurnDefaultsContext, TurnDefaultsContextPort,
};
use synapse_domain::ports::user_profile_context::{
    InMemoryUserProfileContext, UserProfileContextPort,
};
use synapse_domain::ports::user_profile_store::UserProfileStorePort;
use synapse_memory::{self, MemoryCategory, UnifiedMemoryPort};
use synapse_observability::{self, Observer, ObserverEvent};
use synapse_providers::error_classification::classify_context_limit_error;
use synapse_providers::{
    self, ChatMessage, ChatRequest, ConversationMessage, Provider, ProviderCapabilityError,
};
use synapse_security::security_policy_from_config;

const PROVIDER_CONTEXT_CHAT_MESSAGES: usize = 6;
const SESSION_HYGIENE_HARD_MESSAGE_LIMIT: usize = 400;

#[derive(Clone, Default)]
pub struct AgentRuntimePorts {
    pub conversation_store: Option<Arc<dyn ConversationStorePort>>,
    pub conversation_context: Option<Arc<dyn ConversationContextPort>>,
    pub user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
    pub user_profile_context: Option<Arc<dyn UserProfileContextPort>>,
    pub turn_defaults_context: Option<Arc<dyn TurnDefaultsContextPort>>,
    pub scoped_instruction_context: Option<Arc<dyn ScopedInstructionContextPort>>,
    pub channel_registry: Option<Arc<dyn ChannelRegistryPort>>,
    pub run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    pub history_compaction_cache: Option<Arc<dyn HistoryCompactionCachePort>>,
}

fn canonicalize_tool_args(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let normalized = entries
                .into_iter()
                .map(|(key, value)| (key.clone(), canonicalize_tool_args(value)))
                .collect();
            serde_json::Value::Object(normalized)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_tool_args).collect())
        }
        other => other.clone(),
    }
}

fn normalize_tool_call(call: &ParsedToolCall) -> ParsedToolCall {
    call.clone()
}

fn strip_redundant_delivery_target(call: &ParsedToolCall) -> ParsedToolCall {
    let mut normalized = call.clone();
    if let serde_json::Value::Object(arguments) = &mut normalized.arguments {
        arguments.remove("target");
    }
    normalized
}

fn has_noncanonical_string_delivery_target(call: &ParsedToolCall) -> bool {
    call.arguments
        .get("target")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|target| target != "current_conversation")
}

fn provider_context_budget_input_from_stats(
    stats: &synapse_observability::ProviderContextStats,
) -> ProviderContextBudgetInput {
    ProviderContextBudgetInput {
        total_chars: stats.total_chars,
        prior_chat_messages: stats.prior_chat_messages,
        current_turn_messages: stats.current_turn_messages,
        target_context_window_tokens: None,
        target_max_output_tokens: None,
        bootstrap_chars: stats.bootstrap_chars,
        core_memory_chars: stats.core_memory_chars,
        runtime_interpretation_chars: stats.runtime_interpretation_chars,
        scoped_context_chars: stats.scoped_context_chars,
        resolution_chars: stats.resolution_chars,
        prior_chat_chars: stats.prior_chat_chars,
        current_turn_chars: stats.current_turn_chars,
    }
}

fn provider_context_budget_input_from_stats_for_profile(
    stats: &synapse_observability::ProviderContextStats,
    profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
) -> ProviderContextBudgetInput {
    provider_context_budget_input_from_stats(stats).with_target_model_profile(profile)
}

fn route_calibration_signature(
    provider: &str,
    model: &str,
    decision: &CandidateAdmissionDecision,
) -> String {
    format!(
        "provider={provider},model={model},action={},pressure={}",
        turn_admission_action_name(decision.snapshot.action),
        context_pressure_state_name(decision.snapshot.pressure_state)
    )
}

fn route_calibration_confidence(decision: &CandidateAdmissionDecision) -> u16 {
    match decision.snapshot.action {
        TurnAdmissionAction::Proceed if decision.reasons.is_empty() => 9_000,
        TurnAdmissionAction::Proceed => 8_000,
        TurnAdmissionAction::Compact => 6_500,
        TurnAdmissionAction::Reroute => 6_000,
        TurnAdmissionAction::Block => 4_500,
    }
}

fn tool_calibration_signature(result: &ToolExecutionResult) -> String {
    match result.repair_trace.as_ref() {
        Some(trace) => format!(
            "tool={},failure={}",
            result.name,
            tool_failure_kind_name(trace.failure_kind)
        ),
        None => format!("tool={}", result.name),
    }
}

fn tool_calibration_confidence(result: &ToolExecutionResult) -> u16 {
    if result.success {
        7_500
    } else {
        8_000
    }
}

fn history_compaction_cache_key(
    transcript: &str,
    policy: &HistoryCompressionPolicy,
    context_window_tokens: Option<usize>,
) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"synapseclaw:history-compaction:v2\0");
    hasher.update(history_compaction_policy_fingerprint(policy, context_window_tokens).as_bytes());
    hasher.update(b"\0");
    hasher.update(transcript.as_bytes());
    hex::encode(hasher.finalize())
}

fn history_compaction_policy_fingerprint(
    policy: &HistoryCompressionPolicy,
    context_window_tokens: Option<usize>,
) -> String {
    format!(
        "enabled={} threshold={:.6} target={:.6} first={} last={} summary={:.6} min={} max={} source_chars={} summary_chars={} context={}",
        policy.enabled,
        policy.threshold_ratio,
        policy.target_ratio,
        policy.protect_first_n,
        policy.protect_last_n,
        policy.summary_ratio,
        policy.min_summary_tokens,
        policy.max_summary_tokens,
        policy.max_source_chars,
        policy.max_summary_chars,
        context_window_tokens.unwrap_or(0),
    )
}

pub struct Agent {
    provider: Box<dyn Provider>,
    tools: Vec<Box<dyn Tool>>,
    tool_specs: Vec<ToolSpec>,
    memory: Arc<dyn UnifiedMemoryPort>,
    observer: Arc<dyn Observer>,
    prompt_builder: SystemPromptBuilder,
    tool_dispatcher: Box<dyn ToolDispatcher>,
    prompt_budget: PromptBudget,
    continuation_policy: ContinuationPolicy,
    /// Turn counter within current session (for continuation policy).
    turn_count: usize,
    config: synapse_domain::config::schema::AgentConfig,
    compression: ContextCompressionConfig,
    compression_overrides:
        Vec<synapse_domain::config::schema::ContextCompressionRouteOverrideConfig>,
    provider_name: String,
    provider_api_url: Option<String>,
    model_name: String,
    active_lane: Option<CapabilityLane>,
    active_candidate_index: Option<usize>,
    temperature: f64,
    workspace_dir: std::path::PathBuf,
    identity_config: synapse_domain::config::schema::IdentityConfig,
    skills: Vec<crate::skills::Skill>,
    skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode,
    auto_save: bool,
    memory_session_id: Option<String>,
    history: Vec<ConversationMessage>,
    classification_config: synapse_domain::config::schema::QueryClassificationConfig,
    available_hints: Vec<String>,
    route_model_by_hint: HashMap<String, String>,
    route_model_preset: Option<String>,
    route_model_lanes: Vec<synapse_domain::config::schema::ModelLaneConfig>,
    route_model_routes: Vec<synapse_domain::config::schema::ModelRouteConfig>,
    current_model_profile:
        synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    /// Cumulative token usage from the last turn (provider-reported).
    last_turn_usage: Option<synapse_providers::traits::TokenUsage>,
    /// Structured facts emitted by tools during the last completed turn.
    last_turn_tool_facts: Vec<TypedToolFact>,
    /// Most recent structured tool self-repair trace from the last turn.
    last_turn_tool_repair: Option<ToolRepairTrace>,
    /// Bounded recent structured tool self-repair traces for this session.
    recent_turn_tool_repairs: Vec<ToolRepairTrace>,
    /// Bounded recent structured route-admission states for this session.
    recent_turn_admissions: Vec<RouteAdmissionState>,
    /// Bounded session/runtime assumption ledger for this live agent.
    recent_runtime_assumptions: Vec<RuntimeAssumption>,
    /// Bounded session/runtime calibration ledger for this live agent.
    recent_runtime_calibrations: Vec<RuntimeCalibrationRecord>,
    allowed_tools: Option<Vec<String>>,
    response_cache: Option<Arc<synapse_memory::response_cache::ResponseCache>>,
    history_summary_generator: Option<Arc<dyn SummaryGeneratorPort>>,
    history_compaction_cache: Arc<dyn HistoryCompactionCachePort>,
    /// Canonical agent ID for memory scoping.
    agent_id: String,
    dialogue_state_store: Option<Arc<DialogueStateStore>>,
    conversation_store: Option<Arc<dyn ConversationStorePort>>,
    run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
    user_profile_key: Option<String>,
    user_profile_context: Arc<dyn UserProfileContextPort>,
    turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
    scoped_instruction_context: Option<Arc<dyn ScopedInstructionContextPort>>,
    channel_registry: Option<Arc<dyn ChannelRegistryPort>>,
}

pub struct AgentBuilder {
    provider: Option<Box<dyn Provider>>,
    tools: Option<Vec<Box<dyn Tool>>>,
    memory: Option<Arc<dyn UnifiedMemoryPort>>,
    observer: Option<Arc<dyn Observer>>,
    prompt_builder: Option<SystemPromptBuilder>,
    tool_dispatcher: Option<Box<dyn ToolDispatcher>>,
    prompt_budget: Option<PromptBudget>,
    continuation_policy: Option<ContinuationPolicy>,
    config: Option<synapse_domain::config::schema::AgentConfig>,
    compression: Option<ContextCompressionConfig>,
    compression_overrides:
        Option<Vec<synapse_domain::config::schema::ContextCompressionRouteOverrideConfig>>,
    provider_name: Option<String>,
    provider_api_url: Option<String>,
    model_name: Option<String>,
    temperature: Option<f64>,
    workspace_dir: Option<std::path::PathBuf>,
    identity_config: Option<synapse_domain::config::schema::IdentityConfig>,
    skills: Option<Vec<crate::skills::Skill>>,
    skills_prompt_mode: Option<synapse_domain::config::schema::SkillsPromptInjectionMode>,
    auto_save: Option<bool>,
    memory_session_id: Option<String>,
    classification_config: Option<synapse_domain::config::schema::QueryClassificationConfig>,
    available_hints: Option<Vec<String>>,
    route_model_by_hint: Option<HashMap<String, String>>,
    route_model_preset: Option<Option<String>>,
    route_model_lanes: Option<Vec<synapse_domain::config::schema::ModelLaneConfig>>,
    route_model_routes: Option<Vec<synapse_domain::config::schema::ModelRouteConfig>>,
    current_model_profile:
        Option<synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile>,
    allowed_tools: Option<Vec<String>>,
    response_cache: Option<Arc<synapse_memory::response_cache::ResponseCache>>,
    history_summary_generator: Option<Arc<dyn SummaryGeneratorPort>>,
    history_compaction_cache: Option<Arc<dyn HistoryCompactionCachePort>>,
    agent_id: Option<String>,
    dialogue_state_store: Option<Arc<DialogueStateStore>>,
    conversation_store: Option<Arc<dyn ConversationStorePort>>,
    run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
    user_profile_key: Option<String>,
    user_profile_context: Option<Arc<dyn UserProfileContextPort>>,
    turn_defaults_context: Option<Arc<dyn TurnDefaultsContextPort>>,
    scoped_instruction_context: Option<Arc<dyn ScopedInstructionContextPort>>,
    channel_registry: Option<Arc<dyn ChannelRegistryPort>>,
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            provider: None,
            tools: None,
            memory: None,
            observer: None,
            prompt_builder: None,
            tool_dispatcher: None,
            prompt_budget: None,
            continuation_policy: None,
            config: None,
            compression: None,
            compression_overrides: None,
            provider_name: None,
            provider_api_url: None,
            model_name: None,
            temperature: None,
            workspace_dir: None,
            identity_config: None,
            skills: None,
            skills_prompt_mode: None,
            auto_save: None,
            memory_session_id: None,
            classification_config: None,
            available_hints: None,
            route_model_by_hint: None,
            route_model_preset: None,
            route_model_lanes: None,
            route_model_routes: None,
            current_model_profile: None,
            allowed_tools: None,
            response_cache: None,
            history_summary_generator: None,
            history_compaction_cache: None,
            agent_id: None,
            dialogue_state_store: None,
            conversation_store: None,
            run_recipe_store: None,
            user_profile_store: None,
            user_profile_key: None,
            user_profile_context: None,
            turn_defaults_context: None,
            scoped_instruction_context: None,
            channel_registry: None,
        }
    }

    pub fn provider(mut self, provider: Box<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn tools(mut self, tools: Vec<Box<dyn Tool>>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn memory(mut self, memory: Arc<dyn UnifiedMemoryPort>) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn observer(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn prompt_builder(mut self, prompt_builder: SystemPromptBuilder) -> Self {
        self.prompt_builder = Some(prompt_builder);
        self
    }

    pub fn tool_dispatcher(mut self, tool_dispatcher: Box<dyn ToolDispatcher>) -> Self {
        self.tool_dispatcher = Some(tool_dispatcher);
        self
    }

    pub fn prompt_budget(mut self, budget: PromptBudget) -> Self {
        self.prompt_budget = Some(budget);
        self
    }

    pub fn continuation_policy(mut self, policy: ContinuationPolicy) -> Self {
        self.continuation_policy = Some(policy);
        self
    }

    pub fn config(mut self, config: synapse_domain::config::schema::AgentConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn compression(mut self, compression: ContextCompressionConfig) -> Self {
        self.compression = Some(compression);
        self
    }

    pub fn compression_overrides(
        mut self,
        compression_overrides: Vec<
            synapse_domain::config::schema::ContextCompressionRouteOverrideConfig,
        >,
    ) -> Self {
        self.compression_overrides = Some(compression_overrides);
        self
    }

    pub fn provider_name(mut self, provider_name: String) -> Self {
        self.provider_name = Some(provider_name);
        self
    }

    pub fn model_name(mut self, model_name: String) -> Self {
        self.model_name = Some(model_name);
        self
    }

    pub fn temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn workspace_dir(mut self, workspace_dir: std::path::PathBuf) -> Self {
        self.workspace_dir = Some(workspace_dir);
        self
    }

    pub fn identity_config(
        mut self,
        identity_config: synapse_domain::config::schema::IdentityConfig,
    ) -> Self {
        self.identity_config = Some(identity_config);
        self
    }

    pub fn skills(mut self, skills: Vec<crate::skills::Skill>) -> Self {
        self.skills = Some(skills);
        self
    }

    pub fn skills_prompt_mode(
        mut self,
        skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode,
    ) -> Self {
        self.skills_prompt_mode = Some(skills_prompt_mode);
        self
    }

    pub fn auto_save(mut self, auto_save: bool) -> Self {
        self.auto_save = Some(auto_save);
        self
    }

    pub fn memory_session_id(mut self, memory_session_id: Option<String>) -> Self {
        self.memory_session_id = memory_session_id;
        self
    }

    pub fn classification_config(
        mut self,
        classification_config: synapse_domain::config::schema::QueryClassificationConfig,
    ) -> Self {
        self.classification_config = Some(classification_config);
        self
    }

    pub fn available_hints(mut self, available_hints: Vec<String>) -> Self {
        self.available_hints = Some(available_hints);
        self
    }

    pub fn route_model_by_hint(mut self, route_model_by_hint: HashMap<String, String>) -> Self {
        self.route_model_by_hint = Some(route_model_by_hint);
        self
    }

    pub fn route_model_preset(mut self, route_model_preset: Option<String>) -> Self {
        self.route_model_preset = Some(route_model_preset);
        self
    }

    pub fn route_model_lanes(
        mut self,
        route_model_lanes: Vec<synapse_domain::config::schema::ModelLaneConfig>,
    ) -> Self {
        self.route_model_lanes = Some(route_model_lanes);
        self
    }

    pub fn route_model_routes(
        mut self,
        route_model_routes: Vec<synapse_domain::config::schema::ModelRouteConfig>,
    ) -> Self {
        self.route_model_routes = Some(route_model_routes);
        self
    }

    pub fn current_model_profile(
        mut self,
        current_model_profile:
            synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    ) -> Self {
        self.current_model_profile = Some(current_model_profile);
        self
    }

    pub fn provider_api_url(mut self, provider_api_url: Option<String>) -> Self {
        self.provider_api_url = provider_api_url;
        self
    }

    pub fn allowed_tools(mut self, allowed_tools: Option<Vec<String>>) -> Self {
        self.allowed_tools = allowed_tools;
        self
    }

    pub fn response_cache(
        mut self,
        cache: Option<Arc<synapse_memory::response_cache::ResponseCache>>,
    ) -> Self {
        self.response_cache = cache;
        self
    }

    pub fn history_summary_generator(
        mut self,
        generator: Option<Arc<dyn SummaryGeneratorPort>>,
    ) -> Self {
        self.history_summary_generator = generator;
        self
    }

    pub fn history_compaction_cache(
        mut self,
        cache: Option<Arc<dyn HistoryCompactionCachePort>>,
    ) -> Self {
        self.history_compaction_cache = cache;
        self
    }

    pub fn agent_id(mut self, id: String) -> Self {
        self.agent_id = Some(id);
        self
    }

    pub fn dialogue_state_store(mut self, store: Option<Arc<DialogueStateStore>>) -> Self {
        self.dialogue_state_store = store;
        self
    }

    pub fn conversation_store(mut self, store: Option<Arc<dyn ConversationStorePort>>) -> Self {
        self.conversation_store = store;
        self
    }

    pub fn run_recipe_store(mut self, store: Option<Arc<dyn RunRecipeStorePort>>) -> Self {
        self.run_recipe_store = store;
        self
    }

    pub fn user_profile_store(mut self, store: Option<Arc<dyn UserProfileStorePort>>) -> Self {
        self.user_profile_store = store;
        self
    }

    pub fn user_profile_key(mut self, key: Option<String>) -> Self {
        self.user_profile_key = key;
        self
    }

    pub fn user_profile_context(
        mut self,
        context: Option<Arc<dyn UserProfileContextPort>>,
    ) -> Self {
        self.user_profile_context = context;
        self
    }

    pub fn turn_defaults_context(
        mut self,
        context: Option<Arc<dyn TurnDefaultsContextPort>>,
    ) -> Self {
        self.turn_defaults_context = context;
        self
    }

    pub fn scoped_instruction_context(
        mut self,
        context: Option<Arc<dyn ScopedInstructionContextPort>>,
    ) -> Self {
        self.scoped_instruction_context = context;
        self
    }

    pub fn channel_registry(mut self, registry: Option<Arc<dyn ChannelRegistryPort>>) -> Self {
        self.channel_registry = registry;
        self
    }

    pub fn build(self) -> Result<Agent> {
        let mut tools = self
            .tools
            .ok_or_else(|| anyhow::anyhow!("tools are required"))?;
        let allowed = self.allowed_tools.clone();
        if let Some(ref allow_list) = allowed {
            tools.retain(|t| allow_list.iter().any(|name| name == t.name()));
        }
        let tool_specs = tools.iter().map(|tool| tool.spec()).collect();
        let user_profile_context = self
            .user_profile_context
            .unwrap_or_else(|| Arc::new(InMemoryUserProfileContext::new()));
        user_profile_context.set_current_key(self.user_profile_key.clone());
        let turn_defaults_context = self
            .turn_defaults_context
            .unwrap_or_else(|| Arc::new(InMemoryTurnDefaultsContext::new()));

        let config = self.config.unwrap_or_default();
        let provider_name = self.provider_name.unwrap_or_else(|| "unknown".into());
        let model_name = self.model_name.unwrap_or_else(|| "default".into());
        let workspace_dir = self
            .workspace_dir
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let agent_id = self.agent_id.unwrap_or_else(|| "default".to_string());
        let history_compaction_cache = self.history_compaction_cache.unwrap_or_else(|| {
            crate::runtime::history_compaction_cache::shared_history_compaction_cache(
                &workspace_dir,
                &agent_id,
            )
        });

        Ok(Agent {
            provider: self
                .provider
                .ok_or_else(|| anyhow::anyhow!("provider is required"))?,
            tools,
            tool_specs,
            memory: self
                .memory
                .ok_or_else(|| anyhow::anyhow!("memory is required"))?,
            observer: self
                .observer
                .ok_or_else(|| anyhow::anyhow!("observer is required"))?,
            prompt_builder: self
                .prompt_builder
                .unwrap_or_else(SystemPromptBuilder::with_defaults),
            tool_dispatcher: self
                .tool_dispatcher
                .ok_or_else(|| anyhow::anyhow!("tool_dispatcher is required"))?,
            prompt_budget: self.prompt_budget.unwrap_or_default(),
            continuation_policy: self.continuation_policy.unwrap_or_default(),
            turn_count: 0,
            config,
            compression: self.compression.unwrap_or_default(),
            compression_overrides: self.compression_overrides.unwrap_or_default(),
            provider_name,
            provider_api_url: self.provider_api_url,
            model_name,
            active_lane: None,
            active_candidate_index: None,
            temperature: self.temperature.unwrap_or(0.7),
            workspace_dir,
            identity_config: self.identity_config.unwrap_or_default(),
            skills: self.skills.unwrap_or_default(),
            skills_prompt_mode: self.skills_prompt_mode.unwrap_or_default(),
            auto_save: self.auto_save.unwrap_or(false),
            memory_session_id: self.memory_session_id,
            history: Vec::new(),
            classification_config: self.classification_config.unwrap_or_default(),
            available_hints: self.available_hints.unwrap_or_default(),
            route_model_by_hint: self.route_model_by_hint.unwrap_or_default(),
            route_model_preset: self.route_model_preset.unwrap_or_default(),
            route_model_lanes: self.route_model_lanes.unwrap_or_default(),
            route_model_routes: self.route_model_routes.unwrap_or_default(),
            current_model_profile: self.current_model_profile.unwrap_or_default(),
            last_turn_usage: None,
            last_turn_tool_facts: Vec::new(),
            last_turn_tool_repair: None,
            recent_turn_tool_repairs: Vec::new(),
            recent_turn_admissions: Vec::new(),
            recent_runtime_assumptions: Vec::new(),
            recent_runtime_calibrations: Vec::new(),
            allowed_tools: allowed,
            response_cache: self.response_cache,
            history_summary_generator: self.history_summary_generator,
            history_compaction_cache,
            agent_id,
            dialogue_state_store: self.dialogue_state_store,
            conversation_store: self.conversation_store,
            run_recipe_store: self.run_recipe_store,
            user_profile_store: self.user_profile_store,
            user_profile_key: self.user_profile_key,
            user_profile_context,
            turn_defaults_context,
            scoped_instruction_context: self.scoped_instruction_context,
            channel_registry: self.channel_registry,
        })
    }
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    fn build_provider_prompt_snapshot_for_profile(
        &self,
        target_profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    ) -> ProviderPromptSnapshot {
        build_provider_prompt_snapshot(
            self.tool_dispatcher.as_ref(),
            &self.history,
            PROVIDER_CONTEXT_CHAT_MESSAGES,
            Some(target_profile),
        )
    }

    fn upsert_scoped_context_block(&mut self, block: Option<String>) {
        const SCOPED_MARKER: &str = "[scoped-context]\n";
        self.history.retain(|msg| {
            if let ConversationMessage::Chat(chat) = msg {
                !(chat.role == "system" && chat.content.starts_with(SCOPED_MARKER))
            } else {
                true
            }
        });
        if let Some(block) = block {
            self.history
                .push(ConversationMessage::Chat(ChatMessage::system(block)));
        }
    }

    pub fn history(&self) -> &[ConversationMessage] {
        &self.history
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.turn_count = 0;
    }

    /// Push a pre-built conversation message (used for session replay from DB).
    pub fn push_history(&mut self, msg: ConversationMessage) {
        // Track user turns for continuation policy
        if let ConversationMessage::Chat(ref chat) = msg {
            if chat.role == "user" {
                self.turn_count += 1;
            }
        }
        self.history.push(msg);
    }

    /// Get a clone of the observer Arc (for wrapping).
    pub fn observer_arc(&self) -> Arc<dyn synapse_observability::Observer> {
        Arc::clone(&self.observer)
    }

    pub fn provider_name_str(&self) -> &str {
        &self.provider_name
    }

    pub fn model_name_str(&self) -> &str {
        &self.model_name
    }

    pub fn active_lane(&self) -> Option<CapabilityLane> {
        self.active_lane
    }

    pub fn active_candidate_index(&self) -> Option<usize> {
        self.active_candidate_index
    }

    pub async fn prepare_for_target_context_window(
        &mut self,
        target_context_window_tokens: usize,
    ) -> RouteSwitchPreflightResolution {
        let target_profile =
            synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile {
                context_window_tokens: Some(target_context_window_tokens),
                context_window_source:
                    synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                max_output_tokens: None,
                features: Vec::new(),
                ..Default::default()
            };
        let snapshot = self.build_provider_prompt_snapshot_for_profile(&target_profile);
        let budget = assess_provider_context_budget(
            provider_context_budget_input_from_stats_for_profile(&snapshot.stats, &target_profile),
        );
        let mut resolution = RouteSwitchPreflightResolution::new(
            assess_route_switch_preflight_for_budget(&budget, &target_profile),
        );

        while resolution.should_attempt_compaction() {
            let compression = self.history_compression_for_route(
                &self.provider_name,
                &self.model_name,
                None,
                None,
            );
            if !self
                .maybe_compact_history_with_route(&compression, &target_profile)
                .await
            {
                break;
            }
            let snapshot = self.build_provider_prompt_snapshot_for_profile(&target_profile);
            let budget = assess_provider_context_budget(
                provider_context_budget_input_from_stats_for_profile(
                    &snapshot.stats,
                    &target_profile,
                ),
            );
            resolution.record_compaction_pass(assess_route_switch_preflight_for_budget(
                &budget,
                &target_profile,
            ));
        }

        resolution
    }

    /// Replace the observer (e.g. to wrap with per-request event forwarding).
    pub fn set_observer(&mut self, observer: Arc<dyn synapse_observability::Observer>) {
        self.observer = observer;
    }

    /// Token usage reported by the provider during the last turn (if any).
    pub fn last_turn_usage(&self) -> Option<&synapse_providers::traits::TokenUsage> {
        self.last_turn_usage.as_ref()
    }

    pub fn last_turn_tool_facts(&self) -> &[TypedToolFact] {
        &self.last_turn_tool_facts
    }

    pub fn last_turn_tool_repair(&self) -> Option<&ToolRepairTrace> {
        self.last_turn_tool_repair.as_ref()
    }

    pub fn recent_turn_tool_repairs(&self) -> &[ToolRepairTrace] {
        &self.recent_turn_tool_repairs
    }

    pub fn recent_turn_admissions(&self) -> &[RouteAdmissionState] {
        &self.recent_turn_admissions
    }

    pub fn recent_runtime_assumptions(&self) -> &[RuntimeAssumption] {
        &self.recent_runtime_assumptions
    }

    pub fn recent_runtime_calibrations(&self) -> &[RuntimeCalibrationRecord] {
        &self.recent_runtime_calibrations
    }

    fn run_runtime_trace_janitor(&mut self, now_unix: i64) {
        let cleaned = run_runtime_trace_janitor(RuntimeTraceJanitorInput {
            tool_repairs: &self.recent_turn_tool_repairs,
            assumptions: &self.recent_runtime_assumptions,
            calibration_records: &self.recent_runtime_calibrations,
            now_unix,
            ..Default::default()
        });
        let removed_total = cleaned.report.removed_total();
        let promotion_candidates = cleaned.report.promotion_candidates.len();
        self.recent_turn_tool_repairs = cleaned.tool_repairs;
        self.recent_runtime_assumptions = cleaned.assumptions;
        self.recent_runtime_calibrations = cleaned.calibration_records;
        if removed_total > 0 || promotion_candidates > 0 {
            tracing::debug!(
                removed_total,
                promotion_candidates,
                "Runtime trace janitor cleaned session ledgers"
            );
        }
    }

    fn record_runtime_calibration_observation(
        &mut self,
        observation: RuntimeCalibrationObservation,
    ) {
        let ledger = append_runtime_calibration_observation(
            &self.recent_runtime_calibrations,
            observation,
            chrono::Utc::now().timestamp(),
        );
        self.recent_runtime_calibrations = ledger.records;
    }

    fn record_route_calibration(
        &mut self,
        decision: &CandidateAdmissionDecision,
        outcome: RuntimeCalibrationOutcome,
        observed_at_unix: i64,
        effective_model: &str,
    ) {
        if decision.snapshot.action == TurnAdmissionAction::Block {
            return;
        }
        self.record_runtime_calibration_observation(RuntimeCalibrationObservation {
            decision_kind: RuntimeCalibrationDecisionKind::RouteChoice,
            decision_signature: route_calibration_signature(
                &self.provider_name,
                effective_model,
                decision,
            ),
            confidence_basis_points: route_calibration_confidence(decision),
            outcome,
            observed_at_unix,
        });
    }

    fn record_tool_calibrations(&mut self, results: &[ToolExecutionResult], observed_at_unix: i64) {
        for result in results {
            self.record_runtime_calibration_observation(RuntimeCalibrationObservation {
                decision_kind: RuntimeCalibrationDecisionKind::ToolChoice,
                decision_signature: tool_calibration_signature(result),
                confidence_basis_points: tool_calibration_confidence(result),
                outcome: if result.success {
                    RuntimeCalibrationOutcome::Succeeded
                } else {
                    RuntimeCalibrationOutcome::Failed
                },
                observed_at_unix,
            });
        }
    }

    pub fn user_profile_key(&self) -> Option<&str> {
        self.user_profile_key.as_deref()
    }

    pub fn set_memory_session_id(&mut self, session_id: Option<String>) {
        self.memory_session_id = session_id;
    }

    pub fn set_dialogue_state_store(&mut self, store: Option<Arc<DialogueStateStore>>) {
        self.dialogue_state_store = store;
    }

    pub fn set_conversation_store(&mut self, store: Option<Arc<dyn ConversationStorePort>>) {
        self.conversation_store = store;
    }

    pub fn set_run_recipe_store(&mut self, store: Option<Arc<dyn RunRecipeStorePort>>) {
        self.run_recipe_store = store;
    }

    pub fn set_user_profile_store(&mut self, store: Option<Arc<dyn UserProfileStorePort>>) {
        self.user_profile_store = store;
    }

    pub fn set_user_profile_key(&mut self, key: Option<String>) {
        self.user_profile_key = key;
        self.user_profile_context
            .set_current_key(self.user_profile_key.clone());
    }

    pub fn set_channel_registry(&mut self, registry: Option<Arc<dyn ChannelRegistryPort>>) {
        self.channel_registry = registry;
    }

    pub async fn switch_runtime_route(
        &mut self,
        config: &Config,
        provider_override: Option<&str>,
        model_override: Option<&str>,
        route_lane: Option<CapabilityLane>,
        route_candidate_index: Option<usize>,
        shared_memory: Arc<dyn UnifiedMemoryPort>,
        runtime_ports: AgentRuntimePorts,
    ) -> Result<()> {
        let mut effective_config = config.clone();
        if let Some(provider) = provider_override {
            effective_config.default_provider = Some(provider.to_string());
            if config.default_provider.as_deref() != Some(provider) {
                effective_config.api_key = None;
                effective_config.api_url = None;
            }
        }
        if let Some(model) = model_override {
            effective_config.default_model = Some(model.to_string());
        }

        let history = self.history.clone();
        let turn_count = self.turn_count;
        let memory_session_id = self.memory_session_id.clone();
        let last_turn_usage = self.last_turn_usage.clone();
        let last_turn_tool_facts = self.last_turn_tool_facts.clone();
        let observer = Arc::clone(&self.observer);
        let dialogue_state_store = self.dialogue_state_store.clone();
        let run_recipe_store = self.run_recipe_store.clone();
        let channel_registry = self.channel_registry.clone();
        let user_profile_key = self.user_profile_key.clone();
        let user_profile_context = Arc::clone(&self.user_profile_context);
        let turn_defaults_context = Arc::clone(&self.turn_defaults_context);
        let scoped_instruction_context = self.scoped_instruction_context.clone();
        let history_compaction_cache = Arc::clone(&self.history_compaction_cache);
        let mut runtime_ports = runtime_ports;
        runtime_ports
            .history_compaction_cache
            .get_or_insert(history_compaction_cache);

        let mut rebuilt = Self::from_config_with_runtime_context(
            &effective_config,
            Some(shared_memory),
            runtime_ports,
        )
        .await?;
        rebuilt.observer = observer;
        rebuilt.history = history;
        rebuilt.turn_count = turn_count;
        rebuilt.memory_session_id = memory_session_id;
        rebuilt.last_turn_usage = last_turn_usage;
        rebuilt.last_turn_tool_facts = last_turn_tool_facts;
        rebuilt.last_turn_tool_repair = None;
        rebuilt.recent_turn_tool_repairs.clear();
        rebuilt.recent_turn_admissions.clear();
        rebuilt.recent_runtime_assumptions.clear();
        rebuilt.recent_runtime_calibrations.clear();
        rebuilt.active_lane = route_lane;
        rebuilt.active_candidate_index = route_candidate_index;
        rebuilt.dialogue_state_store = dialogue_state_store;
        rebuilt.conversation_store = self.conversation_store.clone();
        rebuilt.run_recipe_store = run_recipe_store;
        rebuilt.user_profile_store = self.user_profile_store.clone();
        rebuilt.user_profile_key = user_profile_key;
        rebuilt.user_profile_context = user_profile_context;
        rebuilt.turn_defaults_context = turn_defaults_context;
        rebuilt.scoped_instruction_context = scoped_instruction_context;
        rebuilt.channel_registry = channel_registry;

        *self = rebuilt;
        Ok(())
    }

    pub async fn from_config(config: &Config) -> Result<Self> {
        Self::from_config_with_runtime_context(config, None, AgentRuntimePorts::default()).await
    }

    /// Create an agent from config, optionally reusing a shared memory backend.
    /// In daemon mode, `shared_memory` avoids opening a second SurrealKV lock.
    pub async fn from_config_with_memory(
        config: &Config,
        shared_memory: Option<Arc<dyn UnifiedMemoryPort>>,
    ) -> Result<Self> {
        Self::from_config_with_runtime_context(config, shared_memory, AgentRuntimePorts::default())
            .await
    }

    /// Create an agent from config with optional shared runtime ports used by
    /// web/gateway paths for richer tool surfaces and turn context assembly.
    pub async fn from_config_with_runtime_context(
        config: &Config,
        shared_memory: Option<Arc<dyn UnifiedMemoryPort>>,
        runtime_ports: AgentRuntimePorts,
    ) -> Result<Self> {
        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::create_observer(
            &config.observability,
        ));
        let runtime: Arc<dyn runtime::RuntimeAdapter> =
            Arc::from(runtime::create_runtime(&config.runtime)?);
        let security = Arc::new(security_policy_from_config(
            &config.autonomy,
            &config.workspace_dir,
        ));

        let resolved_agent_id = crate::agent::resolve_agent_id(config);
        let (memory, surreal_handle): (Arc<dyn UnifiedMemoryPort>, _) =
            if let Some(mem) = shared_memory {
                (mem, None)
            } else {
                let memory_backend = synapse_memory::create_memory(
                    &config.memory,
                    &config.workspace_dir,
                    &resolved_agent_id,
                    config.api_key.as_deref(),
                )
                .await?;
                (memory_backend.memory, memory_backend.surreal)
            };

        let composio_key = if config.composio.enabled {
            config.composio.api_key.as_deref()
        } else {
            None
        };
        let composio_entity_id = if config.composio.enabled {
            Some(config.composio.entity_id.as_str())
        } else {
            None
        };
        let resolved_user_profile_store: Arc<dyn UserProfileStorePort> = if let Some(store) =
            runtime_ports.user_profile_store.clone()
        {
            store
        } else if let Some(db) = surreal_handle.as_ref() {
            Arc::new(synapse_memory::SurrealUserProfileStore::new(Arc::clone(db)))
        } else {
            let profile_path = config
                .config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("user_profiles.json");
            match synapse_infra::user_profile_store::FileUserProfileStore::new(&profile_path) {
                Ok(store) => Arc::new(store),
                Err(error) => {
                    tracing::warn!(
                        path = %profile_path.display(),
                        %error,
                        "Failed to initialize persistent user profile store, falling back to memory"
                    );
                    Arc::new(
                        synapse_domain::ports::user_profile_store::InMemoryUserProfileStore::new(),
                    )
                }
            }
        };
        let user_profile_context: Arc<dyn UserProfileContextPort> = runtime_ports
            .user_profile_context
            .clone()
            .unwrap_or_else(|| Arc::new(InMemoryUserProfileContext::new()));
        let turn_defaults_context: Arc<dyn TurnDefaultsContextPort> = runtime_ports
            .turn_defaults_context
            .clone()
            .unwrap_or_else(|| Arc::new(InMemoryTurnDefaultsContext::new()));

        let (tools, _delegate_handle, _) = tools::all_tools_with_runtime(
            Arc::new(config.clone()),
            &security,
            runtime,
            memory.clone(),
            composio_key,
            composio_entity_id,
            &config.browser,
            &config.http_request,
            &config.web_fetch,
            &config.workspace_dir,
            &config.agents,
            config.api_key.as_deref(),
            config,
            None,
            None,
            surreal_handle.clone(),
            runtime_ports.conversation_context.clone(),
            runtime_ports.conversation_store.clone(),
            runtime_ports.channel_registry.clone(),
            None, // standing_order_store
            Some(Arc::clone(&resolved_user_profile_store)),
            Some(Arc::clone(&user_profile_context)),
            Some(Arc::clone(&turn_defaults_context)),
            runtime_ports.run_recipe_store.clone(),
        );

        // Bootstrap core memory blocks from workspace files (USER.md → user_knowledge).
        {
            use synapse_domain::application::services::bootstrap_core_memory as bootstrap;
            let user_md = bootstrap::read_workspace_file(&config.workspace_dir, "USER.md");
            let soul_md = bootstrap::read_workspace_file(&config.workspace_dir, "SOUL.md");
            let files: Vec<(&str, Option<&str>)> = vec![
                ("USER.md", user_md.as_deref()),
                ("SOUL.md", soul_md.as_deref()),
            ];
            bootstrap::ensure_core_blocks_seeded(memory.as_ref(), &resolved_agent_id, &files).await;
        }

        let provider_name = config.default_provider.as_deref().unwrap_or("openrouter");

        let model_name = config
            .default_model
            .as_deref()
            .unwrap_or_else(|| {
                synapse_domain::config::model_catalog::provider_default_model(provider_name)
                    .unwrap_or("default")
            })
            .to_string();

        let provider_runtime_options =
            synapse_providers::provider_runtime_options_from_config(config);

        let provider: Box<dyn Provider> = synapse_providers::create_routed_provider_with_options(
            provider_name,
            config.api_key.as_deref(),
            config.api_url.as_deref(),
            &config.reliability,
            &config.model_routes,
            &model_name,
            &provider_runtime_options,
        )?;

        // Wrap memory with ConsolidatingMemory for LLM-driven consolidation + entity extraction.
        // Clone provider into Arc for ConsolidatingMemory; keep Box for Agent builder.
        let provider_for_consolidation: Arc<dyn Provider> =
            Arc::from(synapse_providers::create_routed_provider_with_options(
                provider_name,
                config.api_key.as_deref(),
                config.api_url.as_deref(),
                &config.reliability,
                &config.model_routes,
                &model_name,
                &provider_runtime_options,
            )?);
        let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(
            crate::memory_adapters::instrumented::InstrumentedMemory::new(Arc::new(
                crate::memory_adapters::memory_adapter::ConsolidatingMemory::new(
                    memory,
                    Arc::clone(&provider_for_consolidation),
                    model_name.clone(),
                    resolved_agent_id.clone(),
                    None,
                ),
            )),
        );

        let dispatcher_choice = config.agent.tool_dispatcher.as_str();
        let tool_dispatcher: Box<dyn ToolDispatcher> = match dispatcher_choice {
            "native" => Box::new(NativeToolDispatcher),
            "xml" => Box::new(XmlToolDispatcher),
            _ if provider.supports_native_tools() => Box::new(NativeToolDispatcher),
            _ => Box::new(XmlToolDispatcher),
        };

        let route_model_by_hint: HashMap<String, String> = config
            .model_routes
            .iter()
            .map(|route| (route.hint.clone(), route.model.clone()))
            .collect();
        let available_hints: Vec<String> = route_model_by_hint.keys().cloned().collect();
        let profile_catalog =
            crate::runtime_routes::WorkspaceModelProfileCatalog::from_config(config);
        let current_model_profile =
            synapse_domain::application::services::model_lane_resolution::resolve_candidate_profile(
                provider_name,
                &model_name,
                &synapse_domain::config::schema::ModelCandidateProfileConfig::default(),
                Some(&profile_catalog),
            );

        let response_cache = if config.memory.response_cache_enabled {
            surreal_handle.map(|db| {
                Arc::new(
                    synapse_memory::response_cache::ResponseCache::with_hot_cache_surreal(
                        db,
                        config.memory.response_cache_ttl_minutes,
                        config.memory.response_cache_max_entries,
                        config.memory.response_cache_hot_entries,
                    ),
                )
            })
        } else {
            None
        };

        let summary_route = resolve_summary_route(config, &model_name);
        let history_summary_generator: Option<Arc<dyn SummaryGeneratorPort>> = {
            let provider_result: Result<Arc<dyn Provider>> =
                if let Some(ref summary_provider_name) = summary_route.provider {
                    let api_key = summary_route
                        .api_key_env
                        .as_deref()
                        .and_then(|env| std::env::var(env).ok())
                        .or_else(|| summary_route.api_key.clone());
                    synapse_providers::create_provider_with_options(
                        summary_provider_name,
                        api_key.as_deref(),
                        &provider_runtime_options,
                    )
                    .map(Arc::from)
                } else {
                    Ok(Arc::clone(&provider_for_consolidation))
                };

            match provider_result {
                Ok(provider) => {
                    tracing::debug!(
                        summary_route_source = summary_route.source.as_str(),
                        summary_provider =
                            summary_route.provider.as_deref().unwrap_or(provider_name),
                        summary_model = summary_route.model.as_str(),
                        "Agent history compaction summary lane ready"
                    );
                    Some(Arc::new(
                        crate::memory_adapters::summary_generator_adapter::ProviderSummaryGenerator::new(
                            provider,
                            summary_route.model.clone(),
                            summary_route.temperature,
                        ),
                    ))
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        summary_route_source = summary_route.source.as_str(),
                        summary_model = summary_route.model.as_str(),
                        "Failed to initialize agent history summary generator; live compaction disabled"
                    );
                    None
                }
            }
        };
        let history_compaction_cache = runtime_ports
            .history_compaction_cache
            .clone()
            .unwrap_or_else(|| {
                crate::runtime::history_compaction_cache::shared_history_compaction_cache(
                    &config.workspace_dir,
                    &resolved_agent_id,
                )
            });

        Agent::builder()
            .provider(provider)
            .tools(tools)
            .memory(memory)
            .observer(observer)
            .response_cache(response_cache)
            .history_summary_generator(history_summary_generator)
            .history_compaction_cache(Some(history_compaction_cache))
            .tool_dispatcher(tool_dispatcher)
            .prompt_budget({
                let mut b = config.memory.prompt_budget.to_prompt_budget();
                b.recall_min_relevance = config.memory.min_relevance_score;
                b
            })
            .continuation_policy(config.memory.prompt_budget.to_continuation_policy())
            .prompt_builder(SystemPromptBuilder::with_defaults())
            .config(config.agent.clone())
            .compression(config.compression.clone())
            .compression_overrides(config.compression_overrides.clone())
            .provider_name(provider_name.to_string())
            .provider_api_url(config.api_url.clone())
            .agent_id(resolved_agent_id)
            .model_name(model_name)
            .temperature(config.default_temperature)
            .workspace_dir(config.workspace_dir.clone())
            .classification_config(config.query_classification.clone())
            .available_hints(available_hints)
            .route_model_by_hint(route_model_by_hint)
            .route_model_preset(config.model_preset.clone())
            .route_model_lanes(config.model_lanes.clone())
            .route_model_routes(config.model_routes.clone())
            .current_model_profile(current_model_profile)
            .identity_config(config.identity.clone())
            .conversation_store(runtime_ports.conversation_store)
            .run_recipe_store(runtime_ports.run_recipe_store)
            .user_profile_store(Some(resolved_user_profile_store))
            .user_profile_context(Some(user_profile_context))
            .turn_defaults_context(Some(turn_defaults_context))
            .scoped_instruction_context(runtime_ports.scoped_instruction_context)
            .channel_registry(runtime_ports.channel_registry)
            .skills(crate::skills::load_skills_with_config(
                &config.workspace_dir,
                config,
            ))
            .skills_prompt_mode(config.skills.prompt_injection_mode)
            .auto_save(config.memory.auto_save)
            .build()
    }

    fn trim_history(&mut self) {
        let max = self.config.max_history_messages;
        if self.history.len() <= max {
            return;
        }

        let mut system_messages = Vec::new();
        let mut other_messages = Vec::new();

        for msg in self.history.drain(..) {
            match &msg {
                ConversationMessage::Chat(chat) if chat.role == "system" => {
                    system_messages.push(msg);
                }
                _ => other_messages.push(msg),
            }
        }

        if other_messages.len() > max {
            let drop_count = other_messages.len() - max;
            other_messages.drain(0..drop_count);
        }

        self.history = system_messages;
        self.history.extend(other_messages);
        let stats = history_compaction::sanitize_tool_protocol_after_compaction(&mut self.history);
        if stats.removed_orphan_results > 0 || stats.inserted_stub_results > 0 {
            tracing::debug!(
                removed_orphan_tool_results = stats.removed_orphan_results,
                inserted_tool_result_stubs = stats.inserted_stub_results,
                "Sanitized provider tool protocol after history trim"
            );
        }
    }

    fn project_non_system_history_for_compaction(&self) -> (Vec<ChatMessage>, Vec<usize>) {
        let mut projected = Vec::new();
        let mut original_indices = Vec::new();

        for (index, message) in self.history.iter().enumerate() {
            match message {
                ConversationMessage::Chat(chat) if chat.role != "system" => {
                    projected.push(chat.clone());
                    original_indices.push(index);
                }
                ConversationMessage::AssistantToolCalls {
                    text,
                    tool_calls,
                    reasoning_content,
                } => {
                    if let Some(text) = text.as_deref() {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            projected.push(ChatMessage::assistant(trimmed.to_string()));
                            original_indices.push(index);
                        }
                    }
                    if let Some(reasoning) = reasoning_content.as_deref() {
                        let trimmed = reasoning.trim();
                        if !trimmed.is_empty() {
                            projected.push(ChatMessage::assistant(format!(
                                "[assistant-reasoning]\n{trimmed}"
                            )));
                            original_indices.push(index);
                        }
                    }
                    for call in tool_calls {
                        projected.push(ChatMessage::assistant(format!(
                            "[tool-call {}]\n{} {}",
                            call.id, call.name, call.arguments
                        )));
                        original_indices.push(index);
                    }
                }
                ConversationMessage::ToolResults(results) => {
                    for result in results {
                        projected.push(ChatMessage {
                            role: "tool".to_string(),
                            content: result.content.clone(),
                        });
                        original_indices.push(index);
                    }
                }
                _ => {}
            }
        }

        (projected, original_indices)
    }

    fn history_compression_for_route(
        &self,
        provider: &str,
        model: &str,
        lane: Option<synapse_domain::config::schema::CapabilityLane>,
        hint: Option<&str>,
    ) -> ContextCompressionConfig {
        history_compaction::resolve_context_compression_config_for_route(
            &self.compression,
            &self.compression_overrides,
            provider,
            model,
            lane,
            hint,
        )
    }

    fn history_compression_policy(&self) -> HistoryCompressionPolicy {
        HistoryCompressionPolicy::from(&self.history_compression_for_route(
            &self.provider_name,
            &self.model_name,
            None,
            None,
        ))
    }

    fn history_compaction_context_window_tokens_for_profile(
        profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    ) -> Option<usize> {
        (profile.context_window_confidence() >= ResolvedModelProfileConfidence::Medium)
            .then_some(profile.context_window_tokens)
            .flatten()
    }

    fn history_compaction_context_window_tokens(&self) -> Option<usize> {
        Self::history_compaction_context_window_tokens_for_profile(&self.current_model_profile)
    }

    fn history_compaction_max_output_tokens_for_profile(
        profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    ) -> Option<usize> {
        (profile.max_output_confidence() >= ResolvedModelProfileConfidence::Medium)
            .then_some(profile.max_output_tokens)
            .flatten()
    }

    fn history_compaction_max_output_tokens(&self) -> Option<usize> {
        Self::history_compaction_max_output_tokens_for_profile(&self.current_model_profile)
    }

    fn history_compaction_threshold_tokens_for_profile(
        &self,
        policy: &HistoryCompressionPolicy,
        profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    ) -> usize {
        let Some(context_window_tokens) =
            Self::history_compaction_context_window_tokens_for_profile(profile)
        else {
            return history_compaction::history_compression_threshold_tokens(
                self.config.max_context_tokens.max(1),
                policy,
            );
        };

        let reserved_output_tokens = provider_context_reserved_output_headroom_tokens(
            Some(context_window_tokens),
            Self::history_compaction_max_output_tokens_for_profile(profile),
            128,
        );
        let safe_input_tokens = context_window_tokens.saturating_sub(reserved_output_tokens);
        if safe_input_tokens == 0 {
            return history_compaction::history_compression_threshold_tokens(
                self.config.max_context_tokens.max(1),
                policy,
            );
        }

        history_compaction::history_compression_threshold_tokens(safe_input_tokens, policy)
    }

    fn history_compaction_threshold_tokens(&self, policy: &HistoryCompressionPolicy) -> usize {
        self.history_compaction_threshold_tokens_for_profile(policy, &self.current_model_profile)
    }

    fn last_provider_input_tokens(&self) -> Option<usize> {
        self.last_turn_usage
            .as_ref()
            .and_then(|usage| usage.input_tokens)
            .and_then(|tokens| usize::try_from(tokens).ok())
            .filter(|tokens| *tokens > 0)
    }

    fn latest_compaction_summary_text(&self) -> Option<String> {
        self.history.iter().rev().find_map(|message| match message {
            ConversationMessage::Chat(chat)
                if history_compaction::is_compaction_summary(&chat.content) =>
            {
                Some(
                    chat.content
                        .trim_start_matches(history_compaction::COMPACTION_SUMMARY_PREFIX)
                        .trim()
                        .to_string(),
                )
            }
            _ => None,
        })
    }

    pub fn history_compaction_cache_stats(&self) -> ContextCacheStats {
        self.history_compaction_cache_stats_for_compression(&self.compression)
    }

    pub fn history_compaction_cache_stats_for_route(
        &self,
        provider: &str,
        model: &str,
        lane: Option<synapse_domain::config::schema::CapabilityLane>,
        hint: Option<&str>,
    ) -> ContextCacheStats {
        let compression = self.history_compression_for_route(provider, model, lane, hint);
        self.history_compaction_cache_stats_for_compression(&compression)
    }

    fn history_compaction_cache_stats_for_compression(
        &self,
        compression: &ContextCompressionConfig,
    ) -> ContextCacheStats {
        self.history_compaction_cache.stats(compression)
    }

    async fn maybe_compact_history(&mut self) -> bool {
        let compression =
            self.history_compression_for_route(&self.provider_name, &self.model_name, None, None);
        let profile = self.current_model_profile.clone();
        self.maybe_compact_history_with_route(&compression, &profile)
            .await
    }

    async fn maybe_compact_history_with_route(
        &mut self,
        compression: &ContextCompressionConfig,
        profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
    ) -> bool {
        self.maybe_compact_history_with_route_and_limits(
            compression,
            profile,
            self.config.max_history_messages,
            self.last_provider_input_tokens(),
        )
        .await
    }

    async fn maybe_compact_history_with_route_and_limits(
        &mut self,
        compression: &ContextCompressionConfig,
        profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
        max_history_messages: usize,
        observed_provider_input_tokens: Option<usize>,
    ) -> bool {
        let policy = HistoryCompressionPolicy::from(compression);
        if !policy.enabled {
            return false;
        }

        let Some(summary_generator) = self.history_summary_generator.clone() else {
            return false;
        };
        if let Err(error) = self.history_compaction_cache.load(compression).await {
            tracing::debug!(%error, "Failed to load history compaction cache");
        }

        let (projected, original_indices) = self.project_non_system_history_for_compaction();
        let threshold_tokens =
            self.history_compaction_threshold_tokens_for_profile(&policy, profile);
        let Some((start, compact_end, transcript)) =
            history_compaction::prepare_compaction_with_policy_and_observed_tokens(
                &projected,
                max_history_messages,
                threshold_tokens,
                &policy,
                observed_provider_input_tokens,
            )
        else {
            return false;
        };

        let original_start = *original_indices
            .get(start)
            .expect("compaction start should map to original history");
        let original_end = original_indices
            .get(compact_end.saturating_sub(1))
            .map(|index| index + 1)
            .expect("compaction end should map to original history");

        let context_window_tokens =
            Self::history_compaction_context_window_tokens_for_profile(profile);
        let cache_key = history_compaction_cache_key(&transcript, &policy, context_window_tokens);
        let summary_raw = if let Some(summary) = match self
            .history_compaction_cache
            .get_summary(compression, &cache_key)
            .await
        {
            Ok(summary) => summary,
            Err(error) => {
                tracing::debug!(%error, "Failed to read history compaction cache entry");
                None
            }
        } {
            tracing::debug!(
                cache_key = %cache_key,
                "Live agent history compaction summary cache hit"
            );
            summary
        } else {
            let previous_summary = self.latest_compaction_summary_text();
            let prompt = history_compaction::compaction_summarizer_prompt_with_policy(
                &transcript,
                previous_summary.as_deref(),
                &policy,
                context_window_tokens,
            );
            match summary_generator.generate_summary(&prompt).await {
                Ok(summary) => {
                    let summary = summary.trim().to_string();
                    if !summary.is_empty() {
                        if let Err(error) = self
                            .history_compaction_cache
                            .remember_summary(compression, cache_key, summary.clone())
                            .await
                        {
                            tracing::debug!(%error, "Failed to persist history compaction cache entry");
                        }
                    }
                    summary
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "Live agent history compaction summary failed; using transcript fallback"
                    );
                    String::new()
                }
            }
        };
        let summary = if summary_raw.is_empty() {
            synapse_domain::domain::util::truncate_with_ellipsis(
                &transcript,
                policy.max_summary_chars,
            )
        } else {
            synapse_domain::domain::util::truncate_with_ellipsis(
                &summary_raw,
                policy.max_summary_chars,
            )
        };
        self.history.splice(
            original_start..original_end,
            std::iter::once(ConversationMessage::Chat(ChatMessage::assistant(format!(
                "{}{}",
                history_compaction::COMPACTION_SUMMARY_PREFIX,
                summary.trim()
            )))),
        );
        let stats = history_compaction::sanitize_tool_protocol_after_compaction(&mut self.history);
        if stats.removed_orphan_results > 0 || stats.inserted_stub_results > 0 {
            tracing::debug!(
                removed_orphan_tool_results = stats.removed_orphan_results,
                inserted_tool_result_stubs = stats.inserted_stub_results,
                "Sanitized provider tool protocol after history compaction"
            );
        }
        true
    }

    pub async fn compact_for_session_hygiene(&mut self) -> bool {
        let mut compression =
            self.history_compression_for_route(&self.provider_name, &self.model_name, None, None);
        let high_water_threshold =
            CONTEXT_SAFETY_CEILING_NUMERATOR as f64 / CONTEXT_SAFETY_CEILING_DENOMINATOR as f64;
        compression.threshold = compression.threshold.max(high_water_threshold);
        let profile = self.current_model_profile.clone();
        self.maybe_compact_history_with_route_and_limits(
            &compression,
            &profile,
            SESSION_HYGIENE_HARD_MESSAGE_LIMIT,
            None,
        )
        .await
    }

    fn build_system_prompt(&self) -> Result<String> {
        let instructions = self.tool_dispatcher.prompt_instructions(&self.tools);
        let ctx = PromptContext {
            workspace_dir: &self.workspace_dir,
            model_name: &self.model_name,
            tools: &self.tools,
            skills: &self.skills,
            skills_prompt_mode: self.skills_prompt_mode,
            identity_config: Some(&self.identity_config),
            dispatcher_instructions: &instructions,
            tool_specs_are_out_of_band: self.tool_dispatcher.should_send_tool_specs(),
        };
        let (prompt, section_stats) = self.prompt_builder.build_with_stats(&ctx)?;
        let rendered_stats = section_stats
            .iter()
            .map(|stat| format!("{}={}", stat.name, stat.chars))
            .collect::<Vec<_>>()
            .join(", ");
        tracing::info!(
            target: "agent.system_prompt",
            total_chars = prompt.chars().count(),
            sections = rendered_stats,
            "Built system prompt"
        );
        Ok(prompt)
    }

    async fn execute_tool_call(&self, call: &ParsedToolCall) -> ToolExecutionResult {
        let normalized_call = normalize_tool_call(call);
        let outcome = execute_one_tool(
            &normalized_call.name,
            normalized_call.arguments.clone(),
            &self.tools,
            None,
            self.observer.as_ref(),
            None,
            None,
            None,
        )
        .await
        .unwrap_or_else(|error| crate::agent::ToolExecutionOutcome {
            output: format!("Error executing {}: {error}", normalized_call.name),
            success: false,
            error_reason: Some(synapse_security::scrub_credentials(&format!(
                "Error executing {}: {error}",
                normalized_call.name
            ))),
            duration: Duration::ZERO,
            tool_facts: Vec::new(),
            repair_trace: Some(classify_tool_execution_error(&normalized_call.name, &error)),
        });

        ToolExecutionResult {
            name: call.name.clone(),
            output: outcome.output,
            success: outcome.success,
            tool_call_id: call.tool_call_id.clone(),
            tool_facts: outcome.tool_facts,
            repair_trace: outcome.repair_trace,
        }
    }

    async fn execute_tools(&self, calls: &[ParsedToolCall]) -> Vec<ToolExecutionResult> {
        if !self.config.parallel_tools {
            let mut results = Vec::with_capacity(calls.len());
            for call in calls {
                results.push(self.execute_tool_call(call).await);
            }
            return results;
        }

        let futs: Vec<_> = calls
            .iter()
            .map(|call| self.execute_tool_call(call))
            .collect();
        futures_util::future::join_all(futs).await
    }

    fn tool_call_signature(&self, call: &ParsedToolCall) -> Option<(String, String)> {
        let normalized_call = normalize_tool_call(call);
        if self
            .config
            .tool_call_dedup_exempt
            .iter()
            .any(|tool| tool == &normalized_call.name)
        {
            return None;
        }
        Some((
            normalized_call.name,
            canonicalize_tool_args(&normalized_call.arguments).to_string(),
        ))
    }

    fn deduplicate_turn_calls(&self, calls: Vec<ParsedToolCall>) -> Vec<ParsedToolCall> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(calls.len());

        for call in calls {
            let Some(signature) = self.tool_call_signature(&call) else {
                deduped.push(call);
                continue;
            };

            if seen.insert(signature) {
                deduped.push(call);
            }
        }

        deduped
    }

    async fn execute_tools_with_cache(
        &self,
        calls: &[ParsedToolCall],
        cache: &mut HashMap<(String, String), ToolExecutionResult>,
    ) -> Vec<ToolExecutionResult> {
        let mut results: Vec<Option<ToolExecutionResult>> = vec![None; calls.len()];
        let mut uncached = Vec::new();
        let mut pending_by_signature: HashMap<(String, String), usize> = HashMap::new();
        let mut pending_waiters: HashMap<usize, Vec<(usize, Option<String>)>> = HashMap::new();

        for (index, call) in calls.iter().cloned().enumerate() {
            if let Some(signature) = self.tool_call_signature(&call) {
                if let Some(cached) = cache.get(&signature) {
                    let mut reused = cached.clone();
                    reused.tool_call_id = call.tool_call_id.clone();
                    tracing::info!(
                        tool = %call.name,
                        "reusing cached tool result for duplicate call within turn"
                    );
                    results[index] = Some(reused);
                    continue;
                }
                if let Some(existing_uncached_index) = pending_by_signature.get(&signature).copied()
                {
                    pending_waiters
                        .entry(existing_uncached_index)
                        .or_default()
                        .push((index, call.tool_call_id.clone()));
                    continue;
                }
                let uncached_index = uncached.len();
                pending_by_signature.insert(signature.clone(), uncached_index);
                uncached.push((index, call, Some(signature)));
            } else {
                uncached.push((index, call, None));
            }
        }

        let pending_calls: Vec<ParsedToolCall> =
            uncached.iter().map(|(_, call, _)| call.clone()).collect();
        let executed = self.execute_tools(&pending_calls).await;

        for (uncached_index, ((index, _, signature), result)) in
            uncached.into_iter().zip(executed.into_iter()).enumerate()
        {
            if let Some(signature) = signature {
                let mut stored = result.clone();
                stored.tool_call_id = None;
                cache.insert(signature, stored);
            }
            results[index] = Some(result);
            if let Some(waiters) = pending_waiters.remove(&uncached_index) {
                for (waiter_index, waiter_tool_call_id) in waiters {
                    let mut reused = results[index]
                        .as_ref()
                        .expect("primary result should be stored before waiters")
                        .clone();
                    reused.tool_call_id = waiter_tool_call_id;
                    results[waiter_index] = Some(reused);
                }
            }
        }

        results
            .into_iter()
            .map(|result| result.expect("every tool call should yield a result"))
            .collect()
    }

    fn classify_model(&self, user_message: &str) -> String {
        if let Some(decision) =
            super::classifier::classify_with_decision(&self.classification_config, user_message)
        {
            if self.available_hints.contains(&decision.hint) {
                let resolved_model = self
                    .route_model_by_hint
                    .get(&decision.hint)
                    .map(String::as_str)
                    .unwrap_or("unknown");
                tracing::info!(
                    target: "query_classification",
                    hint = decision.hint.as_str(),
                    model = resolved_model,
                    rule_priority = decision.priority,
                    message_length = user_message.len(),
                    "Classified message route"
                );
                return format!("hint:{}", decision.hint);
            }
        }
        self.model_name.clone()
    }

    fn build_turn_routing_config(&self) -> Config {
        let mut config = Config::default();
        config.default_provider = Some(self.provider_name.clone());
        config.default_model = Some(self.model_name.clone());
        config.model_preset = self.route_model_preset.clone();
        config.model_lanes = self.route_model_lanes.clone();
        config.model_routes = self.route_model_routes.clone();
        config.compression = self.compression.clone();
        config.compression_overrides = self.compression_overrides.clone();
        config
    }

    pub async fn turn(&mut self, user_message: &str) -> Result<String> {
        struct TurnDefaultsGuard {
            port: Arc<dyn TurnDefaultsContextPort>,
        }

        impl Drop for TurnDefaultsGuard {
            fn drop(&mut self) {
                self.port.set_current(None);
            }
        }

        self.last_turn_usage = None;
        self.last_turn_tool_facts.clear();
        self.last_turn_tool_repair = None;
        self.run_runtime_trace_janitor(chrono::Utc::now().timestamp());
        if self.history.is_empty() {
            let system_prompt = self.build_system_prompt()?;
            self.history
                .push(ConversationMessage::Chat(ChatMessage::system(
                    system_prompt,
                )));
        }

        let dialogue_state = self.memory_session_id.as_deref().and_then(|session_id| {
            self.dialogue_state_store
                .as_ref()
                .and_then(|store| store.get(session_id))
        });
        let user_profile = match (
            self.user_profile_store.as_ref(),
            self.user_profile_key.as_deref(),
        ) {
            (Some(store), Some(key)) => store.load(key),
            _ => None,
        };
        let configured_delivery_target = self
            .channel_registry
            .as_ref()
            .and_then(|registry| registry.configured_delivery_target());
        let turn_interpretation = turn_interpretation::build_turn_interpretation(
            Some(self.memory.as_ref()),
            user_message,
            user_profile,
            None,
            dialogue_state.as_ref(),
            configured_delivery_target.clone(),
        )
        .await;
        let interpretation_block = turn_interpretation.as_ref().and_then(|interpretation| {
            turn_interpretation::format_turn_interpretation_for_turn(user_message, interpretation)
        });
        let resolved_turn_defaults =
            synapse_domain::application::services::turn_defaults_resolution::resolve_turn_defaults(
                turn_interpretation.as_ref(),
                configured_delivery_target,
            );
        let scoped_context_block = if let (Some(loader), Some(plan)) = (
            self.scoped_instruction_context.as_ref(),
            build_scoped_instruction_plan(user_message, turn_interpretation.as_ref()),
        ) {
            let scoped_pressure = {
                let snapshot =
                    self.build_provider_prompt_snapshot_for_profile(&self.current_model_profile);
                assess_provider_context_budget(
                    provider_context_budget_input_from_stats_for_profile(
                        &snapshot.stats,
                        &self.current_model_profile,
                    ),
                )
                .tier
            };
            match adjust_scoped_instruction_plan_for_context_pressure(plan, scoped_pressure) {
                Some(plan) => {
                    let snippets = loader
                        .load_scoped_instructions(ScopedInstructionRequest {
                            session_id: self.memory_session_id.clone(),
                            path_hints: plan.hints.into_iter().map(|hint| hint.path).collect(),
                            max_files: plan.max_files,
                            max_total_chars: plan.max_total_chars,
                        })
                        .await
                        .unwrap_or_default();
                    format_scoped_instruction_block(&snippets)
                }
                None => {
                    tracing::debug!(
                        target: "agent.scoped_context",
                        pressure = provider_context_budget_tier_name(scoped_pressure),
                        "Skipped inferred scoped context under provider-context pressure"
                    );
                    None
                }
            }
        } else {
            None
        };
        self.turn_defaults_context
            .set_current(Some(resolved_turn_defaults.clone()));
        let _turn_defaults_guard = TurnDefaultsGuard {
            port: Arc::clone(&self.turn_defaults_context),
        };
        // ── Unified turn context assembly ──
        let continuation = if self.turn_count > 0 {
            Some(&self.continuation_policy)
        } else {
            None
        };
        let recent_admission_reasons = self
            .recent_turn_admissions
            .last()
            .map(|admission| admission.reasons.as_slice())
            .unwrap_or(&[]);
        let recent_admission_repair = self
            .recent_turn_admissions
            .last()
            .and_then(|admission| admission.recommended_action);
        let observed_assumptions = build_runtime_assumptions(RuntimeAssumptionInput {
            user_message,
            interpretation: turn_interpretation.as_ref(),
            recent_admission_repair,
            recent_admission_reasons,
        });
        self.recent_runtime_assumptions = merge_runtime_assumption_ledger(
            &self.recent_runtime_assumptions,
            &observed_assumptions,
        );
        let turn_ctx = tc::assemble_turn_context(
            self.memory.as_ref(),
            self.run_recipe_store.as_ref().map(|store| store.as_ref()),
            self.conversation_store.as_ref().map(|store| store.as_ref()),
            user_message,
            &self.agent_id,
            self.memory_session_id.as_deref(),
            turn_interpretation.as_ref(),
            &self.recent_turn_tool_repairs,
            recent_admission_reasons,
            recent_admission_repair,
            &self.prompt_budget,
            continuation,
        )
        .await;
        let formatted = turn_context_fmt::format_turn_context(&turn_ctx, &self.prompt_budget);

        // Core blocks → system message (MemGPT: always in prompt)
        if !formatted.core_blocks_system.is_empty() {
            const CORE_MEMORY_MARKER: &str = "[core-memory]\n";
            // Remove previous core-memory system message if present (avoid accumulation)
            self.history.retain(|msg| {
                if let ConversationMessage::Chat(chat) = msg {
                    !(chat.role == "system" && chat.content.starts_with(CORE_MEMORY_MARKER))
                } else {
                    true
                }
            });
            self.history
                .push(ConversationMessage::Chat(ChatMessage::system(format!(
                    "{CORE_MEMORY_MARKER}{}",
                    formatted.core_blocks_system
                ))));
        }
        if let Some(block) = interpretation_block {
            const INTERPRETATION_MARKER: &str = "[runtime-interpretation]\n";
            self.history.retain(|msg| {
                if let ConversationMessage::Chat(chat) = msg {
                    !(chat.role == "system" && chat.content.starts_with(INTERPRETATION_MARKER))
                } else {
                    true
                }
            });
            self.history
                .push(ConversationMessage::Chat(ChatMessage::system(block)));
        }
        self.upsert_scoped_context_block(scoped_context_block);
        if !formatted.resolution_system.is_empty() {
            const RESOLUTION_MARKER: &str = "[resolution-plan]\n";
            self.history.retain(|msg| {
                if let ConversationMessage::Chat(chat) = msg {
                    !(chat.role == "system" && chat.content.starts_with(RESOLUTION_MARKER))
                } else {
                    true
                }
            });
            self.history
                .push(ConversationMessage::Chat(ChatMessage::system(
                    formatted.resolution_system.clone(),
                )));
        }

        if self.auto_save
            && matches!(
                synapse_domain::application::services::memory_quality_governor::assess_autosave_write(
                    user_message,
                    synapse_domain::application::services::inbound_message_service::AUTOSAVE_MIN_MESSAGE_CHARS,
                ),
                synapse_domain::application::services::memory_quality_governor::AutosaveWriteVerdict::Write
            )
        {
            let user_key = autosave_memory_key("user_msg");
            let _ = self
                .memory
                .store(
                    &user_key,
                    user_message,
                    &MemoryCategory::Conversation,
                    self.memory_session_id.as_deref(),
                )
                .await;
        }

        // Store literal raw user message in history
        self.history
            .push(ConversationMessage::Chat(ChatMessage::user(
                user_message.to_string(),
            )));

        // Ephemeral enrichment prefix (for provider call only, not history).
        // No timestamp — it would defeat response caching and diverge from channels.
        let ephemeral_prefix = formatted.enrichment_prefix;
        self.turn_count += 1;

        let mut effective_model = self.classify_model(user_message);
        let mut effective_model_profile = self.current_model_profile.clone();
        let mut effective_lane = None;
        let mut effective_candidate_index = None;
        if !effective_model.starts_with("hint:") {
            let routing_config = self.build_turn_routing_config();
            let profile_catalog =
                crate::runtime_routes::WorkspaceModelProfileCatalog::with_provider_endpoint(
                    self.workspace_dir.clone(),
                    Some(&self.provider_name),
                    self.provider_api_url.as_deref(),
                );
            if let Some(route_override) = resolve_turn_route_override(
                &routing_config,
                user_message,
                &self.provider_name,
                &effective_model,
                &effective_model_profile,
                self.provider.supports_vision(),
                Some(&profile_catalog),
            ) {
                if route_override.provider == self.provider_name {
                    effective_lane = Some(route_override.lane);
                    effective_candidate_index = route_override.candidate_index;
                    effective_model = route_override.model;
                    effective_model_profile = synapse_domain::application::services::model_lane_resolution::resolve_lane_candidates(
                        &routing_config,
                        route_override.lane,
                        Some(&profile_catalog),
                    )
                    .into_iter()
                    .find(|candidate| {
                        candidate.provider == self.provider_name
                            && candidate.model == effective_model
                    })
                    .map(|candidate| candidate.profile)
                    .unwrap_or_else(|| {
                        synapse_domain::application::services::model_lane_resolution::resolve_candidate_profile(
                            &self.provider_name,
                            &effective_model,
                            &synapse_domain::config::schema::ModelCandidateProfileConfig::default(),
                            Some(&profile_catalog),
                        )
                    });
                }
            }
        }
        let effective_hint = effective_model.strip_prefix("hint:");
        let effective_compression = self.history_compression_for_route(
            &self.provider_name,
            &effective_model,
            effective_lane,
            effective_hint,
        );
        let turn_tool_specs =
            synapse_domain::application::services::turn_tool_narrowing::prepare_tool_specs_for_turn(
                self.tool_specs.clone(),
                turn_ctx.execution_guidance.as_ref(),
                &resolved_turn_defaults,
                user_message,
            );
        let mut tool_facts_this_turn = Vec::new();
        let mut last_tool_repair_this_turn = None::<ToolRepairTrace>;
        let mut tool_repairs_this_turn = Vec::<ToolRepairTrace>::new();

        let mut executed_call_cache: HashMap<(String, String), ToolExecutionResult> =
            HashMap::new();

        for _ in 0..self.config.max_tool_iterations {
            let snapshot =
                self.build_provider_prompt_snapshot_for_profile(&effective_model_profile);
            let provider_context_input = provider_context_budget_input_from_stats_for_profile(
                &snapshot.stats,
                &effective_model_profile,
            );
            let budget_assessment = assess_provider_context_budget(provider_context_input);
            let provider_capabilities = self.provider.capabilities();
            let native_context_policy =
                resolve_provider_native_context_policy(ProviderNativeContextPolicyInput {
                    profile: &effective_model_profile,
                    provider_prompt_caching: provider_capabilities.prompt_caching,
                    operator_prompt_caching_enabled: self.config.prompt_caching,
                });
            let admission_decision = assess_turn_admission(TurnAdmissionInput {
                config: None,
                user_message,
                execution_guidance: turn_ctx.execution_guidance.as_ref(),
                tool_specs: &turn_tool_specs,
                current_provider: &self.provider_name,
                current_model: &effective_model,
                current_lane: effective_lane,
                current_profile: &effective_model_profile,
                provider_capabilities: &provider_capabilities,
                provider_context: provider_context_input,
                catalog: None,
            });
            let observed_at_unix = chrono::Utc::now().timestamp();
            let admission_state = RouteAdmissionState {
                observed_at_unix,
                snapshot: admission_decision.snapshot.clone(),
                reasons: admission_decision.reasons.clone(),
                recommended_action: admission_decision.recommended_action,
            };
            self.recent_turn_admissions =
                synapse_domain::application::services::route_admission_history::append_route_admission_state(
                    &self.recent_turn_admissions,
                    Some(admission_state),
                    observed_at_unix,
                );
            let observed_assumptions = build_runtime_assumptions(RuntimeAssumptionInput {
                user_message,
                interpretation: turn_interpretation.as_ref(),
                recent_admission_repair: admission_decision.recommended_action,
                recent_admission_reasons: &admission_decision.reasons,
            });
            self.recent_runtime_assumptions = merge_runtime_assumption_ledger(
                &self.recent_runtime_assumptions,
                &observed_assumptions,
            );
            let system_breakdown = system_message_breakdown(&self.history)
                .into_iter()
                .map(|(name, chars)| format!("{name}={chars}"))
                .collect::<Vec<_>>()
                .join(", ");
            let condensation_plan = admission_decision.condensation_plan;
            tracing::info!(
                target: "agent.provider_context",
                system_messages = snapshot.stats.system_messages,
                system_chars = snapshot.stats.system_chars,
                stable_system_chars = snapshot.stats.stable_system_chars,
                dynamic_system_chars = snapshot.stats.dynamic_system_chars,
                bootstrap_chars = snapshot.stats.bootstrap_chars,
                core_memory_chars = snapshot.stats.core_memory_chars,
                runtime_interpretation_chars = snapshot.stats.runtime_interpretation_chars,
                scoped_context_chars = snapshot.stats.scoped_context_chars,
                resolution_chars = snapshot.stats.resolution_chars,
                system_breakdown = system_breakdown,
                prior_chat_messages = snapshot.stats.prior_chat_messages,
                prior_chat_chars = snapshot.stats.prior_chat_chars,
                current_turn_messages = snapshot.stats.current_turn_messages,
                current_turn_chars = snapshot.stats.current_turn_chars,
                total_messages = snapshot.stats.total_messages,
                total_chars = snapshot.stats.total_chars,
                context_estimated_total_tokens = budget_assessment.snapshot.estimated_total_tokens,
                context_chars_over_target = budget_assessment.snapshot.chars_over_target,
                context_chars_over_ceiling = budget_assessment.snapshot.chars_over_ceiling,
                context_turn_shape = provider_context_turn_shape_name(budget_assessment.turn_shape),
                context_budget_tier = provider_context_budget_tier_name(budget_assessment.tier),
                context_target_total_chars = budget_assessment.target_total_chars,
                context_ceiling_total_chars = budget_assessment.ceiling_total_chars,
                context_target_total_tokens = budget_assessment.snapshot.target_total_tokens,
                context_ceiling_total_tokens = budget_assessment.snapshot.ceiling_total_tokens,
                context_protected_chars = budget_assessment.snapshot.protected_chars,
                context_removable_chars = budget_assessment.snapshot.removable_chars,
                context_protected_tokens = budget_assessment.snapshot.protected_tokens,
                context_removable_tokens = budget_assessment.snapshot.removable_tokens,
                context_tokens_headroom_to_target =
                    budget_assessment.snapshot.tokens_headroom_to_target,
                context_tokens_headroom_to_ceiling =
                    budget_assessment.snapshot.tokens_headroom_to_ceiling,
                context_primary_ballast = budget_assessment
                    .snapshot
                    .primary_ballast_artifact
                    .map(provider_context_artifact_name)
                    .unwrap_or("none"),
                context_condensation_mode = condensation_plan
                    .map(|plan| provider_context_condensation_mode_name(plan.mode))
                    .unwrap_or("none"),
                context_condensation_target = condensation_plan
                    .and_then(|plan| plan.target_artifact)
                    .map(provider_context_artifact_name)
                    .unwrap_or("none"),
                context_condensation_min_reclaim_chars =
                    condensation_plan.map(|plan| plan.minimum_reclaim_chars).unwrap_or(0),
                context_condensation_prefers_cached_artifact =
                    condensation_plan.is_some_and(|plan| plan.prefer_cached_artifact),
                admission_intent = turn_intent_name(admission_decision.snapshot.intent),
                admission_pressure = context_pressure_state_name(
                    admission_decision.snapshot.pressure_state
                ),
                admission_action = turn_admission_action_name(admission_decision.snapshot.action),
                admission_requires_compaction = admission_decision.requires_compaction,
                admission_reasons = ?admission_decision.reasons,
                effective_lane = effective_lane
                    .map(|lane| format!("{lane:?}"))
                    .unwrap_or_else(|| "none".to_string()),
                effective_candidate_index = effective_candidate_index
                    .map(|index| index.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                compression_threshold_ratio = %effective_compression.threshold,
                compression_target_ratio = %effective_compression.target_ratio,
                compression_cache_ttl_secs = effective_compression.cache_ttl_secs,
                compression_cache_max_entries = effective_compression.cache_max_entries,
                native_prompt_caching_supported =
                    native_context_policy.prompt_caching_supported,
                native_prompt_caching_enabled = native_context_policy.prompt_caching_enabled,
                native_server_continuation_supported =
                    native_context_policy.server_continuation_supported,
                tool_specs = turn_tool_specs.len(),
                tool_spec_names = turn_tool_specs
                    .iter()
                    .take(12)
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                "Built provider-facing context snapshot"
            );

            if admission_decision.requires_compaction
                && self
                    .maybe_compact_history_with_route(
                        &effective_compression,
                        &effective_model_profile,
                    )
                    .await
            {
                tracing::info!(
                    target: "agent.turn_admission",
                    action = "compact_and_retry",
                    "Admission policy requested pre-provider compaction"
                );
                continue;
            }

            if admission_decision.snapshot.action == TurnAdmissionAction::Block {
                let handoff_packet =
                    synapse_domain::application::services::session_handoff::build_session_handoff_packet(
                        synapse_domain::application::services::session_handoff::SessionHandoffInput {
                            user_message,
                            interpretation: turn_interpretation.as_ref(),
                            recent_admission_repair: admission_decision.recommended_action,
                            recent_admission_reasons: &admission_decision.reasons,
                            recalled_entries: &turn_ctx.recalled_entries,
                            session_matches: &turn_ctx.session_matches,
                            run_recipes: &turn_ctx.run_recipes,
                        },
                    )
                    .map(|packet| {
                        synapse_domain::application::services::session_handoff::format_session_handoff_packet(
                            &packet,
                        )
                    })
                    .unwrap_or_default();
                anyhow::bail!(
                    "turn admission blocked provider call intent={} pressure={} provider={} model={}\n{}",
                    turn_intent_name(admission_decision.snapshot.intent),
                    context_pressure_state_name(admission_decision.snapshot.pressure_state),
                    self.provider_name,
                    effective_model,
                    handoff_packet
                );
            }
            let mut messages = snapshot.messages;

            // Inject enrichment prefix on the last user message for the provider
            // call — not persisted in history.
            if !ephemeral_prefix.is_empty() {
                if let Some(last_user) = messages.iter_mut().rfind(|m| m.role == "user") {
                    if last_user.content == user_message {
                        last_user.content = format!("{ephemeral_prefix}{}", last_user.content);
                    }
                }
            }
            let mut request_context = snapshot.stats.clone();
            request_context.total_messages = messages.len();
            request_context.total_chars = total_message_chars(&messages);
            self.observer.record_event(&ObserverEvent::LlmRequest {
                provider: self.provider_name.clone(),
                model: effective_model.clone(),
                messages_count: messages.len(),
                context: Some(request_context),
            });

            // Response cache: check before LLM call (only for deterministic, text-only prompts)
            let cache_key = if self.temperature == 0.0 {
                self.response_cache.as_ref().map(|_| {
                    let last_user = messages
                        .iter()
                        .rfind(|m| m.role == "user")
                        .map(|m| m.content.as_str())
                        .unwrap_or("");
                    // Include ALL system messages (static prompt + dynamic core blocks)
                    let system_parts: Vec<&str> = messages
                        .iter()
                        .filter(|m| m.role == "system")
                        .map(|m| m.content.as_str())
                        .collect();
                    let system_concat = system_parts.join("\n---\n");
                    let system = if system_concat.is_empty() {
                        None
                    } else {
                        Some(system_concat.as_str())
                    };
                    synapse_memory::response_cache::ResponseCache::cache_key(
                        &effective_model,
                        system,
                        last_user,
                    )
                })
            } else {
                None
            };

            if let (Some(ref cache), Some(ref key)) = (&self.response_cache, &cache_key) {
                if let Ok(Some(cached)) = cache.get(key).await {
                    self.observer.record_event(&ObserverEvent::CacheHit {
                        cache_type: "response".into(),
                        tokens_saved: 0,
                    });
                    self.history
                        .push(ConversationMessage::Chat(ChatMessage::assistant(
                            cached.clone(),
                        )));
                    let compacted = self
                        .maybe_compact_history_with_route(
                            &effective_compression,
                            &effective_model_profile,
                        )
                        .await;
                    if compacted {
                        tracing::debug!(
                            "Live agent history auto-compaction complete after cache hit"
                        );
                    }
                    self.trim_history();
                    return Ok(cached);
                }
                self.observer.record_event(&ObserverEvent::CacheMiss {
                    cache_type: "response".into(),
                });
            }

            let image_marker_count = synapse_providers::multimodal::count_image_markers(&messages);
            for issue in assess_provider_call_capabilities(ProviderCallCapabilityInput {
                image_marker_count,
                provider_capabilities: &provider_capabilities,
                route_profile: &effective_model_profile,
            })
            .issues
            {
                match issue {
                    ProviderCallCapabilityIssue::MissingVisionInput { image_marker_count } => {
                        self.recent_runtime_assumptions = challenge_runtime_assumption_ledger(
                            &self.recent_runtime_assumptions,
                            RuntimeAssumptionChallenge {
                                kind: RuntimeAssumptionKind::RouteCapability,
                                value: "missing_vision_input",
                                invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
                                replacement_path: RuntimeAssumptionReplacementPath::SwitchRoute,
                            },
                        );
                        return Err(ProviderCapabilityError {
                            provider: self.provider_name.clone(),
                            capability: "vision".to_string(),
                            message: format!(
                                "received {image_marker_count} image marker(s), but this route does not support vision input"
                            ),
                        }
                        .into());
                    }
                }
            }

            let response = match self
                .provider
                .chat(
                    ChatRequest {
                        messages: &messages,
                        tools: if self.tool_dispatcher.should_send_tool_specs() {
                            Some(turn_tool_specs.as_slice())
                        } else {
                            None
                        },
                    },
                    &effective_model,
                    self.temperature,
                )
                .await
            {
                Ok(resp) => {
                    self.record_route_calibration(
                        &admission_decision,
                        RuntimeCalibrationOutcome::Succeeded,
                        chrono::Utc::now().timestamp(),
                        &effective_model,
                    );
                    resp
                }
                Err(err) => {
                    if let Some(observation) = classify_context_limit_error(&err) {
                        self.recent_runtime_assumptions = challenge_runtime_assumption_ledger(
                            &self.recent_runtime_assumptions,
                            RuntimeAssumptionChallenge {
                                kind: RuntimeAssumptionKind::ContextWindow,
                                value: "context_limit_exceeded",
                                invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
                                replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
                            },
                        );
                        if observation.observed_context_window_tokens.is_some() {
                            let profile_catalog =
                                crate::runtime_routes::WorkspaceModelProfileCatalog::with_provider_endpoint(
                                    self.workspace_dir.clone(),
                                    Some(&self.provider_name),
                                    self.provider_api_url.as_deref(),
                                );
                            if let Err(record_error) = profile_catalog
                                .record_context_limit_observation(
                                    &self.provider_name,
                                    &effective_model,
                                    observation,
                                )
                            {
                                tracing::debug!(
                                    provider = self.provider_name,
                                    model = effective_model,
                                    error = %record_error,
                                    "Failed to record context-limit model profile observation"
                                );
                            }
                        }
                    }
                    self.record_route_calibration(
                        &admission_decision,
                        RuntimeCalibrationOutcome::Failed,
                        chrono::Utc::now().timestamp(),
                        &effective_model,
                    );
                    return Err(err);
                }
            };

            // Accumulate token usage from provider response
            if let Some(ref u) = response.usage {
                let prev =
                    self.last_turn_usage
                        .get_or_insert(synapse_providers::traits::TokenUsage {
                            input_tokens: None,
                            output_tokens: None,
                            cached_input_tokens: None,
                        });
                prev.input_tokens =
                    Some(prev.input_tokens.unwrap_or(0) + u.input_tokens.unwrap_or(0));
                prev.output_tokens =
                    Some(prev.output_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0));
            }

            let (text, parsed_calls) = self.tool_dispatcher.parse_response(&response);
            let calls = self
                .deduplicate_turn_calls(parsed_calls)
                .into_iter()
                .map(|call| {
                    let force_implicit_target = synapse_domain::application::services::turn_tool_narrowing::should_force_implicit_target_for_tool(
                        &call.name,
                        &turn_tool_specs,
                        turn_ctx.execution_guidance.as_ref(),
                        &resolved_turn_defaults,
                    );
                    let drop_noncanonical_string_target = resolved_turn_defaults.delivery_target.is_some()
                        && has_noncanonical_string_delivery_target(&call)
                        && turn_tool_specs.iter().any(|spec| {
                            spec.name == call.name
                                && spec.runtime_role
                                    == Some(synapse_domain::ports::tool::ToolRuntimeRole::DirectDelivery)
                        });

                    if force_implicit_target || drop_noncanonical_string_target {
                        strip_redundant_delivery_target(&call)
                    } else {
                        call
                    }
                })
                .collect::<Vec<_>>();
            if calls.is_empty() {
                let final_text = if text.is_empty() {
                    response.text.unwrap_or_default()
                } else {
                    text
                };

                // Store in response cache (text-only, no tool calls)
                if let (Some(ref cache), Some(ref key)) = (&self.response_cache, &cache_key) {
                    let token_count = response
                        .usage
                        .as_ref()
                        .and_then(|u| u.output_tokens)
                        .unwrap_or(0);
                    #[allow(clippy::cast_possible_truncation)]
                    let _ = cache
                        .put(key, &effective_model, &final_text, token_count as u32)
                        .await;
                }

                self.history
                    .push(ConversationMessage::Chat(ChatMessage::assistant(
                        final_text.clone(),
                    )));
                let compacted = self
                    .maybe_compact_history_with_route(
                        &effective_compression,
                        &effective_model_profile,
                    )
                    .await;
                if compacted {
                    tracing::debug!("Live agent history auto-compaction complete");
                }
                self.trim_history();

                if let (Some(session_id), Some(store)) = (
                    self.memory_session_id.as_deref(),
                    self.dialogue_state_store.as_ref(),
                ) {
                    let existing = store.get(session_id);
                    if dialogue_state_service::should_materialize_state(
                        existing.as_ref(),
                        &tool_facts_this_turn,
                    ) {
                        let mut state = existing.unwrap_or_default();
                        dialogue_state_service::update_state_from_turn(
                            &mut state,
                            user_message,
                            &tool_facts_this_turn,
                            &final_text,
                        );
                        store.set(session_id, state);
                    }
                }

                self.last_turn_tool_facts = tool_facts_this_turn;
                self.last_turn_tool_repair = last_tool_repair_this_turn;
                self.recent_turn_tool_repairs =
                    synapse_domain::application::services::tool_repair::append_tool_repair_traces(
                        &self.recent_turn_tool_repairs,
                        &tool_repairs_this_turn,
                        chrono::Utc::now().timestamp(),
                    );
                self.recent_runtime_assumptions = apply_tool_repair_assumption_challenges(
                    &self.recent_runtime_assumptions,
                    &tool_repairs_this_turn,
                );
                self.run_runtime_trace_janitor(chrono::Utc::now().timestamp());

                return Ok(final_text);
            }

            if !text.is_empty() {
                self.history
                    .push(ConversationMessage::Chat(ChatMessage::assistant(
                        text.clone(),
                    )));
                print!("{text}");
                let _ = std::io::stdout().flush();
            }

            self.history.push(ConversationMessage::AssistantToolCalls {
                text: response.text.clone(),
                tool_calls: response.tool_calls.clone(),
                reasoning_content: response.reasoning_content.clone(),
            });

            let results = self
                .execute_tools_with_cache(&calls, &mut executed_call_cache)
                .await;
            self.record_tool_calibrations(&results, chrono::Utc::now().timestamp());
            tool_facts_this_turn.extend(
                results
                    .iter()
                    .flat_map(|result| result.tool_facts.iter().cloned()),
            );
            if let Some(trace) = results
                .iter()
                .filter_map(|result| result.repair_trace.clone())
                .last()
            {
                last_tool_repair_this_turn = Some(trace);
            }
            tool_repairs_this_turn.extend(
                results
                    .iter()
                    .filter_map(|result| result.repair_trace.clone()),
            );
            let formatted = self.tool_dispatcher.format_results(&results);
            self.history.push(formatted);
            self.trim_history();
        }

        anyhow::bail!(
            "Agent exceeded maximum tool iterations ({})",
            self.config.max_tool_iterations
        )
    }

    pub async fn run_single(&mut self, message: &str) -> Result<String> {
        self.turn(message).await
    }

    pub async fn run_interactive(&mut self) -> Result<()> {
        println!("🦀 SynapseClaw Interactive Mode");
        println!("Type /quit to exit.\n");

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let cli = crate::channels::CliChannel::new();

        let listen_handle = tokio::spawn(async move {
            let _ = crate::channels::Channel::listen(&cli, tx).await;
        });

        while let Some(msg) = rx.recv().await {
            let response = match self.turn(&msg.content).await {
                Ok(resp) => resp,
                Err(e) => {
                    eprintln!("\nError: {e}\n");
                    continue;
                }
            };
            println!("\n{response}\n");
        }

        listen_handle.abort();
        Ok(())
    }
}

pub async fn run(
    config: Config,
    message: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
    temperature: f64,
) -> Result<()> {
    run_with_memory(
        config,
        message,
        provider_override,
        model_override,
        temperature,
        None,
    )
    .await
}

/// Run agent with optional shared memory (avoids SurrealKV lock conflicts in daemon mode).
pub async fn run_with_memory(
    config: Config,
    message: Option<String>,
    provider_override: Option<String>,
    model_override: Option<String>,
    temperature: f64,
    shared_memory: Option<Arc<dyn UnifiedMemoryPort>>,
) -> Result<()> {
    let start = Instant::now();

    let mut effective_config = config;
    apply_cli_agent_overrides(&mut effective_config, provider_override, model_override);
    effective_config.default_temperature = temperature;

    let mut agent = Agent::from_config_with_memory(&effective_config, shared_memory).await?;

    let provider_name = effective_config
        .default_provider
        .as_deref()
        .unwrap_or("openrouter")
        .to_string();
    let model_name = effective_config
        .default_model
        .as_deref()
        .unwrap_or_else(|| {
            synapse_domain::config::model_catalog::provider_default_model(provider_name.as_str())
                .unwrap_or("default")
        })
        .to_string();

    agent.observer.record_event(&ObserverEvent::AgentStart {
        provider: provider_name.clone(),
        model: model_name.clone(),
    });

    if let Some(msg) = message {
        let response = agent.run_single(&msg).await?;
        println!("{response}");
    } else {
        agent.run_interactive().await?;
    }

    agent.observer.record_event(&ObserverEvent::AgentEnd {
        provider: provider_name,
        model: model_name,
        duration: start.elapsed(),
        tokens_used: None,
        cost_usd: None,
    });

    Ok(())
}

fn apply_cli_agent_overrides(
    config: &mut Config,
    provider_override: Option<String>,
    model_override: Option<String>,
) {
    if let Some(provider) = provider_override {
        if config.default_provider.as_deref() != Some(provider.as_str()) {
            config.api_key = None;
            config.api_url = None;
        }
        config.default_provider = Some(provider);
    }
    if let Some(model) = model_override {
        config.default_model = Some(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use synapse_domain::config::schema::{
        AgentConfig, CapabilityLane, ContextCompressionConfig,
        ContextCompressionRouteOverrideConfig, ModelCandidateProfileConfig, ModelFeature,
        ModelLaneCandidateConfig, ModelLaneConfig,
    };

    #[test]
    fn cli_provider_override_drops_cross_provider_global_credentials() {
        let mut config = Config::default();
        config.default_provider = Some("openai-codex".into());
        config.default_model = Some("gpt-5.4".into());
        config.api_key = Some("fake-openai-like-key".into());
        config.api_url = Some("https://api.openai.com/v1".into());

        apply_cli_agent_overrides(
            &mut config,
            Some("openrouter".into()),
            Some("x-ai/grok-4.20".into()),
        );

        assert_eq!(config.default_provider.as_deref(), Some("openrouter"));
        assert_eq!(config.default_model.as_deref(), Some("x-ai/grok-4.20"));
        assert_eq!(config.api_key, None);
        assert_eq!(config.api_url, None);
    }

    #[test]
    fn cli_provider_override_keeps_credentials_for_same_provider() {
        let mut config = Config::default();
        config.default_provider = Some("openrouter".into());
        config.api_key = Some("fake-openrouter-route-key".into());

        apply_cli_agent_overrides(&mut config, Some("openrouter".into()), None);

        assert_eq!(config.default_provider.as_deref(), Some("openrouter"));
        assert_eq!(config.api_key.as_deref(), Some("fake-openrouter-route-key"));
    }

    struct MockProvider {
        responses: Mutex<Vec<synapse_providers::ChatResponse>>,
    }

    #[async_trait]
    impl Provider for MockProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> Result<String> {
            Ok("ok".into())
        }

        async fn chat(
            &self,
            _request: ChatRequest<'_>,
            _model: &str,
            _temperature: f64,
        ) -> Result<synapse_providers::ChatResponse> {
            let mut guard = self.responses.lock();
            if guard.is_empty() {
                return Ok(synapse_providers::ChatResponse {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    usage: None,
                    reasoning_content: None,
                });
            }
            Ok(guard.remove(0))
        }
    }

    struct ModelCaptureProvider {
        responses: Mutex<Vec<synapse_providers::ChatResponse>>,
        seen_models: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Provider for ModelCaptureProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> Result<String> {
            Ok("ok".into())
        }

        async fn chat(
            &self,
            _request: ChatRequest<'_>,
            model: &str,
            _temperature: f64,
        ) -> Result<synapse_providers::ChatResponse> {
            self.seen_models.lock().push(model.to_string());
            let mut guard = self.responses.lock();
            if guard.is_empty() {
                return Ok(synapse_providers::ChatResponse {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    usage: None,
                    reasoning_content: None,
                });
            }
            Ok(guard.remove(0))
        }
    }

    struct CountingSummaryGenerator {
        calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl SummaryGeneratorPort for CountingSummaryGenerator {
        async fn generate_summary(&self, _prompt: &str) -> Result<String> {
            let mut calls = self.calls.lock();
            *calls += 1;
            Ok(format!("- cached summary call {}", *calls))
        }
    }

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "echo"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _args: serde_json::Value) -> Result<crate::tools::ToolResult> {
            Ok(crate::tools::ToolResult {
                success: true,
                output: "tool-out".into(),
                error: None,
            })
        }
    }

    #[tokio::test]
    async fn turn_without_tools_returns_text() {
        let provider = Box::new(MockProvider {
            responses: Mutex::new(vec![synapse_providers::ChatResponse {
                text: Some("hello".into()),
                tool_calls: vec![],
                usage: None,
                reasoning_content: None,
            }]),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let mut agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(XmlToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .build()
            .expect("agent builder should succeed with valid config");

        let response = agent.turn("hi").await.unwrap();
        assert_eq!(response, "hello");
    }

    #[tokio::test]
    async fn turn_allows_image_input_when_route_profile_supports_vision() {
        let provider = Box::new(MockProvider {
            responses: Mutex::new(vec![synapse_providers::ChatResponse {
                text: Some("vision-route".into()),
                tool_calls: vec![],
                usage: None,
                reasoning_content: None,
            }]),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);
        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let mut agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(XmlToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .current_model_profile(ResolvedModelProfile {
                features: vec![ModelFeature::Vision],
                features_source:
                    synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            })
            .build()
            .expect("agent builder should succeed with valid config");

        let response = agent
            .turn("inspect [IMAGE:data:image/png;base64,iVBORw0KGgo=]")
            .await
            .expect("route profile vision support should allow image input");

        assert_eq!(response, "vision-route");
    }

    #[tokio::test]
    async fn turn_with_native_dispatcher_handles_tool_results_variant() {
        let provider = Box::new(MockProvider {
            responses: Mutex::new(vec![
                synapse_providers::ChatResponse {
                    text: Some(String::new()),
                    tool_calls: vec![synapse_providers::ToolCall {
                        id: "tc1".into(),
                        name: "echo".into(),
                        arguments: "{}".into(),
                    }],
                    usage: None,
                    reasoning_content: None,
                },
                synapse_providers::ChatResponse {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    usage: None,
                    reasoning_content: None,
                },
            ]),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let mut agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .build()
            .expect("agent builder should succeed with valid config");

        let response = agent.turn("hi").await.unwrap();
        assert_eq!(response, "done");
        assert!(agent
            .history()
            .iter()
            .any(|msg| matches!(msg, ConversationMessage::ToolResults(_))));
    }

    #[tokio::test]
    async fn history_compaction_reuses_cached_summary_for_same_transcript_digest() {
        fn long_history() -> Vec<ConversationMessage> {
            let mut history = vec![ConversationMessage::Chat(ChatMessage::system("bootstrap"))];
            for idx in 0..12 {
                history.push(ConversationMessage::Chat(ChatMessage::user(format!(
                    "user fact {idx}"
                ))));
                history.push(ConversationMessage::Chat(ChatMessage::assistant(format!(
                    "assistant response {idx}"
                ))));
            }
            history
        }

        let tmp = tempfile::TempDir::new().expect("temp dir");
        let cache_path = tmp
            .path()
            .join("state")
            .join("history_compaction_cache")
            .join("persistent-cache.json");
        let calls = Arc::new(Mutex::new(0));
        let summary_generator: Arc<dyn SummaryGeneratorPort> = Arc::new(CountingSummaryGenerator {
            calls: calls.clone(),
        });

        let build_agent = |summary_generator: Arc<dyn SummaryGeneratorPort>,
                           cache: Arc<dyn HistoryCompactionCachePort>| {
            let provider = Box::new(MockProvider {
                responses: Mutex::new(Vec::new()),
            });
            let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);
            let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
            Agent::builder()
                .provider(provider)
                .tools(vec![Box::new(MockTool)])
                .memory(mem)
                .observer(observer)
                .tool_dispatcher(Box::new(NativeToolDispatcher))
                .workspace_dir(tmp.path().to_path_buf())
                .agent_id("cache-test-agent".to_string())
                .config(AgentConfig {
                    max_history_messages: 4,
                    max_context_tokens: usize::MAX,
                    ..Default::default()
                })
                .history_summary_generator(Some(summary_generator))
                .history_compaction_cache(Some(cache))
                .build()
                .expect("agent builder should succeed with valid config")
        };

        let first_cache: Arc<dyn HistoryCompactionCachePort> = Arc::new(
            crate::runtime::history_compaction_cache::FileHistoryCompactionCache::new(
                cache_path.clone(),
            ),
        );
        let mut agent = build_agent(summary_generator.clone(), first_cache);

        agent.history = long_history();
        assert!(agent.maybe_compact_history().await);
        assert_eq!(*calls.lock(), 1);

        agent.history = long_history();
        assert!(agent.maybe_compact_history().await);
        assert_eq!(*calls.lock(), 1);
        assert_eq!(agent.history_compaction_cache_stats().entries, 1);

        let restarted_cache: Arc<dyn HistoryCompactionCachePort> = Arc::new(
            crate::runtime::history_compaction_cache::FileHistoryCompactionCache::new(cache_path),
        );
        let mut restarted_agent = build_agent(summary_generator, restarted_cache);
        restarted_agent.history = long_history();
        assert!(restarted_agent.maybe_compact_history().await);
        assert_eq!(
            *calls.lock(),
            1,
            "restarted agent should reuse persistent cache"
        );
    }

    #[test]
    fn model_profile_threshold_scales_history_compaction_trigger() {
        let agent = Agent::builder()
            .provider(Box::new(MockProvider {
                responses: Mutex::new(Vec::new()),
            }))
            .tools(vec![Box::new(MockTool)])
            .memory(Arc::new(synapse_memory::NoopUnifiedMemory))
            .observer(Arc::from(synapse_observability::NoopObserver {}))
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .config(AgentConfig {
                max_history_messages: 4,
                max_context_tokens: 32,
                ..Default::default()
            })
            .current_model_profile(ResolvedModelProfile {
                context_window_tokens: Some(200_000),
                context_window_source:
                    synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig,
                max_output_tokens: None,
                ..Default::default()
            })
            .build()
            .expect("agent builder should succeed with valid config");

        let policy = agent.history_compression_policy();
        assert_eq!(agent.history_compaction_threshold_tokens(&policy), 87_500);
    }

    #[test]
    fn route_compression_overrides_scale_thresholds_for_different_context_windows() {
        let source =
            synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource::ManualConfig;
        let agent = Agent::builder()
            .provider(Box::new(MockProvider {
                responses: Mutex::new(Vec::new()),
            }))
            .tools(vec![Box::new(MockTool)])
            .memory(Arc::new(synapse_memory::NoopUnifiedMemory))
            .observer(Arc::from(synapse_observability::NoopObserver {}))
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .compression(ContextCompressionConfig {
                threshold: 0.50,
                ..Default::default()
            })
            .compression_overrides(vec![
                ContextCompressionRouteOverrideConfig {
                    provider: Some("deepseek".into()),
                    lane: Some(CapabilityLane::CheapReasoning),
                    threshold: Some(0.25),
                    ..Default::default()
                },
                ContextCompressionRouteOverrideConfig {
                    provider: Some("xai".into()),
                    threshold: Some(0.60),
                    ..Default::default()
                },
            ])
            .build()
            .expect("agent builder should succeed with valid config");

        let deepseek_policy = HistoryCompressionPolicy::from(&agent.history_compression_for_route(
            "deepseek",
            "deepseek-chat",
            Some(CapabilityLane::CheapReasoning),
            None,
        ));
        let deepseek_profile = ResolvedModelProfile {
            context_window_tokens: Some(128_000),
            context_window_source: source,
            max_output_tokens: Some(8_000),
            max_output_source: source,
            ..Default::default()
        };
        assert_eq!(
            agent.history_compaction_threshold_tokens_for_profile(
                &deepseek_policy,
                &deepseek_profile
            ),
            30_000
        );

        let grok_policy = HistoryCompressionPolicy::from(&agent.history_compression_for_route(
            "xai",
            "grok-4.20",
            Some(CapabilityLane::Reasoning),
            None,
        ));
        let grok_profile = ResolvedModelProfile {
            context_window_tokens: Some(2_000_000),
            context_window_source: source,
            max_output_tokens: Some(128_000),
            max_output_source: source,
            ..Default::default()
        };
        assert_eq!(
            agent.history_compaction_threshold_tokens_for_profile(&grok_policy, &grok_profile),
            1_123_200
        );
    }

    #[tokio::test]
    async fn turn_routes_with_hint_when_query_classification_matches() {
        let seen_models = Arc::new(Mutex::new(Vec::new()));
        let provider = Box::new(ModelCaptureProvider {
            responses: Mutex::new(vec![synapse_providers::ChatResponse {
                text: Some("classified".into()),
                tool_calls: vec![],
                usage: None,
                reasoning_content: None,
            }]),
            seen_models: seen_models.clone(),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let mut route_model_by_hint = HashMap::new();
        route_model_by_hint.insert("fast".to_string(), "anthropic/claude-haiku-4-5".to_string());
        let mut agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .classification_config(synapse_domain::config::schema::QueryClassificationConfig {
                enabled: true,
                rules: vec![synapse_domain::config::schema::ClassificationRule {
                    hint: "fast".to_string(),
                    keywords: vec!["quick".to_string()],
                    patterns: vec![],
                    min_length: None,
                    max_length: None,
                    priority: 10,
                }],
            })
            .available_hints(vec!["fast".to_string()])
            .route_model_by_hint(route_model_by_hint)
            .build()
            .expect("agent builder should succeed with valid config");

        let response = agent.turn("quick summary please").await.unwrap();
        assert_eq!(response, "classified");
        let seen = seen_models.lock();
        assert_eq!(seen.as_slice(), &["hint:fast".to_string()]);
    }

    #[tokio::test]
    async fn turn_reroutes_same_provider_specialized_lane() {
        let seen_models = Arc::new(Mutex::new(Vec::new()));
        let provider = Box::new(ModelCaptureProvider {
            responses: Mutex::new(vec![synapse_providers::ChatResponse {
                text: Some("image-ready".into()),
                tool_calls: vec![],
                usage: None,
                reasoning_content: None,
            }]),
            seen_models: seen_models.clone(),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);
        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let mut agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .provider_name("openrouter".to_string())
            .model_name("plain-model".to_string())
            .route_model_lanes(vec![ModelLaneConfig {
                lane: CapabilityLane::ImageGeneration,
                candidates: vec![ModelLaneCandidateConfig {
                    provider: "openrouter".into(),
                    model: "universal-image-model".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig {
                        context_window_tokens: None,
                        max_output_tokens: None,
                        features: vec![ModelFeature::ImageGeneration],
                    },
                }],
            }])
            .build()
            .expect("agent builder should succeed with valid config");

        let response = agent.turn("[GENERATE:IMAGE] poster concept").await.unwrap();

        assert_eq!(response, "image-ready");
        let seen = seen_models.lock();
        assert_eq!(seen.as_slice(), &["universal-image-model".to_string()]);
    }

    #[tokio::test]
    async fn from_config_passes_extra_headers_to_custom_provider() {
        use axum::{http::HeaderMap, routing::post, Json, Router};
        use tempfile::TempDir;
        use tokio::net::TcpListener;

        let captured_headers: Arc<std::sync::Mutex<Option<HashMap<String, String>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let captured_headers_clone = captured_headers.clone();

        let app = Router::new().route(
            "/chat/completions",
            post(
                move |headers: HeaderMap, Json(_body): Json<serde_json::Value>| {
                    let captured_headers = captured_headers_clone.clone();
                    async move {
                        let collected = headers
                            .iter()
                            .filter_map(|(name, value)| {
                                value
                                    .to_str()
                                    .ok()
                                    .map(|value| (name.as_str().to_string(), value.to_string()))
                            })
                            .collect();
                        *captured_headers.lock().unwrap() = Some(collected);
                        Json(serde_json::json!({
                            "choices": [{
                                "message": {
                                    "content": "hello from mock"
                                }
                            }]
                        }))
                    }
                },
            ),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let tmp = TempDir::new().expect("temp dir");
        let workspace_dir = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = workspace_dir;
        config.config_path = tmp.path().join("config.toml");
        config.api_key = Some("test-key".to_string());
        config.default_provider = Some(format!("custom:http://{addr}"));
        config.default_model = Some("test-model".to_string());
        config.memory.backend = "none".to_string();
        config.memory.auto_save = false;
        config.extra_headers.insert(
            "User-Agent".to_string(),
            "synapseclaw-web-test/1.0".to_string(),
        );
        config
            .extra_headers
            .insert("X-Title".to_string(), "synapseclaw-web".to_string());

        let mut agent = Agent::from_config(&config)
            .await
            .expect("agent from config");
        let response = agent.turn("hello").await.expect("agent turn");

        assert_eq!(response, "hello from mock");

        let headers = captured_headers
            .lock()
            .unwrap()
            .clone()
            .expect("captured headers");
        assert_eq!(
            headers.get("user-agent").map(String::as_str),
            Some("synapseclaw-web-test/1.0")
        );
        assert_eq!(
            headers.get("x-title").map(String::as_str),
            Some("synapseclaw-web")
        );

        server_handle.abort();
    }

    #[test]
    fn builder_allowed_tools_none_keeps_all_tools() {
        let provider = Box::new(MockProvider {
            responses: Mutex::new(vec![]),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .allowed_tools(None)
            .build()
            .expect("agent builder should succeed with valid config");

        assert_eq!(agent.tool_specs.len(), 1);
        assert_eq!(agent.tool_specs[0].name, "echo");
    }

    #[test]
    fn builder_allowed_tools_some_filters_tools() {
        let provider = Box::new(MockProvider {
            responses: Mutex::new(vec![]),
        });

        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let observer: Arc<dyn Observer> = Arc::from(synapse_observability::NoopObserver {});
        let agent = Agent::builder()
            .provider(provider)
            .tools(vec![Box::new(MockTool)])
            .memory(mem)
            .observer(observer)
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .workspace_dir(std::path::PathBuf::from("/tmp"))
            .allowed_tools(Some(vec!["nonexistent".to_string()]))
            .build()
            .expect("agent builder should succeed with valid config");

        assert!(
            agent.tool_specs.is_empty(),
            "No tools should match a non-existent allowlist entry"
        );
    }

    #[test]
    fn deduplicate_turn_calls_skips_identical_name_and_args() {
        let agent = AgentBuilder::new()
            .provider(Box::new(MockProvider {
                responses: Mutex::new(Vec::new()),
            }))
            .tools(Vec::new())
            .memory(Arc::new(synapse_memory::NoopUnifiedMemory))
            .observer(Arc::new(synapse_observability::NoopObserver))
            .prompt_builder(SystemPromptBuilder::with_defaults())
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .config(synapse_domain::config::schema::AgentConfig::default())
            .model_name("test".into())
            .temperature(0.0)
            .workspace_dir(std::env::temp_dir())
            .identity_config(synapse_domain::config::schema::IdentityConfig::default())
            .skills(Vec::new())
            .agent_id("default".into())
            .build()
            .expect("agent should build");

        let calls = vec![
            ParsedToolCall {
                name: "shell".into(),
                arguments: serde_json::json!({"command": "date"}),
                tool_call_id: Some("call-1".into()),
            },
            ParsedToolCall {
                name: "shell".into(),
                arguments: serde_json::json!({"command": "date"}),
                tool_call_id: Some("call-2".into()),
            },
            ParsedToolCall {
                name: "shell".into(),
                arguments: serde_json::json!({"command": "uptime"}),
                tool_call_id: Some("call-3".into()),
            },
        ];

        let deduped = agent.deduplicate_turn_calls(calls);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(deduped[1].tool_call_id.as_deref(), Some("call-3"));
    }

    #[test]
    fn deduplicate_turn_calls_normalizes_argument_key_order() {
        let agent = AgentBuilder::new()
            .provider(Box::new(MockProvider {
                responses: Mutex::new(Vec::new()),
            }))
            .tools(Vec::new())
            .memory(Arc::new(synapse_memory::NoopUnifiedMemory))
            .observer(Arc::new(synapse_observability::NoopObserver))
            .prompt_builder(SystemPromptBuilder::with_defaults())
            .tool_dispatcher(Box::new(NativeToolDispatcher))
            .config(synapse_domain::config::schema::AgentConfig::default())
            .model_name("test".into())
            .temperature(0.0)
            .workspace_dir(std::env::temp_dir())
            .identity_config(synapse_domain::config::schema::IdentityConfig::default())
            .skills(Vec::new())
            .agent_id("default".into())
            .build()
            .expect("agent should build");

        let calls = vec![
            ParsedToolCall {
                name: "http_request".into(),
                arguments: serde_json::json!({"method": "GET", "url": "https://example.com"}),
                tool_call_id: Some("call-a".into()),
            },
            ParsedToolCall {
                name: "http_request".into(),
                arguments: serde_json::json!({"url": "https://example.com", "method": "GET"}),
                tool_call_id: Some("call-b".into()),
            },
        ];

        let deduped = agent.deduplicate_turn_calls(calls);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].tool_call_id.as_deref(), Some("call-a"));
    }
}
