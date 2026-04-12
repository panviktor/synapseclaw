#[allow(unused_imports)]
use super::channel_traits::ChannelConfig;
#[allow(unused_imports)]
use super::provider_aliases::{is_glm_alias, is_zai_alias};
use crate::domain::config::AutonomyLevel;
use anyhow::{Context as _, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const SUPPORTED_PROXY_SERVICE_KEYS: &[&str] = &[
    "provider.anthropic",
    "provider.compatible",
    "provider.copilot",
    "provider.gemini",
    "provider.glm",
    "provider.ollama",
    "provider.openai",
    "provider.openrouter",
    "channel.dingtalk",
    "channel.discord",
    "channel.feishu",
    "channel.lark",
    "channel.matrix",
    "channel.mattermost",
    "channel.nextcloud_talk",
    "channel.qq",
    "channel.signal",
    "channel.slack",
    "channel.telegram",
    "channel.wati",
    "channel.whatsapp",
    "tool.browser",
    "tool.composio",
    "tool.http_request",
    "tool.pushover",
    "memory.embeddings",
    "tunnel.custom",
    "transcription.groq",
];

const SUPPORTED_PROXY_SERVICE_SELECTORS: &[&str] = &[
    "provider.*",
    "channel.*",
    "tool.*",
    "memory.*",
    "tunnel.*",
    "transcription.*",
];

// ── Top-level config ──────────────────────────────────────────────

/// Top-level SynapseClaw configuration, loaded from `config.toml`.
///
/// Resolution order: `SYNAPSECLAW_WORKSPACE` env → `active_workspace.toml` marker → `~/.synapseclaw/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// Workspace directory - computed from home, not serialized
    #[serde(skip)]
    pub workspace_dir: PathBuf,
    /// Path to config.toml - computed from home, not serialized
    #[serde(skip)]
    pub config_path: PathBuf,
    /// API key for the selected provider. Overridden by `SYNAPSECLAW_API_KEY` or `API_KEY` env vars.
    pub api_key: Option<String>,
    /// Base URL override for provider API (e.g. "http://10.0.0.1:11434" for remote Ollama)
    pub api_url: Option<String>,
    /// Custom API path suffix for OpenAI-compatible / custom providers
    /// (e.g. "/v2/generate" instead of the default "/v1/chat/completions").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path: Option<String>,
    /// Default provider ID or alias (e.g. `"openrouter"`, `"ollama"`, `"anthropic"`).
    /// The runtime default is resolved from the model catalog's `default_preset`.
    #[serde(alias = "model_provider")]
    pub default_provider: Option<String>,
    /// Default model routed through the selected provider (e.g. `"provider/model-id"`).
    #[serde(alias = "model")]
    pub default_model: Option<String>,
    /// Model used for session summarization (cheaper than primary). Falls back to `default_model`.
    /// Can be a plain model name (uses default provider) or configured via `[summary]` section.
    #[serde(default)]
    pub summary_model: Option<String>,
    /// Context compression policy (`[compression]`), Hermes-style defaults.
    #[serde(default)]
    pub compression: ContextCompressionConfig,
    /// Optional route/lane-specific compression policy overrides.
    #[serde(default)]
    pub compression_overrides: Vec<ContextCompressionRouteOverrideConfig>,
    /// Explicit summary model configuration with its own provider.
    /// When set, overrides `summary_model` string. Allows using a different provider
    /// for summaries while keeping a different default provider.
    ///
    /// ```toml
    /// [summary]
    /// provider = "summary-provider"
    /// model = "summary-model-id"
    /// temperature = 0.3
    /// ```
    #[serde(default)]
    pub summary: SummaryConfig,
    /// Optional named provider profiles keyed by id (Codex app-server compatible layout).
    #[serde(default)]
    pub model_providers: HashMap<String, ModelProviderConfig>,
    /// Default model temperature (0.0–2.0). Default: `0.7`.
    #[serde(
        default = "default_temperature",
        deserialize_with = "deserialize_temperature"
    )]
    pub default_temperature: f64,

    /// HTTP request timeout in seconds for LLM provider API calls. Default: `120`.
    ///
    /// Increase for slower backends (e.g., llama.cpp on constrained hardware)
    /// that need more time processing large contexts.
    #[serde(default = "default_provider_timeout_secs")]
    pub provider_timeout_secs: u64,

    /// Extra HTTP headers to include in LLM provider API requests.
    ///
    /// Some providers require specific headers (e.g., `User-Agent`, `HTTP-Referer`,
    /// `X-Title`) for request routing or policy enforcement. Headers defined here
    /// augment (and override) the program's default headers.
    ///
    /// Can also be set via `SYNAPSECLAW_EXTRA_HEADERS` environment variable using
    /// the format `Key:Value,Key2:Value2`. Env var headers override config file headers.
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,

    /// Observability backend configuration (`[observability]`).
    #[serde(default)]
    pub observability: ObservabilityConfig,

    /// Autonomy and security policy configuration (`[autonomy]`).
    #[serde(default)]
    pub autonomy: AutonomyConfig,

    /// Security subsystem configuration (`[security]`).
    #[serde(default)]
    pub security: SecurityConfig,

    /// Backup tool configuration (`[backup]`).
    #[serde(default)]
    pub backup: BackupConfig,

    /// Data retention and purge configuration (`[data_retention]`).
    #[serde(default)]
    pub data_retention: DataRetentionConfig,

    /// Cloud transformation accelerator configuration (`[cloud_ops]`).
    #[serde(default)]
    pub cloud_ops: CloudOpsConfig,

    /// Conversational AI agent builder configuration (`[conversational_ai]`).
    #[serde(default)]
    pub conversational_ai: ConversationalAiConfig,

    /// Managed cybersecurity service configuration (`[security_ops]`).
    #[serde(default)]
    pub security_ops: SecurityOpsConfig,

    /// Runtime adapter configuration (`[runtime]`). Controls native vs Docker execution.
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// Reliability settings: retries, fallback providers, backoff (`[reliability]`).
    #[serde(default)]
    pub reliability: ReliabilityConfig,

    /// Scheduler configuration for periodic task execution (`[scheduler]`).
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Agent orchestration settings (`[agent]`).
    #[serde(default)]
    pub agent: AgentConfig,

    /// Skills loading and community repository behavior (`[skills]`).
    #[serde(default)]
    pub skills: SkillsConfig,

    /// Catalog route aliases — route `hint:<name>` to specific provider+model combos.
    #[serde(default)]
    pub route_aliases: Vec<ModelRouteConfig>,

    /// Capability-aware model lanes — ordered candidates per runtime lane.
    ///
    /// Candidate `0` is the default for the lane; later entries act as
    /// fallbacks or manual runtime alternatives. This is the primary runtime
    /// routing surface.
    #[serde(default)]
    pub model_lanes: Vec<ModelLaneConfig>,

    /// Optional out-of-the-box routing preset that expands into capability
    /// lanes for typical user setups.
    ///
    /// Examples:
    /// - `chatgpt`
    /// - `claude`
    /// - `openrouter`
    /// - `local`
    ///
    /// Explicit `model_lanes` entries override the preset lane-by-lane.
    #[serde(default, alias = "preset")]
    pub model_preset: Option<String>,

    /// Embedding routing rules — route `hint:<name>` to specific provider+model combos.
    #[serde(default)]
    pub embedding_routes: Vec<EmbeddingRouteConfig>,

    /// Automatic query classification — maps user messages to model hints.
    #[serde(default)]
    pub query_classification: QueryClassificationConfig,

    /// Heartbeat configuration for periodic health pings (`[heartbeat]`).
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,

    /// Cron job configuration (`[cron]`).
    #[serde(default)]
    pub cron: CronConfig,

    /// Channel configurations: Telegram, Discord, Slack, etc. (`[channels_config]`).
    #[serde(default)]
    pub channels_config: ChannelsConfig,

    /// Memory backend configuration: sqlite, markdown, embeddings (`[memory]`).
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Persistent storage provider configuration (`[storage]`).
    #[serde(default)]
    pub storage: StorageConfig,

    /// Tunnel configuration for exposing the gateway publicly (`[tunnel]`).
    #[serde(default)]
    pub tunnel: TunnelConfig,

    /// Gateway server configuration: host, port, pairing, rate limits (`[gateway]`).
    #[serde(default)]
    pub gateway: GatewayConfig,

    /// Composio managed OAuth tools integration (`[composio]`).
    #[serde(default)]
    pub composio: ComposioConfig,

    /// Microsoft 365 Graph API integration (`[microsoft365]`).
    #[serde(default)]
    pub microsoft365: Microsoft365Config,

    /// Secrets encryption configuration (`[secrets]`).
    #[serde(default)]
    pub secrets: SecretsConfig,

    /// Browser automation configuration (`[browser]`).
    #[serde(default)]
    pub browser: BrowserConfig,

    /// Browser delegation configuration (`[browser_delegate]`).
    ///
    /// Delegates browser-based tasks to a browser-capable CLI subprocess (e.g.
    /// Claude Code with `claude-in-chrome` MCP tools). Useful for interacting
    /// with corporate web apps (Teams, Outlook, Jira, Confluence) that lack
    /// direct API access. A persistent Chrome profile can be configured so SSO
    /// sessions survive across invocations.
    ///
    /// Fields:
    /// - `enabled` (`bool`, default `false`) — enable the browser delegation tool.
    /// - `cli_binary` (`String`, default `"claude"`) — CLI binary to spawn for browser tasks.
    /// - `chrome_profile_dir` (`String`, default `""`) — Chrome user-data directory for
    ///   persistent SSO sessions. When empty, a fresh profile is used each invocation.
    /// - `allowed_domains` (`Vec<String>`, default `[]`) — allowlist of domains the browser
    ///   may navigate to. Empty means all non-blocked domains are permitted.
    /// - `blocked_domains` (`Vec<String>`, default `[]`) — denylist of domains. Blocked
    ///   domains take precedence over allowed domains.
    /// - `task_timeout_secs` (`u64`, default `120`) — per-task timeout in seconds.
    ///
    /// Compatibility: additive and disabled by default; existing configs remain valid when omitted.
    /// Rollback/migration: remove `[browser_delegate]` or keep `enabled = false` to disable.
    #[serde(default)]
    pub browser_delegate: super::adapter_configs::BrowserDelegateConfig,

    /// HTTP request tool configuration (`[http_request]`).
    #[serde(default)]
    pub http_request: HttpRequestConfig,

    /// Multimodal (image) handling configuration (`[multimodal]`).
    #[serde(default)]
    pub multimodal: MultimodalConfig,

    /// Web fetch tool configuration (`[web_fetch]`).
    #[serde(default)]
    pub web_fetch: WebFetchConfig,

    /// Web search tool configuration (`[web_search]`).
    #[serde(default)]
    pub web_search: WebSearchConfig,

    /// Project delivery intelligence configuration (`[project_intel]`).
    #[serde(default)]
    pub project_intel: ProjectIntelConfig,

    /// Google Workspace CLI (`gws`) tool configuration (`[google_workspace]`).
    #[serde(default)]
    pub google_workspace: GoogleWorkspaceConfig,

    /// Proxy configuration for outbound HTTP/HTTPS/SOCKS5 traffic (`[proxy]`).
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// Identity format configuration: OpenClaw or AIEOS (`[identity]`).
    #[serde(default)]
    pub identity: IdentityConfig,

    /// Cost tracking and budget enforcement configuration (`[cost]`).
    #[serde(default)]
    pub cost: CostConfig,

    /// Delegate agent configurations for multi-agent workflows.
    #[serde(default)]
    pub agents: HashMap<String, DelegateAgentConfig>,

    /// Swarm configurations for multi-agent orchestration.
    #[serde(default)]
    pub swarms: HashMap<String, SwarmConfig>,

    /// Hooks configuration (lifecycle hooks and built-in hook toggles).
    #[serde(default)]
    pub hooks: HooksConfig,

    /// Voice transcription configuration (Whisper API via Groq).
    #[serde(default)]
    pub transcription: TranscriptionConfig,

    /// Text-to-Speech configuration (`[tts]`).
    #[serde(default)]
    pub tts: TtsConfig,

    /// External MCP server connections (`[mcp]`).
    #[serde(default, alias = "mcpServers")]
    pub mcp: McpConfig,

    /// Inter-agent IPC configuration (`[agents_ipc]`).
    #[serde(default)]
    pub agents_ipc: AgentsIpcConfig,

    /// Pipeline engine configuration (`[pipelines]`).
    #[serde(default)]
    pub pipelines: PipelineEngineConfig,
    /// Dynamic node discovery configuration (`[nodes]`).
    #[serde(default)]
    pub nodes: NodesConfig,

    /// Multi-client workspace isolation configuration (`[workspace]`).
    #[serde(default)]
    pub workspace: WorkspaceConfig,

    /// Notion integration configuration (`[notion]`).
    #[serde(default)]
    pub notion: NotionConfig,

    /// Secure inter-node transport configuration (`[node_transport]`).
    #[serde(default)]
    pub node_transport: NodeTransportConfig,

    /// Knowledge graph configuration (`[knowledge]`).
    #[serde(default)]
    pub knowledge: KnowledgeConfig,

    /// LinkedIn integration configuration (`[linkedin]`).
    #[serde(default)]
    pub linkedin: LinkedInConfig,
}

/// Multi-client workspace isolation configuration.
///
/// When enabled, each client engagement gets an isolated workspace with
/// separate memory, audit, secrets, and tool restrictions.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceConfig {
    /// Enable workspace isolation. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Currently active workspace name.
    #[serde(default)]
    pub active_workspace: Option<String>,
    /// Base directory for workspace profiles.
    #[serde(default = "default_workspaces_dir")]
    pub workspaces_dir: String,
    /// Isolate memory databases per workspace. Default: true.
    #[serde(default = "default_true")]
    pub isolate_memory: bool,
    /// Isolate secrets namespaces per workspace. Default: true.
    #[serde(default = "default_true")]
    pub isolate_secrets: bool,
    /// Isolate audit logs per workspace. Default: true.
    #[serde(default = "default_true")]
    pub isolate_audit: bool,
    /// Allow searching across workspaces. Default: false (security).
    #[serde(default)]
    pub cross_workspace_search: bool,
}

fn default_workspaces_dir() -> String {
    "~/.synapseclaw/workspaces".to_string()
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active_workspace: None,
            workspaces_dir: default_workspaces_dir(),
            isolate_memory: true,
            isolate_secrets: true,
            isolate_audit: true,
            cross_workspace_search: false,
        }
    }
}

/// Named provider profile definition compatible with Codex app-server style config.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ModelProviderConfig {
    /// Optional provider type/name override (e.g. "openai", "openai-codex", or custom profile id).
    #[serde(default)]
    pub name: Option<String>,
    /// Optional base URL for OpenAI-compatible endpoints.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional custom API path suffix (e.g. "/v2/generate" instead of the
    /// default "/v1/chat/completions"). Only used by OpenAI-compatible / custom providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path: Option<String>,
    /// Provider protocol variant ("responses" or "chat_completions").
    #[serde(default)]
    pub wire_api: Option<String>,
    /// If true, load OpenAI auth material (OPENAI_API_KEY or ~/.codex/auth.json).
    #[serde(default)]
    pub requires_openai_auth: bool,
    /// Azure OpenAI resource name (e.g. "my-resource" in https://my-resource.openai.azure.com).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_openai_resource: Option<String>,
    /// Azure OpenAI deployment name (e.g. "chat-deployment").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_openai_deployment: Option<String>,
    /// Azure OpenAI API version (defaults to "2024-08-01-preview").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_openai_api_version: Option<String>,
}

// ── Delegate Agents ──────────────────────────────────────────────

/// Configuration for a delegate sub-agent used by the `delegate` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegateAgentConfig {
    /// Provider name (e.g. "ollama", "openrouter", "anthropic")
    pub provider: String,
    /// Model name
    pub model: String,
    /// Optional system prompt for the sub-agent
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Optional API key override
    #[serde(default)]
    pub api_key: Option<String>,
    /// Temperature override
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Max recursion depth for nested delegation
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Enable agentic sub-agent mode (multi-turn tool-call loop).
    #[serde(default)]
    pub agentic: bool,
    /// Allowlist of tool names available to the sub-agent in agentic mode.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Maximum tool-call iterations in agentic mode.
    #[serde(default = "default_max_tool_iterations")]
    pub max_iterations: usize,
}

// ── Swarms ──────────────────────────────────────────────────────

/// Orchestration strategy for a swarm of agents.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SwarmStrategy {
    /// Run agents sequentially; each agent's output feeds into the next.
    Sequential,
    /// Run agents in parallel; collect all outputs.
    Parallel,
    /// Use the LLM to pick the best agent for the task.
    Router,
}

/// Configuration for a swarm of coordinated agents.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SwarmConfig {
    /// Ordered list of agent names (must reference keys in `agents`).
    pub agents: Vec<String>,
    /// Orchestration strategy.
    pub strategy: SwarmStrategy,
    /// System prompt for router strategy (used to pick the best agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_prompt: Option<String>,
    /// Optional description shown to the LLM when choosing swarms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Maximum total timeout for the swarm execution in seconds.
    #[serde(default = "default_swarm_timeout_secs")]
    pub timeout_secs: u64,
}

const DEFAULT_SWARM_TIMEOUT_SECS: u64 = 300;

fn default_swarm_timeout_secs() -> u64 {
    DEFAULT_SWARM_TIMEOUT_SECS
}

/// Valid temperature range for all paths (config, CLI, env override).
pub const TEMPERATURE_RANGE: std::ops::RangeInclusive<f64> = 0.0..=2.0;

/// Default temperature when the field is absent from config.
const DEFAULT_TEMPERATURE: f64 = 0.7;

fn default_temperature() -> f64 {
    DEFAULT_TEMPERATURE
}

/// Default provider HTTP request timeout: 120 seconds.
const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 120;

fn default_provider_timeout_secs() -> u64 {
    DEFAULT_PROVIDER_TIMEOUT_SECS
}

/// Validate that a temperature value is within the allowed range.
pub fn validate_temperature(value: f64) -> std::result::Result<f64, String> {
    if TEMPERATURE_RANGE.contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "temperature {value} is out of range (expected {}..={})",
            TEMPERATURE_RANGE.start(),
            TEMPERATURE_RANGE.end()
        ))
    }
}

/// Custom serde deserializer that rejects out-of-range temperature values at parse time.
fn deserialize_temperature<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: f64 = serde::Deserialize::deserialize(deserializer)?;
    validate_temperature(value).map_err(serde::de::Error::custom)
}

pub fn normalize_reasoning_effort(value: &str) -> std::result::Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "minimal" | "low" | "medium" | "high" | "xhigh" => Ok(normalized),
        _ => Err(format!(
            "reasoning_effort {value:?} is invalid (expected one of: minimal, low, medium, high, xhigh)"
        )),
    }
}

fn deserialize_reasoning_effort_opt<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    value
        .map(|raw| normalize_reasoning_effort(&raw).map_err(serde::de::Error::custom))
        .transpose()
}

fn default_max_depth() -> u32 {
    3
}

fn default_max_tool_iterations() -> usize {
    10
}

// ── Transcription ────────────────────────────────────────────────

fn default_transcription_api_url() -> String {
    "https://api.groq.com/openai/v1/audio/transcriptions".into()
}

fn default_transcription_model() -> String {
    "whisper-large-v3-turbo".into()
}

fn default_transcription_max_duration_secs() -> u64 {
    120
}

fn default_transcription_provider() -> String {
    "groq".into()
}

fn default_openai_stt_model() -> String {
    "whisper-1".into()
}

fn default_deepgram_stt_model() -> String {
    "nova-2".into()
}

fn default_google_stt_language_code() -> String {
    "en-US".into()
}

/// Voice transcription configuration with multi-provider support.
///
/// The top-level `api_url`, `model`, and `api_key` fields remain for backward
/// compatibility with existing Groq-based configurations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptionConfig {
    /// Enable voice transcription for channels that support it.
    #[serde(default)]
    pub enabled: bool,
    /// Default STT provider: "groq", "openai", "deepgram", "assemblyai", "google".
    #[serde(default = "default_transcription_provider")]
    pub default_provider: String,
    /// API key used for transcription requests (Groq provider).
    ///
    /// If unset, runtime falls back to `GROQ_API_KEY` for backward compatibility.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Whisper API endpoint URL (Groq provider).
    #[serde(default = "default_transcription_api_url")]
    pub api_url: String,
    /// Whisper model name (Groq provider).
    #[serde(default = "default_transcription_model")]
    pub model: String,
    /// Optional language hint (ISO-639-1, e.g. "en", "ru") for Groq provider.
    #[serde(default)]
    pub language: Option<String>,
    /// Optional initial prompt to bias transcription toward expected vocabulary
    /// (proper nouns, technical terms, etc.). Sent as the `prompt` field in the
    /// Whisper API request.
    #[serde(default)]
    pub initial_prompt: Option<String>,
    /// Maximum voice duration in seconds (messages longer than this are skipped).
    #[serde(default = "default_transcription_max_duration_secs")]
    pub max_duration_secs: u64,
    /// OpenAI Whisper STT provider configuration.
    #[serde(default)]
    pub openai: Option<OpenAiSttConfig>,
    /// Deepgram STT provider configuration.
    #[serde(default)]
    pub deepgram: Option<DeepgramSttConfig>,
    /// AssemblyAI STT provider configuration.
    #[serde(default)]
    pub assemblyai: Option<AssemblyAiSttConfig>,
    /// Google Cloud Speech-to-Text provider configuration.
    #[serde(default)]
    pub google: Option<GoogleSttConfig>,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_provider: default_transcription_provider(),
            api_key: None,
            api_url: default_transcription_api_url(),
            model: default_transcription_model(),
            language: None,
            initial_prompt: None,
            max_duration_secs: default_transcription_max_duration_secs(),
            openai: None,
            deepgram: None,
            assemblyai: None,
            google: None,
        }
    }
}

// ── MCP ─────────────────────────────────────────────────────────

/// Transport type for MCP server connections.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Spawn a local process and communicate over stdin/stdout.
    #[default]
    Stdio,
    /// Connect via HTTP POST.
    Http,
    /// Connect via HTTP + Server-Sent Events.
    Sse,
}

/// Configuration for a single external MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct McpServerConfig {
    /// Display name used as a tool prefix (`<server>__<tool>`).
    pub name: String,
    /// Transport type (default: stdio).
    #[serde(default)]
    pub transport: McpTransport,
    /// URL for HTTP/SSE transports.
    #[serde(default)]
    pub url: Option<String>,
    /// Executable to spawn for stdio transport.
    #[serde(default)]
    pub command: String,
    /// Command arguments for stdio transport.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional environment variables for stdio transport.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional HTTP headers for HTTP/SSE transports.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Optional per-call timeout in seconds (hard capped in validation).
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,
}

/// External MCP client configuration (`[mcp]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpConfig {
    /// Enable MCP tool loading.
    #[serde(default)]
    pub enabled: bool,
    /// Load MCP tool schemas on-demand via `tool_search` instead of eagerly
    /// including them in the LLM context window. When `true` (the default),
    /// only tool names are listed in the system prompt; the LLM must call
    /// `tool_search` to fetch full schemas before invoking a deferred tool.
    #[serde(default = "default_deferred_loading")]
    pub deferred_loading: bool,
    /// Configured MCP servers.
    #[serde(default, alias = "mcpServers")]
    pub servers: Vec<McpServerConfig>,
}

fn default_deferred_loading() -> bool {
    true
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            deferred_loading: default_deferred_loading(),
            servers: Vec::new(),
        }
    }
}

// ── Nodes (Dynamic Node Discovery) ───────────────────────────────

/// Configuration for the dynamic node discovery system (`[nodes]`).
///
/// When enabled, external processes/devices can connect via WebSocket
/// at `/ws/nodes` and advertise their capabilities at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NodesConfig {
    /// Enable dynamic node discovery endpoint.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of concurrent node connections.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
    /// Optional bearer token for node authentication.
    #[serde(default)]
    pub auth_token: Option<String>,
}

fn default_max_nodes() -> usize {
    16
}

impl Default for NodesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_nodes: default_max_nodes(),
            auth_token: None,
        }
    }
}

// ── TTS (Text-to-Speech) ─────────────────────────────────────────

fn default_tts_provider() -> String {
    "openai".into()
}

fn default_tts_voice() -> String {
    "alloy".into()
}

fn default_tts_format() -> String {
    "mp3".into()
}

fn default_tts_max_text_length() -> usize {
    4096
}

fn default_openai_tts_model() -> String {
    "tts-1".into()
}

fn default_openai_tts_speed() -> f64 {
    1.0
}

fn default_elevenlabs_model_id() -> String {
    "eleven_monolingual_v1".into()
}

fn default_elevenlabs_stability() -> f64 {
    0.5
}

fn default_elevenlabs_similarity_boost() -> f64 {
    0.5
}

fn default_google_tts_language_code() -> String {
    "en-US".into()
}

fn default_edge_tts_binary_path() -> String {
    "edge-tts".into()
}

/// Text-to-Speech configuration (`[tts]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TtsConfig {
    /// Enable TTS synthesis.
    #[serde(default)]
    pub enabled: bool,
    /// Default TTS provider (`"openai"`, `"elevenlabs"`, `"google"`, `"edge"`).
    #[serde(default = "default_tts_provider")]
    pub default_provider: String,
    /// Default voice ID passed to the selected provider.
    #[serde(default = "default_tts_voice")]
    pub default_voice: String,
    /// Default audio output format (`"mp3"`, `"opus"`, `"wav"`).
    #[serde(default = "default_tts_format")]
    pub default_format: String,
    /// Maximum input text length in characters (default 4096).
    #[serde(default = "default_tts_max_text_length")]
    pub max_text_length: usize,
    /// OpenAI TTS provider configuration (`[tts.openai]`).
    #[serde(default)]
    pub openai: Option<OpenAiTtsConfig>,
    /// ElevenLabs TTS provider configuration (`[tts.elevenlabs]`).
    #[serde(default)]
    pub elevenlabs: Option<ElevenLabsTtsConfig>,
    /// Google Cloud TTS provider configuration (`[tts.google]`).
    #[serde(default)]
    pub google: Option<GoogleTtsConfig>,
    /// Edge TTS provider configuration (`[tts.edge]`).
    #[serde(default)]
    pub edge: Option<EdgeTtsConfig>,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_provider: default_tts_provider(),
            default_voice: default_tts_voice(),
            default_format: default_tts_format(),
            max_text_length: default_tts_max_text_length(),
            openai: None,
            elevenlabs: None,
            google: None,
            edge: None,
        }
    }
}

/// OpenAI TTS provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenAiTtsConfig {
    /// API key for OpenAI TTS.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model name (default `"tts-1"`).
    #[serde(default = "default_openai_tts_model")]
    pub model: String,
    /// Playback speed multiplier (default `1.0`).
    #[serde(default = "default_openai_tts_speed")]
    pub speed: f64,
}

/// ElevenLabs TTS provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ElevenLabsTtsConfig {
    /// API key for ElevenLabs.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model ID (default `"eleven_monolingual_v1"`).
    #[serde(default = "default_elevenlabs_model_id")]
    pub model_id: String,
    /// Voice stability (0.0-1.0, default `0.5`).
    #[serde(default = "default_elevenlabs_stability")]
    pub stability: f64,
    /// Similarity boost (0.0-1.0, default `0.5`).
    #[serde(default = "default_elevenlabs_similarity_boost")]
    pub similarity_boost: f64,
}

/// Google Cloud TTS provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GoogleTtsConfig {
    /// API key for Google Cloud TTS.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Language code (default `"en-US"`).
    #[serde(default = "default_google_tts_language_code")]
    pub language_code: String,
}

/// Edge TTS provider configuration (free, subprocess-based).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EdgeTtsConfig {
    /// Path to the `edge-tts` binary (default `"edge-tts"`).
    #[serde(default = "default_edge_tts_binary_path")]
    pub binary_path: String,
}

/// Determines when a `ToolFilterGroup` is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolFilterGroupMode {
    /// Tools in this group are always included in every turn.
    Always,
    /// Tools in this group are included only when the user message contains
    /// at least one of the configured `keywords` (case-insensitive substring match).
    #[default]
    Dynamic,
}

/// A named group of MCP tool patterns with an activation mode.
///
/// Each group lists glob patterns for MCP tool names (prefix `mcp_`) and an
/// optional set of keywords that trigger inclusion in `dynamic` mode.
/// Built-in (non-MCP) tools always pass through and are never affected by
/// `tool_filter_groups`.
///
/// # Example
/// ```toml
/// [[agent.tool_filter_groups]]
/// mode = "always"
/// tools = ["mcp_filesystem_*"]
/// keywords = []
///
/// [[agent.tool_filter_groups]]
/// mode = "dynamic"
/// tools = ["mcp_browser_*"]
/// keywords = ["browse", "website", "url", "search"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolFilterGroup {
    /// Activation mode: `"always"` or `"dynamic"`.
    #[serde(default)]
    pub mode: ToolFilterGroupMode,
    /// Glob patterns matching MCP tool names (single `*` wildcard supported).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Keywords that activate this group in `dynamic` mode (case-insensitive substring).
    /// Ignored when `mode = "always"`.
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// OpenAI Whisper STT provider configuration (`[transcription.openai]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenAiSttConfig {
    /// OpenAI API key for Whisper transcription.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Whisper model name (default: "whisper-1").
    #[serde(default = "default_openai_stt_model")]
    pub model: String,
}

/// Deepgram STT provider configuration (`[transcription.deepgram]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeepgramSttConfig {
    /// Deepgram API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Deepgram model name (default: "nova-2").
    #[serde(default = "default_deepgram_stt_model")]
    pub model: String,
}

/// AssemblyAI STT provider configuration (`[transcription.assemblyai]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssemblyAiSttConfig {
    /// AssemblyAI API key.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Google Cloud Speech-to-Text provider configuration (`[transcription.google]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GoogleSttConfig {
    /// Google Cloud API key.
    #[serde(default)]
    pub api_key: Option<String>,
    /// BCP-47 language code (default: "en-US").
    #[serde(default = "default_google_stt_language_code")]
    pub language_code: String,
}

/// Agent orchestration configuration (`[agent]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfig {
    /// When true: bootstrap_max_chars=6000, rag_chunk_limit=2. Use for 13B or smaller models.
    #[serde(default)]
    pub compact_context: bool,
    /// Maximum tool-call loop turns per user message. Default: `10`.
    /// Setting to `0` falls back to the safe default of `10`.
    #[serde(default = "default_agent_max_tool_iterations")]
    pub max_tool_iterations: usize,
    /// Maximum conversation history messages retained per session. Default: `50`.
    #[serde(default = "default_agent_max_history_messages")]
    pub max_history_messages: usize,
    /// Maximum estimated tokens for conversation history before compaction triggers.
    /// Uses ~4 chars/token heuristic. When this threshold is exceeded, older messages
    /// are summarized to preserve context while staying within budget. Default: `32000`.
    #[serde(default = "default_agent_max_context_tokens")]
    pub max_context_tokens: usize,
    /// Enable parallel tool execution within a single iteration. Default: `false`.
    #[serde(default)]
    pub parallel_tools: bool,
    /// Tool dispatch strategy (e.g. `"auto"`). Default: `"auto"`.
    #[serde(default = "default_agent_tool_dispatcher")]
    pub tool_dispatcher: String,
    /// Tools exempt from the within-turn duplicate-call dedup check. Default: `[]`.
    #[serde(default)]
    pub tool_call_dedup_exempt: Vec<String>,
    /// Per-turn MCP tool schema filtering groups.
    ///
    /// When non-empty, only MCP tools matched by an active group are included in the
    /// tool schema sent to the LLM for that turn. Built-in tools always pass through.
    /// Default: `[]` (no filtering — all tools included).
    #[serde(default)]
    pub tool_filter_groups: Vec<ToolFilterGroup>,
    /// Enable prompt caching for providers that support it (Anthropic). Default: `false`.
    /// When true, adds cache_control breakpoints to system prompt, tools, and long conversations.
    /// Requires a plan that supports prompt caching; disable for OAuth/free-tier tokens.
    #[serde(default)]
    pub prompt_caching: bool,
}

/// Provider-history compression policy (`[compression]` section).
///
/// Mirrors the operator-facing knobs used by Hermes-style context compressors
/// while leaving model/provider-specific limits to catalog/profile metadata.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ContextCompressionConfig {
    /// Enable provider-history compression. Default: `true`.
    #[serde(default = "default_compression_enabled")]
    pub enabled: bool,
    /// Compression triggers at `threshold * safe_input_context`. Default: `0.50`.
    #[serde(default = "default_compression_threshold")]
    pub threshold: f64,
    /// Tail token budget as a fraction of threshold tokens. Default: `0.20`.
    #[serde(default = "default_compression_target_ratio")]
    pub target_ratio: f64,
    /// Minimum recent messages always preserved. Default: `20`.
    #[serde(default = "default_compression_protect_last_n")]
    pub protect_last_n: usize,
    /// Initial messages always preserved. Default: `3`.
    #[serde(default = "default_compression_protect_first_n")]
    pub protect_first_n: usize,
    /// Summary token budget as a fraction of compressed content. Default: `0.20`.
    #[serde(default = "default_compression_summary_ratio")]
    pub summary_ratio: f64,
    /// Minimum summary budget in estimated tokens. Default: `2000`.
    #[serde(default = "default_compression_min_summary_tokens")]
    pub min_summary_tokens: usize,
    /// Maximum summary budget in estimated tokens. Default: `12000`.
    #[serde(default = "default_compression_max_summary_tokens")]
    pub max_summary_tokens: usize,
    /// Safety cap for source transcript sent to the summarizer. Default: `12000` chars.
    #[serde(default = "default_compression_max_source_chars")]
    pub max_source_chars: usize,
    /// Max chars stored in provider history for one compaction summary. Default: `2000`.
    #[serde(default = "default_compression_max_summary_chars")]
    pub max_summary_chars: usize,
    /// Persistent condensed artifact cache TTL. Default: `172800` seconds (2 days).
    #[serde(default = "default_compression_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    /// Max persistent condensed artifact cache entries. Default: `256`.
    #[serde(default = "default_compression_cache_max_entries")]
    pub cache_max_entries: usize,
}

fn default_compression_enabled() -> bool {
    true
}

fn default_compression_threshold() -> f64 {
    0.50
}

fn default_compression_target_ratio() -> f64 {
    0.20
}

fn default_compression_protect_last_n() -> usize {
    20
}

fn default_compression_protect_first_n() -> usize {
    3
}

fn default_compression_summary_ratio() -> f64 {
    0.20
}

fn default_compression_min_summary_tokens() -> usize {
    2_000
}

fn default_compression_max_summary_tokens() -> usize {
    12_000
}

fn default_compression_max_source_chars() -> usize {
    12_000
}

fn default_compression_max_summary_chars() -> usize {
    2_000
}

fn default_compression_cache_ttl_secs() -> u64 {
    2 * 24 * 60 * 60
}

fn default_compression_cache_max_entries() -> usize {
    256
}

impl Default for ContextCompressionConfig {
    fn default() -> Self {
        Self {
            enabled: default_compression_enabled(),
            threshold: default_compression_threshold(),
            target_ratio: default_compression_target_ratio(),
            protect_last_n: default_compression_protect_last_n(),
            protect_first_n: default_compression_protect_first_n(),
            summary_ratio: default_compression_summary_ratio(),
            min_summary_tokens: default_compression_min_summary_tokens(),
            max_summary_tokens: default_compression_max_summary_tokens(),
            max_source_chars: default_compression_max_source_chars(),
            max_summary_chars: default_compression_max_summary_chars(),
            cache_ttl_secs: default_compression_cache_ttl_secs(),
            cache_max_entries: default_compression_cache_max_entries(),
        }
    }
}

/// Partial compression policy override for a specific route/lane selector.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct ContextCompressionRouteOverrideConfig {
    /// Optional route hint selector, e.g. `"cheap"`.
    #[serde(default)]
    pub hint: Option<String>,
    /// Optional provider selector, e.g. `"deepseek"` or `"openrouter"`.
    #[serde(default)]
    pub provider: Option<String>,
    /// Optional model selector.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional capability-lane selector.
    #[serde(default)]
    pub lane: Option<CapabilityLane>,
    /// Enable/disable compression for the matched route.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Override compression trigger ratio.
    #[serde(default)]
    pub threshold: Option<f64>,
    /// Override retained tail ratio.
    #[serde(default)]
    pub target_ratio: Option<f64>,
    /// Override protected recent messages.
    #[serde(default)]
    pub protect_last_n: Option<usize>,
    /// Override protected initial messages.
    #[serde(default)]
    pub protect_first_n: Option<usize>,
    /// Override summarizer target ratio.
    #[serde(default)]
    pub summary_ratio: Option<f64>,
    /// Override minimum summary token budget.
    #[serde(default)]
    pub min_summary_tokens: Option<usize>,
    /// Override maximum summary token budget.
    #[serde(default)]
    pub max_summary_tokens: Option<usize>,
    /// Override source transcript char cap.
    #[serde(default)]
    pub max_source_chars: Option<usize>,
    /// Override stored summary char cap.
    #[serde(default)]
    pub max_summary_chars: Option<usize>,
    /// Override persistent cache TTL.
    #[serde(default)]
    pub cache_ttl_secs: Option<u64>,
    /// Override persistent cache entry cap.
    #[serde(default)]
    pub cache_max_entries: Option<usize>,
}

impl ContextCompressionConfig {
    pub fn apply_override(&self, override_config: &ContextCompressionRouteOverrideConfig) -> Self {
        Self {
            enabled: override_config.enabled.unwrap_or(self.enabled),
            threshold: override_config.threshold.unwrap_or(self.threshold),
            target_ratio: override_config.target_ratio.unwrap_or(self.target_ratio),
            protect_last_n: override_config
                .protect_last_n
                .unwrap_or(self.protect_last_n),
            protect_first_n: override_config
                .protect_first_n
                .unwrap_or(self.protect_first_n),
            summary_ratio: override_config.summary_ratio.unwrap_or(self.summary_ratio),
            min_summary_tokens: override_config
                .min_summary_tokens
                .unwrap_or(self.min_summary_tokens),
            max_summary_tokens: override_config
                .max_summary_tokens
                .unwrap_or(self.max_summary_tokens),
            max_source_chars: override_config
                .max_source_chars
                .unwrap_or(self.max_source_chars),
            max_summary_chars: override_config
                .max_summary_chars
                .unwrap_or(self.max_summary_chars),
            cache_ttl_secs: override_config
                .cache_ttl_secs
                .unwrap_or(self.cache_ttl_secs),
            cache_max_entries: override_config
                .cache_max_entries
                .unwrap_or(self.cache_max_entries),
        }
    }
}

fn default_agent_max_tool_iterations() -> usize {
    10
}

fn default_agent_max_history_messages() -> usize {
    50
}

fn default_agent_max_context_tokens() -> usize {
    32_000
}

fn default_agent_tool_dispatcher() -> String {
    "auto".into()
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            compact_context: false,
            max_tool_iterations: default_agent_max_tool_iterations(),
            max_history_messages: default_agent_max_history_messages(),
            max_context_tokens: default_agent_max_context_tokens(),
            parallel_tools: false,
            tool_dispatcher: default_agent_tool_dispatcher(),
            tool_call_dedup_exempt: Vec::new(),
            tool_filter_groups: Vec::new(),
            prompt_caching: false,
        }
    }
}

/// Skills loading configuration (`[skills]` section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillsPromptInjectionMode {
    /// Inline full skill instructions and tool metadata into the system prompt.
    #[default]
    Full,
    /// Inline only compact skill metadata (name/description/location) and load details on demand.
    Compact,
}

pub fn parse_skills_prompt_injection_mode(raw: &str) -> Option<SkillsPromptInjectionMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "full" => Some(SkillsPromptInjectionMode::Full),
        "compact" => Some(SkillsPromptInjectionMode::Compact),
        _ => None,
    }
}

/// Skills loading configuration (`[skills]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SkillsConfig {
    /// Enable loading and syncing the community open-skills repository.
    /// Default: `false` (opt-in).
    #[serde(default)]
    pub open_skills_enabled: bool,
    /// Optional path to a local open-skills repository.
    /// If unset, defaults to `$HOME/open-skills` when enabled.
    #[serde(default)]
    pub open_skills_dir: Option<String>,
    /// Controls how skills are injected into the system prompt.
    /// `full` preserves legacy behavior. `compact` keeps context small and loads skills on demand.
    #[serde(default)]
    pub prompt_injection_mode: SkillsPromptInjectionMode,
}

/// Multimodal (image) handling configuration (`[multimodal]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MultimodalConfig {
    /// Maximum number of image attachments accepted per request.
    #[serde(default = "default_multimodal_max_images")]
    pub max_images: usize,
    /// Maximum image payload size in MiB before base64 encoding.
    #[serde(default = "default_multimodal_max_image_size_mb")]
    pub max_image_size_mb: usize,
    /// Allow fetching remote image URLs (http/https). Disabled by default.
    #[serde(default)]
    pub allow_remote_fetch: bool,
}

fn default_multimodal_max_images() -> usize {
    4
}

fn default_multimodal_max_image_size_mb() -> usize {
    5
}

impl MultimodalConfig {
    /// Clamp configured values to safe runtime bounds.
    pub fn effective_limits(&self) -> (usize, usize) {
        let max_images = self.max_images.clamp(1, 16);
        let max_image_size_mb = self.max_image_size_mb.clamp(1, 20);
        (max_images, max_image_size_mb)
    }
}

impl Default for MultimodalConfig {
    fn default() -> Self {
        Self {
            max_images: default_multimodal_max_images(),
            max_image_size_mb: default_multimodal_max_image_size_mb(),
            allow_remote_fetch: false,
        }
    }
}

// ── Identity (AIEOS / OpenClaw format) ──────────────────────────

/// Identity format configuration (`[identity]` section).
///
/// Supports `"openclaw"` (default) or `"aieos"` identity documents.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IdentityConfig {
    /// Identity format: "openclaw" (default) or "aieos"
    #[serde(default = "default_identity_format")]
    pub format: String,
    /// Path to AIEOS JSON file (relative to workspace)
    #[serde(default)]
    pub aieos_path: Option<String>,
    /// Inline AIEOS JSON (alternative to file path)
    #[serde(default)]
    pub aieos_inline: Option<String>,
}

fn default_identity_format() -> String {
    "openclaw".into()
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            format: default_identity_format(),
            aieos_path: None,
            aieos_inline: None,
        }
    }
}

// ── Cost tracking and budget enforcement ───────────────────────────

/// Cost tracking and budget enforcement configuration (`[cost]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CostConfig {
    /// Enable cost tracking (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Daily spending limit in USD (default: 10.00)
    #[serde(default = "default_daily_limit")]
    pub daily_limit_usd: f64,

    /// Monthly spending limit in USD (default: 100.00)
    #[serde(default = "default_monthly_limit")]
    pub monthly_limit_usd: f64,

    /// Warn when spending reaches this percentage of limit (default: 80)
    #[serde(default = "default_warn_percent")]
    pub warn_at_percent: u8,

    /// Allow requests to exceed budget with --override flag (default: false)
    #[serde(default)]
    pub allow_override: bool,

    /// Per-model pricing (USD per 1M tokens)
    #[serde(default)]
    pub prices: std::collections::HashMap<String, ModelPricing>,
}

/// Per-model pricing entry (USD per 1M tokens).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelPricing {
    /// Input price per 1M tokens
    #[serde(default)]
    pub input: f64,

    /// Output price per 1M tokens
    #[serde(default)]
    pub output: f64,
}

fn default_daily_limit() -> f64 {
    10.0
}

fn default_monthly_limit() -> f64 {
    100.0
}

fn default_warn_percent() -> u8 {
    80
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_limit_usd: default_daily_limit(),
            monthly_limit_usd: default_monthly_limit(),
            warn_at_percent: default_warn_percent(),
            allow_override: false,
            prices: get_default_pricing(),
        }
    }
}

/// Default pricing for popular models (USD per 1M tokens)
fn get_default_pricing() -> std::collections::HashMap<String, ModelPricing> {
    super::model_catalog::default_pricing_table()
}

// ── Gateway security ─────────────────────────────────────────────

/// Gateway server configuration (`[gateway]` section).
///
/// Controls the HTTP gateway for webhook and pairing endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GatewayConfig {
    /// Gateway port (default: 42617)
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    /// Gateway host (default: 127.0.0.1)
    #[serde(default = "default_gateway_host")]
    pub host: String,
    /// Require pairing before accepting requests (default: true)
    #[serde(default = "default_true")]
    pub require_pairing: bool,
    /// Allow binding to non-localhost without a tunnel (default: false)
    #[serde(default)]
    pub allow_public_bind: bool,
    /// Paired bearer tokens (managed automatically, not user-edited)
    #[serde(default)]
    pub paired_tokens: Vec<String>,

    /// Max `/pair` requests per minute per client key.
    #[serde(default = "default_pair_rate_limit")]
    pub pair_rate_limit_per_minute: u32,

    /// Max `/webhook` requests per minute per client key.
    #[serde(default = "default_webhook_rate_limit")]
    pub webhook_rate_limit_per_minute: u32,

    /// Trust proxy-forwarded client IP headers (`X-Forwarded-For`, `X-Real-IP`).
    /// Disabled by default; enable only behind a trusted reverse proxy.
    #[serde(default)]
    pub trust_forwarded_headers: bool,

    /// Maximum distinct client keys tracked by gateway rate limiter maps.
    #[serde(default = "default_gateway_rate_limit_max_keys")]
    pub rate_limit_max_keys: usize,

    /// TTL for webhook idempotency keys.
    #[serde(default = "default_idempotency_ttl_secs")]
    pub idempotency_ttl_secs: u64,

    /// Maximum distinct idempotency keys retained in memory.
    #[serde(default = "default_gateway_idempotency_max_keys")]
    pub idempotency_max_keys: usize,

    /// IPC token metadata: maps token hash → agent identity.
    /// Tokens without an entry are treated as legacy human tokens (no IPC).
    #[serde(default)]
    pub token_metadata: HashMap<String, TokenMetadata>,

    /// UI agent provisioning configuration (Phase 3.8 Step 11).
    /// Disabled by default. Entire subtree immutable via PUT /api/config.
    #[serde(default)]
    pub ui_provisioning: UiProvisioningConfig,

    /// Additional CIDR ranges allowed to access admin endpoints (besides localhost).
    /// Example: `["100.64.0.0/10"]` for Tailscale peers.
    /// Parsed and validated at startup — invalid CIDRs prevent boot.
    /// Not changeable via `PUT /api/config`.
    #[serde(default)]
    pub admin_cidrs: Vec<String>,
}

/// UI agent provisioning configuration (broker-only).
///
/// Controls whether the broker dashboard can create agent configs and
/// install services. Disabled by default. The entire subtree is immutable
/// via `PUT /api/config` — can only be changed by local file edit + restart.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UiProvisioningConfig {
    /// Master switch. Requires local config edit + restart to enable.
    #[serde(default)]
    pub enabled: bool,

    /// Operation mode: "config_only" or "service_install".
    /// config_only: create dirs + write config. service_install: also install/start OS service.
    #[serde(default = "default_provisioning_mode")]
    pub mode: String,

    /// Root directory for agent instances. Default: ~/.synapseclaw/agents
    #[serde(default = "default_agents_root")]
    pub agents_root: String,

    /// Allow Phase 3.6 fleet blueprints (multi-agent creation).
    #[serde(default)]
    pub allow_blueprints: bool,
}

fn default_provisioning_mode() -> String {
    "config_only".into()
}

fn default_agents_root() -> String {
    "~/.synapseclaw/agents".into()
}

impl Default for UiProvisioningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: default_provisioning_mode(),
            agents_root: default_agents_root(),
            allow_blueprints: false,
        }
    }
}

fn default_gateway_port() -> u16 {
    42617
}

fn default_gateway_host() -> String {
    "127.0.0.1".into()
}

fn default_pair_rate_limit() -> u32 {
    10
}

fn default_webhook_rate_limit() -> u32 {
    60
}

fn default_idempotency_ttl_secs() -> u64 {
    300
}

fn default_gateway_rate_limit_max_keys() -> usize {
    10_000
}

fn default_gateway_idempotency_max_keys() -> usize {
    10_000
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: default_gateway_port(),
            host: default_gateway_host(),
            require_pairing: true,
            allow_public_bind: false,
            paired_tokens: Vec::new(),
            pair_rate_limit_per_minute: default_pair_rate_limit(),
            webhook_rate_limit_per_minute: default_webhook_rate_limit(),
            trust_forwarded_headers: false,
            rate_limit_max_keys: default_gateway_rate_limit_max_keys(),
            idempotency_ttl_secs: default_idempotency_ttl_secs(),
            idempotency_max_keys: default_gateway_idempotency_max_keys(),
            token_metadata: HashMap::new(),
            ui_provisioning: UiProvisioningConfig::default(),
            admin_cidrs: Vec::new(),
        }
    }
}

// ── Inter-agent IPC ─────────────────────────────────────────────

/// Inter-agent IPC configuration (`[agents_ipc]` section).
///
/// Enables broker-mediated communication between SynapseClaw agent instances
/// through the gateway HTTP API. Each agent authenticates with a bearer token
/// that the broker resolves to an `agent_id` and `trust_level`.
///
/// Disabled by default — existing single-agent setups are unaffected.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentsIpcConfig {
    /// Enable inter-agent IPC (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Broker gateway URL (default: "http://127.0.0.1:42617")
    #[serde(default = "default_broker_url")]
    pub broker_url: String,

    /// Bearer token for authenticating with the broker gateway (agent→broker direction).
    /// Obtained by pairing with the broker instance.
    #[serde(default)]
    pub broker_token: Option<String>,

    /// Bearer token the broker uses to access this agent's gateway (broker→agent direction).
    /// Auto-generated on first daemon start if not set. Used for WS chat proxy.
    #[serde(default)]
    pub proxy_token: Option<String>,

    /// This agent's gateway URL for broker proxy connections.
    /// Auto-detected from `gateway.host:gateway.port` if not set.
    #[serde(default)]
    pub gateway_url: Option<String>,

    /// Agent staleness threshold in seconds (default: 120).
    /// Agents not seen within this window are marked offline.
    #[serde(default = "default_staleness_secs")]
    pub staleness_secs: u64,

    /// Message TTL in seconds (default: 86400 = 24h). `None` = no expiry.
    #[serde(default = "default_message_ttl_secs")]
    pub message_ttl_secs: Option<u64>,

    /// Trust level for this agent instance (0–4, default: 3).
    /// Set by the broker admin during pairing, not self-claimed.
    #[serde(default = "default_trust_level")]
    pub trust_level: u8,

    /// Role label for this agent (default: "agent")
    #[serde(default = "default_ipc_role")]
    pub role: String,

    /// Canonical agent ID for this instance. Must match the agent_id in the
    /// broker's TokenMetadata (set during pairing via `POST /admin/paircode/new`).
    /// Used for Ed25519 message signing context (Phase 3B).
    /// If not set, falls back to SYNAPSECLAW_AGENT_ID env var or role.
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Max outbound messages per hour (default: 60)
    #[serde(default = "default_max_messages_per_hour")]
    pub max_messages_per_hour: u32,

    /// HTTP request timeout for IPC calls in seconds (default: 10)
    #[serde(default = "default_ipc_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// Allowlisted lateral text pairs for L3 agents.
    /// Each entry is `["from_agent", "to_agent"]`.
    #[serde(default)]
    pub lateral_text_pairs: Vec<[String; 2]>,

    /// Logical destination aliases visible to L4 agents.
    /// Maps alias → real agent_id. L4 agents see only alias names with masked
    /// metadata; the broker resolves aliases to real agent_ids transparently.
    /// Example: `{ "supervisor" = "opus", "escalation" = "sentinel" }`
    #[serde(default)]
    pub l4_destinations: HashMap<String, String>,

    /// PromptGuard configuration for IPC payload scanning.
    #[serde(default)]
    pub prompt_guard: IpcPromptGuardConfig,

    /// Max messages per lateral session before auto-escalation (default: 10).
    /// Only applies to same-level exchanges (L2-L2, L3-L3).
    #[serde(default = "default_session_max_exchanges")]
    pub session_max_exchanges: u32,

    /// Agent ID of the coordinator that receives session escalation notifications.
    /// Default: "opus".
    #[serde(default = "default_coordinator_agent")]
    pub coordinator_agent: String,

    /// Named workload profiles for ephemeral spawn (Phase 3A).
    /// Workloads can only narrow the execution boundary — they cannot grant
    /// tools or autonomy beyond what the trust level allows.
    ///
    /// ```toml
    /// [agents_ipc.workload_profiles.research]
    /// model = "provider/model-id"
    /// allowed_tools = ["web_search", "web_fetch", "memory_read"]
    /// ```
    #[serde(default)]
    pub workload_profiles: HashMap<String, super::workload::WorkloadProfile>,

    /// Enable push notifications for new IPC messages (default: true).
    /// When enabled, the broker pushes lightweight notifications to agent
    /// gateways so they fetch their inbox immediately. Polling remains as fallback.
    #[serde(default = "default_push_enabled")]
    pub push_enabled: bool,

    /// Max push delivery retries with exponential backoff (default: 5).
    #[serde(default = "default_push_max_retries")]
    pub push_max_retries: u32,

    /// Max consecutive push-triggered auto-process runs for the same peer
    /// before suppression (default: 3). Counter resets after `push_peer_cooldown_secs`.
    #[serde(default = "default_push_max_auto_processes")]
    pub push_max_auto_processes: u32,

    /// Cooldown in seconds before resetting the per-peer auto-process counter
    /// (default: 300 = 5 minutes).
    #[serde(default = "default_push_peer_cooldown_secs")]
    pub push_peer_cooldown_secs: u64,

    /// Message kinds that trigger automatic inbox processing on push.
    /// Other kinds are delivered but await polling or manual inbox check.
    /// Default: `["task", "query", "result"]`
    #[serde(default = "default_push_auto_process_kinds")]
    pub push_auto_process_kinds: Vec<String>,

    /// One-way dispatch mode (default: false). When enabled, subordinate agents
    /// (higher trust level number) cannot trigger auto-processing on superiors.
    /// Messages are still delivered to inbox but await poll/manual check.
    #[serde(default)]
    pub push_one_way: bool,

    /// Channel name to relay push-triggered agent output to (e.g. "telegram", "matrix").
    /// When set together with `push_relay_recipient`, the push inbox processor
    /// emits an `OutboundIntent` so IPC results reach the user's channel.
    /// Phase 4.0 first vertical slice.
    #[serde(default)]
    pub push_relay_channel: Option<String>,

    /// Platform-specific recipient for push relay (e.g. Telegram chat ID, Matrix room ID).
    #[serde(default)]
    pub push_relay_recipient: Option<String>,

    /// Per-agent inbox filtering configuration.
    /// Controls which IPC messages appear in the agent's inbox (read-side only).
    #[serde(default)]
    pub inbox_filter: InboxFilterConfig,
}

/// Per-agent inbox filtering configuration (AutoGen MessageFilterAgent pattern).
///
/// Applied read-side only — no changes to message storage.
/// When `default_per_source` is 0 and `allowed_kinds` is empty, no filtering occurs.
///
/// ```toml
/// [agents_ipc.inbox_filter]
/// default_per_source = 1
/// allowed_kinds = ["task", "query", "result"]
///
/// [agents_ipc.inbox_filter.per_source]
/// "marketing-lead" = 3
/// "broker" = 5
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct InboxFilterConfig {
    /// Max messages to show per source agent (0 = no limit, show all).
    pub default_per_source: usize,

    /// Per-source overrides. Key = agent_id, value = max messages from that source.
    pub per_source: HashMap<String, usize>,

    /// Only show messages of these kinds. Empty = all kinds allowed.
    pub allowed_kinds: Vec<String>,
}

// Default derived: default_per_source=0, per_source={}, allowed_kinds=[]

impl InboxFilterConfig {
    /// Returns true if any filtering rules are configured.
    pub fn is_active(&self) -> bool {
        self.default_per_source > 0 || !self.per_source.is_empty() || !self.allowed_kinds.is_empty()
    }

    /// Max messages allowed from the given source agent.
    /// Returns `None` if no per-source limit applies (show all).
    pub fn limit_for_source(&self, source: &str) -> Option<usize> {
        if let Some(&limit) = self.per_source.get(source) {
            Some(limit)
        } else if self.default_per_source > 0 {
            Some(self.default_per_source)
        } else {
            None
        }
    }

    /// Returns true if the given message kind is allowed by the filter.
    pub fn kind_allowed(&self, kind: &str) -> bool {
        self.allowed_kinds.is_empty() || self.allowed_kinds.iter().any(|k| k == kind)
    }
}

/// Pipeline engine configuration (`[pipelines]` section).
///
/// Enables deterministic multi-agent workflow execution.
/// Pipeline definitions are loaded from TOML files in the pipeline directory.
///
/// Disabled by default — existing single-agent setups are unaffected.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PipelineEngineConfig {
    /// Enable the pipeline engine (default: false).
    pub enabled: bool,

    /// Directory containing pipeline TOML files.
    /// Defaults to `{workspace_dir}/pipelines`.
    #[serde(default)]
    pub directory: Option<String>,

    /// Directory containing routing TOML file.
    /// Defaults to `{workspace_dir}/pipelines/routing.toml`.
    #[serde(default)]
    pub routing_file: Option<String>,

    /// Enable hot-reload of pipeline TOML files (default: true).
    #[serde(default = "default_true_val")]
    pub hot_reload: bool,

    /// Default fallback agent for message routing (default: agent's own ID).
    #[serde(default)]
    pub routing_fallback: Option<String>,

    /// Agent ID used by the pipeline runner for IPC dispatch.
    /// Defaults to the broker's own agent ID (trust=0, can send tasks to all agents).
    #[serde(default)]
    pub runner_agent_id: Option<String>,

    /// Default rate limit for tool calls per pipeline run (0 = unlimited).
    #[serde(default)]
    pub default_tool_rate_limit: u32,

    /// Tools that require human approval before execution.
    #[serde(default)]
    pub approval_required_tools: Vec<String>,
}

impl Default for PipelineEngineConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: None,
            routing_file: None,
            hot_reload: true,
            routing_fallback: None,
            runner_agent_id: None,
            default_tool_rate_limit: 0,
            approval_required_tools: vec![],
        }
    }
}

#[allow(dead_code)]
fn default_pipeline_runner_id() -> Option<String> {
    None // resolved at runtime to broker's own agent_id
}

fn default_true_val() -> bool {
    true
}

/// PromptGuard configuration for IPC message payload scanning.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct IpcPromptGuardConfig {
    /// Enable PromptGuard scanning on IPC messages (default: true when IPC is enabled).
    pub enabled: bool,

    /// Action when injection detected: "block" or "warn" (default: "block").
    /// "block" = reject message with 403. "warn" = allow but log suspicion.
    pub action: String,

    /// Sensitivity threshold 0.0-1.0 (default: 0.55).
    /// Blocking triggers when max_score > sensitivity (strict greater-than).
    /// Category scores: command_injection=0.6, tool_injection=0.7-0.8,
    /// jailbreak=0.85, role_confusion=0.9, secret_extraction=0.95, system_override=1.0.
    pub sensitivity: f64,

    /// Trust levels exempt from scanning (default: [0, 1]).
    pub exempt_levels: Vec<u8>,
}

impl Default for IpcPromptGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            action: "block".into(),
            sensitivity: 0.55,
            exempt_levels: vec![0, 1],
        }
    }
}

/// Secure transport configuration for inter-node communication (`[node_transport]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NodeTransportConfig {
    /// Enable the secure transport layer.
    #[serde(default = "default_node_transport_enabled")]
    pub enabled: bool,
    /// Shared secret for HMAC authentication between nodes.
    #[serde(default)]
    pub shared_secret: String,
    /// Maximum age of signed requests in seconds (replay protection).
    #[serde(default = "default_max_request_age")]
    pub max_request_age_secs: i64,
    /// Require HTTPS for all node communication.
    #[serde(default = "default_require_https")]
    pub require_https: bool,
    /// Allow specific node IPs/CIDRs.
    #[serde(default)]
    pub allowed_peers: Vec<String>,
    /// Path to TLS certificate file.
    #[serde(default)]
    pub tls_cert_path: Option<String>,
    /// Path to TLS private key file.
    #[serde(default)]
    pub tls_key_path: Option<String>,
    /// Require client certificates (mutual TLS).
    #[serde(default)]
    pub mutual_tls: bool,
    /// Maximum number of connections per peer.
    #[serde(default = "default_connection_pool_size")]
    pub connection_pool_size: usize,
}

fn default_node_transport_enabled() -> bool {
    true
}
fn default_max_request_age() -> i64 {
    300
}
fn default_require_https() -> bool {
    true
}
fn default_connection_pool_size() -> usize {
    4
}

impl Default for NodeTransportConfig {
    fn default() -> Self {
        Self {
            enabled: default_node_transport_enabled(),
            shared_secret: String::new(),
            max_request_age_secs: default_max_request_age(),
            require_https: default_require_https(),
            allowed_peers: Vec::new(),
            tls_cert_path: None,
            tls_key_path: None,
            mutual_tls: false,
            connection_pool_size: default_connection_pool_size(),
        }
    }
}

fn default_broker_url() -> String {
    "http://127.0.0.1:42617".into()
}

fn default_staleness_secs() -> u64 {
    120
}

fn default_message_ttl_secs() -> Option<u64> {
    Some(86400)
}

fn default_trust_level() -> u8 {
    3
}

fn default_ipc_role() -> String {
    "agent".into()
}

fn default_max_messages_per_hour() -> u32 {
    60
}

fn default_ipc_request_timeout_secs() -> u64 {
    10
}

fn default_session_max_exchanges() -> u32 {
    10
}

fn default_push_enabled() -> bool {
    true
}

fn default_push_max_retries() -> u32 {
    5
}

fn default_push_max_auto_processes() -> u32 {
    3
}

fn default_push_peer_cooldown_secs() -> u64 {
    300
}

fn default_push_auto_process_kinds() -> Vec<String> {
    vec![
        "task".into(),
        "query".into(),
        "result".into(),
        "done".into(),
        "report".into(),
    ]
}

fn default_coordinator_agent() -> String {
    "opus".into()
}

impl Default for AgentsIpcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            broker_url: default_broker_url(),
            broker_token: None,
            proxy_token: None,
            gateway_url: None,
            staleness_secs: default_staleness_secs(),
            message_ttl_secs: default_message_ttl_secs(),
            trust_level: default_trust_level(),
            role: default_ipc_role(),
            agent_id: None,
            max_messages_per_hour: default_max_messages_per_hour(),
            request_timeout_secs: default_ipc_request_timeout_secs(),
            lateral_text_pairs: Vec::new(),
            l4_destinations: HashMap::new(),
            prompt_guard: IpcPromptGuardConfig::default(),
            session_max_exchanges: default_session_max_exchanges(),
            coordinator_agent: default_coordinator_agent(),
            workload_profiles: HashMap::new(),
            push_enabled: default_push_enabled(),
            push_max_retries: default_push_max_retries(),
            push_max_auto_processes: default_push_max_auto_processes(),
            push_peer_cooldown_secs: default_push_peer_cooldown_secs(),
            push_auto_process_kinds: default_push_auto_process_kinds(),
            push_one_way: false,
            push_relay_channel: None,
            push_relay_recipient: None,
            inbox_filter: InboxFilterConfig::default(),
        }
    }
}

/// Metadata bound to a bearer token for IPC identity resolution.
///
/// Stored in `[gateway.token_metadata."<token_hash>"]`. Tokens without
/// an entry are treated as legacy human tokens (no IPC access).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TokenMetadata {
    /// Agent identifier (e.g. "opus", "sentinel", "kids")
    pub agent_id: String,

    /// Trust level (0–4). Determines ACL permissions in the broker.
    #[serde(default = "default_trust_level")]
    pub trust_level: u8,

    /// Role label (e.g. "coordinator", "monitor", "worker")
    #[serde(default = "default_ipc_role")]
    pub role: String,
}

impl TokenMetadata {
    /// Returns the effective trust level for IPC operations.
    pub fn effective_trust_level(&self) -> u8 {
        self.trust_level
    }

    /// Whether this token is eligible for IPC (has an agent_id).
    pub fn is_ipc_eligible(&self) -> bool {
        !self.agent_id.is_empty()
    }
}

// ── Composio (managed tool surface) ─────────────────────────────

/// Composio managed OAuth tools integration (`[composio]` section).
///
/// Provides access to 1000+ OAuth-connected tools via the Composio platform.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ComposioConfig {
    /// Enable Composio integration for 1000+ OAuth tools
    #[serde(default, alias = "enable")]
    pub enabled: bool,
    /// Composio API key (stored encrypted when secrets.encrypt = true)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Default entity ID for multi-user setups
    #[serde(default = "default_entity_id")]
    pub entity_id: String,
}

fn default_entity_id() -> String {
    "default".into()
}

impl Default for ComposioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            entity_id: default_entity_id(),
        }
    }
}

// ── Microsoft 365 (Graph API integration) ───────────────────────

/// Microsoft 365 integration via Microsoft Graph API (`[microsoft365]` section).
///
/// Provides access to Outlook mail, Teams messages, Calendar events,
/// OneDrive files, and SharePoint search.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct Microsoft365Config {
    /// Enable Microsoft 365 integration
    #[serde(default, alias = "enable")]
    pub enabled: bool,
    /// Azure AD tenant ID
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Azure AD application (client) ID
    #[serde(default)]
    pub client_id: Option<String>,
    /// Azure AD client secret (stored encrypted when secrets.encrypt = true)
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Authentication flow: "client_credentials" or "device_code"
    #[serde(default = "default_ms365_auth_flow")]
    pub auth_flow: String,
    /// OAuth scopes to request
    #[serde(default = "default_ms365_scopes")]
    pub scopes: Vec<String>,
    /// Encrypt the token cache file on disk
    #[serde(default = "default_true")]
    pub token_cache_encrypted: bool,
    /// User principal name or "me" (for delegated flows)
    #[serde(default)]
    pub user_id: Option<String>,
}

fn default_ms365_auth_flow() -> String {
    "client_credentials".to_string()
}

fn default_ms365_scopes() -> Vec<String> {
    vec!["https://graph.microsoft.com/.default".to_string()]
}

impl std::fmt::Debug for Microsoft365Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Microsoft365Config")
            .field("enabled", &self.enabled)
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "***"))
            .field("auth_flow", &self.auth_flow)
            .field("scopes", &self.scopes)
            .field("token_cache_encrypted", &self.token_cache_encrypted)
            .field("user_id", &self.user_id)
            .finish()
    }
}

impl Default for Microsoft365Config {
    fn default() -> Self {
        Self {
            enabled: false,
            tenant_id: None,
            client_id: None,
            client_secret: None,
            auth_flow: default_ms365_auth_flow(),
            scopes: default_ms365_scopes(),
            token_cache_encrypted: true,
            user_id: None,
        }
    }
}

// ── Secrets (encrypted credential store) ────────────────────────

/// Secrets encryption configuration (`[secrets]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretsConfig {
    /// Enable encryption for API keys and tokens in config.toml
    #[serde(default = "default_true")]
    pub encrypt: bool,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self { encrypt: true }
    }
}

// ── Browser (friendly-service browsing only) ───────────────────

/// Computer-use sidecar configuration (`[browser.computer_use]` section).
///
/// Delegates OS-level mouse, keyboard, and screenshot actions to a local sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserComputerUseConfig {
    /// Sidecar endpoint for computer-use actions (OS-level mouse/keyboard/screenshot)
    #[serde(default = "default_browser_computer_use_endpoint")]
    pub endpoint: String,
    /// Optional bearer token for computer-use sidecar
    #[serde(default)]
    pub api_key: Option<String>,
    /// Per-action request timeout in milliseconds
    #[serde(default = "default_browser_computer_use_timeout_ms")]
    pub timeout_ms: u64,
    /// Allow remote/public endpoint for computer-use sidecar (default: false)
    #[serde(default)]
    pub allow_remote_endpoint: bool,
    /// Optional window title/process allowlist forwarded to sidecar policy
    #[serde(default)]
    pub window_allowlist: Vec<String>,
    /// Optional X-axis boundary for coordinate-based actions
    #[serde(default)]
    pub max_coordinate_x: Option<i64>,
    /// Optional Y-axis boundary for coordinate-based actions
    #[serde(default)]
    pub max_coordinate_y: Option<i64>,
}

fn default_browser_computer_use_endpoint() -> String {
    "http://127.0.0.1:8787/v1/actions".into()
}

fn default_browser_computer_use_timeout_ms() -> u64 {
    15_000
}

impl Default for BrowserComputerUseConfig {
    fn default() -> Self {
        Self {
            endpoint: default_browser_computer_use_endpoint(),
            api_key: None,
            timeout_ms: default_browser_computer_use_timeout_ms(),
            allow_remote_endpoint: false,
            window_allowlist: Vec::new(),
            max_coordinate_x: None,
            max_coordinate_y: None,
        }
    }
}

/// Browser automation configuration (`[browser]` section).
///
/// Controls the `browser_open` tool and browser automation backends.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserConfig {
    /// Enable `browser_open` tool (opens URLs in the system browser without scraping)
    #[serde(default)]
    pub enabled: bool,
    /// Allowed domains for `browser_open` (exact or subdomain match)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Browser session name (for agent-browser automation)
    #[serde(default)]
    pub session_name: Option<String>,
    /// Browser automation backend: "agent_browser" | "rust_native" | "computer_use" | "auto"
    #[serde(default = "default_browser_backend")]
    pub backend: String,
    /// Headless mode for rust-native backend
    #[serde(default = "default_true")]
    pub native_headless: bool,
    /// WebDriver endpoint URL for rust-native backend (e.g. http://127.0.0.1:9515)
    #[serde(default = "default_browser_webdriver_url")]
    pub native_webdriver_url: String,
    /// Optional Chrome/Chromium executable path for rust-native backend
    #[serde(default)]
    pub native_chrome_path: Option<String>,
    /// Computer-use sidecar configuration
    #[serde(default)]
    pub computer_use: BrowserComputerUseConfig,
}

fn default_browser_backend() -> String {
    "agent_browser".into()
}

fn default_browser_webdriver_url() -> String {
    "http://127.0.0.1:9515".into()
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_domains: Vec::new(),
            session_name: None,
            backend: default_browser_backend(),
            native_headless: default_true(),
            native_webdriver_url: default_browser_webdriver_url(),
            native_chrome_path: None,
            computer_use: BrowserComputerUseConfig::default(),
        }
    }
}

// ── HTTP request tool ───────────────────────────────────────────

/// HTTP request tool configuration (`[http_request]` section).
///
/// Deny-by-default: if `allowed_domains` is empty, all HTTP requests are rejected.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HttpRequestConfig {
    /// Enable `http_request` tool for API interactions
    #[serde(default)]
    pub enabled: bool,
    /// Allowed domains for HTTP requests (exact or subdomain match)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Maximum response size in bytes (default: 1MB, 0 = unlimited)
    #[serde(default = "default_http_max_response_size")]
    pub max_response_size: usize,
    /// Request timeout in seconds (default: 30)
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
    /// Allow requests to private/LAN hosts (RFC 1918, loopback, link-local, .local).
    /// Default: false (deny private hosts for SSRF protection).
    #[serde(default)]
    pub allow_private_hosts: bool,
}

impl Default for HttpRequestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_domains: vec![],
            max_response_size: default_http_max_response_size(),
            timeout_secs: default_http_timeout_secs(),
            allow_private_hosts: false,
        }
    }
}

fn default_http_max_response_size() -> usize {
    1_000_000 // 1MB
}

fn default_http_timeout_secs() -> u64 {
    30
}

// ── Web fetch ────────────────────────────────────────────────────

/// Web fetch tool configuration (`[web_fetch]` section).
///
/// Fetches web pages and converts HTML to plain text for LLM consumption.
/// Domain filtering: `allowed_domains` controls which hosts are reachable (use `["*"]`
/// for all public hosts). `blocked_domains` takes priority over `allowed_domains`.
/// If `allowed_domains` is empty, all requests are rejected (deny-by-default).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchConfig {
    /// Enable `web_fetch` tool for fetching web page content
    #[serde(default)]
    pub enabled: bool,
    /// Allowed domains for web fetch (exact or subdomain match; `["*"]` = all public hosts)
    #[serde(default = "default_web_fetch_allowed_domains")]
    pub allowed_domains: Vec<String>,
    /// Blocked domains (exact or subdomain match; always takes priority over allowed_domains)
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    /// Maximum response size in bytes (default: 500KB, plain text is much smaller than raw HTML)
    #[serde(default = "default_web_fetch_max_response_size")]
    pub max_response_size: usize,
    /// Request timeout in seconds (default: 30)
    #[serde(default = "default_web_fetch_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_web_fetch_max_response_size() -> usize {
    500_000 // 500KB
}

fn default_web_fetch_timeout_secs() -> u64 {
    30
}

fn default_web_fetch_allowed_domains() -> Vec<String> {
    vec!["*".into()]
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_domains: vec!["*".into()],
            blocked_domains: vec![],
            max_response_size: default_web_fetch_max_response_size(),
            timeout_secs: default_web_fetch_timeout_secs(),
        }
    }
}

// ── Web search ───────────────────────────────────────────────────

/// Web search tool configuration (`[web_search]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchConfig {
    /// Enable `web_search_tool` for web searches
    #[serde(default)]
    pub enabled: bool,
    /// Search provider: "duckduckgo" (free, no API key) or "brave" (requires API key)
    #[serde(default = "default_web_search_provider")]
    pub provider: String,
    /// Brave Search API key (required if provider is "brave")
    #[serde(default)]
    pub brave_api_key: Option<String>,
    /// Tavily API key (required if provider is "tavily")
    #[serde(default)]
    pub tavily_api_key: Option<String>,
    /// Maximum results per search (1-10)
    #[serde(default = "default_web_search_max_results")]
    pub max_results: usize,
    /// Request timeout in seconds
    #[serde(default = "default_web_search_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_web_search_provider() -> String {
    "duckduckgo".into()
}

fn default_web_search_max_results() -> usize {
    5
}

fn default_web_search_timeout_secs() -> u64 {
    15
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_web_search_provider(),
            brave_api_key: None,
            tavily_api_key: None,
            max_results: default_web_search_max_results(),
            timeout_secs: default_web_search_timeout_secs(),
        }
    }
}

// ── Project Intelligence ────────────────────────────────────────

/// Project delivery intelligence configuration (`[project_intel]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectIntelConfig {
    /// Enable the project_intel tool. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Default report language (en, de, fr, it). Default: "en".
    #[serde(default = "default_project_intel_language")]
    pub default_language: String,
    /// Output directory for generated reports.
    #[serde(default = "default_project_intel_report_dir")]
    pub report_output_dir: String,
    /// Optional custom templates directory.
    #[serde(default)]
    pub templates_dir: Option<String>,
    /// Risk detection sensitivity: low, medium, high. Default: "medium".
    #[serde(default = "default_project_intel_risk_sensitivity")]
    pub risk_sensitivity: String,
    /// Include git log data in reports. Default: true.
    #[serde(default = "default_true")]
    pub include_git_data: bool,
    /// Include Jira data in reports. Default: false.
    #[serde(default)]
    pub include_jira_data: bool,
    /// Jira instance base URL (required if include_jira_data is true).
    #[serde(default)]
    pub jira_base_url: Option<String>,
}

fn default_project_intel_language() -> String {
    "en".into()
}

fn default_project_intel_report_dir() -> String {
    "~/.synapseclaw/project-reports".into()
}

fn default_project_intel_risk_sensitivity() -> String {
    "medium".into()
}

impl Default for ProjectIntelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_language: default_project_intel_language(),
            report_output_dir: default_project_intel_report_dir(),
            templates_dir: None,
            risk_sensitivity: default_project_intel_risk_sensitivity(),
            include_git_data: true,
            include_jira_data: false,
            jira_base_url: None,
        }
    }
}

// ── Backup ──────────────────────────────────────────────────────

/// Backup tool configuration (`[backup]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BackupConfig {
    /// Enable the `backup` tool.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum number of backups to keep (oldest are pruned).
    #[serde(default = "default_backup_max_keep")]
    pub max_keep: usize,
    /// Workspace subdirectories to include in backups.
    #[serde(default = "default_backup_include_dirs")]
    pub include_dirs: Vec<String>,
    /// Output directory for backup archives (relative to workspace root).
    #[serde(default = "default_backup_destination_dir")]
    pub destination_dir: String,
    /// Optional cron expression for scheduled automatic backups.
    #[serde(default)]
    pub schedule_cron: Option<String>,
    /// IANA timezone for `schedule_cron`.
    #[serde(default)]
    pub schedule_timezone: Option<String>,
    /// Compress backup archives.
    #[serde(default = "default_true")]
    pub compress: bool,
    /// Encrypt backup archives (requires a configured secret store key).
    #[serde(default)]
    pub encrypt: bool,
}

fn default_backup_max_keep() -> usize {
    10
}

fn default_backup_include_dirs() -> Vec<String> {
    vec![
        "config".into(),
        "memory".into(),
        "audit".into(),
        "knowledge".into(),
    ]
}

fn default_backup_destination_dir() -> String {
    "state/backups".into()
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_keep: default_backup_max_keep(),
            include_dirs: default_backup_include_dirs(),
            destination_dir: default_backup_destination_dir(),
            schedule_cron: None,
            schedule_timezone: None,
            compress: true,
            encrypt: false,
        }
    }
}

// ── Data Retention ──────────────────────────────────────────────

/// Data retention and purge configuration (`[data_retention]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DataRetentionConfig {
    /// Enable the `data_management` tool.
    #[serde(default)]
    pub enabled: bool,
    /// Days of data to retain before purge eligibility.
    #[serde(default = "default_retention_days")]
    pub retention_days: u64,
    /// Preview what would be deleted without actually removing anything.
    #[serde(default)]
    pub dry_run: bool,
    /// Limit retention enforcement to specific data categories (empty = all).
    #[serde(default)]
    pub categories: Vec<String>,
}

fn default_retention_days() -> u64 {
    90
}

impl Default for DataRetentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_days: default_retention_days(),
            dry_run: false,
            categories: Vec::new(),
        }
    }
}

// ── Google Workspace ─────────────────────────────────────────────

/// Google Workspace CLI (`gws`) tool configuration (`[google_workspace]` section).
///
/// ## Defaults
/// - `enabled`: `false` (tool is not registered unless explicitly opted-in).
/// - `allowed_services`: empty vector, which grants access to the full default
///   service set: `drive`, `sheets`, `gmail`, `calendar`, `docs`, `slides`,
///   `tasks`, `people`, `chat`, `classroom`, `forms`, `keep`, `meet`, `events`.
/// - `credentials_path`: `None` (uses default `gws` credential discovery).
/// - `default_account`: `None` (uses the `gws` active account).
/// - `rate_limit_per_minute`: `60`.
/// - `timeout_secs`: `30`.
/// - `audit_log`: `false`.
/// - `credentials_path`: `None` (uses default `gws` credential discovery).
/// - `default_account`: `None` (uses the `gws` active account).
/// - `rate_limit_per_minute`: `60`.
/// - `timeout_secs`: `30`.
/// - `audit_log`: `false`.
///
/// ## Compatibility
/// Configs that omit the `[google_workspace]` section entirely are treated as
/// `GoogleWorkspaceConfig::default()` (disabled, all defaults allowed). Adding
/// the section is purely opt-in and does not affect other config sections.
///
/// ## Rollback / Migration
/// To revert, remove the `[google_workspace]` section from the config file (or
/// set `enabled = false`). No data migration is required; the tool simply stops
/// being registered.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GoogleWorkspaceConfig {
    /// Enable the `google_workspace` tool. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Restrict which Google Workspace services the agent can access.
    ///
    /// When empty (the default), the full default service set is allowed (see
    /// struct-level docs). When non-empty, only the listed service IDs are
    /// permitted. Each entry must be non-empty, lowercase alphanumeric with
    /// optional underscores/hyphens, and unique.
    #[serde(default)]
    pub allowed_services: Vec<String>,
    /// Path to service account JSON or OAuth client credentials file.
    ///
    /// When `None`, the tool relies on the default `gws` credential discovery
    /// (`gws auth login`). Set this to point at a service-account key or an
    /// OAuth client-secrets JSON for headless / CI environments.
    #[serde(default)]
    pub credentials_path: Option<String>,
    /// Default Google account email to pass to `gws --account`.
    ///
    /// When `None`, the currently active `gws` account is used.
    #[serde(default)]
    pub default_account: Option<String>,
    /// Maximum number of `gws` API calls allowed per minute. Default: `60`.
    #[serde(default = "default_gws_rate_limit")]
    pub rate_limit_per_minute: u32,
    /// Command execution timeout in seconds. Default: `30`.
    #[serde(default = "default_gws_timeout_secs")]
    pub timeout_secs: u64,
    /// Enable audit logging of every `gws` invocation (service, resource,
    /// method, timestamp). Default: `false`.
    #[serde(default)]
    pub audit_log: bool,
}

fn default_gws_rate_limit() -> u32 {
    60
}

fn default_gws_timeout_secs() -> u64 {
    30
}

impl Default for GoogleWorkspaceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_services: Vec::new(),
            credentials_path: None,
            default_account: None,
            rate_limit_per_minute: default_gws_rate_limit(),
            timeout_secs: default_gws_timeout_secs(),
            audit_log: false,
        }
    }
}

// ── Knowledge ───────────────────────────────────────────────────

/// Knowledge graph configuration for capturing and reusing expertise.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeConfig {
    /// Enable the knowledge graph tool. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the knowledge graph SQLite database.
    #[serde(default = "default_knowledge_db_path")]
    pub db_path: String,
    /// Maximum number of knowledge nodes. Default: 100000.
    #[serde(default = "default_knowledge_max_nodes")]
    pub max_nodes: usize,
    /// Automatically capture knowledge from conversations. Default: false.
    #[serde(default)]
    pub auto_capture: bool,
    /// Proactively suggest relevant knowledge on queries. Default: true.
    #[serde(default = "default_true")]
    pub suggest_on_query: bool,
    /// Allow searching across workspaces (disabled by default for client data isolation).
    #[serde(default)]
    pub cross_workspace_search: bool,
}

fn default_knowledge_db_path() -> String {
    "~/.synapseclaw/knowledge.db".into()
}

fn default_knowledge_max_nodes() -> usize {
    100_000
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: default_knowledge_db_path(),
            max_nodes: default_knowledge_max_nodes(),
            auto_capture: false,
            suggest_on_query: true,
            cross_workspace_search: false,
        }
    }
}

// ── LinkedIn ────────────────────────────────────────────────────

/// LinkedIn integration configuration (`[linkedin]` section).
///
/// When enabled, the `linkedin` tool is registered in the agent tool surface.
/// Requires `LINKEDIN_*` credentials in the workspace `.env` file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinkedInConfig {
    /// Enable the LinkedIn tool.
    #[serde(default)]
    pub enabled: bool,

    /// LinkedIn REST API version header (YYYYMM format).
    #[serde(default = "default_linkedin_api_version")]
    pub api_version: String,

    /// Content strategy for automated posting.
    #[serde(default)]
    pub content: LinkedInContentConfig,

    /// Image generation for posts (`[linkedin.image]`).
    #[serde(default)]
    pub image: LinkedInImageConfig,
}

impl Default for LinkedInConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_version: default_linkedin_api_version(),
            content: LinkedInContentConfig::default(),
            image: LinkedInImageConfig::default(),
        }
    }
}

fn default_linkedin_api_version() -> String {
    "202602".to_string()
}

/// Content strategy configuration for LinkedIn auto-posting (`[linkedin.content]`).
///
/// The agent reads this via the `linkedin get_content_strategy` action to know
/// what feeds to check, which repos to highlight, and how to write posts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LinkedInContentConfig {
    /// RSS feed URLs to monitor for topic inspiration (titles only).
    #[serde(default)]
    pub rss_feeds: Vec<String>,

    /// GitHub usernames whose public activity to reference.
    #[serde(default)]
    pub github_users: Vec<String>,

    /// GitHub repositories to highlight (format: `owner/repo`).
    #[serde(default)]
    pub github_repos: Vec<String>,

    /// Topics of expertise and interest for post themes.
    #[serde(default)]
    pub topics: Vec<String>,

    /// Professional persona description (name, role, expertise).
    #[serde(default)]
    pub persona: String,

    /// Freeform posting instructions for the AI agent.
    #[serde(default)]
    pub instructions: String,
}

/// Image generation configuration for LinkedIn posts (`[linkedin.image]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinkedInImageConfig {
    /// Enable image generation for posts.
    #[serde(default)]
    pub enabled: bool,

    /// Provider priority order. Tried in sequence; first success wins.
    #[serde(default = "default_image_providers")]
    pub providers: Vec<String>,

    /// Generate a branded SVG text card when all AI providers fail.
    #[serde(default = "default_true")]
    pub fallback_card: bool,

    /// Accent color for the fallback card (CSS hex).
    #[serde(default = "default_card_accent_color")]
    pub card_accent_color: String,

    /// Temp directory for generated images, relative to workspace.
    #[serde(default = "default_image_temp_dir")]
    pub temp_dir: String,

    /// Stability AI provider settings.
    #[serde(default)]
    pub stability: ImageProviderStabilityConfig,

    /// Google Imagen (Vertex AI) provider settings.
    #[serde(default)]
    pub imagen: ImageProviderImagenConfig,

    /// OpenAI DALL-E provider settings.
    #[serde(default)]
    pub dalle: ImageProviderDalleConfig,

    /// Flux (fal.ai) provider settings.
    #[serde(default)]
    pub flux: ImageProviderFluxConfig,
}

fn default_image_providers() -> Vec<String> {
    vec![
        "stability".into(),
        "imagen".into(),
        "dalle".into(),
        "flux".into(),
    ]
}

fn default_card_accent_color() -> String {
    "#0A66C2".into()
}

fn default_image_temp_dir() -> String {
    "linkedin/images".into()
}

impl Default for LinkedInImageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            providers: default_image_providers(),
            fallback_card: true,
            card_accent_color: default_card_accent_color(),
            temp_dir: default_image_temp_dir(),
            stability: ImageProviderStabilityConfig::default(),
            imagen: ImageProviderImagenConfig::default(),
            dalle: ImageProviderDalleConfig::default(),
            flux: ImageProviderFluxConfig::default(),
        }
    }
}

/// Stability AI image generation settings (`[linkedin.image.stability]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageProviderStabilityConfig {
    /// Environment variable name holding the API key.
    #[serde(default = "default_stability_api_key_env")]
    pub api_key_env: String,
    /// Stability model identifier.
    #[serde(default = "default_stability_model")]
    pub model: String,
}

fn default_stability_api_key_env() -> String {
    "STABILITY_API_KEY".into()
}
fn default_stability_model() -> String {
    "stable-diffusion-xl-1024-v1-0".into()
}

impl Default for ImageProviderStabilityConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_stability_api_key_env(),
            model: default_stability_model(),
        }
    }
}

/// Google Imagen (Vertex AI) settings (`[linkedin.image.imagen]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageProviderImagenConfig {
    /// Environment variable name holding the API key.
    #[serde(default = "default_imagen_api_key_env")]
    pub api_key_env: String,
    /// Environment variable for the Google Cloud project ID.
    #[serde(default = "default_imagen_project_id_env")]
    pub project_id_env: String,
    /// Vertex AI region.
    #[serde(default = "default_imagen_region")]
    pub region: String,
}

fn default_imagen_api_key_env() -> String {
    "GOOGLE_VERTEX_API_KEY".into()
}
fn default_imagen_project_id_env() -> String {
    "GOOGLE_CLOUD_PROJECT".into()
}
fn default_imagen_region() -> String {
    "us-central1".into()
}

impl Default for ImageProviderImagenConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_imagen_api_key_env(),
            project_id_env: default_imagen_project_id_env(),
            region: default_imagen_region(),
        }
    }
}

/// OpenAI DALL-E settings (`[linkedin.image.dalle]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageProviderDalleConfig {
    /// Environment variable name holding the OpenAI API key.
    #[serde(default = "default_dalle_api_key_env")]
    pub api_key_env: String,
    /// DALL-E model identifier.
    #[serde(default = "default_dalle_model")]
    pub model: String,
    /// Image dimensions.
    #[serde(default = "default_dalle_size")]
    pub size: String,
}

fn default_dalle_api_key_env() -> String {
    "OPENAI_API_KEY".into()
}
fn default_dalle_model() -> String {
    "dall-e-3".into()
}
fn default_dalle_size() -> String {
    "1024x1024".into()
}

impl Default for ImageProviderDalleConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_dalle_api_key_env(),
            model: default_dalle_model(),
            size: default_dalle_size(),
        }
    }
}

/// Flux (fal.ai) image generation settings (`[linkedin.image.flux]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageProviderFluxConfig {
    /// Environment variable name holding the fal.ai API key.
    #[serde(default = "default_flux_api_key_env")]
    pub api_key_env: String,
    /// Flux model identifier.
    #[serde(default = "default_flux_model")]
    pub model: String,
}

fn default_flux_api_key_env() -> String {
    "FAL_API_KEY".into()
}
fn default_flux_model() -> String {
    "fal-ai/flux/schnell".into()
}

impl Default for ImageProviderFluxConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_flux_api_key_env(),
            model: default_flux_model(),
        }
    }
}

// ── Proxy ───────────────────────────────────────────────────────

/// Proxy application scope — determines which outbound traffic uses the proxy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProxyScope {
    /// Use system environment proxy variables only.
    Environment,
    /// Apply proxy to all SynapseClaw-managed HTTP traffic (default).
    #[default]
    Internal,
    /// Apply proxy only to explicitly listed service selectors.
    Services,
}

/// Proxy configuration for outbound HTTP/HTTPS/SOCKS5 traffic (`[proxy]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProxyConfig {
    /// Enable proxy support for selected scope.
    #[serde(default)]
    pub enabled: bool,
    /// Proxy URL for HTTP requests (supports http, https, socks5, socks5h).
    #[serde(default)]
    pub http_proxy: Option<String>,
    /// Proxy URL for HTTPS requests (supports http, https, socks5, socks5h).
    #[serde(default)]
    pub https_proxy: Option<String>,
    /// Fallback proxy URL for all schemes.
    #[serde(default)]
    pub all_proxy: Option<String>,
    /// No-proxy bypass list. Same format as NO_PROXY.
    #[serde(default)]
    pub no_proxy: Vec<String>,
    /// Proxy application scope.
    #[serde(default)]
    pub scope: ProxyScope,
    /// Service selectors used when scope = "services".
    #[serde(default)]
    pub services: Vec<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            http_proxy: None,
            https_proxy: None,
            all_proxy: None,
            no_proxy: Vec::new(),
            scope: ProxyScope::Internal,
            services: Vec::new(),
        }
    }
}

impl ProxyConfig {
    pub fn supported_service_keys() -> &'static [&'static str] {
        SUPPORTED_PROXY_SERVICE_KEYS
    }

    pub fn supported_service_selectors() -> &'static [&'static str] {
        SUPPORTED_PROXY_SERVICE_SELECTORS
    }

    pub fn has_any_proxy_url(&self) -> bool {
        normalize_proxy_url_option(self.http_proxy.as_deref()).is_some()
            || normalize_proxy_url_option(self.https_proxy.as_deref()).is_some()
            || normalize_proxy_url_option(self.all_proxy.as_deref()).is_some()
    }

    pub fn normalized_services(&self) -> Vec<String> {
        normalize_service_list(self.services.clone())
    }

    pub fn normalized_no_proxy(&self) -> Vec<String> {
        normalize_no_proxy_list(self.no_proxy.clone())
    }

    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("http_proxy", self.http_proxy.as_deref()),
            ("https_proxy", self.https_proxy.as_deref()),
            ("all_proxy", self.all_proxy.as_deref()),
        ] {
            if let Some(url) = normalize_proxy_url_option(value) {
                validate_proxy_url(field, &url)?;
            }
        }

        for selector in self.normalized_services() {
            if !is_supported_proxy_service_selector(&selector) {
                anyhow::bail!(
                    "Unsupported proxy service selector '{selector}'. Use tool `proxy_config` action `list_services` for valid values"
                );
            }
        }

        if self.enabled && !self.has_any_proxy_url() {
            anyhow::bail!(
                "Proxy is enabled but no proxy URL is configured. Set at least one of http_proxy, https_proxy, or all_proxy"
            );
        }

        if self.enabled
            && self.scope == ProxyScope::Services
            && self.normalized_services().is_empty()
        {
            anyhow::bail!(
                "proxy.scope='services' requires a non-empty proxy.services list when proxy is enabled"
            );
        }

        Ok(())
    }

    pub fn should_apply_to_service(&self, service_key: &str) -> bool {
        if !self.enabled {
            return false;
        }

        match self.scope {
            ProxyScope::Environment => false,
            ProxyScope::Internal => true,
            ProxyScope::Services => {
                let service_key = service_key.trim().to_ascii_lowercase();
                if service_key.is_empty() {
                    return false;
                }

                self.normalized_services()
                    .iter()
                    .any(|selector| service_selector_matches(selector, &service_key))
            }
        }
    }
}

pub fn normalize_proxy_url_option(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn normalize_no_proxy_list(values: Vec<String>) -> Vec<String> {
    normalize_comma_values(values)
}

pub fn normalize_service_list(values: Vec<String>) -> Vec<String> {
    let mut normalized = normalize_comma_values(values)
        .into_iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

fn normalize_comma_values(values: Vec<String>) -> Vec<String> {
    let mut output = Vec::new();
    for value in values {
        for part in value.split(',') {
            let normalized = part.trim();
            if normalized.is_empty() {
                continue;
            }
            output.push(normalized.to_string());
        }
    }
    output.sort_unstable();
    output.dedup();
    output
}

fn is_supported_proxy_service_selector(selector: &str) -> bool {
    if SUPPORTED_PROXY_SERVICE_KEYS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(selector))
    {
        return true;
    }

    SUPPORTED_PROXY_SERVICE_SELECTORS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(selector))
}

fn service_selector_matches(selector: &str, service_key: &str) -> bool {
    if selector == service_key {
        return true;
    }

    if let Some(prefix) = selector.strip_suffix(".*") {
        return service_key.starts_with(prefix)
            && service_key
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('.'));
    }

    false
}

pub const MCP_MAX_TOOL_TIMEOUT_SECS: u64 = 600;

fn validate_proxy_url(field: &str, raw_url: &str) -> Result<()> {
    let parsed = url::Url::parse(raw_url)
        .with_context(|| format!("Invalid {field} URL: '{raw_url}' is not a valid URL"))?;
    if parsed.host().is_none() {
        anyhow::bail!("Invalid {field} URL: host is required");
    }
    Ok(())
}

pub fn validate_mcp_config(config: &McpConfig) -> Result<()> {
    let mut seen_names = std::collections::HashSet::new();
    for (i, server) in config.servers.iter().enumerate() {
        let name = server.name.trim();
        if name.is_empty() {
            anyhow::bail!("mcp.servers[{i}].name must not be empty");
        }
        if !seen_names.insert(name.to_ascii_lowercase()) {
            anyhow::bail!("mcp.servers contains duplicate name: {name}");
        }

        if let Some(timeout) = server.tool_timeout_secs {
            if timeout == 0 {
                anyhow::bail!("mcp.servers[{i}].tool_timeout_secs must be greater than 0");
            }
            if timeout > MCP_MAX_TOOL_TIMEOUT_SECS {
                anyhow::bail!(
                    "mcp.servers[{i}].tool_timeout_secs exceeds max {MCP_MAX_TOOL_TIMEOUT_SECS}"
                );
            }
        }

        match server.transport {
            McpTransport::Stdio => {
                if server.command.trim().is_empty() {
                    anyhow::bail!(
                        "mcp.servers[{i}] with transport=stdio requires non-empty command"
                    );
                }
            }
            McpTransport::Http | McpTransport::Sse => {
                let url = server
                    .url
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "mcp.servers[{i}] with transport={} requires url",
                            match server.transport {
                                McpTransport::Http => "http",
                                McpTransport::Sse => "sse",
                                McpTransport::Stdio => "stdio",
                            }
                        )
                    })?;
                let parsed = url::Url::parse(url)
                    .with_context(|| format!("mcp.servers[{i}].url is not a valid URL"))?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    anyhow::bail!("mcp.servers[{i}].url must use http/https");
                }
            }
        }
    }
    Ok(())
}
pub fn parse_proxy_scope(raw: &str) -> Option<ProxyScope> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "environment" | "env" => Some(ProxyScope::Environment),
        "synapseclaw" | "internal" | "core" => Some(ProxyScope::Internal),
        "services" | "service" => Some(ProxyScope::Services),
        _ => None,
    }
}

pub fn parse_proxy_enabled(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}
// ── Memory ───────────────────────────────────────────────────

/// Persistent storage configuration (`[storage]` section).
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct StorageConfig {
    /// Storage provider settings (e.g. sqlite, postgres).
    #[serde(default)]
    pub provider: StorageProviderSection,
}

/// Wrapper for the storage provider configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct StorageProviderSection {
    /// Storage provider backend settings.
    #[serde(default)]
    pub config: StorageProviderConfig,
}

/// Storage provider backend configuration (e.g. postgres connection details).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StorageProviderConfig {
    /// Storage engine key (e.g. "postgres", "sqlite").
    #[serde(default)]
    pub provider: String,

    /// Connection URL for remote providers.
    /// Accepts legacy aliases: dbURL, database_url, databaseUrl.
    #[serde(
        default,
        alias = "dbURL",
        alias = "database_url",
        alias = "databaseUrl"
    )]
    pub db_url: Option<String>,

    /// Database schema for SQL backends.
    #[serde(default = "default_storage_schema")]
    pub schema: String,

    /// Table name for memory entries.
    #[serde(default = "default_storage_table")]
    pub table: String,

    /// Optional connection timeout in seconds for remote providers.
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
}

fn default_storage_schema() -> String {
    "public".into()
}

fn default_storage_table() -> String {
    "memories".into()
}

impl Default for StorageProviderConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            db_url: None,
            schema: default_storage_schema(),
            table: default_storage_table(),
            connect_timeout_secs: None,
        }
    }
}

/// Memory backend configuration (`[memory]` section).
///
/// Controls conversation memory storage, embeddings, hybrid search, response caching,
/// and memory snapshot/hydration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConfig {
    /// Memory backend: "surrealdb" (default) or "none".
    pub backend: String,
    /// Auto-save user conversation input to memory.
    pub auto_save: bool,

    // ── Embeddings ────────────────────────────────────────────
    /// Embedding provider: "none" | "openai" | "custom:URL"
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,
    /// Embedding model name (e.g. "text-embedding-3-small")
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Embedding vector dimensions
    #[serde(default = "default_embedding_dims")]
    pub embedding_dimensions: usize,

    // ── Search tuning ─────────────────────────────────────────
    /// Weight for vector similarity in hybrid search (0.0–1.0)
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f64,
    /// Weight for keyword BM25 in hybrid search (0.0–1.0)
    #[serde(default = "default_keyword_weight")]
    pub keyword_weight: f64,
    /// Minimum score for a memory to be included in context.
    #[serde(default = "default_min_relevance_score")]
    pub min_relevance_score: f64,

    // ── Response Cache ────────────────────────────────────────
    /// Enable LLM response caching to avoid paying for duplicate prompts
    #[serde(default)]
    pub response_cache_enabled: bool,
    /// TTL in minutes for cached responses (default: 60)
    #[serde(default = "default_response_cache_ttl")]
    pub response_cache_ttl_minutes: u32,
    /// Max number of cached responses before LRU eviction (default: 5000)
    #[serde(default = "default_response_cache_max")]
    pub response_cache_max_entries: usize,
    /// Max in-memory hot cache entries (default: 256)
    #[serde(default = "default_response_cache_hot_entries")]
    pub response_cache_hot_entries: usize,

    // ── Prompt Budget ────────────────────────────────────────
    /// Prompt budget for turn context assembly.
    #[serde(default)]
    pub prompt_budget: PromptBudgetConfig,
}

fn default_embedding_provider() -> String {
    "none".into()
}
fn default_embedding_model() -> String {
    "text-embedding-3-small".into()
}
fn default_embedding_dims() -> usize {
    1536
}
fn default_vector_weight() -> f64 {
    0.7
}
fn default_keyword_weight() -> f64 {
    0.3
}
fn default_min_relevance_score() -> f64 {
    0.4
}
fn default_response_cache_ttl() -> u32 {
    60
}
fn default_response_cache_max() -> usize {
    5_000
}

fn default_response_cache_hot_entries() -> usize {
    256
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: "surrealdb".into(),
            auto_save: true,
            embedding_provider: default_embedding_provider(),
            embedding_model: default_embedding_model(),
            embedding_dimensions: default_embedding_dims(),
            vector_weight: default_vector_weight(),
            keyword_weight: default_keyword_weight(),
            min_relevance_score: default_min_relevance_score(),
            response_cache_enabled: false,
            response_cache_ttl_minutes: default_response_cache_ttl(),
            response_cache_max_entries: default_response_cache_max(),
            response_cache_hot_entries: default_response_cache_hot_entries(),
            prompt_budget: PromptBudgetConfig::default(),
        }
    }
}

// ── Prompt Budget ────────────────────────────────────────────────

/// Prompt budget configuration for turn context assembly.
///
/// Controls how much memory context (recall, skills, entities) is
/// injected into each turn's prompt.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PromptBudgetConfig {
    /// Hard cap on total chars for all core memory blocks combined.
    #[serde(default = "default_core_blocks_total_max_chars")]
    pub core_blocks_total_max_chars: usize,
    /// Max recalled episodic memory entries per turn.
    #[serde(default = "default_recall_max_entries")]
    pub recall_max_entries: usize,
    /// Max nearby/echo entries surfaced around top recall.
    #[serde(default = "default_nearby_max_entries")]
    pub nearby_max_entries: usize,
    /// Max chars per recalled entry (truncated with ellipsis).
    #[serde(default = "default_recall_entry_max_chars")]
    pub recall_entry_max_chars: usize,
    /// Max total chars for all recalled entries combined.
    #[serde(default = "default_recall_total_max_chars")]
    pub recall_total_max_chars: usize,
    /// Max number of skills injected per turn.
    #[serde(default = "default_skills_max_count")]
    pub skills_max_count: usize,
    /// Max total chars for all skills combined.
    #[serde(default = "default_skills_total_max_chars")]
    pub skills_total_max_chars: usize,
    /// Max number of entities injected per turn.
    #[serde(default = "default_entities_max_count")]
    pub entities_max_count: usize,
    /// Max total chars for all entities combined.
    #[serde(default = "default_entities_total_max_chars")]
    pub entities_total_max_chars: usize,
    /// Hard cap on total enrichment chars (recall + skills + entities).
    #[serde(default = "default_enrichment_total_max_chars")]
    pub enrichment_total_max_chars: usize,
    /// Continuation turn policy: "core_only", "core_plus_recall", or "full".
    #[serde(default = "default_continuation_policy")]
    pub continuation_policy: String,
}

fn default_core_blocks_total_max_chars() -> usize {
    1_800
}
fn default_recall_max_entries() -> usize {
    5
}
fn default_nearby_max_entries() -> usize {
    2
}
fn default_recall_entry_max_chars() -> usize {
    800
}
fn default_recall_total_max_chars() -> usize {
    4_000
}
fn default_skills_max_count() -> usize {
    3
}
fn default_skills_total_max_chars() -> usize {
    2_000
}
fn default_entities_max_count() -> usize {
    3
}
fn default_entities_total_max_chars() -> usize {
    1_500
}
fn default_enrichment_total_max_chars() -> usize {
    8_000
}
fn default_continuation_policy() -> String {
    "core_plus_recall".into()
}

impl Default for PromptBudgetConfig {
    fn default() -> Self {
        Self {
            core_blocks_total_max_chars: default_core_blocks_total_max_chars(),
            recall_max_entries: default_recall_max_entries(),
            nearby_max_entries: default_nearby_max_entries(),
            recall_entry_max_chars: default_recall_entry_max_chars(),
            recall_total_max_chars: default_recall_total_max_chars(),
            skills_max_count: default_skills_max_count(),
            skills_total_max_chars: default_skills_total_max_chars(),
            entities_max_count: default_entities_max_count(),
            entities_total_max_chars: default_entities_total_max_chars(),
            enrichment_total_max_chars: default_enrichment_total_max_chars(),
            continuation_policy: default_continuation_policy(),
        }
    }
}

impl PromptBudgetConfig {
    /// Convert to domain `PromptBudget` value object.
    pub fn to_prompt_budget(&self) -> crate::application::services::turn_context::PromptBudget {
        crate::application::services::turn_context::PromptBudget {
            core_blocks_total_max_chars: self.core_blocks_total_max_chars,
            recall_max_entries: self.recall_max_entries,
            nearby_max_entries: self.nearby_max_entries,
            recall_entry_max_chars: self.recall_entry_max_chars,
            recall_total_max_chars: self.recall_total_max_chars,
            recall_min_relevance: 0.0, // set from MemoryConfig.min_relevance_score by caller
            skills_max_count: self.skills_max_count,
            skills_total_max_chars: self.skills_total_max_chars,
            entities_max_count: self.entities_max_count,
            entities_total_max_chars: self.entities_total_max_chars,
            enrichment_total_max_chars: self.enrichment_total_max_chars,
        }
    }

    /// Parse `continuation_policy` string into domain `ContinuationPolicy`.
    pub fn to_continuation_policy(
        &self,
    ) -> crate::application::services::turn_context::ContinuationPolicy {
        use crate::application::services::turn_context::ContinuationPolicy;
        match self.continuation_policy.as_str() {
            "core_only" => ContinuationPolicy::CoreOnly,
            "full" => ContinuationPolicy::Full,
            _ => ContinuationPolicy::CorePlusRecall {
                recall_max_entries: 2,
            },
        }
    }
}

// ── Observability ─────────────────────────────────────────────────

/// Observability backend configuration (`[observability]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ObservabilityConfig {
    /// "none" | "log" | "verbose" | "prometheus" | "otel"
    pub backend: String,

    /// OTLP endpoint (e.g. "http://localhost:4318"). Only used when backend = "otel".
    #[serde(default)]
    pub otel_endpoint: Option<String>,

    /// Service name reported to the OTel collector. Defaults to "synapseclaw".
    #[serde(default)]
    pub otel_service_name: Option<String>,

    /// Runtime trace storage mode: "none" | "rolling" | "full".
    /// Controls whether model replies and tool-call diagnostics are persisted.
    #[serde(default = "default_runtime_trace_mode")]
    pub runtime_trace_mode: String,

    /// Runtime trace file path. Relative paths are resolved under workspace_dir.
    #[serde(default = "default_runtime_trace_path")]
    pub runtime_trace_path: String,

    /// Maximum entries retained when runtime_trace_mode = "rolling".
    #[serde(default = "default_runtime_trace_max_entries")]
    pub runtime_trace_max_entries: usize,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            backend: "none".into(),
            otel_endpoint: None,
            otel_service_name: None,
            runtime_trace_mode: default_runtime_trace_mode(),
            runtime_trace_path: default_runtime_trace_path(),
            runtime_trace_max_entries: default_runtime_trace_max_entries(),
        }
    }
}

fn default_runtime_trace_mode() -> String {
    "none".to_string()
}

fn default_runtime_trace_path() -> String {
    "state/runtime-trace.jsonl".to_string()
}

fn default_runtime_trace_max_entries() -> usize {
    200
}

// ── Hooks ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HooksConfig {
    /// Enable lifecycle hook execution.
    ///
    /// Hooks run in-process with the same privileges as the main runtime.
    /// Keep enabled hook handlers narrowly scoped and auditable.
    pub enabled: bool,
    #[serde(default)]
    pub builtin: BuiltinHooksConfig,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            builtin: BuiltinHooksConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BuiltinHooksConfig {
    /// Enable the command-logger hook (logs tool calls for auditing).
    pub command_logger: bool,
    /// Configuration for the webhook-audit hook.
    ///
    /// When enabled, POSTs a JSON payload to `url` for every tool invocation
    /// that matches one of `tool_patterns`.
    #[serde(default)]
    pub webhook_audit: WebhookAuditConfig,
}

/// Configuration for the webhook-audit builtin hook.
///
/// Sends an HTTP POST with a JSON body to an external endpoint each time
/// a tool call matches one of the configured patterns. Useful for
/// centralised audit logging, SIEM ingestion, or compliance pipelines.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebhookAuditConfig {
    /// Enable the webhook-audit hook. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Target URL that will receive the audit POST requests.
    #[serde(default)]
    pub url: String,
    /// Glob patterns for tool names to audit (e.g. `["Bash", "Write"]`).
    /// An empty list means **no** tools are audited.
    #[serde(default)]
    pub tool_patterns: Vec<String>,
    /// Include tool call arguments in the audit payload. Default: `false`.
    ///
    /// Be mindful of sensitive data — arguments may contain secrets or PII.
    #[serde(default)]
    pub include_args: bool,
    /// Maximum size (in bytes) of serialised arguments included in a single
    /// audit payload. Arguments exceeding this limit are truncated.
    /// Default: `4096`.
    #[serde(default = "default_max_args_bytes")]
    pub max_args_bytes: u64,
}

fn default_max_args_bytes() -> u64 {
    4096
}

impl Default for WebhookAuditConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            tool_patterns: Vec::new(),
            include_args: false,
            max_args_bytes: default_max_args_bytes(),
        }
    }
}

// ── Autonomy / Security ──────────────────────────────────────────

/// Autonomy and security policy configuration (`[autonomy]` section).
///
/// Controls what the agent is allowed to do: shell commands, filesystem access,
/// risk approval gates, and per-policy budgets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AutonomyConfig {
    /// Autonomy level: `read_only`, `supervised` (default), or `full`.
    pub level: AutonomyLevel,
    /// Restrict absolute filesystem paths to workspace-relative references. Default: `true`.
    /// Resolved paths outside the workspace still require `allowed_roots`.
    pub workspace_only: bool,
    /// Allowlist of executable names permitted for shell execution.
    pub allowed_commands: Vec<String>,
    /// Explicit path denylist. Default includes system-critical paths and sensitive dotdirs.
    pub forbidden_paths: Vec<String>,
    /// Maximum actions allowed per hour per policy. Default: `100`.
    pub max_actions_per_hour: u32,
    /// Maximum cost per day in cents per policy. Default: `1000`.
    pub max_cost_per_day_cents: u32,

    /// Require explicit approval for medium-risk shell commands.
    #[serde(default = "default_true")]
    pub require_approval_for_medium_risk: bool,

    /// Block high-risk shell commands even if allowlisted.
    #[serde(default = "default_true")]
    pub block_high_risk_commands: bool,

    /// Additional environment variables allowed for shell tool subprocesses.
    ///
    /// These names are explicitly allowlisted and merged with the built-in safe
    /// baseline (`PATH`, `HOME`, etc.) after `env_clear()`.
    #[serde(default)]
    pub shell_env_passthrough: Vec<String>,

    /// Tools that never require approval (e.g. read-only tools).
    #[serde(default = "default_auto_approve")]
    pub auto_approve: Vec<String>,

    /// Tools that always require interactive approval, even after "Always".
    #[serde(default = "default_always_ask")]
    pub always_ask: Vec<String>,

    /// Extra directory roots the agent may read/write outside the workspace.
    /// Supports absolute, `~/...`, and workspace-relative entries.
    /// Resolved paths under any of these roots pass `is_resolved_path_allowed`.
    #[serde(default)]
    pub allowed_roots: Vec<String>,

    /// Tools to exclude from non-CLI channels (e.g. Telegram, Discord).
    ///
    /// When a tool is listed here, non-CLI channels will not expose it to the
    /// model in tool specs.
    #[serde(default)]
    pub non_cli_excluded_tools: Vec<String>,
}

fn default_auto_approve() -> Vec<String> {
    vec!["file_read".into(), "memory_recall".into()]
}

fn default_always_ask() -> Vec<String> {
    vec![]
}

pub fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self {
            level: AutonomyLevel::Supervised,
            workspace_only: true,
            allowed_commands: vec![
                "git".into(),
                "npm".into(),
                "cargo".into(),
                "ls".into(),
                "cat".into(),
                "grep".into(),
                "find".into(),
                "echo".into(),
                "pwd".into(),
                "wc".into(),
                "head".into(),
                "tail".into(),
                "date".into(),
            ],
            forbidden_paths: vec![
                "/etc".into(),
                "/root".into(),
                "/home".into(),
                "/usr".into(),
                "/bin".into(),
                "/sbin".into(),
                "/lib".into(),
                "/opt".into(),
                "/boot".into(),
                "/dev".into(),
                "/proc".into(),
                "/sys".into(),
                "/var".into(),
                "/tmp".into(),
                "~/.ssh".into(),
                "~/.gnupg".into(),
                "~/.aws".into(),
                "~/.config".into(),
            ],
            max_actions_per_hour: 20,
            max_cost_per_day_cents: 500,
            require_approval_for_medium_risk: true,
            block_high_risk_commands: true,
            shell_env_passthrough: vec![],
            auto_approve: default_auto_approve(),
            always_ask: default_always_ask(),
            allowed_roots: Vec::new(),
            non_cli_excluded_tools: Vec::new(),
        }
    }
}

// ── Runtime ──────────────────────────────────────────────────────

/// Runtime adapter configuration (`[runtime]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeConfig {
    /// Runtime kind (`native` | `docker`).
    #[serde(default = "default_runtime_kind")]
    pub kind: String,

    /// Docker runtime settings (used when `kind = "docker"`).
    #[serde(default)]
    pub docker: DockerRuntimeConfig,

    /// Global reasoning override for providers that expose explicit controls.
    /// - `None`: provider default behavior
    /// - `Some(true)`: request reasoning/thinking when supported
    /// - `Some(false)`: disable reasoning/thinking when supported
    #[serde(default)]
    pub reasoning_enabled: Option<bool>,
    /// Optional reasoning effort for providers that expose a level control.
    #[serde(default, deserialize_with = "deserialize_reasoning_effort_opt")]
    pub reasoning_effort: Option<String>,
}

/// Docker runtime configuration (`[runtime.docker]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DockerRuntimeConfig {
    /// Runtime image used to execute shell commands.
    #[serde(default = "default_docker_image")]
    pub image: String,

    /// Docker network mode (`none`, `bridge`, etc.).
    #[serde(default = "default_docker_network")]
    pub network: String,

    /// Optional memory limit in MB (`None` = no explicit limit).
    #[serde(default = "default_docker_memory_limit_mb")]
    pub memory_limit_mb: Option<u64>,

    /// Optional CPU limit (`None` = no explicit limit).
    #[serde(default = "default_docker_cpu_limit")]
    pub cpu_limit: Option<f64>,

    /// Mount root filesystem as read-only.
    #[serde(default = "default_true")]
    pub read_only_rootfs: bool,

    /// Mount configured workspace into `/workspace`.
    #[serde(default = "default_true")]
    pub mount_workspace: bool,

    /// Optional workspace root allowlist for Docker mount validation.
    #[serde(default)]
    pub allowed_workspace_roots: Vec<String>,
}

fn default_runtime_kind() -> String {
    "native".into()
}

fn default_docker_image() -> String {
    "alpine:3.20".into()
}

fn default_docker_network() -> String {
    "none".into()
}

fn default_docker_memory_limit_mb() -> Option<u64> {
    Some(512)
}

fn default_docker_cpu_limit() -> Option<f64> {
    Some(1.0)
}

impl Default for DockerRuntimeConfig {
    fn default() -> Self {
        Self {
            image: default_docker_image(),
            network: default_docker_network(),
            memory_limit_mb: default_docker_memory_limit_mb(),
            cpu_limit: default_docker_cpu_limit(),
            read_only_rootfs: true,
            mount_workspace: true,
            allowed_workspace_roots: Vec::new(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            kind: default_runtime_kind(),
            docker: DockerRuntimeConfig::default(),
            reasoning_enabled: None,
            reasoning_effort: None,
        }
    }
}

// ── Reliability / supervision ────────────────────────────────────

/// Reliability and supervision configuration (`[reliability]` section).
///
/// Controls provider retries, fallback chains, API key rotation, and channel restart backoff.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReliabilityConfig {
    /// Retries per provider before failing over.
    #[serde(default = "default_provider_retries")]
    pub provider_retries: u32,
    /// Base backoff (ms) for provider retry delay.
    #[serde(default = "default_provider_backoff_ms")]
    pub provider_backoff_ms: u64,
    /// Fallback provider chain (e.g. `["anthropic", "openai"]`).
    #[serde(default)]
    pub fallback_providers: Vec<String>,
    /// Additional API keys for round-robin rotation on rate-limit (429) errors.
    /// The primary `api_key` is always tried first; these are extras.
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// Per-model fallback chains. When a model fails, try these alternatives in order.
    /// Example: `{ "primary-model-id" = ["fallback-model-id", "second-fallback-model-id"] }`
    #[serde(default)]
    pub model_fallbacks: std::collections::HashMap<String, Vec<String>>,
    /// Initial backoff for channel/daemon restarts.
    #[serde(default = "default_channel_backoff_secs")]
    pub channel_initial_backoff_secs: u64,
    /// Max backoff for channel/daemon restarts.
    #[serde(default = "default_channel_backoff_max_secs")]
    pub channel_max_backoff_secs: u64,
    /// Scheduler polling cadence in seconds.
    #[serde(default = "default_scheduler_poll_secs")]
    pub scheduler_poll_secs: u64,
    /// Max retries for cron job execution attempts.
    #[serde(default = "default_scheduler_retries")]
    pub scheduler_retries: u32,
}

fn default_provider_retries() -> u32 {
    2
}

fn default_provider_backoff_ms() -> u64 {
    500
}

fn default_channel_backoff_secs() -> u64 {
    2
}

fn default_channel_backoff_max_secs() -> u64 {
    60
}

fn default_scheduler_poll_secs() -> u64 {
    15
}

fn default_scheduler_retries() -> u32 {
    2
}

impl Default for ReliabilityConfig {
    fn default() -> Self {
        Self {
            provider_retries: default_provider_retries(),
            provider_backoff_ms: default_provider_backoff_ms(),
            fallback_providers: Vec::new(),
            api_keys: Vec::new(),
            model_fallbacks: std::collections::HashMap::new(),
            channel_initial_backoff_secs: default_channel_backoff_secs(),
            channel_max_backoff_secs: default_channel_backoff_max_secs(),
            scheduler_poll_secs: default_scheduler_poll_secs(),
            scheduler_retries: default_scheduler_retries(),
        }
    }
}

// ── Scheduler ────────────────────────────────────────────────────

/// Scheduler configuration for periodic task execution (`[scheduler]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchedulerConfig {
    /// Enable the built-in scheduler loop.
    #[serde(default = "default_scheduler_enabled")]
    pub enabled: bool,
    /// Maximum number of persisted scheduled tasks.
    #[serde(default = "default_scheduler_max_tasks")]
    pub max_tasks: usize,
    /// Maximum tasks executed per scheduler polling cycle.
    #[serde(default = "default_scheduler_max_concurrent")]
    pub max_concurrent: usize,
}

fn default_scheduler_enabled() -> bool {
    true
}

fn default_scheduler_max_tasks() -> usize {
    64
}

fn default_scheduler_max_concurrent() -> usize {
    4
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: default_scheduler_enabled(),
            max_tasks: default_scheduler_max_tasks(),
            max_concurrent: default_scheduler_max_concurrent(),
        }
    }
}

// ── Model routing ────────────────────────────────────────────────

/// Explicit summary model configuration (`[summary]` section).
///
/// When `provider` is set, the summary path creates its own provider instance
/// instead of reusing the default. This allows using a cheaper/smaller route for summaries
/// while the default provider is DashScope.
///
/// ```toml
/// [summary]
/// provider = "summary-provider"
/// model = "summary-model-id"
/// temperature = 0.3
/// api_key_env = "SUMMARY_PROVIDER_API_KEY"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummaryConfig {
    /// Provider name for summaries. When empty, uses default provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Model name for summaries. When empty, falls back to `summary_model` or `default_model`.
    #[serde(default)]
    pub model: Option<String>,
    /// Temperature for summary generation. Default: 0.3.
    #[serde(default = "default_summary_temperature")]
    pub temperature: f64,
    /// Environment variable name to read API key from (e.g. "ANTHROPIC_API_KEY").
    /// When set, reads the key from this env var instead of config.api_key.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

fn default_summary_temperature() -> f64 {
    0.3
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            temperature: default_summary_temperature(),
            api_key_env: None,
        }
    }
}

/// Explicit capability lanes for route selection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityLane {
    Reasoning,
    CheapReasoning,
    Embedding,
    ImageGeneration,
    AudioGeneration,
    VideoGeneration,
    MusicGeneration,
    MultimodalUnderstanding,
}

impl CapabilityLane {
    pub const ALL: [CapabilityLane; 8] = [
        CapabilityLane::Reasoning,
        CapabilityLane::CheapReasoning,
        CapabilityLane::Embedding,
        CapabilityLane::ImageGeneration,
        CapabilityLane::AudioGeneration,
        CapabilityLane::VideoGeneration,
        CapabilityLane::MusicGeneration,
        CapabilityLane::MultimodalUnderstanding,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CapabilityLane::Reasoning => "reasoning",
            CapabilityLane::CheapReasoning => "cheap_reasoning",
            CapabilityLane::Embedding => "embedding",
            CapabilityLane::ImageGeneration => "image_generation",
            CapabilityLane::AudioGeneration => "audio_generation",
            CapabilityLane::VideoGeneration => "video_generation",
            CapabilityLane::MusicGeneration => "music_generation",
            CapabilityLane::MultimodalUnderstanding => "multimodal_understanding",
        }
    }
}

impl std::str::FromStr for CapabilityLane {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let normalized = value.trim().trim_matches('`').replace('-', "_");
        CapabilityLane::ALL
            .into_iter()
            .find(|lane| lane.as_str().eq_ignore_ascii_case(&normalized))
            .ok_or(())
    }
}

/// Explicit model features used for lane routing and candidate selection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelFeature {
    ToolCalling,
    Vision,
    ImageGeneration,
    AudioGeneration,
    VideoGeneration,
    MusicGeneration,
    Embedding,
    MultimodalUnderstanding,
    ServerContinuation,
    PromptCaching,
}

/// Optional candidate profile metadata.
///
/// All fields are best-effort. When omitted, runtime may try to resolve
/// provider/model metadata automatically from cached provider catalogs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub struct ModelCandidateProfileConfig {
    /// Manual override for maximum context window tokens.
    #[serde(default)]
    pub context_window_tokens: Option<usize>,
    /// Manual override for maximum output tokens.
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
    /// Explicit feature set. When non-empty, it overrides auto-detected
    /// feature metadata for this candidate.
    #[serde(default)]
    pub features: Vec<ModelFeature>,
}

/// One provider:model candidate within a capability lane.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub struct ModelLaneCandidateConfig {
    /// Provider to route to (must match a known provider name).
    pub provider: String,
    /// Model id to use with that provider.
    pub model: String,
    /// Optional API key override for this candidate.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional env var for the API key override.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Optional embedding dimension override for embedding candidates.
    #[serde(default)]
    pub dimensions: Option<usize>,
    /// Optional manual profile overrides for this candidate.
    #[serde(default)]
    pub profile: ModelCandidateProfileConfig,
}

/// Ordered candidates for a single capability lane.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ModelLaneConfig {
    /// Capability lane this list serves.
    pub lane: CapabilityLane,
    /// Ordered provider:model candidates for the lane.
    #[serde(default)]
    pub candidates: Vec<ModelLaneCandidateConfig>,
}

/// Catalog alias from a task hint to a specific provider + model.
///
/// ```toml
/// [[route_aliases]]
/// hint = "reasoning"
/// provider = "example-provider"
/// model = "example-reasoning-model"
///
/// [[route_aliases]]
/// hint = "fast"
/// provider = "example-provider"
/// model = "example-fast-model"
/// ```
///
/// Runtime routing should prefer `[[model_lanes]]`; this shape remains for
/// explicit catalog aliases and provider-router dispatch compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelRouteConfig {
    /// Alias name, such as a capability selector or operator-defined shortcut.
    pub hint: String,
    /// Optional capability lane resolved by runtime/domain services.
    #[serde(default)]
    pub capability: Option<CapabilityLane>,
    /// Provider to route to (must match a known provider name)
    pub provider: String,
    /// Model to use with that provider
    pub model: String,
    /// Optional API key override for this route's provider
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional manual profile overrides for this route.
    #[serde(default)]
    pub profile: ModelCandidateProfileConfig,
}

// ── Embedding routing ───────────────────────────────────────────

/// Route an embedding hint to a specific provider + model.
///
/// ```toml
/// [[embedding_routes]]
/// hint = "semantic"
/// provider = "openai"
/// model = "text-embedding-3-small"
/// dimensions = 1536
///
/// [memory]
/// embedding_model = "hint:semantic"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingRouteConfig {
    /// Route hint name (e.g. "semantic", "archive", "faq")
    pub hint: String,
    /// Optional capability lane resolved by runtime/domain services.
    #[serde(default)]
    pub capability: Option<CapabilityLane>,
    /// Embedding provider (`none`, `openai`, or `custom:<url>`)
    pub provider: String,
    /// Embedding model to use with that provider
    pub model: String,
    /// Optional embedding dimension override for this route
    #[serde(default)]
    pub dimensions: Option<usize>,
    /// Optional API key override for this route's provider
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional manual profile overrides for this route.
    #[serde(default)]
    pub profile: ModelCandidateProfileConfig,
}

// ── Query Classification ─────────────────────────────────────────

// Canonical definitions live in synapse_domain; re-exported here for backward compat.
pub use crate::domain::query_classification::{ClassificationRule, QueryClassificationConfig};

// ── Heartbeat ────────────────────────────────────────────────────

/// Heartbeat configuration for periodic health pings (`[heartbeat]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HeartbeatConfig {
    /// Enable periodic heartbeat pings. Default: `false`.
    pub enabled: bool,
    /// Interval in minutes between heartbeat pings. Default: `30`.
    pub interval_minutes: u32,
    /// Enable two-phase heartbeat: Phase 1 asks LLM whether to run, Phase 2
    /// executes only when the LLM decides there is work to do. Saves API cost
    /// during quiet periods. Default: `true`.
    #[serde(default = "default_two_phase")]
    pub two_phase: bool,
    /// Optional fallback task text when `HEARTBEAT.md` has no task entries.
    #[serde(default)]
    pub message: Option<String>,
    /// Optional delivery channel for heartbeat output (for example: `telegram`).
    /// When omitted, auto-selects the first configured channel.
    #[serde(default, alias = "channel")]
    pub target: Option<String>,
    /// Optional delivery recipient/chat identifier (required when `target` is
    /// explicitly set).
    #[serde(default, alias = "recipient")]
    pub to: Option<String>,
    /// Enable adaptive intervals that back off on failures and speed up for
    /// high-priority tasks. Default: `false`.
    #[serde(default)]
    pub adaptive: bool,
    /// Minimum interval in minutes when adaptive mode is enabled. Default: `5`.
    #[serde(default = "default_heartbeat_min_interval")]
    pub min_interval_minutes: u32,
    /// Maximum interval in minutes when adaptive mode backs off. Default: `120`.
    #[serde(default = "default_heartbeat_max_interval")]
    pub max_interval_minutes: u32,
    /// Dead-man's switch timeout in minutes. If the heartbeat has not ticked
    /// within this window, an alert is sent. `0` disables. Default: `0`.
    #[serde(default)]
    pub deadman_timeout_minutes: u32,
    /// Channel for dead-man's switch alerts (e.g. `telegram`). Falls back to
    /// the heartbeat delivery channel.
    #[serde(default)]
    pub deadman_channel: Option<String>,
    /// Recipient for dead-man's switch alerts. Falls back to `to`.
    #[serde(default)]
    pub deadman_to: Option<String>,
    /// Maximum number of heartbeat run history records to retain. Default: `100`.
    #[serde(default = "default_heartbeat_max_run_history")]
    pub max_run_history: u32,
}

fn default_two_phase() -> bool {
    true
}

fn default_heartbeat_min_interval() -> u32 {
    5
}

fn default_heartbeat_max_interval() -> u32 {
    120
}

fn default_heartbeat_max_run_history() -> u32 {
    100
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 30,
            two_phase: true,
            message: None,
            target: None,
            to: None,
            adaptive: false,
            min_interval_minutes: default_heartbeat_min_interval(),
            max_interval_minutes: default_heartbeat_max_interval(),
            deadman_timeout_minutes: 0,
            deadman_channel: None,
            deadman_to: None,
            max_run_history: default_heartbeat_max_run_history(),
        }
    }
}

// ── Cron ────────────────────────────────────────────────────────

/// Cron job configuration (`[cron]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CronConfig {
    /// Enable the cron subsystem. Default: `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum number of historical cron run records to retain. Default: `50`.
    #[serde(default = "default_max_run_history")]
    pub max_run_history: u32,
}

fn default_max_run_history() -> u32 {
    50
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_run_history: default_max_run_history(),
        }
    }
}

// ── Tunnel ──────────────────────────────────────────────────────

/// Tunnel configuration for exposing the gateway publicly (`[tunnel]` section).
///
/// Supported providers: `"none"` (default), `"cloudflare"`, `"tailscale"`, `"ngrok"`, `"openvpn"`, `"custom"`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelConfig {
    /// Tunnel provider: `"none"`, `"cloudflare"`, `"tailscale"`, `"ngrok"`, `"openvpn"`, or `"custom"`. Default: `"none"`.
    pub provider: String,

    /// Cloudflare Tunnel configuration (used when `provider = "cloudflare"`).
    #[serde(default)]
    pub cloudflare: Option<CloudflareTunnelConfig>,

    /// Tailscale Funnel/Serve configuration (used when `provider = "tailscale"`).
    #[serde(default)]
    pub tailscale: Option<TailscaleTunnelConfig>,

    /// ngrok tunnel configuration (used when `provider = "ngrok"`).
    #[serde(default)]
    pub ngrok: Option<NgrokTunnelConfig>,

    /// OpenVPN tunnel configuration (used when `provider = "openvpn"`).
    #[serde(default)]
    pub openvpn: Option<OpenVpnTunnelConfig>,

    /// Custom tunnel command configuration (used when `provider = "custom"`).
    #[serde(default)]
    pub custom: Option<CustomTunnelConfig>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            provider: "none".into(),
            cloudflare: None,
            tailscale: None,
            ngrok: None,
            openvpn: None,
            custom: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CloudflareTunnelConfig {
    /// Cloudflare Tunnel token (from Zero Trust dashboard)
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TailscaleTunnelConfig {
    /// Use Tailscale Funnel (public internet) vs Serve (tailnet only)
    #[serde(default)]
    pub funnel: bool,
    /// Optional hostname override
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NgrokTunnelConfig {
    /// ngrok auth token
    pub auth_token: String,
    /// Optional custom domain
    pub domain: Option<String>,
}

/// OpenVPN tunnel configuration (`[tunnel.openvpn]`).
///
/// Required when `tunnel.provider = "openvpn"`. Omitting this section entirely
/// preserves previous behavior. Setting `tunnel.provider = "none"` (or removing
/// the `[tunnel.openvpn]` block) cleanly reverts to no-tunnel mode.
///
/// Defaults: `connect_timeout_secs = 30`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenVpnTunnelConfig {
    /// Path to `.ovpn` configuration file (must not be empty).
    pub config_file: String,
    /// Optional path to auth credentials file (`--auth-user-pass`).
    #[serde(default)]
    pub auth_file: Option<String>,
    /// Advertised address once VPN is connected (e.g., `"10.8.0.2:42617"`).
    /// When omitted the tunnel falls back to `http://{local_host}:{local_port}`.
    #[serde(default)]
    pub advertise_address: Option<String>,
    /// Connection timeout in seconds (default: 30, must be > 0).
    #[serde(default = "default_openvpn_timeout")]
    pub connect_timeout_secs: u64,
    /// Extra openvpn CLI arguments forwarded verbatim.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

fn default_openvpn_timeout() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomTunnelConfig {
    /// Command template to start the tunnel. Use {port} and {host} placeholders.
    /// Example: "bore local {port} --to bore.pub"
    pub start_command: String,
    /// Optional URL to check tunnel health
    pub health_url: Option<String>,
    /// Optional regex to extract public URL from command stdout
    pub url_pattern: Option<String>,
}

// ── Channels ─────────────────────────────────────────────────────

struct ConfigWrapper<T: ChannelConfig>(std::marker::PhantomData<T>);

impl<T: ChannelConfig> ConfigWrapper<T> {
    fn new(_: Option<&T>) -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T: ChannelConfig> super::channel_traits::ConfigHandle for ConfigWrapper<T> {
    fn name(&self) -> &'static str {
        T::name()
    }
    fn desc(&self) -> &'static str {
        T::desc()
    }
}

/// Top-level channel configurations (`[channels_config]` section).
///
/// Each channel sub-section (e.g. `telegram`, `discord`) is optional;
/// setting it to `Some(...)` enables that channel.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelsConfig {
    /// Enable the CLI interactive channel. Default: `true`.
    #[serde(default = "default_true")]
    pub cli: bool,
    /// Telegram bot channel configuration.
    pub telegram: Option<TelegramConfig>,
    /// Discord bot channel configuration.
    pub discord: Option<DiscordConfig>,
    /// Slack bot channel configuration.
    pub slack: Option<SlackConfig>,
    /// Mattermost bot channel configuration.
    pub mattermost: Option<MattermostConfig>,
    /// Webhook channel configuration.
    pub webhook: Option<WebhookConfig>,
    /// iMessage channel configuration (macOS only).
    pub imessage: Option<IMessageConfig>,
    /// Matrix channel configuration.
    pub matrix: Option<MatrixConfig>,
    /// Signal channel configuration.
    pub signal: Option<SignalConfig>,
    /// WhatsApp channel configuration (Cloud API or Web mode).
    pub whatsapp: Option<WhatsAppConfig>,
    /// Linq Partner API channel configuration.
    pub linq: Option<LinqConfig>,
    /// WATI WhatsApp Business API channel configuration.
    pub wati: Option<WatiConfig>,
    /// Nextcloud Talk bot channel configuration.
    pub nextcloud_talk: Option<NextcloudTalkConfig>,
    /// Email channel configuration.
    pub email: Option<super::adapter_configs::EmailConfig>,
    /// IRC channel configuration.
    pub irc: Option<IrcConfig>,
    /// Lark channel configuration.
    pub lark: Option<LarkConfig>,
    /// Feishu channel configuration.
    pub feishu: Option<FeishuConfig>,
    /// DingTalk channel configuration.
    pub dingtalk: Option<DingTalkConfig>,
    /// WeCom (WeChat Enterprise) Bot Webhook channel configuration.
    pub wecom: Option<WeComConfig>,
    /// QQ Official Bot channel configuration.
    pub qq: Option<QQConfig>,
    /// X/Twitter channel configuration.
    pub twitter: Option<TwitterConfig>,
    /// Mochat customer service channel configuration.
    pub mochat: Option<MochatConfig>,
    #[cfg(feature = "channel-nostr")]
    pub nostr: Option<NostrConfig>,
    /// ClawdTalk voice channel configuration.
    pub clawdtalk: Option<super::adapter_configs::ClawdTalkConfig>,
    /// Reddit channel configuration (OAuth2 bot).
    pub reddit: Option<RedditConfig>,
    /// Bluesky channel configuration (AT Protocol).
    pub bluesky: Option<BlueskyConfig>,
    /// Base timeout in seconds for processing a single channel message (LLM + tools).
    /// Runtime uses this as a per-turn budget that scales with tool-loop depth
    /// (up to 4x, capped) so one slow/retried model call does not consume the
    /// entire conversation budget.
    /// Default: 300s for on-device LLMs (Ollama) which are slower than cloud APIs.
    #[serde(default = "default_channel_message_timeout_secs")]
    pub message_timeout_secs: u64,
    /// Whether to add acknowledgement reactions (👀 on receipt, ✅/⚠️ on
    /// completion) to incoming channel messages. Default: `true`.
    #[serde(default = "default_true")]
    pub ack_reactions: bool,
    /// Explicitly opt into verbose tool trace in messaging channels.
    /// When `false` (default), channels stay human-first: no raw tool-call spam,
    /// only normal replies plus compact progress/approval/error updates.
    /// When `true`, tool calls may be rendered as individual channel messages.
    #[serde(default = "default_false")]
    pub show_tool_calls: bool,
    /// Persist channel conversation history to JSONL files so sessions survive
    /// daemon restarts. Files are stored in `{workspace}/sessions/`. Default: `true`.
    #[serde(default = "default_true")]
    pub session_persistence: bool,
    /// Session persistence backend: `"jsonl"` (legacy) or `"sqlite"` (new default).
    /// SQLite provides FTS5 search, metadata tracking, and TTL cleanup.
    #[serde(default = "default_session_backend")]
    pub session_backend: String,
    /// Auto-archive stale sessions older than this many hours. `0` disables. Default: `0`.
    #[serde(default)]
    pub session_ttl_hours: u32,
}

impl ChannelsConfig {
    /// get channels' metadata and `.is_some()`, except webhook
    #[rustfmt::skip]
    pub fn channels_except_webhook(&self) -> Vec<(Box<dyn super::channel_traits::ConfigHandle>, bool)> {
        vec![
            (
                Box::new(ConfigWrapper::new(self.telegram.as_ref())),
                self.telegram.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.discord.as_ref())),
                self.discord.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.slack.as_ref())),
                self.slack.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.mattermost.as_ref())),
                self.mattermost.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.imessage.as_ref())),
                self.imessage.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.matrix.as_ref())),
                self.matrix.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.signal.as_ref())),
                self.signal.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.whatsapp.as_ref())),
                self.whatsapp.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.linq.as_ref())),
                self.linq.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.wati.as_ref())),
                self.wati.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.nextcloud_talk.as_ref())),
                self.nextcloud_talk.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.email.as_ref())),
                self.email.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.irc.as_ref())),
                self.irc.is_some()
            ),
            (
                Box::new(ConfigWrapper::new(self.lark.as_ref())),
                self.lark.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.feishu.as_ref())),
                self.feishu.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.dingtalk.as_ref())),
                self.dingtalk.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.wecom.as_ref())),
                self.wecom.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.qq.as_ref())),
                self.qq.is_some()
            ),
            #[cfg(feature = "channel-nostr")]
            (
                Box::new(ConfigWrapper::new(self.nostr.as_ref())),
                self.nostr.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.clawdtalk.as_ref())),
                self.clawdtalk.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.reddit.as_ref())),
                self.reddit.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.bluesky.as_ref())),
                self.bluesky.is_some(),
            ),
        ]
    }

    pub fn channels(&self) -> Vec<(Box<dyn super::channel_traits::ConfigHandle>, bool)> {
        let mut ret = self.channels_except_webhook();
        ret.push((
            Box::new(ConfigWrapper::new(self.webhook.as_ref())),
            self.webhook.is_some(),
        ));
        ret
    }
}

fn default_channel_message_timeout_secs() -> u64 {
    300
}

pub fn default_session_backend() -> String {
    "sqlite".into()
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: None,
            matrix: None,
            signal: None,
            whatsapp: None,
            linq: None,
            wati: None,
            nextcloud_talk: None,
            email: None,
            irc: None,
            lark: None,
            feishu: None,
            dingtalk: None,
            wecom: None,
            qq: None,
            twitter: None,
            mochat: None,
            #[cfg(feature = "channel-nostr")]
            nostr: None,
            clawdtalk: None,
            reddit: None,
            bluesky: None,
            message_timeout_secs: default_channel_message_timeout_secs(),
            ack_reactions: true,
            show_tool_calls: false,
            session_persistence: true,
            session_backend: default_session_backend(),
            session_ttl_hours: 0,
        }
    }
}

/// Streaming mode for channels that support progressive message updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StreamMode {
    /// No streaming -- send the complete response as a single message (default).
    #[default]
    Off,
    /// Update a draft message with every flush interval.
    Partial,
}

pub fn default_draft_update_interval_ms() -> u64 {
    1000
}

/// Telegram bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TelegramConfig {
    /// Telegram Bot API token (from @BotFather).
    pub bot_token: String,
    /// Allowed Telegram user IDs or usernames. Empty = deny all.
    pub allowed_users: Vec<String>,
    /// Streaming mode for progressive response delivery via message edits.
    #[serde(default)]
    pub stream_mode: StreamMode,
    /// Minimum interval (ms) between draft message edits to avoid rate limits.
    #[serde(default = "default_draft_update_interval_ms")]
    pub draft_update_interval_ms: u64,
    /// When true, a newer Telegram message from the same sender in the same chat
    /// cancels the in-flight request and starts a fresh response with preserved history.
    #[serde(default)]
    pub interrupt_on_new_message: bool,
    /// When true, only respond to messages that @-mention the bot in groups.
    /// Direct messages are always processed.
    #[serde(default)]
    pub mention_only: bool,
}

impl ChannelConfig for TelegramConfig {
    fn name() -> &'static str {
        "Telegram"
    }
    fn desc() -> &'static str {
        "connect your bot"
    }
}

/// Discord bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscordConfig {
    /// Discord bot token (from Discord Developer Portal).
    pub bot_token: String,
    /// Optional guild (server) ID to restrict the bot to a single guild.
    pub guild_id: Option<String>,
    /// Allowed Discord user IDs. Empty = deny all.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true, process messages from other bots (not just humans).
    /// The bot still ignores its own messages to prevent feedback loops.
    #[serde(default)]
    pub listen_to_bots: bool,
    /// When true, only respond to messages that @-mention the bot.
    /// Other messages in the guild are silently ignored.
    #[serde(default)]
    pub mention_only: bool,
}

impl ChannelConfig for DiscordConfig {
    fn name() -> &'static str {
        "Discord"
    }
    fn desc() -> &'static str {
        "connect your bot"
    }
}

/// Slack bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SlackConfig {
    /// Slack bot OAuth token (xoxb-...).
    pub bot_token: String,
    /// Slack app-level token for Socket Mode (xapp-...).
    pub app_token: Option<String>,
    /// Optional channel ID to restrict the bot to a single channel.
    /// Omit (or set `"*"`) to listen across all accessible channels.
    pub channel_id: Option<String>,
    /// Allowed Slack user IDs. Empty = deny all.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true, a newer Slack message from the same sender in the same channel
    /// cancels the in-flight request and starts a fresh response with preserved history.
    #[serde(default)]
    pub interrupt_on_new_message: bool,
    /// When true, only respond to messages that @-mention the bot in groups.
    /// Direct messages remain allowed.
    #[serde(default)]
    pub mention_only: bool,
}

impl ChannelConfig for SlackConfig {
    fn name() -> &'static str {
        "Slack"
    }
    fn desc() -> &'static str {
        "connect your bot"
    }
}

/// Mattermost bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MattermostConfig {
    /// Mattermost server URL (e.g. `"https://mattermost.example.com"`).
    pub url: String,
    /// Mattermost bot access token.
    pub bot_token: String,
    /// Optional channel ID to restrict the bot to a single channel.
    pub channel_id: Option<String>,
    /// Allowed Mattermost user IDs. Empty = deny all.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true (default), replies thread on the original post.
    /// When false, replies go to the channel root.
    #[serde(default)]
    pub thread_replies: Option<bool>,
    /// When true, only respond to messages that @-mention the bot.
    /// Other messages in the channel are silently ignored.
    #[serde(default)]
    pub mention_only: Option<bool>,
}

impl ChannelConfig for MattermostConfig {
    fn name() -> &'static str {
        "Mattermost"
    }
    fn desc() -> &'static str {
        "connect to your bot"
    }
}

/// Webhook channel configuration.
///
/// Receives messages via HTTP POST and sends replies to a configurable outbound URL.
/// This is the "universal adapter" for any system that supports webhooks.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebhookConfig {
    /// Port to listen on for incoming webhooks.
    pub port: u16,
    /// URL path to listen on (default: `/webhook`).
    #[serde(default)]
    pub listen_path: Option<String>,
    /// URL to POST/PUT outbound messages to.
    #[serde(default)]
    pub send_url: Option<String>,
    /// HTTP method for outbound messages (`POST` or `PUT`). Default: `POST`.
    #[serde(default)]
    pub send_method: Option<String>,
    /// Optional `Authorization` header value for outbound requests.
    #[serde(default)]
    pub auth_header: Option<String>,
    /// Optional shared secret for webhook signature verification (HMAC-SHA256).
    pub secret: Option<String>,
}

impl ChannelConfig for WebhookConfig {
    fn name() -> &'static str {
        "Webhook"
    }
    fn desc() -> &'static str {
        "HTTP endpoint"
    }
}

/// iMessage channel configuration (macOS only).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IMessageConfig {
    /// Allowed iMessage contacts (phone numbers or email addresses). Empty = deny all.
    pub allowed_contacts: Vec<String>,
}

impl ChannelConfig for IMessageConfig {
    fn name() -> &'static str {
        "iMessage"
    }
    fn desc() -> &'static str {
        "macOS only"
    }
}

/// Matrix channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MatrixConfig {
    /// Matrix homeserver URL (e.g. `"https://matrix.org"`).
    pub homeserver: String,
    /// Matrix access token for the bot account.
    /// Optional when `password` is set (bot will login and obtain a token automatically).
    #[serde(default)]
    pub access_token: Option<String>,
    /// Optional Matrix user ID (e.g. `"@bot:matrix.org"`).
    /// Required when using password-based login without access_token.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Optional Matrix device ID.
    #[serde(default)]
    pub device_id: Option<String>,
    /// Matrix room ID to listen in (e.g. `"!abc123:matrix.org"`).
    pub room_id: String,
    /// Allowed Matrix user IDs. Empty = deny all.
    pub allowed_users: Vec<String>,
    /// Optional Matrix account password. When set, the bot will:
    /// - Login automatically if no access_token is configured (simplest setup).
    /// - Bootstrap cross-signing so the device is marked as "verified by its owner".
    #[serde(default)]
    pub password: Option<String>,
    /// Maximum media download size in megabytes. Set to 0 for no limit.
    /// Defaults to 50 MB when omitted.
    #[serde(default)]
    pub max_media_download_mb: Option<u32>,
}

impl ChannelConfig for MatrixConfig {
    fn name() -> &'static str {
        "Matrix"
    }
    fn desc() -> &'static str {
        "self-hosted chat"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SignalConfig {
    /// Base URL for the signal-cli HTTP daemon (e.g. "http://127.0.0.1:8686").
    pub http_url: String,
    /// E.164 phone number of the signal-cli account (e.g. "+1234567890").
    pub account: String,
    /// Optional group ID to filter messages.
    /// - `None` or omitted: accept all messages (DMs and groups)
    /// - `"dm"`: only accept direct messages
    /// - Specific group ID: only accept messages from that group
    #[serde(default)]
    pub group_id: Option<String>,
    /// Allowed sender phone numbers (E.164) or "*" for all.
    #[serde(default)]
    pub allowed_from: Vec<String>,
    /// Skip messages that are attachment-only (no text body).
    #[serde(default)]
    pub ignore_attachments: bool,
    /// Skip incoming story messages.
    #[serde(default)]
    pub ignore_stories: bool,
}

impl ChannelConfig for SignalConfig {
    fn name() -> &'static str {
        "Signal"
    }
    fn desc() -> &'static str {
        "An open-source, encrypted messaging service"
    }
}

/// WhatsApp channel configuration (Cloud API or Web mode).
///
/// Set `phone_number_id` for Cloud API mode, or `session_path` for Web mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WhatsAppConfig {
    /// Access token from Meta Business Suite (Cloud API mode)
    #[serde(default)]
    pub access_token: Option<String>,
    /// Phone number ID from Meta Business API (Cloud API mode)
    #[serde(default)]
    pub phone_number_id: Option<String>,
    /// Webhook verify token (you define this, Meta sends it back for verification)
    /// Only used in Cloud API mode
    #[serde(default)]
    pub verify_token: Option<String>,
    /// App secret from Meta Business Suite (for webhook signature verification)
    /// Can also be set via `SYNAPSECLAW_WHATSAPP_APP_SECRET` environment variable
    /// Only used in Cloud API mode
    #[serde(default)]
    pub app_secret: Option<String>,
    /// Session database path for WhatsApp Web client (Web mode)
    /// When set, enables native WhatsApp Web mode with wa-rs
    #[serde(default)]
    pub session_path: Option<String>,
    /// Phone number for pair code linking (Web mode, optional)
    /// Format: country code + number (e.g., "15551234567")
    /// If not set, QR code pairing will be used
    #[serde(default)]
    pub pair_phone: Option<String>,
    /// Custom pair code for linking (Web mode, optional)
    /// Leave empty to let WhatsApp generate one
    #[serde(default)]
    pub pair_code: Option<String>,
    /// Allowed phone numbers (E.164 format: +1234567890) or "*" for all
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
}

impl ChannelConfig for WhatsAppConfig {
    fn name() -> &'static str {
        "WhatsApp"
    }
    fn desc() -> &'static str {
        "Business Cloud API"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinqConfig {
    /// Linq Partner API token (Bearer auth)
    pub api_token: String,
    /// Phone number to send from (E.164 format)
    pub from_phone: String,
    /// Webhook signing secret for signature verification
    #[serde(default)]
    pub signing_secret: Option<String>,
    /// Allowed sender handles (phone numbers) or "*" for all
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

impl ChannelConfig for LinqConfig {
    fn name() -> &'static str {
        "Linq"
    }
    fn desc() -> &'static str {
        "iMessage/RCS/SMS via Linq API"
    }
}

/// WATI WhatsApp Business API channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WatiConfig {
    /// WATI API token (Bearer auth).
    pub api_token: String,
    /// WATI API base URL (default: https://live-mt-server.wati.io).
    #[serde(default = "default_wati_api_url")]
    pub api_url: String,
    /// Tenant ID for multi-channel setups (optional).
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Allowed phone numbers (E.164 format) or "*" for all.
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
}

fn default_wati_api_url() -> String {
    "https://live-mt-server.wati.io".to_string()
}

impl ChannelConfig for WatiConfig {
    fn name() -> &'static str {
        "WATI"
    }
    fn desc() -> &'static str {
        "WhatsApp via WATI Business API"
    }
}

/// Nextcloud Talk bot configuration (webhook receive + OCS send API).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NextcloudTalkConfig {
    /// Nextcloud base URL (e.g. "https://cloud.example.com").
    pub base_url: String,
    /// Bot app token used for OCS API bearer auth.
    pub app_token: String,
    /// Shared secret for webhook signature verification.
    ///
    /// Can also be set via `SYNAPSECLAW_NEXTCLOUD_TALK_WEBHOOK_SECRET`.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Allowed Nextcloud actor IDs (`[]` = deny all, `"*"` = allow all).
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for NextcloudTalkConfig {
    fn name() -> &'static str {
        "NextCloud Talk"
    }
    fn desc() -> &'static str {
        "NextCloud Talk platform"
    }
}

impl WhatsAppConfig {
    /// Detect which backend to use based on config fields.
    /// Returns "cloud" if phone_number_id is set, "web" if session_path is set.
    pub fn backend_type(&self) -> &'static str {
        if self.phone_number_id.is_some() {
            "cloud"
        } else if self.session_path.is_some() {
            "web"
        } else {
            // Default to Cloud API for backward compatibility
            "cloud"
        }
    }

    /// Check if this is a valid Cloud API config
    pub fn is_cloud_config(&self) -> bool {
        self.phone_number_id.is_some() && self.access_token.is_some() && self.verify_token.is_some()
    }

    /// Check if this is a valid Web config
    pub fn is_web_config(&self) -> bool {
        self.session_path.is_some()
    }

    /// Returns true when both Cloud and Web selectors are present.
    ///
    /// Runtime currently prefers Cloud mode in this case for backward compatibility.
    pub fn is_ambiguous_config(&self) -> bool {
        self.phone_number_id.is_some() && self.session_path.is_some()
    }
}

/// IRC channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IrcConfig {
    /// IRC server hostname
    pub server: String,
    /// IRC server port (default: 6697 for TLS)
    #[serde(default = "default_irc_port")]
    pub port: u16,
    /// Bot nickname
    pub nickname: String,
    /// Username (defaults to nickname if not set)
    pub username: Option<String>,
    /// Channels to join on connect
    #[serde(default)]
    pub channels: Vec<String>,
    /// Allowed nicknames (case-insensitive) or "*" for all
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Server password (for bouncers like ZNC)
    pub server_password: Option<String>,
    /// NickServ IDENTIFY password
    pub nickserv_password: Option<String>,
    /// SASL PLAIN password (IRCv3)
    pub sasl_password: Option<String>,
    /// Verify TLS certificate (default: true)
    pub verify_tls: Option<bool>,
}

impl ChannelConfig for IrcConfig {
    fn name() -> &'static str {
        "IRC"
    }
    fn desc() -> &'static str {
        "IRC over TLS"
    }
}

fn default_irc_port() -> u16 {
    6697
}

/// How SynapseClaw receives events from Feishu / Lark.
///
/// - `websocket` (default) — persistent WSS long-connection; no public URL required.
/// - `webhook`             — HTTP callback server; requires a public HTTPS endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LarkReceiveMode {
    #[default]
    Websocket,
    Webhook,
}

/// Lark/Feishu configuration for messaging integration.
/// Lark is the international version; Feishu is the Chinese version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LarkConfig {
    /// App ID from Lark/Feishu developer console
    pub app_id: String,
    /// App Secret from Lark/Feishu developer console
    pub app_secret: String,
    /// Encrypt key for webhook message decryption (optional)
    #[serde(default)]
    pub encrypt_key: Option<String>,
    /// Verification token for webhook validation (optional)
    #[serde(default)]
    pub verification_token: Option<String>,
    /// Allowed user IDs or union IDs (empty = deny all, "*" = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true, only respond to messages that @-mention the bot in groups.
    /// Direct messages are always processed.
    #[serde(default)]
    pub mention_only: bool,
    /// Whether to use the Feishu (Chinese) endpoint instead of Lark (International)
    #[serde(default)]
    pub use_feishu: bool,
    /// Event receive mode: "websocket" (default) or "webhook"
    #[serde(default)]
    pub receive_mode: LarkReceiveMode,
    /// HTTP port for webhook mode only. Must be set when receive_mode = "webhook".
    /// Not required (and ignored) for websocket mode.
    #[serde(default)]
    pub port: Option<u16>,
}

impl ChannelConfig for LarkConfig {
    fn name() -> &'static str {
        "Lark"
    }
    fn desc() -> &'static str {
        "Lark Bot"
    }
}

/// Feishu configuration for messaging integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FeishuConfig {
    /// App ID from Feishu developer console
    pub app_id: String,
    /// App Secret from Feishu developer console
    pub app_secret: String,
    /// Encrypt key for webhook message decryption (optional)
    #[serde(default)]
    pub encrypt_key: Option<String>,
    /// Verification token for webhook validation (optional)
    #[serde(default)]
    pub verification_token: Option<String>,
    /// Allowed user IDs or union IDs (empty = deny all, "*" = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Event receive mode: "websocket" (default) or "webhook"
    #[serde(default)]
    pub receive_mode: LarkReceiveMode,
    /// HTTP port for webhook mode only. Must be set when receive_mode = "webhook".
    /// Not required (and ignored) for websocket mode.
    #[serde(default)]
    pub port: Option<u16>,
}

impl ChannelConfig for FeishuConfig {
    fn name() -> &'static str {
        "Feishu"
    }
    fn desc() -> &'static str {
        "Feishu Bot"
    }
}

// ── Security Config ─────────────────────────────────────────────────

/// Security configuration for sandboxing, resource limits, and audit logging
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct SecurityConfig {
    /// Sandbox configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Resource limits
    #[serde(default)]
    pub resources: ResourceLimitsConfig,

    /// Audit logging configuration
    #[serde(default)]
    pub audit: AuditConfig,

    /// OTP gating configuration for sensitive actions/domains.
    #[serde(default)]
    pub otp: OtpConfig,

    /// Emergency-stop state machine configuration.
    #[serde(default)]
    pub estop: EstopConfig,

    /// Nevis IAM integration for SSO/MFA authentication and role-based access.
    #[serde(default)]
    pub nevis: NevisConfig,
}

/// OTP validation strategy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OtpMethod {
    /// Time-based one-time password (RFC 6238).
    #[default]
    Totp,
    /// Future method for paired-device confirmations.
    Pairing,
    /// Future method for local CLI challenge prompts.
    CliPrompt,
}

/// Security OTP configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OtpConfig {
    /// Enable OTP gating. Defaults to disabled for backward compatibility.
    #[serde(default)]
    pub enabled: bool,

    /// OTP method.
    #[serde(default)]
    pub method: OtpMethod,

    /// TOTP time-step in seconds.
    #[serde(default = "default_otp_token_ttl_secs")]
    pub token_ttl_secs: u64,

    /// Reuse window for recently validated OTP codes.
    #[serde(default = "default_otp_cache_valid_secs")]
    pub cache_valid_secs: u64,

    /// Tool/action names gated by OTP.
    #[serde(default = "default_otp_gated_actions")]
    pub gated_actions: Vec<String>,

    /// Explicit domain patterns gated by OTP.
    #[serde(default)]
    pub gated_domains: Vec<String>,

    /// Domain-category presets expanded into `gated_domains`.
    #[serde(default)]
    pub gated_domain_categories: Vec<String>,
}

fn default_otp_token_ttl_secs() -> u64 {
    30
}

fn default_otp_cache_valid_secs() -> u64 {
    300
}

fn default_otp_gated_actions() -> Vec<String> {
    vec![
        "shell".to_string(),
        "file_write".to_string(),
        "browser_open".to_string(),
        "browser".to_string(),
        "memory_forget".to_string(),
    ]
}

impl Default for OtpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            method: OtpMethod::Totp,
            token_ttl_secs: default_otp_token_ttl_secs(),
            cache_valid_secs: default_otp_cache_valid_secs(),
            gated_actions: default_otp_gated_actions(),
            gated_domains: Vec::new(),
            gated_domain_categories: Vec::new(),
        }
    }
}

/// Emergency stop configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EstopConfig {
    /// Enable emergency stop controls.
    #[serde(default)]
    pub enabled: bool,

    /// File path used to persist estop state.
    #[serde(default = "default_estop_state_file")]
    pub state_file: String,

    /// Require a valid OTP before resume operations.
    #[serde(default = "default_true")]
    pub require_otp_to_resume: bool,
}

fn default_estop_state_file() -> String {
    "~/.synapseclaw/estop-state.json".to_string()
}

impl Default for EstopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            state_file: default_estop_state_file(),
            require_otp_to_resume: true,
        }
    }
}

/// Nevis IAM integration configuration.
///
/// When `enabled` is true, SynapseClaw validates incoming requests against a Nevis
/// Security Suite instance and maps Nevis roles to tool/workspace permissions.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NevisConfig {
    /// Enable Nevis IAM integration. Defaults to false for backward compatibility.
    #[serde(default)]
    pub enabled: bool,

    /// Base URL of the Nevis instance (e.g. `https://nevis.example.com`).
    #[serde(default)]
    pub instance_url: String,

    /// Nevis realm to authenticate against.
    #[serde(default = "default_nevis_realm")]
    pub realm: String,

    /// OAuth2 client ID registered in Nevis.
    #[serde(default)]
    pub client_id: String,

    /// OAuth2 client secret. Encrypted via SecretStore when stored on disk.
    #[serde(default)]
    pub client_secret: Option<String>,

    /// Token validation strategy: `"local"` (JWKS) or `"remote"` (introspection).
    #[serde(default = "default_nevis_token_validation")]
    pub token_validation: String,

    /// JWKS endpoint URL for local token validation.
    #[serde(default)]
    pub jwks_url: Option<String>,

    /// Nevis role to SynapseClaw permission mappings.
    #[serde(default)]
    pub role_mapping: Vec<NevisRoleMappingConfig>,

    /// Require MFA verification for all Nevis-authenticated requests.
    #[serde(default)]
    pub require_mfa: bool,

    /// Session timeout in seconds.
    #[serde(default = "default_nevis_session_timeout_secs")]
    pub session_timeout_secs: u64,
}

impl std::fmt::Debug for NevisConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NevisConfig")
            .field("enabled", &self.enabled)
            .field("instance_url", &self.instance_url)
            .field("realm", &self.realm)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("token_validation", &self.token_validation)
            .field("jwks_url", &self.jwks_url)
            .field("role_mapping", &self.role_mapping)
            .field("require_mfa", &self.require_mfa)
            .field("session_timeout_secs", &self.session_timeout_secs)
            .finish()
    }
}

impl NevisConfig {
    /// Validate that required fields are present when Nevis is enabled.
    ///
    /// Call at config load time to fail fast on invalid configuration rather
    /// than deferring errors to the first authentication request.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        if self.instance_url.trim().is_empty() {
            return Err("nevis.instance_url is required when Nevis IAM is enabled".into());
        }

        if self.client_id.trim().is_empty() {
            return Err("nevis.client_id is required when Nevis IAM is enabled".into());
        }

        if self.realm.trim().is_empty() {
            return Err("nevis.realm is required when Nevis IAM is enabled".into());
        }

        match self.token_validation.as_str() {
            "local" | "remote" => {}
            other => {
                return Err(format!(
                    "nevis.token_validation has invalid value '{other}': \
                     expected 'local' or 'remote'"
                ));
            }
        }

        if self.token_validation == "local" && self.jwks_url.is_none() {
            return Err("nevis.jwks_url is required when token_validation is 'local'".into());
        }

        if self.session_timeout_secs == 0 {
            return Err("nevis.session_timeout_secs must be greater than 0".into());
        }

        Ok(())
    }
}

fn default_nevis_realm() -> String {
    "master".into()
}

fn default_nevis_token_validation() -> String {
    "local".into()
}

fn default_nevis_session_timeout_secs() -> u64 {
    3600
}

impl Default for NevisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            instance_url: String::new(),
            realm: default_nevis_realm(),
            client_id: String::new(),
            client_secret: None,
            token_validation: default_nevis_token_validation(),
            jwks_url: None,
            role_mapping: Vec::new(),
            require_mfa: false,
            session_timeout_secs: default_nevis_session_timeout_secs(),
        }
    }
}

/// Maps a Nevis role to SynapseClaw tool permissions and workspace access.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NevisRoleMappingConfig {
    /// Nevis role name (case-insensitive).
    pub nevis_role: String,

    /// Tool names this role can access. Use `"all"` for unrestricted tool access.
    #[serde(default)]
    pub synapseclaw_permissions: Vec<String>,

    /// Workspace names this role can access. Use `"all"` for unrestricted.
    #[serde(default)]
    pub workspace_access: Vec<String>,
}

/// Sandbox configuration for OS-level isolation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxConfig {
    /// Enable sandboxing (None = auto-detect, Some = explicit)
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Sandbox backend to use
    #[serde(default)]
    pub backend: SandboxBackend,

    /// Custom Firejail arguments (when backend = firejail)
    #[serde(default)]
    pub firejail_args: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: None, // Auto-detect
            backend: SandboxBackend::Auto,
            firejail_args: Vec::new(),
        }
    }
}

/// Sandbox backend selection
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SandboxBackend {
    /// Auto-detect best available (default)
    #[default]
    Auto,
    /// Landlock (Linux kernel LSM, native)
    Landlock,
    /// Firejail (user-space sandbox)
    Firejail,
    /// Bubblewrap (user namespaces)
    Bubblewrap,
    /// Docker container isolation
    Docker,
    /// No sandboxing (application-layer only)
    None,
}

/// Resource limits for command execution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceLimitsConfig {
    /// Maximum memory in MB per command
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU time in seconds per command
    #[serde(default = "default_max_cpu_time_seconds")]
    pub max_cpu_time_seconds: u64,

    /// Maximum number of subprocesses
    #[serde(default = "default_max_subprocesses")]
    pub max_subprocesses: u32,

    /// Enable memory monitoring
    #[serde(default = "default_memory_monitoring_enabled")]
    pub memory_monitoring: bool,
}

fn default_max_memory_mb() -> u32 {
    512
}

fn default_max_cpu_time_seconds() -> u64 {
    60
}

fn default_max_subprocesses() -> u32 {
    10
}

fn default_memory_monitoring_enabled() -> bool {
    true
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
            max_cpu_time_seconds: default_max_cpu_time_seconds(),
            max_subprocesses: default_max_subprocesses(),
            memory_monitoring: default_memory_monitoring_enabled(),
        }
    }
}

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditConfig {
    /// Enable audit logging
    #[serde(default = "default_audit_enabled")]
    pub enabled: bool,

    /// Path to audit log file (relative to synapseclaw dir)
    #[serde(default = "default_audit_log_path")]
    pub log_path: String,

    /// Maximum log size in MB before rotation
    #[serde(default = "default_audit_max_size_mb")]
    pub max_size_mb: u32,

    /// Sign events with HMAC for tamper evidence
    #[serde(default)]
    pub sign_events: bool,
}

fn default_audit_enabled() -> bool {
    true
}

fn default_audit_log_path() -> String {
    "audit.log".to_string()
}

fn default_audit_max_size_mb() -> u32 {
    100
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: default_audit_enabled(),
            log_path: default_audit_log_path(),
            max_size_mb: default_audit_max_size_mb(),
            sign_events: false,
        }
    }
}

/// DingTalk configuration for Stream Mode messaging
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DingTalkConfig {
    /// Client ID (AppKey) from DingTalk developer console
    pub client_id: String,
    /// Client Secret (AppSecret) from DingTalk developer console
    pub client_secret: String,
    /// Allowed user IDs (staff IDs). Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for DingTalkConfig {
    fn name() -> &'static str {
        "DingTalk"
    }
    fn desc() -> &'static str {
        "DingTalk Stream Mode"
    }
}

/// WeCom (WeChat Enterprise) Bot Webhook configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WeComConfig {
    /// Webhook key from WeCom Bot configuration
    pub webhook_key: String,
    /// Allowed user IDs. Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for WeComConfig {
    fn name() -> &'static str {
        "WeCom"
    }
    fn desc() -> &'static str {
        "WeCom Bot Webhook"
    }
}

/// QQ Official Bot configuration (Tencent QQ Bot SDK)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QQConfig {
    /// App ID from QQ Bot developer console
    pub app_id: String,
    /// App Secret from QQ Bot developer console
    pub app_secret: String,
    /// Allowed user IDs. Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for QQConfig {
    fn name() -> &'static str {
        "QQ Official"
    }
    fn desc() -> &'static str {
        "Tencent QQ Bot"
    }
}

/// X/Twitter channel configuration (Twitter API v2)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TwitterConfig {
    /// Twitter API v2 Bearer Token (OAuth 2.0)
    pub bearer_token: String,
    /// Allowed usernames or user IDs. Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for TwitterConfig {
    fn name() -> &'static str {
        "X/Twitter"
    }
    fn desc() -> &'static str {
        "X/Twitter Bot via API v2"
    }
}

/// Mochat channel configuration (Mochat customer service API)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MochatConfig {
    /// Mochat API base URL
    pub api_url: String,
    /// Mochat API token
    pub api_token: String,
    /// Allowed user IDs. Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Poll interval in seconds for new messages. Default: 5
    #[serde(default = "default_mochat_poll_interval")]
    pub poll_interval_secs: u64,
}

fn default_mochat_poll_interval() -> u64 {
    5
}

impl ChannelConfig for MochatConfig {
    fn name() -> &'static str {
        "Mochat"
    }
    fn desc() -> &'static str {
        "Mochat Customer Service"
    }
}

/// Reddit channel configuration (OAuth2 bot).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RedditConfig {
    /// Reddit OAuth2 client ID.
    pub client_id: String,
    /// Reddit OAuth2 client secret.
    pub client_secret: String,
    /// Reddit OAuth2 refresh token for persistent access.
    pub refresh_token: String,
    /// Reddit bot username (without `u/` prefix).
    pub username: String,
    /// Optional subreddit to filter messages (without `r/` prefix).
    /// When set, only messages from this subreddit are processed.
    #[serde(default)]
    pub subreddit: Option<String>,
}

impl ChannelConfig for RedditConfig {
    fn name() -> &'static str {
        "Reddit"
    }
    fn desc() -> &'static str {
        "Reddit bot (OAuth2)"
    }
}

/// Bluesky channel configuration (AT Protocol).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BlueskyConfig {
    /// Bluesky handle (e.g. `"mybot.bsky.social"`).
    pub handle: String,
    /// App-specific password (from Bluesky settings).
    pub app_password: String,
}

impl ChannelConfig for BlueskyConfig {
    fn name() -> &'static str {
        "Bluesky"
    }
    fn desc() -> &'static str {
        "AT Protocol"
    }
}

/// Nostr channel configuration (NIP-04 + NIP-17 private messages)
#[cfg(feature = "channel-nostr")]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NostrConfig {
    /// Private key in hex or nsec bech32 format
    pub private_key: String,
    /// Relay URLs (wss://). Defaults to popular public relays if omitted.
    #[serde(default = "default_nostr_relays")]
    pub relays: Vec<String>,
    /// Allowed sender public keys (hex or npub). Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_pubkeys: Vec<String>,
}

#[cfg(feature = "channel-nostr")]
impl ChannelConfig for NostrConfig {
    fn name() -> &'static str {
        "Nostr"
    }
    fn desc() -> &'static str {
        "Nostr DMs"
    }
}

#[cfg(feature = "channel-nostr")]
pub fn default_nostr_relays() -> Vec<String> {
    vec![
        "wss://relay.damus.io".to_string(),
        "wss://nos.lol".to_string(),
        "wss://relay.primal.net".to_string(),
        "wss://relay.snort.social".to_string(),
    ]
}

// -- Notion --

/// Notion integration configuration (`[notion]`).
///
/// When `enabled = true`, the agent polls a Notion database for pending tasks
/// and exposes a `notion` tool for querying, reading, creating, and updating pages.
/// Requires `api_key` (or the `NOTION_API_KEY` env var) and `database_id`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub database_id: String,
    #[serde(default = "default_notion_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_notion_status_prop")]
    pub status_property: String,
    #[serde(default = "default_notion_input_prop")]
    pub input_property: String,
    #[serde(default = "default_notion_result_prop")]
    pub result_property: String,
    #[serde(default = "default_notion_max_concurrent")]
    pub max_concurrent: usize,
    #[serde(default = "default_notion_recover_stale")]
    pub recover_stale: bool,
}

fn default_notion_poll_interval() -> u64 {
    5
}
fn default_notion_status_prop() -> String {
    "Status".into()
}
fn default_notion_input_prop() -> String {
    "Input".into()
}
fn default_notion_result_prop() -> String {
    "Result".into()
}
fn default_notion_max_concurrent() -> usize {
    4
}
fn default_notion_recover_stale() -> bool {
    true
}

impl Default for NotionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            database_id: String::new(),
            poll_interval_secs: default_notion_poll_interval(),
            status_property: default_notion_status_prop(),
            input_property: default_notion_input_prop(),
            result_property: default_notion_result_prop(),
            max_concurrent: default_notion_max_concurrent(),
            recover_stale: default_notion_recover_stale(),
        }
    }
}

///
/// Controls the read-only cloud transformation analysis tools:
/// IaC review, migration assessment, cost analysis, and architecture review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CloudOpsConfig {
    /// Enable cloud operations tools. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Default cloud provider for analysis context. Default: "aws".
    #[serde(default = "default_cloud_ops_cloud")]
    pub default_cloud: String,
    /// Supported cloud providers. Default: [`aws`, `azure`, `gcp`].
    #[serde(default = "default_cloud_ops_supported_clouds")]
    pub supported_clouds: Vec<String>,
    /// Supported IaC tools for review. Default: [`terraform`].
    #[serde(default = "default_cloud_ops_iac_tools")]
    pub iac_tools: Vec<String>,
    /// Monthly USD threshold to flag cost items. Default: 100.0.
    #[serde(default = "default_cloud_ops_cost_threshold")]
    pub cost_threshold_monthly_usd: f64,
    /// Well-Architected Frameworks to check against. Default: [`aws-waf`].
    #[serde(default = "default_cloud_ops_waf")]
    pub well_architected_frameworks: Vec<String>,
}

impl Default for CloudOpsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_cloud: default_cloud_ops_cloud(),
            supported_clouds: default_cloud_ops_supported_clouds(),
            iac_tools: default_cloud_ops_iac_tools(),
            cost_threshold_monthly_usd: default_cloud_ops_cost_threshold(),
            well_architected_frameworks: default_cloud_ops_waf(),
        }
    }
}

impl CloudOpsConfig {
    pub fn validate(&self) -> Result<()> {
        if self.enabled {
            if self.default_cloud.trim().is_empty() {
                anyhow::bail!(
                    "cloud_ops.default_cloud must not be empty when cloud_ops is enabled"
                );
            }
            if self.supported_clouds.is_empty() {
                anyhow::bail!(
                    "cloud_ops.supported_clouds must not be empty when cloud_ops is enabled"
                );
            }
            for (i, cloud) in self.supported_clouds.iter().enumerate() {
                if cloud.trim().is_empty() {
                    anyhow::bail!("cloud_ops.supported_clouds[{i}] must not be empty");
                }
            }
            if !self.supported_clouds.contains(&self.default_cloud) {
                anyhow::bail!(
                    "cloud_ops.default_cloud '{}' is not in cloud_ops.supported_clouds {:?}",
                    self.default_cloud,
                    self.supported_clouds
                );
            }
            if self.cost_threshold_monthly_usd < 0.0 {
                anyhow::bail!(
                    "cloud_ops.cost_threshold_monthly_usd must be non-negative, got {}",
                    self.cost_threshold_monthly_usd
                );
            }
            if self.iac_tools.is_empty() {
                anyhow::bail!("cloud_ops.iac_tools must not be empty when cloud_ops is enabled");
            }
        }
        Ok(())
    }
}

fn default_cloud_ops_cloud() -> String {
    "aws".into()
}

fn default_cloud_ops_supported_clouds() -> Vec<String> {
    vec!["aws".into(), "azure".into(), "gcp".into()]
}

fn default_cloud_ops_iac_tools() -> Vec<String> {
    vec!["terraform".into()]
}

fn default_cloud_ops_cost_threshold() -> f64 {
    100.0
}

fn default_cloud_ops_waf() -> Vec<String> {
    vec!["aws-waf".into()]
}

// ── Conversational AI ──────────────────────────────────────────────

fn default_conversational_ai_language() -> String {
    "en".into()
}

fn default_conversational_ai_supported_languages() -> Vec<String> {
    vec!["en".into(), "de".into(), "fr".into(), "it".into()]
}

fn default_conversational_ai_escalation_threshold() -> f64 {
    0.3
}

fn default_conversational_ai_max_turns() -> usize {
    50
}

fn default_conversational_ai_timeout_secs() -> u64 {
    1800
}

/// Conversational AI agent builder configuration (`[conversational_ai]` section).
///
/// Controls language detection, escalation behavior, conversation limits, and
/// analytics for conversational agent workflows. Disabled by default.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConversationalAiConfig {
    /// Enable conversational AI features. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Default language for conversations (BCP-47 tag). Default: "en".
    #[serde(default = "default_conversational_ai_language")]
    pub default_language: String,
    /// Supported languages for conversations. Default: [`en`, `de`, `fr`, `it`].
    #[serde(default = "default_conversational_ai_supported_languages")]
    pub supported_languages: Vec<String>,
    /// Automatically detect user language from message content. Default: true.
    #[serde(default = "default_true")]
    pub auto_detect_language: bool,
    /// Intent confidence below this threshold triggers escalation. Default: 0.3.
    #[serde(default = "default_conversational_ai_escalation_threshold")]
    pub escalation_confidence_threshold: f64,
    /// Maximum conversation turns before auto-ending. Default: 50.
    #[serde(default = "default_conversational_ai_max_turns")]
    pub max_conversation_turns: usize,
    /// Conversation timeout in seconds (inactivity). Default: 1800.
    #[serde(default = "default_conversational_ai_timeout_secs")]
    pub conversation_timeout_secs: u64,
    /// Enable conversation analytics tracking. Default: false (privacy-by-default).
    #[serde(default)]
    pub analytics_enabled: bool,
    /// Optional tool name for RAG-based knowledge base lookup during conversations.
    #[serde(default)]
    pub knowledge_base_tool: Option<String>,
}

impl Default for ConversationalAiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_language: default_conversational_ai_language(),
            supported_languages: default_conversational_ai_supported_languages(),
            auto_detect_language: true,
            escalation_confidence_threshold: default_conversational_ai_escalation_threshold(),
            max_conversation_turns: default_conversational_ai_max_turns(),
            conversation_timeout_secs: default_conversational_ai_timeout_secs(),
            analytics_enabled: false,
            knowledge_base_tool: None,
        }
    }
}

// ── Security ops config ─────────────────────────────────────────

/// Managed Cybersecurity Service (MCSS) dashboard agent configuration (`[security_ops]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecurityOpsConfig {
    /// Enable security operations tools.
    #[serde(default)]
    pub enabled: bool,
    /// Directory containing incident response playbook definitions (JSON).
    #[serde(default = "default_playbooks_dir")]
    pub playbooks_dir: String,
    /// Automatically triage incoming alerts without user prompt.
    #[serde(default)]
    pub auto_triage: bool,
    /// Require human approval before executing playbook actions.
    #[serde(default = "default_require_approval")]
    pub require_approval_for_actions: bool,
    /// Maximum severity level that can be auto-remediated without approval.
    /// One of: "low", "medium", "high", "critical". Default: "low".
    #[serde(default = "default_max_auto_severity")]
    pub max_auto_severity: String,
    /// Directory for generated security reports.
    #[serde(default = "default_report_output_dir")]
    pub report_output_dir: String,
    /// Optional SIEM webhook URL for alert ingestion.
    #[serde(default)]
    pub siem_integration: Option<String>,
}

fn default_playbooks_dir() -> String {
    "~/.synapseclaw/playbooks".into()
}

fn default_require_approval() -> bool {
    true
}

fn default_max_auto_severity() -> String {
    "low".into()
}

fn default_report_output_dir() -> String {
    "~/.synapseclaw/security-reports".into()
}

impl Default for SecurityOpsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            playbooks_dir: default_playbooks_dir(),
            auto_triage: false,
            require_approval_for_actions: true,
            max_auto_severity: default_max_auto_severity(),
            report_output_dir: default_report_output_dir(),
            siem_integration: None,
        }
    }
}

// ── Config impl ──────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let synapseclaw_dir = home.join(".synapseclaw");

        Self {
            workspace_dir: synapseclaw_dir.join("workspace"),
            config_path: synapseclaw_dir.join("config.toml"),
            api_key: None,
            api_url: None,
            api_path: None,
            default_provider: super::model_catalog::default_reasoning_seed()
                .map(|(provider, _)| provider.to_string()),
            default_model: super::model_catalog::default_reasoning_seed()
                .map(|(_, model)| model.to_string()),
            summary_model: None,
            compression: ContextCompressionConfig::default(),
            compression_overrides: Vec::new(),
            summary: SummaryConfig::default(),
            model_providers: HashMap::new(),
            default_temperature: default_temperature(),
            provider_timeout_secs: default_provider_timeout_secs(),
            extra_headers: HashMap::new(),
            observability: ObservabilityConfig::default(),
            autonomy: AutonomyConfig::default(),
            backup: BackupConfig::default(),
            data_retention: DataRetentionConfig::default(),
            cloud_ops: CloudOpsConfig::default(),
            conversational_ai: ConversationalAiConfig::default(),
            security: SecurityConfig::default(),
            security_ops: SecurityOpsConfig::default(),
            runtime: RuntimeConfig::default(),
            reliability: ReliabilityConfig::default(),
            scheduler: SchedulerConfig::default(),
            agent: AgentConfig::default(),
            skills: SkillsConfig::default(),
            route_aliases: Vec::new(),
            model_lanes: Vec::new(),
            model_preset: None,
            embedding_routes: Vec::new(),
            heartbeat: HeartbeatConfig::default(),
            cron: CronConfig::default(),
            channels_config: ChannelsConfig::default(),
            memory: MemoryConfig::default(),
            storage: StorageConfig::default(),
            tunnel: TunnelConfig::default(),
            gateway: GatewayConfig::default(),
            composio: ComposioConfig::default(),
            microsoft365: Microsoft365Config::default(),
            secrets: SecretsConfig::default(),
            browser: BrowserConfig::default(),
            browser_delegate: super::adapter_configs::BrowserDelegateConfig::default(),
            http_request: HttpRequestConfig::default(),
            multimodal: MultimodalConfig::default(),
            web_fetch: WebFetchConfig::default(),
            web_search: WebSearchConfig::default(),
            project_intel: ProjectIntelConfig::default(),
            google_workspace: GoogleWorkspaceConfig::default(),
            proxy: ProxyConfig::default(),
            identity: IdentityConfig::default(),
            cost: CostConfig::default(),

            agents: HashMap::new(),
            swarms: HashMap::new(),
            hooks: HooksConfig::default(),

            query_classification: QueryClassificationConfig::default(),
            transcription: TranscriptionConfig::default(),
            tts: TtsConfig::default(),
            mcp: McpConfig::default(),
            agents_ipc: AgentsIpcConfig::default(),
            pipelines: PipelineEngineConfig::default(),
            nodes: NodesConfig::default(),
            workspace: WorkspaceConfig::default(),
            notion: NotionConfig::default(),
            node_transport: NodeTransportConfig::default(),
            knowledge: KnowledgeConfig::default(),
            linkedin: LinkedInConfig::default(),
        }
    }
}
