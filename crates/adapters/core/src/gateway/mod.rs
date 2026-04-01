//! Axum-based HTTP gateway with proper HTTP/1.1 compliance, body limits, and timeouts.
//!
//! This module replaces the raw TCP implementation with axum for:
//! - Proper HTTP/1.1 parsing and compliance
//! - Content-Length validation (handled by hyper)
//! - Request body size limits (64KB max)
//! - Request timeouts (30s) to prevent slow-loris attacks
//! - Header sanitization (handled by axum/hyper)

pub mod agent_registry;
pub mod api;
pub mod chat_db;
pub mod ipc;
pub mod nodes;
pub mod provisioning;
pub mod sse;
pub mod static_files;
pub mod ws;

use crate::channels::{
    Channel, LinqChannel, NextcloudTalkChannel, SendMessage, WatiChannel, WhatsAppChannel,
};
use crate::cost::CostTracker;
use crate::runtime;
use crate::tools;
use crate::tools::traits::ToolSpec;
use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{delete, get, post, put},
    Router,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::util::truncate_with_ellipsis;
use synapse_infra::config_io::ConfigIO;
use synapse_memory::{self, MemoryCategory, UnifiedMemoryPort};
use synapse_providers::{self, ChatMessage, Provider};
use synapse_security::pairing::{constant_time_eq, is_public_bind, PairingGuard};
use synapse_security::security_factory::security_policy_from_config;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use uuid::Uuid;

/// Maximum request body size (64KB) — prevents memory exhaustion
pub const MAX_BODY_SIZE: usize = 65_536;
/// Request timeout (30s) — prevents slow-loris attacks
pub const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Sliding window used by gateway rate limiting.
pub const RATE_LIMIT_WINDOW_SECS: u64 = 60;
/// Fallback max distinct client keys tracked in gateway rate limiter.
pub const RATE_LIMIT_MAX_KEYS_DEFAULT: usize = 10_000;
/// Fallback max distinct idempotency keys retained in gateway memory.
pub const IDEMPOTENCY_MAX_KEYS_DEFAULT: usize = 10_000;

fn webhook_memory_key() -> String {
    format!("webhook_msg_{}", Uuid::new_v4())
}

fn whatsapp_memory_key(msg: &crate::channels::traits::ChannelMessage) -> String {
    format!("whatsapp_{}_{}", msg.sender, msg.id)
}

fn linq_memory_key(msg: &crate::channels::traits::ChannelMessage) -> String {
    format!("linq_{}_{}", msg.sender, msg.id)
}

fn wati_memory_key(msg: &crate::channels::traits::ChannelMessage) -> String {
    format!("wati_{}_{}", msg.sender, msg.id)
}

fn nextcloud_talk_memory_key(msg: &crate::channels::traits::ChannelMessage) -> String {
    format!("nextcloud_talk_{}_{}", msg.sender, msg.id)
}

fn sender_session_id(channel: &str, msg: &crate::channels::traits::ChannelMessage) -> String {
    match &msg.thread_ts {
        Some(thread_id) => format!("{channel}_{thread_id}_{}", msg.sender),
        None => format!("{channel}_{}", msg.sender),
    }
}

fn webhook_session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("X-Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn hash_webhook_secret(value: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(value.as_bytes());
    hex::encode(digest)
}

/// How often the rate limiter sweeps stale IP entries from its map.
const RATE_LIMITER_SWEEP_INTERVAL_SECS: u64 = 300; // 5 minutes

#[derive(Debug)]
pub struct SlidingWindowRateLimiter {
    limit_per_window: u32,
    window: Duration,
    max_keys: usize,
    requests: Mutex<(HashMap<String, Vec<Instant>>, Instant)>,
}

impl SlidingWindowRateLimiter {
    fn new(limit_per_window: u32, window: Duration, max_keys: usize) -> Self {
        Self {
            limit_per_window,
            window,
            max_keys: max_keys.max(1),
            requests: Mutex::new((HashMap::new(), Instant::now())),
        }
    }

    fn prune_stale(requests: &mut HashMap<String, Vec<Instant>>, cutoff: Instant) {
        requests.retain(|_, timestamps| {
            timestamps.retain(|t| *t > cutoff);
            !timestamps.is_empty()
        });
    }

    fn allow(&self, key: &str) -> bool {
        if self.limit_per_window == 0 {
            return true;
        }

        let now = Instant::now();
        let cutoff = now.checked_sub(self.window).unwrap_or_else(Instant::now);

        let mut guard = self.requests.lock();
        let (requests, last_sweep) = &mut *guard;

        // Periodic sweep: remove keys with no recent requests
        if last_sweep.elapsed() >= Duration::from_secs(RATE_LIMITER_SWEEP_INTERVAL_SECS) {
            Self::prune_stale(requests, cutoff);
            *last_sweep = now;
        }

        if !requests.contains_key(key) && requests.len() >= self.max_keys {
            // Opportunistic stale cleanup before eviction under cardinality pressure.
            Self::prune_stale(requests, cutoff);
            *last_sweep = now;

            if requests.len() >= self.max_keys {
                let evict_key = requests
                    .iter()
                    .min_by_key(|(_, timestamps)| timestamps.last().copied().unwrap_or(cutoff))
                    .map(|(k, _)| k.clone());
                if let Some(evict_key) = evict_key {
                    requests.remove(&evict_key);
                }
            }
        }

        let entry = requests.entry(key.to_owned()).or_default();
        entry.retain(|instant| *instant > cutoff);

        if entry.len() >= self.limit_per_window as usize {
            return false;
        }

        entry.push(now);
        true
    }
}

#[derive(Debug)]
pub struct GatewayRateLimiter {
    pair: SlidingWindowRateLimiter,
    webhook: SlidingWindowRateLimiter,
}

impl GatewayRateLimiter {
    pub(crate) fn new(pair_per_minute: u32, webhook_per_minute: u32, max_keys: usize) -> Self {
        let window = Duration::from_secs(RATE_LIMIT_WINDOW_SECS);
        Self {
            pair: SlidingWindowRateLimiter::new(pair_per_minute, window, max_keys),
            webhook: SlidingWindowRateLimiter::new(webhook_per_minute, window, max_keys),
        }
    }

    fn allow_pair(&self, key: &str) -> bool {
        self.pair.allow(key)
    }

    fn allow_webhook(&self, key: &str) -> bool {
        self.webhook.allow(key)
    }
}

#[derive(Debug)]
pub struct IdempotencyStore {
    ttl: Duration,
    max_keys: usize,
    keys: Mutex<HashMap<String, Instant>>,
}

impl IdempotencyStore {
    pub(crate) fn new(ttl: Duration, max_keys: usize) -> Self {
        Self {
            ttl,
            max_keys: max_keys.max(1),
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// Returns true if this key is new and is now recorded.
    fn record_if_new(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut keys = self.keys.lock();

        keys.retain(|_, seen_at| now.duration_since(*seen_at) < self.ttl);

        if keys.contains_key(key) {
            return false;
        }

        if keys.len() >= self.max_keys {
            let evict_key = keys
                .iter()
                .min_by_key(|(_, seen_at)| *seen_at)
                .map(|(k, _)| k.clone());
            if let Some(evict_key) = evict_key {
                keys.remove(&evict_key);
            }
        }

        keys.insert(key.to_owned(), now);
        true
    }
}

fn parse_client_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim().trim_matches('"').trim();
    if value.is_empty() {
        return None;
    }

    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip);
    }

    if let Ok(addr) = value.parse::<SocketAddr>() {
        return Some(addr.ip());
    }

    let value = value.trim_matches(['[', ']']);
    value.parse::<IpAddr>().ok()
}

fn forwarded_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    if let Some(xff) = headers.get("X-Forwarded-For").and_then(|v| v.to_str().ok()) {
        for candidate in xff.split(',') {
            if let Some(ip) = parse_client_ip(candidate) {
                return Some(ip);
            }
        }
    }

    headers
        .get("X-Real-IP")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_client_ip)
}

fn client_key_from_request(
    peer_addr: Option<SocketAddr>,
    headers: &HeaderMap,
    trust_forwarded_headers: bool,
) -> String {
    if trust_forwarded_headers {
        if let Some(ip) = forwarded_client_ip(headers) {
            return ip.to_string();
        }
    }

    peer_addr
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn normalize_max_keys(configured: usize, fallback: usize) -> usize {
    if configured == 0 {
        fallback.max(1)
    } else {
        configured
    }
}

/// An in-memory chat session (agent + metadata).
pub struct ChatSession {
    pub agent: crate::agent::Agent,
    pub created_at: std::time::Instant,
    pub last_active: std::time::Instant,
    pub label: Option<String>,
    pub message_count: u32,
    pub current_goal: Option<String>,
    pub session_summary: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub run_id: Option<String>,
    /// Abort signal: send `true` to cancel the active run.
    pub abort_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Message count at last summary generation — for robust interval triggering.
    pub last_summary_count: u32,
}

/// Shared state for all axum handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub provider: Arc<dyn Provider>,
    pub model: String,
    /// Model for session summarization (falls back to `model` if None).
    pub summary_model: Option<String>,
    pub temperature: f64,
    pub mem: Arc<dyn UnifiedMemoryPort>,
    pub auto_save: bool,
    /// SHA-256 hash of `X-Webhook-Secret` (hex-encoded), never plaintext.
    pub webhook_secret_hash: Option<Arc<str>>,
    pub pairing: Arc<PairingGuard>,
    pub trust_forwarded_headers: bool,
    pub rate_limiter: Arc<GatewayRateLimiter>,
    pub idempotency_store: Arc<IdempotencyStore>,
    pub whatsapp: Option<Arc<WhatsAppChannel>>,
    /// `WhatsApp` app secret for webhook signature verification (`X-Hub-Signature-256`)
    pub whatsapp_app_secret: Option<Arc<str>>,
    pub linq: Option<Arc<LinqChannel>>,
    /// Linq webhook signing secret for signature verification
    pub linq_signing_secret: Option<Arc<str>>,
    pub nextcloud_talk: Option<Arc<NextcloudTalkChannel>>,
    /// Nextcloud Talk webhook secret for signature verification
    pub nextcloud_talk_webhook_secret: Option<Arc<str>>,
    pub wati: Option<Arc<WatiChannel>>,
    /// Observability backend for metrics scraping
    pub observer: Arc<dyn synapse_observability::Observer>,
    /// Registered tool specs (for web dashboard tools page)
    pub tools_registry: Arc<Vec<ToolSpec>>,
    /// Cost tracker (optional, for web dashboard cost page)
    pub cost_tracker: Option<Arc<CostTracker>>,
    /// SSE broadcast channel for real-time events
    pub event_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// Shutdown signal sender for graceful shutdown
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Audit logger for persistent security event logging
    pub audit_logger: Option<Arc<synapse_security::AuditLogger>>,
    /// PromptGuard for IPC payload injection scanning
    pub ipc_prompt_guard: Option<synapse_security::PromptGuard>,
    /// LeakDetector for IPC credential leak scanning
    pub ipc_leak_detector: Option<synapse_security::LeakDetector>,
    /// IPC broker database (None when agents_ipc.enabled = false)
    pub ipc_db: Option<Arc<ipc::IpcDb>>,
    /// IPC per-agent send rate limiter (None when IPC is disabled)
    pub ipc_rate_limiter: Option<Arc<SlidingWindowRateLimiter>>,
    /// IPC per-agent read rate limiter (None when IPC is disabled)
    pub ipc_read_rate_limiter: Option<Arc<SlidingWindowRateLimiter>>,
    /// Registry of dynamically connected nodes
    pub node_registry: Arc<nodes::NodeRegistry>,
    /// Registry of agent daemons for broker proxy (Phase 3.8)
    pub agent_registry: Arc<agent_registry::AgentRegistry>,
    /// Port for invoking agent runs without depending on `crate::agent` directly.
    pub agent_runner: Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
    /// Runtime provisioning state (Phase 3.8 Step 11)
    pub provisioning_state: Arc<provisioning::ProvisioningState>,
    /// In-memory chat sessions keyed by session key (e.g. `web:<hash>:<id>`)
    pub chat_sessions: Arc<std::sync::Mutex<HashMap<String, ChatSession>>>,
    /// Persistent chat database (SQLite)
    pub chat_db: Option<Arc<chat_db::ChatDb>>,
    /// Parsed admin CIDR allowlist for non-localhost admin access
    pub admin_cidrs: Arc<Vec<AdminCidr>>,
    /// IPC push dispatcher for broker→agent push notifications
    pub ipc_push_dispatcher: Option<Arc<ipc::PushDispatcher>>,
    /// Dedup set for received push notifications (agent-side)
    pub ipc_push_dedup: Option<Arc<ipc::PushDedupSet>>,
    /// Signal channel for push notifications → inbox processor (agent-side)
    pub ipc_push_signal: Option<tokio::sync::mpsc::UnboundedSender<ipc::PushMeta>>,
    /// Channel session backend for JSONL/SQLite channel conversation persistence
    pub channel_session_backend: Option<Arc<dyn crate::channels::session_backend::SessionBackend>>,
    /// Phase 4.0: Channel adapter registry with long-lived cached instances
    pub channel_registry:
        Option<Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>>,
    /// Phase 4.0: Unified conversation/session store
    pub conversation_store:
        Option<Arc<dyn synapse_domain::ports::conversation_store::ConversationStorePort>>,
    /// Phase 4.0: Unified run execution store
    pub run_store: Option<Arc<dyn synapse_domain::ports::run_store::RunStorePort>>,
    /// Phase 4.1: Pipeline definition store (TOML loader)
    pub pipeline_store: Option<Arc<dyn synapse_domain::ports::pipeline_store::PipelineStorePort>>,
    /// Phase 4.1: Pipeline step executor (IPC bridge)
    pub pipeline_executor:
        Option<Arc<dyn synapse_domain::ports::pipeline_executor::PipelineExecutorPort>>,
    /// Phase 4.1: Message router for deterministic inbound routing
    pub message_router: Option<Arc<dyn synapse_domain::ports::message_router::MessageRouterPort>>,
    /// Phase 4.1: Tool middleware chain
    pub tool_middleware: Option<
        Arc<synapse_domain::application::services::tool_middleware_service::ToolMiddlewareChain>,
    >,
}

/// Run the HTTP gateway using axum with proper HTTP/1.1 compliance.
#[allow(clippy::too_many_lines)]
pub async fn run_gateway(
    host: &str,
    port: u16,
    config: Config,
    outbound_tx: Option<synapse_domain::bus::OutboundIntentSender>,
    channel_registry: Option<Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>>,
    shared_ipc_client: Option<Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>>,
    agent_runner: Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
) -> Result<()> {
    // ── Security: refuse public bind without tunnel or explicit opt-in ──
    if is_public_bind(host) && config.tunnel.provider == "none" && !config.gateway.allow_public_bind
    {
        anyhow::bail!(
            "🛑 Refusing to bind to {host} — gateway would be exposed to the internet.\n\
             Fix: use --host 127.0.0.1 (default), configure a tunnel, or set\n\
             [gateway] allow_public_bind = true in config.toml (NOT recommended)."
        );
    }
    let config_state = Arc::new(Mutex::new(config.clone()));

    // ── Hooks ──────────────────────────────────────────────────────
    let hooks: Option<std::sync::Arc<crate::hooks::HookRunner>> = if config.hooks.enabled {
        Some(std::sync::Arc::new(crate::hooks::HookRunner::new()))
    } else {
        None
    };

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_port = listener.local_addr()?.port();
    let display_addr = format!("{host}:{actual_port}");

    let provider: Arc<dyn Provider> =
        Arc::from(synapse_providers::create_resilient_provider_with_options(
            config.default_provider.as_deref().unwrap_or("openrouter"),
            config.api_key.as_deref(),
            config.api_url.as_deref(),
            &config.reliability,
            &synapse_providers::ProviderRuntimeOptions {
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
            },
        )?);
    let model = config
        .default_model
        .clone()
        .unwrap_or_else(|| "anthropic/claude-sonnet-4".into());
    let summary_model = config.summary_model.clone();
    let temperature = config.default_temperature;
    let mem: Arc<dyn UnifiedMemoryPort> = synapse_memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        "default",
        config.api_key.as_deref(),
    )
    .await?;
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(security_policy_from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };

    let (tools_registry_raw, _delegate_handle_gw, _) = tools::all_tools_with_runtime(
        Arc::new(config.clone()),
        &security,
        runtime,
        Arc::clone(&mem),
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.web_fetch,
        &config.workspace_dir,
        &config.agents,
        config.api_key.as_deref(),
        &config,
        None, // IPC tools get their own client from config
        Some(agent_runner.clone()),
    );
    let tools_registry: Arc<Vec<ToolSpec>> =
        Arc::new(tools_registry_raw.iter().map(|t| t.spec()).collect());

    // Cost tracker (optional)
    let cost_tracker = if config.cost.enabled {
        match CostTracker::new(config.cost.clone(), &config.workspace_dir) {
            Ok(ct) => Some(Arc::new(ct)),
            Err(e) => {
                tracing::warn!("Failed to initialize cost tracker: {e}");
                None
            }
        }
    } else {
        None
    };

    // SSE broadcast channel for real-time events
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel::<serde_json::Value>(256);
    // Extract webhook secret for authentication
    let webhook_secret_hash: Option<Arc<str>> =
        config.channels_config.webhook.as_ref().and_then(|webhook| {
            webhook.secret.as_ref().and_then(|raw_secret| {
                let trimmed_secret = raw_secret.trim();
                (!trimmed_secret.is_empty())
                    .then(|| Arc::<str>::from(hash_webhook_secret(trimmed_secret)))
            })
        });

    // WhatsApp channel (if configured)
    let whatsapp_channel: Option<Arc<WhatsAppChannel>> = config
        .channels_config
        .whatsapp
        .as_ref()
        .filter(|wa| wa.is_cloud_config())
        .map(|wa| {
            Arc::new(WhatsAppChannel::new(
                wa.access_token.clone().unwrap_or_default(),
                wa.phone_number_id.clone().unwrap_or_default(),
                wa.verify_token.clone().unwrap_or_default(),
                wa.allowed_numbers.clone(),
            ))
        });

    // WhatsApp app secret for webhook signature verification
    // Priority: environment variable > config file
    let whatsapp_app_secret: Option<Arc<str>> = std::env::var("SYNAPSECLAW_WHATSAPP_APP_SECRET")
        .ok()
        .and_then(|secret| {
            let secret = secret.trim();
            (!secret.is_empty()).then(|| secret.to_owned())
        })
        .or_else(|| {
            config.channels_config.whatsapp.as_ref().and_then(|wa| {
                wa.app_secret
                    .as_deref()
                    .map(str::trim)
                    .filter(|secret| !secret.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
        .map(Arc::from);

    // Linq channel (if configured)
    let linq_channel: Option<Arc<LinqChannel>> = config.channels_config.linq.as_ref().map(|lq| {
        Arc::new(LinqChannel::new(
            lq.api_token.clone(),
            lq.from_phone.clone(),
            lq.allowed_senders.clone(),
        ))
    });

    // Linq signing secret for webhook signature verification
    // Priority: environment variable > config file
    let linq_signing_secret: Option<Arc<str>> = std::env::var("SYNAPSECLAW_LINQ_SIGNING_SECRET")
        .ok()
        .and_then(|secret| {
            let secret = secret.trim();
            (!secret.is_empty()).then(|| secret.to_owned())
        })
        .or_else(|| {
            config.channels_config.linq.as_ref().and_then(|lq| {
                lq.signing_secret
                    .as_deref()
                    .map(str::trim)
                    .filter(|secret| !secret.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
        .map(Arc::from);

    // WATI channel (if configured)
    let wati_channel: Option<Arc<WatiChannel>> =
        config.channels_config.wati.as_ref().map(|wati_cfg| {
            Arc::new(WatiChannel::new(
                wati_cfg.api_token.clone(),
                wati_cfg.api_url.clone(),
                wati_cfg.tenant_id.clone(),
                wati_cfg.allowed_numbers.clone(),
            ))
        });

    // Nextcloud Talk channel (if configured)
    let nextcloud_talk_channel: Option<Arc<NextcloudTalkChannel>> =
        config.channels_config.nextcloud_talk.as_ref().map(|nc| {
            Arc::new(NextcloudTalkChannel::new(
                nc.base_url.clone(),
                nc.app_token.clone(),
                nc.allowed_users.clone(),
            ))
        });

    // Nextcloud Talk webhook secret for signature verification
    // Priority: environment variable > config file
    let nextcloud_talk_webhook_secret: Option<Arc<str>> =
        std::env::var("SYNAPSECLAW_NEXTCLOUD_TALK_WEBHOOK_SECRET")
            .ok()
            .and_then(|secret| {
                let secret = secret.trim();
                (!secret.is_empty()).then(|| secret.to_owned())
            })
            .or_else(|| {
                config
                    .channels_config
                    .nextcloud_talk
                    .as_ref()
                    .and_then(|nc| {
                        nc.webhook_secret
                            .as_deref()
                            .map(str::trim)
                            .filter(|secret| !secret.is_empty())
                            .map(ToOwned::to_owned)
                    })
            })
            .map(Arc::from);

    // ── Pairing guard ──────────────────────────────────────
    let pairing = Arc::new(PairingGuard::with_metadata(
        config.gateway.require_pairing,
        &config.gateway.paired_tokens,
        &config.gateway.token_metadata,
    ));
    let rate_limit_max_keys = normalize_max_keys(
        config.gateway.rate_limit_max_keys,
        RATE_LIMIT_MAX_KEYS_DEFAULT,
    );
    let rate_limiter = Arc::new(GatewayRateLimiter::new(
        config.gateway.pair_rate_limit_per_minute,
        config.gateway.webhook_rate_limit_per_minute,
        rate_limit_max_keys,
    ));
    let idempotency_max_keys = normalize_max_keys(
        config.gateway.idempotency_max_keys,
        IDEMPOTENCY_MAX_KEYS_DEFAULT,
    );
    let idempotency_store = Arc::new(IdempotencyStore::new(
        Duration::from_secs(config.gateway.idempotency_ttl_secs.max(1)),
        idempotency_max_keys,
    ));

    // ── Tunnel ────────────────────────────────────────────────
    let tunnel = crate::tunnel::create_tunnel(&config.tunnel)?;
    let mut tunnel_url: Option<String> = None;

    if let Some(ref tun) = tunnel {
        println!("🔗 Starting {} tunnel...", tun.name());
        match tun.start(host, actual_port).await {
            Ok(url) => {
                println!("🌐 Tunnel active: {url}");
                tunnel_url = Some(url);
            }
            Err(e) => {
                println!("⚠️  Tunnel failed to start: {e}");
                println!("   Falling back to local-only mode.");
            }
        }
    }

    println!("🦀 SynapseClaw Gateway listening on http://{display_addr}");
    if let Some(ref url) = tunnel_url {
        println!("  🌐 Public URL: {url}");
    }
    println!("  🌐 Web Dashboard: http://{display_addr}/");
    println!("  POST /pair      — pair a new client (X-Pairing-Code header)");
    println!("  POST /webhook   — {{\"message\": \"your prompt\"}}");
    if whatsapp_channel.is_some() {
        println!("  GET  /whatsapp  — Meta webhook verification");
        println!("  POST /whatsapp  — WhatsApp message webhook");
    }
    if linq_channel.is_some() {
        println!("  POST /linq      — Linq message webhook (iMessage/RCS/SMS)");
    }
    if wati_channel.is_some() {
        println!("  GET  /wati      — WATI webhook verification");
        println!("  POST /wati      — WATI message webhook");
    }
    if nextcloud_talk_channel.is_some() {
        println!("  POST /nextcloud-talk — Nextcloud Talk bot webhook");
    }
    println!("  GET  /api/*     — REST API (bearer token required)");
    println!("  GET  /ws/chat   — WebSocket agent chat");
    if config.nodes.enabled {
        println!("  GET  /ws/nodes  — WebSocket node discovery");
    }
    println!("  GET  /health    — health check");
    println!("  GET  /metrics   — Prometheus metrics");
    if let Some(code) = pairing.pairing_code() {
        println!();
        println!("  🔐 PAIRING REQUIRED — use this one-time code:");
        println!("     ┌──────────────┐");
        println!("     │  {code}  │");
        println!("     └──────────────┘");
        println!("     Send: POST /pair with header X-Pairing-Code: {code}");
    } else if pairing.require_pairing() {
        println!("  🔒 Pairing: ACTIVE (bearer token required)");
        println!("     To pair a new device: synapseclaw gateway get-paircode --new");
    } else {
        println!("  ⚠️  Pairing: DISABLED (all requests accepted)");
    }
    println!("  Press Ctrl+C to stop.\n");

    crate::health::mark_component_ok("gateway");

    // Fire gateway start hook
    if let Some(ref hooks) = hooks {
        hooks.fire_gateway_start(host, actual_port).await;
    }

    // Wrap observer with broadcast capability for SSE
    let broadcast_observer: Arc<dyn synapse_observability::Observer> =
        Arc::new(sse::BroadcastObserver::new(
            synapse_observability::create_observer(&config.observability),
            event_tx.clone(),
        ));

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    // Node registry for dynamic node discovery
    let node_registry = Arc::new(nodes::NodeRegistry::new(config.nodes.max_nodes));

    // Audit logger for persistent security event logging
    let audit_logger = if config.security.audit.enabled {
        match synapse_security::AuditLogger::new(
            config.security.audit.clone(),
            config.workspace_dir.clone(),
        ) {
            Ok(logger) => Some(Arc::new(logger)),
            Err(e) => {
                tracing::warn!("Failed to initialize audit logger: {e}");
                None
            }
        }
    } else {
        None
    };

    // PromptGuard for IPC payload scanning
    let ipc_prompt_guard = if config.agents_ipc.enabled && config.agents_ipc.prompt_guard.enabled {
        let action =
            synapse_security::GuardAction::from_str(&config.agents_ipc.prompt_guard.action);
        Some(synapse_security::PromptGuard::with_config(
            action,
            config.agents_ipc.prompt_guard.sensitivity,
        ))
    } else {
        None
    };

    // LeakDetector for IPC credential leak scanning
    let ipc_leak_detector = if config.agents_ipc.enabled {
        Some(synapse_security::LeakDetector::with_sensitivity(0.7))
    } else {
        None
    };

    // ── Chat DB (SQLite persistence for web chat sessions) ──
    let chat_db = {
        let chat_dir = config.workspace_dir.join("chat");
        match chat_db::ChatDb::open(&chat_dir.join("sessions.db")) {
            Ok(db) => Some(Arc::new(db)),
            Err(e) => {
                tracing::warn!("Failed to open chat database: {e}");
                None
            }
        }
    };

    // ── Phase 4.0: ConversationStorePort (wraps ChatDb) ──
    let conversation_store: Option<
        Arc<dyn synapse_domain::ports::conversation_store::ConversationStorePort>,
    > = chat_db.as_ref().map(|db| {
        Arc::new(crate::storage::conversation_store::ChatDbConversationStore::new(Arc::clone(db)))
            as Arc<dyn synapse_domain::ports::conversation_store::ConversationStorePort>
    });

    // ── Phase 4.0: RunStorePort (wraps ChatDb) ──
    let run_store: Option<Arc<dyn synapse_domain::ports::run_store::RunStorePort>> =
        chat_db.as_ref().map(|db| {
            Arc::new(crate::storage::run_store::ChatDbRunStore::new(Arc::clone(
                db,
            ))) as Arc<dyn synapse_domain::ports::run_store::RunStorePort>
        });

    // Parse admin CIDR allowlist — fail at boot on invalid entries
    let admin_cidrs: Vec<AdminCidr> = config
        .gateway
        .admin_cidrs
        .iter()
        .map(|s| AdminCidr::parse(s).with_context(|| format!("invalid admin_cidrs entry: {s}")))
        .collect::<Result<_>>()?;
    if !admin_cidrs.is_empty() {
        tracing::info!(
            count = admin_cidrs.len(),
            cidrs = ?config.gateway.admin_cidrs,
            "Admin CIDR allowlist configured"
        );
    }

    let mut state = AppState {
        config: config_state,
        provider: Arc::clone(&provider),
        model: model.clone(),
        summary_model: summary_model.clone(),
        temperature,
        mem: Arc::new(
            crate::memory_adapters::memory_adapter::ConsolidatingMemory::new(
                mem,
                Arc::clone(&provider),
                model.clone(),
            ),
        ),
        auto_save: config.memory.auto_save,
        webhook_secret_hash,
        pairing,
        trust_forwarded_headers: config.gateway.trust_forwarded_headers,
        rate_limiter,
        idempotency_store,
        whatsapp: whatsapp_channel,
        whatsapp_app_secret,
        linq: linq_channel,
        linq_signing_secret,
        nextcloud_talk: nextcloud_talk_channel,
        nextcloud_talk_webhook_secret,
        wati: wati_channel,
        observer: broadcast_observer,
        tools_registry,
        cost_tracker,
        event_tx,
        shutdown_tx,
        audit_logger,
        ipc_prompt_guard,
        ipc_leak_detector,
        ipc_db: if config.agents_ipc.enabled {
            let ipc_dir = config.workspace_dir.join("ipc");
            if let Err(e) = std::fs::create_dir_all(&ipc_dir) {
                tracing::warn!("Failed to create IPC directory: {e}");
            }
            match ipc::IpcDb::open(&ipc_dir.join("agents.db")) {
                Ok(db) => {
                    let db = Arc::new(db);
                    // Broker restart recovery: interrupt orphaned ephemeral sessions
                    let interrupted = db.interrupt_all_ephemeral_spawn_runs();
                    if interrupted > 0 {
                        tracing::info!(
                            interrupted = interrupted,
                            "IPC DB: interrupted orphaned ephemeral spawn runs on startup"
                        );
                    }
                    Some(db)
                }
                Err(e) => {
                    tracing::error!("Failed to open IPC database: {e}");
                    None
                }
            }
        } else {
            None
        },
        ipc_rate_limiter: if config.agents_ipc.enabled {
            Some(Arc::new(SlidingWindowRateLimiter::new(
                config.agents_ipc.max_messages_per_hour,
                Duration::from_secs(3600),
                256,
            )))
        } else {
            None
        },
        ipc_read_rate_limiter: if config.agents_ipc.enabled {
            Some(Arc::new(SlidingWindowRateLimiter::new(
                config.agents_ipc.max_messages_per_hour * 5, // reads are cheaper
                Duration::from_secs(3600),
                256,
            )))
        } else {
            None
        },
        node_registry,
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner,
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(admin_cidrs),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db,
        ipc_push_dispatcher: None, // initialized below after state is built
        ipc_push_dedup: if config.agents_ipc.enabled && config.agents_ipc.push_enabled {
            Some(Arc::new(ipc::PushDedupSet::new(1000)))
        } else {
            None
        },
        ipc_push_signal: None, // initialized below for agent-side inbox processor
        channel_session_backend: if config.channels_config.session_persistence {
            match crate::channels::session_store::SessionStore::new(&config.workspace_dir) {
                Ok(store) => Some(Arc::new(store)),
                Err(e) => {
                    tracing::warn!("Channel session backend disabled: {e}");
                    None
                }
            }
        } else {
            None
        },
        channel_registry,
        conversation_store,
        run_store: run_store.clone(),
        pipeline_store: None,    // initialized below if pipelines enabled
        pipeline_executor: None, // initialized below if pipelines enabled
        message_router: None,    // initialized below if pipelines enabled
        tool_middleware: None,   // initialized below if pipelines enabled
    };

    // Phase 4.1: Initialize pipeline engine if enabled
    if config.pipelines.enabled {
        let pipeline_dir = config
            .pipelines
            .directory
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| config.workspace_dir.join("pipelines"));

        // Ensure pipeline directory exists
        if let Err(e) = std::fs::create_dir_all(&pipeline_dir) {
            tracing::warn!(dir = %pipeline_dir.display(), error = %e, "cannot create pipeline directory");
        }

        // Load pipeline TOML definitions
        let pipeline_store: Arc<dyn synapse_domain::ports::pipeline_store::PipelineStorePort> =
            Arc::new(crate::pipeline::toml_loader::TomlPipelineLoader::new(
                &pipeline_dir,
            ));
        if let Err(e) = pipeline_store.reload().await {
            tracing::error!(error = %e, "failed to load pipeline definitions");
        } else {
            let names = pipeline_store.list().await;
            tracing::info!(pipelines = ?names, "pipeline definitions loaded");
        }

        // Create IPC step executor for pipeline dispatch.
        // If a shared IpcClient was provided (daemon mode), reuse it to avoid
        // duplicate AtomicI64 seq counters → replay_rejected.
        // In standalone gateway mode, create a local IpcClient.
        let pipeline_executor: Option<
            Arc<dyn synapse_domain::ports::pipeline_executor::PipelineExecutorPort>,
        > = if config.agents_ipc.enabled {
            if let Some(ref broker_token) = config.agents_ipc.broker_token {
                let ipc_client = if let Some(ref shared) = shared_ipc_client {
                    // Daemon mode: reuse the shared IpcClient (single seq counter).
                    // Still sync sender_seq from broker DB in case DB advanced.
                    if let Some(ref db) = state.ipc_db {
                        let runner_id = config
                            .pipelines
                            .runner_agent_id
                            .clone()
                            .or_else(|| config.agents_ipc.agent_id.clone())
                            .unwrap_or_else(|| config.agents_ipc.role.clone());
                        let db_seq = db.get_last_sender_seq(&runner_id);
                        shared.sync_sender_seq(db_seq);
                    }
                    Arc::clone(shared)
                } else {
                    // Standalone gateway mode: create a local IpcClient.
                    let broker_url = format!("http://{}:{}", host, port);
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

                    let mut client = crate::tools::agents_ipc::IpcClient::new(
                        &broker_url,
                        broker_token,
                        config.agents_ipc.request_timeout_secs,
                    );
                    if let Ok(identity) =
                        synapse_security::identity::AgentIdentity::load_or_generate(&key_path)
                    {
                        client = client.with_identity(identity, runner_id.clone());
                        tracing::info!(
                            agent_id = %runner_id,
                            "pipeline executor: IpcClient with Ed25519 identity"
                        );
                    }
                    if let Some(ref db) = state.ipc_db {
                        let db_seq = db.get_last_sender_seq(&runner_id);
                        client.sync_sender_seq(db_seq);
                    }

                    let client: Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort> =
                        Arc::new(client);

                    // Register public key with broker
                    {
                        let c = Arc::clone(&client);
                        tokio::spawn(async move {
                            if let Err(e) = c.register_public_key().await {
                                tracing::warn!("pipeline executor: key registration failed: {e}");
                            }
                        });
                    }
                    client
                };

                Some(
                    Arc::new(crate::pipeline::ipc_step_executor::IpcStepExecutor::new(
                        ipc_client,
                    ))
                        as Arc<dyn synapse_domain::ports::pipeline_executor::PipelineExecutorPort>,
                )
            } else {
                tracing::warn!("Pipeline engine disabled: agents_ipc.broker_token not configured");
                None
            }
        } else {
            None
        };

        // Load routing table
        let routing_path = config
            .pipelines
            .routing_file
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| pipeline_dir.join("routing.toml"));
        let routing_fallback = config
            .pipelines
            .routing_fallback
            .clone()
            .unwrap_or_else(|| "default".into());
        let message_router: Arc<dyn synapse_domain::ports::message_router::MessageRouterPort> =
            Arc::new(crate::routing::rule_chain::TomlMessageRouter::load(
                &routing_path,
                &routing_fallback,
            ));

        // Build tool middleware chain
        let mut middleware_chain =
            synapse_domain::application::services::tool_middleware_service::ToolMiddlewareChain::new(
            );

        if config.pipelines.default_tool_rate_limit > 0 {
            middleware_chain.push(Box::new(
                crate::middleware::rate_limit::RateLimitMiddleware::with_default_limit(
                    config.pipelines.default_tool_rate_limit,
                ),
            ));
        }

        if !config.pipelines.approval_required_tools.is_empty() {
            let tools: std::collections::HashSet<String> = config
                .pipelines
                .approval_required_tools
                .iter()
                .cloned()
                .collect();
            middleware_chain.push(Box::new(
                crate::middleware::approval_gate::ApprovalGateMiddleware::new(tools),
            ));
        }

        state.pipeline_store = Some(pipeline_store.clone());
        state.pipeline_executor = pipeline_executor;
        state.message_router = Some(message_router);
        if !middleware_chain.is_empty() {
            state.tool_middleware = Some(Arc::new(middleware_chain));
        }

        // Start hot-reload watcher
        if config.pipelines.hot_reload {
            match crate::pipeline::hot_reload::start_watcher(pipeline_dir, pipeline_store) {
                Ok(_handle) => {
                    // Handle is stored implicitly — watcher runs until daemon exits.
                    // TODO: store handle for graceful shutdown
                    tracing::info!("pipeline hot-reload watcher started");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to start pipeline hot-reload watcher");
                }
            }
        }

        // Pipeline recovery: resume incomplete runs
        if let (Some(ref ps), Some(ref pe), Some(ref rs)) = (
            &state.pipeline_store,
            &state.pipeline_executor,
            &state.run_store,
        ) {
            let ports =
                synapse_domain::application::services::pipeline_service::PipelineRunnerPorts {
                    pipeline_store: ps.clone(),
                    run_store: rs.clone(),
                    executor: pe.clone(),
                };
            let report =
                synapse_domain::application::use_cases::resume_pipeline::recover_all(&ports).await;
            if report.found > 0 {
                tracing::info!(
                    found = report.found,
                    resumed = report.resumed,
                    failed = report.failed,
                    skipped = report.skipped,
                    "pipeline recovery complete"
                );
            }
        }
    }

    // Phase 3.8: seed AgentRegistry from DB + start health polling
    if config.agents_ipc.enabled {
        if let Some(ref db) = state.ipc_db {
            if let Ok(gateways) = db.list_agent_gateways() {
                // Also fetch trust/role from IPC agents table
                let ipc_agents = db.list_agents(config.agents_ipc.staleness_secs);
                for gw in &gateways {
                    state
                        .agent_registry
                        .upsert(&gw.agent_id, &gw.gateway_url, &gw.proxy_token);
                    // Restore trust_level/role from IPC agents table
                    if let Some(ipc_agent) = ipc_agents.iter().find(|a| a.agent_id == gw.agent_id) {
                        if let (Some(tl), Some(role)) =
                            (ipc_agent.trust_level, ipc_agent.role.as_deref())
                        {
                            state.agent_registry.set_trust_info(&gw.agent_id, tl, role);
                        }
                    }
                }
                if !gateways.is_empty() {
                    tracing::info!(
                        "Seeded AgentRegistry with {} gateways from DB (trust/role restored)",
                        gateways.len()
                    );
                }
            }

            // Spawn push dispatcher (broker-side)
            if config.agents_ipc.push_enabled {
                let dispatcher = ipc::PushDispatcher::spawn(
                    Arc::clone(db),
                    Arc::clone(&state.agent_registry),
                    config.agents_ipc.push_max_retries,
                );
                state.ipc_push_dispatcher = Some(Arc::new(dispatcher));
                tracing::info!("IPC push dispatcher started");
            }
        }
        // Start background health polling
        let poll_state = state.clone();
        tokio::spawn(async move {
            agent_health_poll_loop(poll_state).await;
        });
    }

    // Agent-side: spawn inbox processor if this agent has IPC push support
    if config.agents_ipc.enabled
        && config.agents_ipc.push_enabled
        && config.agents_ipc.proxy_token.is_some()
    {
        let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel::<ipc::PushMeta>();
        state.ipc_push_signal = Some(push_tx);
        let inbox_config = config.clone();
        let inbox_outbound_tx = outbound_tx.clone();
        let inbox_run_store = state.run_store.clone();
        let inbox_agent_runner = state.agent_runner.clone();
        tokio::spawn(async move {
            Box::pin(agent_inbox_processor(
                inbox_config,
                push_rx,
                inbox_outbound_tx,
                inbox_run_store,
                inbox_agent_runner,
            ))
            .await;
        });
    }

    // Config PUT needs larger body limit (1MB)
    let config_put_router = Router::new()
        .route("/api/config", put(api::handle_api_config_put))
        .layer(RequestBodyLimitLayer::new(1_048_576));

    // Build router with middleware
    let app = Router::new()
        // ── Admin routes (for CLI management) ──
        .route("/admin/shutdown", post(handle_admin_shutdown))
        .route("/admin/paircode", get(handle_admin_paircode))
        .route("/admin/paircode/new", post(handle_admin_paircode_new))
        // ── Existing routes ──
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
        .route("/pair", post(handle_pair))
        .route("/webhook", post(handle_webhook))
        .route("/whatsapp", get(handle_whatsapp_verify))
        .route("/whatsapp", post(handle_whatsapp_message))
        .route("/linq", post(handle_linq_webhook))
        .route("/wati", get(handle_wati_verify))
        .route("/wati", post(handle_wati_webhook))
        .route("/nextcloud-talk", post(handle_nextcloud_talk_webhook))
        // ── IPC routes (broker-mediated inter-agent communication) ──
        .route("/api/ipc/agents", get(ipc::handle_ipc_agents))
        .route("/api/ipc/send", post(ipc::handle_ipc_send))
        .route("/api/ipc/inbox", get(ipc::handle_ipc_inbox))
        .route("/api/ipc/ack", post(ipc::handle_ipc_ack))
        .route("/api/ipc/state", get(ipc::handle_ipc_state_get))
        .route("/api/ipc/state", post(ipc::handle_ipc_state_set))
        // ── Pipeline routes (Phase 4.1) ──
        .route("/api/pipelines/start", post(ipc::handle_pipeline_start))
        .route("/api/pipelines/list", get(ipc::handle_pipeline_list))
        .route(
            "/api/ipc/provision-ephemeral",
            post(ipc::handle_ipc_provision_ephemeral),
        )
        .route("/api/ipc/spawn-status", get(ipc::handle_ipc_spawn_status))
        .route("/api/ipc/register-key", post(ipc::handle_ipc_register_key))
        .route(
            "/api/ipc/register-gateway",
            post(ipc::handle_ipc_register_gateway),
        )
        .route("/api/ipc/push", post(ipc::handle_ipc_push_notification))
        // ── IPC admin routes (localhost only) ──
        .route("/admin/ipc/agents", get(ipc::handle_admin_ipc_agents))
        .route("/admin/ipc/revoke", post(ipc::handle_admin_ipc_revoke))
        .route("/admin/ipc/disable", post(ipc::handle_admin_ipc_disable))
        .route(
            "/admin/ipc/quarantine",
            post(ipc::handle_admin_ipc_quarantine),
        )
        .route(
            "/admin/ipc/downgrade",
            post(ipc::handle_admin_ipc_downgrade),
        )
        .route("/admin/ipc/promote", post(ipc::handle_admin_ipc_promote))
        // ── IPC admin read endpoints (Phase 3.5) ──
        .route(
            "/admin/ipc/agents/{id}/detail",
            get(ipc::handle_admin_ipc_agent_detail),
        )
        .route("/admin/ipc/messages", get(ipc::handle_admin_ipc_messages))
        .route(
            "/admin/ipc/spawn-runs",
            get(ipc::handle_admin_ipc_spawn_runs),
        )
        .route("/admin/ipc/audit", get(ipc::handle_admin_ipc_audit))
        .route(
            "/admin/ipc/audit/verify",
            post(ipc::handle_admin_ipc_audit_verify),
        )
        .route(
            "/admin/ipc/dismiss-message",
            post(ipc::handle_admin_ipc_dismiss_message),
        )
        .route("/admin/activity", get(ipc::handle_admin_activity))
        // ── Provisioning admin routes (localhost + admin auth) ──
        .route(
            "/admin/provisioning/arm",
            post(provisioning::handle_provisioning_arm),
        )
        .route(
            "/admin/provisioning/status",
            get(provisioning::handle_provisioning_status),
        )
        .route(
            "/admin/provisioning/create",
            post(provisioning::handle_provisioning_create),
        )
        .route(
            "/admin/provisioning/install",
            post(provisioning::handle_provisioning_install),
        )
        .route(
            "/admin/provisioning/start",
            post(provisioning::handle_provisioning_start),
        )
        .route(
            "/admin/provisioning/stop",
            post(provisioning::handle_provisioning_stop),
        )
        .route(
            "/admin/provisioning/uninstall",
            post(provisioning::handle_provisioning_uninstall),
        )
        .route(
            "/admin/provisioning/patch-broker",
            post(provisioning::handle_provisioning_patch_broker),
        )
        .route(
            "/admin/provisioning/used-ports",
            get(provisioning::handle_provisioning_used_ports),
        )
        .route(
            "/admin/provisioning/topology",
            get(provisioning::handle_provisioning_topology),
        )
        // ── Web Dashboard API routes ──
        .route("/api/agents", get(api::handle_api_agents))
        .route(
            "/api/agents/{agent_id}/status",
            get(api::handle_api_agent_status_proxy),
        )
        .route(
            "/api/agents/{agent_id}/summary-model",
            put(api::handle_api_agent_summary_model_proxy),
        )
        .route(
            "/api/agents/{agent_id}/cron",
            get(api::handle_api_agent_cron_list_proxy),
        )
        .route(
            "/api/agents/{agent_id}/cron",
            post(api::handle_api_agent_cron_add_proxy),
        )
        .route(
            "/api/agents/{agent_id}/cron/{job_id}",
            delete(api::handle_api_agent_cron_delete_proxy),
        )
        .route(
            "/api/agents/{agent_id}/cron/{job_id}/runs",
            get(api::handle_api_agent_cron_runs_proxy),
        )
        .route(
            "/api/agents/{agent_id}/chat/sessions",
            get(api::handle_api_agent_chat_sessions_proxy),
        )
        .route(
            "/api/agents/{agent_id}/chat/sessions/{key}/messages",
            get(api::handle_api_agent_chat_messages_proxy),
        )
        .route("/api/status", get(api::handle_api_status))
        .route("/api/summary-model", put(api::handle_api_summary_model_put))
        .route("/api/config", get(api::handle_api_config_get))
        .route("/api/tools", get(api::handle_api_tools))
        .route("/api/activity", get(api::handle_api_activity))
        .route("/api/chat/sessions", get(api::handle_api_chat_sessions))
        .route(
            "/api/chat/sessions/{key}/messages",
            get(api::handle_api_chat_session_messages),
        )
        .route(
            "/api/channel/sessions",
            get(api::handle_api_channel_sessions),
        )
        .route(
            "/api/channel/sessions/{key}/messages",
            get(api::handle_api_channel_session_messages),
        )
        .route(
            "/api/channel/sessions/{key}",
            delete(api::handle_api_channel_session_delete),
        )
        // ── Phase 4.0: Conversation REST API ──
        .route(
            "/api/conversations",
            get(api::handle_api_conversations_list),
        )
        .route(
            "/api/conversations/{key}",
            get(api::handle_api_conversations_get).delete(api::handle_api_conversations_delete),
        )
        // ── Phase 4.0: Runs REST API ──
        .route("/api/runs", get(api::handle_api_runs_list))
        .route("/api/runs/{run_id}", get(api::handle_api_runs_get))
        // ── Phase 4.0: Channel capabilities + deliver ──
        .route(
            "/api/channels/capabilities",
            get(api::handle_api_channel_capabilities),
        )
        .route(
            "/api/channels/deliver",
            post(api::handle_api_channel_deliver),
        )
        .route("/api/cron", get(api::handle_api_cron_list))
        .route("/api/cron", post(api::handle_api_cron_add))
        .route("/api/cron/{id}", delete(api::handle_api_cron_delete))
        .route("/api/cron/{id}/runs", get(api::handle_api_cron_runs))
        .route("/api/integrations", get(api::handle_api_integrations))
        .route(
            "/api/integrations/settings",
            get(api::handle_api_integrations_settings),
        )
        .route(
            "/api/doctor",
            get(api::handle_api_doctor).post(api::handle_api_doctor),
        )
        .route("/api/memory", get(api::handle_api_memory_list))
        .route("/api/memory", post(api::handle_api_memory_store))
        .route("/api/memory/{key}", delete(api::handle_api_memory_delete))
        .route("/api/cost", get(api::handle_api_cost))
        .route("/api/cli-tools", get(api::handle_api_cli_tools))
        .route("/api/health", get(api::handle_api_health))
        // ── SSE event stream ──
        .route("/api/events", get(sse::handle_sse_events))
        // ── WebSocket agent chat ──
        .route("/ws/chat", get(ws::handle_ws_chat))
        .route("/ws/chat/proxy", get(ws::handle_ws_chat_proxy))
        // ── WebSocket node discovery ──
        .route("/ws/nodes", get(nodes::handle_ws_nodes))
        // ── Static assets (web dashboard) ──
        .route("/_app/{*path}", get(static_files::handle_static))
        // ── Config PUT with larger body limit ──
        .merge(config_put_router)
        .with_state(state)
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        // ── SPA fallback: non-API GET requests serve index.html ──
        .fallback(get(static_files::handle_spa_fallback));

    // Run the server with graceful shutdown
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown_rx.changed().await;
        tracing::info!("🦀 SynapseClaw Gateway shutting down...");
    })
    .await?;

    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// AXUM HANDLERS
// ══════════════════════════════════════════════════════════════════════════════

/// GET /health — always public (no secrets leaked)
/// Background loop: poll each registered agent's /api/status every 30s.
async fn agent_health_poll_loop(state: AppState) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    let interval = Duration::from_secs(30);

    loop {
        tokio::time::sleep(interval).await;

        let agents = state.agent_registry.list();
        for agent in agents {
            let url = format!("{}/api/status", agent.gateway_url);
            match client
                .get(&url)
                .bearer_auth(&agent.proxy_token)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        let model = body["model"].as_str().map(String::from);
                        let uptime = body["uptime_seconds"].as_u64();
                        let channels = body["channels"]
                            .as_object()
                            .map(|m| {
                                m.iter()
                                    .filter(|(_, v)| v.as_bool() == Some(true))
                                    .map(|(k, _)| k.clone())
                                    .collect()
                            })
                            .unwrap_or_default();
                        state.agent_registry.update_metadata(
                            &agent.agent_id,
                            model,
                            uptime,
                            channels,
                        );
                    }
                }
                _ => {
                    state.agent_registry.record_poll_failure(&agent.agent_id);
                    tracing::debug!(
                        agent_id = %agent.agent_id,
                        "Agent health poll failed"
                    );
                }
            }
        }
    }
}

/// Per-peer state for push auto-process counter (Phase 3.10).
struct PeerState {
    auto_process_count: u32,
    last_processed: std::time::Instant,
}

/// Agent-side background processor: on push signal, coalesce, check limits,
/// pre-fetch scoped messages from broker via HTTP peek, inject into prompt,
/// invoke agent run, then ack via broker HTTP.
///
/// Phase 3.10: broker-authoritative peek/ack for at-least-once delivery.
/// One agent::run() per peer, sequential. One-way trust check uses
/// broker-returned `from_trust_level` from peeked messages.
async fn agent_inbox_processor(
    config: Config,
    mut push_rx: tokio::sync::mpsc::UnboundedReceiver<ipc::PushMeta>,
    outbound_tx: Option<synapse_domain::bus::OutboundIntentSender>,
    run_store: Option<Arc<dyn synapse_domain::ports::run_store::RunStorePort>>,
    agent_runner: Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
) {
    let max_auto = config.agents_ipc.push_max_auto_processes;
    let cooldown = Duration::from_secs(config.agents_ipc.push_peer_cooldown_secs);
    let auto_kinds = config.agents_ipc.push_auto_process_kinds.clone();
    let one_way = config.agents_ipc.push_one_way;
    let my_trust = config.agents_ipc.trust_level;

    // Build HTTP client for broker communication
    let broker_token = match config.agents_ipc.broker_token.as_deref() {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => {
            tracing::error!("Push inbox processor: no broker_token configured, exiting");
            return;
        }
    };
    let ipc_client: Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort> =
        Arc::new(crate::tools::agents_ipc::IpcClient::new(
            &config.agents_ipc.broker_url,
            &broker_token,
            config.agents_ipc.request_timeout_secs,
        ));

    let mut peers: std::collections::HashMap<String, PeerState> = std::collections::HashMap::new();

    loop {
        // Wait for push signal
        let meta = match push_rx.recv().await {
            Some(m) => m,
            None => break,
        };

        // Coalesce: wait 100ms, collect unique peers
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut from_peers = vec![meta.from_agent.clone()];
        while let Ok(extra) = push_rx.try_recv() {
            if !from_peers.contains(&extra.from_agent) {
                from_peers.push(extra.from_agent);
            }
        }

        let now = std::time::Instant::now();

        // Process each peer sequentially (one run per peer)
        for peer in &from_peers {
            // Check per-peer counter
            let state = peers.entry(peer.clone()).or_insert(PeerState {
                auto_process_count: 0,
                last_processed: now.checked_sub(cooldown).unwrap_or(now),
            });

            // Reset counter if cooldown elapsed
            if now.duration_since(state.last_processed) >= cooldown {
                state.auto_process_count = 0;
            }

            if state.auto_process_count >= max_auto {
                tracing::warn!(
                    peer = %peer,
                    count = state.auto_process_count,
                    max = max_auto,
                    "Push auto-process limit reached for peer, suppressing"
                );
                continue;
            }

            // Pre-fetch: broker-authoritative non-consuming peek via HTTP
            let kind_refs: Vec<&str> = auto_kinds.iter().map(|s| s.as_str()).collect();
            let messages = match ipc_client
                .peek_inbox(Some(peer), Some(&kind_refs), 20)
                .await
            {
                Ok(msgs) => msgs,
                Err(e) => {
                    tracing::warn!(peer = %peer, "Push: broker peek_inbox failed: {e}");
                    continue;
                }
            };

            if messages.is_empty() {
                tracing::debug!(peer = %peer, "Push: no unread messages from peer after broker peek");
                continue;
            }

            // Extract message IDs and trust levels from broker response
            let msg_ids: Vec<i64> = messages.iter().filter_map(|m| m["id"].as_i64()).collect();

            // One-way trust check: use broker-authoritative from_trust_level
            if one_way {
                #[allow(clippy::cast_possible_truncation)]
                let from_trust = messages
                    .first()
                    .and_then(|m| m["from_trust_level"].as_u64())
                    .map(|t| t as u8);
                // Fail-closed: unknown trust → suppress when one_way=true
                if from_trust.unwrap_or(u8::MAX) > my_trust {
                    tracing::debug!(
                        peer = %peer,
                        from_trust = ?from_trust,
                        my_trust = my_trust,
                        "Push one-way: subordinate sender, suppressing auto-processing"
                    );
                    continue;
                }
            }

            // Collect pending task/query messages that expect a reply (have session_id).
            // Used by auto-reply safety net after agent::run() completes.
            let pending_replies: Vec<(String, String)> = messages
                .iter()
                .filter_map(|m| {
                    let kind = m["kind"].as_str().unwrap_or("");
                    if kind == "task" || kind == "query" {
                        if let Some(sid) = m["session_id"].as_str() {
                            let from = m["from_agent"].as_str().unwrap_or("").to_string();
                            if !from.is_empty() && !sid.is_empty() {
                                return Some((from, sid.to_string()));
                            }
                        }
                    }
                    None
                })
                .collect();

            // Format messages for injection into prompt.
            // Include session_id so the agent can call agents_reply with the
            // correct session — without it the agent literally cannot reply
            // and the auto-reply safety net becomes the only path.
            let mut formatted = String::new();
            let mut pipeline_task_detected = false;
            let mut pipeline_output_schema: Option<String> = None;
            for msg in &messages {
                let id = msg["id"].as_i64().unwrap_or(0);
                let kind = msg["kind"].as_str().unwrap_or("unknown");
                let from = msg["from_agent"].as_str().unwrap_or("unknown");
                let session_id = msg["session_id"].as_str().unwrap_or("");
                let payload = msg["payload"].as_str().unwrap_or("");

                // Phase 4.1: detect pipeline task messages
                if kind == "task" {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) {
                        if parsed.get("pipeline_step").is_some() {
                            pipeline_task_detected = true;
                            // Extract description and input for cleaner prompt
                            let step_desc = parsed["description"].as_str().unwrap_or("");
                            let step_input = &parsed["input"];
                            // Extract output schema hint from step definition (if pipeline_store available)
                            if let Some(schema) = parsed.get("output_schema") {
                                pipeline_output_schema = Some(schema.to_string());
                            }
                            use std::fmt::Write;
                            let _ = write!(
                                formatted,
                                "--- Pipeline Task (from: {from}, session_id: {session_id}) ---\n\
                                 Task: {step_desc}\n\
                                 Input data: {step_input}\n\n",
                            );
                            continue;
                        }
                    }
                }

                let payload_preview = if payload.len() > 4000 {
                    let end = truncate_at_char_boundary(payload, 4000);
                    format!("{end}… [truncated]")
                } else {
                    payload.to_string()
                };
                use std::fmt::Write;
                if session_id.is_empty() {
                    let _ = write!(
                        formatted,
                        "--- Message #{id} (kind: {kind}, from: {from}) ---\n{payload_preview}\n\n",
                    );
                } else {
                    let _ = write!(
                        formatted,
                        "--- Message #{id} (kind: {kind}, from: {from}, session_id: {session_id}) ---\n{payload_preview}\n\n",
                    );
                }
            }

            let prompt = if pipeline_task_detected {
                // Phase 4.1: pipeline-specific prompt — enforce JSON response
                let schema_hint = pipeline_output_schema
                    .map(|s| format!("\nRequired output JSON schema: {s}\n"))
                    .unwrap_or_default();
                format!(
                    "[Pipeline task from \"{peer}\"]\n\n\
                     {formatted}\
                     {schema_hint}\
                     IMPORTANT: You MUST respond with a VALID JSON object as your final output.\n\
                     Do NOT wrap it in markdown code blocks. Do NOT add explanatory text before or after.\n\
                     The pipeline engine will parse your response as JSON. If it cannot parse it, \
                     the step will be retried.\n\n\
                     Use agents_reply with the session_id above, kind=\"result\", and your JSON as payload.\n\
                     If you cannot complete the task, reply with: {{\"error\": \"reason\"}}",
                    peer = peer,
                )
            } else {
                format!(
                    "[IPC push: {} new message(s) from \"{}\"]\n\n\
                     {}\
                     Process the messages above and take action if required.\n\
                     If a message has a session_id and requires a response, use \
                     agents_reply with that session_id to send results back.\n\
                     IMPORTANT: Do NOT send acknowledgments, confirmations, or \
                     \"understood\" messages. Only reply if the message requires \
                     concrete action or contains a question that needs answering.",
                    messages.len(),
                    peer,
                    formatted,
                )
            };

            tracing::info!(
                peer = %peer,
                count = messages.len(),
                "Push notification received, invoking scoped agent inbox processing"
            );

            // Create RunContext to track tool calls during this push-triggered run.
            // Used by auto-reply safety net to detect if agents_reply was called.
            let run_ctx =
                std::sync::Arc::new(synapse_domain::domain::tool_audit::RunContext::new());

            // Phase 4.0: Track IPC run in RunStore
            let ipc_run_id = uuid::Uuid::new_v4().to_string();
            if let Some(ref store) = run_store {
                let run = synapse_domain::domain::run::Run {
                    run_id: ipc_run_id.clone(),
                    conversation_key: Some(format!("ipc:{peer}")),
                    origin: synapse_domain::domain::run::RunOrigin::Ipc,
                    state: synapse_domain::domain::run::RunState::Running,
                    #[allow(clippy::cast_sign_loss)]
                    started_at: chrono::Utc::now().timestamp() as u64,
                    finished_at: None,
                };
                if let Err(e) = store.create_run(&run).await {
                    tracing::warn!("run_store: failed to create IPC run: {e}");
                }
            }

            match agent_runner
                .run(
                    Some(prompt),
                    None,
                    None,
                    config.default_temperature,
                    false,
                    None,
                    None,
                    Some(run_ctx.clone()),
                )
                .await
            {
                Ok(last_text) => {
                    // Mark IPC run completed
                    if let Some(ref store) = run_store {
                        #[allow(clippy::cast_sign_loss)]
                        let _ = store
                            .update_state(
                                &ipc_run_id,
                                synapse_domain::domain::run::RunState::Completed,
                                Some(chrono::Utc::now().timestamp() as u64),
                            )
                            .await;
                    }

                    // Ack via broker HTTP — mark as read only after success
                    if let Err(e) = ipc_client.ack_messages(&msg_ids).await {
                        tracing::warn!(
                            peer = %peer,
                            "Push: broker ack_messages failed: {e}"
                        );
                    }

                    // ── Auto-reply safety net ────────────────────────────
                    // For each pending task/query with session_id, check if the
                    // agent sent a reply for that SPECIFIC session (via agents_reply
                    // OR agents_send kind=result).  Only fire auto-reply for
                    // sessions that got no explicit response.
                    //
                    // NOTE: auto-reply is sent unsigned (gateway ipc_client has no
                    // Ed25519 identity).  The broker accepts unsigned messages when
                    // no public key is registered for the sender.  If the broker
                    // later enforces mandatory signatures, this path needs the
                    // agent's identity loaded.
                    for (to_agent, session_id) in &pending_replies {
                        if run_ctx.was_ipc_reply_sent_for_session(session_id) {
                            continue;
                        }

                        let auto_payload = if last_text.trim().is_empty() {
                            if pipeline_task_detected {
                                r#"{"error": "agent produced no output"}"#.to_string()
                            } else {
                                "[auto-reply] Agent completed processing but produced no output."
                                    .to_string()
                            }
                        } else {
                            let scrubbed = synapse_security::scrub_credentials(last_text.trim());
                            let truncated = truncate_at_char_boundary(&scrubbed, 4000);

                            if pipeline_task_detected {
                                // Phase 4.1: try to extract JSON from agent output
                                // Agent may have wrapped JSON in text or markdown
                                extract_json_from_text(truncated)
                            } else {
                                format!(
                                    "[auto-reply] Agent completed processing but did not \
                                     send explicit reply. Last output:\n{truncated}"
                                )
                            }
                        };

                        let body = serde_json::json!({
                            "to": to_agent,
                            "kind": "result",
                            "payload": auto_payload,
                            "session_id": session_id,
                            "priority": 0,
                        });

                        match ipc_client.send_message(&body).await {
                            Ok(result) if result.success => {
                                tracing::info!(
                                    peer = %peer,
                                    to = %to_agent,
                                    session_id = %session_id,
                                    "Auto-reply safety net: sent unsigned result to originator"
                                );
                            }
                            Ok(result) => {
                                tracing::warn!(
                                    peer = %peer,
                                    to = %to_agent,
                                    status = result.status_code,
                                    "Auto-reply safety net: broker rejected auto-reply"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    peer = %peer,
                                    to = %to_agent,
                                    "Auto-reply safety net: failed to send: {e}"
                                );
                            }
                        }
                    }

                    // ── Phase 4.0: Push relay via OutboundIntent ────────
                    // If push_relay_channel + push_relay_recipient are configured,
                    // relay the agent's output to the user's channel so IPC results
                    // are visible in Matrix/Telegram/etc.
                    //
                    // Guard: only relay when there were pending task/query replies —
                    // this is the delegation use case.  FYI `text` messages should
                    // not spam the user's channel.
                    if !pending_replies.is_empty() {
                        if let (Some(relay_ch), Some(relay_rcpt)) = (
                            config.agents_ipc.push_relay_channel.as_deref(),
                            config.agents_ipc.push_relay_recipient.as_deref(),
                        ) {
                            if let Some(tx) = &outbound_tx {
                                // Scrub credentials before relaying to a human-facing
                                // channel — last_text is raw LLM output that may
                                // contain secrets from tool execution.
                                let scrubbed =
                                    synapse_security::scrub_credentials(last_text.trim());
                                let relay_text = scrubbed.as_str();
                                if !relay_text.is_empty() {
                                    let content = if relay_text.len() > 4000 {
                                        let end = truncate_at_char_boundary(relay_text, 4000);
                                        format!("{end}… [truncated]")
                                    } else {
                                        relay_text.to_string()
                                    };
                                    let intent =
                                        synapse_domain::domain::channel::OutboundIntent::notify(
                                            relay_ch, relay_rcpt, content,
                                        );
                                    if tx.send(intent) {
                                        tracing::info!(
                                            peer = %peer,
                                            channel = relay_ch,
                                            "Push relay: emitted OutboundIntent for channel delivery"
                                        );
                                    } else {
                                        tracing::warn!(
                                            peer = %peer,
                                            "Push relay: OutboundIntent bus closed, relay dropped"
                                        );
                                    }
                                }
                            }
                        }
                    }

                    state.auto_process_count += 1;
                    state.last_processed = std::time::Instant::now();
                    tracing::info!(
                        peer = %peer,
                        acked = msg_ids.len(),
                        auto_process_count = state.auto_process_count,
                        "Push-triggered inbox processing completed"
                    );
                }
                Err(e) => {
                    // Mark IPC run failed
                    if let Some(ref store) = run_store {
                        #[allow(clippy::cast_sign_loss)]
                        let _ = store
                            .update_state(
                                &ipc_run_id,
                                synapse_domain::domain::run::RunState::Failed,
                                Some(chrono::Utc::now().timestamp() as u64),
                            )
                            .await;
                    }
                    // Messages stay unread on broker — picked up by next poll/push
                    tracing::warn!(
                        peer = %peer,
                        "Push-triggered inbox processing failed: {e}"
                    );
                }
            }
        }
    }
}

async fn handle_health(State(state): State<AppState>) -> impl IntoResponse {
    let body = serde_json::json!({
        "status": "ok",
        "paired": state.pairing.is_paired(),
        "require_pairing": state.pairing.require_pairing(),
        "runtime": crate::health::snapshot_json(),
    });
    Json(body)
}

/// Prometheus content type for text exposition format.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Truncate a UTF-8 string to at most `max_bytes`, ending on a char boundary.
/// Returns the truncated slice (never panics on multi-byte characters).
/// Phase 4.1: Extract JSON from agent text output.
///
/// LLMs often wrap JSON in markdown code blocks or add explanatory text.
/// This function tries to find a JSON object in the text. Falls back to
/// wrapping the raw text as `{"result": "..."}`.
fn extract_json_from_text(text: &str) -> String {
    let trimmed = text.trim();

    // Try direct parse
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return trimmed.to_string();
    }

    // Try to find JSON in markdown code block: ```json ... ``` or ``` ... ```
    if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        // Skip optional language tag
        let content_start = after_fence.find('\n').map_or(0, |n| n + 1);
        if let Some(end) = after_fence[content_start..].find("```") {
            let candidate = after_fence[content_start..content_start + end].trim();
            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                return candidate.to_string();
            }
        }
    }

    // Try to find a JSON object between first { and last }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            let candidate = &trimmed[start..=end];
            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                return candidate.to_string();
            }
        }
    }

    // Fallback: wrap raw text as JSON
    serde_json::json!({"result": trimmed}).to_string()
}

fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn prometheus_disabled_hint() -> String {
    String::from("# Prometheus backend not enabled. Set [observability] backend = \"prometheus\" in config.\n")
}

/// GET /metrics — Prometheus text exposition format
async fn handle_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let body = {
        #[cfg(feature = "observability-prometheus")]
        {
            if let Some(prom) = state
                .observer
                .as_ref()
                .as_any()
                .downcast_ref::<synapse_observability::PrometheusObserver>()
            {
                prom.encode()
            } else {
                prometheus_disabled_hint()
            }
        }
        #[cfg(not(feature = "observability-prometheus"))]
        {
            let _ = &state;
            prometheus_disabled_hint()
        }
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        body,
    )
}

/// POST /pair — exchange one-time code for bearer token
#[axum::debug_handler]
async fn handle_pair(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let rate_key =
        client_key_from_request(Some(peer_addr), &headers, state.trust_forwarded_headers);
    if !state.rate_limiter.allow_pair(&rate_key) {
        tracing::warn!("/pair rate limit exceeded");
        let err = serde_json::json!({
            "error": "Too many pairing requests. Please retry later.",
            "retry_after": RATE_LIMIT_WINDOW_SECS,
        });
        return (StatusCode::TOO_MANY_REQUESTS, Json(err));
    }

    let code = headers
        .get("X-Pairing-Code")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match state.pairing.try_pair(code, &rate_key).await {
        Ok(Some(token)) => {
            tracing::info!("🔐 New client paired successfully");
            if let Err(err) =
                Box::pin(persist_pairing_tokens(state.config.clone(), &state.pairing)).await
            {
                tracing::error!("🔐 Pairing succeeded but token persistence failed: {err:#}");
                let body = serde_json::json!({
                    "paired": true,
                    "persisted": false,
                    "token": token,
                    "message": "Paired for this process, but failed to persist token to config.toml. Check config path and write permissions.",
                });
                return (StatusCode::OK, Json(body));
            }

            let body = serde_json::json!({
                "paired": true,
                "persisted": true,
                "token": token,
                "message": "Save this token — use it as Authorization: Bearer <token>"
            });
            (StatusCode::OK, Json(body))
        }
        Ok(None) => {
            tracing::warn!("🔐 Pairing attempt with invalid code");
            let err = serde_json::json!({"error": "Invalid pairing code"});
            (StatusCode::FORBIDDEN, Json(err))
        }
        Err(lockout_secs) => {
            tracing::warn!(
                "🔐 Pairing locked out — too many failed attempts ({lockout_secs}s remaining)"
            );
            let err = serde_json::json!({
                "error": format!("Too many failed attempts. Try again in {lockout_secs}s."),
                "retry_after": lockout_secs
            });
            (StatusCode::TOO_MANY_REQUESTS, Json(err))
        }
    }
}

async fn persist_pairing_tokens(config: Arc<Mutex<Config>>, pairing: &PairingGuard) -> Result<()> {
    let paired_tokens = pairing.tokens();
    let token_metadata = pairing.token_metadata();
    // This is needed because parking_lot's guard is not Send so we clone the inner
    // this should be removed once async mutexes are used everywhere
    let mut updated_cfg = { config.lock().clone() };
    updated_cfg.gateway.paired_tokens = paired_tokens;
    updated_cfg.gateway.token_metadata = token_metadata;
    updated_cfg
        .save()
        .await
        .context("Failed to persist paired tokens to config.toml")?;

    // Keep shared runtime config in sync with persisted tokens.
    *config.lock() = updated_cfg;
    Ok(())
}

/// Simple chat for webhook endpoint (no tools, for backward compatibility and testing).
async fn run_gateway_chat_simple(state: &AppState, message: &str) -> anyhow::Result<String> {
    let user_messages = vec![ChatMessage::user(message)];

    // Keep webhook/gateway prompts aligned with channel behavior by injecting
    // workspace-aware system context before model invocation.
    let system_prompt = {
        let config_guard = state.config.lock();
        crate::channels::build_system_prompt(
            &config_guard.workspace_dir,
            &state.model,
            &[], // tools - empty for simple chat
            &[], // skills
            Some(&config_guard.identity),
            None, // bootstrap_max_chars - use default
        )
    };

    let mut messages = Vec::with_capacity(1 + user_messages.len());
    messages.push(ChatMessage::system(system_prompt));
    messages.extend(user_messages);

    let multimodal_config = state.config.lock().multimodal.clone();
    let prepared =
        synapse_providers::multimodal::prepare_messages_for_provider(&messages, &multimodal_config)
            .await?;

    state
        .provider
        .chat_with_history(&prepared.messages, &state.model, state.temperature)
        .await
}

/// Full-featured chat with tools for channel handlers (WhatsApp, Linq, Nextcloud Talk).
async fn run_gateway_chat_with_tools(
    state: &AppState,
    message: &str,
    session_id: Option<&str>,
) -> anyhow::Result<String> {
    state
        .agent_runner
        .process_message(message, session_id)
        .await
}

/// Webhook request body
#[derive(serde::Deserialize)]
pub struct WebhookBody {
    pub message: String,
}

/// POST /webhook — main webhook endpoint
async fn handle_webhook(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<WebhookBody>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let rate_key =
        client_key_from_request(Some(peer_addr), &headers, state.trust_forwarded_headers);
    if !state.rate_limiter.allow_webhook(&rate_key) {
        tracing::warn!("/webhook rate limit exceeded");
        let err = serde_json::json!({
            "error": "Too many webhook requests. Please retry later.",
            "retry_after": RATE_LIMIT_WINDOW_SECS,
        });
        return (StatusCode::TOO_MANY_REQUESTS, Json(err));
    }

    // ── Bearer token auth (pairing) ──
    if state.pairing.require_pairing() {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = auth.strip_prefix("Bearer ").unwrap_or("");
        if !state.pairing.is_authenticated(token) {
            tracing::warn!("Webhook: rejected — not paired / invalid bearer token");
            let err = serde_json::json!({
                "error": "Unauthorized — pair first via POST /pair, then send Authorization: Bearer <token>"
            });
            return (StatusCode::UNAUTHORIZED, Json(err));
        }
    }

    // ── Webhook secret auth (optional, additional layer) ──
    if let Some(ref secret_hash) = state.webhook_secret_hash {
        let header_hash = headers
            .get("X-Webhook-Secret")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(hash_webhook_secret);
        match header_hash {
            Some(val) if constant_time_eq(&val, secret_hash.as_ref()) => {}
            _ => {
                tracing::warn!("Webhook: rejected request — invalid or missing X-Webhook-Secret");
                let err = serde_json::json!({"error": "Unauthorized — invalid or missing X-Webhook-Secret header"});
                return (StatusCode::UNAUTHORIZED, Json(err));
            }
        }
    }

    // ── Parse body ──
    let Json(webhook_body) = match body {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Webhook JSON parse error: {e}");
            let err = serde_json::json!({
                "error": "Invalid JSON body. Expected: {\"message\": \"...\"}"
            });
            return (StatusCode::BAD_REQUEST, Json(err));
        }
    };

    // ── Idempotency (optional) ──
    if let Some(idempotency_key) = headers
        .get("X-Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !state.idempotency_store.record_if_new(idempotency_key) {
            tracing::info!("Webhook duplicate ignored (idempotency key: {idempotency_key})");
            let body = serde_json::json!({
                "status": "duplicate",
                "idempotent": true,
                "message": "Request already processed for this idempotency key"
            });
            return (StatusCode::OK, Json(body));
        }
    }

    let message = &webhook_body.message;
    let session_id = webhook_session_id(&headers);

    if state.auto_save && !synapse_domain::domain::util::should_skip_autosave_content(message) {
        let key = webhook_memory_key();
        let _ = state
            .mem
            .store(
                &key,
                message,
                &MemoryCategory::Conversation,
                session_id.as_deref(),
            )
            .await;
    }

    let provider_label = state
        .config
        .lock()
        .default_provider
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let model_label = state.model.clone();
    let started_at = Instant::now();

    state
        .observer
        .record_event(&synapse_observability::ObserverEvent::AgentStart {
            provider: provider_label.clone(),
            model: model_label.clone(),
        });
    state
        .observer
        .record_event(&synapse_observability::ObserverEvent::LlmRequest {
            provider: provider_label.clone(),
            model: model_label.clone(),
            messages_count: 1,
        });

    match run_gateway_chat_simple(&state, message).await {
        Ok(response) => {
            let duration = started_at.elapsed();
            state
                .observer
                .record_event(&synapse_observability::ObserverEvent::LlmResponse {
                    provider: provider_label.clone(),
                    model: model_label.clone(),
                    duration,
                    success: true,
                    error_message: None,
                    input_tokens: None,
                    output_tokens: None,
                });
            state.observer.record_metric(
                &synapse_observability::traits::ObserverMetric::RequestLatency(duration),
            );
            state
                .observer
                .record_event(&synapse_observability::ObserverEvent::AgentEnd {
                    provider: provider_label,
                    model: model_label,
                    duration,
                    tokens_used: None,
                    cost_usd: None,
                });

            let body = serde_json::json!({"response": response, "model": state.model});
            (StatusCode::OK, Json(body))
        }
        Err(e) => {
            let duration = started_at.elapsed();
            let sanitized = synapse_providers::sanitize_api_error(&e.to_string());

            state
                .observer
                .record_event(&synapse_observability::ObserverEvent::LlmResponse {
                    provider: provider_label.clone(),
                    model: model_label.clone(),
                    duration,
                    success: false,
                    error_message: Some(sanitized.clone()),
                    input_tokens: None,
                    output_tokens: None,
                });
            state.observer.record_metric(
                &synapse_observability::traits::ObserverMetric::RequestLatency(duration),
            );
            state
                .observer
                .record_event(&synapse_observability::ObserverEvent::Error {
                    component: "gateway".to_string(),
                    message: sanitized.clone(),
                });
            state
                .observer
                .record_event(&synapse_observability::ObserverEvent::AgentEnd {
                    provider: provider_label,
                    model: model_label,
                    duration,
                    tokens_used: None,
                    cost_usd: None,
                });

            tracing::error!("Webhook provider error: {}", sanitized);
            let err = serde_json::json!({"error": "LLM request failed"});
            (StatusCode::INTERNAL_SERVER_ERROR, Json(err))
        }
    }
}

/// `WhatsApp` verification query params
#[derive(serde::Deserialize)]
pub struct WhatsAppVerifyQuery {
    #[serde(rename = "hub.mode")]
    pub mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    pub verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    pub challenge: Option<String>,
}

/// GET /whatsapp — Meta webhook verification
async fn handle_whatsapp_verify(
    State(state): State<AppState>,
    Query(params): Query<WhatsAppVerifyQuery>,
) -> impl IntoResponse {
    let Some(ref wa) = state.whatsapp else {
        return (StatusCode::NOT_FOUND, "WhatsApp not configured".to_string());
    };

    // Verify the token matches (constant-time comparison to prevent timing attacks)
    let token_matches = params
        .verify_token
        .as_deref()
        .is_some_and(|t| constant_time_eq(t, wa.verify_token()));
    if params.mode.as_deref() == Some("subscribe") && token_matches {
        if let Some(ch) = params.challenge {
            tracing::info!("WhatsApp webhook verified successfully");
            return (StatusCode::OK, ch);
        }
        return (StatusCode::BAD_REQUEST, "Missing hub.challenge".to_string());
    }

    tracing::warn!("WhatsApp webhook verification failed — token mismatch");
    (StatusCode::FORBIDDEN, "Forbidden".to_string())
}

/// Verify `WhatsApp` webhook signature (`X-Hub-Signature-256`).
/// Returns true if the signature is valid, false otherwise.
/// See: <https://developers.facebook.com/docs/graph-api/webhooks/getting-started#verification-requests>
pub fn verify_whatsapp_signature(app_secret: &str, body: &[u8], signature_header: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // Signature format: "sha256=<hex_signature>"
    let Some(hex_sig) = signature_header.strip_prefix("sha256=") else {
        return false;
    };

    // Decode hex signature
    let Ok(expected) = hex::decode(hex_sig) else {
        return false;
    };

    // Compute HMAC-SHA256
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()) else {
        return false;
    };
    mac.update(body);

    // Constant-time comparison
    mac.verify_slice(&expected).is_ok()
}

/// POST /whatsapp — incoming message webhook
async fn handle_whatsapp_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(ref wa) = state.whatsapp else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "WhatsApp not configured"})),
        );
    };

    // ── Security: Verify X-Hub-Signature-256 if app_secret is configured ──
    if let Some(ref app_secret) = state.whatsapp_app_secret {
        let signature = headers
            .get("X-Hub-Signature-256")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !verify_whatsapp_signature(app_secret, &body, signature) {
            tracing::warn!(
                "WhatsApp webhook signature verification failed (signature: {})",
                if signature.is_empty() {
                    "missing"
                } else {
                    "invalid"
                }
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Invalid signature"})),
            );
        }
    }

    // Parse JSON body
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid JSON payload"})),
        );
    };

    // Parse messages from the webhook payload
    let messages = wa.parse_webhook_payload(&payload);

    if messages.is_empty() {
        // Acknowledge the webhook even if no messages (could be status updates)
        return (StatusCode::OK, Json(serde_json::json!({"status": "ok"})));
    }

    // Process each message
    for msg in &messages {
        tracing::info!(
            "WhatsApp message from {}: {}",
            msg.sender,
            truncate_with_ellipsis(&msg.content, 50)
        );
        let session_id = sender_session_id("whatsapp", msg);

        // Auto-save to memory
        if state.auto_save
            && !synapse_domain::domain::util::should_skip_autosave_content(&msg.content)
        {
            let key = whatsapp_memory_key(msg);
            let _ = state
                .mem
                .store(
                    &key,
                    &msg.content,
                    &MemoryCategory::Conversation,
                    Some(&session_id),
                )
                .await;
        }

        match Box::pin(run_gateway_chat_with_tools(
            &state,
            &msg.content,
            Some(&session_id),
        ))
        .await
        {
            Ok(response) => {
                // Send reply via WhatsApp
                if let Err(e) = wa
                    .send(&SendMessage::new(response, &msg.reply_target))
                    .await
                {
                    tracing::error!("Failed to send WhatsApp reply: {e}");
                }
                // Fire-and-forget: rolling session summary
                let st = state.clone();
                let sk = session_id.clone();
                tokio::spawn(async move {
                    ws::summarize_session_if_needed(&st, &sk).await;
                });
            }
            Err(e) => {
                tracing::error!("LLM error for WhatsApp message: {e:#}");
                let _ = wa
                    .send(&SendMessage::new(
                        "Sorry, I couldn't process your message right now.",
                        &msg.reply_target,
                    ))
                    .await;
            }
        }
    }

    // Acknowledge the webhook
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

/// POST /linq — incoming message webhook (iMessage/RCS/SMS via Linq)
async fn handle_linq_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(ref linq) = state.linq else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Linq not configured"})),
        );
    };

    let body_str = String::from_utf8_lossy(&body);

    // ── Security: Verify X-Webhook-Signature if signing_secret is configured ──
    if let Some(ref signing_secret) = state.linq_signing_secret {
        let timestamp = headers
            .get("X-Webhook-Timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let signature = headers
            .get("X-Webhook-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !crate::channels::linq::verify_linq_signature(
            signing_secret,
            &body_str,
            timestamp,
            signature,
        ) {
            tracing::warn!(
                "Linq webhook signature verification failed (signature: {})",
                if signature.is_empty() {
                    "missing"
                } else {
                    "invalid"
                }
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Invalid signature"})),
            );
        }
    }

    // Parse JSON body
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid JSON payload"})),
        );
    };

    // Parse messages from the webhook payload
    let messages = linq.parse_webhook_payload(&payload);

    if messages.is_empty() {
        // Acknowledge the webhook even if no messages (could be status/delivery events)
        return (StatusCode::OK, Json(serde_json::json!({"status": "ok"})));
    }

    // Process each message
    for msg in &messages {
        tracing::info!(
            "Linq message from {}: {}",
            msg.sender,
            truncate_with_ellipsis(&msg.content, 50)
        );
        let session_id = sender_session_id("linq", msg);

        // Auto-save to memory
        if state.auto_save
            && !synapse_domain::domain::util::should_skip_autosave_content(&msg.content)
        {
            let key = linq_memory_key(msg);
            let _ = state
                .mem
                .store(
                    &key,
                    &msg.content,
                    &MemoryCategory::Conversation,
                    Some(&session_id),
                )
                .await;
        }

        // Call the LLM
        match Box::pin(run_gateway_chat_with_tools(
            &state,
            &msg.content,
            Some(&session_id),
        ))
        .await
        {
            Ok(response) => {
                // Send reply via Linq
                if let Err(e) = linq
                    .send(&SendMessage::new(response, &msg.reply_target))
                    .await
                {
                    tracing::error!("Failed to send Linq reply: {e}");
                }
                // Fire-and-forget: rolling session summary
                let st = state.clone();
                let sk = session_id.clone();
                tokio::spawn(async move {
                    ws::summarize_session_if_needed(&st, &sk).await;
                });
            }
            Err(e) => {
                tracing::error!("LLM error for Linq message: {e:#}");
                let _ = linq
                    .send(&SendMessage::new(
                        "Sorry, I couldn't process your message right now.",
                        &msg.reply_target,
                    ))
                    .await;
            }
        }
    }

    // Acknowledge the webhook
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

/// GET /wati — WATI webhook verification (echoes hub.challenge)
async fn handle_wati_verify(
    State(state): State<AppState>,
    Query(params): Query<WatiVerifyQuery>,
) -> impl IntoResponse {
    if state.wati.is_none() {
        return (StatusCode::NOT_FOUND, "WATI not configured".to_string());
    }

    // WATI may use Meta-style webhook verification; echo the challenge
    if let Some(challenge) = params.challenge {
        tracing::info!("WATI webhook verified successfully");
        return (StatusCode::OK, challenge);
    }

    (StatusCode::BAD_REQUEST, "Missing hub.challenge".to_string())
}

#[derive(Debug, serde::Deserialize)]
pub struct WatiVerifyQuery {
    #[serde(rename = "hub.challenge")]
    pub challenge: Option<String>,
}

/// POST /wati — incoming WATI WhatsApp message webhook
async fn handle_wati_webhook(State(state): State<AppState>, body: Bytes) -> impl IntoResponse {
    let Some(ref wati) = state.wati else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "WATI not configured"})),
        );
    };

    // Parse JSON body
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid JSON payload"})),
        );
    };

    // Parse messages from the webhook payload
    let messages = wati.parse_webhook_payload(&payload);

    if messages.is_empty() {
        return (StatusCode::OK, Json(serde_json::json!({"status": "ok"})));
    }

    // Process each message
    for msg in &messages {
        tracing::info!(
            "WATI message from {}: {}",
            msg.sender,
            truncate_with_ellipsis(&msg.content, 50)
        );
        let session_id = sender_session_id("wati", msg);

        // Auto-save to memory
        if state.auto_save
            && !synapse_domain::domain::util::should_skip_autosave_content(&msg.content)
        {
            let key = wati_memory_key(msg);
            let _ = state
                .mem
                .store(
                    &key,
                    &msg.content,
                    &MemoryCategory::Conversation,
                    Some(&session_id),
                )
                .await;
        }

        // Call the LLM
        match Box::pin(run_gateway_chat_with_tools(
            &state,
            &msg.content,
            Some(&session_id),
        ))
        .await
        {
            Ok(response) => {
                // Send reply via WATI
                if let Err(e) = wati
                    .send(&SendMessage::new(response, &msg.reply_target))
                    .await
                {
                    tracing::error!("Failed to send WATI reply: {e}");
                }
                // Fire-and-forget: rolling session summary
                let st = state.clone();
                let sk = session_id.clone();
                tokio::spawn(async move {
                    ws::summarize_session_if_needed(&st, &sk).await;
                });
            }
            Err(e) => {
                tracing::error!("LLM error for WATI message: {e:#}");
                let _ = wati
                    .send(&SendMessage::new(
                        "Sorry, I couldn't process your message right now.",
                        &msg.reply_target,
                    ))
                    .await;
            }
        }
    }

    // Acknowledge the webhook
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

/// POST /nextcloud-talk — incoming message webhook (Nextcloud Talk bot API)
async fn handle_nextcloud_talk_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(ref nextcloud_talk) = state.nextcloud_talk else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Nextcloud Talk not configured"})),
        );
    };

    let body_str = String::from_utf8_lossy(&body);

    // ── Security: Verify Nextcloud Talk HMAC signature if secret is configured ──
    if let Some(ref webhook_secret) = state.nextcloud_talk_webhook_secret {
        let random = headers
            .get("X-Nextcloud-Talk-Random")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let signature = headers
            .get("X-Nextcloud-Talk-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !crate::channels::nextcloud_talk::verify_nextcloud_talk_signature(
            webhook_secret,
            random,
            &body_str,
            signature,
        ) {
            tracing::warn!(
                "Nextcloud Talk webhook signature verification failed (signature: {})",
                if signature.is_empty() {
                    "missing"
                } else {
                    "invalid"
                }
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Invalid signature"})),
            );
        }
    }

    // Parse JSON body
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid JSON payload"})),
        );
    };

    // Parse messages from webhook payload
    let messages = nextcloud_talk.parse_webhook_payload(&payload);
    if messages.is_empty() {
        // Acknowledge webhook even if payload does not contain actionable user messages.
        return (StatusCode::OK, Json(serde_json::json!({"status": "ok"})));
    }

    for msg in &messages {
        tracing::info!(
            "Nextcloud Talk message from {}: {}",
            msg.sender,
            truncate_with_ellipsis(&msg.content, 50)
        );
        let session_id = sender_session_id("nextcloud_talk", msg);

        if state.auto_save
            && !synapse_domain::domain::util::should_skip_autosave_content(&msg.content)
        {
            let key = nextcloud_talk_memory_key(msg);
            let _ = state
                .mem
                .store(
                    &key,
                    &msg.content,
                    &MemoryCategory::Conversation,
                    Some(&session_id),
                )
                .await;
        }

        match Box::pin(run_gateway_chat_with_tools(
            &state,
            &msg.content,
            Some(&session_id),
        ))
        .await
        {
            Ok(response) => {
                if let Err(e) = nextcloud_talk
                    .send(&SendMessage::new(response, &msg.reply_target))
                    .await
                {
                    tracing::error!("Failed to send Nextcloud Talk reply: {e}");
                }
                // Fire-and-forget: rolling session summary
                let st = state.clone();
                let sk = session_id.clone();
                tokio::spawn(async move {
                    ws::summarize_session_if_needed(&st, &sk).await;
                });
            }
            Err(e) => {
                tracing::error!("LLM error for Nextcloud Talk message: {e:#}");
                let _ = nextcloud_talk
                    .send(&SendMessage::new(
                        "Sorry, I couldn't process your message right now.",
                        &msg.reply_target,
                    ))
                    .await;
            }
        }
    }

    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

// ══════════════════════════════════════════════════════════════════════════════
// ADMIN HANDLERS (for CLI management)
// ══════════════════════════════════════════════════════════════════════════════

/// Response for admin endpoints
#[derive(serde::Serialize)]
struct AdminResponse {
    success: bool,
    message: String,
}

// ── Admin CIDR allowlist ─────────────────────────────────────────

/// Parsed IPv4 CIDR for admin endpoint access control.
#[derive(Debug, Clone)]
pub struct AdminCidr {
    network: u32,
    mask: u32,
}

impl AdminCidr {
    /// Parse a CIDR string like "100.64.0.0/10".
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let (addr_str, prefix_str) = s
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("expected CIDR format A.B.C.D/N, got: {s}"))?;
        let addr: std::net::Ipv4Addr = addr_str
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid IPv4 address in CIDR {s}: {e}"))?;
        let prefix: u32 = prefix_str
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid prefix length in CIDR {s}: {e}"))?;
        if prefix > 32 {
            anyhow::bail!("prefix length must be 0..=32, got {prefix} in CIDR {s}");
        }
        let mask = if prefix == 0 {
            0
        } else {
            !0u32 << (32 - prefix)
        };
        let network = u32::from(addr) & mask;
        Ok(Self { network, mask })
    }

    /// Check whether an IPv4 address falls within this CIDR range.
    pub fn contains(&self, ip: std::net::Ipv4Addr) -> bool {
        u32::from(ip) & self.mask == self.network
    }
}

/// Reject requests not from loopback or a configured admin CIDR.
pub(crate) fn require_localhost(
    peer: &SocketAddr,
    admin_cidrs: &[AdminCidr],
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let ip = peer.ip();
    if ip.is_loopback() {
        return Ok(());
    }
    // Extract IPv4 from IPv6-mapped addresses (::ffff:A.B.C.D)
    let v4 = match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    };
    if let Some(v4) = v4 {
        if v4.is_loopback() {
            return Ok(());
        }
        if admin_cidrs.iter().any(|cidr| cidr.contains(v4)) {
            return Ok(());
        }
    }
    tracing::debug!(
        peer_ip = %ip,
        v4 = ?v4,
        cidrs = admin_cidrs.len(),
        "admin access denied"
    );
    Err((
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": format!("Admin endpoints restricted to localhost/admin_cidrs (peer: {ip})")
        })),
    ))
}

/// POST /admin/shutdown — graceful shutdown from CLI (localhost only)
async fn handle_admin_shutdown(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    tracing::info!("🔌 Admin shutdown request received — initiating graceful shutdown");

    let body = AdminResponse {
        success: true,
        message: "Gateway shutdown initiated".to_string(),
    };

    let _ = state.shutdown_tx.send(true);

    Ok((StatusCode::OK, Json(body)))
}

/// GET /admin/paircode — fetch current pairing code (localhost only)
async fn handle_admin_paircode(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    let code = state.pairing.pairing_code();

    let body = if let Some(c) = code {
        serde_json::json!({
            "success": true,
            "pairing_required": state.pairing.require_pairing(),
            "pairing_code": c,
            "message": "Use this one-time code to pair"
        })
    } else {
        serde_json::json!({
            "success": true,
            "pairing_required": state.pairing.require_pairing(),
            "pairing_code": null,
            "message": if state.pairing.require_pairing() {
                "Pairing is active but no new code available (already paired or code expired)"
            } else {
                "Pairing is disabled for this gateway"
            }
        })
    };

    Ok((StatusCode::OK, Json(body)))
}

/// Optional body for `POST /admin/paircode/new` to bind IPC metadata to the new code.
#[derive(Debug, serde::Deserialize)]
struct PaircodeNewBody {
    agent_id: String,
    #[serde(default = "default_paircode_trust_level")]
    trust_level: u8,
    #[serde(default = "default_paircode_role")]
    role: String,
}

fn default_paircode_trust_level() -> u8 {
    3
}

fn default_paircode_role() -> String {
    "agent".into()
}

/// POST /admin/paircode/new — generate a new pairing code (localhost only)
///
/// Accepts an optional JSON body with `agent_id`, `trust_level`, and `role`
/// to bind IPC metadata to the pairing code. When the code is used to pair,
/// the resulting token inherits this metadata. Without a body, the code
/// produces a legacy human token (no IPC access).
async fn handle_admin_paircode_new(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Option<Json<PaircodeNewBody>>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    match state.pairing.generate_new_pairing_code() {
        Some(code) => {
            // If metadata was provided, bind it to the pairing code
            if let Some(Json(meta_body)) = body {
                let metadata = synapse_domain::config::schema::TokenMetadata {
                    agent_id: meta_body.agent_id,
                    trust_level: meta_body.trust_level,
                    role: meta_body.role,
                };
                state.pairing.set_pending_metadata(&code, metadata);
                tracing::info!("🔐 New IPC pairing code generated via admin endpoint");
            } else {
                tracing::info!("🔐 New pairing code generated via admin endpoint");
            }
            let body = serde_json::json!({
                "success": true,
                "pairing_required": state.pairing.require_pairing(),
                "pairing_code": code,
                "message": "New pairing code generated — use this one-time code to pair"
            });
            Ok((StatusCode::OK, Json(body)))
        }
        None => {
            let body = serde_json::json!({
                "success": false,
                "pairing_required": false,
                "pairing_code": null,
                "message": "Pairing is disabled for this gateway"
            });
            Ok((StatusCode::BAD_REQUEST, Json(body)))
        }
    }
}

#[cfg(test)]
mod gateway_tests;
