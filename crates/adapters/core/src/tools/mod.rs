//! Tool subsystem for agent-callable capabilities.
//!
//! This module implements the tool execution surface exposed to the LLM during
//! agentic loops. Each tool implements the [`Tool`] trait defined in [`traits`],
//! which requires a name, description, JSON parameter schema, and an async
//! `execute` method returning a structured [`ToolResult`].
//!
//! Tools are assembled into registries by [`default_tools`] (shell, file read/write)
//! and [`all_tools`] (full set including memory, browser, cron, HTTP, delegation,
//! and optional integrations). Security policy enforcement is injected via
//! [`SecurityPolicy`](synapse_domain::domain::security_policy::SecurityPolicy) at construction time.
//!
//! # Extension
//!
//! To add a new tool, implement [`Tool`] in a new submodule and register it in
//! [`all_tools_with_runtime`]. See `AGENTS.md` §7.3 for the full change playbook.

// ── Re-exports from synapse_tools crate ──
pub use synapse_tools::*;

// ── Re-exports from synapse_mcp crate ──
pub use synapse_mcp::tool_search::ToolSearchTool;
pub use synapse_mcp::McpRegistry;
pub use synapse_mcp::McpToolWrapper;
pub use synapse_mcp::{ActivatedToolSet, DeferredMcpToolSet};

// ── Modules that remain in core (agent/gateway dependencies) ──
pub mod agents_ipc;
pub mod delegate;
pub mod node_tool;
pub mod traits;

pub use agents_ipc::{
    AgentsInboxTool, AgentsListTool, AgentsReplyTool, AgentsSendTool, AgentsSpawnTool, IpcClient,
    StateGetTool, StateSetTool,
};
pub use delegate::DelegateTool;
#[allow(unused_imports)]
pub use node_tool::NodeTool;

use crate::runtime::native::NativeRuntime;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use synapse_domain::config::schema::{Config, DelegateAgentConfig};
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::runtime::RuntimeAdapter;
use synapse_memory::UnifiedMemoryPort;
use synapse_tools::core_memory_update::CoreMemoryUpdateTool;

/// No-op agent runner for contexts where the runner is not available (e.g., tests).
struct NoopAgentRunner;

#[async_trait]
impl synapse_domain::ports::agent_runner::AgentRunnerPort for NoopAgentRunner {
    async fn run(
        &self,
        _message: Option<String>,
        _provider_override: Option<String>,
        _model_override: Option<String>,
        _temperature: f64,
        _interactive: bool,
        _session_state_file: Option<std::path::PathBuf>,
        _allowed_tools: Option<Vec<String>>,
        _run_ctx: Option<Arc<synapse_domain::domain::tool_audit::RunContext>>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("AgentRunner not available in this context")
    }

    async fn process_message(
        &self,
        _message: &str,
        _session_id: Option<&str>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("AgentRunner not available in this context")
    }
}

/// Shared handle to the delegate tool's parent-tools list.
/// Callers can push additional tools (e.g. MCP wrappers) after construction.
pub type DelegateParentToolsHandle = Arc<RwLock<Vec<Arc<dyn Tool>>>>;

pub use synapse_domain::ports::tool::ArcToolRef;

#[derive(Clone)]
struct ArcDelegatingTool {
    inner: Arc<dyn Tool>,
}

impl ArcDelegatingTool {
    fn boxed(inner: Arc<dyn Tool>) -> Box<dyn Tool> {
        Box::new(Self { inner })
    }
}

#[async_trait]
impl Tool for ArcDelegatingTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    fn runtime_role(&self) -> Option<synapse_domain::ports::tool::ToolRuntimeRole> {
        self.inner.runtime_role()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.inner.execute(args).await
    }

    async fn execute_with_facts(
        &self,
        args: serde_json::Value,
    ) -> anyhow::Result<synapse_domain::ports::tool::ToolExecution> {
        self.inner.execute_with_facts(args).await
    }
}

fn boxed_registry_from_arcs(tools: Vec<Arc<dyn Tool>>) -> Vec<Box<dyn Tool>> {
    tools.into_iter().map(ArcDelegatingTool::boxed).collect()
}

/// Create the default tool registry
pub fn default_tools(security: Arc<SecurityPolicy>) -> Vec<Box<dyn Tool>> {
    default_tools_with_runtime(security, Arc::new(NativeRuntime::new()))
}

/// Create the default tool registry with explicit runtime adapter.
pub fn default_tools_with_runtime(
    security: Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ShellTool::new(security.clone(), runtime)),
        Box::new(FileReadTool::new(security.clone())),
        Box::new(FileWriteTool::new(security.clone())),
        Box::new(FileEditTool::new(security.clone())),
        Box::new(GlobSearchTool::new(security.clone())),
        Box::new(ContentSearchTool::new(security)),
    ]
}

/// Create full tool registry including memory tools and optional Composio
#[allow(
    clippy::implicit_hasher,
    clippy::too_many_arguments,
    clippy::type_complexity
)]
pub fn all_tools(
    config: Arc<Config>,
    security: &Arc<SecurityPolicy>,
    memory: Arc<dyn UnifiedMemoryPort>,
    composio_key: Option<&str>,
    composio_entity_id: Option<&str>,
    browser_config: &synapse_domain::config::schema::BrowserConfig,
    http_config: &synapse_domain::config::schema::HttpRequestConfig,
    web_fetch_config: &synapse_domain::config::schema::WebFetchConfig,
    workspace_dir: &std::path::Path,
    agents: &HashMap<String, DelegateAgentConfig>,
    fallback_api_key: Option<&str>,
    root_config: &synapse_domain::config::schema::Config,
    shared_ipc_client: Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
) -> (
    Vec<Box<dyn Tool>>,
    Option<DelegateParentToolsHandle>,
    Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
) {
    all_tools_with_runtime(
        config,
        security,
        Arc::new(NativeRuntime::new()),
        memory,
        composio_key,
        composio_entity_id,
        browser_config,
        http_config,
        web_fetch_config,
        workspace_dir,
        agents,
        fallback_api_key,
        root_config,
        shared_ipc_client,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Create full tool registry including memory tools and optional Composio.
#[allow(
    clippy::implicit_hasher,
    clippy::too_many_arguments,
    clippy::type_complexity
)]
pub fn all_tools_with_runtime(
    config: Arc<Config>,
    security: &Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
    memory: Arc<dyn UnifiedMemoryPort>,
    composio_key: Option<&str>,
    composio_entity_id: Option<&str>,
    browser_config: &synapse_domain::config::schema::BrowserConfig,
    http_config: &synapse_domain::config::schema::HttpRequestConfig,
    web_fetch_config: &synapse_domain::config::schema::WebFetchConfig,
    workspace_dir: &std::path::Path,
    agents: &HashMap<String, DelegateAgentConfig>,
    fallback_api_key: Option<&str>,
    root_config: &synapse_domain::config::schema::Config,
    shared_ipc_client: Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
    agent_runner: Option<Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>>,
    cron_db: Option<Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>>,
    conversation_context: Option<
        Arc<dyn synapse_domain::ports::conversation_context::ConversationContextPort>,
    >,
    conversation_store: Option<
        Arc<dyn synapse_domain::ports::conversation_store::ConversationStorePort>,
    >,
    channel_registry: Option<Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>>,
    standing_order_store: Option<
        Arc<dyn synapse_domain::ports::standing_order_store::StandingOrderStorePort>,
    >,
    user_profile_store: Option<
        Arc<dyn synapse_domain::ports::user_profile_store::UserProfileStorePort>,
    >,
    user_profile_context: Option<
        Arc<dyn synapse_domain::ports::user_profile_context::UserProfileContextPort>,
    >,
    turn_defaults_context: Option<
        Arc<dyn synapse_domain::ports::turn_defaults_context::TurnDefaultsContextPort>,
    >,
    run_recipe_store: Option<Arc<dyn synapse_domain::ports::run_recipe_store::RunRecipeStorePort>>,
) -> (
    Vec<Box<dyn Tool>>,
    Option<DelegateParentToolsHandle>,
    Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
) {
    let standing_order_store: Arc<
        dyn synapse_domain::ports::standing_order_store::StandingOrderStorePort,
    > = if let Some(store) = standing_order_store {
        store
    } else {
        let store_path = config
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("standing_orders.json");
        match synapse_infra::standing_order_store::FileStandingOrderStore::new(&store_path) {
            Ok(store) => Arc::new(store),
            Err(error) => {
                tracing::warn!(
                    path = %store_path.display(),
                    %error,
                    "Failed to initialize persistent standing order store, falling back to memory"
                );
                Arc::new(
                    synapse_domain::ports::standing_order_store::InMemoryStandingOrderStore::new(),
                )
            }
        }
    };
    let user_profile_store: Arc<
        dyn synapse_domain::ports::user_profile_store::UserProfileStorePort,
    > = if let Some(store) = user_profile_store {
        store
    } else {
        let store_path = config
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("user_profiles.json");
        match synapse_infra::user_profile_store::FileUserProfileStore::new(&store_path) {
            Ok(store) => Arc::new(store),
            Err(error) => {
                tracing::warn!(
                    path = %store_path.display(),
                    %error,
                    "Failed to initialize persistent user profile store, falling back to memory"
                );
                Arc::new(synapse_domain::ports::user_profile_store::InMemoryUserProfileStore::new())
            }
        }
    };
    let run_recipe_store: Arc<dyn synapse_domain::ports::run_recipe_store::RunRecipeStorePort> =
        if let Some(store) = run_recipe_store {
            store
        } else {
            let store_path = config
                .config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("run_recipes.json");
            match synapse_infra::run_recipe_store::FileRunRecipeStore::new(&store_path) {
                Ok(store) => Arc::new(store),
                Err(error) => {
                    tracing::warn!(
                        path = %store_path.display(),
                        %error,
                        "Failed to initialize persistent run recipe store, falling back to memory"
                    );
                    Arc::new(synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new())
                }
            }
        };

    let has_shell_access = runtime.has_shell_access();
    let mut tool_arcs: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ShellTool::new(security.clone(), runtime)),
        Arc::new(FileReadTool::new(security.clone())),
        Arc::new(FileWriteTool::new(security.clone())),
        Arc::new(FileEditTool::new(security.clone())),
        Arc::new(GlobSearchTool::new(security.clone())),
        Arc::new(ContentSearchTool::new(security.clone())),
    ];

    // Cron tools require a SurrealDB handle; only register when available.
    if let Some(ref db) = cron_db {
        tool_arcs.extend([
            Arc::new(CronAddTool::new(
                config.clone(),
                security.clone(),
                db.clone(),
                conversation_context.clone(),
            )) as Arc<dyn Tool>,
            Arc::new(CronListTool::new(config.clone(), db.clone())),
            Arc::new(CronRemoveTool::new(
                config.clone(),
                security.clone(),
                db.clone(),
            )),
            Arc::new(CronUpdateTool::new(
                config.clone(),
                security.clone(),
                db.clone(),
            )),
            Arc::new(CronRunTool::new(
                config.clone(),
                security.clone(),
                agent_runner
                    .clone()
                    .unwrap_or_else(|| Arc::from(NoopAgentRunner)),
                db.clone(),
            )),
            Arc::new(CronRunsTool::new(config.clone(), db.clone())),
        ]);
    }

    tool_arcs.extend([
        Arc::new(MemoryStoreTool::new(memory.clone(), security.clone())) as Arc<dyn Tool>,
        Arc::new(MemoryRecallTool::new(
            memory.clone(),
            crate::agent::resolve_agent_id(root_config),
        )),
        Arc::new(MemoryForgetTool::new(
            memory.clone(),
            security.clone(),
            crate::agent::resolve_agent_id(root_config),
        )),
        Arc::new(CoreMemoryUpdateTool::new(
            memory.clone(),
            security.clone(),
            crate::agent::resolve_agent_id(root_config),
        )),
    ]);

    if let Some(ref db) = cron_db {
        tool_arcs.push(Arc::new(ScheduleTool::new(
            security.clone(),
            root_config.clone(),
            db.clone(),
        )));
    }

    tool_arcs.extend([
        Arc::new(ModelRoutingConfigTool::new(
            config.clone(),
            security.clone(),
        )) as Arc<dyn Tool>,
        Arc::new(ProxyConfigTool::new(config.clone(), security.clone())),
        Arc::new(GitOperationsTool::new(
            security.clone(),
            workspace_dir.to_path_buf(),
        )),
        Arc::new(PushoverTool::new(
            security.clone(),
            workspace_dir.to_path_buf(),
        )),
        Arc::new(telegram_post::TelegramPostTool::new(
            security.clone(),
            workspace_dir.to_path_buf(),
        )),
    ]);

    if browser_config.enabled {
        // Add legacy browser_open tool for simple URL opening
        tool_arcs.push(Arc::new(BrowserOpenTool::new(
            security.clone(),
            browser_config.allowed_domains.clone(),
        )));
        // Add full browser automation tool (pluggable backend)
        tool_arcs.push(Arc::new(BrowserTool::new_with_backend(
            security.clone(),
            browser_config.allowed_domains.clone(),
            browser_config.session_name.clone(),
            browser_config.backend.clone(),
            browser_config.native_headless,
            browser_config.native_webdriver_url.clone(),
            browser_config.native_chrome_path.clone(),
            ComputerUseConfig {
                endpoint: browser_config.computer_use.endpoint.clone(),
                api_key: browser_config.computer_use.api_key.clone(),
                timeout_ms: browser_config.computer_use.timeout_ms,
                allow_remote_endpoint: browser_config.computer_use.allow_remote_endpoint,
                window_allowlist: browser_config.computer_use.window_allowlist.clone(),
                max_coordinate_x: browser_config.computer_use.max_coordinate_x,
                max_coordinate_y: browser_config.computer_use.max_coordinate_y,
            },
        )));
    }

    // Browser delegation tool (conditionally registered; requires shell access)
    #[cfg(feature = "browser-native")]
    if root_config.browser_delegate.enabled {
        if has_shell_access {
            tool_arcs.push(Arc::new(synapse_tools::BrowserDelegateTool::new(
                security.clone(),
                root_config.browser_delegate.clone(),
            )));
        } else {
            tracing::warn!(
                "browser_delegate: skipped registration because the current runtime does not allow shell access"
            );
        }
    }

    if http_config.enabled {
        tool_arcs.push(Arc::new(HttpRequestTool::new(
            security.clone(),
            http_config.allowed_domains.clone(),
            http_config.max_response_size,
            http_config.timeout_secs,
            http_config.allow_private_hosts,
        )));
    }

    if web_fetch_config.enabled {
        tool_arcs.push(Arc::new(WebFetchTool::new(
            security.clone(),
            web_fetch_config.allowed_domains.clone(),
            web_fetch_config.blocked_domains.clone(),
            web_fetch_config.max_response_size,
            web_fetch_config.timeout_secs,
        )));
    }

    // Web search tool (enabled by default for GLM and other models)
    if root_config.web_search.enabled {
        tool_arcs.push(Arc::new(WebSearchTool::new_with_config(
            root_config.web_search.provider.clone(),
            root_config.web_search.brave_api_key.clone(),
            root_config.web_search.tavily_api_key.clone(),
            root_config.web_search.max_results,
            root_config.web_search.timeout_secs,
            root_config.config_path.clone(),
            root_config.secrets.encrypt,
        )));

        // Tavily Extract tool — available when Tavily is the search provider
        if root_config.web_search.provider == "tavily" {
            tool_arcs.push(Arc::new(
                tavily_extract::TavilyExtractTool::new_with_config(
                    root_config.web_search.tavily_api_key.clone(),
                    root_config.web_search.timeout_secs,
                    root_config.config_path.clone(),
                    root_config.secrets.encrypt,
                ),
            ));
        }
    }

    // Notion API tool (conditionally registered)
    if root_config.notion.enabled {
        let notion_api_key = if root_config.notion.api_key.trim().is_empty() {
            std::env::var("NOTION_API_KEY").unwrap_or_default()
        } else {
            root_config.notion.api_key.trim().to_string()
        };
        if notion_api_key.trim().is_empty() {
            tracing::warn!(
                "Notion tool enabled but no API key found (set notion.api_key or NOTION_API_KEY env var)"
            );
        } else {
            tool_arcs.push(Arc::new(NotionTool::new(notion_api_key, security.clone())));
        }
    }

    // Project delivery intelligence
    if root_config.project_intel.enabled {
        tool_arcs.push(Arc::new(ProjectIntelTool::new(
            root_config.project_intel.default_language.clone(),
            root_config.project_intel.risk_sensitivity.clone(),
        )));
    }

    // MCSS Security Operations
    if root_config.security_ops.enabled {
        tool_arcs.push(Arc::new(SecurityOpsTool::new(
            root_config.security_ops.clone(),
        )));
    }

    // Backup tool (enabled by default)
    if root_config.backup.enabled {
        tool_arcs.push(Arc::new(BackupTool::new(
            workspace_dir.to_path_buf(),
            root_config.backup.include_dirs.clone(),
            root_config.backup.max_keep,
        )));
    }

    // Data management tool (disabled by default)
    if root_config.data_retention.enabled {
        tool_arcs.push(Arc::new(DataManagementTool::new(
            workspace_dir.to_path_buf(),
            root_config.data_retention.retention_days,
        )));
    }

    // Cloud operations advisory tools (read-only analysis)
    if root_config.cloud_ops.enabled {
        tool_arcs.push(Arc::new(CloudOpsTool::new(root_config.cloud_ops.clone())));
        tool_arcs.push(Arc::new(CloudPatternsTool::new()));
    }

    // Google Workspace CLI (gws) integration — requires shell access
    if root_config.google_workspace.enabled && has_shell_access {
        tool_arcs.push(Arc::new(GoogleWorkspaceTool::new(
            security.clone(),
            root_config.google_workspace.allowed_services.clone(),
            root_config.google_workspace.credentials_path.clone(),
            root_config.google_workspace.default_account.clone(),
            root_config.google_workspace.rate_limit_per_minute,
            root_config.google_workspace.timeout_secs,
            root_config.google_workspace.audit_log,
        )));
    } else if root_config.google_workspace.enabled {
        tracing::warn!(
            "google_workspace: skipped registration because shell access is unavailable"
        );
    }

    // PDF extraction (feature-gated at compile time via rag-pdf)
    #[cfg(feature = "rag-pdf")]
    tool_arcs.push(Arc::new(synapse_tools::pdf_read::PdfReadTool::new(
        security.clone(),
    )));

    // Vision tools are always available
    tool_arcs.push(Arc::new(ScreenshotTool::new(security.clone())));
    tool_arcs.push(Arc::new(ImageInfoTool::new(security.clone())));

    // LinkedIn integration (config-gated)
    if root_config.linkedin.enabled {
        tool_arcs.push(Arc::new(LinkedInTool::new(
            security.clone(),
            workspace_dir.to_path_buf(),
            root_config.linkedin.api_version.clone(),
            root_config.linkedin.content.clone(),
            root_config.linkedin.image.clone(),
        )));
    }

    if let Some(key) = composio_key {
        if !key.is_empty() {
            tool_arcs.push(Arc::new(ComposioTool::new(
                key,
                composio_entity_id,
                security.clone(),
            )));
        }
    }

    // IPC tools (inter-agent communication via broker).
    // In daemon mode, a shared IpcClient is injected to avoid duplicate seq counters.
    let mut ipc_client_for_registration: Option<
        Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>,
    > = None;
    if root_config.agents_ipc.enabled {
        if let Some(ref token) = root_config.agents_ipc.broker_token {
            let ipc_client = if let Some(shared) = shared_ipc_client {
                // Daemon mode: reuse the shared IpcClient (single AtomicI64 seq counter).
                // Key registration is handled by daemon::run().
                shared
            } else {
                // Standalone / agent mode: create a local IpcClient.
                let mut client = IpcClient::new(
                    &root_config.agents_ipc.broker_url,
                    token,
                    root_config.agents_ipc.request_timeout_secs,
                );

                // Phase 3B: Load or generate Ed25519 identity for message signing
                let key_path = root_config
                    .config_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("agent.key");
                match synapse_security::identity::AgentIdentity::load_or_generate(&key_path) {
                    Ok(identity) => {
                        let agent_id = root_config
                            .agents_ipc
                            .agent_id
                            .clone()
                            .unwrap_or_else(|| root_config.agents_ipc.role.clone());
                        tracing::info!(
                            key_path = %key_path.display(),
                            agent_id = %agent_id,
                            pubkey = &identity.public_key_hex()[..16],
                            "Ed25519 agent identity loaded"
                        );
                        client = client.with_identity(identity, agent_id);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Failed to load Ed25519 identity — messages will be unsigned"
                        );
                    }
                }
                Arc::new(client) as Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>
            };

            if ipc_client.has_identity() {
                ipc_client_for_registration = Some(ipc_client.clone());
            }
            tool_arcs.push(Arc::new(AgentsListTool::new(ipc_client.clone())));
            tool_arcs.push(Arc::new(AgentsSendTool::new(ipc_client.clone())));
            tool_arcs.push(Arc::new(AgentsInboxTool::with_filter(
                ipc_client.clone(),
                root_config.agents_ipc.inbox_filter.clone(),
            )));
            tool_arcs.push(Arc::new(AgentsReplyTool::new(ipc_client.clone())));
            tool_arcs.push(Arc::new(StateGetTool::new(ipc_client.clone())));
            tool_arcs.push(Arc::new(StateSetTool::new(ipc_client.clone())));
            // Broker-backed spawn: provision ephemeral identity + subprocess + wait
            tool_arcs.push(Arc::new(AgentsSpawnTool::with_broker(
                config.clone(),
                security.clone(),
                root_config.agents_ipc.trust_level,
                ipc_client,
                cron_db.clone(),
            )));
        } else {
            // Legacy spawn: fire-and-forget in-process cron job (no broker)
            tool_arcs.push(Arc::new(AgentsSpawnTool::new(
                config.clone(),
                security.clone(),
                root_config.agents_ipc.trust_level,
                cron_db.clone(),
            )));
        }
    }

    // Microsoft 365 Graph API integration
    if root_config.microsoft365.enabled {
        let ms_cfg = &root_config.microsoft365;
        let tenant_id = ms_cfg
            .tenant_id
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        let client_id = ms_cfg
            .client_id
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !tenant_id.is_empty() && !client_id.is_empty() {
            // Fail fast: client_credentials flow requires a client_secret at registration time.
            if ms_cfg.auth_flow.trim() == "client_credentials"
                && ms_cfg
                    .client_secret
                    .as_deref()
                    .map_or(true, |s| s.trim().is_empty())
            {
                tracing::error!(
                    "microsoft365: client_credentials auth_flow requires a non-empty client_secret"
                );
                return (boxed_registry_from_arcs(tool_arcs), None, None);
            }

            let resolved = microsoft365::types::Microsoft365ResolvedConfig {
                tenant_id,
                client_id,
                client_secret: ms_cfg.client_secret.clone(),
                auth_flow: ms_cfg.auth_flow.clone(),
                scopes: ms_cfg.scopes.clone(),
                token_cache_encrypted: ms_cfg.token_cache_encrypted,
                user_id: ms_cfg.user_id.as_deref().unwrap_or("me").to_string(),
            };
            // Store token cache in the config directory (next to config.toml),
            // not the workspace directory, to keep bearer tokens out of the
            // project tree.
            let cache_dir = root_config.config_path.parent().unwrap_or(workspace_dir);
            match Microsoft365Tool::new(resolved, security.clone(), cache_dir) {
                Ok(tool) => tool_arcs.push(Arc::new(tool)),
                Err(e) => {
                    tracing::error!("microsoft365: failed to initialize tool: {e}");
                }
            }
        } else {
            tracing::warn!(
                "microsoft365: skipped registration because tenant_id or client_id is empty"
            );
        }
    }

    // Phase 4.3: Knowledge graph via SemanticMemoryPort in SurrealDB.
    if root_config.knowledge.enabled {
        tool_arcs.push(Arc::new(synapse_tools::knowledge_tool::KnowledgeTool::new(
            Arc::clone(&memory),
        )));
    }

    // Add delegation tool when agents are configured
    let delegate_fallback_credential = fallback_api_key.and_then(|value| {
        let trimmed_value = value.trim();
        (!trimmed_value.is_empty()).then(|| trimmed_value.to_owned())
    });
    let provider_runtime_options = synapse_providers::ProviderRuntimeOptions {
        auth_profile_override: None,
        provider_api_url: root_config.api_url.clone(),
        synapseclaw_dir: root_config
            .config_path
            .parent()
            .map(std::path::PathBuf::from),
        secrets_encrypt: root_config.secrets.encrypt,
        reasoning_enabled: root_config.runtime.reasoning_enabled,
        reasoning_effort: root_config.runtime.reasoning_effort.clone(),
        provider_timeout_secs: Some(root_config.provider_timeout_secs),
        extra_headers: root_config.extra_headers.clone(),
        api_path: root_config.api_path.clone(),
        prompt_caching: root_config.agent.prompt_caching,
    };

    let delegate_handle: Option<DelegateParentToolsHandle> = if agents.is_empty() {
        None
    } else {
        let delegate_agents: HashMap<String, DelegateAgentConfig> = agents
            .iter()
            .map(|(name, cfg)| (name.clone(), cfg.clone()))
            .collect();
        let parent_tools = Arc::new(RwLock::new(tool_arcs.clone()));
        let delegate_tool = DelegateTool::new_with_options(
            delegate_agents,
            delegate_fallback_credential.clone(),
            security.clone(),
            provider_runtime_options.clone(),
        )
        .with_parent_tools(Arc::clone(&parent_tools))
        .with_multimodal_config(root_config.multimodal.clone());
        tool_arcs.push(Arc::new(delegate_tool));
        Some(parent_tools)
    };

    // Add swarm tool when swarms are configured
    if !root_config.swarms.is_empty() {
        let swarm_agents: HashMap<String, DelegateAgentConfig> = agents
            .iter()
            .map(|(name, cfg)| (name.clone(), cfg.clone()))
            .collect();
        tool_arcs.push(Arc::new(SwarmTool::new(
            root_config.swarms.clone(),
            swarm_agents,
            delegate_fallback_credential,
            security.clone(),
            provider_runtime_options,
        )));
    }

    // Workspace management tool (conditionally registered when workspace isolation is enabled)
    if root_config.workspace.enabled {
        let workspaces_dir = if root_config.workspace.workspaces_dir.starts_with("~/") {
            let home = directories::UserDirs::new()
                .map(|u| u.home_dir().to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            home.join(&root_config.workspace.workspaces_dir[2..])
        } else {
            std::path::PathBuf::from(&root_config.workspace.workspaces_dir)
        };
        let ws_manager = synapse_infra::workspace::WorkspaceManager::new(workspaces_dir);
        tool_arcs.push(Arc::new(WorkspaceTool::new(
            Arc::new(tokio::sync::RwLock::new(ws_manager)),
            security.clone(),
        )));
    }

    // ── Phase 4.6: Orchestration tools ──
    tool_arcs.push(Arc::new(synapse_tools::clarify::ClarifyTool::new()));
    tool_arcs.push(Arc::new(synapse_tools::todo::TodoTool::new(
        conversation_context.clone(),
    )));
    tool_arcs.push(Arc::new(synapse_tools::user_profile::UserProfileTool::new(
        Arc::clone(&user_profile_store),
        security.clone(),
        conversation_context.clone(),
        user_profile_context,
    )));
    if let (Some(ctx), Some(defaults), Some(reg)) = (
        conversation_context.as_ref(),
        turn_defaults_context.as_ref(),
        channel_registry.as_ref(),
    ) {
        tool_arcs.push(Arc::new(synapse_tools::message_send::MessageSendTool::new(
            Arc::clone(ctx),
            Arc::clone(defaults),
            Arc::clone(reg),
        )));
    }
    if let Some(store) = conversation_store {
        tool_arcs.push(Arc::new(
            synapse_tools::session_search::SessionSearchTool::new(Arc::clone(&memory), store),
        ));
    }
    tool_arcs.push(Arc::new(
        synapse_tools::precedent_search::PrecedentSearchTool::new(
            Arc::clone(&memory),
            Arc::clone(&run_recipe_store),
            crate::agent::resolve_agent_id(config.as_ref()),
        ),
    ));
    tool_arcs.push(Arc::new(
        synapse_tools::standing_order::StandingOrderTool::new(
            conversation_context,
            standing_order_store,
            crate::agent::resolve_agent_id(config.as_ref()),
        ),
    ));

    (
        boxed_registry_from_arcs(tool_arcs),
        delegate_handle,
        ipc_client_for_registration,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::config::schema::{BrowserConfig, Config};
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        }
    }

    #[test]
    fn default_tools_has_expected_count() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = default_tools(security);
        assert_eq!(tools.len(), 6);
    }

    #[test]
    fn all_tools_excludes_browser_when_disabled() {
        let tmp = TempDir::new().unwrap();
        let security = Arc::new(SecurityPolicy::default());
        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let browser = BrowserConfig {
            enabled: false,
            allowed_domains: vec!["example.com".into()],
            session_name: None,
            ..BrowserConfig::default()
        };
        let http = synapse_domain::config::schema::HttpRequestConfig::default();
        let cfg = test_config(&tmp);

        let (tools, _, _) = all_tools(
            Arc::new(Config::default()),
            &security,
            mem,
            None,
            None,
            &browser,
            &http,
            &synapse_domain::config::schema::WebFetchConfig::default(),
            tmp.path(),
            &HashMap::new(),
            None,
            &cfg,
            None,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(!names.contains(&"browser_open"));
        assert!(names.contains(&"schedule"));
        assert!(names.contains(&"model_routing_config"));
        assert!(names.contains(&"pushover"));
        assert!(names.contains(&"proxy_config"));
    }

    #[test]
    fn all_tools_includes_browser_when_enabled() {
        let tmp = TempDir::new().unwrap();
        let security = Arc::new(SecurityPolicy::default());
        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let browser = BrowserConfig {
            enabled: true,
            allowed_domains: vec!["example.com".into()],
            session_name: None,
            ..BrowserConfig::default()
        };
        let http = synapse_domain::config::schema::HttpRequestConfig::default();
        let cfg = test_config(&tmp);

        let (tools, _, _) = all_tools(
            Arc::new(Config::default()),
            &security,
            mem,
            None,
            None,
            &browser,
            &http,
            &synapse_domain::config::schema::WebFetchConfig::default(),
            tmp.path(),
            &HashMap::new(),
            None,
            &cfg,
            None,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"browser_open"));
        assert!(names.contains(&"content_search"));
        assert!(names.contains(&"model_routing_config"));
        assert!(names.contains(&"pushover"));
        assert!(names.contains(&"proxy_config"));
    }

    #[test]
    fn default_tools_names() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = default_tools(security);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"file_write"));
        assert!(names.contains(&"file_edit"));
        assert!(names.contains(&"glob_search"));
        assert!(names.contains(&"content_search"));
    }

    #[test]
    fn default_tools_all_have_descriptions() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = default_tools(security);
        for tool in &tools {
            assert!(
                !tool.description().is_empty(),
                "Tool {} has empty description",
                tool.name()
            );
        }
    }

    #[test]
    fn default_tools_all_have_schemas() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = default_tools(security);
        for tool in &tools {
            let schema = tool.parameters_schema();
            assert!(
                schema.is_object(),
                "Tool {} schema is not an object",
                tool.name()
            );
            assert!(
                schema["properties"].is_object(),
                "Tool {} schema has no properties",
                tool.name()
            );
        }
    }

    #[test]
    fn tool_spec_generation() {
        let security = Arc::new(SecurityPolicy::default());
        let tools = default_tools(security);
        for tool in &tools {
            let spec = tool.spec();
            assert_eq!(spec.name, tool.name());
            assert_eq!(spec.description, tool.description());
            assert!(spec.parameters.is_object());
        }
    }

    #[test]
    fn tool_result_serde() {
        let result = ToolResult {
            success: true,
            output: "hello".into(),
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.output, "hello");
        assert!(parsed.error.is_none());
    }

    #[test]
    fn tool_result_with_error_serde() {
        let result = ToolResult {
            success: false,
            output: String::new(),
            error: Some("boom".into()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(!parsed.success);
        assert_eq!(parsed.error.as_deref(), Some("boom"));
    }

    #[test]
    fn tool_spec_serde() {
        let spec = ToolSpec {
            name: "test".into(),
            description: "A test tool".into(),
            parameters: serde_json::json!({"type": "object"}),
            runtime_role: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: ToolSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.description, "A test tool");
    }

    #[test]
    fn all_tools_includes_delegate_when_agents_configured() {
        let tmp = TempDir::new().unwrap();
        let security = Arc::new(SecurityPolicy::default());
        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let browser = BrowserConfig::default();
        let http = synapse_domain::config::schema::HttpRequestConfig::default();
        let cfg = test_config(&tmp);

        let mut agents = HashMap::new();
        agents.insert(
            "researcher".to_string(),
            DelegateAgentConfig {
                provider: "ollama".to_string(),
                model: "llama3".to_string(),
                system_prompt: None,
                api_key: None,
                temperature: None,
                max_depth: 3,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: 10,
            },
        );

        let (tools, _, _) = all_tools(
            Arc::new(Config::default()),
            &security,
            mem,
            None,
            None,
            &browser,
            &http,
            &synapse_domain::config::schema::WebFetchConfig::default(),
            tmp.path(),
            &agents,
            Some("delegate-test-credential"),
            &cfg,
            None,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"delegate"));
    }

    #[test]
    fn all_tools_excludes_delegate_when_no_agents() {
        let tmp = TempDir::new().unwrap();
        let security = Arc::new(SecurityPolicy::default());
        let mem: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

        let browser = BrowserConfig::default();
        let http = synapse_domain::config::schema::HttpRequestConfig::default();
        let cfg = test_config(&tmp);

        let (tools, _, _) = all_tools(
            Arc::new(Config::default()),
            &security,
            mem,
            None,
            None,
            &browser,
            &http,
            &synapse_domain::config::schema::WebFetchConfig::default(),
            tmp.path(),
            &HashMap::new(),
            None,
            &cfg,
            None,
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(!names.contains(&"delegate"));
    }
}
