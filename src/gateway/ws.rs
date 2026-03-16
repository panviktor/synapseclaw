//! WebSocket agent chat handler with RPC support for session management.
//!
//! Protocol:
//! ```text
//! Client -> Server: {"type":"message","content":"Hello"}                       (legacy)
//! Client -> Server: {"type":"rpc","id":"x","method":"chat.send","params":{}}   (RPC)
//! Server -> Client: {"type":"rpc_response","id":"x","result":{}}               (RPC response)
//! Server -> Client: {"type":"chunk","content":"Hi! "}                          (streaming)
//! Server -> Client: {"type":"tool_call","name":"shell","args":{...}}
//! Server -> Client: {"type":"tool_result","name":"shell","output":"..."}
//! Server -> Client: {"type":"done","full_response":"..."}
//! ```

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

/// Derive token hash prefix for session keys: first 8 hex chars of SHA-256.
fn token_hash_prefix(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(&digest[..4])
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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
    let (mut sender, mut receiver) = socket.split();

    // Derive session key
    let sid = session_id.unwrap_or_else(|| "default".to_string());
    let session_key = format!("web:{token_prefix}:{sid}");

    // Ensure session exists in memory (create agent if needed)
    if let Err(e) = ensure_session(&state, &session_key) {
        let err = serde_json::json!({"type": "error", "message": format!("Failed to initialise session: {e}")});
        let _ = sender.send(Message::Text(err.to_string().into())).await;
        return;
    }

    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        // Parse incoming message
        let parsed: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => {
                let err = serde_json::json!({"type": "error", "message": "Invalid JSON"});
                let _ = sender.send(Message::Text(err.to_string().into())).await;
                continue;
            }
        };

        let msg_type = parsed["type"].as_str().unwrap_or("");

        match msg_type {
            "rpc" => {
                let id = parsed["id"].as_str().unwrap_or("").to_string();
                let method = parsed["method"].as_str().unwrap_or("");
                let params = parsed["params"].clone();

                let result = handle_rpc(method, &params, &state, &token_prefix, &session_key).await;

                let response = match result {
                    Ok(val) => serde_json::json!({
                        "type": "rpc_response",
                        "id": id,
                        "result": val,
                    }),
                    Err(e) => serde_json::json!({
                        "type": "rpc_response",
                        "id": id,
                        "error": e.to_string(),
                    }),
                };
                let _ = sender
                    .send(Message::Text(response.to_string().into()))
                    .await;
            }

            // Legacy "message" type — wrap into chat.send
            "message" => {
                let content = parsed["content"].as_str().unwrap_or("").to_string();
                if content.is_empty() {
                    continue;
                }

                handle_chat_send_streaming(&content, &state, &session_key, &mut sender).await;
            }

            _ => {}
        }
    }

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
) -> anyhow::Result<serde_json::Value> {
    match method {
        "chat.history" => handle_chat_history(params, state, default_session),
        "chat.send" => handle_chat_send_rpc(params, state, default_session).await,
        "chat.abort" => handle_chat_abort(params, state, default_session),
        "sessions.list" => handle_sessions_list(state, token_prefix),
        "sessions.new" => handle_sessions_new(params, state, token_prefix),
        "sessions.rename" => handle_sessions_rename(params, state),
        "sessions.delete" => handle_sessions_delete(params, state),
        "sessions.reset" => handle_sessions_reset(params, state),
        _ => Err(anyhow::anyhow!("Unknown RPC method: {method}")),
    }
}

// ── RPC: chat.history ───────────────────────────────────────────────────────

fn handle_chat_history(
    params: &serde_json::Value,
    state: &AppState,
    default_session: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_key = params["session"]
        .as_str()
        .unwrap_or(default_session)
        .to_string();
    let limit = params["limit"].as_i64().unwrap_or(50);

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
) -> anyhow::Result<serde_json::Value> {
    let session_key = params["session"]
        .as_str()
        .unwrap_or(default_session)
        .to_string();
    let message = params["message"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'message' param"))?
        .to_string();

    let run_id = uuid::Uuid::new_v4().to_string();

    // Store run_id
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(&session_key) {
            s.run_id = Some(run_id.clone());
        }
    }

    // Persist user message to DB
    persist_message(
        state,
        &session_key,
        "user",
        Some("user"),
        &message,
        None,
        None,
    );

    // Auto-label on first message
    auto_label_if_needed(state, &session_key, &message);

    // Run the agent turn
    let result = {
        let agent_result = {
            let mut sessions = state
                .chat_sessions
                .lock()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let session = sessions
                .get_mut(&session_key)
                .ok_or_else(|| anyhow::anyhow!("session not found"))?;
            session.last_active = Instant::now();
            // We need to drop the lock before awaiting
            None::<String> // placeholder
        };
        let _ = agent_result;

        // Take the agent out briefly for the turn
        run_agent_turn(state, &session_key, &message).await
    };

    // Clear run_id
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(&session_key) {
            s.run_id = None;
            s.last_active = Instant::now();
        }
    }

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

            // Update last_active in DB
            if let Some(db) = state.chat_db.as_ref() {
                let _ = db.touch_session(&session_key, now_secs());
            }

            Ok(serde_json::json!({
                "run_id": run_id,
                "response": response,
            }))
        }
        Err(e) => {
            let sanitized = crate::providers::sanitize_api_error(&e.to_string());
            persist_message(state, &session_key, "error", None, &sanitized, None, None);
            Err(anyhow::anyhow!("{sanitized}"))
        }
    }
}

/// Execute agent.turn() without holding the sessions lock.
async fn run_agent_turn(
    state: &AppState,
    session_key: &str,
    message: &str,
) -> anyhow::Result<String> {
    // We can't hold Mutex across await, so we take the agent out, run, put back.
    let mut agent = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let session = sessions
            .get_mut(session_key)
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;

        // Take the agent temporarily
        std::mem::replace(
            &mut session.agent,
            // Placeholder — will be replaced back
            crate::agent::Agent::from_config(&state.config.lock().clone())?,
        )
    };

    let result = agent.turn(message).await;

    // Put the agent back
    {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(session) = sessions.get_mut(session_key) {
            session.agent = agent;
            session.message_count += 2; // user + assistant
        }
    }

    result
}

/// Legacy streaming send — uses the old message protocol.
async fn handle_chat_send_streaming(
    content: &str,
    state: &AppState,
    session_key: &str,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
) {
    // Persist user message
    persist_message(
        state,
        session_key,
        "user",
        Some("user"),
        content,
        None,
        None,
    );
    auto_label_if_needed(state, session_key, content);

    let provider_label = state
        .config
        .lock()
        .default_provider
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let _ = state.event_tx.send(serde_json::json!({
        "type": "agent_start",
        "provider": provider_label,
        "model": state.model,
    }));

    let result = run_agent_turn(state, session_key, content).await;

    match result {
        Ok(response) => {
            persist_message(
                state,
                session_key,
                "assistant",
                Some("assistant"),
                &response,
                None,
                None,
            );

            let done = serde_json::json!({
                "type": "done",
                "full_response": response,
            });
            let _ = sender.send(Message::Text(done.to_string().into())).await;

            let _ = state.event_tx.send(serde_json::json!({
                "type": "agent_end",
                "provider": provider_label,
                "model": state.model,
            }));
        }
        Err(e) => {
            let sanitized = crate::providers::sanitize_api_error(&e.to_string());
            persist_message(state, session_key, "error", None, &sanitized, None, None);

            let err = serde_json::json!({
                "type": "error",
                "message": sanitized,
            });
            let _ = sender.send(Message::Text(err.to_string().into())).await;

            let _ = state.event_tx.send(serde_json::json!({
                "type": "error",
                "component": "ws_chat",
                "message": sanitized,
            }));
        }
    }

    if let Some(db) = state.chat_db.as_ref() {
        let _ = db.touch_session(session_key, now_secs());
    }
}

// ── RPC: chat.abort ─────────────────────────────────────────────────────────

fn handle_chat_abort(
    params: &serde_json::Value,
    state: &AppState,
    default_session: &str,
) -> anyhow::Result<serde_json::Value> {
    let session_key = params["session"].as_str().unwrap_or(default_session);

    let run_id = {
        let mut sessions = state
            .chat_sessions
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(s) = sessions.get_mut(session_key) {
            s.run_id.take()
        } else {
            None
        }
    };

    if let Some(ref rid) = run_id {
        persist_message(
            state,
            session_key,
            "interrupted",
            None,
            "Generation aborted by user",
            None,
            Some(rid),
        );
    }

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
    let session_id = &uuid::Uuid::new_v4().to_string()[..8];
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
) -> anyhow::Result<serde_json::Value> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'key'"))?;
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

    Ok(serde_json::json!({ "ok": true }))
}

// ── RPC: sessions.delete ────────────────────────────────────────────────────

fn handle_sessions_delete(
    params: &serde_json::Value,
    state: &AppState,
) -> anyhow::Result<serde_json::Value> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'key'"))?;

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

    Ok(serde_json::json!({ "ok": true }))
}

// ── RPC: sessions.reset ─────────────────────────────────────────────────────

fn handle_sessions_reset(
    params: &serde_json::Value,
    state: &AppState,
) -> anyhow::Result<serde_json::Value> {
    let key = params["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'key'"))?;

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
        let _ = db.append_message(&msg);
        let _ = db.increment_message_count(session_key);
        let _ = db.touch_session(session_key, now_secs());
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
            .map_or(false, |s| s.label.is_none() && s.message_count == 0)
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
        assert_eq!(a.len(), 8);
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
