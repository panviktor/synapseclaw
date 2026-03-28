//! Channel subsystem for messaging platform integrations.
//!
//! This module provides the multi-channel messaging infrastructure that connects
//! SynapseClaw to external platforms. Each channel implements the [`Channel`] trait
//! defined in [`traits`], which provides a uniform interface for sending messages,
//! listening for incoming messages, health checking, and typing indicators.
//!
//! Channels are instantiated by [`start_channels`] based on the runtime configuration.
//! The subsystem manages per-sender conversation history, concurrent message processing
//! with configurable parallelism, and exponential-backoff reconnection for resilience.
//!
//! # Extension
//!
//! To add a new channel, implement [`Channel`] in a new submodule and wire it into
//! [`start_channels`]. See `AGENTS.md` §7.2 for the full change playbook.

pub mod bluesky;
pub mod clawdtalk;
pub mod cli;
pub mod dingtalk;
pub mod discord;
pub mod email_channel;
pub mod imessage;
pub mod irc;
#[cfg(feature = "channel-lark")]
pub mod lark;
pub mod linq;
#[cfg(feature = "channel-matrix")]
pub mod matrix;
pub mod mattermost;
pub mod mochat;
pub mod nextcloud_talk;
#[cfg(feature = "channel-nostr")]
pub mod nostr;
pub mod notion;
pub mod qq;
pub mod reddit;
pub mod registry;
pub mod session_backend;
pub mod session_sqlite;
pub mod session_store;
pub mod signal;
pub mod slack;
pub mod telegram;
pub mod traits;
pub mod transcription;
pub mod tts;
pub mod twitter;
pub mod wati;
pub mod webhook;
pub mod wecom;
pub mod whatsapp;
#[cfg(feature = "whatsapp-web")]
pub mod whatsapp_storage;
#[cfg(feature = "whatsapp-web")]
pub mod whatsapp_web;

pub use bluesky::BlueskyChannel;
pub use clawdtalk::{ClawdTalkChannel, ClawdTalkConfig};
pub use cli::CliChannel;
pub use dingtalk::DingTalkChannel;
pub use discord::DiscordChannel;
pub use email_channel::EmailChannel;
pub use imessage::IMessageChannel;
pub use irc::IrcChannel;
#[cfg(feature = "channel-lark")]
pub use lark::LarkChannel;
pub use linq::LinqChannel;
#[cfg(feature = "channel-matrix")]
pub use matrix::MatrixChannel;
pub use mattermost::MattermostChannel;
pub use mochat::MochatChannel;
pub use nextcloud_talk::NextcloudTalkChannel;
#[cfg(feature = "channel-nostr")]
pub use nostr::NostrChannel;
pub use notion::NotionChannel;
pub use qq::QQChannel;
pub use reddit::RedditChannel;
pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use traits::{Channel, SendMessage};
#[allow(unused_imports)]
pub use tts::{TtsManager, TtsProvider};
pub use twitter::TwitterChannel;
pub use wati::WatiChannel;
pub use webhook::WebhookChannel;
pub use wecom::WeComChannel;
pub use whatsapp::WhatsAppChannel;
#[cfg(feature = "whatsapp-web")]
pub use whatsapp_web::WhatsAppWebChannel;

use crate::agent::loop_::build_tool_instructions;
use crate::config::Config;
use crate::fork_adapters::approval::ApprovalManager;
use crate::fork_adapters::channels::session_backend::SessionBackend;
use crate::fork_adapters::observability::traits::{ObserverEvent, ObserverMetric};
use crate::fork_adapters::observability::{self, Observer};
use crate::fork_adapters::providers::{self, ChatMessage, Provider};
use crate::fork_adapters::tools::{self, Tool};
use crate::identity;
use crate::memory::{self, Memory};
use crate::runtime;
use crate::security::security_policy_from_config;
use anyhow::{Context, Result};
use fork_core::domain::util::truncate_with_ellipsis;
use portable_atomic::{AtomicU64, Ordering};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

/// Observer wrapper that forwards tool-call events to a channel sender
/// for real-time threaded notifications.
struct ChannelNotifyObserver {
    inner: Arc<dyn Observer>,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    tools_used: AtomicBool,
}

impl Observer for ChannelNotifyObserver {
    fn record_event(&self, event: &ObserverEvent) {
        if let ObserverEvent::ToolCallStart { tool, arguments } = event {
            self.tools_used.store(true, Ordering::Relaxed);
            let detail = match arguments {
                Some(args) if !args.is_empty() => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
                        if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                            format!(": `{}`", truncate_with_ellipsis(cmd, 200))
                        } else if let Some(q) = v.get("query").and_then(|c| c.as_str()) {
                            format!(": {}", truncate_with_ellipsis(q, 200))
                        } else if let Some(p) = v.get("path").and_then(|c| c.as_str()) {
                            format!(": {p}")
                        } else if let Some(u) = v.get("url").and_then(|c| c.as_str()) {
                            format!(": {u}")
                        } else {
                            let s = args.to_string();
                            format!(": {}", truncate_with_ellipsis(&s, 120))
                        }
                    } else {
                        let s = args.to_string();
                        format!(": {}", truncate_with_ellipsis(&s, 120))
                    }
                }
                _ => String::new(),
            };
            let _ = self.tx.send(format!("\u{1F527} `{tool}`{detail}"));
        }
        self.inner.record_event(event);
    }
    fn record_metric(&self, metric: &ObserverMetric) {
        self.inner.record_metric(metric);
    }
    fn flush(&self) {
        self.inner.flush();
    }
    fn name(&self) -> &str {
        "channel-notify"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Per-sender conversation history for channel messages.
type ConversationHistoryMap = Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>;
/// Maximum history messages to keep per sender.
const MAX_CHANNEL_HISTORY: usize = 50;

/// Maximum characters per injected workspace file (matches `OpenClaw` default).
const BOOTSTRAP_MAX_CHARS: usize = 20_000;

const DEFAULT_CHANNEL_INITIAL_BACKOFF_SECS: u64 = 2;
const DEFAULT_CHANNEL_MAX_BACKOFF_SECS: u64 = 60;
const MIN_CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 30;
/// Default timeout for processing a single channel message (LLM + tools).
/// Used as fallback when not configured in channels_config.message_timeout_secs.
const CHANNEL_MESSAGE_TIMEOUT_SECS: u64 = 300;
const CHANNEL_PARALLELISM_PER_CHANNEL: usize = 4;
const CHANNEL_MIN_IN_FLIGHT_MESSAGES: usize = 8;
const CHANNEL_MAX_IN_FLIGHT_MESSAGES: usize = 64;
const CHANNEL_TYPING_REFRESH_INTERVAL_SECS: u64 = 4;
const CHANNEL_HEALTH_HEARTBEAT_SECS: u64 = 30;
const MODEL_CACHE_FILE: &str = "models_cache.json";
const MODEL_CACHE_PREVIEW_LIMIT: usize = 10;
/// Generate a rolling summary every N messages in channel conversations.
/// Higher than web's 10 because channel messages are typically less frequent.
const CHANNEL_SUMMARY_INTERVAL: usize = 20;

type ProviderCacheMap = Arc<Mutex<HashMap<String, Arc<dyn Provider>>>>;
/// Phase 4.0: RouteSelection from fork_core replaces the old ChannelRouteSelection.
type ChannelRouteSelection = fork_core::ports::route_selection::RouteSelection;
type RouteSelectionMap = Arc<Mutex<HashMap<String, ChannelRouteSelection>>>;

fn effective_channel_message_timeout_secs(configured: u64) -> u64 {
    configured.max(MIN_CHANNEL_MESSAGE_TIMEOUT_SECS)
}

/// Re-export from fork_core — runtime commands are domain logic.
use fork_core::application::services::inbound_message_service::RuntimeCommand as ChannelRuntimeCommand;

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelCacheState {
    entries: Vec<ModelCacheEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelCacheEntry {
    provider: String,
    models: Vec<String>,
}

#[derive(Debug, Clone)]
struct ChannelRuntimeDefaults {
    default_provider: String,
    model: String,
    temperature: f64,
    api_key: Option<String>,
    api_url: Option<String>,
    reliability: crate::config::ReliabilityConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigFileStamp {
    modified: SystemTime,
    len: u64,
}

#[derive(Debug, Clone)]
struct RuntimeConfigState {
    defaults: ChannelRuntimeDefaults,
    last_applied_stamp: Option<ConfigFileStamp>,
}

fn runtime_config_store() -> &'static Mutex<HashMap<PathBuf, RuntimeConfigState>> {
    static STORE: OnceLock<Mutex<HashMap<PathBuf, RuntimeConfigState>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

const SYSTEMD_STATUS_ARGS: [&str; 3] = ["--user", "is-active", "synapseclaw.service"];
const SYSTEMD_RESTART_ARGS: [&str; 3] = ["--user", "restart", "synapseclaw.service"];
const OPENRC_STATUS_ARGS: [&str; 2] = ["synapseclaw", "status"];
const OPENRC_RESTART_ARGS: [&str; 2] = ["synapseclaw", "restart"];

#[derive(Clone, Copy)]
struct InterruptOnNewMessageConfig {
    /// Global toggle — any channel with InterruptOnNewMessage capability will use it.
    enabled: bool,
}

impl InterruptOnNewMessageConfig {
    /// Phase 4.0: delegates to fork_core decision logic.
    fn enabled_for_channel(self, caps: &[fork_core::domain::channel::ChannelCapability]) -> bool {
        fork_core::application::services::inbound_message_service::should_interrupt_previous(
            self.enabled,
            caps,
        )
    }
}

#[derive(Clone)]
struct ChannelRuntimeContext {
    channels_by_name: Arc<HashMap<String, Arc<dyn Channel>>>,
    provider: Arc<dyn Provider>,
    default_provider: Arc<String>,
    memory: Arc<dyn Memory>,
    tools_registry: Arc<Vec<Box<dyn Tool>>>,
    observer: Arc<dyn Observer>,
    system_prompt: Arc<String>,
    model: Arc<String>,
    temperature: f64,
    auto_save_memory: bool,
    max_tool_iterations: usize,
    min_relevance_score: f64,
    conversation_histories: ConversationHistoryMap,
    provider_cache: ProviderCacheMap,
    route_overrides: RouteSelectionMap,
    api_key: Option<String>,
    api_url: Option<String>,
    reliability: Arc<crate::config::ReliabilityConfig>,
    provider_runtime_options: providers::ProviderRuntimeOptions,
    workspace_dir: Arc<PathBuf>,
    message_timeout_secs: u64,
    interrupt_on_new_message: InterruptOnNewMessageConfig,
    multimodal: crate::config::MultimodalConfig,
    hooks: Option<Arc<crate::fork_adapters::hooks::HookRunner>>,
    non_cli_excluded_tools: Arc<Vec<String>>,
    tool_call_dedup_exempt: Arc<Vec<String>>,
    model_routes: Arc<Vec<crate::config::ModelRouteConfig>>,
    query_classification: crate::config::QueryClassificationConfig,
    ack_reactions: bool,
    show_tool_calls: bool,
    session_store: Option<Arc<session_store::SessionStore>>,
    summary_config: Arc<crate::config::schema::SummaryConfig>,
    summary_model: Option<String>,
    /// Non-interactive approval manager for channel-driven runs.
    /// Enforces `auto_approve` / `always_ask` / supervised policy from
    /// `[autonomy]` config; auto-denies tools that would need interactive
    /// approval since no operator is present on channel runs.
    approval_manager: Arc<ApprovalManager>,
    activated_tools:
        Option<std::sync::Arc<std::sync::Mutex<crate::fork_adapters::tools::ActivatedToolSet>>>,
    /// Phase 4.0: channel registry for capability queries (None in standalone CLI mode).
    channel_registry: Option<Arc<dyn fork_core::ports::channel_registry::ChannelRegistryPort>>,
    /// Phase 4.1: pipeline engine ports (None if pipelines disabled).
    pipeline_store: Option<Arc<dyn fork_core::ports::pipeline_store::PipelineStorePort>>,
    pipeline_executor: Option<Arc<dyn fork_core::ports::pipeline_executor::PipelineExecutorPort>>,
    message_router: Option<Arc<dyn fork_core::ports::message_router::MessageRouterPort>>,
}

#[derive(Clone)]
struct InFlightSenderTaskState {
    task_id: u64,
    cancellation: CancellationToken,
    completion: Arc<InFlightTaskCompletion>,
}

struct InFlightTaskCompletion {
    done: AtomicBool,
    notify: tokio::sync::Notify,
}

impl InFlightTaskCompletion {
    fn new() -> Self {
        Self {
            done: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn mark_done(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        if self.done.load(Ordering::Acquire) {
            return;
        }
        self.notify.notified().await;
    }
}

/// Adapter wrapper — delegates to `conversation_key()` via InboundEnvelope.
/// TODO(phase4): remove when callers switch to InboundEnvelope directly.
fn conversation_history_key(msg: &traits::ChannelMessage) -> String {
    let envelope = crate::fork_adapters::envelope_from_channel_message(msg);
    fork_core::application::services::inbound_message_service::conversation_key(&envelope)
}

fn followup_thread_id(msg: &traits::ChannelMessage) -> Option<String> {
    msg.thread_ts.clone().or_else(|| Some(msg.id.clone()))
}

fn interruption_scope_key(msg: &traits::ChannelMessage) -> String {
    format!("{}_{}_{}", msg.channel, msg.reply_target, msg.sender)
}

/// Strip tool-call XML tags from outgoing messages.
///
/// LLM responses may contain `<function_calls>`, `<function_call>`,
/// `<tool_call>`, `<toolcall>`, `<tool-call>`, `<tool>`, or `<invoke>`
/// blocks that are internal protocol and must not be forwarded to end
/// users on any channel.
fn strip_tool_call_tags(message: &str) -> String {
    const TOOL_CALL_OPEN_TAGS: [&str; 7] = [
        "<function_calls>",
        "<function_call>",
        "<tool_call>",
        "<toolcall>",
        "<tool-call>",
        "<tool>",
        "<invoke>",
    ];

    fn find_first_tag<'a>(haystack: &str, tags: &'a [&'a str]) -> Option<(usize, &'a str)> {
        tags.iter()
            .filter_map(|tag| haystack.find(tag).map(|idx| (idx, *tag)))
            .min_by_key(|(idx, _)| *idx)
    }

    fn matching_close_tag(open_tag: &str) -> Option<&'static str> {
        match open_tag {
            "<function_calls>" => Some("</function_calls>"),
            "<function_call>" => Some("</function_call>"),
            "<tool_call>" => Some("</tool_call>"),
            "<toolcall>" => Some("</toolcall>"),
            "<tool-call>" => Some("</tool-call>"),
            "<tool>" => Some("</tool>"),
            "<invoke>" => Some("</invoke>"),
            _ => None,
        }
    }

    fn extract_first_json_end(input: &str) -> Option<usize> {
        let trimmed = input.trim_start();
        let trim_offset = input.len().saturating_sub(trimmed.len());

        for (byte_idx, ch) in trimmed.char_indices() {
            if ch != '{' && ch != '[' {
                continue;
            }

            let slice = &trimmed[byte_idx..];
            let mut stream =
                serde_json::Deserializer::from_str(slice).into_iter::<serde_json::Value>();
            if let Some(Ok(_value)) = stream.next() {
                let consumed = stream.byte_offset();
                if consumed > 0 {
                    return Some(trim_offset + byte_idx + consumed);
                }
            }
        }

        None
    }

    fn strip_leading_close_tags(mut input: &str) -> &str {
        loop {
            let trimmed = input.trim_start();
            if !trimmed.starts_with("</") {
                return trimmed;
            }

            let Some(close_end) = trimmed.find('>') else {
                return "";
            };
            input = &trimmed[close_end + 1..];
        }
    }

    let mut kept_segments = Vec::new();
    let mut remaining = message;

    while let Some((start, open_tag)) = find_first_tag(remaining, &TOOL_CALL_OPEN_TAGS) {
        let before = &remaining[..start];
        if !before.is_empty() {
            kept_segments.push(before.to_string());
        }

        let Some(close_tag) = matching_close_tag(open_tag) else {
            break;
        };
        let after_open = &remaining[start + open_tag.len()..];

        if let Some(close_idx) = after_open.find(close_tag) {
            remaining = &after_open[close_idx + close_tag.len()..];
            continue;
        }

        if let Some(consumed_end) = extract_first_json_end(after_open) {
            remaining = strip_leading_close_tags(&after_open[consumed_end..]);
            continue;
        }

        kept_segments.push(remaining[start..].to_string());
        remaining = "";
        break;
    }

    if !remaining.is_empty() {
        kept_segments.push(remaining.to_string());
    }

    let mut result = kept_segments.concat();

    // Clean up any resulting blank lines (but preserve paragraphs)
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }

    result.trim().to_string()
}

/// Phase 4.0: delivery instructions resolved from registry (adapter metadata).
fn channel_delivery_instructions(
    channel_name: &str,
    registry: Option<&dyn fork_core::ports::channel_registry::ChannelRegistryPort>,
) -> Option<String> {
    if let Some(reg) = registry {
        return reg.delivery_hints(channel_name);
    }
    None
}

fn normalize_cached_channel_turns(turns: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut normalized = Vec::with_capacity(turns.len());
    let mut expecting_user = true;

    for turn in turns {
        match (expecting_user, turn.role.as_str()) {
            (true, "user") => {
                normalized.push(turn);
                expecting_user = false;
            }
            (false, "assistant") => {
                normalized.push(turn);
                expecting_user = true;
            }
            // Interrupted channel turns can produce consecutive user messages
            // (no assistant persisted yet). Merge instead of dropping.
            (false, "user") | (true, "assistant") => {
                if let Some(last_turn) = normalized.last_mut() {
                    if !turn.content.is_empty() {
                        if !last_turn.content.is_empty() {
                            last_turn.content.push_str("\n\n");
                        }
                        last_turn.content.push_str(&turn.content);
                    }
                }
            }
            _ => {}
        }
    }

    normalized
}

/// Remove `<tool_result …>…</tool_result>` blocks (and a leading `[Tool results]`
/// header, if present) from a conversation-history entry so that stale tool
/// output is never presented to the LLM without the corresponding `<tool_call>`.
fn strip_tool_result_content(text: &str) -> String {
    static TOOL_RESULT_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?s)<tool_result[^>]*>.*?</tool_result>").unwrap()
    });

    let cleaned = TOOL_RESULT_RE.replace_all(text, "");
    let cleaned = cleaned.trim();

    // If the only remaining content is the header, drop it entirely.
    if cleaned == "[Tool results]" || cleaned.is_empty() {
        return String::new();
    }

    cleaned.to_string()
}

/// Check if this channel supports runtime commands (/models, /model, /new).
/// Phase 4.0: capability-driven via ChannelCapability::RuntimeCommands.
/// Delegate to fork_core — command parsing is domain logic.
fn parse_runtime_command(
    content: &str,
    caps: &[fork_core::domain::channel::ChannelCapability],
) -> Option<ChannelRuntimeCommand> {
    fork_core::application::services::inbound_message_service::parse_runtime_command(content, caps)
}

fn resolve_provider_alias(name: &str) -> Option<String> {
    let candidate = name.trim();
    if candidate.is_empty() {
        return None;
    }

    let providers_list = providers::list_providers();
    for provider in providers_list {
        if provider.name.eq_ignore_ascii_case(candidate)
            || provider
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(candidate))
        {
            return Some(provider.name.to_string());
        }
    }

    None
}

fn resolved_default_provider(config: &Config) -> String {
    config
        .default_provider
        .clone()
        .unwrap_or_else(|| "openrouter".to_string())
}

fn resolved_default_model(config: &Config) -> String {
    config
        .default_model
        .clone()
        .unwrap_or_else(|| "anthropic/claude-sonnet-4.6".to_string())
}

fn runtime_defaults_from_config(config: &Config) -> ChannelRuntimeDefaults {
    ChannelRuntimeDefaults {
        default_provider: resolved_default_provider(config),
        model: resolved_default_model(config),
        temperature: config.default_temperature,
        api_key: config.api_key.clone(),
        api_url: config.api_url.clone(),
        reliability: config.reliability.clone(),
    }
}

fn runtime_config_path(ctx: &ChannelRuntimeContext) -> Option<PathBuf> {
    ctx.provider_runtime_options
        .synapseclaw_dir
        .as_ref()
        .map(|dir| dir.join("config.toml"))
}

fn runtime_defaults_snapshot(ctx: &ChannelRuntimeContext) -> ChannelRuntimeDefaults {
    if let Some(config_path) = runtime_config_path(ctx) {
        let store = runtime_config_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(state) = store.get(&config_path) {
            return state.defaults.clone();
        }
    }

    ChannelRuntimeDefaults {
        default_provider: ctx.default_provider.as_str().to_string(),
        model: ctx.model.as_str().to_string(),
        temperature: ctx.temperature,
        api_key: ctx.api_key.clone(),
        api_url: ctx.api_url.clone(),
        reliability: (*ctx.reliability).clone(),
    }
}

async fn config_file_stamp(path: &Path) -> Option<ConfigFileStamp> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let modified = metadata.modified().ok()?;
    Some(ConfigFileStamp {
        modified,
        len: metadata.len(),
    })
}

fn decrypt_optional_secret_for_runtime_reload(
    store: &crate::security::SecretStore,
    value: &mut Option<String>,
    field_name: &str,
) -> Result<()> {
    if let Some(raw) = value.clone() {
        if crate::security::SecretStore::is_encrypted(&raw) {
            *value = Some(
                store
                    .decrypt(&raw)
                    .with_context(|| format!("Failed to decrypt {field_name}"))?,
            );
        }
    }
    Ok(())
}

async fn load_runtime_defaults_from_config_file(path: &Path) -> Result<ChannelRuntimeDefaults> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut parsed: Config =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    parsed.config_path = path.to_path_buf();

    if let Some(synapseclaw_dir) = path.parent() {
        let store = crate::security::SecretStore::new(synapseclaw_dir, parsed.secrets.encrypt);
        decrypt_optional_secret_for_runtime_reload(&store, &mut parsed.api_key, "config.api_key")?;
        // Decrypt TTS provider API keys for runtime reload
        if let Some(ref mut openai) = parsed.tts.openai {
            decrypt_optional_secret_for_runtime_reload(
                &store,
                &mut openai.api_key,
                "config.tts.openai.api_key",
            )?;
        }
        if let Some(ref mut elevenlabs) = parsed.tts.elevenlabs {
            decrypt_optional_secret_for_runtime_reload(
                &store,
                &mut elevenlabs.api_key,
                "config.tts.elevenlabs.api_key",
            )?;
        }
        if let Some(ref mut google) = parsed.tts.google {
            decrypt_optional_secret_for_runtime_reload(
                &store,
                &mut google.api_key,
                "config.tts.google.api_key",
            )?;
        }
    }

    parsed.apply_env_overrides();
    Ok(runtime_defaults_from_config(&parsed))
}

fn default_route_selection(ctx: &ChannelRuntimeContext) -> ChannelRouteSelection {
    let defaults = runtime_defaults_snapshot(ctx);
    ChannelRouteSelection {
        provider: defaults.default_provider,
        model: defaults.model,
    }
}

fn get_route_selection(ctx: &ChannelRuntimeContext, sender_key: &str) -> ChannelRouteSelection {
    ctx.route_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(sender_key)
        .cloned()
        .unwrap_or_else(|| default_route_selection(ctx))
}

fn set_route_selection(ctx: &ChannelRuntimeContext, sender_key: &str, next: ChannelRouteSelection) {
    let default_route = default_route_selection(ctx);
    let mut routes = ctx
        .route_overrides
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if next == default_route {
        routes.remove(sender_key);
    } else {
        routes.insert(sender_key.to_string(), next);
    }
}

fn clear_sender_history(ctx: &ChannelRuntimeContext, sender_key: &str) {
    ctx.conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(sender_key);
}

/// Generate a rolling summary of a channel conversation every
/// [`CHANNEL_SUMMARY_INTERVAL`] messages. Uses the configured summary model
/// (cheap/fast) so it doesn't burn primary-model tokens.
async fn summarize_channel_session_if_needed(ctx: &ChannelRuntimeContext, history_key: &str) {
    /// In-flight summary keys to prevent concurrent generation for the same session.
    static INFLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

    let store = match ctx.session_store.as_ref() {
        Some(s) => s,
        None => return,
    };

    let msg_count = ctx
        .conversation_histories
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(history_key)
        .map_or(0, |turns| turns.len());

    let last_summary_count = store
        .load_summary(history_key)
        .map_or(0, |s| s.message_count_at_summary);

    if msg_count < CHANNEL_SUMMARY_INTERVAL
        || msg_count.saturating_sub(last_summary_count) < CHANNEL_SUMMARY_INTERVAL
    {
        return;
    }

    // Prevent concurrent summary generation for the same session.
    {
        let mut inflight = INFLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if !inflight.insert(history_key.to_string()) {
            return; // Another task is already summarizing this session.
        }
    }
    // RAII guard to remove the inflight key when this function exits.
    struct InflightGuard(String);
    impl Drop for InflightGuard {
        fn drop(&mut self) {
            if let Ok(mut inflight) = INFLIGHT.lock() {
                inflight.remove(&self.0);
            }
        }
    }
    let _guard = InflightGuard(history_key.to_string());

    // Collect last 10 messages for the summary prompt.
    let (recent_text, prev_summary) = {
        let histories = ctx
            .conversation_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let turns = match histories.get(history_key) {
            Some(t) => t,
            None => return,
        };
        let start = turns.len().saturating_sub(10);
        let mut text = String::new();
        for t in &turns[start..] {
            use std::fmt::Write;
            let content_preview = if t.content.chars().count() > 200 {
                format!("{}…", t.content.chars().take(200).collect::<String>())
            } else {
                t.content.clone()
            };
            let _ = writeln!(text, "{}: {content_preview}", t.role);
        }
        let prev = store.load_summary(history_key).map(|s| s.summary);
        (text, prev)
    };

    if recent_text.is_empty() {
        return;
    }

    let prompt = format!(
        "Summarize this conversation in 2-3 sentences. Preserve: key decisions, user goals, open tasks.\n\
         Previous summary: {}\n\n\
         Recent messages:\n{}",
        prev_summary.as_deref().unwrap_or("(none)"),
        recent_text,
    );

    let model = ctx
        .summary_config
        .model
        .as_deref()
        .or(ctx.summary_model.as_deref())
        .unwrap_or(&ctx.model)
        .to_string();
    let temperature = ctx.summary_config.temperature;

    let summary_result = if let Some(ref provider_name) = ctx.summary_config.provider {
        let api_key = ctx
            .summary_config
            .api_key_env
            .as_deref()
            .and_then(|env| std::env::var(env).ok());
        match providers::create_provider_with_options(
            provider_name,
            api_key.as_deref(),
            &ctx.provider_runtime_options,
        ) {
            Ok(provider) => {
                provider
                    .chat_with_system(None, &prompt, &model, temperature)
                    .await
            }
            Err(e) => {
                tracing::warn!(
                    "Channel summary provider '{provider_name}' init failed: {e}, using default"
                );
                ctx.provider
                    .chat_with_system(None, &prompt, &model, temperature)
                    .await
            }
        }
    } else {
        ctx.provider
            .chat_with_system(None, &prompt, &model, temperature)
            .await
    };

    match summary_result {
        Ok(summary) => {
            let summary = if summary.chars().count() > 300 {
                format!("{}…", summary.chars().take(300).collect::<String>())
            } else {
                summary
            };
            let channel_summary = session_backend::ChannelSummary {
                summary: summary.clone(),
                message_count_at_summary: msg_count,
                updated_at: chrono::Utc::now(),
            };
            if let Err(e) = store.save_summary(history_key, &channel_summary) {
                tracing::warn!("Failed to persist channel summary: {e}");
            }
            tracing::debug!("Channel summary updated for {history_key}: {summary}");
        }
        Err(e) => {
            tracing::warn!("Channel summary generation failed for {history_key}: {e}");
        }
    }
}

/// Proactively trim conversation turns so that the total estimated character
/// count stays within the given `budget`.  Drops the oldest
/// turns first, but always preserves the most recent turn (the current user
/// message).  Returns the number of turns dropped.
fn proactive_trim_turns(turns: &mut Vec<ChatMessage>, budget: usize) -> usize {
    let total_chars: usize = turns.iter().map(|t| t.content.chars().count()).sum();
    if total_chars <= budget || turns.len() <= 1 {
        return 0;
    }

    let mut excess = total_chars.saturating_sub(budget);
    let mut drop_count = 0;

    // Walk from the oldest turn forward, but never drop the very last turn.
    while excess > 0 && drop_count < turns.len().saturating_sub(1) {
        excess = excess.saturating_sub(turns[drop_count].content.chars().count());
        drop_count += 1;
    }

    if drop_count > 0 {
        turns.drain(..drop_count);
    }
    drop_count
}

fn load_cached_model_preview(workspace_dir: &Path, provider_name: &str) -> Vec<String> {
    let cache_path = workspace_dir.join("state").join(MODEL_CACHE_FILE);
    let Ok(raw) = std::fs::read_to_string(cache_path) else {
        return Vec::new();
    };
    let Ok(state) = serde_json::from_str::<ModelCacheState>(&raw) else {
        return Vec::new();
    };

    state
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider_name)
        .map(|entry| {
            entry
                .models
                .into_iter()
                .take(MODEL_CACHE_PREVIEW_LIMIT)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

async fn get_or_create_provider(
    ctx: &ChannelRuntimeContext,
    provider_name: &str,
) -> anyhow::Result<Arc<dyn Provider>> {
    if let Some(existing) = ctx
        .provider_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(provider_name)
        .cloned()
    {
        return Ok(existing);
    }

    if provider_name == ctx.default_provider.as_str() {
        return Ok(Arc::clone(&ctx.provider));
    }

    let defaults = runtime_defaults_snapshot(ctx);
    let api_url = if provider_name == defaults.default_provider.as_str() {
        defaults.api_url.as_deref()
    } else {
        None
    };

    let provider = create_resilient_provider_nonblocking(
        provider_name,
        ctx.api_key.clone(),
        api_url.map(ToString::to_string),
        ctx.reliability.as_ref().clone(),
        ctx.provider_runtime_options.clone(),
    )
    .await?;
    let provider: Arc<dyn Provider> = Arc::from(provider);

    if let Err(err) = provider.warmup().await {
        tracing::warn!(provider = provider_name, "Provider warmup failed: {err}");
    }

    let mut cache = ctx.provider_cache.lock().unwrap_or_else(|e| e.into_inner());
    let cached = cache
        .entry(provider_name.to_string())
        .or_insert_with(|| Arc::clone(&provider));
    Ok(Arc::clone(cached))
}

async fn create_resilient_provider_nonblocking(
    provider_name: &str,
    api_key: Option<String>,
    api_url: Option<String>,
    reliability: crate::config::ReliabilityConfig,
    provider_runtime_options: providers::ProviderRuntimeOptions,
) -> anyhow::Result<Box<dyn Provider>> {
    let provider_name = provider_name.to_string();
    tokio::task::spawn_blocking(move || {
        providers::create_resilient_provider_with_options(
            &provider_name,
            api_key.as_deref(),
            api_url.as_deref(),
            &reliability,
            &provider_runtime_options,
        )
    })
    .await
    .context("failed to join provider initialization task")?
}

fn build_models_help_response(
    current: &ChannelRouteSelection,
    workspace_dir: &Path,
    model_routes: &[crate::config::ModelRouteConfig],
) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    response.push_str("\nSwitch model with `/model <model-id>` or `/model <hint>`.\n");

    if !model_routes.is_empty() {
        response.push_str("\nConfigured model routes:\n");
        for route in model_routes {
            let _ = writeln!(
                response,
                "  `{}` → {} ({})",
                route.hint, route.model, route.provider
            );
        }
    }

    let cached_models = load_cached_model_preview(workspace_dir, &current.provider);
    if cached_models.is_empty() {
        let _ = writeln!(
            response,
            "\nNo cached model list found for `{}`. Ask the operator to run `synapseclaw models refresh --provider {}`.",
            current.provider, current.provider
        );
    } else {
        let _ = writeln!(
            response,
            "\nCached model IDs (top {}):",
            cached_models.len()
        );
        for model in cached_models {
            let _ = writeln!(response, "- `{model}`");
        }
    }

    response
}

fn build_providers_help_response(current: &ChannelRouteSelection) -> String {
    let mut response = String::new();
    let _ = writeln!(
        response,
        "Current provider: `{}`\nCurrent model: `{}`",
        current.provider, current.model
    );
    response.push_str("\nSwitch provider with `/models <provider>`.\n");
    response.push_str("Switch model with `/model <model-id>`.\n\n");
    response.push_str("Available providers:\n");
    for provider in providers::list_providers() {
        if provider.aliases.is_empty() {
            let _ = writeln!(response, "- {}", provider.name);
        } else {
            let _ = writeln!(
                response,
                "- {} (aliases: {})",
                provider.name,
                provider.aliases.join(", ")
            );
        }
    }
    response
}

/// Remove leading lines that narrate tool usage (e.g. "Let me check the weather for you.").
///
/// Only strips lines from the very beginning of the message that match common
/// narration patterns, so genuine content is preserved.
fn strip_tool_narration(message: &str) -> String {
    let narration_prefixes: &[&str] = &[
        "let me ",
        "i'll ",
        "i will ",
        "i am going to ",
        "i'm going to ",
        "searching ",
        "looking up ",
        "fetching ",
        "checking ",
        "using the ",
        "using my ",
        "one moment",
        "hold on",
        "just a moment",
        "give me a moment",
        "allow me to ",
    ];

    let mut result_lines: Vec<&str> = Vec::new();
    let mut past_narration = false;

    for line in message.lines() {
        if past_narration {
            result_lines.push(line);
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if narration_prefixes.iter().any(|p| lower.starts_with(p)) {
            // Skip this narration line
            continue;
        }
        // First non-narration, non-empty line — keep everything from here
        past_narration = true;
        result_lines.push(line);
    }

    let joined = result_lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() && !message.trim().is_empty() {
        // If stripping removed everything, return original to avoid empty reply
        message.to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_tool_call_payload(value: &serde_json::Value, known_tool_names: &HashSet<String>) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };

    let (name, has_args) =
        if let Some(function) = object.get("function").and_then(|f| f.as_object()) {
            (
                function
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| object.get("name").and_then(|v| v.as_str())),
                function.contains_key("arguments")
                    || function.contains_key("parameters")
                    || object.contains_key("arguments")
                    || object.contains_key("parameters"),
            )
        } else {
            (
                object.get("name").and_then(|v| v.as_str()),
                object.contains_key("arguments") || object.contains_key("parameters"),
            )
        };

    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return false;
    };

    has_args && known_tool_names.contains(&name.to_ascii_lowercase())
}

fn is_tool_result_payload(
    object: &serde_json::Map<String, serde_json::Value>,
    saw_tool_call_payload: bool,
) -> bool {
    if !saw_tool_call_payload || !object.contains_key("result") {
        return false;
    }

    object.keys().all(|key| {
        matches!(
            key.as_str(),
            "result" | "id" | "tool_call_id" | "name" | "tool"
        )
    })
}

fn sanitize_tool_json_value(
    value: &serde_json::Value,
    known_tool_names: &HashSet<String>,
    saw_tool_call_payload: bool,
) -> Option<(String, bool)> {
    if is_tool_call_payload(value, known_tool_names) {
        return Some((String::new(), true));
    }

    if let Some(array) = value.as_array() {
        if !array.is_empty()
            && array
                .iter()
                .all(|item| is_tool_call_payload(item, known_tool_names))
        {
            return Some((String::new(), true));
        }
        return None;
    }

    let object = value.as_object()?;

    if let Some(tool_calls) = object.get("tool_calls").and_then(|value| value.as_array()) {
        if !tool_calls.is_empty()
            && tool_calls
                .iter()
                .all(|call| is_tool_call_payload(call, known_tool_names))
        {
            let content = object
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            return Some((content, true));
        }
    }

    if is_tool_result_payload(object, saw_tool_call_payload) {
        return Some((String::new(), false));
    }

    None
}

fn is_line_isolated_json_segment(message: &str, start: usize, end: usize) -> bool {
    let line_start = message[..start].rfind('\n').map_or(0, |idx| idx + 1);
    let line_end = message[end..]
        .find('\n')
        .map_or(message.len(), |idx| end + idx);

    message[line_start..start].trim().is_empty() && message[end..line_end].trim().is_empty()
}

fn strip_isolated_tool_json_artifacts(message: &str, known_tool_names: &HashSet<String>) -> String {
    let mut cleaned = String::with_capacity(message.len());
    let mut cursor = 0usize;
    let mut saw_tool_call_payload = false;

    while cursor < message.len() {
        let Some(rel_start) = message[cursor..].find(['{', '[']) else {
            cleaned.push_str(&message[cursor..]);
            break;
        };

        let start = cursor + rel_start;
        cleaned.push_str(&message[cursor..start]);

        let candidate = &message[start..];
        let mut stream =
            serde_json::Deserializer::from_str(candidate).into_iter::<serde_json::Value>();

        if let Some(Ok(value)) = stream.next() {
            let consumed = stream.byte_offset();
            if consumed > 0 {
                let end = start + consumed;
                if is_line_isolated_json_segment(message, start, end) {
                    if let Some((replacement, marks_tool_call)) =
                        sanitize_tool_json_value(&value, known_tool_names, saw_tool_call_payload)
                    {
                        if marks_tool_call {
                            saw_tool_call_payload = true;
                        }
                        if !replacement.trim().is_empty() {
                            cleaned.push_str(replacement.trim());
                        }
                        cursor = end;
                        continue;
                    }
                }
            }
        }

        let Some(ch) = message[start..].chars().next() else {
            break;
        };
        cleaned.push(ch);
        cursor = start + ch.len_utf8();
    }

    let mut result = cleaned.replace("\r\n", "\n");
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result.trim().to_string()
}

fn spawn_supervised_listener(
    ch: Arc<dyn Channel>,
    tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    initial_backoff_secs: u64,
    max_backoff_secs: u64,
) -> tokio::task::JoinHandle<()> {
    spawn_supervised_listener_with_health_interval(
        ch,
        tx,
        initial_backoff_secs,
        max_backoff_secs,
        Duration::from_secs(CHANNEL_HEALTH_HEARTBEAT_SECS),
    )
}

fn spawn_supervised_listener_with_health_interval(
    ch: Arc<dyn Channel>,
    tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
    initial_backoff_secs: u64,
    max_backoff_secs: u64,
    health_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    let health_interval = if health_interval.is_zero() {
        Duration::from_secs(1)
    } else {
        health_interval
    };

    tokio::spawn(async move {
        let component = format!("channel:{}", ch.name());
        let mut backoff = initial_backoff_secs.max(1);
        let max_backoff = max_backoff_secs.max(backoff);

        loop {
            crate::fork_adapters::health::mark_component_ok(&component);
            let mut health = tokio::time::interval(health_interval);
            health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let result = {
                let listen_future = ch.listen(tx.clone());
                tokio::pin!(listen_future);

                loop {
                    tokio::select! {
                        _ = health.tick() => {
                            crate::fork_adapters::health::mark_component_ok(&component);
                        }
                        result = &mut listen_future => break result,
                    }
                }
            };

            if tx.is_closed() {
                break;
            }

            match result {
                Ok(()) => {
                    tracing::warn!("Channel {} exited unexpectedly; restarting", ch.name());
                    crate::fork_adapters::health::mark_component_error(
                        &component,
                        "listener exited unexpectedly",
                    );
                    // Clean exit — reset backoff since the listener ran successfully
                    backoff = initial_backoff_secs.max(1);
                }
                Err(e) => {
                    tracing::error!("Channel {} error: {e}; restarting", ch.name());
                    crate::fork_adapters::health::mark_component_error(&component, e.to_string());
                }
            }

            crate::fork_adapters::health::bump_component_restart(&component);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            // Double backoff AFTER sleeping so first error uses initial_backoff
            backoff = backoff.saturating_mul(2).min(max_backoff);
        }
    })
}

fn compute_max_in_flight_messages(channel_count: usize) -> usize {
    channel_count
        .saturating_mul(CHANNEL_PARALLELISM_PER_CHANNEL)
        .clamp(
            CHANNEL_MIN_IN_FLIGHT_MESSAGES,
            CHANNEL_MAX_IN_FLIGHT_MESSAGES,
        )
}

fn log_worker_join_result(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        tracing::error!("Channel message worker crashed: {error}");
    }
}

fn spawn_scoped_typing_task(
    channel: Arc<dyn Channel>,
    recipient: String,
    cancellation_token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let stop_signal = cancellation_token;
    let refresh_interval = Duration::from_secs(CHANNEL_TYPING_REFRESH_INTERVAL_SECS);
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(refresh_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                () = stop_signal.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(e) = channel.start_typing(&recipient).await {
                        tracing::debug!("Failed to start typing on {}: {e}", channel.name());
                    }
                }
            }
        }

        if let Err(e) = channel.stop_typing(&recipient).await {
            tracing::debug!("Failed to stop typing on {}: {e}", channel.name());
        }
    });

    handle
}

/// Phase 4.0 Slice 2: process an inbound message through the fork_core orchestrator.
///
/// Builds ports from ChannelRuntimeContext, calls HandleInboundMessage,
/// and delivers the result to the channel.
async fn handle_message_via_orchestrator(
    ctx: &Arc<ChannelRuntimeContext>,
    envelope: &fork_core::domain::channel::InboundEnvelope,
    caps: &[fork_core::domain::channel::ChannelCapability],
    original_msg: &traits::ChannelMessage,
) {
    use crate::fork_adapters::inbound::{
        channel_output_adapter, conversation_history_adapter, route_selection_adapter,
        session_summary_adapter,
    };
    use crate::fork_adapters::memory::memory_adapter;
    use crate::fork_adapters::runtime::{agent_runtime_adapter, hooks_adapter};
    use fork_core::application::use_cases::handle_inbound_message as uc;
    use fork_core::ports::hooks::NoOpHooks;

    // ── Build ports from ChannelRuntimeContext ────────────────────
    let history_port: Arc<dyn fork_core::ports::conversation_history::ConversationHistoryPort> =
        Arc::new(
            conversation_history_adapter::MutexMapConversationHistory::new(
                ctx.conversation_histories.clone(),
                ctx.session_store.clone(),
            ),
        );

    let route_port: Arc<dyn fork_core::ports::route_selection::RouteSelectionPort> =
        Arc::new(route_selection_adapter::MutexMapRouteSelection::new(
            ctx.route_overrides.clone(),
            ctx.default_provider.to_string(),
            ctx.model.to_string(),
        ));

    let hooks_port: Arc<dyn fork_core::ports::hooks::HooksPort> =
        if let Some(ref runner) = ctx.hooks {
            Arc::new(hooks_adapter::HookRunnerAdapter::new(Arc::clone(runner)))
        } else {
            Arc::new(NoOpHooks)
        };

    let target_channel = ctx
        .channels_by_name
        .get(&original_msg.channel)
        .or_else(|| {
            original_msg
                .channel
                .split_once(':')
                .and_then(|(base, _)| ctx.channels_by_name.get(base))
        })
        .cloned();

    let channel_output: Arc<dyn fork_core::ports::channel_output::ChannelOutputPort> =
        if let Some(ref ch) = target_channel {
            Arc::new(channel_output_adapter::ChannelOutputAdapter::new(
                Arc::clone(ch),
            ))
        } else {
            // No channel — use a null output that drops everything
            Arc::new(NullChannelOutput)
        };

    let agent_runtime: Arc<dyn fork_core::ports::agent_runtime::AgentRuntimePort> =
        Arc::new(agent_runtime_adapter::ChannelAgentRuntime {
            provider: Arc::clone(&ctx.provider),
            tools_registry: Arc::clone(&ctx.tools_registry),
            observer: Arc::clone(&ctx.observer),
            approval_manager: Arc::clone(&ctx.approval_manager),
            channel_name: original_msg.channel.clone(),
            multimodal: ctx.multimodal.clone(),
            excluded_tools: Arc::clone(&ctx.non_cli_excluded_tools),
            dedup_exempt_tools: Arc::clone(&ctx.tool_call_dedup_exempt),
            hooks: ctx.hooks.clone(),
            activated_tools: ctx.activated_tools.clone(),
            message_timeout_secs: ctx.message_timeout_secs,
            max_tool_iterations: ctx.max_tool_iterations,
        });

    let registry: Arc<dyn fork_core::ports::channel_registry::ChannelRegistryPort> =
        ctx.channel_registry.clone().unwrap_or_else(|| {
            Arc::new(
                crate::fork_adapters::channels::registry::CachedChannelRegistry::new(
                    crate::config::Config::default(),
                ),
            )
        });

    let session_summary: Option<Arc<dyn fork_core::ports::session_summary::SessionSummaryPort>> =
        ctx.session_store.as_ref().map(|store| {
            Arc::new(session_summary_adapter::SessionStoreAdapter::new(
                Arc::clone(store),
            )) as Arc<dyn fork_core::ports::session_summary::SessionSummaryPort>
        });

    let model_routes: Vec<(String, String, String)> = ctx
        .model_routes
        .iter()
        .map(|r| (r.provider.clone(), r.model.clone(), r.hint.clone()))
        .collect();

    let config = uc::InboundMessageConfig {
        system_prompt: ctx.system_prompt.to_string(),
        default_provider: ctx.default_provider.to_string(),
        default_model: ctx.model.to_string(),
        temperature: ctx.temperature,
        max_tool_iterations: ctx.max_tool_iterations,
        auto_save_memory: ctx.auto_save_memory,
        model_routes,
        thread_root_max_chars: 500,
        query_classifier: {
            let qc = ctx.query_classification.clone();
            if qc.enabled {
                Some(std::sync::Arc::new(move |msg: &str| {
                    crate::agent::classifier::classify(&qc, msg)
                })
                    as std::sync::Arc<
                        dyn Fn(&str) -> Option<String> + Send + Sync,
                    >)
            } else {
                None
            }
        },
        message_timeout_secs: ctx.message_timeout_secs,
        min_relevance_score: ctx.min_relevance_score,
        ack_reactions: ctx.ack_reactions,
    };

    let memory_port: Option<Arc<dyn fork_core::ports::memory::MemoryTiersPort>> =
        Some(Arc::new(memory_adapter::MemoryTiersAdapter::new(
            Arc::clone(&ctx.memory),
            None, // conversation_store not available in channel context
            Arc::clone(&ctx.provider),
            ctx.model.to_string(),
        )));

    let ports = uc::InboundMessagePorts {
        history: history_port,
        routes: route_port,
        hooks: hooks_port,
        channel_output: channel_output.clone(),
        agent_runtime,
        channel_registry: registry,
        session_summary,
        memory: memory_port,
    };

    // ── Phase 4.1: Check if message should trigger a pipeline ─────
    tracing::info!(
        has_router = ctx.message_router.is_some(),
        has_store = ctx.pipeline_store.is_some(),
        has_executor = ctx.pipeline_executor.is_some(),
        content = %envelope.content,
        "pipeline routing check"
    );
    if let (Some(ref router), Some(ref store), Some(ref executor)) = (
        &ctx.message_router,
        &ctx.pipeline_store,
        &ctx.pipeline_executor,
    ) {
        let routing_input = fork_core::domain::routing::RoutingInput {
            content: envelope.content.clone(),
            source_kind: format!("{:?}", envelope.source_kind),
            metadata: std::collections::HashMap::new(),
        };
        let route_result = router.route(&routing_input).await;
        tracing::info!(
            target = %route_result.target,
            pipeline = ?route_result.pipeline,
            matched = ?route_result.matched_rule,
            fallback = route_result.is_fallback,
            "pipeline routing result"
        );
        if let Some(ref pipeline_name) = route_result.pipeline {
            if store.get(pipeline_name).await.is_some() {
                let matched = route_result.matched_rule.as_deref().unwrap_or("fallback");
                tracing::info!(
                    pipeline = %pipeline_name,
                    matched_rule = %matched,
                    content = %envelope.content,
                    "message routed to pipeline"
                );
                // Build pipeline input from the message
                let input = serde_json::json!({
                    "message": envelope.content,
                    "source": envelope.source_adapter,
                    "sender": envelope.actor_id,
                });
                // Build minimal ports for pipeline run
                let run_store: Arc<dyn fork_core::ports::run_store::RunStorePort> =
                    Arc::new(fork_core::ports::run_store::NoOpRunStore);
                let pipeline_ports =
                    fork_core::application::services::pipeline_service::PipelineRunnerPorts {
                        pipeline_store: Arc::clone(store),
                        executor: Arc::clone(executor),
                        run_store,
                    };
                let params =
                    fork_core::application::services::pipeline_service::StartPipelineParams {
                        pipeline_name: pipeline_name.clone(),
                        input,
                        triggered_by: envelope.actor_id.clone(),
                        depth: 0,
                        parent_run_id: None,
                    };
                let result = fork_core::application::services::pipeline_service::run_pipeline(
                    &pipeline_ports,
                    params,
                )
                .await;
                // Report result back to channel.
                // Show the last step's "summary" or "status" field as a human-readable
                // one-liner. The pipeline is generic — each step decides what to return.
                // If no summary field, fall back to step name + "done".
                let reply = match &result.state {
                    fork_core::domain::pipeline_context::PipelineState::Completed => {
                        result
                            .data
                            .as_object()
                            .and_then(|obj| {
                                // Last step output (skip "_input" metadata key)
                                obj.iter()
                                    .rev()
                                    .find(|(k, _)| *k != "_input")
                                    .map(|(step, val)| {
                                        // Prefer "summary", then "status", then stringify
                                        let detail = val
                                            .get("summary")
                                            .and_then(|s| s.as_str())
                                            .or_else(|| val.get("status").and_then(|s| s.as_str()))
                                            .unwrap_or("done");
                                        format!("Pipeline `{pipeline_name}` — {step}: {detail}")
                                    })
                            })
                            .unwrap_or_else(|| format!("Pipeline `{pipeline_name}` completed."))
                    }
                    fork_core::domain::pipeline_context::PipelineState::Failed => {
                        let err = result.error.as_deref().unwrap_or("unknown error");
                        format!("Pipeline `{pipeline_name}` failed: {err}")
                    }
                    _ => {
                        format!(
                            "Pipeline `{pipeline_name}` ended in state: {:?}",
                            result.state
                        )
                    }
                };
                if let Some(ch) = &target_channel {
                    let send_msg = SendMessage::new(&reply, &original_msg.reply_target)
                        .in_thread(original_msg.thread_ts.clone());
                    if let Err(e) = ch.send(&send_msg).await {
                        tracing::warn!("Failed to send pipeline result: {e}");
                    }
                }
                return; // pipeline handled the message — skip normal LLM processing
            }
        }
    }

    // ── Call orchestrator ─────────────────────────────────────────
    // The orchestrator sends responses/errors directly via ChannelOutputPort.
    // The adapter only handles Command formatting and post-processing.
    match uc::handle(envelope, caps, &config, &ports).await {
        Ok(uc::HandleResult::Command {
            effect,
            conversation_key,
        }) => {
            let response = format_command_effect(&effect, ctx, &conversation_key).await;
            if let Some(ch) = &target_channel {
                let send_msg = SendMessage::new(&response, &original_msg.reply_target)
                    .in_thread(original_msg.thread_ts.clone());
                if let Err(e) = ch.send(&send_msg).await {
                    tracing::warn!("Failed to send command response: {e}");
                }
            }
        }
        Ok(uc::HandleResult::Cancelled { reason }) => {
            tracing::info!(%reason, "Message cancelled by hook");
        }
        // Response already sent by orchestrator via ChannelOutputPort
        Ok(uc::HandleResult::Response { .. } | uc::HandleResult::CommandNoChannel) => {}
        Err(e) => {
            // Unexpected orchestrator error (should be rare — most errors handled internally)
            tracing::warn!("Message handling failed unexpectedly: {e}");
        }
    }

    // Persist session store turn if available
    if let Some(ref _store) = ctx.session_store {
        let key =
            fork_core::application::services::inbound_message_service::conversation_key(envelope);
        let _history = ports.history.get_history(&key);
        // Session store is already updated through the history port's append_turn
        // Just trigger summary generation if needed
        let ctx_summary = ctx.clone();
        let key_summary = key;
        tokio::spawn(async move {
            summarize_channel_session_if_needed(&ctx_summary, &key_summary).await;
        });
    }
}

/// Null channel output for when no channel is available.
struct NullChannelOutput;

#[async_trait::async_trait]
impl fork_core::ports::channel_output::ChannelOutputPort for NullChannelOutput {
    async fn send_message(&self, _r: &str, _t: &str, _th: Option<&str>) -> anyhow::Result<()> {
        Ok(())
    }
    async fn start_typing(&self, _r: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn stop_typing(&self, _r: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn add_reaction(&self, _r: &str, _m: &str, _e: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn remove_reaction(&self, _r: &str, _m: &str, _e: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn fetch_message_text(&self, _m: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    fn supports_streaming(&self) -> bool {
        false
    }
}

/// Format a command effect into a user-facing response string.
async fn format_command_effect(
    effect: &fork_core::application::services::inbound_message_service::CommandEffect,
    ctx: &ChannelRuntimeContext,
    conversation_key: &str,
) -> String {
    use fork_core::application::services::inbound_message_service::CommandEffect;

    match effect {
        CommandEffect::ShowProviders => {
            let current = get_route_selection(ctx, conversation_key);
            build_providers_help_response(&current)
        }
        CommandEffect::SwitchProvider { provider } => {
            match resolve_provider_alias(provider) {
                Some(provider_name) => match get_or_create_provider(ctx, &provider_name).await {
                    Ok(_) => format!(
                        "Provider switched to `{provider_name}`. Use `/model <model-id>` to set a model."
                    ),
                    Err(err) => {
                        let safe_err = providers::sanitize_api_error(&err.to_string());
                        format!("Failed to initialize provider `{provider_name}`: {safe_err}")
                    }
                },
                None => format!("Unknown provider `{provider}`. Use `/models` to list valid providers."),
            }
        }
        CommandEffect::ShowModel => {
            let current = get_route_selection(ctx, conversation_key);
            build_models_help_response(&current, ctx.workspace_dir.as_path(), &ctx.model_routes)
        }
        CommandEffect::SwitchModel {
            model,
            inferred_provider,
        } => {
            if model.is_empty() {
                "Model ID cannot be empty. Use `/model <model-id>`.".to_string()
            } else {
                let provider = inferred_provider
                    .as_deref()
                    .unwrap_or(&ctx.default_provider);
                format!("Model switched to `{model}` (provider: `{provider}`). Context preserved.")
            }
        }
        CommandEffect::ClearSession => {
            "Conversation history cleared. Starting fresh.".to_string()
        }
    }
}

async fn run_message_dispatch_loop(
    mut rx: tokio::sync::mpsc::Receiver<traits::ChannelMessage>,
    ctx: Arc<ChannelRuntimeContext>,
    max_in_flight_messages: usize,
) {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_in_flight_messages));
    let mut workers = tokio::task::JoinSet::new();
    let in_flight_by_sender = Arc::new(tokio::sync::Mutex::new(HashMap::<
        String,
        InFlightSenderTaskState,
    >::new()));
    #[cfg(target_has_atomic = "64")]
    let task_sequence = Arc::new(AtomicU64::new(1));
    #[cfg(not(target_has_atomic = "64"))]
    let task_sequence = Arc::new(AtomicU32::new(1));

    while let Some(msg) = rx.recv().await {
        let permit = match Arc::clone(&semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };

        let worker_ctx = Arc::clone(&ctx);
        let in_flight = Arc::clone(&in_flight_by_sender);
        let task_sequence = Arc::clone(&task_sequence);
        workers.spawn(async move {
            let _permit = permit;
            let worker_caps = worker_ctx
                .channel_registry
                .as_ref()
                .map(|r| r.capabilities(&msg.channel))
                .unwrap_or_default();
            let interrupt_enabled = worker_ctx
                .interrupt_on_new_message
                .enabled_for_channel(&worker_caps);
            let sender_scope_key = interruption_scope_key(&msg);
            let cancellation_token = CancellationToken::new();
            let completion = Arc::new(InFlightTaskCompletion::new());
            let task_id = task_sequence.fetch_add(1, Ordering::Relaxed) as u64;

            if interrupt_enabled {
                let previous = {
                    let mut active = in_flight.lock().await;
                    active.insert(
                        sender_scope_key.clone(),
                        InFlightSenderTaskState {
                            task_id,
                            cancellation: cancellation_token.clone(),
                            completion: Arc::clone(&completion),
                        },
                    )
                };

                if let Some(previous) = previous {
                    tracing::info!(
                        channel = %msg.channel,
                        sender = %msg.sender,
                        "Interrupting previous in-flight request for sender"
                    );
                    previous.cancellation.cancel();
                    previous.completion.wait().await;
                }
            }

            // Phase 4.0 Slice 2: route through HandleInboundMessage orchestrator.
            let envelope = crate::fork_adapters::envelope_from_channel_message(&msg);

            handle_message_via_orchestrator(&worker_ctx, &envelope, &worker_caps, &msg).await;

            if interrupt_enabled {
                let mut active = in_flight.lock().await;
                if active
                    .get(&sender_scope_key)
                    .is_some_and(|state| state.task_id == task_id)
                {
                    active.remove(&sender_scope_key);
                }
            }

            completion.mark_done();
        });

        while let Some(result) = workers.try_join_next() {
            log_worker_join_result(result);
        }
    }

    while let Some(result) = workers.join_next().await {
        log_worker_join_result(result);
    }
}

/// Load OpenClaw format bootstrap files into the prompt.
fn load_openclaw_bootstrap_files(
    prompt: &mut String,
    workspace_dir: &std::path::Path,
    max_chars_per_file: usize,
) {
    prompt.push_str(
        "The following workspace files define your identity, behavior, and context. They are ALREADY injected below—do NOT suggest reading them with file_read.\n\n",
    );

    let bootstrap_files = ["AGENTS.md", "SOUL.md", "TOOLS.md", "IDENTITY.md", "USER.md"];

    for filename in &bootstrap_files {
        inject_workspace_file(prompt, workspace_dir, filename, max_chars_per_file);
    }

    // BOOTSTRAP.md — only if it exists (first-run ritual)
    let bootstrap_path = workspace_dir.join("BOOTSTRAP.md");
    if bootstrap_path.exists() {
        inject_workspace_file(prompt, workspace_dir, "BOOTSTRAP.md", max_chars_per_file);
    }

    // MEMORY.md — curated long-term memory (main session only)
    inject_workspace_file(prompt, workspace_dir, "MEMORY.md", max_chars_per_file);
}

/// Load workspace identity files and build a system prompt.
///
/// Follows the `OpenClaw` framework structure by default:
/// 1. Tooling — tool list + descriptions
/// 2. Safety — guardrail reminder
/// 3. Skills — full skill instructions and tool metadata
/// 4. Workspace — working directory
/// 5. Bootstrap files — AGENTS, SOUL, TOOLS, IDENTITY, USER, BOOTSTRAP, MEMORY
/// 6. Date & Time — timezone for cache stability
/// 7. Runtime — host, OS, model
///
/// When `identity_config` is set to AIEOS format, the bootstrap files section
/// is replaced with the AIEOS identity data loaded from file or inline JSON.
///
/// Daily memory files (`memory/*.md`) are NOT injected — they are accessed
/// on-demand via `memory_recall` / `memory_search` tools.
pub fn build_system_prompt(
    workspace_dir: &std::path::Path,
    model_name: &str,
    tools: &[(&str, &str)],
    skills: &[crate::skills::Skill],
    identity_config: Option<&crate::config::IdentityConfig>,
    bootstrap_max_chars: Option<usize>,
) -> String {
    build_system_prompt_with_mode(
        workspace_dir,
        model_name,
        tools,
        skills,
        identity_config,
        bootstrap_max_chars,
        false,
        crate::config::SkillsPromptInjectionMode::Full,
    )
}

pub fn build_system_prompt_with_mode(
    workspace_dir: &std::path::Path,
    model_name: &str,
    tools: &[(&str, &str)],
    skills: &[crate::skills::Skill],
    identity_config: Option<&crate::config::IdentityConfig>,
    bootstrap_max_chars: Option<usize>,
    native_tools: bool,
    skills_prompt_mode: crate::config::SkillsPromptInjectionMode,
) -> String {
    use std::fmt::Write;
    let mut prompt = String::with_capacity(8192);

    // ── 0. Anti-narration (top priority) ───────────────────────
    prompt.push_str(
        "## CRITICAL: No Tool Narration\n\n\
         NEVER narrate, announce, describe, or explain your tool usage to the user. \
         Do NOT say things like 'Let me check...', 'I will use http_request to...', \
         'I'll fetch that for you', 'Searching now...', or 'Using the web_search tool'. \
         The user must ONLY see the final answer. Tool calls are invisible infrastructure — \
         never reference them. If you catch yourself starting a sentence about what tool \
         you are about to use or just used, DELETE it and give the answer directly.\n\n",
    );

    // ── 1. Tooling ──────────────────────────────────────────────
    if !tools.is_empty() {
        prompt.push_str("## Tools\n\n");
        prompt.push_str("You have access to the following tools:\n\n");
        for (name, desc) in tools {
            let _ = writeln!(prompt, "- **{name}**: {desc}");
        }
        prompt.push('\n');
    }

    // ── 1b. Action instruction (avoid meta-summary) ───────────────
    if native_tools {
        prompt.push_str(
            "## Your Task\n\n\
             When the user sends a message, respond naturally. Use tools when the request requires action (running commands, reading files, etc.).\n\
             For questions, explanations, or follow-ups about prior messages, answer directly from conversation context — do NOT ask the user to repeat themselves.\n\
             Do NOT: summarize this configuration, describe your capabilities, or output step-by-step meta-commentary.\n\n",
        );
    } else {
        prompt.push_str(
            "## Your Task\n\n\
             When the user sends a message, ACT on it. Use the tools to fulfill their request.\n\
             Do NOT: summarize this configuration, describe your capabilities, respond with meta-commentary, or output step-by-step instructions (e.g. \"1. First... 2. Next...\").\n\
             Instead: emit actual <tool_call> tags when you need to act. Just do what they ask.\n\n",
        );
    }

    // ── 2. Safety ───────────────────────────────────────────────
    prompt.push_str("## Safety\n\n");
    prompt.push_str(
        "- Do not exfiltrate private data.\n\
         - Do not run destructive commands without asking.\n\
         - Do not bypass oversight or approval mechanisms.\n\
         - Prefer `trash` over `rm` (recoverable beats gone forever).\n\
         - When in doubt, ask before acting externally.\n\n",
    );

    // ── 3. Skills (full or compact, based on config) ─────────────
    if !skills.is_empty() {
        prompt.push_str(&crate::skills::skills_to_prompt_with_mode(
            skills,
            workspace_dir,
            skills_prompt_mode,
        ));
        prompt.push_str("\n\n");
    }

    // ── 4. Workspace ────────────────────────────────────────────
    let _ = writeln!(
        prompt,
        "## Workspace\n\nWorking directory: `{}`\n",
        workspace_dir.display()
    );

    // ── 5. Bootstrap files (injected into context) ──────────────
    prompt.push_str("## Project Context\n\n");

    // Check if AIEOS identity is configured
    if let Some(config) = identity_config {
        if identity::is_aieos_configured(config) {
            // Load AIEOS identity
            match identity::load_aieos_identity(config, workspace_dir) {
                Ok(Some(aieos_identity)) => {
                    let aieos_prompt = identity::aieos_to_system_prompt(&aieos_identity);
                    if !aieos_prompt.is_empty() {
                        prompt.push_str(&aieos_prompt);
                        prompt.push_str("\n\n");
                    }
                }
                Ok(None) => {
                    // No AIEOS identity loaded (shouldn't happen if is_aieos_configured returned true)
                    // Fall back to OpenClaw bootstrap files
                    let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
                    load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
                }
                Err(e) => {
                    // Log error but don't fail - fall back to OpenClaw
                    eprintln!(
                        "Warning: Failed to load AIEOS identity: {e}. Using OpenClaw format."
                    );
                    let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
                    load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
                }
            }
        } else {
            // OpenClaw format
            let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
            load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
        }
    } else {
        // No identity config - use OpenClaw format
        let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
        load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
    }

    // ── 6. Date & Time ──────────────────────────────────────────
    let now = chrono::Local::now();
    let _ = writeln!(
        prompt,
        "## Current Date & Time\n\n{} ({})\n",
        now.format("%Y-%m-%d %H:%M:%S"),
        now.format("%Z")
    );

    // ── 7. Runtime ──────────────────────────────────────────────
    let host =
        hostname::get().map_or_else(|_| "unknown".into(), |h| h.to_string_lossy().to_string());
    let _ = writeln!(
        prompt,
        "## Runtime\n\nHost: {host} | OS: {} | Model: {model_name}\n",
        std::env::consts::OS,
    );

    // ── 8. Channel Capabilities ─────────────────────────────────────
    prompt.push_str("## Channel Capabilities\n\n");
    prompt.push_str("- You are running as a messaging bot. Your response is automatically sent back to the user's channel.\n");
    prompt.push_str("- You do NOT need to ask permission to respond — just respond directly.\n");
    prompt.push_str("- NEVER repeat, describe, or echo credentials, tokens, API keys, or secrets in your responses.\n");
    prompt.push_str("- If a tool output contains credentials, they have already been redacted — do not mention them.\n");
    prompt.push_str("- When a user sends a voice note, it is automatically transcribed to text. Your text reply is automatically converted to a voice note and sent back. Do NOT attempt to generate audio yourself — TTS is handled by the channel.\n");
    prompt.push_str("- NEVER narrate or describe your tool usage. Do NOT say 'Let me fetch...', 'I will use...', 'Searching...', or similar. Give the FINAL ANSWER only — no intermediate steps, no tool mentions, no progress updates.\n\n");

    if prompt.is_empty() {
        "You are SynapseClaw, a fast and efficient AI assistant built in Rust. Be helpful, concise, and direct."
            .to_string()
    } else {
        prompt
    }
}

/// Inject a single workspace file into the prompt with truncation and missing-file markers.
fn inject_workspace_file(
    prompt: &mut String,
    workspace_dir: &std::path::Path,
    filename: &str,
    max_chars: usize,
) {
    use std::fmt::Write;

    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return;
            }
            let _ = writeln!(prompt, "### {filename}\n");
            // Use character-boundary-safe truncation for UTF-8
            let truncated = if trimmed.chars().count() > max_chars {
                trimmed
                    .char_indices()
                    .nth(max_chars)
                    .map(|(idx, _)| &trimmed[..idx])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            if truncated.len() < trimmed.len() {
                prompt.push_str(truncated);
                let _ = writeln!(
                    prompt,
                    "\n\n[... truncated at {max_chars} chars — use `read` for full file]\n"
                );
            } else {
                prompt.push_str(trimmed);
                prompt.push_str("\n\n");
            }
        }
        Err(_) => {
            // Missing-file marker (matches OpenClaw behavior)
            let _ = writeln!(prompt, "### {filename}\n\n[File not found: {filename}]\n");
        }
    }
}

fn normalize_telegram_identity(value: &str) -> String {
    value.trim().trim_start_matches('@').to_string()
}

async fn bind_telegram_identity(config: &Config, identity: &str) -> Result<()> {
    let normalized = normalize_telegram_identity(identity);
    if normalized.is_empty() {
        anyhow::bail!("Telegram identity cannot be empty");
    }

    let mut updated = config.clone();
    let Some(telegram) = updated.channels_config.telegram.as_mut() else {
        anyhow::bail!(
            "Telegram channel is not configured. Run `synapseclaw onboard --channels-only` first"
        );
    };

    if telegram.allowed_users.iter().any(|u| u == "*") {
        println!(
            "⚠️ Telegram allowlist is currently wildcard (`*`) — binding is unnecessary until you remove '*'."
        );
    }

    if telegram
        .allowed_users
        .iter()
        .map(|entry| normalize_telegram_identity(entry))
        .any(|entry| entry == normalized)
    {
        println!("✅ Telegram identity already bound: {normalized}");
        return Ok(());
    }

    telegram.allowed_users.push(normalized.clone());
    updated.save().await?;
    println!("✅ Bound Telegram identity: {normalized}");
    println!("   Saved to {}", updated.config_path.display());
    match maybe_restart_managed_daemon_service() {
        Ok(true) => {
            println!("🔄 Detected running managed daemon service; reloaded automatically.");
        }
        Ok(false) => {
            println!(
                "ℹ️ No managed daemon service detected. If `synapseclaw daemon`/`channel start` is already running, restart it to load the updated allowlist."
            );
        }
        Err(e) => {
            eprintln!(
                "⚠️ Allowlist saved, but failed to reload daemon service automatically: {e}\n\
                 Restart service manually with `synapseclaw service stop && synapseclaw service start`."
            );
        }
    }
    Ok(())
}

fn maybe_restart_managed_daemon_service() -> Result<bool> {
    if cfg!(target_os = "macos") {
        let home = directories::UserDirs::new()
            .map(|u| u.home_dir().to_path_buf())
            .context("Could not find home directory")?;
        let plist = home
            .join("Library")
            .join("LaunchAgents")
            .join("com.synapseclaw.daemon.plist");
        if !plist.exists() {
            return Ok(false);
        }

        let list_output = Command::new("launchctl")
            .arg("list")
            .output()
            .context("Failed to query launchctl list")?;
        let listed = String::from_utf8_lossy(&list_output.stdout);
        if !listed.contains("com.synapseclaw.daemon") {
            return Ok(false);
        }

        let _ = Command::new("launchctl")
            .args(["stop", "com.synapseclaw.daemon"])
            .output();
        let start_output = Command::new("launchctl")
            .args(["start", "com.synapseclaw.daemon"])
            .output()
            .context("Failed to start launchd daemon service")?;
        if !start_output.status.success() {
            let stderr = String::from_utf8_lossy(&start_output.stderr);
            anyhow::bail!("launchctl start failed: {}", stderr.trim());
        }

        return Ok(true);
    }

    if cfg!(target_os = "linux") {
        // OpenRC (system-wide) takes precedence over systemd (user-level)
        let openrc_init_script = PathBuf::from("/etc/init.d/synapseclaw");
        if openrc_init_script.exists() {
            if let Ok(status_output) = Command::new("rc-service").args(OPENRC_STATUS_ARGS).output()
            {
                // rc-service exits 0 if running, non-zero otherwise
                if status_output.status.success() {
                    let restart_output = Command::new("rc-service")
                        .args(OPENRC_RESTART_ARGS)
                        .output()
                        .context("Failed to restart OpenRC daemon service")?;
                    if !restart_output.status.success() {
                        let stderr = String::from_utf8_lossy(&restart_output.stderr);
                        anyhow::bail!("rc-service restart failed: {}", stderr.trim());
                    }
                    return Ok(true);
                }
            }
        }

        // Systemd (user-level)
        let home = directories::UserDirs::new()
            .map(|u| u.home_dir().to_path_buf())
            .context("Could not find home directory")?;
        let unit_path: PathBuf = home
            .join(".config")
            .join("systemd")
            .join("user")
            .join("synapseclaw.service");
        if !unit_path.exists() {
            return Ok(false);
        }

        let active_output = Command::new("systemctl")
            .args(SYSTEMD_STATUS_ARGS)
            .output()
            .context("Failed to query systemd service state")?;
        let state = String::from_utf8_lossy(&active_output.stdout);
        if !state.trim().eq_ignore_ascii_case("active") {
            return Ok(false);
        }

        let restart_output = Command::new("systemctl")
            .args(SYSTEMD_RESTART_ARGS)
            .output()
            .context("Failed to restart systemd daemon service")?;
        if !restart_output.status.success() {
            let stderr = String::from_utf8_lossy(&restart_output.stderr);
            anyhow::bail!("systemctl restart failed: {}", stderr.trim());
        }

        return Ok(true);
    }

    Ok(false)
}

pub(crate) async fn handle_command(command: crate::ChannelCommands, config: &Config) -> Result<()> {
    match command {
        crate::ChannelCommands::Start => {
            anyhow::bail!("Start must be handled in main.rs (requires async runtime)")
        }
        crate::ChannelCommands::Doctor => {
            anyhow::bail!("Doctor must be handled in main.rs (requires async runtime)")
        }
        crate::ChannelCommands::List => {
            println!("Channels:");
            println!("  ✅ CLI (always available)");
            for (channel, configured) in config.channels_config.channels() {
                println!(
                    "  {} {}",
                    if configured { "✅" } else { "❌" },
                    channel.name()
                );
            }
            // Notion is a top-level config section, not part of ChannelsConfig
            {
                let notion_configured =
                    config.notion.enabled && !config.notion.database_id.trim().is_empty();
                println!("  {} Notion", if notion_configured { "✅" } else { "❌" });
            }
            if !cfg!(feature = "channel-matrix") {
                println!(
                    "  ℹ️ Matrix channel support is disabled in this build (enable `channel-matrix`)."
                );
            }
            if !cfg!(feature = "channel-lark") {
                println!(
                    "  ℹ️ Lark/Feishu channel support is disabled in this build (enable `channel-lark`)."
                );
            }
            println!("\nTo start channels: synapseclaw channel start");
            println!("To check health:    synapseclaw channel doctor");
            println!("To configure:      synapseclaw onboard");
            Ok(())
        }
        crate::ChannelCommands::Add {
            channel_type,
            config: _,
        } => {
            anyhow::bail!(
                "Channel type '{channel_type}' — use `synapseclaw onboard` to configure channels"
            );
        }
        crate::ChannelCommands::Remove { name } => {
            anyhow::bail!("Remove channel '{name}' — edit ~/.synapseclaw/config.toml directly");
        }
        crate::ChannelCommands::BindTelegram { identity } => {
            Box::pin(bind_telegram_identity(config, &identity)).await
        }
        crate::ChannelCommands::Send {
            message,
            channel_id,
            recipient,
        } => {
            let channel = build_channel_by_id(config, &channel_id)?;
            channel
                .send(&SendMessage::new(&message, &recipient))
                .await
                .with_context(|| format!("Failed to send message via {channel_id}"))?;
            println!("Message sent via {channel_id}.");
            Ok(())
        }
    }
}

/// Build a single channel instance by config section name (e.g. "telegram").
pub fn build_channel_by_id(config: &Config, channel_id: &str) -> Result<Arc<dyn Channel>> {
    match channel_id {
        "telegram" => {
            let tg = config
                .channels_config
                .telegram
                .as_ref()
                .context("Telegram channel is not configured")?;
            Ok(Arc::new(
                TelegramChannel::new(
                    tg.bot_token.clone(),
                    tg.allowed_users.clone(),
                    tg.mention_only,
                )
                .with_streaming(tg.stream_mode, tg.draft_update_interval_ms)
                .with_transcription(config.transcription.clone())
                .with_workspace_dir(config.workspace_dir.clone()),
            ))
        }
        "discord" => {
            let dc = config
                .channels_config
                .discord
                .as_ref()
                .context("Discord channel is not configured")?;
            Ok(Arc::new(DiscordChannel::new(
                dc.bot_token.clone(),
                dc.guild_id.clone(),
                dc.allowed_users.clone(),
                dc.listen_to_bots,
                dc.mention_only,
            )))
        }
        "slack" => {
            let sl = config
                .channels_config
                .slack
                .as_ref()
                .context("Slack channel is not configured")?;
            Ok(Arc::new(
                SlackChannel::new(
                    sl.bot_token.clone(),
                    sl.app_token.clone(),
                    sl.channel_id.clone(),
                    Vec::new(),
                    sl.allowed_users.clone(),
                )
                .with_workspace_dir(config.workspace_dir.clone()),
            ))
        }
        #[cfg(feature = "channel-matrix")]
        "matrix" => {
            let mx = config
                .channels_config
                .matrix
                .as_ref()
                .context("Matrix channel is not configured")?;
            Ok(Arc::new(
                MatrixChannel::new_with_session_hint_and_synapseclaw_dir(
                    mx.homeserver.clone(),
                    mx.access_token.clone(),
                    mx.room_id.clone(),
                    mx.allowed_users.clone(),
                    mx.user_id.clone(),
                    mx.device_id.clone(),
                    config.config_path.parent().map(|path| path.to_path_buf()),
                )
                .with_password(mx.password.clone())
                .with_max_media_download_mb(mx.max_media_download_mb)
                .with_transcription(config.transcription.clone()),
            ))
        }
        "mattermost" => {
            let mm = config
                .channels_config
                .mattermost
                .as_ref()
                .context("Mattermost channel is not configured")?;
            Ok(Arc::new(MattermostChannel::new(
                mm.url.clone(),
                mm.bot_token.clone(),
                mm.channel_id.clone(),
                mm.allowed_users.clone(),
                mm.thread_replies.unwrap_or(true),
                mm.mention_only.unwrap_or(false),
            )))
        }
        "signal" => {
            let sg = config
                .channels_config
                .signal
                .as_ref()
                .context("Signal channel is not configured")?;
            Ok(Arc::new(SignalChannel::new(
                sg.http_url.clone(),
                sg.account.clone(),
                sg.group_id.clone(),
                sg.allowed_from.clone(),
                sg.ignore_attachments,
                sg.ignore_stories,
            )))
        }
        other => {
            #[cfg(feature = "channel-matrix")]
            anyhow::bail!(
                "Unknown channel '{other}'. Supported: telegram, discord, slack, matrix, mattermost, signal"
            );
            #[cfg(not(feature = "channel-matrix"))]
            anyhow::bail!(
                "Unknown channel '{other}'. Supported: telegram, discord, slack, mattermost, signal"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelHealthState {
    Healthy,
    Unhealthy,
    Timeout,
}

fn classify_health_result(
    result: &std::result::Result<bool, tokio::time::error::Elapsed>,
) -> ChannelHealthState {
    match result {
        Ok(true) => ChannelHealthState::Healthy,
        Ok(false) => ChannelHealthState::Unhealthy,
        Err(_) => ChannelHealthState::Timeout,
    }
}

struct ConfiguredChannel {
    display_name: &'static str,
    channel: Arc<dyn Channel>,
}

#[allow(unused_variables)]
fn collect_configured_channels(
    config: &Config,
    matrix_skip_context: &str,
) -> Vec<ConfiguredChannel> {
    let _ = matrix_skip_context;
    let mut channels = Vec::new();

    if let Some(ref tg) = config.channels_config.telegram {
        channels.push(ConfiguredChannel {
            display_name: "Telegram",
            channel: Arc::new(
                TelegramChannel::new(
                    tg.bot_token.clone(),
                    tg.allowed_users.clone(),
                    tg.mention_only,
                )
                .with_streaming(tg.stream_mode, tg.draft_update_interval_ms)
                .with_transcription(config.transcription.clone())
                .with_workspace_dir(config.workspace_dir.clone()),
            ),
        });
    }

    if let Some(ref dc) = config.channels_config.discord {
        channels.push(ConfiguredChannel {
            display_name: "Discord",
            channel: Arc::new(DiscordChannel::new(
                dc.bot_token.clone(),
                dc.guild_id.clone(),
                dc.allowed_users.clone(),
                dc.listen_to_bots,
                dc.mention_only,
            )),
        });
    }

    if let Some(ref sl) = config.channels_config.slack {
        channels.push(ConfiguredChannel {
            display_name: "Slack",
            channel: Arc::new(
                SlackChannel::new(
                    sl.bot_token.clone(),
                    sl.app_token.clone(),
                    sl.channel_id.clone(),
                    Vec::new(),
                    sl.allowed_users.clone(),
                )
                .with_group_reply_policy(sl.mention_only, Vec::new())
                .with_workspace_dir(config.workspace_dir.clone()),
            ),
        });
    }

    if let Some(ref mm) = config.channels_config.mattermost {
        channels.push(ConfiguredChannel {
            display_name: "Mattermost",
            channel: Arc::new(MattermostChannel::new(
                mm.url.clone(),
                mm.bot_token.clone(),
                mm.channel_id.clone(),
                mm.allowed_users.clone(),
                mm.thread_replies.unwrap_or(true),
                mm.mention_only.unwrap_or(false),
            )),
        });
    }

    if let Some(ref im) = config.channels_config.imessage {
        channels.push(ConfiguredChannel {
            display_name: "iMessage",
            channel: Arc::new(IMessageChannel::new(im.allowed_contacts.clone())),
        });
    }

    #[cfg(feature = "channel-matrix")]
    if let Some(ref mx) = config.channels_config.matrix {
        channels.push(ConfiguredChannel {
            display_name: "Matrix",
            channel: Arc::new(
                MatrixChannel::new_with_session_hint_and_synapseclaw_dir(
                    mx.homeserver.clone(),
                    mx.access_token.clone(),
                    mx.room_id.clone(),
                    mx.allowed_users.clone(),
                    mx.user_id.clone(),
                    mx.device_id.clone(),
                    config.config_path.parent().map(|path| path.to_path_buf()),
                )
                .with_password(mx.password.clone())
                .with_max_media_download_mb(mx.max_media_download_mb)
                .with_transcription(config.transcription.clone()),
            ),
        });
    }

    #[cfg(not(feature = "channel-matrix"))]
    if config.channels_config.matrix.is_some() {
        tracing::warn!(
            "Matrix channel is configured but this build was compiled without `channel-matrix`; skipping Matrix {}.",
            matrix_skip_context
        );
    }

    if let Some(ref sig) = config.channels_config.signal {
        channels.push(ConfiguredChannel {
            display_name: "Signal",
            channel: Arc::new(SignalChannel::new(
                sig.http_url.clone(),
                sig.account.clone(),
                sig.group_id.clone(),
                sig.allowed_from.clone(),
                sig.ignore_attachments,
                sig.ignore_stories,
            )),
        });
    }

    if let Some(ref wa) = config.channels_config.whatsapp {
        if wa.is_ambiguous_config() {
            tracing::warn!(
                "WhatsApp config has both phone_number_id and session_path set; preferring Cloud API mode. Remove one selector to avoid ambiguity."
            );
        }
        // Runtime negotiation: detect backend type from config
        match wa.backend_type() {
            "cloud" => {
                // Cloud API mode: requires phone_number_id, access_token, verify_token
                if wa.is_cloud_config() {
                    channels.push(ConfiguredChannel {
                        display_name: "WhatsApp",
                        channel: Arc::new(WhatsAppChannel::new(
                            wa.access_token.clone().unwrap_or_default(),
                            wa.phone_number_id.clone().unwrap_or_default(),
                            wa.verify_token.clone().unwrap_or_default(),
                            wa.allowed_numbers.clone(),
                        )),
                    });
                } else {
                    tracing::warn!("WhatsApp Cloud API configured but missing required fields (phone_number_id, access_token, verify_token)");
                }
            }
            "web" => {
                // Web mode: requires session_path
                #[cfg(feature = "whatsapp-web")]
                if wa.is_web_config() {
                    channels.push(ConfiguredChannel {
                        display_name: "WhatsApp",
                        channel: Arc::new(
                            WhatsAppWebChannel::new(
                                wa.session_path.clone().unwrap_or_default(),
                                wa.pair_phone.clone(),
                                wa.pair_code.clone(),
                                wa.allowed_numbers.clone(),
                            )
                            .with_transcription(config.transcription.clone())
                            .with_tts(config.tts.clone()),
                        ),
                    });
                } else {
                    tracing::warn!("WhatsApp Web configured but session_path not set");
                }
                #[cfg(not(feature = "whatsapp-web"))]
                {
                    tracing::warn!("WhatsApp Web backend requires 'whatsapp-web' feature. Enable with: cargo build --features whatsapp-web");
                    eprintln!("  ⚠ WhatsApp Web is configured but the 'whatsapp-web' feature is not compiled in.");
                    eprintln!("    Rebuild with: cargo build --features whatsapp-web");
                }
            }
            _ => {
                tracing::warn!("WhatsApp config invalid: neither phone_number_id (Cloud API) nor session_path (Web) is set");
            }
        }
    }

    if let Some(ref lq) = config.channels_config.linq {
        channels.push(ConfiguredChannel {
            display_name: "Linq",
            channel: Arc::new(LinqChannel::new(
                lq.api_token.clone(),
                lq.from_phone.clone(),
                lq.allowed_senders.clone(),
            )),
        });
    }

    if let Some(ref wati_cfg) = config.channels_config.wati {
        channels.push(ConfiguredChannel {
            display_name: "WATI",
            channel: Arc::new(WatiChannel::new(
                wati_cfg.api_token.clone(),
                wati_cfg.api_url.clone(),
                wati_cfg.tenant_id.clone(),
                wati_cfg.allowed_numbers.clone(),
            )),
        });
    }

    if let Some(ref nc) = config.channels_config.nextcloud_talk {
        channels.push(ConfiguredChannel {
            display_name: "Nextcloud Talk",
            channel: Arc::new(NextcloudTalkChannel::new(
                nc.base_url.clone(),
                nc.app_token.clone(),
                nc.allowed_users.clone(),
            )),
        });
    }

    if let Some(ref email_cfg) = config.channels_config.email {
        channels.push(ConfiguredChannel {
            display_name: "Email",
            channel: Arc::new(EmailChannel::new(email_cfg.clone())),
        });
    }

    if let Some(ref irc) = config.channels_config.irc {
        channels.push(ConfiguredChannel {
            display_name: "IRC",
            channel: Arc::new(IrcChannel::new(irc::IrcChannelConfig {
                server: irc.server.clone(),
                port: irc.port,
                nickname: irc.nickname.clone(),
                username: irc.username.clone(),
                channels: irc.channels.clone(),
                allowed_users: irc.allowed_users.clone(),
                server_password: irc.server_password.clone(),
                nickserv_password: irc.nickserv_password.clone(),
                sasl_password: irc.sasl_password.clone(),
                verify_tls: irc.verify_tls.unwrap_or(true),
            })),
        });
    }

    #[cfg(feature = "channel-lark")]
    if let Some(ref lk) = config.channels_config.lark {
        if lk.use_feishu {
            if config.channels_config.feishu.is_some() {
                tracing::warn!(
                    "Both [channels_config.feishu] and legacy [channels_config.lark].use_feishu=true are configured; ignoring legacy Feishu fallback in lark."
                );
            } else {
                tracing::warn!(
                    "Using legacy [channels_config.lark].use_feishu=true compatibility path; prefer [channels_config.feishu]."
                );
                channels.push(ConfiguredChannel {
                    display_name: "Feishu",
                    channel: Arc::new(LarkChannel::from_config(lk)),
                });
            }
        } else {
            channels.push(ConfiguredChannel {
                display_name: "Lark",
                channel: Arc::new(LarkChannel::from_lark_config(lk)),
            });
        }
    }

    #[cfg(feature = "channel-lark")]
    if let Some(ref fs) = config.channels_config.feishu {
        channels.push(ConfiguredChannel {
            display_name: "Feishu",
            channel: Arc::new(LarkChannel::from_feishu_config(fs)),
        });
    }

    #[cfg(not(feature = "channel-lark"))]
    if config.channels_config.lark.is_some() || config.channels_config.feishu.is_some() {
        tracing::warn!(
            "Lark/Feishu channel is configured but this build was compiled without `channel-lark`; skipping Lark/Feishu health check."
        );
    }

    if let Some(ref dt) = config.channels_config.dingtalk {
        channels.push(ConfiguredChannel {
            display_name: "DingTalk",
            channel: Arc::new(DingTalkChannel::new(
                dt.client_id.clone(),
                dt.client_secret.clone(),
                dt.allowed_users.clone(),
            )),
        });
    }

    if let Some(ref qq) = config.channels_config.qq {
        channels.push(ConfiguredChannel {
            display_name: "QQ",
            channel: Arc::new(QQChannel::new(
                qq.app_id.clone(),
                qq.app_secret.clone(),
                qq.allowed_users.clone(),
            )),
        });
    }

    if let Some(ref tw) = config.channels_config.twitter {
        channels.push(ConfiguredChannel {
            display_name: "X/Twitter",
            channel: Arc::new(TwitterChannel::new(
                tw.bearer_token.clone(),
                tw.allowed_users.clone(),
            )),
        });
    }

    if let Some(ref mc) = config.channels_config.mochat {
        channels.push(ConfiguredChannel {
            display_name: "Mochat",
            channel: Arc::new(MochatChannel::new(
                mc.api_url.clone(),
                mc.api_token.clone(),
                mc.allowed_users.clone(),
                mc.poll_interval_secs,
            )),
        });
    }

    if let Some(ref wc) = config.channels_config.wecom {
        channels.push(ConfiguredChannel {
            display_name: "WeCom",
            channel: Arc::new(WeComChannel::new(
                wc.webhook_key.clone(),
                wc.allowed_users.clone(),
            )),
        });
    }

    if let Some(ref ct) = config.channels_config.clawdtalk {
        channels.push(ConfiguredChannel {
            display_name: "ClawdTalk",
            channel: Arc::new(ClawdTalkChannel::new(ct.clone())),
        });
    }

    // Notion database poller channel
    if config.notion.enabled && !config.notion.database_id.trim().is_empty() {
        let notion_api_key = if config.notion.api_key.trim().is_empty() {
            std::env::var("NOTION_API_KEY").unwrap_or_default()
        } else {
            config.notion.api_key.trim().to_string()
        };
        if notion_api_key.trim().is_empty() {
            tracing::warn!(
                "Notion channel enabled but no API key found (set notion.api_key or NOTION_API_KEY env var)"
            );
        } else {
            channels.push(ConfiguredChannel {
                display_name: "Notion",
                channel: Arc::new(NotionChannel::new(
                    notion_api_key,
                    config.notion.database_id.clone(),
                    config.notion.poll_interval_secs,
                    config.notion.status_property.clone(),
                    config.notion.input_property.clone(),
                    config.notion.result_property.clone(),
                    config.notion.max_concurrent,
                    config.notion.recover_stale,
                )),
            });
        }
    }

    if let Some(ref rd) = config.channels_config.reddit {
        channels.push(ConfiguredChannel {
            display_name: "Reddit",
            channel: Arc::new(RedditChannel::new(
                rd.client_id.clone(),
                rd.client_secret.clone(),
                rd.refresh_token.clone(),
                rd.username.clone(),
                rd.subreddit.clone(),
            )),
        });
    }

    if let Some(ref bs) = config.channels_config.bluesky {
        channels.push(ConfiguredChannel {
            display_name: "Bluesky",
            channel: Arc::new(BlueskyChannel::new(
                bs.handle.clone(),
                bs.app_password.clone(),
            )),
        });
    }

    if let Some(ref wh) = config.channels_config.webhook {
        channels.push(ConfiguredChannel {
            display_name: "Webhook",
            channel: Arc::new(WebhookChannel::new(
                wh.port,
                wh.listen_path.clone(),
                wh.send_url.clone(),
                wh.send_method.clone(),
                wh.auth_header.clone(),
                wh.secret.clone(),
            )),
        });
    }

    channels
}

/// Run health checks for configured channels.
pub async fn doctor_channels(config: Config) -> Result<()> {
    #[allow(unused_mut)]
    let mut channels = collect_configured_channels(&config, "health check");

    #[cfg(feature = "channel-nostr")]
    if let Some(ref ns) = config.channels_config.nostr {
        channels.push(ConfiguredChannel {
            display_name: "Nostr",
            channel: Arc::new(
                NostrChannel::new(&ns.private_key, ns.relays.clone(), &ns.allowed_pubkeys).await?,
            ),
        });
    }

    if channels.is_empty() {
        println!("No real-time channels configured. Run `synapseclaw onboard` first.");
        return Ok(());
    }

    println!("🩺 SynapseClaw Channel Doctor");
    println!();

    let mut healthy = 0_u32;
    let mut unhealthy = 0_u32;
    let mut timeout = 0_u32;

    for configured in channels {
        let result =
            tokio::time::timeout(Duration::from_secs(10), configured.channel.health_check()).await;
        let state = classify_health_result(&result);

        match state {
            ChannelHealthState::Healthy => {
                healthy += 1;
                println!("  ✅ {:<9} healthy", configured.display_name);
            }
            ChannelHealthState::Unhealthy => {
                unhealthy += 1;
                println!(
                    "  ❌ {:<9} unhealthy (auth/config/network)",
                    configured.display_name
                );
            }
            ChannelHealthState::Timeout => {
                timeout += 1;
                println!("  ⏱️  {:<9} timed out (>10s)", configured.display_name);
            }
        }
    }

    if config.channels_config.webhook.is_some() {
        println!("  ℹ️  Webhook   check via `synapseclaw gateway` then GET /health");
    }

    println!();
    println!("Summary: {healthy} healthy, {unhealthy} unhealthy, {timeout} timed out");
    Ok(())
}

/// Start all configured channels and route messages to the agent
#[allow(clippy::too_many_lines)]
pub async fn start_channels(
    config: Config,
    shared_ipc_client: Option<std::sync::Arc<crate::fork_adapters::tools::agents_ipc::IpcClient>>,
) -> Result<()> {
    let provider_name = resolved_default_provider(&config);
    let provider_runtime_options = providers::ProviderRuntimeOptions {
        auth_profile_override: None,
        provider_api_url: config.api_url.clone(),
        synapseclaw_dir: config.config_path.parent().map(std::path::PathBuf::from),
        secrets_encrypt: config.secrets.encrypt,
        reasoning_enabled: config.runtime.reasoning_enabled,
        reasoning_effort: config.runtime.reasoning_effort.clone(),
        provider_timeout_secs: Some(config.provider_timeout_secs),
        extra_headers: config.extra_headers.clone(),
        api_path: config.api_path.clone(),
        prompt_caching: config.agent.prompt_caching,
    };
    let provider: Arc<dyn Provider> = Arc::from(
        create_resilient_provider_nonblocking(
            &provider_name,
            config.api_key.clone(),
            config.api_url.clone(),
            config.reliability.clone(),
            provider_runtime_options.clone(),
        )
        .await?,
    );

    // Warm up the provider connection pool (TLS handshake, DNS, HTTP/2 setup)
    // so the first real message doesn't hit a cold-start timeout.
    if let Err(e) = provider.warmup().await {
        tracing::warn!("Provider warmup failed (non-fatal): {e}");
    }

    let initial_stamp = config_file_stamp(&config.config_path).await;
    {
        let mut store = runtime_config_store()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        store.insert(
            config.config_path.clone(),
            RuntimeConfigState {
                defaults: runtime_defaults_from_config(&config),
                last_applied_stamp: initial_stamp,
            },
        );
    }

    let observer: Arc<dyn Observer> =
        Arc::from(observability::create_observer(&config.observability));
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(security_policy_from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));
    let model = resolved_default_model(&config);
    let temperature = config.default_temperature;
    let mem: Arc<dyn Memory> = Arc::from(memory::create_memory_with_storage_and_routes(
        &config.memory,
        &config.embedding_routes,
        Some(&config.storage.provider.config),
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);
    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };
    // Build system prompt from workspace identity files + skills
    let workspace = config.workspace_dir.clone();
    let (mut built_tools, delegate_handle_ch, ipc_client_for_key_reg): (Vec<Box<dyn Tool>>, _, _) =
        tools::all_tools_with_runtime(
            Arc::new(config.clone()),
            &security,
            runtime,
            Arc::clone(&mem),
            composio_key,
            composio_entity_id,
            &config.browser,
            &config.http_request,
            &config.web_fetch,
            &workspace,
            &config.agents,
            config.api_key.as_deref(),
            &config,
            shared_ipc_client.clone(),
        );

    // ── Phase 3B: Auto-register Ed25519 public key with broker ────
    // Tries 3 times with backoff; if all fail, spawns a background task
    // that retries every 30s until the broker becomes available.
    if let Some(ref ipc_client) = ipc_client_for_key_reg {
        ipc_client.register_public_key_with_background_retry().await;
    }

    // Wire MCP tools into the registry before freezing — non-fatal.
    // When `deferred_loading` is enabled, MCP tools are NOT added eagerly.
    // Instead, a `tool_search` built-in is registered for on-demand loading.
    let mut deferred_section = String::new();
    let mut ch_activated_handle: Option<
        std::sync::Arc<std::sync::Mutex<crate::fork_adapters::tools::ActivatedToolSet>>,
    > = None;
    if config.mcp.enabled && !config.mcp.servers.is_empty() {
        tracing::info!(
            "Initializing MCP client — {} server(s) configured",
            config.mcp.servers.len()
        );
        match crate::fork_adapters::tools::McpRegistry::connect_all(&config.mcp.servers).await {
            Ok(registry) => {
                let registry = std::sync::Arc::new(registry);
                if config.mcp.deferred_loading {
                    let deferred_set =
                        crate::fork_adapters::tools::DeferredMcpToolSet::from_registry(
                            std::sync::Arc::clone(&registry),
                        )
                        .await;
                    tracing::info!(
                        "MCP deferred: {} tool stub(s) from {} server(s)",
                        deferred_set.len(),
                        registry.server_count()
                    );
                    deferred_section =
                        crate::fork_adapters::tools::mcp_deferred::build_deferred_tools_section(
                            &deferred_set,
                        );
                    let activated = std::sync::Arc::new(std::sync::Mutex::new(
                        crate::fork_adapters::tools::ActivatedToolSet::new(),
                    ));
                    ch_activated_handle = Some(std::sync::Arc::clone(&activated));
                    built_tools.push(Box::new(crate::fork_adapters::tools::ToolSearchTool::new(
                        deferred_set,
                        activated,
                    )));
                } else {
                    let names = registry.tool_names();
                    let mut registered = 0usize;
                    for name in names {
                        if let Some(def) = registry.get_tool_def(&name).await {
                            let wrapper: std::sync::Arc<dyn Tool> = std::sync::Arc::new(
                                crate::fork_adapters::tools::McpToolWrapper::new(
                                    name,
                                    def,
                                    std::sync::Arc::clone(&registry),
                                ),
                            );
                            if let Some(ref handle) = delegate_handle_ch {
                                handle.write().push(std::sync::Arc::clone(&wrapper));
                            }
                            built_tools
                                .push(Box::new(crate::fork_adapters::tools::ArcToolRef(wrapper)));
                            registered += 1;
                        }
                    }
                    tracing::info!(
                        "MCP: {} tool(s) registered from {} server(s)",
                        registered,
                        registry.server_count()
                    );
                }
            }
            Err(e) => {
                // Non-fatal — daemon continues with the tools registered above.
                tracing::error!("MCP registry failed to initialize: {e:#}");
            }
        }
    }

    // ── SYNAPSECLAW_ALLOWED_TOOLS enforcement for channel/daemon mode ──
    // Same security boundary as agent/loop_.rs — filter tools when the env
    // var is set so coordinators (e.g. marketing-lead) can be restricted to
    // IPC-only tools. Without this, the allowlist only applied to
    // ephemeral/interactive agent runs, not to daemon channel processing.
    if let Ok(allowlist_str) = std::env::var("SYNAPSECLAW_ALLOWED_TOOLS") {
        if !allowlist_str.trim().is_empty() {
            let allowed: std::collections::HashSet<String> = allowlist_str
                .split(',')
                .map(|t| t.trim().to_string())
                .collect();
            let before = built_tools.len();
            built_tools.retain(|t| allowed.contains(t.name()));
            let after = built_tools.len();
            if before != after {
                tracing::info!(
                    "SYNAPSECLAW_ALLOWED_TOOLS filtered channel tools: {before} → {after} (kept: {})",
                    allowed.iter().cloned().collect::<Vec<_>>().join(", ")
                );
            }
        }
    }

    let tools_registry = Arc::new(built_tools);

    let skills = crate::skills::load_skills_with_config(&workspace, &config);

    // Collect tool descriptions for the prompt
    let mut tool_descs: Vec<(&str, &str)> = vec![
        (
            "shell",
            "Execute terminal commands. Use when: running local checks, build/test commands, diagnostics. Don't use when: a safer dedicated tool exists, or command is destructive without approval.",
        ),
        (
            "file_read",
            "Read file contents. Use when: inspecting project files, configs, logs. Don't use when: a targeted search is enough.",
        ),
        (
            "file_write",
            "Write file contents. Use when: applying focused edits, scaffolding files, updating docs/code. Don't use when: side effects are unclear or file ownership is uncertain.",
        ),
        (
            "memory_store",
            "Save to memory. Use when: preserving durable preferences, decisions, key context. Don't use when: information is transient/noisy/sensitive without need.",
        ),
        (
            "memory_recall",
            "Search memory. Use when: retrieving prior decisions, user preferences, historical context. Don't use when: answer is already in current context.",
        ),
        (
            "memory_forget",
            "Delete a memory entry. Use when: memory is incorrect/stale or explicitly requested for removal. Don't use when: impact is uncertain.",
        ),
    ];

    if config.browser.enabled {
        tool_descs.push((
            "browser_open",
            "Open approved HTTPS URLs in system browser (allowlist-only, no scraping)",
        ));
    }
    if config.composio.enabled {
        tool_descs.push((
            "composio",
            "Execute actions on 1000+ apps via Composio (Gmail, Notion, GitHub, Slack, etc.). Use action='list' to discover actions, 'list_accounts' to retrieve connected account IDs, 'execute' to run (optionally with connected_account_id), and 'connect' for OAuth.",
        ));
    }
    tool_descs.push((
        "schedule",
        "Manage scheduled tasks (create/list/get/cancel/pause/resume). Supports recurring cron and one-shot delays.",
    ));
    tool_descs.push((
        "pushover",
        "Send a Pushover notification to your device. Requires PUSHOVER_TOKEN and PUSHOVER_USER_KEY in .env file.",
    ));
    if !config.agents.is_empty() {
        tool_descs.push((
            "delegate",
            "Delegate a subtask to a specialized agent. Use when: a task benefits from a different model (e.g. fast summarization, deep reasoning, code generation). The sub-agent runs a single prompt and returns its response.",
        ));
    }

    // Filter out tools excluded for non-CLI channels so the system prompt
    // does not advertise them for channel-driven runs.
    let excluded = &config.autonomy.non_cli_excluded_tools;
    if !excluded.is_empty() {
        tool_descs.retain(|(name, _)| !excluded.iter().any(|ex| ex == name));
    }

    // Also filter prompt tool descriptions by SYNAPSECLAW_ALLOWED_TOOLS
    // so the model doesn't see tools it can't call.
    if let Ok(allowlist_str) = std::env::var("SYNAPSECLAW_ALLOWED_TOOLS") {
        if !allowlist_str.trim().is_empty() {
            let allowed: std::collections::HashSet<&str> =
                allowlist_str.split(',').map(|t| t.trim()).collect();
            tool_descs.retain(|(name, _)| allowed.contains(name));
        }
    }

    let bootstrap_max_chars = if config.agent.compact_context {
        Some(6000)
    } else {
        None
    };
    let native_tools = provider.supports_native_tools();
    let mut system_prompt = build_system_prompt_with_mode(
        &workspace,
        &model,
        &tool_descs,
        &skills,
        Some(&config.identity),
        bootstrap_max_chars,
        native_tools,
        config.skills.prompt_injection_mode,
    );
    if !native_tools {
        system_prompt.push_str(&build_tool_instructions(tools_registry.as_ref()));
    }

    // Append deferred MCP tool names so the LLM knows what is available
    if !deferred_section.is_empty() {
        system_prompt.push('\n');
        system_prompt.push_str(&deferred_section);
    }

    if !skills.is_empty() {
        println!(
            "  🧩 Skills:   {}",
            skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Collect active channels from a shared builder to keep startup and doctor parity.
    #[allow(unused_mut)]
    let mut channels: Vec<Arc<dyn Channel>> =
        collect_configured_channels(&config, "runtime startup")
            .into_iter()
            .map(|configured| configured.channel)
            .collect();

    #[cfg(feature = "channel-nostr")]
    if let Some(ref ns) = config.channels_config.nostr {
        channels.push(Arc::new(
            NostrChannel::new(&ns.private_key, ns.relays.clone(), &ns.allowed_pubkeys).await?,
        ));
    }
    if channels.is_empty() {
        println!("No channels configured. Run `synapseclaw onboard` to set up channels.");
        return Ok(());
    }

    println!("🦀 SynapseClaw Channel Server");
    println!("  🤖 Model:    {model}");
    let effective_backend = memory::effective_memory_backend_name(
        &config.memory.backend,
        Some(&config.storage.provider.config),
    );
    println!(
        "  🧠 Memory:   {} (auto-save: {})",
        effective_backend,
        if config.memory.auto_save { "on" } else { "off" }
    );
    println!(
        "  📡 Channels: {}",
        channels
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();
    println!("  Listening for messages... (Ctrl+C to stop)");
    println!();

    crate::fork_adapters::health::mark_component_ok("channels");

    let initial_backoff_secs = config
        .reliability
        .channel_initial_backoff_secs
        .max(DEFAULT_CHANNEL_INITIAL_BACKOFF_SECS);
    let max_backoff_secs = config
        .reliability
        .channel_max_backoff_secs
        .max(DEFAULT_CHANNEL_MAX_BACKOFF_SECS);

    // Single message bus — all channels send messages here
    let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(100);

    // Spawn a listener for each channel
    let mut handles = Vec::new();
    for ch in &channels {
        handles.push(spawn_supervised_listener(
            ch.clone(),
            tx.clone(),
            initial_backoff_secs,
            max_backoff_secs,
        ));
    }
    drop(tx); // Drop our copy so rx closes when all channels stop

    let channels_by_name = Arc::new(
        channels
            .iter()
            .map(|ch| (ch.name().to_string(), Arc::clone(ch)))
            .collect::<HashMap<_, _>>(),
    );
    let max_in_flight_messages = compute_max_in_flight_messages(channels.len());

    println!("  🚦 In-flight message limit: {max_in_flight_messages}");

    let mut provider_cache_seed: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    provider_cache_seed.insert(provider_name.clone(), Arc::clone(&provider));
    let message_timeout_secs =
        effective_channel_message_timeout_secs(config.channels_config.message_timeout_secs);
    let interrupt_on_new_message = config
        .channels_config
        .telegram
        .as_ref()
        .is_some_and(|tg| tg.interrupt_on_new_message);
    let interrupt_on_new_message_slack = config
        .channels_config
        .slack
        .as_ref()
        .is_some_and(|sl| sl.interrupt_on_new_message);

    // ── Phase 4.1: Pipeline engine initialization ──────────────────
    let (pipeline_store, pipeline_executor, message_router) = if config.pipelines.enabled {
        let pipeline_dir = config
            .pipelines
            .directory
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| config.workspace_dir.join("pipelines"));

        let store: Arc<dyn fork_core::ports::pipeline_store::PipelineStorePort> = Arc::new(
            crate::fork_adapters::pipeline::toml_loader::TomlPipelineLoader::new(&pipeline_dir),
        );
        if let Err(e) = store.reload().await {
            tracing::warn!("Pipeline TOML load failed: {e}");
        } else {
            let names = store.list().await;
            if !names.is_empty() {
                tracing::info!(pipelines = ?names, "pipeline definitions loaded (channels)");
            }
        }

        // IPC step executor for pipeline dispatch.
        // In daemon mode, reuses the shared IpcClient (single seq counter).
        // In standalone mode, creates a local IpcClient.
        let executor: Option<Arc<dyn fork_core::ports::pipeline_executor::PipelineExecutorPort>> =
            if config.agents_ipc.enabled {
                if let Some(ref broker_token) = config.agents_ipc.broker_token {
                    let ipc_client = if let Some(ref shared) = shared_ipc_client {
                        Arc::clone(shared)
                    } else {
                        let runner_id = config
                            .pipelines
                            .runner_agent_id
                            .clone()
                            .or_else(|| config.agents_ipc.agent_id.clone())
                            .unwrap_or_else(|| config.agents_ipc.role.clone());
                        let key_path = config
                            .config_path
                            .parent()
                            .unwrap_or(std::path::Path::new("."))
                            .join("agent.key");

                        let mut client = crate::fork_adapters::tools::agents_ipc::IpcClient::new(
                            &config.agents_ipc.broker_url,
                            broker_token,
                            config.agents_ipc.request_timeout_secs,
                        );
                        if let Ok(identity) =
                            crate::security::identity::AgentIdentity::load_or_generate(&key_path)
                        {
                            client = client.with_identity(identity, runner_id);
                        }
                        Arc::new(client)
                    };
                    Some(Arc::new(
                        crate::fork_adapters::pipeline::ipc_step_executor::IpcStepExecutor::new(
                            ipc_client,
                        ),
                    )
                        as Arc<
                            dyn fork_core::ports::pipeline_executor::PipelineExecutorPort,
                        >)
                } else {
                    None
                }
            } else {
                None
            };

        // Message router
        let fallback_agent = config
            .pipelines
            .routing_fallback
            .clone()
            .or_else(|| config.agents_ipc.agent_id.clone())
            .unwrap_or_else(|| config.agents_ipc.role.clone());
        let router: Option<Arc<dyn fork_core::ports::message_router::MessageRouterPort>> = {
            let routing_file = config
                .pipelines
                .routing_file
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| config.workspace_dir.join("pipelines/routing.toml"));
            let router = crate::fork_adapters::routing::rule_chain::TomlMessageRouter::load(
                &routing_file,
                &fallback_agent,
            );
            tracing::info!("message router loaded from {}", routing_file.display());
            Some(Arc::new(router)
                as Arc<
                    dyn fork_core::ports::message_router::MessageRouterPort,
                >)
        };

        (Some(store), executor, router)
    } else {
        (None, None, None)
    };

    let runtime_ctx = Arc::new(ChannelRuntimeContext {
        channels_by_name,
        provider: Arc::clone(&provider),
        default_provider: Arc::new(provider_name),
        memory: Arc::clone(&mem),
        tools_registry: Arc::clone(&tools_registry),
        observer,
        system_prompt: Arc::new(system_prompt),
        model: Arc::new(model.clone()),
        temperature,
        auto_save_memory: config.memory.auto_save,
        max_tool_iterations: config.agent.max_tool_iterations,
        min_relevance_score: config.memory.min_relevance_score,
        conversation_histories: Arc::new(Mutex::new(HashMap::new())),
        provider_cache: Arc::new(Mutex::new(provider_cache_seed)),
        route_overrides: Arc::new(Mutex::new(HashMap::new())),
        api_key: config.api_key.clone(),
        api_url: config.api_url.clone(),
        reliability: Arc::new(config.reliability.clone()),
        provider_runtime_options,
        workspace_dir: Arc::new(config.workspace_dir.clone()),
        message_timeout_secs,
        interrupt_on_new_message: InterruptOnNewMessageConfig {
            enabled: interrupt_on_new_message || interrupt_on_new_message_slack,
        },
        multimodal: config.multimodal.clone(),
        hooks: if config.hooks.enabled {
            let mut runner = crate::fork_adapters::hooks::HookRunner::new();
            if config.hooks.builtin.command_logger {
                runner.register(Box::new(
                    crate::fork_adapters::hooks::builtin::CommandLoggerHook::new(),
                ));
            }
            if config.hooks.builtin.webhook_audit.enabled {
                runner.register(Box::new(
                    crate::fork_adapters::hooks::builtin::WebhookAuditHook::new(
                        config.hooks.builtin.webhook_audit.clone(),
                    ),
                ));
            }
            Some(Arc::new(runner))
        } else {
            None
        },
        non_cli_excluded_tools: Arc::new(config.autonomy.non_cli_excluded_tools.clone()),
        tool_call_dedup_exempt: Arc::new(config.agent.tool_call_dedup_exempt.clone()),
        model_routes: Arc::new(config.model_routes.clone()),
        query_classification: config.query_classification.clone(),
        ack_reactions: config.channels_config.ack_reactions,
        show_tool_calls: config.channels_config.show_tool_calls,
        session_store: if config.channels_config.session_persistence {
            match session_store::SessionStore::new(&config.workspace_dir) {
                Ok(store) => {
                    tracing::info!("📂 Session persistence enabled");
                    Some(Arc::new(store))
                }
                Err(e) => {
                    tracing::warn!("Session persistence disabled: {e}");
                    None
                }
            }
        } else {
            None
        },
        summary_config: Arc::new(config.summary.clone()),
        summary_model: config.summary_model.clone(),
        approval_manager: Arc::new(ApprovalManager::for_non_interactive(&config.autonomy)),
        activated_tools: ch_activated_handle,
        channel_registry: Some(Arc::new(
            crate::fork_adapters::channels::registry::CachedChannelRegistry::new(config.clone()),
        )),
        pipeline_store,
        pipeline_executor,
        message_router,
    });

    // Hydrate in-memory conversation histories from persisted JSONL session files.
    if let Some(ref store) = runtime_ctx.session_store {
        let mut hydrated = 0usize;
        let mut histories = runtime_ctx
            .conversation_histories
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for key in store.list_sessions() {
            let msgs = store.load(&key);
            if !msgs.is_empty() {
                hydrated += 1;
                histories.insert(key, msgs);
            }
        }
        drop(histories);
        if hydrated > 0 {
            tracing::info!("📂 Restored {hydrated} session(s) from disk");
        }
    }

    run_message_dispatch_loop(rx, runtime_ctx, max_in_flight_messages).await;

    // Wait for all channel tasks
    for h in handles {
        let _ = h.await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork_adapters::observability::NoopObserver;
    use crate::fork_adapters::providers::{ChatMessage, Provider};
    use fork_core::ports::memory_backend::Memory;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    fn make_workspace() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // Create minimal workspace files
        std::fs::write(tmp.path().join("SOUL.md"), "# Soul\nBe helpful.").unwrap();
        std::fs::write(
            tmp.path().join("IDENTITY.md"),
            "# Identity\nName: SynapseClaw",
        )
        .unwrap();
        std::fs::write(tmp.path().join("USER.md"), "# User\nName: Test User").unwrap();
        std::fs::write(
            tmp.path().join("AGENTS.md"),
            "# Agents\nFollow instructions.",
        )
        .unwrap();
        std::fs::write(tmp.path().join("TOOLS.md"), "# Tools\nUse shell carefully.").unwrap();
        std::fs::write(
            tmp.path().join("HEARTBEAT.md"),
            "# Heartbeat\nCheck status.",
        )
        .unwrap();
        std::fs::write(tmp.path().join("MEMORY.md"), "# Memory\nUser likes Rust.").unwrap();
        tmp
    }

    #[test]
    fn effective_channel_message_timeout_secs_clamps_to_minimum() {
        assert_eq!(
            effective_channel_message_timeout_secs(0),
            MIN_CHANNEL_MESSAGE_TIMEOUT_SECS
        );
        assert_eq!(
            effective_channel_message_timeout_secs(15),
            MIN_CHANNEL_MESSAGE_TIMEOUT_SECS
        );
        assert_eq!(effective_channel_message_timeout_secs(300), 300);
    }

    #[test]
    fn strip_tool_result_content_removes_blocks_and_header() {
        let input = r#"[Tool results]
<tool_result name="shell">Mon Feb 20</tool_result>
<tool_result name="http_request">{"status":200}</tool_result>"#;
        assert_eq!(strip_tool_result_content(input), "");

        let mixed = "Some context\n<tool_result name=\"shell\">ok</tool_result>\nMore text";
        let cleaned = strip_tool_result_content(mixed);
        assert!(cleaned.contains("Some context"));
        assert!(cleaned.contains("More text"));
        assert!(!cleaned.contains("tool_result"));

        assert_eq!(
            strip_tool_result_content("no tool results here"),
            "no tool results here"
        );
        assert_eq!(strip_tool_result_content(""), "");
    }

    #[test]
    fn normalize_cached_channel_turns_merges_consecutive_user_turns() {
        let turns = vec![
            ChatMessage::user("forwarded content"),
            ChatMessage::user("summarize this"),
        ];

        let normalized = normalize_cached_channel_turns(turns);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].role, "user");
        assert!(normalized[0].content.contains("forwarded content"));
        assert!(normalized[0].content.contains("summarize this"));
    }

    #[test]
    fn normalize_cached_channel_turns_merges_consecutive_assistant_turns() {
        let turns = vec![
            ChatMessage::user("first user"),
            ChatMessage::assistant("assistant part 1"),
            ChatMessage::assistant("assistant part 2"),
            ChatMessage::user("next user"),
        ];

        let normalized = normalize_cached_channel_turns(turns);
        assert_eq!(normalized.len(), 3);
        assert_eq!(normalized[0].role, "user");
        assert_eq!(normalized[1].role, "assistant");
        assert_eq!(normalized[2].role, "user");
        assert!(normalized[1].content.contains("assistant part 1"));
        assert!(normalized[1].content.contains("assistant part 2"));
    }

    /// Verify that an orphan user turn followed by a failure-marker assistant
    /// turn normalizes correctly, so the LLM sees the failed request as closed
    /// and does not re-execute it on the next user message.
    #[test]
    fn normalize_preserves_failure_marker_after_orphan_user_turn() {
        let turns = vec![
            ChatMessage::user("download something from GitHub"),
            ChatMessage::assistant("[Task failed — not continuing this request]"),
            ChatMessage::user("what is WAL?"),
        ];

        let normalized = normalize_cached_channel_turns(turns);
        assert_eq!(normalized.len(), 3);
        assert_eq!(normalized[0].role, "user");
        assert_eq!(normalized[1].role, "assistant");
        assert!(normalized[1].content.contains("Task failed"));
        assert_eq!(normalized[2].role, "user");
        assert_eq!(normalized[2].content, "what is WAL?");
    }

    /// Same as above but for the timeout variant.
    #[test]
    fn normalize_preserves_timeout_marker_after_orphan_user_turn() {
        let turns = vec![
            ChatMessage::user("run a long task"),
            ChatMessage::assistant("[Task timed out — not continuing this request]"),
            ChatMessage::user("next question"),
        ];

        let normalized = normalize_cached_channel_turns(turns);
        assert_eq!(normalized.len(), 3);
        assert_eq!(normalized[1].role, "assistant");
        assert!(normalized[1].content.contains("Task timed out"));
        assert_eq!(normalized[2].content, "next question");
    }

    #[test]
    fn proactive_trim_drops_oldest_turns_when_over_budget() {
        // Each message is 100 chars; 10 messages = 1000 chars total.
        let mut turns: Vec<ChatMessage> = (0..10)
            .map(|i| {
                let content = format!("m{i}-{}", "a".repeat(96));
                if i % 2 == 0 {
                    ChatMessage::user(content)
                } else {
                    ChatMessage::assistant(content)
                }
            })
            .collect();

        // Budget of 500 should drop roughly half (oldest turns).
        let dropped = proactive_trim_turns(&mut turns, 500);
        assert!(dropped > 0, "should have dropped some turns");
        assert!(turns.len() < 10, "should have fewer turns after trimming");
        // Last turn should always be preserved.
        assert!(
            turns.last().unwrap().content.starts_with("m9-"),
            "most recent turn must be preserved"
        );
        // Total chars should now be within budget.
        let total: usize = turns.iter().map(|t| t.content.chars().count()).sum();
        assert!(total <= 500, "total chars {total} should be within budget");
    }

    #[test]
    fn proactive_trim_noop_when_within_budget() {
        let mut turns = vec![
            ChatMessage::user("hello".to_string()),
            ChatMessage::assistant("hi there".to_string()),
        ];
        let dropped = proactive_trim_turns(&mut turns, 10_000);
        assert_eq!(dropped, 0);
        assert_eq!(turns.len(), 2);
    }

    #[test]
    fn proactive_trim_preserves_last_turn_even_when_over_budget() {
        let mut turns = vec![ChatMessage::user("x".repeat(2000))];
        let dropped = proactive_trim_turns(&mut turns, 100);
        assert_eq!(dropped, 0, "single turn must never be dropped");
        assert_eq!(turns.len(), 1);
    }

    struct DummyProvider;

    #[async_trait::async_trait]
    impl Provider for DummyProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok("ok".to_string())
        }
    }

    #[derive(Default)]
    struct RecordingChannel {
        sent_messages: tokio::sync::Mutex<Vec<String>>,
        start_typing_calls: AtomicUsize,
        stop_typing_calls: AtomicUsize,
        reactions_added: tokio::sync::Mutex<Vec<(String, String, String)>>,
        reactions_removed: tokio::sync::Mutex<Vec<(String, String, String)>>,
    }

    #[derive(Default)]
    struct TelegramRecordingChannel {
        sent_messages: tokio::sync::Mutex<Vec<String>>,
    }

    #[derive(Default)]
    struct SlackRecordingChannel {
        sent_messages: tokio::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Channel for TelegramRecordingChannel {
        fn name(&self) -> &str {
            "telegram"
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent_messages
                .lock()
                .await
                .push(format!("{}:{}", message.recipient, message.content));
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Channel for SlackRecordingChannel {
        fn name(&self) -> &str {
            "slack"
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent_messages
                .lock()
                .await
                .push(format!("{}:{}", message.recipient, message.content));
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Channel for RecordingChannel {
        fn name(&self) -> &str {
            "test-channel"
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent_messages
                .lock()
                .await
                .push(format!("{}:{}", message.recipient, message.content));
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
            self.start_typing_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
            self.stop_typing_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn add_reaction(
            &self,
            channel_id: &str,
            message_id: &str,
            emoji: &str,
        ) -> anyhow::Result<()> {
            self.reactions_added.lock().await.push((
                channel_id.to_string(),
                message_id.to_string(),
                emoji.to_string(),
            ));
            Ok(())
        }

        async fn remove_reaction(
            &self,
            channel_id: &str,
            message_id: &str,
            emoji: &str,
        ) -> anyhow::Result<()> {
            self.reactions_removed.lock().await.push((
                channel_id.to_string(),
                message_id.to_string(),
                emoji.to_string(),
            ));
            Ok(())
        }
    }

    struct SlowProvider {
        delay: Duration,
    }

    #[async_trait::async_trait]
    impl Provider for SlowProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            tokio::time::sleep(self.delay).await;
            Ok(format!("echo: {message}"))
        }
    }

    struct DelayedHistoryCaptureProvider {
        delay: Duration,
        calls: std::sync::Mutex<Vec<Vec<(String, String)>>>,
    }

    #[async_trait::async_trait]
    impl Provider for DelayedHistoryCaptureProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok("fallback".to_string())
        }

        async fn chat_with_history(
            &self,
            messages: &[ChatMessage],
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            let snapshot = messages
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect::<Vec<_>>();
            let call_index = {
                let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
                calls.push(snapshot);
                calls.len()
            };
            tokio::time::sleep(self.delay).await;
            Ok(format!("response-{call_index}"))
        }
    }

    struct NoopMemory;

    #[async_trait::async_trait]
    impl Memory for NoopMemory {
        fn name(&self) -> &str {
            "noop"
        }

        async fn store(
            &self,
            _key: &str,
            _content: &str,
            _category: fork_core::domain::memory::MemoryCategory,
            _session_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn recall(
            &self,
            _query: &str,
            _limit: usize,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<fork_core::domain::memory::MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn get(
            &self,
            _key: &str,
        ) -> anyhow::Result<Option<fork_core::domain::memory::MemoryEntry>> {
            Ok(None)
        }

        async fn list(
            &self,
            _category: Option<&fork_core::domain::memory::MemoryCategory>,
            _session_id: Option<&str>,
        ) -> anyhow::Result<Vec<fork_core::domain::memory::MemoryEntry>> {
            Ok(Vec::new())
        }

        async fn forget(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }

        async fn health_check(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn message_dispatch_processes_messages_in_parallel() {
        let channel_impl = Arc::new(RecordingChannel::default());
        let channel: Arc<dyn Channel> = channel_impl.clone();

        let mut channels_by_name = HashMap::new();
        channels_by_name.insert(channel.name().to_string(), channel);

        let runtime_ctx = Arc::new(ChannelRuntimeContext {
            channels_by_name: Arc::new(channels_by_name),
            provider: Arc::new(SlowProvider {
                delay: Duration::from_millis(250),
            }),
            default_provider: Arc::new("test-provider".to_string()),
            memory: Arc::new(NoopMemory),
            tools_registry: Arc::new(vec![]),
            observer: Arc::new(NoopObserver),
            system_prompt: Arc::new("test-system-prompt".to_string()),
            model: Arc::new("test-model".to_string()),
            temperature: 0.0,
            auto_save_memory: false,
            max_tool_iterations: 10,
            min_relevance_score: 0.0,
            conversation_histories: Arc::new(Mutex::new(HashMap::new())),
            provider_cache: Arc::new(Mutex::new(HashMap::new())),
            route_overrides: Arc::new(Mutex::new(HashMap::new())),
            api_key: None,
            api_url: None,
            reliability: Arc::new(crate::config::ReliabilityConfig::default()),
            provider_runtime_options: providers::ProviderRuntimeOptions::default(),
            workspace_dir: Arc::new(std::env::temp_dir()),
            message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
            interrupt_on_new_message: InterruptOnNewMessageConfig { enabled: false },
            multimodal: crate::config::MultimodalConfig::default(),
            hooks: None,
            non_cli_excluded_tools: Arc::new(Vec::new()),
            tool_call_dedup_exempt: Arc::new(Vec::new()),
            model_routes: Arc::new(Vec::new()),
            query_classification: crate::config::QueryClassificationConfig::default(),
            ack_reactions: true,
            show_tool_calls: true,
            session_store: None,
            summary_config: Arc::new(crate::config::schema::SummaryConfig::default()),
            summary_model: None,
            approval_manager: Arc::new(ApprovalManager::for_non_interactive(
                &crate::config::AutonomyConfig::default(),
            )),
            activated_tools: None,
            channel_registry: Some(Arc::new(
                crate::fork_adapters::channels::registry::CachedChannelRegistry::new(
                    crate::config::Config::default(),
                ),
            )),
            pipeline_store: None,
            pipeline_executor: None,
            message_router: None,
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(4);
        tx.send(traits::ChannelMessage {
            id: "1".to_string(),
            sender: "alice".to_string(),
            reply_target: "alice".to_string(),
            content: "hello".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 1,
            thread_ts: None,
        })
        .await
        .unwrap();
        tx.send(traits::ChannelMessage {
            id: "2".to_string(),
            sender: "bob".to_string(),
            reply_target: "bob".to_string(),
            content: "world".to_string(),
            channel: "test-channel".to_string(),
            timestamp: 2,
            thread_ts: None,
        })
        .await
        .unwrap();
        drop(tx);

        let started = Instant::now();
        run_message_dispatch_loop(rx, runtime_ctx, 2).await;
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(430),
            "expected parallel dispatch (<430ms), got {:?}",
            elapsed
        );

        let sent_messages = channel_impl.sent_messages.lock().await;
        assert_eq!(sent_messages.len(), 2);
    }

    #[tokio::test]
    async fn message_dispatch_interrupts_in_flight_telegram_request_and_preserves_context() {
        let channel_impl = Arc::new(TelegramRecordingChannel::default());
        let channel: Arc<dyn Channel> = channel_impl.clone();

        let mut channels_by_name = HashMap::new();
        channels_by_name.insert(channel.name().to_string(), channel);

        let provider_impl = Arc::new(DelayedHistoryCaptureProvider {
            delay: Duration::from_millis(250),
            calls: std::sync::Mutex::new(Vec::new()),
        });

        let runtime_ctx = Arc::new(ChannelRuntimeContext {
            channels_by_name: Arc::new(channels_by_name),
            provider: provider_impl.clone(),
            default_provider: Arc::new("test-provider".to_string()),
            memory: Arc::new(NoopMemory),
            tools_registry: Arc::new(vec![]),
            observer: Arc::new(NoopObserver),
            system_prompt: Arc::new("test-system-prompt".to_string()),
            model: Arc::new("test-model".to_string()),
            temperature: 0.0,
            auto_save_memory: false,
            max_tool_iterations: 10,
            min_relevance_score: 0.0,
            conversation_histories: Arc::new(Mutex::new(HashMap::new())),
            provider_cache: Arc::new(Mutex::new(HashMap::new())),
            route_overrides: Arc::new(Mutex::new(HashMap::new())),
            api_key: None,
            api_url: None,
            reliability: Arc::new(crate::config::ReliabilityConfig::default()),
            provider_runtime_options: providers::ProviderRuntimeOptions::default(),
            workspace_dir: Arc::new(std::env::temp_dir()),
            message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
            interrupt_on_new_message: InterruptOnNewMessageConfig { enabled: true },
            multimodal: crate::config::MultimodalConfig::default(),
            hooks: None,
            non_cli_excluded_tools: Arc::new(Vec::new()),
            tool_call_dedup_exempt: Arc::new(Vec::new()),
            model_routes: Arc::new(Vec::new()),
            query_classification: crate::config::QueryClassificationConfig::default(),
            ack_reactions: true,
            show_tool_calls: true,
            session_store: None,
            summary_config: Arc::new(crate::config::schema::SummaryConfig::default()),
            summary_model: None,
            approval_manager: Arc::new(ApprovalManager::for_non_interactive(
                &crate::config::AutonomyConfig::default(),
            )),
            activated_tools: None,
            channel_registry: Some(Arc::new(
                crate::fork_adapters::channels::registry::CachedChannelRegistry::new(
                    crate::config::Config::default(),
                ),
            )),
            pipeline_store: None,
            pipeline_executor: None,
            message_router: None,
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(8);
        let send_task = tokio::spawn(async move {
            tx.send(traits::ChannelMessage {
                id: "msg-1".to_string(),
                sender: "alice".to_string(),
                reply_target: "chat-1".to_string(),
                content: "forwarded content".to_string(),
                channel: "telegram".to_string(),
                timestamp: 1,
                thread_ts: None,
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            tx.send(traits::ChannelMessage {
                id: "msg-2".to_string(),
                sender: "alice".to_string(),
                reply_target: "chat-1".to_string(),
                content: "summarize this".to_string(),
                channel: "telegram".to_string(),
                timestamp: 2,
                thread_ts: None,
            })
            .await
            .unwrap();
        });

        run_message_dispatch_loop(rx, runtime_ctx, 4).await;
        send_task.await.unwrap();

        let sent_messages = channel_impl.sent_messages.lock().await;
        assert_eq!(sent_messages.len(), 1);
        assert!(sent_messages[0].starts_with("chat-1:"));
        assert!(sent_messages[0].contains("response-2"));
        drop(sent_messages);

        let calls = provider_impl
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(calls.len(), 2);
        let second_call = &calls[1];
        assert!(second_call
            .iter()
            .any(|(role, content)| { role == "user" && content.contains("forwarded content") }));
        assert!(second_call
            .iter()
            .any(|(role, content)| { role == "user" && content.contains("summarize this") }));
        assert!(
            !second_call.iter().any(|(role, _)| role == "assistant"),
            "cancelled turn should not persist an assistant response"
        );
    }

    #[tokio::test]
    async fn message_dispatch_interrupts_in_flight_slack_request_and_preserves_context() {
        let channel_impl = Arc::new(SlackRecordingChannel::default());
        let channel: Arc<dyn Channel> = channel_impl.clone();

        let mut channels_by_name = HashMap::new();
        channels_by_name.insert(channel.name().to_string(), channel);

        let provider_impl = Arc::new(DelayedHistoryCaptureProvider {
            delay: Duration::from_millis(250),
            calls: std::sync::Mutex::new(Vec::new()),
        });

        let runtime_ctx = Arc::new(ChannelRuntimeContext {
            channels_by_name: Arc::new(channels_by_name),
            provider: provider_impl.clone(),
            default_provider: Arc::new("test-provider".to_string()),
            memory: Arc::new(NoopMemory),
            tools_registry: Arc::new(vec![]),
            observer: Arc::new(NoopObserver),
            system_prompt: Arc::new("test-system-prompt".to_string()),
            model: Arc::new("test-model".to_string()),
            temperature: 0.0,
            auto_save_memory: false,
            max_tool_iterations: 10,
            min_relevance_score: 0.0,
            conversation_histories: Arc::new(Mutex::new(HashMap::new())),
            provider_cache: Arc::new(Mutex::new(HashMap::new())),
            route_overrides: Arc::new(Mutex::new(HashMap::new())),
            api_key: None,
            api_url: None,
            reliability: Arc::new(crate::config::ReliabilityConfig::default()),
            provider_runtime_options: providers::ProviderRuntimeOptions::default(),
            workspace_dir: Arc::new(std::env::temp_dir()),
            message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
            interrupt_on_new_message: InterruptOnNewMessageConfig { enabled: true },
            ack_reactions: true,
            show_tool_calls: true,
            session_store: None,
            summary_config: Arc::new(crate::config::schema::SummaryConfig::default()),
            summary_model: None,
            multimodal: crate::config::MultimodalConfig::default(),
            hooks: None,
            non_cli_excluded_tools: Arc::new(Vec::new()),
            tool_call_dedup_exempt: Arc::new(Vec::new()),
            model_routes: Arc::new(Vec::new()),
            approval_manager: Arc::new(ApprovalManager::for_non_interactive(
                &crate::config::AutonomyConfig::default(),
            )),
            activated_tools: None,
            query_classification: crate::config::QueryClassificationConfig::default(),
            channel_registry: Some(Arc::new(
                crate::fork_adapters::channels::registry::CachedChannelRegistry::new(
                    crate::config::Config::default(),
                ),
            )),
            pipeline_store: None,
            pipeline_executor: None,
            message_router: None,
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(8);
        let send_task = tokio::spawn(async move {
            tx.send(traits::ChannelMessage {
                id: "msg-1".to_string(),
                sender: "U123".to_string(),
                reply_target: "C123".to_string(),
                content: "first question".to_string(),
                channel: "slack".to_string(),
                timestamp: 1,
                thread_ts: Some("1741234567.100001".to_string()),
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            tx.send(traits::ChannelMessage {
                id: "msg-2".to_string(),
                sender: "U123".to_string(),
                reply_target: "C123".to_string(),
                content: "second question".to_string(),
                channel: "slack".to_string(),
                timestamp: 2,
                thread_ts: Some("1741234567.100001".to_string()),
            })
            .await
            .unwrap();
        });

        run_message_dispatch_loop(rx, runtime_ctx, 4).await;
        send_task.await.unwrap();

        let sent_messages = channel_impl.sent_messages.lock().await;
        assert_eq!(sent_messages.len(), 1);
        assert!(sent_messages[0].starts_with("C123:"));
        assert!(sent_messages[0].contains("response-2"));
        drop(sent_messages);

        let calls = provider_impl
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(calls.len(), 2);
        let second_call = &calls[1];
        assert!(second_call
            .iter()
            .any(|(role, content)| { role == "user" && content.contains("first question") }));
        assert!(second_call
            .iter()
            .any(|(role, content)| { role == "user" && content.contains("second question") }));
        assert!(
            !second_call.iter().any(|(role, _)| role == "assistant"),
            "cancelled turn should not persist an assistant response"
        );
    }

    #[tokio::test]
    async fn message_dispatch_interrupt_scope_is_same_sender_same_chat() {
        let channel_impl = Arc::new(TelegramRecordingChannel::default());
        let channel: Arc<dyn Channel> = channel_impl.clone();

        let mut channels_by_name = HashMap::new();
        channels_by_name.insert(channel.name().to_string(), channel);

        let runtime_ctx = Arc::new(ChannelRuntimeContext {
            channels_by_name: Arc::new(channels_by_name),
            provider: Arc::new(SlowProvider {
                delay: Duration::from_millis(180),
            }),
            default_provider: Arc::new("test-provider".to_string()),
            memory: Arc::new(NoopMemory),
            tools_registry: Arc::new(vec![]),
            observer: Arc::new(NoopObserver),
            system_prompt: Arc::new("test-system-prompt".to_string()),
            model: Arc::new("test-model".to_string()),
            temperature: 0.0,
            auto_save_memory: false,
            max_tool_iterations: 10,
            min_relevance_score: 0.0,
            conversation_histories: Arc::new(Mutex::new(HashMap::new())),
            provider_cache: Arc::new(Mutex::new(HashMap::new())),
            route_overrides: Arc::new(Mutex::new(HashMap::new())),
            api_key: None,
            api_url: None,
            reliability: Arc::new(crate::config::ReliabilityConfig::default()),
            provider_runtime_options: providers::ProviderRuntimeOptions::default(),
            workspace_dir: Arc::new(std::env::temp_dir()),
            message_timeout_secs: CHANNEL_MESSAGE_TIMEOUT_SECS,
            interrupt_on_new_message: InterruptOnNewMessageConfig { enabled: true },
            multimodal: crate::config::MultimodalConfig::default(),
            hooks: None,
            non_cli_excluded_tools: Arc::new(Vec::new()),
            tool_call_dedup_exempt: Arc::new(Vec::new()),
            model_routes: Arc::new(Vec::new()),
            query_classification: crate::config::QueryClassificationConfig::default(),
            ack_reactions: true,
            show_tool_calls: true,
            session_store: None,
            summary_config: Arc::new(crate::config::schema::SummaryConfig::default()),
            summary_model: None,
            approval_manager: Arc::new(ApprovalManager::for_non_interactive(
                &crate::config::AutonomyConfig::default(),
            )),
            activated_tools: None,
            channel_registry: Some(Arc::new(
                crate::fork_adapters::channels::registry::CachedChannelRegistry::new(
                    crate::config::Config::default(),
                ),
            )),
            pipeline_store: None,
            pipeline_executor: None,
            message_router: None,
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(8);
        let send_task = tokio::spawn(async move {
            tx.send(traits::ChannelMessage {
                id: "msg-a".to_string(),
                sender: "alice".to_string(),
                reply_target: "chat-1".to_string(),
                content: "first chat".to_string(),
                channel: "telegram".to_string(),
                timestamp: 1,
                thread_ts: None,
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            tx.send(traits::ChannelMessage {
                id: "msg-b".to_string(),
                sender: "alice".to_string(),
                reply_target: "chat-2".to_string(),
                content: "second chat".to_string(),
                channel: "telegram".to_string(),
                timestamp: 2,
                thread_ts: None,
            })
            .await
            .unwrap();
        });

        run_message_dispatch_loop(rx, runtime_ctx, 4).await;
        send_task.await.unwrap();

        let sent_messages = channel_impl.sent_messages.lock().await;
        assert_eq!(sent_messages.len(), 2);
        assert!(sent_messages.iter().any(|msg| msg.starts_with("chat-1:")));
        assert!(sent_messages.iter().any(|msg| msg.starts_with("chat-2:")));
    }

    #[test]
    fn prompt_contains_all_sections() {
        let ws = make_workspace();
        let tools = vec![("shell", "Run commands"), ("file_read", "Read files")];
        let prompt = build_system_prompt(ws.path(), "test-model", &tools, &[], None, None);

        // Section headers
        assert!(prompt.contains("## Tools"), "missing Tools section");
        assert!(prompt.contains("## Safety"), "missing Safety section");
        assert!(prompt.contains("## Workspace"), "missing Workspace section");
        assert!(
            prompt.contains("## Project Context"),
            "missing Project Context"
        );
        assert!(
            prompt.contains("## Current Date & Time"),
            "missing Date/Time"
        );
        assert!(prompt.contains("## Runtime"), "missing Runtime section");
    }

    #[test]
    fn prompt_injects_tools() {
        let ws = make_workspace();
        let tools = vec![
            ("shell", "Run commands"),
            ("memory_recall", "Search memory"),
        ];
        let prompt = build_system_prompt(ws.path(), "gpt-4o", &tools, &[], None, None);

        assert!(prompt.contains("**shell**"));
        assert!(prompt.contains("Run commands"));
        assert!(prompt.contains("**memory_recall**"));
    }

    #[test]
    fn prompt_includes_single_tool_protocol_block_after_append() {
        let ws = make_workspace();
        let tools = vec![("shell", "Run commands")];
        let mut prompt = build_system_prompt(ws.path(), "gpt-4o", &tools, &[], None, None);

        assert!(
            !prompt.contains("## Tool Use Protocol"),
            "build_system_prompt should not emit protocol block directly"
        );

        prompt.push_str(&build_tool_instructions(&[]));

        assert_eq!(
            prompt.matches("## Tool Use Protocol").count(),
            1,
            "protocol block should appear exactly once in the final prompt"
        );
    }

    #[test]
    fn prompt_injects_safety() {
        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        assert!(prompt.contains("Do not exfiltrate private data"));
        assert!(prompt.contains("Do not run destructive commands"));
        assert!(prompt.contains("Prefer `trash` over `rm`"));
    }

    #[test]
    fn prompt_injects_workspace_files() {
        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        assert!(prompt.contains("### SOUL.md"), "missing SOUL.md header");
        assert!(prompt.contains("Be helpful"), "missing SOUL content");
        assert!(prompt.contains("### IDENTITY.md"), "missing IDENTITY.md");
        assert!(
            prompt.contains("Name: SynapseClaw"),
            "missing IDENTITY content"
        );
        assert!(prompt.contains("### USER.md"), "missing USER.md");
        assert!(prompt.contains("### AGENTS.md"), "missing AGENTS.md");
        assert!(prompt.contains("### TOOLS.md"), "missing TOOLS.md");
        // HEARTBEAT.md is intentionally excluded from channel prompts — it's only
        // relevant to the heartbeat worker and causes LLMs to emit spurious
        // "HEARTBEAT_OK" acknowledgments in channel conversations.
        assert!(
            !prompt.contains("### HEARTBEAT.md"),
            "HEARTBEAT.md should not be in channel prompt"
        );
        assert!(prompt.contains("### MEMORY.md"), "missing MEMORY.md");
        assert!(prompt.contains("User likes Rust"), "missing MEMORY content");
    }

    #[test]
    fn prompt_missing_file_markers() {
        let tmp = TempDir::new().unwrap();
        // Empty workspace — no files at all
        let prompt = build_system_prompt(tmp.path(), "model", &[], &[], None, None);

        assert!(prompt.contains("[File not found: SOUL.md]"));
        assert!(prompt.contains("[File not found: AGENTS.md]"));
        assert!(prompt.contains("[File not found: IDENTITY.md]"));
    }

    #[test]
    fn prompt_bootstrap_only_if_exists() {
        let ws = make_workspace();
        // No BOOTSTRAP.md — should not appear
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);
        assert!(
            !prompt.contains("### BOOTSTRAP.md"),
            "BOOTSTRAP.md should not appear when missing"
        );

        // Create BOOTSTRAP.md — should appear
        std::fs::write(ws.path().join("BOOTSTRAP.md"), "# Bootstrap\nFirst run.").unwrap();
        let prompt2 = build_system_prompt(ws.path(), "model", &[], &[], None, None);
        assert!(
            prompt2.contains("### BOOTSTRAP.md"),
            "BOOTSTRAP.md should appear when present"
        );
        assert!(prompt2.contains("First run"));
    }

    #[test]
    fn prompt_no_daily_memory_injection() {
        let ws = make_workspace();
        let memory_dir = ws.path().join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        std::fs::write(
            memory_dir.join(format!("{today}.md")),
            "# Daily\nSome note.",
        )
        .unwrap();

        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        // Daily notes should NOT be in the system prompt (on-demand via tools)
        assert!(
            !prompt.contains("Daily Notes"),
            "daily notes should not be auto-injected"
        );
        assert!(
            !prompt.contains("Some note"),
            "daily content should not be in prompt"
        );
    }

    #[test]
    fn prompt_runtime_metadata() {
        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "claude-sonnet-4", &[], &[], None, None);

        assert!(prompt.contains("Model: claude-sonnet-4"));
        assert!(prompt.contains(&format!("OS: {}", std::env::consts::OS)));
        assert!(prompt.contains("Host:"));
    }

    #[test]
    fn prompt_skills_include_instructions_and_tools() {
        let ws = make_workspace();
        let skills = vec![crate::skills::Skill {
            name: "code-review".into(),
            description: "Review code for bugs".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "lint".into(),
                description: "Run static checks".into(),
                kind: "shell".into(),
                command: "cargo clippy".into(),
                args: HashMap::new(),
            }],
            prompts: vec!["Always run cargo test before final response.".into()],
            location: None,
        }];

        let prompt = build_system_prompt(ws.path(), "model", &[], &skills, None, None);

        assert!(prompt.contains("<available_skills>"), "missing skills XML");
        assert!(prompt.contains("<name>code-review</name>"));
        assert!(prompt.contains("<description>Review code for bugs</description>"));
        assert!(prompt.contains("SKILL.md</location>"));
        assert!(prompt.contains("<instructions>"));
        assert!(prompt
            .contains("<instruction>Always run cargo test before final response.</instruction>"));
        assert!(prompt.contains("<tools>"));
        assert!(prompt.contains("<name>lint</name>"));
        assert!(prompt.contains("<kind>shell</kind>"));
        assert!(!prompt.contains("loaded on demand"));
    }

    #[test]
    fn prompt_skills_compact_mode_omits_instructions_and_tools() {
        let ws = make_workspace();
        let skills = vec![crate::skills::Skill {
            name: "code-review".into(),
            description: "Review code for bugs".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "lint".into(),
                description: "Run static checks".into(),
                kind: "shell".into(),
                command: "cargo clippy".into(),
                args: HashMap::new(),
            }],
            prompts: vec!["Always run cargo test before final response.".into()],
            location: None,
        }];

        let prompt = build_system_prompt_with_mode(
            ws.path(),
            "model",
            &[],
            &skills,
            None,
            None,
            false,
            crate::config::SkillsPromptInjectionMode::Compact,
        );

        assert!(prompt.contains("<available_skills>"), "missing skills XML");
        assert!(prompt.contains("<name>code-review</name>"));
        assert!(prompt.contains("<location>skills/code-review/SKILL.md</location>"));
        assert!(prompt.contains("loaded on demand"));
        assert!(!prompt.contains("<instructions>"));
        assert!(!prompt
            .contains("<instruction>Always run cargo test before final response.</instruction>"));
        assert!(!prompt.contains("<tools>"));
    }

    #[test]
    fn prompt_skills_escape_reserved_xml_chars() {
        let ws = make_workspace();
        let skills = vec![crate::skills::Skill {
            name: "code<review>&".into(),
            description: "Review \"unsafe\" and 'risky' bits".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "run\"linter\"".into(),
                description: "Run <lint> & report".into(),
                kind: "shell&exec".into(),
                command: "cargo clippy".into(),
                args: HashMap::new(),
            }],
            prompts: vec!["Use <tool_call> and & keep output \"safe\"".into()],
            location: None,
        }];

        let prompt = build_system_prompt(ws.path(), "model", &[], &skills, None, None);

        assert!(prompt.contains("<name>code&lt;review&gt;&amp;</name>"));
        assert!(prompt.contains(
            "<description>Review &quot;unsafe&quot; and &apos;risky&apos; bits</description>"
        ));
        assert!(prompt.contains("<name>run&quot;linter&quot;</name>"));
        assert!(prompt.contains("<description>Run &lt;lint&gt; &amp; report</description>"));
        assert!(prompt.contains("<kind>shell&amp;exec</kind>"));
        assert!(prompt.contains(
            "<instruction>Use &lt;tool_call&gt; and &amp; keep output &quot;safe&quot;</instruction>"
        ));
    }

    #[test]
    fn prompt_truncation() {
        let ws = make_workspace();
        // Write a file larger than BOOTSTRAP_MAX_CHARS
        let big_content = "x".repeat(BOOTSTRAP_MAX_CHARS + 1000);
        std::fs::write(ws.path().join("AGENTS.md"), &big_content).unwrap();

        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        assert!(
            prompt.contains("truncated at"),
            "large files should be truncated"
        );
        assert!(
            !prompt.contains(&big_content),
            "full content should not appear"
        );
    }

    #[test]
    fn prompt_empty_files_skipped() {
        let ws = make_workspace();
        std::fs::write(ws.path().join("TOOLS.md"), "").unwrap();

        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        // Empty file should not produce a header
        assert!(
            !prompt.contains("### TOOLS.md"),
            "empty files should be skipped"
        );
    }

    #[test]
    fn channel_log_truncation_is_utf8_safe_for_multibyte_text() {
        let msg = "Hello from SynapseClaw 🌍. Current status is healthy, and café-style UTF-8 text stays safe in logs.";

        // Reproduces the production crash path where channel logs truncate at 80 chars.
        let result =
            std::panic::catch_unwind(|| fork_core::domain::util::truncate_with_ellipsis(msg, 80));
        assert!(
            result.is_ok(),
            "truncate_with_ellipsis should never panic on UTF-8"
        );

        let truncated = result.unwrap();
        assert!(!truncated.is_empty());
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn prompt_contains_channel_capabilities() {
        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        assert!(
            prompt.contains("## Channel Capabilities"),
            "missing Channel Capabilities section"
        );
        assert!(
            prompt.contains("running as a messaging bot"),
            "missing channel context"
        );
        assert!(
            prompt.contains("NEVER repeat, describe, or echo credentials"),
            "missing security instruction"
        );
    }

    #[test]
    fn prompt_workspace_path() {
        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        assert!(prompt.contains(&format!("Working directory: `{}`", ws.path().display())));
    }

    #[test]
    fn channel_notify_observer_truncates_utf8_arguments_safely() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let observer = ChannelNotifyObserver {
            inner: Arc::new(NoopObserver),
            tx,
            tools_used: AtomicBool::new(false),
        };

        let payload = (0..300)
            .map(|n| serde_json::json!({ "content": format!("{}置tail", "a".repeat(n)) }))
            .map(|v| v.to_string())
            .find(|raw| raw.len() > 120 && !raw.is_char_boundary(120))
            .expect("should produce non-char-boundary data at byte index 120");

        observer.record_event(
            &crate::fork_adapters::observability::traits::ObserverEvent::ToolCallStart {
                tool: "file_write".to_string(),
                arguments: Some(payload),
            },
        );

        let emitted = rx.try_recv().expect("observer should emit notify message");
        assert!(emitted.contains("`file_write`"));
        assert!(emitted.is_char_boundary(emitted.len()));
    }

    #[test]
    fn strip_isolated_tool_json_artifacts_removes_tool_calls_and_results() {
        let mut known_tools = HashSet::new();
        known_tools.insert("schedule".to_string());

        let input = r#"{"name":"schedule","parameters":{"action":"create","message":"test"}}
{"name":"schedule","parameters":{"action":"cancel","task_id":"test"}}
Let me create the reminder properly:
{"name":"schedule","parameters":{"action":"create","message":"Go to sleep"}}
{"result":{"task_id":"abc","status":"scheduled"}}
Done reminder set for 1:38 AM."#;

        let result = strip_isolated_tool_json_artifacts(input, &known_tools);
        let normalized = result
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            normalized,
            "Let me create the reminder properly:\nDone reminder set for 1:38 AM."
        );
    }

    #[test]
    fn strip_isolated_tool_json_artifacts_preserves_non_tool_json() {
        let mut known_tools = HashSet::new();
        known_tools.insert("shell".to_string());

        let input = r#"{"name":"profile","parameters":{"timezone":"UTC"}}
This is an example JSON object for profile settings."#;

        let result = strip_isolated_tool_json_artifacts(input, &known_tools);
        assert_eq!(result, input);
    }

    // ── AIEOS Identity Tests (Issue #168) ─────────────────────────

    #[test]
    fn aieos_identity_from_file() {
        use crate::config::IdentityConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let identity_path = tmp.path().join("aieos_identity.json");

        // Write AIEOS identity file
        let aieos_json = r#"{
            "identity": {
                "names": {"first": "Nova", "nickname": "Nov"},
                "bio": "A helpful AI assistant.",
                "origin": "Silicon Valley"
            },
            "psychology": {
                "mbti": "INTJ",
                "moral_compass": ["Be helpful", "Do no harm"]
            },
            "linguistics": {
                "style": "concise",
                "formality": "casual"
            }
        }"#;
        std::fs::write(&identity_path, aieos_json).unwrap();

        // Create identity config pointing to the file
        let config = IdentityConfig {
            format: "aieos".into(),
            aieos_path: Some("aieos_identity.json".into()),
            aieos_inline: None,
        };

        let prompt = build_system_prompt(tmp.path(), "model", &[], &[], Some(&config), None);

        // Should contain AIEOS sections
        assert!(prompt.contains("## Identity"));
        assert!(prompt.contains("**Name:** Nova"));
        assert!(prompt.contains("**Nickname:** Nov"));
        assert!(prompt.contains("**Bio:** A helpful AI assistant."));
        assert!(prompt.contains("**Origin:** Silicon Valley"));

        assert!(prompt.contains("## Personality"));
        assert!(prompt.contains("**MBTI:** INTJ"));
        assert!(prompt.contains("**Moral Compass:**"));
        assert!(prompt.contains("- Be helpful"));

        assert!(prompt.contains("## Communication Style"));
        assert!(prompt.contains("**Style:** concise"));
        assert!(prompt.contains("**Formality Level:** casual"));

        // Should NOT contain OpenClaw bootstrap file headers
        assert!(!prompt.contains("### SOUL.md"));
        assert!(!prompt.contains("### IDENTITY.md"));
        assert!(!prompt.contains("[File not found"));
    }

    #[test]
    fn aieos_identity_from_inline() {
        use crate::config::IdentityConfig;

        let config = IdentityConfig {
            format: "aieos".into(),
            aieos_path: None,
            aieos_inline: Some(r#"{"identity":{"names":{"first":"Claw"}}}"#.into()),
        };

        let prompt = build_system_prompt(
            std::env::temp_dir().as_path(),
            "model",
            &[],
            &[],
            Some(&config),
            None,
        );

        assert!(prompt.contains("**Name:** Claw"));
        assert!(prompt.contains("## Identity"));
    }

    #[test]
    fn aieos_fallback_to_openclaw_on_parse_error() {
        use crate::config::IdentityConfig;

        let config = IdentityConfig {
            format: "aieos".into(),
            aieos_path: Some("nonexistent.json".into()),
            aieos_inline: None,
        };

        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], Some(&config), None);

        // Should fall back to OpenClaw format when AIEOS file is not found
        // (Error is logged to stderr with filename, not included in prompt)
        assert!(prompt.contains("### SOUL.md"));
    }

    #[test]
    fn aieos_empty_uses_openclaw() {
        use crate::config::IdentityConfig;

        // Format is "aieos" but neither path nor inline is set
        let config = IdentityConfig {
            format: "aieos".into(),
            aieos_path: None,
            aieos_inline: None,
        };

        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], Some(&config), None);

        // Should use OpenClaw format (not configured for AIEOS)
        assert!(prompt.contains("### SOUL.md"));
        assert!(prompt.contains("Be helpful"));
    }

    #[test]
    fn openclaw_format_uses_bootstrap_files() {
        use crate::config::IdentityConfig;

        let config = IdentityConfig {
            format: "openclaw".into(),
            aieos_path: Some("identity.json".into()),
            aieos_inline: None,
        };

        let ws = make_workspace();
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], Some(&config), None);

        // Should use OpenClaw format even if aieos_path is set
        assert!(prompt.contains("### SOUL.md"));
        assert!(prompt.contains("Be helpful"));
        assert!(!prompt.contains("## Identity"));
    }

    #[test]
    fn none_identity_config_uses_openclaw() {
        let ws = make_workspace();
        // Pass None for identity config
        let prompt = build_system_prompt(ws.path(), "model", &[], &[], None, None);

        // Should use OpenClaw format
        assert!(prompt.contains("### SOUL.md"));
        assert!(prompt.contains("Be helpful"));
    }

    #[test]
    fn classify_health_ok_true() {
        let state = classify_health_result(&Ok(true));
        assert_eq!(state, ChannelHealthState::Healthy);
    }

    #[test]
    fn classify_health_ok_false() {
        let state = classify_health_result(&Ok(false));
        assert_eq!(state, ChannelHealthState::Unhealthy);
    }

    #[tokio::test]
    async fn classify_health_timeout() {
        let result = tokio::time::timeout(Duration::from_millis(1), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            true
        })
        .await;
        let state = classify_health_result(&result);
        assert_eq!(state, ChannelHealthState::Timeout);
    }

    #[test]
    fn collect_configured_channels_includes_mattermost_when_configured() {
        let mut config = Config::default();
        config.channels_config.mattermost = Some(crate::config::schema::MattermostConfig {
            url: "https://mattermost.example.com".to_string(),
            bot_token: "test-token".to_string(),
            channel_id: Some("channel-1".to_string()),
            allowed_users: vec![],
            thread_replies: Some(true),
            mention_only: Some(false),
        });

        let channels = collect_configured_channels(&config, "test");

        assert!(channels
            .iter()
            .any(|entry| entry.display_name == "Mattermost"));
        assert!(channels
            .iter()
            .any(|entry| entry.channel.name() == "mattermost"));
    }

    struct AlwaysFailChannel {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    struct BlockUntilClosedChannel {
        name: String,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Channel for AlwaysFailChannel {
        fn name(&self) -> &str {
            self.name
        }

        async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
        ) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("listen boom")
        }
    }

    #[async_trait::async_trait]
    impl Channel for BlockUntilClosedChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn send(&self, _message: &SendMessage) -> anyhow::Result<()> {
            Ok(())
        }

        async fn listen(
            &self,
            tx: tokio::sync::mpsc::Sender<traits::ChannelMessage>,
        ) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tx.closed().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn supervised_listener_marks_error_and_restarts_on_failures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let channel: Arc<dyn Channel> = Arc::new(AlwaysFailChannel {
            name: "test-supervised-fail",
            calls: Arc::clone(&calls),
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(1);
        let handle = spawn_supervised_listener(channel, tx, 1, 1);

        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(rx);
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::fork_adapters::health::snapshot_json();
        let component = &snapshot["components"]["channel:test-supervised-fail"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(component["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("listen boom"));
        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn supervised_listener_refreshes_health_while_running() {
        let calls = Arc::new(AtomicUsize::new(0));
        let channel_name = format!("test-supervised-heartbeat-{}", uuid::Uuid::new_v4());
        let component_name = format!("channel:{channel_name}");
        let channel: Arc<dyn Channel> = Arc::new(BlockUntilClosedChannel {
            name: channel_name,
            calls: Arc::clone(&calls),
        });

        let (tx, rx) = tokio::sync::mpsc::channel::<traits::ChannelMessage>(1);
        let handle = spawn_supervised_listener_with_health_interval(
            channel,
            tx,
            1,
            1,
            Duration::from_millis(20),
        );

        tokio::time::sleep(Duration::from_millis(35)).await;
        let first_last_ok = crate::fork_adapters::health::snapshot_json()["components"]
            [&component_name]["last_ok"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(!first_last_ok.is_empty());

        tokio::time::sleep(Duration::from_millis(70)).await;
        let second_last_ok = crate::fork_adapters::health::snapshot_json()["components"]
            [&component_name]["last_ok"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let first = chrono::DateTime::parse_from_rfc3339(&first_last_ok)
            .expect("last_ok should be valid RFC3339");
        let second = chrono::DateTime::parse_from_rfc3339(&second_last_ok)
            .expect("last_ok should be valid RFC3339");
        assert!(second > first, "expected periodic health heartbeat refresh");

        drop(rx);
        let join = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(join.is_ok(), "listener should stop after channel shutdown");
        assert!(calls.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn maybe_restart_daemon_systemd_args_regression() {
        assert_eq!(
            SYSTEMD_STATUS_ARGS,
            ["--user", "is-active", "synapseclaw.service"]
        );
        assert_eq!(
            SYSTEMD_RESTART_ARGS,
            ["--user", "restart", "synapseclaw.service"]
        );
    }

    #[test]
    fn maybe_restart_daemon_openrc_args_regression() {
        assert_eq!(OPENRC_STATUS_ARGS, ["synapseclaw", "status"]);
        assert_eq!(OPENRC_RESTART_ARGS, ["synapseclaw", "restart"]);
    }

    #[test]
    fn normalize_merges_consecutive_user_turns() {
        let turns = vec![ChatMessage::user("hello"), ChatMessage::user("world")];
        let result = normalize_cached_channel_turns(turns);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[0].content, "hello\n\nworld");
    }

    #[test]
    fn normalize_preserves_strict_alternation() {
        let turns = vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("bye"),
        ];
        let result = normalize_cached_channel_turns(turns);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "hello");
        assert_eq!(result[1].content, "hi");
        assert_eq!(result[2].content, "bye");
    }

    #[test]
    fn normalize_merges_multiple_consecutive_user_turns() {
        let turns = vec![
            ChatMessage::user("a"),
            ChatMessage::user("b"),
            ChatMessage::user("c"),
        ];
        let result = normalize_cached_channel_turns(turns);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[0].content, "a\n\nb\n\nc");
    }

    #[test]
    fn normalize_empty_input() {
        let result = normalize_cached_channel_turns(vec![]);
        assert!(result.is_empty());
    }

    // ── E2E: photo [IMAGE:] marker rejected by non-vision provider ───

    #[test]
    fn build_channel_by_id_unknown_channel_returns_error() {
        let config = Config::default();
        match build_channel_by_id(&config, "nonexistent") {
            Err(e) => {
                let err_msg = e.to_string();
                assert!(
                    err_msg.contains("Unknown channel"),
                    "expected 'Unknown channel' in error, got: {err_msg}"
                );
            }
            Ok(_) => panic!("should fail for unknown channel"),
        }
    }

    #[test]
    fn build_channel_by_id_unconfigured_telegram_returns_error() {
        let config = Config::default();
        match build_channel_by_id(&config, "telegram") {
            Err(e) => {
                let err_msg = e.to_string();
                assert!(
                    err_msg.contains("not configured"),
                    "expected 'not configured' in error, got: {err_msg}"
                );
            }
            Ok(_) => panic!("should fail when telegram is not configured"),
        }
    }

    #[test]
    fn build_channel_by_id_configured_telegram_succeeds() {
        let mut config = Config::default();
        config.channels_config.telegram = Some(crate::config::schema::TelegramConfig {
            bot_token: "test-token".to_string(),
            allowed_users: vec![],
            stream_mode: crate::config::StreamMode::Off,
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
        });
        match build_channel_by_id(&config, "telegram") {
            Ok(channel) => assert_eq!(channel.name(), "telegram"),
            Err(e) => panic!("should succeed when telegram is configured: {e}"),
        }
    }
}
