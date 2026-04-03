use crate::agent::dispatcher::{
    NativeToolDispatcher, ParsedToolCall, ToolDispatcher, ToolExecutionResult, XmlToolDispatcher,
};
use crate::agent::prompt::{PromptContext, SystemPromptBuilder};
use crate::agent::turn_context_fmt;
use synapse_domain::application::services::turn_context::{
    self as tc, ContinuationPolicy, PromptBudget,
};
use crate::runtime;
use crate::tools::{self, Tool, ToolSpec};
use anyhow::Result;
use std::collections::HashMap;
use std::io::Write as IoWrite;
use std::sync::Arc;
use std::time::Instant;
use synapse_domain::config::schema::Config;
use synapse_memory::{self, MemoryCategory, UnifiedMemoryPort};
use synapse_observability::{self, Observer, ObserverEvent};
use synapse_providers::{self, ChatMessage, ChatRequest, ConversationMessage, Provider};
use synapse_security::security_policy_from_config;

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
    model_name: String,
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
    /// Cumulative token usage from the last turn (provider-reported).
    last_turn_usage: Option<synapse_providers::traits::TokenUsage>,
    allowed_tools: Option<Vec<String>>,
    response_cache: Option<Arc<synapse_memory::response_cache::ResponseCache>>,
    /// Canonical agent ID for memory scoping.
    agent_id: String,
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
    allowed_tools: Option<Vec<String>>,
    response_cache: Option<Arc<synapse_memory::response_cache::ResponseCache>>,
    agent_id: Option<String>,
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
            allowed_tools: None,
            response_cache: None,
            agent_id: None,
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

    pub fn agent_id(mut self, id: String) -> Self {
        self.agent_id = Some(id);
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
            config: self.config.unwrap_or_default(),
            model_name: self
                .model_name
                .unwrap_or_else(|| "anthropic/claude-sonnet-4-20250514".into()),
            temperature: self.temperature.unwrap_or(0.7),
            workspace_dir: self
                .workspace_dir
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
            identity_config: self.identity_config.unwrap_or_default(),
            skills: self.skills.unwrap_or_default(),
            skills_prompt_mode: self.skills_prompt_mode.unwrap_or_default(),
            auto_save: self.auto_save.unwrap_or(false),
            memory_session_id: self.memory_session_id,
            history: Vec::new(),
            classification_config: self.classification_config.unwrap_or_default(),
            available_hints: self.available_hints.unwrap_or_default(),
            route_model_by_hint: self.route_model_by_hint.unwrap_or_default(),
            last_turn_usage: None,
            allowed_tools: allowed,
            response_cache: self.response_cache,
            agent_id: self.agent_id.unwrap_or_else(|| "default".to_string()),
        })
    }
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
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

    /// Replace the observer (e.g. to wrap with per-request event forwarding).
    pub fn set_observer(&mut self, observer: Arc<dyn synapse_observability::Observer>) {
        self.observer = observer;
    }

    /// Token usage reported by the provider during the last turn (if any).
    pub fn last_turn_usage(&self) -> Option<&synapse_providers::traits::TokenUsage> {
        self.last_turn_usage.as_ref()
    }

    pub fn set_memory_session_id(&mut self, session_id: Option<String>) {
        self.memory_session_id = session_id;
    }

    pub async fn from_config(config: &Config) -> Result<Self> {
        Self::from_config_with_memory(config, None).await
    }

    /// Create an agent from config, optionally reusing a shared memory backend.
    /// In daemon mode, `shared_memory` avoids opening a second SurrealKV lock.
    pub async fn from_config_with_memory(
        config: &Config,
        shared_memory: Option<Arc<dyn UnifiedMemoryPort>>,
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

        let resolved_agent_id = crate::agent::loop_::resolve_agent_id(config);
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
        );

        let provider_name = config.default_provider.as_deref().unwrap_or("openrouter");

        let model_name = config
            .default_model
            .as_deref()
            .unwrap_or("anthropic/claude-sonnet-4-20250514")
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
                    provider_for_consolidation,
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

        Agent::builder()
            .provider(provider)
            .tools(tools)
            .memory(memory)
            .observer(observer)
            .response_cache(response_cache)
            .tool_dispatcher(tool_dispatcher)
            .prompt_budget({
                let mut b = config.memory.prompt_budget.to_prompt_budget();
                b.recall_min_relevance = config.memory.min_relevance_score;
                b
            })
            .continuation_policy(config.memory.prompt_budget.to_continuation_policy())
            .prompt_builder(SystemPromptBuilder::with_defaults())
            .config(config.agent.clone())
            .agent_id(resolved_agent_id)
            .model_name(model_name)
            .temperature(config.default_temperature)
            .workspace_dir(config.workspace_dir.clone())
            .classification_config(config.query_classification.clone())
            .available_hints(available_hints)
            .route_model_by_hint(route_model_by_hint)
            .identity_config(config.identity.clone())
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
        };
        self.prompt_builder.build(&ctx)
    }

    async fn execute_tool_call(&self, call: &ParsedToolCall) -> ToolExecutionResult {
        let start = Instant::now();
        let args_preview =
            synapse_domain::domain::util::truncate_with_ellipsis(&call.arguments.to_string(), 300);

        self.observer.record_event(&ObserverEvent::ToolCallStart {
            tool: call.name.clone(),
            arguments: Some(args_preview),
        });

        let result = if let Some(tool) = self.tools.iter().find(|t| t.name() == call.name) {
            match tool.execute(call.arguments.clone()).await {
                Ok(r) => {
                    let duration = start.elapsed();
                    self.observer.record_event(&ObserverEvent::ToolCall {
                        tool: call.name.clone(),
                        duration,
                        success: r.success,
                    });
                    if r.success {
                        self.observer.record_event(&ObserverEvent::ToolResult {
                            tool: call.name.clone(),
                            output: synapse_domain::domain::util::truncate_with_ellipsis(
                                &r.output, 500,
                            ),
                            success: true,
                        });
                        r.output
                    } else {
                        let reason = r.error.unwrap_or(r.output);
                        self.observer.record_event(&ObserverEvent::ToolResult {
                            tool: call.name.clone(),
                            output: synapse_domain::domain::util::truncate_with_ellipsis(
                                &reason, 500,
                            ),
                            success: false,
                        });
                        format!("Error: {reason}")
                    }
                }
                Err(e) => {
                    let duration = start.elapsed();
                    self.observer.record_event(&ObserverEvent::ToolCall {
                        tool: call.name.clone(),
                        duration,
                        success: false,
                    });
                    let reason = format!("Error executing {}: {e}", call.name);
                    self.observer.record_event(&ObserverEvent::ToolResult {
                        tool: call.name.clone(),
                        output: synapse_domain::domain::util::truncate_with_ellipsis(&reason, 500),
                        success: false,
                    });
                    reason
                }
            }
        } else {
            let reason = format!("Unknown tool: {}", call.name);
            self.observer.record_event(&ObserverEvent::ToolResult {
                tool: call.name.clone(),
                output: reason.clone(),
                success: false,
            });
            reason
        };

        ToolExecutionResult {
            name: call.name.clone(),
            output: result,
            success: true,
            tool_call_id: call.tool_call_id.clone(),
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

    pub async fn turn(&mut self, user_message: &str) -> Result<String> {
        self.last_turn_usage = None;
        if self.history.is_empty() {
            let system_prompt = self.build_system_prompt()?;
            self.history
                .push(ConversationMessage::Chat(ChatMessage::system(
                    system_prompt,
                )));
        }

        // ── Unified turn context assembly ──
        let continuation = if self.turn_count > 0 {
            Some(&self.continuation_policy)
        } else {
            None
        };
        let turn_ctx = tc::assemble_turn_context(
            self.memory.as_ref(),
            user_message,
            &self.agent_id,
            self.memory_session_id.as_deref(),
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

        if self.auto_save {
            let _ = self
                .memory
                .store(
                    "user_msg",
                    user_message,
                    &MemoryCategory::Conversation,
                    self.memory_session_id.as_deref(),
                )
                .await;
        }

        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z");
        let raw_user_msg = format!("[{now}] {user_message}");

        // Store raw user message in history (no recall/skills/entities baked in)
        self.history
            .push(ConversationMessage::Chat(ChatMessage::user(
                raw_user_msg.clone(),
            )));

        // Enrichment prefix for provider call only (not persisted in history)
        let enrichment_prefix = formatted.enrichment_prefix;
        self.turn_count += 1;

        let effective_model = self.classify_model(user_message);

        for _ in 0..self.config.max_tool_iterations {
            let mut messages = self.tool_dispatcher.to_provider_messages(&self.history);

            // Inject recall/skills/entities as ephemeral prefix on the user
            // message for the provider call — not persisted in history.
            if !enrichment_prefix.is_empty() {
                if let Some(last_user) = messages.iter_mut().rfind(|m| m.role == "user") {
                    if last_user.content == raw_user_msg {
                        last_user.content =
                            format!("{enrichment_prefix}{}", last_user.content);
                    }
                }
            }

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
                    self.trim_history();
                    return Ok(cached);
                }
                self.observer.record_event(&ObserverEvent::CacheMiss {
                    cache_type: "response".into(),
                });
            }

            let response = match self
                .provider
                .chat(
                    ChatRequest {
                        messages: &messages,
                        tools: if self.tool_dispatcher.should_send_tool_specs() {
                            Some(&self.tool_specs)
                        } else {
                            None
                        },
                    },
                    &effective_model,
                    self.temperature,
                )
                .await
            {
                Ok(resp) => resp,
                Err(err) => return Err(err),
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

            let (text, calls) = self.tool_dispatcher.parse_response(&response);
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
                self.trim_history();

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

            let results = self.execute_tools(&calls).await;
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
    if let Some(p) = provider_override {
        effective_config.default_provider = Some(p);
    }
    if let Some(m) = model_override {
        effective_config.default_model = Some(m);
    }
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
        .unwrap_or("anthropic/claude-sonnet-4-20250514")
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use std::collections::HashMap;

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
}
