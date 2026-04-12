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

use super::chat_db::ChatMessageRow;
use super::{AppState, ChatSession};
use crate::runtime_adapter_contract::{
    execute_runtime_command_effect, RuntimeCommandHost, RuntimeModelHelpSnapshot,
    RuntimeModelSwitchOutcome, RuntimeProviderSwitchOutcome, RuntimeRouteMutationRequest,
    WebRuntimeAdapterContract,
};
use crate::runtime_routes::WorkspaceModelProfileCatalog;
use crate::runtime_tool_notifications::RuntimeToolNotification;
use crate::runtime_tool_observer::{RuntimeToolNotificationHandler, RuntimeToolNotifyObserver};
use synapse_domain::application::services::route_switch_preflight::{
    RouteSwitchPreflight, RouteSwitchStatus,
};
use synapse_domain::application::services::summary_route_resolution::resolve_summary_route;
use synapse_domain::config::schema::CapabilityLane;
use synapse_domain::domain::channel::ChannelCapability;
use synapse_domain::domain::conversation::{
    ConversationEvent, ConversationKind, ConversationSession, EventType,
};
use synapse_domain::domain::conversation_target::CurrentConversationContext;
// Run types no longer needed directly — lifecycle managed by conversation_service
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap},
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use synapse_domain::ports::route_selection::RouteSelection;

/// The sub-protocol we support for the chat WebSocket.
const WS_PROTOCOL: &str = "synapseclaw.v1";

/// Prefix used in `Sec-WebSocket-Protocol` to carry a bearer token.
const BEARER_SUBPROTO_PREFIX: &str = "bearer.";

/// Max sessions kept in memory per token prefix.
const MAX_MEMORY_SESSIONS: usize = 50;

/// Auto-label truncation length.
const AUTO_LABEL_MAX_LEN: usize = 40;

fn web_runtime_ports(state: &AppState) -> crate::agent::AgentRuntimePorts {
    crate::agent::AgentRuntimePorts {
        conversation_store: state.conversation_store.clone(),
        conversation_context: Some(Arc::clone(&state.conversation_context)),
        user_profile_store: Some(Arc::clone(&state.user_profile_store)),
        user_profile_context: Some(Arc::clone(&state.user_profile_context)),
        turn_defaults_context: Some(Arc::clone(&state.turn_defaults_context)),
        scoped_instruction_context: Some(Arc::clone(&state.scoped_instruction_context)),
        channel_registry: state.channel_registry.clone(),
        run_recipe_store: Some(Arc::clone(&state.run_recipe_store)),
        history_compaction_cache: None,
    }
}

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
    if let Err(e) = ensure_session(&state, &session_key).await {
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

/// Ensure a session exists in memory. If not, create from DB or fresh.
async fn ensure_session(state: &AppState, session_key: &str) -> anyhow::Result<()> {
    {
        let sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if sessions.contains_key(session_key) {
            return Ok(());
        }
    }

    // Try resuming via synapse_domain use case (ConversationStorePort path)
    // or fall back to legacy ChatDb path.
    let db_session;
    let config = state.config.lock().clone();
    let mut agent = crate::agent::Agent::from_config_with_runtime_context(
        &config,
        Some(state.mem.clone()),
        web_runtime_ports(state),
    )
    .await?;
    agent.set_dialogue_state_store(Some(Arc::clone(&state.dialogue_state_store)));
    agent.set_conversation_store(state.conversation_store.clone());
    agent.set_run_recipe_store(Some(Arc::clone(&state.run_recipe_store)));
    agent.set_user_profile_store(Some(Arc::clone(&state.user_profile_store)));
    agent.set_channel_registry(state.channel_registry.clone());

    let now = Instant::now();
    let _now_secs_val = now_secs();

    let (label, msg_count, input_tok, output_tok, current_goal, session_summary) =
        if let Some(store) = state.conversation_store.as_ref() {
            // Phase 4.0 path: use ResumeConversation use case
            match synapse_domain::application::use_cases::resume_conversation::execute(
                store.as_ref(),
                session_key,
            )
            .await
            {
                Ok(resumed) => {
                    // Replay transcript into agent
                    for turn in &resumed.transcript {
                        agent.push_history(synapse_providers::ConversationMessage::Chat(
                            synapse_providers::ChatMessage {
                                role: turn.role.clone(),
                                content: turn.content.clone(),
                            },
                        ));
                    }
                    db_session = Some(resumed.session.clone());
                    (
                        resumed.session.label,
                        resumed.session.message_count,
                        resumed.session.input_tokens,
                        resumed.session.output_tokens,
                        resumed.session.current_goal,
                        resumed.session.summary,
                    )
                }
                Err(_) => {
                    // Session not found — will create fresh
                    db_session = None;
                    (None, 0, 0, 0, None, None)
                }
            }
        } else {
            // Legacy ChatDb fallback
            db_session = if let Some(db) = state.chat_db.as_ref() {
                db.get_session(session_key)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| ConversationSession {
                        key: r.key,
                        kind: ConversationKind::Web,
                        label: r.label,
                        summary: r.session_summary,
                        current_goal: r.current_goal,
                        #[allow(clippy::cast_sign_loss)]
                        created_at: r.created_at as u64,
                        #[allow(clippy::cast_sign_loss)]
                        last_active: r.last_active as u64,
                        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                        message_count: r.message_count as u32,
                        #[allow(clippy::cast_sign_loss)]
                        input_tokens: r.input_tokens as u64,
                        #[allow(clippy::cast_sign_loss)]
                        output_tokens: r.output_tokens as u64,
                    })
            } else {
                None
            };
            if let Some(ref session) = db_session {
                if let Some(db) = state.chat_db.as_ref() {
                    let messages = db.get_messages(session_key, 200).await?;
                    replay_messages_into_agent(&mut agent, &messages)?;
                }
                (
                    session.label.clone(),
                    session.message_count,
                    session.input_tokens,
                    session.output_tokens,
                    session.current_goal.clone(),
                    session.summary.clone(),
                )
            } else {
                (None, 0, 0, 0, None, None)
            }
        };

    // Bind memory recall to this web session (episodic recall is session-scoped).
    agent.set_memory_session_id(Some(session_key.to_string()));
    if agent.compact_for_session_hygiene().await {
        tracing::info!(
            session_key = %session_key,
            "Compacted resumed web session before agent execution"
        );
    }

    let session = ChatSession {
        agent,
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
        last_summary_count: 0,
    };

    let _need_persist = {
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
            }
        }

        sessions.insert(session_key.to_string(), session);
        db_session.is_none()
    }; // MutexGuard dropped here

    // Don't persist empty sessions on WS connect — only persist when
    // the first message is sent (in handle_chat_send_rpc).
    // This prevents phantom empty sessions from appearing on every page visit.

    Ok(())
}

/// Replay persisted messages into an Agent instance (user/assistant/system only).
fn replay_messages_into_agent(
    agent: &mut crate::agent::Agent,
    messages: &[ChatMessageRow],
) -> anyhow::Result<()> {
    use synapse_providers::{ChatMessage, ConversationMessage};

    for msg in messages {
        let conv_msg = match msg.kind.as_str() {
            "user" => ConversationMessage::Chat(ChatMessage::user(msg.content.clone())),
            "assistant" => ConversationMessage::Chat(ChatMessage::assistant(msg.content.clone())),
            "system" => ConversationMessage::Chat(ChatMessage::system(msg.content.clone())),
            _ => continue, // tool_call, tool_result, error, interrupted — UI-only
        };
        agent.push_history(conv_msg);
    }
    Ok(())
}

/// Replay ConversationEvents into an Agent (Phase 4.0 path).
fn replay_events_into_agent(
    agent: &mut crate::agent::Agent,
    events: &[ConversationEvent],
) -> anyhow::Result<()> {
    use synapse_providers::{ChatMessage, ConversationMessage};

    for event in events {
        let conv_msg = match event.event_type {
            EventType::User => ConversationMessage::Chat(ChatMessage::user(event.content.clone())),
            EventType::Assistant => {
                ConversationMessage::Chat(ChatMessage::assistant(event.content.clone()))
            }
            EventType::System => {
                ConversationMessage::Chat(ChatMessage::system(event.content.clone()))
            }
            _ => continue, // ToolCall, ToolResult, Error, Interrupted — UI-only
        };
        agent.push_history(conv_msg);
    }
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
    ensure_session(state, &session_key).await?;

    let events = if let Some(store) = state.conversation_store.as_ref() {
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        store.get_events(&session_key, limit as usize).await
    } else if let Some(db) = state.chat_db.as_ref() {
        db.get_messages(&session_key, limit)
            .await
            .unwrap_or_default()
            .iter()
            .map(|m| ConversationEvent {
                event_type: match m.kind.as_str() {
                    "user" => EventType::User,
                    "assistant" => EventType::Assistant,
                    "tool_call" => EventType::ToolCall,
                    "tool_result" => EventType::ToolResult,
                    "error" => EventType::Error,
                    "interrupted" => EventType::Interrupted,
                    _ => EventType::System,
                },
                actor: m.role.clone().unwrap_or_else(|| m.kind.clone()),
                content: m.content.clone(),
                tool_name: m.tool_name.clone(),
                run_id: m.run_id.clone(),
                #[allow(clippy::cast_sign_loss)]
                input_tokens: m.input_tokens.map(|t| t as u64),
                #[allow(clippy::cast_sign_loss)]
                output_tokens: m.output_tokens.map(|t| t as u64),
                #[allow(clippy::cast_sign_loss)]
                timestamp: m.timestamp as u64,
            })
            .collect()
    } else {
        Vec::new()
    };

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
    let surreal = state
        .surreal
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("SurrealDB not available"))?;

    // Load messages
    let mut resp = surreal
        .query(
            "SELECT role, content, created_at FROM channel_session
             WHERE session_key = $key ORDER BY created_at ASC LIMIT $limit",
        )
        .bind(("key", session_key.to_string()))
        .bind(("limit", limit))
        .await
        .map_err(|e| anyhow::anyhow!("channel history: {e}"))?;

    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();

    let messages: Vec<serde_json::Value> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let role = row.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = row.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let event_type = if role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            serde_json::json!({
                "id": i + 1,
                "event_type": event_type,
                "role": role,
                "content": content,
                "tool_name": null,
                "run_id": null,
                "timestamp": 0,
                "input_tokens": null,
                "output_tokens": null,
            })
        })
        .collect();

    // Load summary if available
    let summary = match surreal
        .query(
            "SELECT summary FROM channel_session_summary
             WHERE session_key = $key LIMIT 1",
        )
        .bind(("key", session_key.to_string()))
        .await
    {
        Ok(mut sr) => {
            let srows: Vec<serde_json::Value> = sr.take(0).unwrap_or_default();
            srows
                .first()
                .and_then(|v| v.get("summary"))
                .and_then(|v| v.as_str())
                .map(String::from)
        }
        Err(_) => None,
    };

    // Parse channel + sender from key
    let (channel, sender) = session_key
        .split_once('_')
        .unwrap_or(("unknown", session_key));

    Ok(serde_json::json!({
        "messages": messages,
        "session_key": session_key,
        "label": format!("{} · {}", channel, sender),
        "session_summary": summary,
        "current_goal": null,
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
    ensure_session(state, &session_key).await?;
    let message = params["message"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'message' param"))?
        .to_string();

    if let Some(result) =
        handle_runtime_command_if_needed(state, &session_key, &message, token_prefix, out_tx)
            .await?
    {
        return Ok(result);
    }

    // Phase 4.0 Slice 3: run lifecycle via conversation_service
    let run_id = if let Some(store) = state.run_store.as_ref() {
        match synapse_domain::application::use_cases::start_conversation_run::create_and_track_run(
            state
                .conversation_store
                .as_deref()
                .expect("conversation_store required"),
            store.as_ref(),
            &session_key,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("run_store: failed to create run: {e}");
                uuid::Uuid::new_v4().to_string()
            }
        }
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
            s.agent
                .set_user_profile_key(Some(format!("web:{token_prefix}")));
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
        &message,
        None,
        None,
    )
    .await;
    auto_label_if_needed(state, &session_key, &message).await;

    // Record history length before turn (for extracting tool events after)
    let history_len_before = {
        let sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sessions
            .get(&session_key)
            .map_or(0, |s| s.agent.history().len())
    };

    // Run agent turn with abort support + real-time tool event push
    let result = run_agent_turn_with_abort(
        state,
        &session_key,
        token_prefix,
        &message,
        abort_rx,
        out_tx,
    )
    .await;

    // Clear run_id + abort_tx, extract usage + collect tool history for async persist
    let (usage, tool_history, tool_facts, user_profile_key) = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let history_snapshot = if let Some(s) = sessions.get_mut(&session_key) {
            s.run_id = None;
            s.abort_tx = None;
            s.last_active = Instant::now();
            // Tool events already pushed in real-time via RuntimeToolNotifyObserver.
            // Snapshot history for async persist (after lock release)
            let history = s.agent.history();
            if history_len_before <= history.len() {
                history[history_len_before..].to_vec()
            } else {
                let fallback_start = history
                    .iter()
                    .rposition(|entry| {
                        matches!(
                            entry,
                            synapse_providers::ConversationMessage::Chat(chat)
                                if chat.role == "user" && chat.content == message
                        )
                    })
                    .unwrap_or(history.len());
                tracing::warn!(
                    session_key = %session_key,
                    history_len_before,
                    history_len_after = history.len(),
                    fallback_start,
                    "Agent history shrank during turn; reconstructing delta from latest user message"
                );
                history[fallback_start..].to_vec()
            }
        } else {
            Vec::new()
        };
        let u = sessions
            .get(&session_key)
            .and_then(|s| s.agent.last_turn_usage().cloned());
        let facts = sessions
            .get(&session_key)
            .map(|s| s.agent.last_turn_tool_facts().to_vec())
            .unwrap_or_default();
        let profile_key = sessions
            .get(&session_key)
            .and_then(|s| s.agent.user_profile_key().map(str::to_string));
        (u, history_snapshot, facts, profile_key)
    };
    match result {
        Ok(response) => {
            // Push assistant message via the same out_tx channel as tool events.
            // This guarantees FIFO order: tool_call → tool_result → assistant.
            // The RPC response only carries metadata (run_id), not the message.
            let _ = out_tx.send(
                serde_json::json!({
                    "type": "assistant",
                    "session_key": session_key,
                    "content": response,
                    "timestamp": now_secs(),
                })
                .to_string(),
            );
            let state_bg = state.clone();
            let session_key_bg = session_key.clone();
            let run_id_bg = run_id.clone();
            let message_bg = message.clone();
            let response_bg = response.clone();
            let tool_history_bg = tool_history.clone();
            let tool_facts_bg = tool_facts.clone();
            let user_profile_key_bg = user_profile_key.clone();
            let usage_bg = usage.clone();
            tokio::spawn(async move {
                persist_tool_events(&state_bg, &session_key_bg, &tool_history_bg).await;
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
                let input = usage_bg.as_ref().and_then(|u| u.input_tokens).unwrap_or(0) as i64;
                let output = usage_bg.as_ref().and_then(|u| u.output_tokens).unwrap_or(0) as i64;
                if let (Some(cs), Some(rs)) = (
                    state_bg.conversation_store.as_ref(),
                    state_bg.run_store.as_ref(),
                ) {
                    let _ = synapse_domain::application::use_cases::start_conversation_run::finalize_success(
                        cs.as_ref(), rs.as_ref(), &session_key_bg, &run_id_bg, input, output,
                    ).await;
                }
                sync_memory_count(&state_bg, &session_key_bg, 2);
                persist_usage_memory(&state_bg, &session_key_bg, usage_bg.as_ref());
                update_session_goal(&state_bg, &session_key_bg, &message_bg).await;
                emit_session_event(&state_bg, "session.updated", &session_key_bg);
                emit_run_event(
                    &state_bg,
                    "session.run_finished",
                    &session_key_bg,
                    &run_id_bg,
                );
                summarize_session_if_needed(&state_bg, &session_key_bg).await;

                let mem = state_bg.mem.clone();
                let input =
                    synapse_domain::application::services::post_turn_orchestrator::PostTurnInput {
                        agent_id: state_bg.agent_id.clone(),
                        user_message: message_bg,
                        assistant_response: response_bg,
                        tools_used: extract_tool_names(&tool_history_bg),
                        tool_facts: tool_facts_bg,
                        run_recipe_store: Some(state_bg.run_recipe_store.clone()),
                        user_profile_store: Some(state_bg.user_profile_store.clone()),
                        user_profile_key: user_profile_key_bg,
                        auto_save_enabled: state_bg.auto_save,
                        event_tx: Some(state_bg.event_tx.clone()),
                    };
                synapse_domain::application::services::post_turn_orchestrator::execute_post_turn_learning(
                    mem.as_ref(),
                    input,
                )
                .await;
            });

            // RPC response: metadata only (assistant message already pushed above)
            Ok(serde_json::json!({
                "run_id": run_id,
            }))
        }
        Err(e) => {
            let msg = e.to_string();
            if msg == "aborted" {
                persist_tool_events(state, &session_key, &tool_history).await;
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
            let sanitized = synapse_providers::sanitize_api_error(&msg);
            persist_tool_events(state, &session_key, &tool_history).await;
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

async fn handle_runtime_command_if_needed(
    state: &AppState,
    session_key: &str,
    message: &str,
    token_prefix: &str,
    out_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<Option<serde_json::Value>> {
    let caps = vec![ChannelCapability::RuntimeCommands];
    let Some(command) =
        synapse_domain::application::services::inbound_message_service::parse_runtime_command(
            message, &caps,
        )
    else {
        return Ok(None);
    };

    let config_snapshot = state.config.lock().clone();
    let effect = synapse_domain::application::services::inbound_message_service::command_effect(
        &command,
        &config_snapshot.model_routes,
    );
    let default_provider = config_snapshot
        .default_provider
        .clone()
        .unwrap_or_else(|| "openrouter".to_string());
    let adapter_contract = WebRuntimeAdapterContract;
    let mut command_host = WebRuntimeCommandHost {
        state,
        session_key,
        config: &config_snapshot,
        default_provider: default_provider.as_str(),
        token_prefix,
    };
    let response = execute_runtime_command_effect(
        &adapter_contract,
        &mut command_host,
        &effect,
        default_provider.as_str(),
    )
    .await?;

    let _ = out_tx.send(
        serde_json::json!({
            "type": "assistant",
            "session_key": session_key,
            "content": response,
            "timestamp": now_secs(),
        })
        .to_string(),
    );

    if let Some(store) = state.conversation_store.as_ref() {
        let session = synapse_domain::application::services::conversation_service::new_web_session(
            session_key,
            None,
        );
        let _ = store.upsert_session(&session).await;
    }

    Ok(Some(serde_json::json!({
        "command": true,
    })))
}

fn current_web_route_selection(
    state: &AppState,
    session_key: &str,
) -> anyhow::Result<RouteSelection> {
    let sessions = state
        .chat_sessions
        .lock()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let session = sessions
        .get(session_key)
        .ok_or_else(|| anyhow::anyhow!("session not found"))?;
    let provider = session.agent.provider_name_str().to_string();
    let model = session.agent.model_name_str().to_string();
    Ok(RouteSelection {
        provider: provider.clone(),
        model: model.clone(),
        lane: session.agent.active_lane(),
        candidate_index: session.agent.active_candidate_index(),
        last_admission: session.agent.recent_turn_admissions().last().cloned(),
        recent_admissions: session.agent.recent_turn_admissions().to_vec(),
        last_tool_repair: session.agent.last_turn_tool_repair().cloned(),
        recent_tool_repairs: session.agent.recent_turn_tool_repairs().to_vec(),
        context_cache: Some(session.agent.history_compaction_cache_stats_for_route(
            &provider,
            &model,
            session.agent.active_lane(),
            None,
        )),
        assumptions: session.agent.recent_runtime_assumptions().to_vec(),
        calibrations: session.agent.recent_runtime_calibrations().to_vec(),
    })
}

struct WebRuntimeCommandHost<'a> {
    state: &'a AppState,
    session_key: &'a str,
    config: &'a synapse_domain::config::schema::Config,
    default_provider: &'a str,
    token_prefix: &'a str,
}

#[async_trait::async_trait]
impl RuntimeCommandHost for WebRuntimeCommandHost<'_> {
    fn fallback_provider(&self) -> String {
        current_web_route_selection(self.state, self.session_key)
            .ok()
            .map(|route| route.provider)
            .unwrap_or_else(|| self.default_provider.to_string())
    }

    async fn provider_help_route(&mut self) -> anyhow::Result<RouteSelection> {
        current_web_route_selection(self.state, self.session_key)
    }

    async fn model_help_snapshot(&mut self) -> anyhow::Result<RuntimeModelHelpSnapshot> {
        Ok(RuntimeModelHelpSnapshot {
            route: current_web_route_selection(self.state, self.session_key)?,
            config: self.config.clone(),
        })
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
            self.session_key,
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
            .unwrap_or_else(|| self.fallback_provider());
        let model = request
            .model
            .clone()
            .ok_or_else(|| anyhow::anyhow!("model route mutation request missing model"))?;
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
            },
            Some(&catalog),
        );
        let outcome = apply_web_runtime_route(
            self.state,
            self.session_key,
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
        clear_web_session_runtime_state(self.state, self.session_key)
    }
}

#[derive(Debug)]
struct WebRuntimeRouteApplyOutcome {
    compacted: bool,
    blocked_preflight: Option<RouteSwitchPreflight>,
}

async fn apply_web_runtime_route(
    state: &AppState,
    session_key: &str,
    config: &synapse_domain::config::schema::Config,
    provider_override: Option<&str>,
    model_override: Option<&str>,
    route_lane: Option<CapabilityLane>,
    route_candidate_index: Option<usize>,
    target_context_window_tokens: Option<usize>,
    token_prefix: &str,
) -> anyhow::Result<WebRuntimeRouteApplyOutcome> {
    let placeholder = crate::agent::Agent::from_config_with_runtime_context(
        config,
        Some(state.mem.clone()),
        web_runtime_ports(state),
    )
    .await?;

    let mut agent = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = sessions
            .get_mut(session_key)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        std::mem::replace(&mut session.agent, placeholder)
    };

    let mut compacted = false;

    if let Some(target_context_window_tokens) = target_context_window_tokens {
        let preflight = agent
            .prepare_for_target_context_window(target_context_window_tokens)
            .await;
        compacted = preflight.compacted;
        if preflight.preflight.status == RouteSwitchStatus::TooLarge {
            let blocked_preflight = preflight.into_preflight();
            let mut sessions = state
                .chat_sessions
                .lock()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let session = sessions
                .get_mut(session_key)
                .ok_or_else(|| anyhow::anyhow!("session not found"))?;
            session.agent = agent;
            return Ok(WebRuntimeRouteApplyOutcome {
                compacted,
                blocked_preflight: Some(blocked_preflight),
            });
        }
    }

    if let Err(error) = agent
        .switch_runtime_route(
            config,
            provider_override,
            model_override,
            route_lane,
            route_candidate_index,
            state.mem.clone(),
            web_runtime_ports(state),
        )
        .await
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = sessions
            .get_mut(session_key)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        session.agent = agent;
        return Err(error);
    }
    agent.set_dialogue_state_store(Some(Arc::clone(&state.dialogue_state_store)));
    agent.set_conversation_store(state.conversation_store.clone());
    agent.set_run_recipe_store(Some(Arc::clone(&state.run_recipe_store)));
    agent.set_user_profile_store(Some(Arc::clone(&state.user_profile_store)));
    agent.set_channel_registry(state.channel_registry.clone());
    agent.set_memory_session_id(Some(session_key.to_string()));
    agent.set_user_profile_key(Some(format!("web:{token_prefix}")));

    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = sessions
            .get_mut(session_key)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        session.agent = agent;
        session.last_active = Instant::now();
    }

    Ok(WebRuntimeRouteApplyOutcome {
        compacted,
        blocked_preflight: None,
    })
}

fn clear_web_session_runtime_state(state: &AppState, session_key: &str) -> anyhow::Result<()> {
    let mut sessions = state
        .chat_sessions
        .lock()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let session = sessions
        .get_mut(session_key)
        .ok_or_else(|| anyhow::anyhow!("session not found"))?;
    session.agent.clear_history();
    session.message_count = 0;
    session.input_tokens = 0;
    session.output_tokens = 0;
    session.current_goal = None;
    session.session_summary = None;
    Ok(())
}

/// Execute agent.turn() with abort support.
/// Swaps the agent out of sessions lock, runs turn, puts it back.
async fn run_agent_turn_with_abort(
    state: &AppState,
    session_key: &str,
    token_prefix: &str,
    message: &str,
    mut abort_rx: tokio::sync::watch::Receiver<bool>,
    out_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<String> {
    struct ConversationContextGuard {
        port: Arc<dyn synapse_domain::ports::conversation_context::ConversationContextPort>,
    }

    impl Drop for ConversationContextGuard {
        fn drop(&mut self) {
            self.port.set_current(None);
        }
    }

    // Clone config before await to avoid holding MutexGuard across await.
    let config_snapshot = state.config.lock().clone();
    let replacement_agent = crate::agent::Agent::from_config_with_runtime_context(
        &config_snapshot,
        Some(state.mem.clone()),
        web_runtime_ports(state),
    )
    .await?;
    let mut replacement_agent = replacement_agent;
    replacement_agent.set_dialogue_state_store(Some(Arc::clone(&state.dialogue_state_store)));
    replacement_agent.set_conversation_store(state.conversation_store.clone());
    replacement_agent.set_run_recipe_store(Some(Arc::clone(&state.run_recipe_store)));
    replacement_agent.set_user_profile_store(Some(Arc::clone(&state.user_profile_store)));
    replacement_agent.set_channel_registry(state.channel_registry.clone());
    state
        .conversation_context
        .set_current(Some(CurrentConversationContext {
            source_adapter: "web".to_string(),
            conversation_ref: session_key.to_string(),
            reply_ref: session_key.to_string(),
            thread_ref: None,
            actor_id: format!("web:{token_prefix}"),
        }));
    let _conversation_context_guard = ConversationContextGuard {
        port: Arc::clone(&state.conversation_context),
    };
    let mut agent = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = sessions
            .get_mut(session_key)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        std::mem::replace(&mut session.agent, replacement_agent)
    };

    // Wrap the agent's observer to push real-time tool events through WS.
    let base_observer = agent.observer_arc();
    let ws_observer: std::sync::Arc<dyn synapse_observability::Observer> =
        std::sync::Arc::new(RuntimeToolNotifyObserver::new(
            Arc::clone(&base_observer),
            WsToolNotificationHandler {
                tx: out_tx.clone(),
                session_key: session_key.to_string(),
                seen_tool_calls: std::sync::Mutex::new(std::collections::HashSet::new()),
                seen_tool_results: std::sync::Mutex::new(std::collections::HashSet::new()),
            },
            "ws-tool-notify",
        ));
    agent.set_observer(ws_observer);

    // Race: agent.turn vs abort signal
    let result = tokio::select! {
        biased;
        _ = abort_rx.wait_for(|v| *v) => {
            Err(anyhow::anyhow!("aborted"))
        }
        r = agent.turn(message) => r,
    };

    // Put agent back
    agent.set_observer(base_observer);
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(session) = sessions.get_mut(session_key) {
            session.agent = agent;
        }
    }

    result
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
    let store_sessions = if let Some(store) = state.conversation_store.as_ref() {
        store.list_sessions(None).await
    } else if let Some(db) = state.chat_db.as_ref() {
        db.list_sessions("")
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| ConversationSession {
                key: r.key.clone(),
                kind: ConversationKind::Web,
                label: r.label.clone(),
                summary: r.session_summary.clone(),
                current_goal: r.current_goal.clone(),
                #[allow(clippy::cast_sign_loss)]
                created_at: r.created_at as u64,
                #[allow(clippy::cast_sign_loss)]
                last_active: r.last_active as u64,
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                message_count: r.message_count as u32,
                #[allow(clippy::cast_sign_loss)]
                input_tokens: r.input_tokens as u64,
                #[allow(clippy::cast_sign_loss)]
                output_tokens: r.output_tokens as u64,
            })
            .collect()
    } else {
        Vec::new()
    };

    // Also include channel sessions (Matrix, Telegram, etc.) from channel_session_meta.
    let mut all_sessions = store_sessions;
    if let Some(surreal) = state.surreal.as_ref() {
        if let Ok(mut resp) = surreal
            .query(
                "SELECT session_key, message_count, created_at, last_activity
                 FROM channel_session_meta ORDER BY last_activity DESC LIMIT 50",
            )
            .await
        {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            for row in &rows {
                let key = row
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if key.is_empty() {
                    continue;
                }
                let msg_count = row
                    .get("message_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                // Parse ISO datetime strings to epoch seconds
                let parse_ts = |field: &str| -> u64 {
                    row.get(field)
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.timestamp() as u64)
                        .unwrap_or(0)
                };
                let created = parse_ts("created_at");
                let last_active = parse_ts("last_activity");
                all_sessions.push(ConversationSession {
                    key: key.to_string(),
                    kind: ConversationKind::Channel,
                    label: Some(key.to_string()),
                    summary: None,
                    current_goal: None,
                    created_at: created,
                    last_active,
                    #[allow(clippy::cast_possible_truncation)]
                    message_count: msg_count as u32,
                    input_tokens: 0,
                    output_tokens: 0,
                });
            }
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
        let preview = if let Some(store) = state.conversation_store.as_ref() {
            let evts = store.get_events(&s.key, 1).await;
            evts.last().map(|e| truncate_str(&e.content, 60))
        } else if let Some(db) = state.chat_db.as_ref() {
            db.get_messages(&s.key, 1)
                .await
                .ok()
                .and_then(|msgs| msgs.last().map(|m| truncate_str(&m.content, 60)))
        } else {
            None
        };

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

    ensure_session(state, &session_key).await?;

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
            s.agent.clear_history();
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

    // Build summary generator from config
    let (summary_route, provider) = {
        let config_guard = state.config.lock();
        let opts = synapse_providers::provider_runtime_options_from_config(&config_guard);
        let summary_route = resolve_summary_route(&config_guard, &state.model);
        let provider: std::sync::Arc<dyn synapse_providers::Provider> =
            if let Some(ref provider_name) = summary_route.provider {
                let api_key = summary_route
                    .api_key_env
                    .as_deref()
                    .and_then(|env| std::env::var(env).ok())
                    .or_else(|| summary_route.api_key.clone());
                match synapse_providers::create_provider_with_options(
                    provider_name,
                    api_key.as_deref(),
                    &opts,
                ) {
                    Ok(p) => p.into(),
                    Err(e) => {
                        tracing::warn!(
                            %e,
                            summary_route_source = summary_route.source.as_str(),
                            summary_model = summary_route.model.as_str(),
                            "Summary provider init failed; using current route"
                        );
                        state.provider.clone()
                    }
                }
            } else {
                state.provider.clone()
            };
        (summary_route, provider)
    };

    tracing::debug!(
        session_key,
        summary_route_source = summary_route.source.as_str(),
        summary_provider = summary_route.provider.as_deref().unwrap_or("current"),
        summary_model = summary_route.model.as_str(),
        "Web session summary lane selected"
    );

    let generator =
        crate::memory_adapters::summary_generator_adapter::ProviderSummaryGenerator::new(
            provider,
            summary_route.model.clone(),
            summary_route.temperature,
        );

    match synapse_domain::application::services::conversation_service::generate_session_summary(
        store.as_ref(),
        &generator,
        session_key,
        msg_count,
        last_summary,
        prev_summary.as_deref(),
        WEB_SUMMARY_INTERVAL,
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
