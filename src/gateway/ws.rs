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

use super::chat_db::{ChatMessageRow, ChatSessionRow};
use super::{AppState, ChatSession};
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
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The sub-protocol we support for the chat WebSocket.
const WS_PROTOCOL: &str = "zeroclaw.v1";

/// Prefix used in `Sec-WebSocket-Protocol` to carry a bearer token.
const BEARER_SUBPROTO_PREFIX: &str = "bearer.";

/// Max sessions kept in memory per token prefix.
const MAX_MEMORY_SESSIONS: usize = 50;

/// Auto-label truncation length.
const AUTO_LABEL_MAX_LEN: usize = 40;

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
    let request = tungstenite::http::Request::builder()
        .uri(&upstream_url)
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
    let token_prefix = if let Some(op) = sid.strip_prefix("op:") {
        format!("{token_prefix}:op:{op}")
    } else {
        token_prefix
    };
    let session_key = format!("web:{token_prefix}:{sid}");

    // Ensure session exists in memory (create agent if needed)
    if let Err(e) = ensure_session(&state, &session_key) {
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
fn ensure_session(state: &AppState, session_key: &str) -> anyhow::Result<()> {
    {
        let sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if sessions.contains_key(session_key) {
            return Ok(());
        }
    }

    // Try loading from DB
    let db_session = state
        .chat_db
        .as_ref()
        .and_then(|db| db.get_session(session_key).ok().flatten());

    let config = state.config.lock().clone();
    let mut agent = crate::agent::Agent::from_config(&config)?;

    let now = Instant::now();
    let now_secs_val = now_secs();

    let (label, msg_count, input_tok, output_tok, current_goal, session_summary) =
        if let Some(ref db_row) = db_session {
            // Replay messages into agent
            if let Some(db) = state.chat_db.as_ref() {
                let messages = db.get_messages(session_key, 200)?;
                replay_messages_into_agent(&mut agent, &messages)?;
            }
            (
                db_row.label.clone(),
                u32::try_from(db_row.message_count).unwrap_or(0),
                u64::try_from(db_row.input_tokens).unwrap_or(0),
                u64::try_from(db_row.output_tokens).unwrap_or(0),
                db_row.current_goal.clone(),
                db_row.session_summary.clone(),
            )
        } else {
            (None, 0, 0, 0, None, None)
        };

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

    // Persist new session to DB if it doesn't exist yet
    if db_session.is_none() {
        if let Some(db) = state.chat_db.as_ref() {
            let _ = db.upsert_session(&ChatSessionRow {
                key: session_key.to_string(),
                label: None,
                current_goal: None,
                session_summary: None,
                created_at: now_secs_val,
                last_active: now_secs_val,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            });
        }
    }

    Ok(())
}

/// Replay persisted messages into an Agent instance (user/assistant/system only).
fn replay_messages_into_agent(
    agent: &mut crate::agent::Agent,
    messages: &[ChatMessageRow],
) -> anyhow::Result<()> {
    use crate::providers::{ChatMessage, ConversationMessage};

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
        "chat.history" => handle_chat_history(params, state, default_session, token_prefix),
        "chat.send" => {
            handle_chat_send_rpc(params, state, default_session, token_prefix, out_tx).await
        }
        "chat.abort" => handle_chat_abort(params, state, default_session, token_prefix),
        "sessions.list" => handle_sessions_list(state, token_prefix),
        "sessions.new" => handle_sessions_new(params, state, token_prefix),
        "sessions.rename" => handle_sessions_rename(params, state, token_prefix),
        "sessions.delete" => handle_sessions_delete(params, state, token_prefix),
        "sessions.reset" => handle_sessions_reset(params, state, token_prefix),
        _ => Err(anyhow::anyhow!("Unknown RPC method: {method}")),
    }
}

/// Verify that a session key belongs to the current token's namespace.
fn check_session_ownership(session_key: &str, token_prefix: &str) -> anyhow::Result<()> {
    let expected_prefix = format!("web:{token_prefix}:");
    if !session_key.starts_with(&expected_prefix) {
        return Err(anyhow::anyhow!("session key does not belong to this token"));
    }
    Ok(())
}

// ── RPC: chat.history ───────────────────────────────────────────────────────

fn handle_chat_history(
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

    // Ensure session is loaded
    ensure_session(state, &session_key)?;

    let messages = state
        .chat_db
        .as_ref()
        .map(|db| db.get_messages(&session_key, limit))
        .transpose()?
        .unwrap_or_default();

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

    let msg_json: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "kind": m.kind,
                "role": m.role,
                "content": m.content,
                "tool_name": m.tool_name,
                "run_id": m.run_id,
                "timestamp": m.timestamp,
                "input_tokens": m.input_tokens,
                "output_tokens": m.output_tokens,
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
    let message = params["message"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'message' param"))?
        .to_string();

    let run_id = uuid::Uuid::new_v4().to_string();

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

    // Persist user message + auto-label
    persist_message(
        state,
        &session_key,
        "user",
        Some("user"),
        &message,
        None,
        None,
    );
    auto_label_if_needed(state, &session_key, &message);

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

    // Run agent turn with abort support
    let result = run_agent_turn_with_abort(state, &session_key, &message, abort_rx).await;

    // Clear run_id + abort_tx, extract usage + persist tool events + push live
    let usage = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(&session_key) {
            s.run_id = None;
            s.abort_tx = None;
            s.last_active = Instant::now();
            // Push tool events to WS (live) + persist to DB
            push_tool_events(out_tx, &session_key, s.agent.history(), history_len_before);
            persist_tool_events(state, &session_key, s.agent.history(), history_len_before);
        }
        sessions
            .get(&session_key)
            .and_then(|s| s.agent.last_turn_usage().cloned())
    };

    match result {
        Ok(response) => {
            persist_message(
                state,
                &session_key,
                "assistant",
                Some("assistant"),
                &response,
                None,
                None,
            );
            persist_increment_count(state, &session_key, 2);
            sync_memory_count(state, &session_key, 2);
            persist_usage(state, &session_key, usage.as_ref());
            update_session_goal(state, &session_key, &message);
            emit_session_event(state, "session.updated", &session_key);
            emit_run_event(state, "session.run_finished", &session_key, &run_id);

            // Fire-and-forget: rolling session summary every N messages
            let st = state.clone();
            let sk = session_key.clone();
            tokio::spawn(async move {
                summarize_session_if_needed(&st, &sk).await;
            });

            Ok(serde_json::json!({
                "run_id": run_id,
                "response": response,
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
                );
                persist_increment_count(state, &session_key, 2);
                sync_memory_count(state, &session_key, 2);
                emit_run_event(state, "session.run_interrupted", &session_key, &run_id);
                return Ok(serde_json::json!({
                    "run_id": run_id,
                    "aborted": true,
                }));
            }
            let sanitized = crate::providers::sanitize_api_error(&msg);
            persist_message(state, &session_key, "error", None, &sanitized, None, None);
            persist_increment_count(state, &session_key, 2);
            sync_memory_count(state, &session_key, 2);
            emit_run_event(state, "session.run_finished", &session_key, &run_id);
            Err(anyhow::anyhow!("{sanitized}"))
        }
    }
}

/// Execute agent.turn() with abort support.
/// Swaps the agent out of sessions lock, runs turn, puts it back.
async fn run_agent_turn_with_abort(
    state: &AppState,
    session_key: &str,
    message: &str,
    mut abort_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<String> {
    // Swap agent out so we don't hold lock across await
    let mut agent = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = sessions
            .get_mut(session_key)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        std::mem::replace(
            &mut session.agent,
            crate::agent::Agent::from_config(&state.config.lock().clone())?,
        )
    };

    // Race: agent.turn vs abort signal
    let result = tokio::select! {
        biased;
        _ = abort_rx.wait_for(|v| *v) => {
            Err(anyhow::anyhow!("aborted"))
        }
        r = agent.turn(message) => r,
    };

    // Put agent back
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

fn handle_sessions_list(state: &AppState, token_prefix: &str) -> anyhow::Result<serde_json::Value> {
    let prefix = format!("web:{token_prefix}:");

    let db_sessions = state
        .chat_db
        .as_ref()
        .map(|db| db.list_sessions(&prefix))
        .transpose()?
        .unwrap_or_default();

    // Enrich with in-memory state (active run, etc.)
    let sessions_lock = state
        .chat_sessions
        .lock()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let sessions: Vec<serde_json::Value> = db_sessions
        .iter()
        .map(|s| {
            let has_active_run = sessions_lock
                .get(&s.key)
                .map_or(false, |ms| ms.run_id.is_some());

            // Get preview from last message
            let preview = state.chat_db.as_ref().and_then(|db| {
                db.get_messages(&s.key, 1)
                    .ok()
                    .and_then(|msgs| msgs.last().map(|m| truncate_str(&m.content, 60)))
            });

            serde_json::json!({
                "key": s.key,
                "label": s.label,
                "last_active": s.last_active,
                "message_count": s.message_count,
                "preview": preview,
                "has_active_run": has_active_run,
                "input_tokens": s.input_tokens,
                "output_tokens": s.output_tokens,
                "current_goal": s.current_goal,
                "session_summary": s.session_summary,
            })
        })
        .collect();

    Ok(serde_json::json!({ "sessions": sessions }))
}

// ── RPC: sessions.new ───────────────────────────────────────────────────────

fn handle_sessions_new(
    params: &serde_json::Value,
    state: &AppState,
    token_prefix: &str,
) -> anyhow::Result<serde_json::Value> {
    let label = params["label"].as_str().map(String::from);
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_key = format!("web:{token_prefix}:{session_id}");

    ensure_session(state, &session_key)?;

    if let Some(ref lbl) = label {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(&session_key) {
            s.label = Some(lbl.clone());
        }
        if let Some(db) = state.chat_db.as_ref() {
            let _ = db.update_session_label(&session_key, lbl);
        }
    }

    Ok(serde_json::json!({
        "session_key": session_key,
        "label": label,
    }))
}

// ── RPC: sessions.rename ────────────────────────────────────────────────────

fn handle_sessions_rename(
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

    if let Some(db) = state.chat_db.as_ref() {
        db.update_session_label(key, label)?;
    }

    emit_session_event(state, "session.updated", key);
    Ok(serde_json::json!({ "ok": true }))
}

// ── RPC: sessions.delete ────────────────────────────────────────────────────

fn handle_sessions_delete(
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

    if let Some(db) = state.chat_db.as_ref() {
        db.delete_session(key)?;
    }

    emit_session_event(state, "session.deleted", key);
    Ok(serde_json::json!({ "ok": true }))
}

// ── RPC: sessions.reset ─────────────────────────────────────────────────────

fn handle_sessions_reset(
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

    if let Some(db) = state.chat_db.as_ref() {
        db.clear_messages(key)?;
    }

    emit_session_event(state, "session.updated", key);
    Ok(serde_json::json!({ "ok": true }))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn persist_message(
    state: &AppState,
    session_key: &str,
    kind: &str,
    role: Option<&str>,
    content: &str,
    tool_name: Option<&str>,
    run_id: Option<&str>,
) {
    if let Some(db) = state.chat_db.as_ref() {
        let msg = ChatMessageRow {
            id: 0,
            session_key: session_key.to_string(),
            kind: kind.to_string(),
            role: role.map(String::from),
            content: content.to_string(),
            tool_name: tool_name.map(String::from),
            run_id: run_id.map(String::from),
            input_tokens: None,
            output_tokens: None,
            timestamp: now_secs(),
        };
        if let Err(e) = db.append_message(&msg) {
            tracing::warn!("chat_db: failed to append message: {e}");
        }
        if let Err(e) = db.touch_session(session_key, now_secs()) {
            tracing::warn!("chat_db: failed to touch session: {e}");
        }
    }
}

/// Increment DB message count for a session.
fn persist_increment_count(state: &AppState, session_key: &str, count: i64) {
    if let Some(db) = state.chat_db.as_ref() {
        for _ in 0..count {
            if let Err(e) = db.increment_message_count(session_key) {
                tracing::warn!("chat_db: failed to increment count: {e}");
                break;
            }
        }
    }
}

/// Persist token usage from a completed turn.
fn persist_usage(
    state: &AppState,
    session_key: &str,
    usage: Option<&crate::providers::traits::TokenUsage>,
) {
    if let Some(u) = usage {
        let input = u.input_tokens.unwrap_or(0) as i64;
        let output = u.output_tokens.unwrap_or(0) as i64;
        if input > 0 || output > 0 {
            if let Some(db) = state.chat_db.as_ref() {
                if let Err(e) = db.add_token_usage(session_key, input, output) {
                    tracing::warn!("chat_db: failed to add token usage: {e}");
                }
            }
            if let Ok(mut sessions) = state.chat_sessions.lock() {
                if let Some(s) = sessions.get_mut(session_key) {
                    s.input_tokens += u.input_tokens.unwrap_or(0);
                    s.output_tokens += u.output_tokens.unwrap_or(0);
                }
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

fn auto_label_if_needed(state: &AppState, session_key: &str, first_message: &str) {
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
    if let Some(db) = state.chat_db.as_ref() {
        let _ = db.update_session_label(session_key, &label);
    }
}

/// Update the session's current_goal from the latest user message.
/// This is a lightweight resume hint — not hidden reasoning.
fn update_session_goal(state: &AppState, session_key: &str, user_message: &str) {
    let goal = truncate_str(user_message, 80);
    {
        if let Ok(mut sessions) = state.chat_sessions.lock() {
            if let Some(s) = sessions.get_mut(session_key) {
                s.current_goal = Some(goal.clone());
            }
        }
    }
    if let Some(db) = state.chat_db.as_ref() {
        let conn = db;
        // Direct update via upsert — only current_goal field
        if let Err(e) = conn.update_session_goal(session_key, &goal) {
            tracing::warn!("chat_db: failed to update goal: {e}");
        }
    }
}

/// Persist tool_call and tool_result events from the agent's history after a turn.
fn persist_tool_events(
    state: &AppState,
    session_key: &str,
    history: &[crate::providers::ConversationMessage],
    start_idx: usize,
) {
    use crate::providers::ConversationMessage;

    for msg in history.iter().skip(start_idx) {
        match msg {
            ConversationMessage::AssistantToolCalls { tool_calls, .. } => {
                for tc in tool_calls {
                    persist_message(
                        state,
                        session_key,
                        "tool_call",
                        Some("assistant"),
                        &format!("{}({})", tc.name, tc.arguments),
                        Some(&tc.name),
                        None,
                    );
                }
            }
            ConversationMessage::ToolResults(results) => {
                for tr in results {
                    persist_message(
                        state,
                        session_key,
                        "tool_result",
                        None,
                        &tr.content,
                        None,
                        None,
                    );
                }
            }
            ConversationMessage::Chat(_) => {} // handled separately by caller
        }
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

/// Push tool events from the agent's history delta via WS before the RPC response.
fn push_tool_events(
    out_tx: &tokio::sync::mpsc::UnboundedSender<String>,
    session_key: &str,
    history: &[crate::providers::ConversationMessage],
    start_idx: usize,
) {
    use crate::providers::ConversationMessage;

    for msg in history.iter().skip(start_idx) {
        match msg {
            ConversationMessage::AssistantToolCalls { tool_calls, .. } => {
                for tc in tool_calls {
                    let evt = serde_json::json!({
                        "type": "tool_call",
                        "session_key": session_key,
                        "tool_name": tc.name,
                        "content": format!("{}({})", tc.name, tc.arguments),
                        "timestamp": now_secs(),
                    });
                    let _ = out_tx.send(evt.to_string());
                }
            }
            ConversationMessage::ToolResults(results) => {
                for tr in results {
                    let evt = serde_json::json!({
                        "type": "tool_result",
                        "session_key": session_key,
                        "content": truncate_str(&tr.content, 500),
                        "timestamp": now_secs(),
                    });
                    let _ = out_tx.send(evt.to_string());
                }
            }
            ConversationMessage::Chat(_) => {}
        }
    }
}

/// Generate a rolling session summary every N messages (fire-and-forget).
///
/// Uses `last_summary_count` instead of modulo to avoid skipping summaries
/// when message count jumps over a multiple (e.g. error drops a message).
pub(crate) async fn summarize_session_if_needed(state: &AppState, session_key: &str) {
    const SUMMARY_INTERVAL: u32 = 10;

    // Try in-memory session first, fall back to chat_db (for channel sessions).
    let (msg_count, last_summary, prev_summary) = {
        let from_memory = state.chat_sessions.lock().ok().and_then(|sessions| {
            sessions.get(session_key).map(|s| {
                (
                    s.message_count,
                    s.last_summary_count,
                    s.session_summary.clone(),
                )
            })
        });
        match from_memory {
            Some(v) => v,
            None => {
                // Fall back to chat_db for channel sessions
                match state
                    .chat_db
                    .as_ref()
                    .and_then(|db| db.get_session(session_key).ok().flatten())
                {
                    Some(row) => (
                        row.message_count.try_into().unwrap_or(0),
                        0,
                        row.session_summary,
                    ),
                    None => return,
                }
            }
        }
    };

    if msg_count < SUMMARY_INTERVAL || msg_count - last_summary < SUMMARY_INTERVAL {
        return;
    }

    // Fetch last 10 messages from DB
    let recent = match state.chat_db.as_ref() {
        Some(db) => match db.get_messages(session_key, 10) {
            Ok(msgs) => msgs,
            Err(_) => return,
        },
        None => return,
    };

    if recent.is_empty() {
        return;
    }

    // Build prompt
    let mut messages_text = String::new();
    for m in &recent {
        let role = m.role.as_deref().unwrap_or(&m.kind);
        use std::fmt::Write;
        let _ = writeln!(messages_text, "{role}: {}", truncate_str(&m.content, 200));
    }

    let prompt = format!(
        "Summarize this conversation in 2-3 sentences. Preserve: key decisions, user goals, open tasks.\n\
         Previous summary: {}\n\n\
         Recent messages:\n{}",
        prev_summary.as_deref().unwrap_or("(none)"),
        messages_text,
    );

    // Read summary config from live config (supports runtime switching)
    let (summary_cfg, config_summary_model, options) = {
        let config_guard = state.config.lock();
        let sc = config_guard.summary.clone();
        let sm = config_guard.summary_model.clone();
        let opts = crate::providers::provider_runtime_options_from_config(&config_guard);
        (sc, sm, opts)
    };

    let model = summary_cfg
        .model
        .as_deref()
        .or(config_summary_model.as_deref())
        .unwrap_or(&state.model)
        .to_string();
    let temperature = summary_cfg.temperature;
    let summary_result = if let Some(ref provider_name) = summary_cfg.provider {
        let api_key = summary_cfg
            .api_key_env
            .as_deref()
            .and_then(|env| std::env::var(env).ok());
        match crate::providers::create_provider_with_options(
            provider_name,
            api_key.as_deref(),
            &options,
        ) {
            Ok(provider) => {
                provider
                    .chat_with_system(None, &prompt, &model, temperature)
                    .await
            }
            Err(e) => {
                tracing::warn!("Summary provider '{provider_name}' failed to init: {e}, falling back to default");
                state
                    .provider
                    .chat_with_system(None, &prompt, &model, temperature)
                    .await
            }
        }
    } else {
        state
            .provider
            .chat_with_system(None, &prompt, &model, temperature)
            .await
    };

    match summary_result {
        Ok(summary) => {
            let summary = truncate_str(&summary, 300);
            // Update in-memory
            if let Ok(mut sessions) = state.chat_sessions.lock() {
                if let Some(s) = sessions.get_mut(session_key) {
                    s.session_summary = Some(summary.clone());
                    s.last_summary_count = s.message_count;
                }
            }
            // Update DB
            if let Some(db) = state.chat_db.as_ref() {
                if let Err(e) = db.update_session_summary(session_key, &summary) {
                    tracing::warn!("chat_db: failed to update session summary: {e}");
                }
            }
            // Notify other tabs/clients about the updated summary
            emit_session_event(state, "session.updated", session_key);
            tracing::debug!("session summary updated for {session_key}");
        }
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
            "zeroclaw.v1, bearer.zc_sub456".parse().unwrap(),
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
            "zeroclaw.v1, bearer.zc_tok, other".parse().unwrap(),
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
}
