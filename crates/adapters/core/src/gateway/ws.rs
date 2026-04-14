//! WebSocket agent chat handler with RPC-based session management.
//!
//! Protocol:
//! ```text
//! Client -> Server: {"type":"rpc","id":"x","method":"chat.send","params":{}}
//! Server -> Client: {"type":"rpc_response","id":"x","result":{}}
//! Server -> Client: {"type":"error","message":"..."}                          (server-push)
//! ```
//!
//! RPC methods: chat.send, chat.history, chat.abort,
//!              sessions.list, sessions.new, sessions.rename, sessions.delete, sessions.reset

use super::{AppState, ChatSession};
use crate::runtime::agent_runtime_adapter::ChannelAgentRuntime;
use crate::runtime::runtime_error_classification::classify_agent_runtime_error;
use crate::runtime_adapter_contract::{
    execute_runtime_command_output, RuntimeCommandHost, RuntimeModelHelpSnapshot,
    RuntimeModelSwitchOutcome, RuntimeProviderSwitchOutcome, RuntimeRouteMutationRequest,
    WebRuntimeAdapterContract,
};
use crate::runtime_routes::{RuntimeCapabilityDoctorInput, WorkspaceModelProfileCatalog};
use crate::runtime_tool_notifications::RuntimeToolNotification;
use crate::runtime_tool_observer::{RuntimeToolNotificationHandler, RuntimeToolNotifyObserver};
use synapse_domain::application::services::assistant_output_presentation::PresentedOutput;
use synapse_domain::application::services::route_switch_preflight::{
    RouteSwitchPreflight, RouteSwitchStatus,
};
use synapse_domain::application::services::runtime_error_presentation::format_context_limit_recovery_response;
use synapse_domain::application::use_cases::handle_inbound_message::{
    self as web_inbound, HandleResult,
};
use synapse_domain::config::schema::CapabilityLane;
use synapse_domain::domain::channel::{
    InboundEnvelope, InboundMediaAttachment, InboundMediaKind, SourceKind,
};
use synapse_domain::domain::conversation::{
    ConversationEvent, ConversationKind, ConversationSession, EventType,
};
// Run types no longer needed directly — lifecycle managed by conversation_service
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Multipart, Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use synapse_domain::ports::agent_runtime::AgentRuntimeErrorKind;
use synapse_domain::ports::channel_registry::ChannelRegistryPort;
use synapse_domain::ports::hooks::NoOpHooks;
use synapse_domain::ports::route_selection::RouteSelection;
use synapse_infra::approval::ApprovalManager;

/// The sub-protocol we support for the chat WebSocket.
const WS_PROTOCOL: &str = "synapseclaw.v1";

/// Prefix used in `Sec-WebSocket-Protocol` to carry a bearer token.
const BEARER_SUBPROTO_PREFIX: &str = "bearer.";

/// Max sessions kept in memory per token prefix.
const MAX_MEMORY_SESSIONS: usize = 50;

/// Auto-label truncation length.
const AUTO_LABEL_MAX_LEN: usize = 40;
const MAX_WEB_MEDIA_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
    pub session_id: Option<String>,
}

/// Extract a bearer token from WebSocket-compatible sources.
///
/// Precedence (first non-empty wins):
/// 1. `Authorization: Bearer <token>` header
/// 2. `Sec-WebSocket-Protocol: bearer.<token>` subprotocol
/// 3. `?token=<token>` query parameter
fn extract_ws_token<'a>(headers: &'a HeaderMap, query_token: Option<&'a str>) -> Option<&'a str> {
    // 1. Authorization header
    if let Some(t) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
    {
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 2. Sec-WebSocket-Protocol: bearer.<token>
    if let Some(t) = headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .and_then(|protos| {
            protos
                .split(',')
                .map(|p| p.trim())
                .find_map(|p| p.strip_prefix(BEARER_SUBPROTO_PREFIX))
        })
    {
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 3. ?token= query parameter
    if let Some(t) = query_token {
        if !t.is_empty() {
            return Some(t);
        }
    }

    None
}

/// Derive token hash prefix for session keys: first 16 hex chars of SHA-256.
pub(crate) fn token_hash_prefix(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(&digest[..8])
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn canonical_web_conversation_key(
    agent_id: &str,
    session_key: &str,
    token_prefix: &str,
    thread_ref: Option<String>,
) -> String {
    let envelope = InboundEnvelope {
        source_kind: SourceKind::Web,
        source_adapter: "web".to_string(),
        actor_id: format!("web:{token_prefix}"),
        conversation_id: session_key.to_string(),
        event_ref: None,
        reply_ref: session_key.to_string(),
        thread_ref,
        media_attachments: Vec::new(),
        content: String::new(),
        received_at: 0,
    };
    synapse_domain::application::services::inbound_message_service::conversation_key_for_agent(
        &envelope, agent_id,
    )
}

fn canonical_web_conversation_scope_prefix(agent_id: &str, session_key: &str) -> String {
    let envelope = InboundEnvelope {
        source_kind: SourceKind::Web,
        source_adapter: "web".to_string(),
        actor_id: String::new(),
        conversation_id: session_key.to_string(),
        event_ref: None,
        reply_ref: session_key.to_string(),
        thread_ref: None,
        media_attachments: Vec::new(),
        content: String::new(),
        received_at: 0,
    };
    synapse_domain::application::services::inbound_message_service::conversation_scope_key_prefix_for_agent(
        &envelope, agent_id,
    )
}

fn canonical_web_conversation_key_for_state(
    state: &AppState,
    session_key: &str,
    token_prefix: &str,
    thread_ref: Option<String>,
) -> anyhow::Result<String> {
    let config_snapshot = state.config.lock().clone();
    let inbound_config = build_web_inbound_config(state, &config_snapshot)?;
    Ok(canonical_web_conversation_key(
        &inbound_config.agent_id,
        session_key,
        token_prefix,
        thread_ref,
    ))
}

fn canonical_web_conversation_scope_prefix_for_state(
    state: &AppState,
    session_key: &str,
) -> anyhow::Result<String> {
    let config_snapshot = state.config.lock().clone();
    let inbound_config = build_web_inbound_config(state, &config_snapshot)?;
    Ok(canonical_web_conversation_scope_prefix(
        &inbound_config.agent_id,
        session_key,
    ))
}

fn default_web_provider(config: &synapse_domain::config::schema::Config) -> String {
    config
        .default_provider
        .clone()
        .or_else(|| synapse_domain::config::model_catalog::default_provider().map(str::to_string))
        .unwrap_or_default()
}

fn default_web_model(state: &AppState, config: &synapse_domain::config::schema::Config) -> String {
    config
        .default_model
        .clone()
        .unwrap_or_else(|| state.model.clone())
}

fn default_web_route_selection(
    state: &AppState,
    config: &synapse_domain::config::schema::Config,
) -> RouteSelection {
    RouteSelection {
        provider: default_web_provider(config),
        model: default_web_model(state, config),
        lane: None,
        candidate_index: None,
        last_admission: None,
        recent_admissions: Vec::new(),
        last_tool_repair: None,
        recent_tool_repairs: Vec::new(),
        context_cache: None,
        assumptions: Vec::new(),
        calibrations: Vec::new(),
        watchdog_alerts: Vec::new(),
        handoff_artifacts: Vec::new(),
        runtime_decision_traces: Vec::new(),
    }
}

fn web_provider_capabilities_for_route(
    state: &AppState,
    config: &synapse_domain::config::schema::Config,
    route: &RouteSelection,
) -> synapse_domain::ports::provider::ProviderCapabilities {
    let default_provider = default_web_provider(config);
    if route
        .provider
        .eq_ignore_ascii_case(default_provider.as_str())
    {
        return state.provider.capabilities();
    }
    state
        .provider_cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&route.provider)
        .map(|provider| provider.capabilities())
        .unwrap_or_default()
}

/// Query params for the proxy WebSocket.
#[derive(Deserialize)]
pub struct WsProxyQuery {
    pub token: Option<String>,
    pub agent: Option<String>,
}

/// GET /ws/chat/proxy — WebSocket proxy to a specific agent's chat (Phase 3.8).
///
/// Browser connects here with `?agent=<agent_id>`. Broker looks up the agent
/// in AgentRegistry, opens upstream WS to agent's gateway, and relays frames
/// bidirectionally (transparent, no parsing).
///
/// **Operator isolation (Phase 3.8 Finding 1):** The broker derives an
/// `operator_id` from the browser's bearer token hash and forwards it as
/// `?session_id=op:{operator_id}` to the agent. This ensures each browser
/// operator gets an isolated session namespace on the remote agent, even
/// though all proxied connections share the same `proxy_token`.
pub async fn handle_ws_chat_proxy(
    State(state): State<AppState>,
    Query(params): Query<WsProxyQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Auth: browser must be paired with broker
    let raw_token = extract_ws_token(&headers, params.token.as_deref()).unwrap_or("");
    if state.pairing.require_pairing() && !state.pairing.is_authenticated(raw_token) {
        return (axum::http::StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    let agent_id = match params.agent {
        Some(ref id) if !id.is_empty() => id.clone(),
        _ => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "Missing ?agent= parameter",
            )
                .into_response();
        }
    };

    // Look up agent in registry
    let agent_info = match state.agent_registry.get(&agent_id) {
        Some(info) => info,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                "Agent not found in registry",
            )
                .into_response();
        }
    };

    if agent_info.status == super::agent_registry::AgentStatus::Offline {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Agent is offline",
        )
            .into_response();
    }

    let ws = if headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |protos| {
            protos.split(',').any(|p| p.trim() == WS_PROTOCOL)
        }) {
        ws.protocols([WS_PROTOCOL])
    } else {
        ws
    };

    // Derive operator identity from browser token for per-operator isolation
    let operator_id = token_hash_prefix(raw_token);

    ws.on_upgrade(move |socket| handle_proxy_socket(socket, agent_info, operator_id))
        .into_response()
}

/// Bidirectional WS relay: browser ↔ broker ↔ agent.
async fn handle_proxy_socket(
    browser_socket: WebSocket,
    agent_info: super::agent_registry::AgentInfo,
    operator_id: String,
) {
    use tokio_tungstenite::tungstenite;

    // Build upstream URL with operator-scoped session_id for isolation.
    // Each browser operator gets a unique session prefix on the remote agent,
    // preventing shared namespace when multiple operators use the same broker.
    let upstream_url = agent_info
        .gateway_url
        .replace("http://", "ws://")
        .replace("https://", "wss://");
    let upstream_url = format!("{upstream_url}/ws/chat?session_id=op:{operator_id}");

    // Connect to agent's WS with subprotocol auth
    let host = tungstenite::http::Uri::try_from(&upstream_url)
        .ok()
        .and_then(|u| u.authority().map(|a| a.to_string()))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let request = tungstenite::http::Request::builder()
        .uri(&upstream_url)
        .header("Host", &host)
        .header(
            "Sec-WebSocket-Protocol",
            format!(
                "{WS_PROTOCOL}, {BEARER_SUBPROTO_PREFIX}{}",
                agent_info.proxy_token
            ),
        )
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .unwrap();

    let agent_ws = match tokio_tungstenite::connect_async(request).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            tracing::warn!("Failed to connect to agent WS at {upstream_url}: {e}");
            let (mut sender, _) = browser_socket.split();
            let err = serde_json::json!({"type": "error", "message": "Failed to connect to agent"});
            let _ = sender.send(Message::Text(err.to_string().into())).await;
            return;
        }
    };

    let (mut browser_send, mut browser_recv) = browser_socket.split();
    let (mut agent_send, mut agent_recv) = agent_ws.split();

    // Browser → Agent relay
    let b2a = tokio::spawn(async move {
        while let Some(Ok(msg)) = browser_recv.next().await {
            let tung_msg = match msg {
                Message::Text(t) => tungstenite::Message::text(t.to_string()),
                Message::Binary(b) => tungstenite::Message::binary(b.to_vec()),
                Message::Ping(p) => tungstenite::Message::Ping(p.to_vec().into()),
                Message::Pong(p) => tungstenite::Message::Pong(p.to_vec().into()),
                Message::Close(_) => break,
            };
            if agent_send.send(tung_msg).await.is_err() {
                break;
            }
        }
    });

    // Agent → Browser relay
    let a2b = tokio::spawn(async move {
        while let Some(Ok(msg)) = agent_recv.next().await {
            let axum_msg = match msg {
                tungstenite::Message::Text(t) => Message::Text(t.to_string().into()),
                tungstenite::Message::Binary(b) => Message::Binary(b.to_vec().into()),
                tungstenite::Message::Ping(p) => Message::Ping(p.to_vec().into()),
                tungstenite::Message::Pong(p) => Message::Pong(p.to_vec().into()),
                tungstenite::Message::Close(_) | tungstenite::Message::Frame(_) => break,
            };
            if browser_send.send(axum_msg).await.is_err() {
                break;
            }
        }
    });

    // When either direction closes, abort the other
    tokio::select! {
        _ = b2a => {},
        _ = a2b => {},
    }
}

/// GET /ws/chat — WebSocket upgrade for agent chat
pub async fn handle_ws_chat(
    State(state): State<AppState>,
    Query(params): Query<WsQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Auth: check header, subprotocol, then query param (precedence order)
    let raw_token = extract_ws_token(&headers, params.token.as_deref()).unwrap_or("");

    if state.pairing.require_pairing() && !state.pairing.is_authenticated(raw_token) {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "Unauthorized — provide Authorization header, Sec-WebSocket-Protocol bearer, or ?token= query param",
        )
            .into_response();
    }

    // Echo Sec-WebSocket-Protocol if the client requests our sub-protocol.
    let ws = if headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |protos| {
            protos.split(',').any(|p| p.trim() == WS_PROTOCOL)
        }) {
        ws.protocols([WS_PROTOCOL])
    } else {
        ws
    };

    let token_prefix = token_hash_prefix(raw_token);
    let session_id = params.session_id.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, state, token_prefix, session_id))
        .into_response()
}

async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    token_prefix: String,
    session_id: Option<String>,
) {
    let (sender, mut receiver) = socket.split();

    // Derive session key.
    // When broker proxies a connection it sets session_id=op:<operator_hash>.
    // We fold the operator prefix into token_prefix so that ALL session CRUD
    // (sessions.list, sessions.new, etc.) is scoped per-operator, not just
    // the default session.  Direct browser connections are unaffected.
    let sid = session_id.unwrap_or_else(|| "default".to_string());
    // Operator-proxied sessions keep token scoping for isolation.
    // Direct browser sessions drop token_prefix so re-pairing doesn't
    // orphan existing sessions (single-user deployment).
    let (token_prefix, session_key) = if let Some(op) = sid.strip_prefix("op:") {
        let tp = format!("{token_prefix}:op:{op}");
        let key = format!("web:{tp}:{sid}");
        (tp, key)
    } else {
        let key = format!("web:{sid}");
        (token_prefix, key)
    };

    // Ensure session exists in memory (create agent if needed)
    if let Err(e) = ensure_session(&state, &session_key, &token_prefix).await {
        let mut sender = sender;
        let err = serde_json::json!({"type": "error", "message": format!("Failed to initialise session: {e}")});
        let _ = sender.send(Message::Text(err.to_string().into())).await;
        return;
    }

    // Outbound channel: allows spawned tasks to send WS frames without blocking the reader.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Subscribe to broadcast for server-push session events (multi-tab freshness)
    let mut event_rx = state.event_tx.subscribe();
    let session_prefix = format!("web:{token_prefix}:");

    // Writer task: drains out_tx + broadcast events → WS sender
    let writer = tokio::spawn(async move {
        let mut sender = sender;
        loop {
            tokio::select! {
                msg = out_rx.recv() => {
                    match msg {
                        Some(m) => {
                            if sender.send(Message::Text(m.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break, // out_tx dropped
                    }
                }
                evt = event_rx.recv() => {
                    if let Ok(evt) = evt {
                        // Forward session.* events that belong to this token's namespace
                        let evt_type = evt["type"].as_str().unwrap_or("");
                        if evt_type.starts_with("session.") {
                            let evt_key = evt["session_key"].as_str().unwrap_or("");
                            if evt_key.starts_with(&session_prefix) {
                                let _ = sender.send(Message::Text(evt.to_string().into())).await;
                            }
                        }
                    }
                }
            }
        }
    });

    // Reader loop: never blocks on long-running operations.
    // chat.send is spawned as a separate task so abort can be processed concurrently.
    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        let parsed: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => {
                let _ = out_tx.send(
                    serde_json::json!({"type": "error", "message": "Invalid JSON"}).to_string(),
                );
                continue;
            }
        };

        let msg_type = parsed["type"].as_str().unwrap_or("");

        if msg_type != "rpc" {
            continue;
        }

        let id = parsed["id"].as_str().unwrap_or("").to_string();
        let method = parsed["method"].as_str().unwrap_or("").to_string();
        let params = parsed["params"].clone();

        if method == "chat.send" {
            // Spawn long-running send so reader loop stays free for abort
            let tx = out_tx.clone();
            let st = state.clone();
            let sk = session_key.clone();
            let tp = token_prefix.clone();
            tokio::spawn(async move {
                let result = handle_chat_send_rpc(&params, &st, &sk, &tp, &tx).await;
                let response = match result {
                    Ok(val) => serde_json::json!({
                        "type": "rpc_response", "id": id, "result": val,
                    }),
                    Err(e) => serde_json::json!({
                        "type": "rpc_response", "id": id, "error": e.to_string(),
                    }),
                };
                let _ = tx.send(response.to_string());
            });
        } else {
            // Fast RPCs: handle inline
            let result = handle_rpc(
                &method,
                &params,
                &state,
                &token_prefix,
                &session_key,
                &out_tx,
            )
            .await;
            let response = match result {
                Ok(val) => serde_json::json!({
                    "type": "rpc_response", "id": id, "result": val,
                }),
                Err(e) => serde_json::json!({
                    "type": "rpc_response", "id": id, "error": e.to_string(),
                }),
            };
            let _ = out_tx.send(response.to_string());
        }
    }

    // Shutdown: drop out_tx so writer task exits
    drop(out_tx);
    let _ = writer.await;
    // WS disconnect: agent stays alive in AppState (not dropped)
}

/// Ensure a session exists in memory. If not, resume it from the shared conversation store or create fresh.
async fn ensure_session(
    state: &AppState,
    session_key: &str,
    token_prefix: &str,
) -> anyhow::Result<()> {
    {
        let sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if sessions.contains_key(session_key) {
            return Ok(());
        }
    }

    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("web conversation_store is required"))?;
    let runtime_conversation_key =
        canonical_web_conversation_key_for_state(state, session_key, token_prefix, None)?;
    let now = Instant::now();
    let mut web_runtime_history = Vec::<synapse_providers::ChatMessage>::new();

    let (
        label,
        msg_count,
        input_tok,
        output_tok,
        current_goal,
        session_summary,
        last_summary_count,
    ) = match synapse_domain::application::use_cases::resume_conversation::execute(
        store.as_ref(),
        session_key,
    )
    .await
    {
        Ok(resumed) => {
            web_runtime_history.extend(resumed.transcript.iter().map(|turn| {
                synapse_providers::ChatMessage {
                    role: turn.role.clone(),
                    content: turn.content.clone(),
                }
            }));
            let last_summary_count = if resumed.session.summary.is_some() {
                resumed.session.message_count
            } else {
                0
            };
            (
                resumed.session.label,
                resumed.session.message_count,
                resumed.session.input_tokens,
                resumed.session.output_tokens,
                resumed.session.current_goal,
                resumed.session.summary,
                last_summary_count,
            )
        }
        Err(_) => (None, 0, 0, 0, None, None, 0),
    };

    if synapse_domain::application::services::history_compaction::compact_provider_history_for_session_hygiene(
        &mut web_runtime_history,
        synapse_domain::application::services::history_compaction::SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
    ) {
        tracing::info!(
            session_key = %session_key,
            "Compacted resumed web session before runtime execution"
        );
    }

    let session = ChatSession {
        created_at: now,
        last_active: now,
        label,
        message_count: msg_count,
        current_goal,
        session_summary,
        input_tokens: input_tok,
        output_tokens: output_tok,
        run_id: None,
        abort_tx: None,
        last_summary_count,
    };

    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // LRU eviction if at capacity
        if sessions.len() >= MAX_MEMORY_SESSIONS && !sessions.contains_key(session_key) {
            let oldest = sessions
                .iter()
                .min_by_key(|(_, s)| s.last_active)
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest {
                sessions.remove(&key);
                let runtime_scope_prefix =
                    canonical_web_conversation_scope_prefix_for_state(state, &key)?;
                if let Ok(mut histories) = state.web_conversation_histories.lock() {
                    histories
                        .retain(|runtime_key, _| !runtime_key.starts_with(&runtime_scope_prefix));
                }
                if let Ok(mut routes) = state.web_route_overrides.lock() {
                    routes.retain(|runtime_key, _| !runtime_key.starts_with(&runtime_scope_prefix));
                }
            }
        }

        sessions.insert(session_key.to_string(), session);
    }; // MutexGuard dropped here

    if !web_runtime_history.is_empty() {
        state
            .web_conversation_histories
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .insert(runtime_conversation_key, web_runtime_history);
    }

    // Don't persist empty sessions on WS connect — only persist when
    // the first message is sent (in handle_chat_send_rpc).
    // This prevents phantom empty sessions from appearing on every page visit.

    Ok(())
}

// ── RPC dispatcher ──────────────────────────────────────────────────────────

async fn handle_rpc(
    method: &str,
    params: &serde_json::Value,
    state: &AppState,
    token_prefix: &str,
    default_session: &str,
    out_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<serde_json::Value> {
    match method {
        "chat.history" => handle_chat_history(params, state, default_session, token_prefix).await,
        "chat.send" => {
            handle_chat_send_rpc(params, state, default_session, token_prefix, out_tx).await
        }
        "chat.abort" => handle_chat_abort(params, state, default_session, token_prefix),
        "sessions.list" => handle_sessions_list(state, token_prefix).await,
        "sessions.new" => handle_sessions_new(params, state, token_prefix).await,
        "sessions.rename" => handle_sessions_rename(params, state, token_prefix).await,
        "sessions.delete" => handle_sessions_delete(params, state, token_prefix).await,
        "sessions.reset" => handle_sessions_reset(params, state, token_prefix).await,
        _ => Err(anyhow::anyhow!("Unknown RPC method: {method}")),
    }
}

/// Verify that a session key belongs to the current token's namespace.
/// Direct browser sessions use `web:{sid}` format (no token scoping).
/// Operator-proxied sessions use `web:{token_prefix}:op:{op}:{sid}`.
fn check_session_ownership(session_key: &str, _token_prefix: &str) -> anyhow::Result<()> {
    // Single-user deployment: any authenticated connection can access any session.
    // Session isolation is only meaningful for multi-tenant setups (not supported).
    let _ = session_key;
    Ok(())
}

// ── RPC: chat.history ───────────────────────────────────────────────────────

async fn handle_chat_history(
    params: &serde_json::Value,
    state: &AppState,
    default_session: &str,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_key = params["session"]
        .as_str()
        .unwrap_or(default_session)
        .to_string();
    check_session_ownership(&session_key, token_prefix)?;
    let limit = params["limit"].as_i64().unwrap_or(50).min(500);

    // Channel sessions live in a different table — load directly from SurrealDB.
    if !session_key.starts_with("web:") {
        return handle_channel_history(state, &session_key, limit).await;
    }

    // Ensure web session is loaded
    ensure_session(state, &session_key, token_prefix).await?;

    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("web conversation_store is required"))?;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let events = store.get_events(&session_key, limit as usize).await;

    let (label, session_summary, current_goal) = {
        let sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sessions
            .get(&session_key)
            .map(|s| {
                (
                    s.label.clone(),
                    s.session_summary.clone(),
                    s.current_goal.clone(),
                )
            })
            .unwrap_or((None, None, None))
    };

    let msg_json: Vec<serde_json::Value> = events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            serde_json::json!({
                "id": i + 1,
                "event_type": e.event_type.to_string(),
                "role": e.actor,
                "content": e.content,
                "tool_name": e.tool_name,
                "run_id": e.run_id,
                "timestamp": e.timestamp,
                "input_tokens": e.input_tokens,
                "output_tokens": e.output_tokens,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "messages": msg_json,
        "session_key": session_key,
        "label": label,
        "session_summary": session_summary,
        "current_goal": current_goal,
    }))
}

/// Load channel session messages (Matrix, Telegram, etc.) from channel_session table.
async fn handle_channel_history(
    state: &AppState,
    session_key: &str,
    limit: i64,
) -> anyhow::Result<serde_json::Value> {
    let backend = state
        .channel_session_backend
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("channel session backend is required"))?;

    let messages = backend.load(session_key).await;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let limit = limit.max(0) as usize;
    let start = if limit == 0 {
        messages.len()
    } else {
        messages.len().saturating_sub(limit)
    };
    let messages: Vec<serde_json::Value> = messages[start..]
        .iter()
        .enumerate()
        .map(|(i, message)| {
            let event_type = match message.role.as_str() {
                "assistant" => "assistant",
                "tool" => "tool_result",
                "system" => "system",
                _ => "user",
            };
            serde_json::json!({
                "id": i + 1,
                "event_type": event_type,
                "role": message.role,
                "content": message.content,
                "tool_name": null,
                "run_id": null,
                "timestamp": 0,
                "input_tokens": null,
                "output_tokens": null,
            })
        })
        .collect();

    let summary = backend
        .load_summary(session_key)
        .await
        .map(|summary| summary.summary);
    let metadata = backend
        .list_sessions_with_metadata()
        .await
        .into_iter()
        .find(|session| session.key == session_key);
    let (channel, sender) = session_key
        .split_once('_')
        .unwrap_or(("channel", session_key));

    Ok(serde_json::json!({
        "messages": messages,
        "session_key": session_key,
        "channel": channel,
        "sender": sender,
        "label": metadata.as_ref().and_then(|session| session.label.clone()),
        "session_summary": summary,
        "current_goal": metadata.as_ref().and_then(|session| session.current_goal.clone()),
        "input_tokens": metadata.as_ref().map(|session| session.input_tokens),
        "output_tokens": metadata.as_ref().map(|session| session.output_tokens),
    }))
}

// ── RPC: chat.send ──────────────────────────────────────────────────────────

async fn handle_chat_send_rpc(
    params: &serde_json::Value,
    state: &AppState,
    default_session: &str,
    token_prefix: &str,
    out_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<serde_json::Value> {
    let session_key = params["session"]
        .as_str()
        .unwrap_or(default_session)
        .to_string();
    check_session_ownership(&session_key, token_prefix)?;
    ensure_session(state, &session_key, token_prefix).await?;
    let message = params["message"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'message' param"))?
        .to_string();
    let thread_ref = web_thread_ref(params);
    let media_attachments = web_media_attachments(params)?;
    let provider_facing_message =
        synapse_domain::application::services::inbound_message_service::provider_facing_content(
            &message,
            &media_attachments,
        );

    // Phase 4.0 Slice 3: run lifecycle via conversation_service
    let run_id = if let Some(store) = state.run_store.as_ref() {
        synapse_domain::application::use_cases::start_conversation_run::create_and_track_run(
            state
                .conversation_store
                .as_deref()
                .expect("conversation_store required"),
            store.as_ref(),
            &session_key,
        )
        .await
        .map_err(|e| anyhow::anyhow!("run_store: failed to create run: {e}"))?
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    // Create abort channel and store run_id
    let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(&session_key) {
            s.run_id = Some(run_id.clone());
            s.abort_tx = Some(abort_tx);
            s.last_active = Instant::now();
        }
    }

    // Emit run_started lifecycle event
    emit_run_event(state, "session.run_started", &session_key, &run_id);

    // Ensure session is persisted to DB on first message (lazy creation).
    if let Some(store) = state.conversation_store.as_ref() {
        if store.get_session(&session_key).await.is_none() {
            let session =
                synapse_domain::application::services::conversation_service::new_web_session(
                    &session_key,
                    None,
                );
            let _ = store.upsert_session(&session).await;
        }
    }

    // Persist user message + auto-label
    persist_message(
        state,
        &session_key,
        "user",
        Some("user"),
        &provider_facing_message,
        None,
        None,
    )
    .await;
    auto_label_if_needed(state, &session_key, &message).await;

    // Run through the same inbound use case used by channels; WebSocket is only transport.
    let result = run_web_inbound_turn_with_abort(
        state,
        &session_key,
        token_prefix,
        &message,
        thread_ref.clone(),
        media_attachments,
        abort_rx,
        out_tx,
    )
    .await;

    // Clear run_id + abort_tx. Usage/tool learning is owned by the shared inbound runtime path.
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(&session_key) {
            s.run_id = None;
            s.abort_tx = None;
            s.last_active = Instant::now();
        }
    }
    match result {
        Ok(response) => {
            // Push assistant message via the same out_tx channel as tool events.
            // This guarantees FIFO order: tool_call → tool_result → assistant.
            // The RPC response only carries metadata (run_id), not the message.
            let _ = out_tx.send(web_presented_output_json(&response, &session_key).to_string());
            let state_bg = state.clone();
            let session_key_bg = session_key.clone();
            let run_id_bg = run_id.clone();
            let message_bg = message.clone();
            let response_bg = response.text.clone();
            tokio::spawn(async move {
                persist_message(
                    &state_bg,
                    &session_key_bg,
                    "assistant",
                    Some("assistant"),
                    &response_bg,
                    None,
                    None,
                )
                .await;
                if let (Some(cs), Some(rs)) = (
                    state_bg.conversation_store.as_ref(),
                    state_bg.run_store.as_ref(),
                ) {
                    let _ = synapse_domain::application::use_cases::start_conversation_run::finalize_success(
                        cs.as_ref(), rs.as_ref(), &session_key_bg, &run_id_bg, 0, 0,
                    ).await;
                }
                sync_memory_count(&state_bg, &session_key_bg, 2);
                persist_usage_memory(&state_bg, &session_key_bg, None);
                update_session_goal(&state_bg, &session_key_bg, &message_bg).await;
                emit_session_event(&state_bg, "session.updated", &session_key_bg);
                emit_run_event(
                    &state_bg,
                    "session.run_finished",
                    &session_key_bg,
                    &run_id_bg,
                );
                summarize_session_if_needed(&state_bg, &session_key_bg).await;
            });

            // RPC response: metadata only (assistant message already pushed above)
            Ok(serde_json::json!({
                "run_id": run_id,
            }))
        }
        Err(e) => {
            let msg = e.to_string();
            if msg == "aborted" {
                persist_message(
                    state,
                    &session_key,
                    "interrupted",
                    None,
                    "Generation aborted by user",
                    None,
                    Some(&run_id),
                )
                .await;
                // Phase 4.0 Slice 3: finalize interrupted
                if let (Some(cs), Some(rs)) =
                    (state.conversation_store.as_ref(), state.run_store.as_ref())
                {
                    let _ = synapse_domain::application::use_cases::start_conversation_run::finalize_interrupted(
                        rs.as_ref(), cs.as_ref(), &session_key, &run_id,
                    ).await;
                }
                sync_memory_count(state, &session_key, 2);
                emit_run_event(state, "session.run_interrupted", &session_key, &run_id);
                return Ok(serde_json::json!({
                    "run_id": run_id,
                    "aborted": true,
                }));
            }
            let runtime_error = classify_agent_runtime_error(e);
            if matches!(
                runtime_error.kind,
                AgentRuntimeErrorKind::ContextLimitExceeded
            ) {
                let runtime_conversation_key = canonical_web_conversation_key_for_state(
                    state,
                    &session_key,
                    token_prefix,
                    thread_ref.clone(),
                )
                .unwrap_or_else(|_| session_key.clone());
                let compacted =
                    compact_web_session_after_context_limit(state, &runtime_conversation_key)
                        .await
                        .unwrap_or_else(|error| {
                            tracing::debug!(
                                session_key = %session_key,
                                error = %error,
                                "Web session context-limit recovery compaction failed"
                            );
                            false
                        });
                let recovery_message = format_context_limit_recovery_response(compacted);
                persist_message(
                    state,
                    &session_key,
                    "assistant",
                    Some("assistant"),
                    recovery_message,
                    None,
                    Some(&run_id),
                )
                .await;
                if let (Some(cs), Some(rs)) =
                    (state.conversation_store.as_ref(), state.run_store.as_ref())
                {
                    let _ =
                        synapse_domain::application::use_cases::start_conversation_run::finalize_failure(
                            rs.as_ref(),
                            cs.as_ref(),
                            &session_key,
                            &run_id,
                        )
                        .await;
                }
                sync_memory_count(state, &session_key, 2);
                emit_run_event(state, "session.run_finished", &session_key, &run_id);
                let _ = out_tx.send(
                    serde_json::json!({
                        "type": "assistant",
                        "session_key": session_key,
                        "content": recovery_message,
                        "timestamp": now_secs(),
                    })
                    .to_string(),
                );
                return Ok(serde_json::json!({
                    "run_id": run_id,
                    "runtime_error_kind": "context_limit_exceeded",
                    "compacted": compacted,
                }));
            }

            let sanitized = synapse_providers::sanitize_api_error(&runtime_error.to_string());
            persist_message(state, &session_key, "error", None, &sanitized, None, None).await;
            // Phase 4.0 Slice 3: finalize failed
            if let (Some(cs), Some(rs)) =
                (state.conversation_store.as_ref(), state.run_store.as_ref())
            {
                let _ =
                    synapse_domain::application::use_cases::start_conversation_run::finalize_failure(
                        rs.as_ref(),
                        cs.as_ref(),
                        &session_key,
                        &run_id,
                    )
                    .await;
            }
            sync_memory_count(state, &session_key, 2);
            emit_run_event(state, "session.run_finished", &session_key, &run_id);
            Err(anyhow::anyhow!("{sanitized}"))
        }
    }
}

fn build_web_system_prompt(
    state: &AppState,
    config: &synapse_domain::config::schema::Config,
) -> anyhow::Result<String> {
    let tool_descs: Vec<(&str, &str)> = state
        .tools_registry
        .iter()
        .map(|spec| (spec.name.as_str(), spec.description.as_str()))
        .collect();
    let skills = crate::skills::load_skills_with_config(&config.workspace_dir, config);
    let bootstrap_max_chars = config.agent.compact_context.then_some(6000);
    let native_tools = state.provider.supports_native_tools();
    if !native_tools && !state.runtime_tools_registry.is_empty() {
        anyhow::bail!(
            "provider {} does not support native tool calling; prompt-guided tool fallback has been removed",
            default_web_provider(config)
        );
    }

    crate::runtime_system_prompt::build_system_prompt_with_mode(
        &config.workspace_dir,
        default_web_model(state, config).as_str(),
        &tool_descs,
        &skills,
        Some(&config.identity),
        bootstrap_max_chars,
        native_tools,
        config.skills.prompt_injection_mode,
    )
}

fn build_web_inbound_config(
    state: &AppState,
    config: &synapse_domain::config::schema::Config,
) -> anyhow::Result<web_inbound::InboundMessageConfig> {
    Ok(crate::inbound_runtime_config::InboundRuntimeConfigFactory::build(
        crate::inbound_runtime_config::InboundRuntimeConfigInput {
            system_prompt: build_web_system_prompt(state, config)?,
            default_provider: default_web_provider(config),
            default_model: default_web_model(state, config),
            temperature: state.temperature,
            max_tool_iterations: config.agent.max_tool_iterations,
            auto_save_memory: state.auto_save,
            model_lanes: config.model_lanes.clone(),
            model_preset: config.model_preset.clone(),
            query_classification: config.query_classification.clone(),
            message_timeout_secs: config.channels_config.message_timeout_secs,
            min_relevance_score: config.memory.min_relevance_score,
            ack_reactions: false,
            agent_id: state.agent_id.clone(),
            prompt_budget_config: config.memory.prompt_budget.clone(),
            presentation_mode: synapse_domain::application::services::channel_presentation::ChannelPresentationMode::from_show_tool_calls(true),
        },
    ))
}

fn build_web_agent_runtime(
    state: &AppState,
    config: &synapse_domain::config::schema::Config,
    observer: Arc<dyn synapse_observability::Observer>,
) -> ChannelAgentRuntime {
    let default_provider = default_web_provider(config);
    crate::agent_runtime_factory::ChannelAgentRuntimeFactory::build(
        crate::agent_runtime_factory::ChannelAgentRuntimeInput {
            provider: Arc::clone(&state.provider),
            default_provider_name: default_provider,
            default_api_key: config.api_key.clone(),
            default_api_url: config.api_url.clone(),
            provider_cache: Arc::clone(&state.provider_cache),
            reliability: config.reliability.clone(),
            provider_runtime_options: synapse_providers::provider_runtime_options_from_config(
                config,
            ),
            workspace_dir: config.workspace_dir.clone(),
            tools_registry: Arc::clone(&state.runtime_tools_registry),
            observer,
            approval_manager: Arc::new(ApprovalManager::for_non_interactive(&config.autonomy)),
            channel_name: "web".to_string(),
            multimodal: config.multimodal.clone(),
            excluded_tools: Arc::new(config.autonomy.non_cli_excluded_tools.clone()),
            dedup_exempt_tools: Arc::new(config.agent.tool_call_dedup_exempt.clone()),
            hooks: None,
            activated_tools: state.runtime_mcp_activated_tools.clone(),
            message_timeout_secs: config.channels_config.message_timeout_secs,
            max_tool_iterations: config.agent.max_tool_iterations,
        },
    )
}

fn web_presented_output_json(output: &PresentedOutput, session_key: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "session_key": session_key,
        "content": output.text,
        "media_artifacts": output.media_artifacts,
        "thread_ref": output.delivery_hints.thread_ref,
        "timestamp": now_secs(),
    })
}

fn web_thread_ref(params: &serde_json::Value) -> Option<String> {
    ["thread_ref", "thread_id", "reply_to"]
        .iter()
        .find_map(|key| params.get(*key).and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn web_media_attachments(
    params: &serde_json::Value,
) -> anyhow::Result<Vec<InboundMediaAttachment>> {
    let Some(items) = params
        .get("media_attachments")
        .or_else(|| params.get("attachments"))
    else {
        return Ok(Vec::new());
    };
    let items = items
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("media_attachments must be an array"))?;
    items
        .iter()
        .map(web_media_attachment)
        .collect::<anyhow::Result<Vec<_>>>()
}

fn web_media_attachment(value: &serde_json::Value) -> anyhow::Result<InboundMediaAttachment> {
    let kind = value
        .get("kind")
        .or_else(|| value.get("type"))
        .and_then(|value| value.as_str())
        .map(web_media_kind)
        .transpose()?
        .unwrap_or(InboundMediaKind::File);
    let uri = value
        .get("uri")
        .or_else(|| value.get("url"))
        .or_else(|| value.get("path"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("media attachment requires uri/url/path"))?;
    Ok(InboundMediaAttachment {
        kind,
        uri: uri.to_string(),
        mime_type: value
            .get("mime_type")
            .or_else(|| value.get("mime"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        label: value
            .get("label")
            .or_else(|| value.get("name"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn web_media_kind(value: &str) -> anyhow::Result<InboundMediaKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "image" => Ok(InboundMediaKind::Image),
        "audio" | "voice" | "music" | "song" => Ok(InboundMediaKind::Audio),
        "video" => Ok(InboundMediaKind::Video),
        "file" | "document" | "attachment" => Ok(InboundMediaKind::File),
        other => Err(anyhow::anyhow!(
            "unsupported media attachment kind: {other}"
        )),
    }
}

pub async fn handle_api_chat_media_upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    match store_web_chat_media(&state, &mut multipart).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn store_web_chat_media(
    state: &AppState,
    multipart: &mut Multipart,
) -> anyhow::Result<serde_json::Value> {
    let workspace_dir = state.config.lock().workspace_dir.clone();
    let upload_dir = workspace_dir.join("web-media").join("chat");
    tokio::fs::create_dir_all(&upload_dir).await?;

    let mut requested_kind: Option<InboundMediaKind> = None;
    while let Some(field) = multipart.next_field().await? {
        let field_name = field.name().unwrap_or("").to_string();
        if field_name == "kind" || field_name == "type" {
            requested_kind = Some(web_media_kind(field.text().await?.trim())?);
            continue;
        }
        if field_name != "file" && field.file_name().is_none() {
            continue;
        }

        let original_name = sanitize_web_media_filename(field.file_name().unwrap_or("upload.bin"));
        let mime_type = field.content_type().map(str::to_string);
        let bytes = field.bytes().await?;
        if bytes.len() > MAX_WEB_MEDIA_UPLOAD_BYTES {
            anyhow::bail!(
                "media upload too large: {} bytes > {} bytes",
                bytes.len(),
                MAX_WEB_MEDIA_UPLOAD_BYTES
            );
        }
        let kind =
            requested_kind.unwrap_or_else(|| infer_web_media_kind(&original_name, &mime_type));
        let stored_name = format!("{}-{original_name}", uuid::Uuid::new_v4());
        let path = upload_dir.join(stored_name);
        tokio::fs::write(&path, &bytes).await?;
        let uri = path.to_string_lossy().to_string();
        let attachment = serde_json::json!({
            "kind": web_media_kind_label(kind),
            "uri": uri,
            "mime_type": mime_type,
            "label": original_name,
        });
        return Ok(serde_json::json!({
            "attachment": attachment,
            "media_attachments": [attachment],
        }));
    }

    anyhow::bail!("multipart upload requires a file field")
}

fn sanitize_web_media_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('.');
    if sanitized.is_empty() {
        "upload.bin".to_string()
    } else {
        sanitized.to_string()
    }
}

fn infer_web_media_kind(file_name: &str, mime_type: &Option<String>) -> InboundMediaKind {
    if let Some(mime) = mime_type.as_deref() {
        if mime.starts_with("image/") {
            return InboundMediaKind::Image;
        }
        if mime.starts_with("audio/") {
            return InboundMediaKind::Audio;
        }
        if mime.starts_with("video/") {
            return InboundMediaKind::Video;
        }
    }
    match file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "webp" | "gif") => InboundMediaKind::Image,
        Some("mp3" | "wav" | "ogg" | "oga" | "opus" | "m4a" | "flac") => InboundMediaKind::Audio,
        Some("mp4" | "mov" | "mkv" | "webm") => InboundMediaKind::Video,
        _ => InboundMediaKind::File,
    }
}

fn web_media_kind_label(kind: InboundMediaKind) -> &'static str {
    match kind {
        InboundMediaKind::Image => "image",
        InboundMediaKind::Audio => "audio",
        InboundMediaKind::Video => "video",
        InboundMediaKind::File => "file",
    }
}

async fn run_web_inbound_turn_with_abort(
    state: &AppState,
    session_key: &str,
    token_prefix: &str,
    message: &str,
    thread_ref: Option<String>,
    media_attachments: Vec<InboundMediaAttachment>,
    mut abort_rx: tokio::sync::watch::Receiver<bool>,
    out_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<PresentedOutput> {
    let config_snapshot = state.config.lock().clone();
    let inbound_config = build_web_inbound_config(state, &config_snapshot)?;
    let runtime_conversation_key = canonical_web_conversation_key(
        &inbound_config.agent_id,
        session_key,
        token_prefix,
        thread_ref.clone(),
    );
    let default_provider = inbound_config.default_provider.clone();
    let observer_for_runtime: Arc<dyn synapse_observability::Observer> =
        Arc::new(RuntimeToolNotifyObserver::new(
            Arc::clone(&state.observer),
            WsToolNotificationHandler {
                tx: out_tx.clone(),
                session_key: session_key.to_string(),
                seen_tool_calls: std::sync::Mutex::new(std::collections::HashSet::new()),
                seen_tool_results: std::sync::Mutex::new(std::collections::HashSet::new()),
            },
            "web-runtime-tool-notify",
        ));
    let history = crate::inbound_runtime_ports::InboundRuntimeStoreFactory::history(
        Arc::clone(&state.web_conversation_histories),
        None,
    );
    let routes = crate::inbound_runtime_ports::InboundRuntimeStoreFactory::routes(
        Arc::clone(&state.web_route_overrides),
        inbound_config.default_provider.clone(),
        inbound_config.default_model.clone(),
    );
    let hooks: Arc<dyn synapse_domain::ports::hooks::HooksPort> = Arc::new(NoOpHooks);
    let channel_output = Arc::new(WebChannelOutput {
        tx: out_tx.clone(),
        session_key: session_key.to_string(),
    });
    let agent_runtime: Arc<dyn synapse_domain::ports::agent_runtime::AgentRuntimePort> = Arc::new(
        build_web_agent_runtime(state, &config_snapshot, observer_for_runtime),
    );
    let channel_registry: Arc<dyn ChannelRegistryPort> = state
        .channel_registry
        .clone()
        .ok_or_else(|| anyhow::anyhow!("web channel_registry is required"))?;
    let channel_conversation_store =
        crate::inbound_runtime_ports::InboundRuntimeStoreFactory::conversation_store(
            state.channel_session_backend.clone(),
        );
    let retrieval_conversation_store =
        crate::inbound_runtime_ports::InboundRuntimeStoreFactory::composite_conversation_store(
            vec![state.conversation_store.clone(), channel_conversation_store],
        );
    let ports = crate::inbound_runtime_ports::InboundRuntimePortsFactory::build(crate::inbound_runtime_ports::InboundRuntimePortsInput {
        history,
        routes,
        hooks,
        channel_output,
        agent_runtime,
        channel_registry: Arc::clone(&channel_registry),
        session_summary: crate::inbound_runtime_ports::InboundRuntimeStoreFactory::conversation_summary_for_key(
            state.conversation_store.clone(),
            session_key.to_string(),
        ),
        memory: Some(Arc::clone(&state.mem)),
        event_tx: Some(state.event_tx.clone()),
        conversation_context: Some(Arc::clone(&state.conversation_context)),
        model_profile_catalog: Some(Arc::new(
            WorkspaceModelProfileCatalog::with_provider_endpoint(
                config_snapshot.workspace_dir.clone(),
                Some(default_provider.as_str()),
                config_snapshot.api_url.as_deref(),
            ),
        )),
        turn_defaults_context: Some(Arc::clone(&state.turn_defaults_context)),
        scoped_instruction_context: Some(Arc::clone(&state.scoped_instruction_context)),
        conversation_store: retrieval_conversation_store,
        dialogue_state_store: Some(Arc::clone(&state.dialogue_state_store)),
        run_recipe_store: Some(Arc::clone(&state.run_recipe_store)),
        user_profile_store: Some(Arc::clone(&state.user_profile_store)),
    });
    let envelope = InboundEnvelope {
        source_kind: SourceKind::Web,
        source_adapter: "web".to_string(),
        actor_id: format!("web:{token_prefix}"),
        conversation_id: session_key.to_string(),
        event_ref: None,
        reply_ref: session_key.to_string(),
        thread_ref: thread_ref.clone(),
        media_attachments,
        content: message.to_string(),
        received_at: now_secs() as u64,
    };
    let caps = channel_registry.capabilities("web");
    if let Some(output) = crate::message_routing_service::route_explicit_message(
        &envelope,
        crate::message_routing_service::MessageRoutingPorts {
            router: state.message_router.clone(),
            pipeline_store: state.pipeline_store.clone(),
            pipeline_executor: state.pipeline_executor.clone(),
            run_store: state.run_store.clone(),
            dead_letter: state.dead_letter.clone(),
        },
        synapse_domain::application::services::assistant_output_presentation::OutputDeliveryHints {
            reply_ref: Some(session_key.to_string()),
            thread_ref: thread_ref.clone(),
            already_delivered: false,
        },
    )
    .await
    {
        return Ok(output);
    }

    let handled = tokio::select! {
        biased;
        _ = abort_rx.wait_for(|v| *v) => {
            return Err(anyhow::anyhow!("aborted"));
        }
        result = web_inbound::handle(&envelope, &caps, &inbound_config, &ports) => result?,
    };

    match handled {
        HandleResult::Response { output, .. } => Ok(output),
        HandleResult::Cancelled { reason } => Err(anyhow::anyhow!(reason)),
        HandleResult::Command { effect, .. } => {
            let adapter_contract = WebRuntimeAdapterContract;
            let mut command_host = WebRuntimeCommandHost {
                state,
                ui_session_key: session_key,
                conversation_key: &runtime_conversation_key,
                config: &config_snapshot,
                default_provider: default_provider.as_str(),
                token_prefix,
            };
            execute_runtime_command_output(
                &adapter_contract,
                &mut command_host,
                &effect,
                default_provider.as_str(),
                synapse_domain::application::services::assistant_output_presentation::OutputDeliveryHints {
                    reply_ref: Some(session_key.to_string()),
                    thread_ref: thread_ref.clone(),
                    already_delivered: false,
                },
            )
            .await
        }
        HandleResult::CommandNoChannel => {
            Err(anyhow::anyhow!("runtime command channel unavailable"))
        }
    }
}

fn current_web_route_selection(
    state: &AppState,
    session_key: &str,
) -> anyhow::Result<RouteSelection> {
    if let Some(route) = state
        .web_route_overrides
        .lock()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .get(session_key)
        .cloned()
    {
        return Ok(route);
    }
    let config_snapshot = state.config.lock().clone();
    Ok(default_web_route_selection(state, &config_snapshot))
}

struct WebRuntimeCommandHost<'a> {
    state: &'a AppState,
    ui_session_key: &'a str,
    conversation_key: &'a str,
    config: &'a synapse_domain::config::schema::Config,
    default_provider: &'a str,
    token_prefix: &'a str,
}

#[async_trait::async_trait]
impl RuntimeCommandHost for WebRuntimeCommandHost<'_> {
    fn current_provider(&self) -> String {
        current_web_route_selection(self.state, self.conversation_key)
            .ok()
            .map(|route| route.provider)
            .unwrap_or_else(|| self.default_provider.to_string())
    }

    async fn provider_help_route(&mut self) -> anyhow::Result<RouteSelection> {
        current_web_route_selection(self.state, self.conversation_key)
    }

    async fn model_help_snapshot(&mut self) -> anyhow::Result<RuntimeModelHelpSnapshot> {
        let mut route = current_web_route_selection(self.state, self.conversation_key)?;
        if route.context_cache.is_none() {
            route.context_cache = Some(
                web_route_effective_context_cache_stats(self.state, self.config, &route).await,
            );
        }
        Ok(RuntimeModelHelpSnapshot {
            route,
            config: self.config.clone(),
        })
    }

    async fn capability_doctor_report(
        &mut self,
    ) -> anyhow::Result<
        synapse_domain::application::services::capability_doctor::CapabilityDoctorReport,
    > {
        let route = current_web_route_selection(self.state, self.conversation_key)?;
        let provider_capabilities =
            web_provider_capabilities_for_route(self.state, self.config, &route);
        let memory_backend_healthy = Some(self.state.mem.health_check().await);
        let embedding_profile = self.state.mem.embedding_profile();
        Ok(
            crate::runtime_routes::build_runtime_capability_doctor_report(
                RuntimeCapabilityDoctorInput {
                    route: &route,
                    config: self.config,
                    provider_capabilities,
                    provider_plan_denial: None,
                    tool_registry_count: self.state.runtime_tools_registry.len(),
                    memory_backend_name: Some(self.state.mem.name()),
                    memory_backend_healthy,
                    memory_backend_configured: true,
                    embedding_profile: Some(&embedding_profile),
                    channel_name: Some("web"),
                    channel_available: Some(true),
                },
            ),
        )
    }

    async fn switch_provider(
        &mut self,
        request: RuntimeRouteMutationRequest,
    ) -> anyhow::Result<RuntimeProviderSwitchOutcome> {
        let provider = request
            .provider
            .ok_or_else(|| anyhow::anyhow!("provider route mutation request missing provider"))?;
        apply_web_runtime_route(
            self.state,
            self.ui_session_key,
            self.conversation_key,
            self.config,
            Some(provider.as_str()),
            None,
            None,
            None,
            None,
            self.token_prefix,
        )
        .await
        .map(|_| RuntimeProviderSwitchOutcome {
            provider,
            already_current: false,
        })
    }

    async fn switch_model(
        &mut self,
        request: RuntimeRouteMutationRequest,
        _compacted: bool,
    ) -> anyhow::Result<RuntimeModelSwitchOutcome> {
        let provider = request
            .provider
            .clone()
            .unwrap_or_else(|| self.current_provider());
        let model = request
            .model
            .clone()
            .ok_or_else(|| anyhow::anyhow!("runtime route mutation request missing model"))?;
        let catalog = WorkspaceModelProfileCatalog::from_config(self.config);
        let route_profile = synapse_domain::application::services::model_lane_resolution::resolve_route_selection_profile(
            self.config,
            &RouteSelection {
                provider: provider.clone(),
                model: model.clone(),
                lane: request.lane,
                candidate_index: request.candidate_index,
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
            runtime_decision_traces: Vec::new(),
            },
            Some(&catalog),
        );
        let outcome = apply_web_runtime_route(
            self.state,
            self.ui_session_key,
            self.conversation_key,
            self.config,
            Some(provider.as_str()),
            Some(model.as_str()),
            request.lane,
            request.candidate_index,
            request
                .target_context_window_tokens
                .or(route_profile.context_window_tokens),
            self.token_prefix,
        )
        .await?;
        if let Some(preflight) = outcome.blocked_preflight {
            Ok(RuntimeModelSwitchOutcome::Blocked {
                provider,
                lane: request.lane,
                compacted: outcome.compacted,
                preflight,
            })
        } else {
            Ok(RuntimeModelSwitchOutcome::Applied {
                provider,
                lane: request.lane,
                compacted: outcome.compacted,
            })
        }
    }

    async fn clear_session(&mut self) -> anyhow::Result<()> {
        clear_web_session_runtime_state(self.state, self.ui_session_key)
    }
}

#[derive(Debug)]
struct WebRuntimeRouteApplyOutcome {
    compacted: bool,
    blocked_preflight: Option<RouteSwitchPreflight>,
}

async fn apply_web_runtime_route(
    state: &AppState,
    ui_session_key: &str,
    conversation_key: &str,
    config: &synapse_domain::config::schema::Config,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    route_lane: Option<CapabilityLane>,
    route_candidate_index: Option<usize>,
    target_context_window_tokens: Option<usize>,
    _token_prefix: &str,
) -> anyhow::Result<WebRuntimeRouteApplyOutcome> {
    if let Some(provider) = provider_override {
        build_web_agent_runtime(state, config, Arc::clone(&state.observer))
            .get_or_create_provider(provider)
            .await?;
    }

    let mut route = current_web_route_selection(state, conversation_key)
        .unwrap_or_else(|_| default_web_route_selection(state, config));
    if let Some(provider) = provider_override {
        route.provider = provider.to_string();
        if model_override.is_none() {
            route.lane = None;
            route.candidate_index = None;
        }
    }
    if let Some(model) = model_override {
        route.model = model.to_string();
        route.lane = route_lane;
        route.candidate_index = route_candidate_index;
    }
    route.clear_runtime_diagnostics();

    let mut target_profile =
        synapse_domain::application::services::model_lane_resolution::resolve_route_selection_profile(
            config,
            &route,
            Some(&WorkspaceModelProfileCatalog::with_provider_endpoint(
                config.workspace_dir.clone(),
                Some(default_web_provider(config).as_str()),
                config.api_url.as_deref(),
            )),
        );
    if let Some(tokens) = target_context_window_tokens {
        target_profile.context_window_tokens = Some(tokens);
    }

    let preflight =
        resolve_web_runtime_route_switch_preflight(state, conversation_key, &target_profile);
    if preflight.preflight.status == RouteSwitchStatus::TooLarge {
        return Ok(WebRuntimeRouteApplyOutcome {
            compacted: preflight.compacted,
            blocked_preflight: Some(preflight.into_preflight()),
        });
    }
    let compacted = preflight.compacted;

    {
        let default_route = default_web_route_selection(state, config);
        let mut routes = state
            .web_route_overrides
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if route == default_route {
            routes.remove(conversation_key);
        } else {
            routes.insert(conversation_key.to_string(), route);
        }
    }
    if let Ok(mut sessions) = state.chat_sessions.lock() {
        if let Some(session) = sessions.get_mut(ui_session_key) {
            session.last_active = Instant::now();
        }
    }

    Ok(WebRuntimeRouteApplyOutcome {
        compacted,
        blocked_preflight: None,
    })
}

fn web_history_port(
    state: &AppState,
) -> Arc<dyn synapse_domain::ports::conversation_history::ConversationHistoryPort> {
    crate::inbound_runtime_ports::InboundRuntimeStoreFactory::history(
        Arc::clone(&state.web_conversation_histories),
        None,
    )
}

fn resolve_web_runtime_route_switch_preflight(
    state: &AppState,
    conversation_key: &str,
    target_profile: &synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile,
) -> synapse_domain::application::services::route_switch_preflight::RouteSwitchPreflightResolution {
    let history = web_history_port(state);
    crate::runtime_history_hygiene::resolve_route_switch_preflight_for_history_port(
        history.as_ref(),
        conversation_key,
        target_profile,
        synapse_domain::application::services::history_compaction::SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
    )
}

async fn web_route_effective_context_cache_stats(
    state: &AppState,
    config: &synapse_domain::config::schema::Config,
    route: &RouteSelection,
) -> synapse_domain::ports::route_selection::ContextCacheStats {
    let history_compaction_cache =
        crate::runtime::history_compaction_cache::shared_history_compaction_cache(
            &config.workspace_dir,
            &state.agent_id,
        );
    crate::runtime_history_hygiene::route_effective_context_cache_stats(
        &config.compression,
        config.compression_overrides.as_slice(),
        history_compaction_cache.as_ref(),
        route,
    )
    .await
}

fn clear_web_session_runtime_state(state: &AppState, ui_session_key: &str) -> anyhow::Result<()> {
    let runtime_scope_prefix =
        canonical_web_conversation_scope_prefix_for_state(state, ui_session_key)?;
    if let Ok(mut histories) = state.web_conversation_histories.lock() {
        histories.retain(|runtime_key, _| !runtime_key.starts_with(&runtime_scope_prefix));
    }
    state
        .web_route_overrides
        .lock()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .retain(|runtime_key, _| !runtime_key.starts_with(&runtime_scope_prefix));
    if let Ok(mut sessions) = state.chat_sessions.lock() {
        if let Some(session) = sessions.get_mut(ui_session_key) {
            session.message_count = 0;
            session.input_tokens = 0;
            session.output_tokens = 0;
            session.current_goal = None;
            session.session_summary = None;
        }
    }
    Ok(())
}

async fn compact_web_session_after_context_limit(
    state: &AppState,
    session_key: &str,
) -> anyhow::Result<bool> {
    Ok(web_history_port(state).compact_history(
        session_key,
        synapse_domain::application::services::history_compaction::SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
    ))
}

// ── RPC: chat.abort ─────────────────────────────────────────────────────────

fn handle_chat_abort(
    params: &serde_json::Value,
    state: &AppState,
    default_session: &str,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_key = params["session"].as_str().unwrap_or(default_session);
    check_session_ownership(session_key, token_prefix)?;

    let run_id = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(session_key) {
            // Signal the abort channel
            if let Some(tx) = s.abort_tx.take() {
                let _ = tx.send(true);
            }
            s.run_id.take()
        } else {
            None
        }
    };

    Ok(serde_json::json!({
        "ok": true,
        "run_id": run_id,
    }))
}

// ── RPC: sessions.list ──────────────────────────────────────────────────────

async fn handle_sessions_list(
    state: &AppState,
    _token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    // List ALL sessions (web, channel, ipc) — single-user dashboard shows everything.
    let store = state
        .conversation_store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("web conversation_store is required"))?;
    let store_sessions = store.list_sessions(None).await;
    let mut all_sessions = store_sessions;
    let channel_conversation_store =
        crate::inbound_runtime_ports::InboundRuntimeStoreFactory::conversation_store(
            state.channel_session_backend.clone(),
        );
    let preview_store =
        crate::inbound_runtime_ports::InboundRuntimeStoreFactory::composite_conversation_store(
            vec![Some(store.clone()), channel_conversation_store],
        )
        .unwrap_or_else(|| store.clone());

    // Also include channel sessions through the same backend port used by channels.
    if let Some(backend) = state.channel_session_backend.as_ref() {
        for session in backend.list_sessions_with_metadata().await {
            let summary = backend
                .load_summary(&session.key)
                .await
                .map(|summary| summary.summary);
            all_sessions.push(ConversationSession {
                key: session.key.clone(),
                kind: ConversationKind::Channel,
                label: session.label.or_else(|| Some(session.key.clone())),
                summary,
                current_goal: session.current_goal,
                created_at: session.created_at.timestamp().max(0) as u64,
                last_active: session.last_activity.timestamp().max(0) as u64,
                #[allow(clippy::cast_possible_truncation)]
                message_count: session.message_count as u32,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
            });
        }
    }
    // Sort all sessions by last_active descending
    all_sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));

    // Snapshot in-memory run state (release lock before async work)
    let active_runs: std::collections::HashSet<String> = {
        let sessions_lock = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sessions_lock
            .iter()
            .filter(|(_, ms)| ms.run_id.is_some())
            .map(|(k, _)| k.clone())
            .collect()
    };

    let mut sessions: Vec<serde_json::Value> = Vec::with_capacity(all_sessions.len());
    for s in &all_sessions {
        let has_active_run = active_runs.contains(&s.key);

        // Get preview from last message
        let evts = preview_store.get_events(&s.key, 1).await;
        let preview = evts.last().map(|e| truncate_str(&e.content, 60));

        let kind_str = match s.kind {
            ConversationKind::Web => "web",
            ConversationKind::Channel => "channel",
            ConversationKind::Ipc => "ipc",
        };
        // Parse channel type from key (e.g. "matrix_room_user" → "matrix")
        let channel = if s.kind == ConversationKind::Channel {
            s.key.split('_').next().map(String::from)
        } else {
            None
        };

        sessions.push(serde_json::json!({
            "key": s.key,
            "kind": kind_str,
            "channel": channel,
            "label": s.label,
            "last_active": s.last_active,
            "message_count": s.message_count,
            "preview": preview,
            "has_active_run": has_active_run,
            "input_tokens": s.input_tokens,
            "output_tokens": s.output_tokens,
            "current_goal": s.current_goal,
            "session_summary": s.summary,
        }));
    }

    Ok(serde_json::json!({ "sessions": sessions }))
}

// ── RPC: sessions.new ───────────────────────────────────────────────────────

async fn handle_sessions_new(
    params: &serde_json::Value,
    state: &AppState,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let label = params["label"].as_str().map(String::from);
    // Phase 4.0 Slice 3: session key from conversation_service
    let session_key =
        synapse_domain::application::services::conversation_service::new_web_session_key(
            token_prefix,
        );

    ensure_session(state, &session_key, token_prefix).await?;

    if let Some(ref lbl) = label {
        {
            let mut sessions = state
                .chat_sessions
                .lock()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(s) = sessions.get_mut(&session_key) {
                s.label = Some(lbl.clone());
            }
        }
        if let Some(store) = state.conversation_store.as_ref() {
            let _ = store.update_label(&session_key, lbl).await;
        }
    }

    Ok(serde_json::json!({
        "session_key": session_key,
        "label": label,
    }))
}

// ── RPC: sessions.rename ────────────────────────────────────────────────────

async fn handle_sessions_rename(
    params: &serde_json::Value,
    state: &AppState,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'key'"))?;
    check_session_ownership(key, token_prefix)?;
    let label = params["label"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'label'"))?;

    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(key) {
            s.label = Some(label.to_string());
        }
    }

    if let Some(store) = state.conversation_store.as_ref() {
        store.update_label(key, label).await?;
    }

    emit_session_event(state, "session.updated", key);
    Ok(serde_json::json!({ "ok": true }))
}

// ── RPC: sessions.delete ────────────────────────────────────────────────────

async fn handle_sessions_delete(
    params: &serde_json::Value,
    state: &AppState,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'key'"))?;
    check_session_ownership(key, token_prefix)?;

    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sessions.remove(key);
    }

    // Phase 4.0 Slice 3: delete via conversation_service
    if let Some(store) = state.conversation_store.as_ref() {
        let _ = synapse_domain::application::services::conversation_service::delete_session(
            store.as_ref(),
            key,
        )
        .await;
    }

    emit_session_event(state, "session.deleted", key);
    Ok(serde_json::json!({ "ok": true }))
}

// ── RPC: sessions.reset ─────────────────────────────────────────────────────

async fn handle_sessions_reset(
    params: &serde_json::Value,
    state: &AppState,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'key'"))?;
    check_session_ownership(key, token_prefix)?;

    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(key) {
            s.message_count = 0;
            s.input_tokens = 0;
            s.output_tokens = 0;
            s.current_goal = None;
            s.session_summary = None;
        }
    }

    // Phase 4.0 Slice 3: reset via conversation_service
    if let Some(store) = state.conversation_store.as_ref() {
        let _ = synapse_domain::application::services::conversation_service::reset_session(
            store.as_ref(),
            key,
        )
        .await;
    }

    emit_session_event(state, "session.updated", key);
    Ok(serde_json::json!({ "ok": true }))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn persist_message(
    state: &AppState,
    session_key: &str,
    kind: &str,
    role: Option<&str>,
    content: &str,
    tool_name: Option<&str>,
    run_id: Option<&str>,
) {
    if let Some(store) = state.conversation_store.as_ref() {
        tracing::info!(session_key, kind, "ws.persist_message");
        let event_type = match kind {
            "user" => EventType::User,
            "assistant" => EventType::Assistant,
            "tool_call" => EventType::ToolCall,
            "tool_result" => EventType::ToolResult,
            "error" => EventType::Error,
            "interrupted" => EventType::Interrupted,
            _ => EventType::System,
        };
        let event = ConversationEvent {
            event_type,
            actor: role.unwrap_or(kind).to_string(),
            content: content.to_string(),
            tool_name: tool_name.map(String::from),
            run_id: run_id.map(String::from),
            input_tokens: None,
            output_tokens: None,
            #[allow(clippy::cast_sign_loss)]
            timestamp: now_secs() as u64,
        };
        if let Err(e) = store.append_event(session_key, &event).await {
            tracing::warn!("conversation_store: failed to append event: {e}");
        }
        if let Err(e) = store.touch_session(session_key).await {
            tracing::warn!("conversation_store: failed to touch session: {e}");
        }
    }
}

/// Update in-memory token counters only (DB update handled by conversation_service).
fn persist_usage_memory(
    state: &AppState,
    session_key: &str,
    usage: Option<&synapse_providers::traits::TokenUsage>,
) {
    if let Some(u) = usage {
        if let Ok(mut sessions) = state.chat_sessions.lock() {
            if let Some(s) = sessions.get_mut(session_key) {
                s.input_tokens += u.input_tokens.unwrap_or(0);
                s.output_tokens += u.output_tokens.unwrap_or(0);
            }
        }
    }
}

/// Sync in-memory message count with the given delta.
fn sync_memory_count(state: &AppState, session_key: &str, delta: u32) {
    if let Ok(mut sessions) = state.chat_sessions.lock() {
        if let Some(s) = sessions.get_mut(session_key) {
            s.message_count += delta;
            s.last_active = Instant::now();
        }
    }
}

async fn auto_label_if_needed(state: &AppState, session_key: &str, first_message: &str) {
    let needs_label = {
        let sessions = match state.chat_sessions.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        sessions
            .get(session_key)
            .map_or(false, |s| s.label.is_none())
    };

    if !needs_label {
        return;
    }

    let label = truncate_str(first_message, AUTO_LABEL_MAX_LEN);
    {
        let mut sessions = match state.chat_sessions.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Some(s) = sessions.get_mut(session_key) {
            s.label = Some(label.clone());
        }
    }
    if let Some(store) = state.conversation_store.as_ref() {
        let _ = store.update_label(session_key, &label).await;
    }
}

/// Update the session's current_goal from the latest user message.
/// This is a lightweight resume hint — not hidden reasoning.
async fn update_session_goal(state: &AppState, session_key: &str, user_message: &str) {
    let goal = truncate_str(user_message, 80);
    {
        if let Ok(mut sessions) = state.chat_sessions.lock() {
            if let Some(s) = sessions.get_mut(session_key) {
                s.current_goal = Some(goal.clone());
            }
        }
    }
    if let Some(store) = state.conversation_store.as_ref() {
        if let Err(e) = store.update_goal(session_key, &goal).await {
            tracing::warn!("conversation_store: failed to update goal: {e}");
        }
    }
}

/// Persist tool_call and tool_result events from the agent's history after a turn.
async fn persist_tool_events(
    state: &AppState,
    session_key: &str,
    history: &[synapse_providers::ConversationMessage],
) {
    use synapse_providers::ConversationMessage;
    let mut call_signature_by_id = std::collections::HashMap::new();
    let mut seen_tool_calls = std::collections::HashSet::new();
    let mut seen_tool_results = std::collections::HashSet::new();

    for msg in history {
        match msg {
            ConversationMessage::AssistantToolCalls { tool_calls, .. } => {
                for tc in tool_calls {
                    let call_signature = persisted_tool_call_signature(tc);
                    if !tc.id.trim().is_empty() {
                        call_signature_by_id.insert(tc.id.clone(), call_signature.clone());
                    }
                    if !seen_tool_calls.insert(call_signature) {
                        continue;
                    }
                    persist_message(
                        state,
                        session_key,
                        "tool_call",
                        Some("assistant"),
                        &format!("{}({})", tc.name, tc.arguments),
                        Some(&tc.name),
                        None,
                    )
                    .await;
                }
            }
            ConversationMessage::ToolResults(results) => {
                for tr in results {
                    let result_signature =
                        persisted_tool_result_signature(tr, &call_signature_by_id);
                    if !seen_tool_results.insert(result_signature) {
                        continue;
                    }
                    persist_message(
                        state,
                        session_key,
                        "tool_result",
                        None,
                        &tr.content,
                        None,
                        None,
                    )
                    .await;
                }
            }
            ConversationMessage::Chat(_) => {} // handled separately by caller
        }
    }
}

fn persisted_tool_call_signature(tool_call: &synapse_providers::ToolCall) -> String {
    format!("call:{}:{}", tool_call.name, tool_call.arguments)
}

fn persisted_tool_result_signature(
    result: &synapse_providers::ToolResultMessage,
    call_signature_by_id: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(call_signature) = call_signature_by_id.get(&result.tool_call_id) {
        format!("result:{call_signature}")
    } else {
        format!("result:{}", result.content)
    }
}

/// Emit a session event on the broadcast channel for multi-tab freshness.
fn emit_session_event(state: &AppState, event_type: &str, session_key: &str) {
    let _ = state.event_tx.send(serde_json::json!({
        "type": event_type,
        "session_key": session_key,
        "timestamp": now_secs(),
    }));
}

/// Emit a session lifecycle event with an associated run_id.
fn emit_run_event(state: &AppState, event_type: &str, session_key: &str, run_id: &str) {
    let _ = state.event_tx.send(serde_json::json!({
        "type": event_type,
        "session_key": session_key,
        "run_id": run_id,
        "timestamp": now_secs(),
    }));
}

/// WebSocket transport sink for real-time tool notifications.
/// Events are sent through the same `out_tx` channel as the assistant response.
struct WsToolNotificationHandler {
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    session_key: String,
    seen_tool_calls: std::sync::Mutex<std::collections::HashSet<String>>,
    seen_tool_results: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// Web transport sink for inbound lifecycle events. Final assistant output is
/// still delivered through `PresentedOutput`; this port only prevents web from
/// silently dropping core lifecycle signals that channel transports can expose.
struct WebChannelOutput {
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    session_key: String,
}

#[async_trait::async_trait]
impl synapse_domain::ports::channel_output::ChannelOutputPort for WebChannelOutput {
    async fn send_message(
        &self,
        _recipient: &str,
        text: &str,
        _thread_ref: Option<&str>,
    ) -> anyhow::Result<()> {
        self.tx
            .send(
                serde_json::json!({
                    "type": "assistant_response",
                    "session_key": self.session_key,
                    "content": text,
                    "timestamp": now_secs(),
                })
                .to_string(),
            )
            .map_err(|error| anyhow::anyhow!("web channel output send failed: {error}"))
    }

    async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        self.send_status("typing_start")
    }

    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        self.send_status("typing_stop")
    }

    async fn add_reaction(
        &self,
        _recipient: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.send_reaction("reaction_add", message_id, emoji)
    }

    async fn remove_reaction(
        &self,
        _recipient: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        self.send_reaction("reaction_remove", message_id, emoji)
    }

    async fn fetch_message_text(&self, _message_id: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    fn supports_streaming(&self) -> bool {
        false
    }
}

impl WebChannelOutput {
    fn send_status(&self, status: &str) -> anyhow::Result<()> {
        self.tx
            .send(
                serde_json::json!({
                    "type": "runtime_status",
                    "session_key": self.session_key,
                    "status": status,
                    "timestamp": now_secs(),
                })
                .to_string(),
            )
            .map_err(|error| anyhow::anyhow!("web channel output status send failed: {error}"))
    }

    fn send_reaction(&self, event_type: &str, message_id: &str, emoji: &str) -> anyhow::Result<()> {
        self.tx
            .send(
                serde_json::json!({
                    "type": event_type,
                    "session_key": self.session_key,
                    "message_id": message_id,
                    "emoji": emoji,
                    "timestamp": now_secs(),
                })
                .to_string(),
            )
            .map_err(|error| anyhow::anyhow!("web channel output reaction send failed: {error}"))
    }
}

impl RuntimeToolNotificationHandler for WsToolNotificationHandler {
    fn notify(&self, notification: RuntimeToolNotification) {
        match &notification {
            RuntimeToolNotification::CallStart { .. } => {
                let key = notification.web_dedupe_key();
                if let Ok(mut seen) = self.seen_tool_calls.lock() {
                    if !seen.insert(key) {
                        return;
                    }
                }
            }
            RuntimeToolNotification::Result { .. } => {
                let result_key = notification.web_dedupe_key();
                if let Ok(mut seen) = self.seen_tool_results.lock() {
                    if !seen.insert(result_key) {
                        return;
                    }
                }
            }
        }
        let _ = self
            .tx
            .send(notification.web_json(&self.session_key, now_secs()));
    }
}

/// Generate a rolling session summary every N messages (fire-and-forget).
///
/// Phase 4.0 Slice 3: delegates to conversation_service::generate_session_summary.
pub(crate) async fn summarize_session_if_needed(state: &AppState, session_key: &str) {
    use synapse_domain::application::services::conversation_service::WEB_SUMMARY_INTERVAL;

    let Some(store) = state.conversation_store.as_ref() else {
        return;
    };

    // Read session state (in-memory first, then DB)
    let (msg_count, last_summary, prev_summary) = {
        let from_memory = state.chat_sessions.lock().ok().and_then(|sessions| {
            sessions.get(session_key).map(|s| {
                (
                    s.message_count as usize,
                    s.last_summary_count as usize,
                    s.session_summary.clone(),
                )
            })
        });
        match from_memory {
            Some(v) => v,
            None => match store.get_session(session_key).await {
                Some(s) => (s.message_count as usize, 0, s.summary),
                None => return,
            },
        }
    };

    let config_snapshot = state.config.lock().clone();
    let provider_runtime_options =
        synapse_providers::provider_runtime_options_from_config(&config_snapshot);

    match crate::inbound_runtime_summary::summarize_session_if_needed(
        crate::inbound_runtime_summary::InboundRuntimeSummaryInput {
            store: store.as_ref(),
            current_provider: Arc::clone(&state.provider),
            config: &config_snapshot,
            current_model: &state.model,
            provider_runtime_options: &provider_runtime_options,
            session_key,
            message_count: msg_count,
            last_summary_count: last_summary,
            previous_summary: prev_summary.as_deref(),
            interval: WEB_SUMMARY_INTERVAL,
            transport_label: "web",
        },
    )
    .await
    {
        Ok(Some(summary)) => {
            // Update in-memory cache
            if let Ok(mut sessions) = state.chat_sessions.lock() {
                if let Some(s) = sessions.get_mut(session_key) {
                    s.session_summary = Some(summary.clone());
                    s.last_summary_count = s.message_count;
                }
            }
            emit_session_event(state, "session.updated", session_key);
            tracing::debug!("session summary updated for {session_key}");
        }
        Ok(None) => {} // not needed yet
        Err(e) => {
            tracing::warn!("session summary generation failed: {e}");
        }
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 1).collect();
        format!("{truncated}…")
    }
}

/// Extract unique tool names from a conversation history snapshot.
fn extract_tool_names(history: &[synapse_providers::ConversationMessage]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for msg in history {
        if let synapse_providers::ConversationMessage::AssistantToolCalls { tool_calls, .. } = msg {
            for tc in tool_calls {
                if seen.insert(tc.name.clone()) {
                    names.push(tc.name.clone());
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn extract_ws_token_from_authorization_header() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer zc_test123".parse().unwrap());
        assert_eq!(extract_ws_token(&headers, None), Some("zc_test123"));
    }

    #[test]
    fn extract_ws_token_from_subprotocol() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "sec-websocket-protocol",
            "synapseclaw.v1, bearer.zc_sub456".parse().unwrap(),
        );
        assert_eq!(extract_ws_token(&headers, None), Some("zc_sub456"));
    }

    #[test]
    fn extract_ws_token_from_query_param() {
        let headers = HeaderMap::new();
        assert_eq!(
            extract_ws_token(&headers, Some("zc_query789")),
            Some("zc_query789")
        );
    }

    #[test]
    fn extract_ws_token_precedence_header_over_subprotocol() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer zc_header".parse().unwrap());
        headers.insert("sec-websocket-protocol", "bearer.zc_sub".parse().unwrap());
        assert_eq!(
            extract_ws_token(&headers, Some("zc_query")),
            Some("zc_header")
        );
    }

    #[test]
    fn extract_ws_token_precedence_subprotocol_over_query() {
        let mut headers = HeaderMap::new();
        headers.insert("sec-websocket-protocol", "bearer.zc_sub".parse().unwrap());
        assert_eq!(extract_ws_token(&headers, Some("zc_query")), Some("zc_sub"));
    }

    #[test]
    fn extract_ws_token_returns_none_when_empty() {
        let headers = HeaderMap::new();
        assert_eq!(extract_ws_token(&headers, None), None);
    }

    #[test]
    fn extract_ws_token_skips_empty_header_value() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer ".parse().unwrap());
        assert_eq!(
            extract_ws_token(&headers, Some("zc_fallback")),
            Some("zc_fallback")
        );
    }

    #[test]
    fn extract_ws_token_skips_empty_query_param() {
        let headers = HeaderMap::new();
        assert_eq!(extract_ws_token(&headers, Some("")), None);
    }

    #[test]
    fn extract_ws_token_subprotocol_with_multiple_entries() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "sec-websocket-protocol",
            "synapseclaw.v1, bearer.zc_tok, other".parse().unwrap(),
        );
        assert_eq!(extract_ws_token(&headers, None), Some("zc_tok"));
    }

    #[test]
    fn token_hash_prefix_deterministic() {
        let a = token_hash_prefix("test_token");
        let b = token_hash_prefix("test_token");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let result = truncate_str("hello world this is long", 10);
        assert!(result.len() <= 14); // UTF-8 ellipsis is 3 bytes
        assert!(result.ends_with('…'));
    }

    #[test]
    fn persisted_tool_event_keys_are_stable() {
        let tool_call = synapse_providers::ToolCall {
            id: "call-123".into(),
            name: "shell".into(),
            arguments: "{\"cmd\":\"printf ok\"}".into(),
        };
        let duplicate_call = synapse_providers::ToolCall {
            id: "call-456".into(),
            name: "shell".into(),
            arguments: "{\"cmd\":\"printf ok\"}".into(),
        };
        let result = synapse_providers::ToolResultMessage {
            tool_call_id: "call-123".into(),
            content: "ok".into(),
        };
        let duplicate_result = synapse_providers::ToolResultMessage {
            tool_call_id: "call-123".into(),
            content: "ok".into(),
        };
        let call_map = std::collections::HashMap::from([
            (
                tool_call.id.clone(),
                persisted_tool_call_signature(&tool_call),
            ),
            (
                duplicate_call.id.clone(),
                persisted_tool_call_signature(&duplicate_call),
            ),
        ]);

        assert_eq!(
            persisted_tool_call_signature(&tool_call),
            persisted_tool_call_signature(&duplicate_call)
        );
        assert_eq!(
            persisted_tool_result_signature(&result, &call_map),
            persisted_tool_result_signature(&duplicate_result, &call_map)
        );
    }
}
