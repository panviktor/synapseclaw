//! ClawdTalk voice channel - real-time voice calling via Telnyx SIP infrastructure.
//!
//! ClawdTalk (https://clawdtalk.com) provides AI-powered voice conversations
//! using Telnyx's global SIP network for low-latency, high-quality calls.

use super::traits::{Channel, ChannelMessage, SendMessage};
use crate::realtime_call_ledger::{
    load_persisted_transport_call_sessions, replace_persisted_transport_call_sessions,
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock as ParkingRwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use synapse_domain::application::services::realtime_call_prompt_service::{
    resolve_realtime_call_objective, resolve_realtime_call_prompt,
};
use synapse_domain::application::services::realtime_call_session_service::{
    active_realtime_call_sessions, cleanup_stale_realtime_call_sessions,
    record_realtime_call_ended, record_realtime_call_state, record_realtime_inbound_call_message,
    record_realtime_inbound_call_started, record_realtime_outbound_call_started,
    trim_recent_realtime_call_sessions,
};
#[cfg(test)]
use synapse_domain::ports::realtime_call::RealtimeCallTriggerSource;
use synapse_domain::ports::realtime_call::{
    RealtimeCallActionResult, RealtimeCallAnswerRequest, RealtimeCallDirection,
    RealtimeCallHangupRequest, RealtimeCallKind, RealtimeCallOrigin, RealtimeCallRuntimePort,
    RealtimeCallSessionSnapshot, RealtimeCallSpeakRequest, RealtimeCallStartRequest,
    RealtimeCallStartResult, RealtimeCallState,
};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// ClawdTalk channel configuration
pub struct ClawdTalkChannel {
    /// Telnyx API key for authentication
    api_key: String,
    /// ClawdTalk outbound WebSocket endpoint for transcript/response bridging
    websocket_url: Option<String>,
    /// ClawdTalk REST API base URL for outbound calls and call lifecycle actions
    api_base_url: Option<String>,
    /// Optional assistant id advertised to ClawdTalk
    assistant_id: Option<String>,
    /// Telnyx connection ID (SIP connection)
    connection_id: String,
    /// Phone number or SIP URI to call from
    from_number: String,
    /// Allowed destination numbers/patterns
    allowed_destinations: Vec<String>,
    /// HTTP client for Telnyx API
    client: Client,
    /// Webhook secret for verifying incoming calls
    webhook_secret: Option<String>,
    /// Telnyx answering-machine detection mode
    answering_machine_detection_mode: Option<String>,
    /// Telnyx Call Control voice for speak actions
    speak_voice: String,
    /// Telnyx Call Control language for speak actions
    speak_language: String,
    /// Telnyx Call Control service level for speak actions
    speak_service_level: String,
    /// Telnyx AI conversation voice
    ai_voice: String,
    /// Telnyx AI conversation speed
    ai_speed: f32,
    /// Active ClawdTalk WebSocket writer used to respond to inbound call turns.
    websocket_tx: std::sync::Arc<RwLock<Option<mpsc::Sender<ClawdTalkSocketOutgoing>>>>,
}

// Re-export from synapse_domain::config — single source of truth.
pub use synapse_domain::config::adapter_configs::ClawdTalkConfig;

const MAX_CLAWDTALK_BRIDGE_EVENTS: usize = 50;
const MAX_CLAWDTALK_RECENT_SESSIONS: usize = 50;
const CLOWDTALK_CALL_IDLE_TIMEOUT_SECS: i64 = 15 * 60;

/// Process-local ClawdTalk bridge status. The running channel updates this
/// status; gateway handlers in the same daemon can expose it without logging
/// call transcript text.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClawdTalkBridgeStatus {
    pub configured: bool,
    pub api_key_configured: bool,
    pub websocket_configured: bool,
    pub websocket_url: Option<String>,
    pub api_base_url: Option<String>,
    pub assistant_configured: bool,
    pub bridge_ready: bool,
    pub outbound_start_ready: bool,
    pub call_control_ready: bool,
    pub connected: bool,
    pub last_connected_at: Option<String>,
    pub last_disconnected_at: Option<String>,
    pub last_error: Option<String>,
    pub reconnect_attempts: u64,
    pub active_calls: Vec<RealtimeCallSessionSnapshot>,
    pub recent_sessions: Vec<RealtimeCallSessionSnapshot>,
    pub recent_events: Vec<ClawdTalkBridgeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClawdTalkBridgeEventKind {
    Configured,
    ConnectAttempt,
    Connected,
    Disconnected,
    Error,
    CallStarted,
    CallMessage,
    CallEnded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClawdTalkBridgeEvent {
    pub at: String,
    pub kind: ClawdTalkBridgeEventKind,
    pub call_id: Option<String>,
    pub sequence: Option<u64>,
    pub detail: Option<String>,
}

pub fn clawdtalk_bridge_status(config: Option<&ClawdTalkConfig>) -> ClawdTalkBridgeStatus {
    let mut status = clawdtalk_bridge_status_slot().write();
    load_clawdtalk_recent_sessions_from_ledger(&mut status);
    refresh_clawdtalk_active_calls(&mut status);
    match config {
        Some(config) => apply_clawdtalk_config_to_status(&mut status, config),
        None => {
            status.configured = false;
            status.api_key_configured = false;
            status.websocket_configured = false;
            status.websocket_url = None;
            status.api_base_url = None;
            status.assistant_configured = false;
            status.bridge_ready = false;
            status.outbound_start_ready = false;
            status.call_control_ready = false;
        }
    }
    status.clone()
}

pub fn clawdtalk_recent_sessions() -> Vec<RealtimeCallSessionSnapshot> {
    let mut status = clawdtalk_bridge_status_slot().write();
    load_clawdtalk_recent_sessions_from_ledger(&mut status);
    refresh_clawdtalk_active_calls(&mut status);
    status.recent_sessions.clone()
}

pub fn clawdtalk_session(call_control_id: &str) -> Option<RealtimeCallSessionSnapshot> {
    if let Some(session) = clawdtalk_bridge_status_slot()
        .read()
        .recent_sessions
        .iter()
        .find(|session| session.call_control_id == call_control_id)
        .cloned()
    {
        return Some(session);
    }
    clawdtalk_recent_sessions()
        .into_iter()
        .find(|session| session.call_control_id == call_control_id)
}

pub fn clawdtalk_set_call_state_for_reply_target(
    reply_target: &str,
    state: RealtimeCallState,
) -> bool {
    let Some(call_id) = ClawdTalkChannel::call_id_from_reply_target(reply_target) else {
        return false;
    };
    record_clawdtalk_call_state(call_id, state);
    true
}

pub fn clawdtalk_call_session_for_reply_target(
    reply_target: &str,
) -> Option<RealtimeCallSessionSnapshot> {
    let call_id = ClawdTalkChannel::call_id_from_reply_target(reply_target)?;
    clawdtalk_session(call_id)
}

fn clawdtalk_bridge_status_slot() -> &'static ParkingRwLock<ClawdTalkBridgeStatus> {
    static STATUS: OnceLock<ParkingRwLock<ClawdTalkBridgeStatus>> = OnceLock::new();
    STATUS.get_or_init(|| ParkingRwLock::new(ClawdTalkBridgeStatus::default()))
}

fn clawdtalk_call_ledger_dir_slot() -> &'static ParkingRwLock<Option<PathBuf>> {
    static SLOT: OnceLock<ParkingRwLock<Option<PathBuf>>> = OnceLock::new();
    SLOT.get_or_init(|| ParkingRwLock::new(None))
}

pub(crate) fn configure_clawdtalk_call_ledger_dir(synapseclaw_dir: Option<PathBuf>) {
    *clawdtalk_call_ledger_dir_slot().write() = synapseclaw_dir;
}

fn clawdtalk_call_ledger_dir() -> Option<PathBuf> {
    clawdtalk_call_ledger_dir_slot().read().clone()
}

fn update_clawdtalk_bridge_status(update: impl FnOnce(&mut ClawdTalkBridgeStatus)) {
    let mut status = clawdtalk_bridge_status_slot().write();
    load_clawdtalk_recent_sessions_from_ledger(&mut status);
    update(&mut status);
}

fn utc_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn bounded_status_detail(value: impl AsRef<str>) -> String {
    const MAX_DETAIL_CHARS: usize = 240;
    let mut detail = value.as_ref().trim().to_string();
    if detail.chars().count() > MAX_DETAIL_CHARS {
        detail = detail.chars().take(MAX_DETAIL_CHARS).collect::<String>();
        detail.push_str("...");
    }
    detail
}

fn push_clawdtalk_bridge_event(
    status: &mut ClawdTalkBridgeStatus,
    kind: ClawdTalkBridgeEventKind,
    call_id: Option<String>,
    sequence: Option<u64>,
    detail: Option<String>,
) {
    status.recent_events.push(ClawdTalkBridgeEvent {
        at: utc_timestamp(),
        kind,
        call_id,
        sequence,
        detail: detail.map(bounded_status_detail),
    });
    if status.recent_events.len() > MAX_CLAWDTALK_BRIDGE_EVENTS {
        let overflow = status.recent_events.len() - MAX_CLAWDTALK_BRIDGE_EVENTS;
        status.recent_events.drain(0..overflow);
    }
}

fn load_clawdtalk_recent_sessions_from_ledger(status: &mut ClawdTalkBridgeStatus) {
    let Some(synapseclaw_dir) = clawdtalk_call_ledger_dir() else {
        return;
    };
    match load_persisted_transport_call_sessions(Some(synapseclaw_dir.as_path()), "clawdtalk") {
        Ok(sessions) => {
            status.recent_sessions = sessions;
        }
        Err(error) => {
            tracing::warn!(error = %error, "failed to load persisted ClawdTalk call sessions");
        }
    }
}

fn persist_clawdtalk_recent_sessions(sessions: &[RealtimeCallSessionSnapshot]) {
    let Some(synapseclaw_dir) = clawdtalk_call_ledger_dir() else {
        return;
    };
    if let Err(error) = replace_persisted_transport_call_sessions(
        Some(synapseclaw_dir.as_path()),
        "clawdtalk",
        sessions,
    ) {
        tracing::warn!(error = %error, "failed to persist ClawdTalk call sessions");
    }
}

fn refresh_clawdtalk_active_calls(status: &mut ClawdTalkBridgeStatus) {
    cleanup_stale_realtime_call_sessions(
        &mut status.recent_sessions,
        chrono::Utc::now(),
        CLOWDTALK_CALL_IDLE_TIMEOUT_SECS,
    );
    status.active_calls = active_realtime_call_sessions(&status.recent_sessions);
}

fn trim_clawdtalk_recent_sessions(status: &mut ClawdTalkBridgeStatus) {
    trim_recent_realtime_call_sessions(&mut status.recent_sessions, MAX_CLAWDTALK_RECENT_SESSIONS);
    refresh_clawdtalk_active_calls(status);
    persist_clawdtalk_recent_sessions(&status.recent_sessions);
}

fn apply_clawdtalk_config_to_status(status: &mut ClawdTalkBridgeStatus, config: &ClawdTalkConfig) {
    let api_key_configured = !config.api_key.trim().is_empty();
    let websocket_url = config
        .websocket_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .map(normalize_clawdtalk_websocket_url);
    let api_base_url = config
        .api_base_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
        .map(normalize_clawdtalk_api_base_url)
        .or_else(|| {
            config
                .websocket_url
                .as_deref()
                .filter(|url| !url.trim().is_empty())
                .map(clawdtalk_api_base_from_websocket_endpoint)
        });

    status.configured = true;
    status.api_key_configured = api_key_configured;
    status.websocket_configured = websocket_url.is_some();
    status.websocket_url = websocket_url;
    status.api_base_url = api_base_url;
    status.assistant_configured = config
        .assistant_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    status.bridge_ready = api_key_configured && status.websocket_configured;
    status.outbound_start_ready = api_key_configured && status.api_base_url.is_some();
    status.call_control_ready = api_key_configured
        && !config.connection_id.trim().is_empty()
        && !config.from_number.trim().is_empty();
}

fn record_clawdtalk_bridge_config(config: &ClawdTalkConfig) {
    update_clawdtalk_bridge_status(|status| {
        let was_configured = status.configured;
        apply_clawdtalk_config_to_status(status, config);
        if !was_configured {
            push_clawdtalk_bridge_event(
                status,
                ClawdTalkBridgeEventKind::Configured,
                None,
                None,
                None,
            );
        }
    });
}

fn record_clawdtalk_bridge_connect_attempt(endpoint: &str) {
    update_clawdtalk_bridge_status(|status| {
        status.websocket_url = Some(endpoint.to_string());
        status.websocket_configured = true;
        status.reconnect_attempts = status.reconnect_attempts.saturating_add(1);
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::ConnectAttempt,
            None,
            None,
            None,
        );
    });
}

fn record_clawdtalk_bridge_connected(endpoint: &str) {
    update_clawdtalk_bridge_status(|status| {
        status.websocket_url = Some(endpoint.to_string());
        status.websocket_configured = true;
        status.connected = true;
        status.last_connected_at = Some(utc_timestamp());
        status.last_error = None;
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::Connected,
            None,
            None,
            None,
        );
    });
}

fn record_clawdtalk_bridge_disconnected(detail: Option<&str>) {
    update_clawdtalk_bridge_status(|status| {
        status.connected = false;
        status.last_disconnected_at = Some(utc_timestamp());
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::Disconnected,
            None,
            None,
            detail.map(ToString::to_string),
        );
    });
}

fn record_clawdtalk_bridge_error(error: &str) {
    let detail = bounded_status_detail(error);
    update_clawdtalk_bridge_status(|status| {
        status.connected = false;
        status.last_error = Some(detail.clone());
        status.last_disconnected_at = Some(utc_timestamp());
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::Error,
            None,
            None,
            Some(detail),
        );
    });
}

fn record_clawdtalk_call_started(call_id: &str) {
    update_clawdtalk_bridge_status(|status| {
        record_realtime_inbound_call_started(
            &mut status.recent_sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            call_id,
        );
        trim_clawdtalk_recent_sessions(status);
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::CallStarted,
            Some(call_id.to_string()),
            None,
            None,
        );
    });
}

fn record_clawdtalk_call_message(
    call_id: &str,
    caller: Option<&str>,
    sequence: Option<u64>,
    is_interruption: bool,
) {
    update_clawdtalk_bridge_status(|status| {
        record_realtime_inbound_call_message(
            &mut status.recent_sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            call_id,
            sequence,
            is_interruption,
        );
        let _ = caller;
        trim_clawdtalk_recent_sessions(status);
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::CallMessage,
            Some(call_id.to_string()),
            sequence,
            None,
        );
    });
}

fn record_clawdtalk_call_state(call_id: &str, state: RealtimeCallState) {
    update_clawdtalk_bridge_status(|status| {
        record_realtime_call_state(
            &mut status.recent_sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            call_id,
            RealtimeCallDirection::Unknown,
            state,
        );
        trim_clawdtalk_recent_sessions(status);
    });
}

fn record_clawdtalk_call_ended_with_reason(call_id: &str, end_reason: &'static str) {
    update_clawdtalk_bridge_status(|status| {
        record_realtime_call_ended(
            &mut status.recent_sessions,
            call_id,
            Some(end_reason),
            None,
            &[],
        );
        trim_clawdtalk_recent_sessions(status);
        push_clawdtalk_bridge_event(
            status,
            ClawdTalkBridgeEventKind::CallEnded,
            Some(call_id.to_string()),
            None,
            None,
        );
    });
}

fn record_clawdtalk_call_ended(call_id: &str) {
    record_clawdtalk_call_ended_with_reason(call_id, "remote_ended");
}

fn record_clawdtalk_outbound_call_started(
    session: &CallSession,
    origin: RealtimeCallOrigin,
    objective: Option<String>,
) {
    update_clawdtalk_bridge_status(|status| {
        record_realtime_outbound_call_started(
            &mut status.recent_sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            &session.call_control_id,
            Some(session.call_leg_id.as_str()),
            Some(session.call_session_id.as_str()),
            origin,
            objective,
        );
        trim_clawdtalk_recent_sessions(status);
    });
}

impl ClawdTalkChannel {
    /// Create a new ClawdTalk channel
    pub fn new(config: ClawdTalkConfig) -> Self {
        Self::new_with_synapseclaw_dir(config, None)
    }

    pub fn new_with_synapseclaw_dir(
        config: ClawdTalkConfig,
        synapseclaw_dir: Option<PathBuf>,
    ) -> Self {
        configure_clawdtalk_call_ledger_dir(synapseclaw_dir);
        record_clawdtalk_bridge_config(&config);
        Self {
            api_key: config.api_key,
            websocket_url: config.websocket_url,
            api_base_url: config.api_base_url,
            assistant_id: config.assistant_id,
            connection_id: config.connection_id,
            from_number: config.from_number,
            allowed_destinations: config.allowed_destinations,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            webhook_secret: config.webhook_secret,
            answering_machine_detection_mode: config.answering_machine_detection_mode,
            speak_voice: config.speak_voice,
            speak_language: config.speak_language,
            speak_service_level: config.speak_service_level,
            ai_voice: config.ai_voice,
            ai_speed: config.ai_speed,
            websocket_tx: std::sync::Arc::new(RwLock::new(None)),
        }
    }

    /// Telnyx API base URL
    const TELNYX_API_URL: &'static str = "https://api.telnyx.com/v2";
    const CALL_REPLY_TARGET_PREFIX: &'static str = "clawdtalk-call:";

    /// Check if a destination is allowed
    fn is_destination_allowed(&self, destination: &str) -> bool {
        if self.allowed_destinations.is_empty() {
            return true;
        }
        self.allowed_destinations.iter().any(|pattern| {
            pattern == "*" || destination.starts_with(pattern) || pattern == destination
        })
    }

    fn call_reply_target(call_id: &str) -> String {
        format!("{}{call_id}", Self::CALL_REPLY_TARGET_PREFIX)
    }

    fn call_id_from_reply_target(recipient: &str) -> Option<&str> {
        recipient
            .strip_prefix(Self::CALL_REPLY_TARGET_PREFIX)
            .filter(|call_id| !call_id.trim().is_empty())
    }

    fn websocket_endpoint(&self) -> Option<String> {
        self.websocket_url
            .as_deref()
            .map(normalize_clawdtalk_websocket_url)
    }

    fn clawdtalk_api_base_url(&self) -> Option<String> {
        self.api_base_url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .map(normalize_clawdtalk_api_base_url)
            .or_else(|| {
                self.websocket_url
                    .as_deref()
                    .filter(|url| !url.trim().is_empty())
                    .map(clawdtalk_api_base_from_websocket_endpoint)
            })
    }

    async fn send_call_response(&self, call_id: &str, text: &str) -> anyhow::Result<()> {
        let message = ClawdTalkSocketOutgoing::Response {
            call_id: call_id.to_string(),
            text: text.to_string(),
        };
        let tx = self.websocket_tx.read().await.clone();
        let Some(tx) = tx else {
            anyhow::bail!("ClawdTalk WebSocket is not connected");
        };
        tx.send(message)
            .await
            .map_err(|_| anyhow::anyhow!("ClawdTalk WebSocket writer is closed"))?;
        record_clawdtalk_call_state(call_id, RealtimeCallState::Speaking);
        Ok(())
    }

    async fn listen_websocket(
        &self,
        tx: mpsc::Sender<ChannelMessage>,
        endpoint: String,
    ) -> anyhow::Result<()> {
        let mut retry_delay = std::time::Duration::from_secs(1);
        loop {
            if tx.is_closed() {
                return Ok(());
            }
            match self
                .run_websocket_session(tx.clone(), endpoint.clone())
                .await
            {
                Ok(()) => retry_delay = std::time::Duration::from_secs(1),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        retry_in_ms = retry_delay.as_millis(),
                        "ClawdTalk WebSocket session failed"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(30));
                }
            }
        }
    }

    async fn run_websocket_session(
        &self,
        tx: mpsc::Sender<ChannelMessage>,
        endpoint: String,
    ) -> anyhow::Result<()> {
        record_clawdtalk_bridge_connect_attempt(&endpoint);
        let request = self.websocket_request(&endpoint)?;
        let (socket, _) = match tokio_tungstenite::connect_async(request).await {
            Ok(socket) => socket,
            Err(error) => {
                record_clawdtalk_bridge_error(&format!("connect failed: {error}"));
                return Err(error.into());
            }
        };
        tracing::info!(endpoint = %endpoint, "ClawdTalk WebSocket connected");
        record_clawdtalk_bridge_connected(&endpoint);
        let (mut write, mut read) = socket.split();
        let (out_tx, mut out_rx) = mpsc::channel::<ClawdTalkSocketOutgoing>(32);
        {
            let mut active = self.websocket_tx.write().await;
            *active = Some(out_tx);
        }

        let writer = tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                match message {
                    ClawdTalkSocketOutgoing::Response { call_id, text } => {
                        let payload = serde_json::to_string(&ClawdTalkSocketResponse::Response {
                            call_id,
                            text,
                        })?;
                        write.send(WsMessage::Text(payload.into())).await?;
                    }
                    ClawdTalkSocketOutgoing::Pong(payload) => {
                        write.send(WsMessage::Pong(payload.into())).await?;
                    }
                }
            }
            anyhow::Ok(())
        });

        let session_result = async {
            while let Some(frame) = read.next().await {
                match frame? {
                    WsMessage::Text(text) => {
                        self.handle_socket_text(&tx, text.as_ref()).await?;
                    }
                    WsMessage::Ping(payload) => {
                        let active = self.websocket_tx.read().await.clone();
                        if let Some(active) = active {
                            let _ = active
                                .send(ClawdTalkSocketOutgoing::Pong(payload.to_vec()))
                                .await;
                        }
                        tracing::trace!(bytes = payload.len(), "ClawdTalk WebSocket ping received");
                    }
                    WsMessage::Close(frame) => {
                        tracing::info!(?frame, "ClawdTalk WebSocket closed");
                        break;
                    }
                    WsMessage::Binary(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => {}
                }
            }
            anyhow::Ok(())
        }
        .await;

        {
            let mut active = self.websocket_tx.write().await;
            *active = None;
        }
        writer.abort();
        match &session_result {
            Ok(()) => record_clawdtalk_bridge_disconnected(Some("websocket session closed")),
            Err(error) => record_clawdtalk_bridge_error(&format!("session failed: {error}")),
        }
        session_result
    }

    fn websocket_request(
        &self,
        endpoint: &str,
    ) -> anyhow::Result<tokio_tungstenite::tungstenite::http::Request<()>> {
        let uri = tokio_tungstenite::tungstenite::http::Uri::try_from(endpoint)
            .map_err(|error| anyhow::anyhow!("invalid ClawdTalk WebSocket URL: {error}"))?;
        let host = uri
            .host()
            .ok_or_else(|| anyhow::anyhow!("ClawdTalk WebSocket URL is missing host"))?;
        let mut builder = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(endpoint)
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header("User-Agent", "synapseclaw-clawdtalk/1");
        if !self.api_key.trim().is_empty() {
            builder = builder.header("Authorization", format!("Bearer {}", self.api_key.trim()));
        }
        if let Some(assistant_id) = self.assistant_id.as_deref() {
            if !assistant_id.trim().is_empty() {
                builder = builder.header("X-Assistant-Id", assistant_id.trim());
            }
        }
        builder
            .body(())
            .map_err(|error| anyhow::anyhow!("invalid ClawdTalk WebSocket request: {error}"))
    }

    async fn handle_socket_text(
        &self,
        tx: &mpsc::Sender<ChannelMessage>,
        text: &str,
    ) -> anyhow::Result<()> {
        let incoming = match serde_json::from_str::<ClawdTalkSocketIncoming>(text) {
            Ok(incoming) => incoming,
            Err(error) => {
                record_clawdtalk_bridge_error(&format!("invalid websocket event: {error}"));
                return Err(error.into());
            }
        };
        match incoming {
            ClawdTalkSocketIncoming::Message {
                call_id,
                text,
                timestamp,
                sequence,
                is_interruption,
                caller,
            } => {
                if text.trim().is_empty() {
                    return Ok(());
                }
                let is_interruption = is_interruption.unwrap_or(false);
                record_clawdtalk_call_message(
                    &call_id,
                    caller.as_deref(),
                    sequence,
                    is_interruption,
                );
                let message = ChannelMessage {
                    id: clawdtalk_message_id(&call_id, sequence, timestamp.as_deref()),
                    sender: caller.unwrap_or_else(|| call_id.clone()),
                    reply_target: Self::call_reply_target(&call_id),
                    content: text,
                    channel: "clawdtalk".into(),
                    timestamp: chrono::Utc::now().timestamp().max(0) as u64,
                    thread_ts: Some(call_id),
                    media_attachments: Vec::new(),
                };
                if is_interruption {
                    tracing::debug!(
                        message_id = %message.id,
                        "ClawdTalk marked inbound turn as interruption"
                    );
                }
                tx.send(message).await.map_err(|error| {
                    anyhow::anyhow!("failed to enqueue ClawdTalk turn: {error}")
                })?;
            }
            ClawdTalkSocketIncoming::CallStarted { call_id } => {
                record_clawdtalk_call_started(&call_id);
                tracing::info!(%call_id, "ClawdTalk call started");
            }
            ClawdTalkSocketIncoming::CallEnded { call_id } => {
                record_clawdtalk_call_ended(&call_id);
                tracing::info!(%call_id, "ClawdTalk call ended");
            }
            ClawdTalkSocketIncoming::Unknown => {}
        }
        Ok(())
    }

    /// Initiate an outbound call via Telnyx
    pub async fn initiate_call(
        &self,
        to: &str,
        _prompt: Option<&str>,
        origin: RealtimeCallOrigin,
        objective: Option<String>,
    ) -> anyhow::Result<CallSession> {
        if self.connection_id.trim().is_empty() {
            anyhow::bail!("channels_config.clawdtalk.connection_id is required for Telnyx call-control actions");
        }
        if self.from_number.trim().is_empty() {
            anyhow::bail!(
                "channels_config.clawdtalk.from_number is required for Telnyx call-control actions"
            );
        }
        if !self.is_destination_allowed(to) {
            anyhow::bail!("Destination {} is not in allowed list", to);
        }

        let request = CallRequest {
            connection_id: self.connection_id.clone(),
            to: to.to_string(),
            from: self.from_number.clone(),
            answering_machine_detection: self
                .answering_machine_detection_mode
                .as_ref()
                .map(|mode| AnsweringMachineDetection { mode: mode.clone() }),
            webhook_url: None,
            // AI voice settings via Telnyx Call Control
            command_id: None,
        };

        let response = self
            .client
            .post(format!("{}/calls", Self::TELNYX_API_URL))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("Failed to initiate call: {}", error);
        }

        let call_response: CallResponse = response.json().await?;
        let session = CallSession {
            call_control_id: call_response.call_control_id,
            call_leg_id: call_response.call_leg_id,
            call_session_id: call_response.call_session_id,
        };
        record_clawdtalk_outbound_call_started(&session, origin, objective);
        Ok(session)
    }

    /// Initiate an outbound call through ClawdTalk's current REST API.
    pub async fn initiate_clawdtalk_call(
        &self,
        to: &str,
        prompt: Option<&str>,
        origin: RealtimeCallOrigin,
        objective: Option<String>,
    ) -> anyhow::Result<CallSession> {
        let Some(api_base_url) = self.clawdtalk_api_base_url() else {
            anyhow::bail!("channels_config.clawdtalk.api_base_url or websocket_url is required for ClawdTalk REST calls");
        };
        if self.api_key.trim().is_empty() {
            anyhow::bail!("channels_config.clawdtalk.api_key is required for ClawdTalk REST calls");
        }
        if !self.is_destination_allowed(to) {
            anyhow::bail!("Destination {} is not in allowed list", to);
        }

        let mut request = serde_json::json!({ "to": to });
        if let Some(prompt) = prompt.filter(|value| !value.trim().is_empty()) {
            request["prompt"] = serde_json::Value::String(prompt.trim().to_string());
        }
        if let Some(assistant_id) = self.assistant_id.as_deref() {
            if !assistant_id.trim().is_empty() {
                request["assistant_id"] =
                    serde_json::Value::String(assistant_id.trim().to_string());
            }
        }

        let response = self
            .client
            .post(format!("{}/calls", api_base_url.trim_end_matches('/')))
            .header("Authorization", format!("Bearer {}", self.api_key.trim()))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("Failed to initiate ClawdTalk call: {}", error);
        }

        let value: serde_json::Value = response.json().await?;
        let session = clawdtalk_call_session_from_response(value)?;
        record_clawdtalk_outbound_call_started(&session, origin, objective);
        Ok(session)
    }

    /// Send audio or TTS to an active call
    pub async fn speak(&self, call_control_id: &str, text: &str) -> anyhow::Result<()> {
        let request = SpeakRequest {
            payload: text.to_string(),
            payload_type: "text".to_string(),
            service_level: self.speak_service_level.clone(),
            voice: self.speak_voice.clone(),
            language: self.speak_language.clone(),
        };

        let response = self
            .client
            .post(format!(
                "{}/calls/{}/actions/speak",
                Self::TELNYX_API_URL,
                call_control_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("Failed to speak: {}", error);
        }

        record_clawdtalk_call_state(call_control_id, RealtimeCallState::Speaking);
        Ok(())
    }

    pub async fn answer(&self, call_control_id: &str) -> anyhow::Result<RealtimeCallState> {
        let Some(session) = clawdtalk_session(call_control_id) else {
            anyhow::bail!("unknown clawdtalk call_control_id `{call_control_id}`");
        };
        if session.state.is_terminal() {
            anyhow::bail!(
                "cannot answer terminal clawdtalk call `{call_control_id}` in state {:?}",
                session.state
            );
        }
        if !matches!(
            session.direction,
            RealtimeCallDirection::Inbound | RealtimeCallDirection::Unknown
        ) {
            anyhow::bail!(
                "answer is only valid for inbound clawdtalk sessions; call `{call_control_id}` is {:?}",
                session.direction
            );
        }

        let next_state = match session.state {
            RealtimeCallState::Created | RealtimeCallState::Ringing => RealtimeCallState::Connected,
            RealtimeCallState::Connected => RealtimeCallState::Listening,
            RealtimeCallState::Listening
            | RealtimeCallState::Thinking
            | RealtimeCallState::Speaking => session.state,
            RealtimeCallState::Ended | RealtimeCallState::Failed => {
                unreachable!("terminal handled above")
            }
        };
        record_clawdtalk_call_state(call_control_id, next_state);

        let session = clawdtalk_session(call_control_id).ok_or_else(|| {
            anyhow::anyhow!("failed to reload clawdtalk call `{call_control_id}` after answer")
        })?;
        Ok(session.state)
    }

    /// Hang up an active call
    pub async fn hangup(&self, call_control_id: &str) -> anyhow::Result<()> {
        let response = self
            .client
            .post(format!(
                "{}/calls/{}/actions/hangup",
                Self::TELNYX_API_URL,
                call_control_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            tracing::warn!("Failed to hangup call: {}", error);
        }

        record_clawdtalk_call_ended_with_reason(call_control_id, "operator_hangup");
        Ok(())
    }

    /// Start AI-powered conversation using Telnyx AI inference
    pub async fn start_ai_conversation(
        &self,
        call_control_id: &str,
        system_prompt: &str,
        model: &str,
    ) -> anyhow::Result<()> {
        let request = AiConversationRequest {
            system_prompt: system_prompt.to_string(),
            model: model.to_string(),
            voice_settings: VoiceSettings {
                voice: self.ai_voice.clone(),
                speed: self.ai_speed,
            },
        };

        let response = self
            .client
            .post(format!(
                "{}/calls/{}/actions/ai_conversation",
                Self::TELNYX_API_URL,
                call_control_id
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            anyhow::bail!("Failed to start AI conversation: {}", error);
        }

        Ok(())
    }
}

#[async_trait]
impl RealtimeCallRuntimePort for ClawdTalkChannel {
    fn channel_name(&self) -> &'static str {
        "clawdtalk"
    }

    fn supports_call_kind(&self, kind: RealtimeCallKind) -> bool {
        matches!(kind, RealtimeCallKind::Audio)
    }

    async fn start_audio_call(
        &self,
        request: RealtimeCallStartRequest,
    ) -> anyhow::Result<RealtimeCallStartResult> {
        let prompt = resolve_realtime_call_prompt(&request);
        let objective = resolve_realtime_call_objective(&request);
        let origin = request.origin.clone();
        let session = if self.clawdtalk_api_base_url().is_some() {
            self.initiate_clawdtalk_call(
                &request.to,
                prompt.as_deref(),
                origin.clone(),
                objective.clone(),
            )
            .await?
        } else {
            self.initiate_call(
                &request.to,
                prompt.as_deref(),
                origin.clone(),
                objective.clone(),
            )
            .await?
        };
        Ok(RealtimeCallStartResult {
            channel: self.channel_name().into(),
            call_control_id: session.call_control_id,
            call_leg_id: session.call_leg_id,
            call_session_id: session.call_session_id,
            state: RealtimeCallState::Ringing,
            origin,
            objective,
        })
    }

    async fn speak(
        &self,
        request: RealtimeCallSpeakRequest,
    ) -> anyhow::Result<RealtimeCallActionResult> {
        self.speak(&request.call_control_id, &request.text).await?;
        Ok(RealtimeCallActionResult {
            channel: self.channel_name().into(),
            call_control_id: request.call_control_id,
            status: "spoken".into(),
            state: RealtimeCallState::Speaking,
        })
    }

    async fn answer(
        &self,
        request: RealtimeCallAnswerRequest,
    ) -> anyhow::Result<RealtimeCallActionResult> {
        let state = self.answer(&request.call_control_id).await?;
        Ok(RealtimeCallActionResult {
            channel: self.channel_name().into(),
            call_control_id: request.call_control_id,
            status: "answered".into(),
            state,
        })
    }

    async fn hangup(
        &self,
        request: RealtimeCallHangupRequest,
    ) -> anyhow::Result<RealtimeCallActionResult> {
        self.hangup(&request.call_control_id).await?;
        Ok(RealtimeCallActionResult {
            channel: self.channel_name().into(),
            call_control_id: request.call_control_id,
            status: "hung_up".into(),
            state: RealtimeCallState::Ended,
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<RealtimeCallSessionSnapshot>> {
        Ok(clawdtalk_recent_sessions())
    }

    async fn get_session(
        &self,
        call_control_id: &str,
    ) -> anyhow::Result<Option<RealtimeCallSessionSnapshot>> {
        Ok(clawdtalk_session(call_control_id))
    }
}

/// Active call session
#[derive(Debug, Clone)]
pub struct CallSession {
    pub call_control_id: String,
    pub call_leg_id: String,
    pub call_session_id: String,
}

/// Telnyx call initiation request
#[derive(Debug, Serialize)]
struct CallRequest {
    connection_id: String,
    to: String,
    from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    answering_machine_detection: Option<AnsweringMachineDetection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    webhook_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnsweringMachineDetection {
    mode: String,
}

/// Telnyx call response
#[derive(Debug, Deserialize)]
struct CallResponse {
    call_control_id: String,
    call_leg_id: String,
    call_session_id: String,
}

/// TTS speak request
#[derive(Debug, Serialize)]
struct SpeakRequest {
    payload: String,
    payload_type: String,
    service_level: String,
    voice: String,
    language: String,
}

/// AI conversation request
#[derive(Debug, Serialize)]
struct AiConversationRequest {
    system_prompt: String,
    model: String,
    voice_settings: VoiceSettings,
}

#[derive(Debug, Serialize)]
struct VoiceSettings {
    voice: String,
    speed: f32,
}

#[async_trait]
impl Channel for ClawdTalkChannel {
    fn name(&self) -> &str {
        "ClawdTalk"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        if let Some(call_id) = Self::call_id_from_reply_target(&message.recipient) {
            return self.send_call_response(call_id, &message.content).await;
        }

        // For ClawdTalk, "send" initiates a call with the message as TTS
        let session = self
            .initiate_call(
                &message.recipient,
                None,
                RealtimeCallOrigin::default(),
                None,
            )
            .await?;

        // Wait for call to be answered, then speak
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        self.speak(&session.call_control_id, &message.content)
            .await?;

        // Give time for TTS to complete before hanging up
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        self.hangup(&session.call_control_id).await?;

        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        if let Some(endpoint) = self.websocket_endpoint() {
            return self.listen_websocket(tx, endpoint).await;
        }

        // ClawdTalk listens for incoming calls via webhooks
        // This would typically be handled by the gateway module
        // For now, we signal that this channel is ready and wait indefinitely
        tracing::info!("ClawdTalk channel listening for incoming calls");

        // Keep the listener alive
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;

            // Check if channel is still open
            if tx.is_closed() {
                break;
            }
        }

        Ok(())
    }

    async fn health_check(&self) -> bool {
        // Verify API key by checking Telnyx number configuration
        let response = self
            .client
            .get(format!("{}/phone_numbers", Self::TELNYX_API_URL))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await;

        match response {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                tracing::warn!("ClawdTalk health check failed: {}", e);
                false
            }
        }
    }
}

fn normalize_clawdtalk_websocket_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if trimmed.starts_with("wss://") || trimmed.starts_with("ws://") {
        trimmed.to_string()
    } else {
        format!("wss://{trimmed}")
    }
}

fn normalize_clawdtalk_api_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

fn clawdtalk_api_base_from_websocket_endpoint(raw: &str) -> String {
    let normalized = normalize_clawdtalk_websocket_url(raw);
    let without_scheme = normalized
        .strip_prefix("wss://")
        .or_else(|| normalized.strip_prefix("ws://"))
        .unwrap_or(normalized.as_str())
        .trim_end_matches('/');
    let scheme = if normalized.starts_with("ws://") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{without_scheme}/v1")
}

fn clawdtalk_call_session_from_response(value: serde_json::Value) -> anyhow::Result<CallSession> {
    let data = value.get("data").unwrap_or(&value);
    let call_id = first_string(data, &["call_id", "id", "call_control_id"])
        .or_else(|| first_string(&value, &["call_id", "id", "call_control_id"]))
        .ok_or_else(|| anyhow::anyhow!("ClawdTalk call response did not include call_id"))?;
    let call_leg_id = first_string(data, &["call_leg_id", "leg_id"]).unwrap_or_default();
    let call_session_id = first_string(data, &["call_session_id", "conversation_id", "session_id"])
        .unwrap_or_else(|| call_id.clone());
    Ok(CallSession {
        call_control_id: call_id,
        call_leg_id,
        call_session_id,
    })
}

fn first_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .map(ToString::to_string)
}

fn clawdtalk_message_id(call_id: &str, sequence: Option<u64>, timestamp: Option<&str>) -> String {
    match (sequence, timestamp) {
        (Some(sequence), _) => format!("clawdtalk:{call_id}:{sequence}"),
        (None, Some(timestamp)) if !timestamp.trim().is_empty() => {
            format!("clawdtalk:{call_id}:{timestamp}")
        }
        _ => format!("clawdtalk:{call_id}:message"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ClawdTalkSocketIncoming {
    Message {
        call_id: String,
        text: String,
        #[serde(default)]
        timestamp: Option<String>,
        #[serde(default)]
        sequence: Option<u64>,
        #[serde(default)]
        is_interruption: Option<bool>,
        #[serde(default, alias = "caller_id")]
        caller: Option<String>,
    },
    CallStarted {
        call_id: String,
    },
    CallEnded {
        call_id: String,
    },
    #[serde(other)]
    Unknown,
}

enum ClawdTalkSocketOutgoing {
    Response { call_id: String, text: String },
    Pong(Vec<u8>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClawdTalkSocketResponse {
    Response { call_id: String, text: String },
}

/// Webhook event from Telnyx for incoming calls
#[derive(Debug, Deserialize)]
pub struct TelnyxWebhookEvent {
    pub data: TelnyxWebhookData,
}

#[derive(Debug, Deserialize)]
pub struct TelnyxWebhookData {
    pub event_type: String,
    pub payload: TelnyxCallPayload,
}

#[derive(Debug, Deserialize)]
pub struct TelnyxCallPayload {
    pub call_control_id: Option<String>,
    pub call_leg_id: Option<String>,
    pub call_session_id: Option<String>,
    pub direction: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub state: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_bridge_status() {
        configure_clawdtalk_call_ledger_dir(None);
        *clawdtalk_bridge_status_slot().write() = ClawdTalkBridgeStatus::default();
    }

    fn test_config() -> ClawdTalkConfig {
        ClawdTalkConfig {
            api_key: "test-key".to_string(),
            websocket_url: None,
            api_base_url: None,
            assistant_id: None,
            connection_id: "test-connection".to_string(),
            from_number: "+15551234567".to_string(),
            allowed_destinations: vec!["+1555".to_string()],
            webhook_secret: None,
            answering_machine_detection_mode: Some("premium".to_string()),
            speak_voice: "female".to_string(),
            speak_language: "en-US".to_string(),
            speak_service_level: "premium".to_string(),
            ai_voice: "alloy".to_string(),
            ai_speed: 1.0,
        }
    }

    #[test]
    fn creates_channel() {
        let channel = ClawdTalkChannel::new(test_config());
        assert_eq!(channel.name(), "ClawdTalk");
        assert_eq!(channel.speak_voice, "female");
        assert_eq!(channel.speak_language, "en-US");
        assert_eq!(channel.speak_service_level, "premium");
        assert_eq!(channel.ai_voice, "alloy");
        assert_eq!(channel.ai_speed, 1.0);
    }

    #[test]
    fn normalizes_public_clawdtalk_websocket_endpoint() {
        assert_eq!(
            normalize_clawdtalk_websocket_url("https://clawdtalk.com"),
            "wss://clawdtalk.com"
        );
        assert_eq!(
            normalize_clawdtalk_websocket_url("clawdtalk.com/v1/ws"),
            "wss://clawdtalk.com/v1/ws"
        );
    }

    #[test]
    fn derives_clawdtalk_rest_api_base_from_websocket_endpoint() {
        assert_eq!(
            clawdtalk_api_base_from_websocket_endpoint("https://clawdtalk.com"),
            "https://clawdtalk.com/v1"
        );
        assert_eq!(
            normalize_clawdtalk_api_base_url("https://clawdtalk.com/v1"),
            "https://clawdtalk.com/v1"
        );
    }

    #[test]
    fn parses_current_clawdtalk_call_response_shapes() {
        let session = clawdtalk_call_session_from_response(serde_json::json!({
            "call_id": "clk_7xK9mP",
            "conversation_id": "conv_123"
        }))
        .unwrap();
        assert_eq!(session.call_control_id, "clk_7xK9mP");
        assert_eq!(session.call_session_id, "conv_123");

        let nested = clawdtalk_call_session_from_response(serde_json::json!({
            "data": {
                "id": "clk_nested"
            }
        }))
        .unwrap();
        assert_eq!(nested.call_control_id, "clk_nested");
        assert_eq!(nested.call_session_id, "clk_nested");
    }

    #[test]
    fn call_reply_target_roundtrips() {
        let target = ClawdTalkChannel::call_reply_target("clk_7xK9mP");
        assert_eq!(target, "clawdtalk-call:clk_7xK9mP");
        assert_eq!(
            ClawdTalkChannel::call_id_from_reply_target(&target),
            Some("clk_7xK9mP")
        );
        assert_eq!(ClawdTalkChannel::call_id_from_reply_target("+1555"), None);
    }

    #[tokio::test]
    async fn websocket_message_becomes_channel_turn() {
        let channel = ClawdTalkChannel::new(test_config());
        let (tx, mut rx) = mpsc::channel(1);

        channel
            .handle_socket_text(
                &tx,
                r#"{
                    "event": "message",
                    "call_id": "clk_7xK9mP",
                    "text": "Can you check my calendar?",
                    "timestamp": "2025-02-01T19:58:00Z",
                    "sequence": 1,
                    "is_interruption": false
                }"#,
            )
            .await
            .unwrap();

        let message = rx.recv().await.unwrap();
        assert_eq!(message.channel, "clawdtalk");
        assert_eq!(message.content, "Can you check my calendar?");
        assert_eq!(message.reply_target, "clawdtalk-call:clk_7xK9mP");
        assert_eq!(message.thread_ts.as_deref(), Some("clk_7xK9mP"));
        assert_eq!(message.id, "clawdtalk:clk_7xK9mP:1");
    }

    #[tokio::test]
    async fn bridge_status_tracks_call_lifecycle_without_transcript_text() {
        reset_bridge_status();
        let mut config = test_config();
        config.websocket_url = Some("https://clawdtalk.com".into());
        let channel = ClawdTalkChannel::new(config.clone());
        let (tx, _rx) = mpsc::channel(1);

        channel
            .handle_socket_text(
                &tx,
                r#"{
                    "event": "call_started",
                    "call_id": "clk_status"
                }"#,
            )
            .await
            .unwrap();
        let status = clawdtalk_bridge_status(Some(&config));
        assert!(status.configured);
        assert!(status.api_key_configured);
        assert!(status.websocket_configured);
        assert_eq!(status.websocket_url.as_deref(), Some("wss://clawdtalk.com"));
        assert_eq!(
            status.api_base_url.as_deref(),
            Some("https://clawdtalk.com/v1")
        );
        assert!(status.bridge_ready);
        assert!(status.outbound_start_ready);
        assert!(status.call_control_ready);
        assert!(status
            .active_calls
            .iter()
            .any(|call| call.call_control_id == "clk_status"));
        assert_eq!(
            status
                .active_calls
                .iter()
                .find(|call| call.call_control_id == "clk_status")
                .unwrap()
                .state,
            RealtimeCallState::Connected
        );

        channel
            .handle_socket_text(
                &tx,
                r#"{
                    "event": "message",
                    "call_id": "clk_status",
                    "caller_id": "+15551234567",
                    "text": "private transcript body must not enter status",
                    "sequence": 9,
                    "is_interruption": true
                }"#,
            )
            .await
            .unwrap();
        let status = clawdtalk_bridge_status(Some(&config));
        let call = status
            .active_calls
            .iter()
            .find(|call| call.call_control_id == "clk_status")
            .unwrap();
        assert_eq!(call.message_count, 1);
        assert_eq!(call.interruption_count, 1);
        assert_eq!(call.last_sequence, Some(9));
        assert_eq!(call.direction, RealtimeCallDirection::Inbound);
        assert_eq!(call.state, RealtimeCallState::Listening);
        let status_json = serde_json::to_string(&status).unwrap();
        assert!(!status_json.contains("private transcript body"));
        assert!(status.recent_events.iter().any(|event| {
            event.kind == ClawdTalkBridgeEventKind::CallMessage && event.sequence == Some(9)
        }));

        channel
            .handle_socket_text(
                &tx,
                r#"{
                    "event": "call_ended",
                    "call_id": "clk_status"
                }"#,
            )
            .await
            .unwrap();
        let status = clawdtalk_bridge_status(Some(&config));
        assert!(!status
            .active_calls
            .iter()
            .any(|call| call.call_control_id == "clk_status"));
        assert!(status
            .recent_sessions
            .iter()
            .any(|call| call.call_control_id == "clk_status"
                && call.state == RealtimeCallState::Ended));
    }

    #[test]
    fn reply_target_state_update_marks_thinking() {
        reset_bridge_status();
        record_clawdtalk_call_started("clk_thinking");
        record_clawdtalk_call_message("clk_thinking", None, Some(1), false);

        assert!(clawdtalk_set_call_state_for_reply_target(
            "clawdtalk-call:clk_thinking",
            RealtimeCallState::Thinking,
        ));

        let session = clawdtalk_session("clk_thinking").expect("session recorded");
        assert_eq!(session.state, RealtimeCallState::Thinking);
        assert!(!clawdtalk_set_call_state_for_reply_target(
            "+15551234567",
            RealtimeCallState::Thinking,
        ));
    }

    #[tokio::test]
    async fn answer_advances_inbound_call_without_network_side_effects() {
        reset_bridge_status();
        let channel = ClawdTalkChannel::new(test_config());
        record_clawdtalk_call_started("clk_answer");

        let result = RealtimeCallRuntimePort::answer(
            &channel,
            RealtimeCallAnswerRequest {
                call_control_id: "clk_answer".into(),
            },
        )
        .await
        .expect("inbound call should be answerable");

        assert_eq!(result.status, "answered");
        assert_eq!(result.state, RealtimeCallState::Listening);
        let session = clawdtalk_session("clk_answer").expect("session recorded");
        assert_eq!(session.direction, RealtimeCallDirection::Inbound);
        assert_eq!(session.state, RealtimeCallState::Listening);
    }

    #[test]
    fn outbound_session_records_assistant_call_objective() {
        reset_bridge_status();
        record_clawdtalk_outbound_call_started(
            &CallSession {
                call_control_id: "clk_briefing".into(),
                call_leg_id: "leg_briefing".into(),
                call_session_id: "sess_briefing".into(),
            },
            RealtimeCallOrigin::chat_request(
                Some("ops-room".into()),
                Some("matrix".into()),
                Some("!ops:example".into()),
                Some("$thread".into()),
            ),
            Some("Call about the morning work plan.".into()),
        );

        let session = clawdtalk_session("clk_briefing").expect("session recorded");
        assert_eq!(session.direction, RealtimeCallDirection::Outbound);
        assert_eq!(
            session.origin.source,
            RealtimeCallTriggerSource::ChatRequest
        );
        assert_eq!(
            session.objective.as_deref(),
            Some("Call about the morning work plan.")
        );
        assert_eq!(session.state, RealtimeCallState::Ringing);
    }

    #[test]
    fn status_read_cleans_up_stale_active_calls() {
        reset_bridge_status();
        update_clawdtalk_bridge_status(|status| {
            status.recent_sessions.push(RealtimeCallSessionSnapshot {
                channel: "clawdtalk".into(),
                kind: RealtimeCallKind::Audio,
                direction: RealtimeCallDirection::Outbound,
                origin: RealtimeCallOrigin::cli_request(),
                objective: Some("Call the operator and ask for a progress update.".into()),
                call_control_id: "clk_stale".into(),
                call_leg_id: None,
                call_session_id: None,
                state: RealtimeCallState::Listening,
                created_at: "2000-01-01T00:00:00Z".into(),
                updated_at: "2000-01-01T00:00:00Z".into(),
                ended_at: None,
                end_reason: None,
                summary: None,
                decisions: Vec::new(),
                message_count: 1,
                interruption_count: 0,
                last_sequence: Some(1),
            });
        });

        let status = clawdtalk_bridge_status(None);
        let session = status
            .recent_sessions
            .iter()
            .find(|session| session.call_control_id == "clk_stale")
            .expect("stale session present");
        assert_eq!(session.state, RealtimeCallState::Failed);
        assert!(session.ended_at.is_some());
        assert!(!status
            .active_calls
            .iter()
            .any(|session| session.call_control_id == "clk_stale"));
    }

    #[test]
    fn status_distinguishes_bridge_and_outbound_runtime_readiness() {
        reset_bridge_status();
        let mut config = test_config();
        config.connection_id.clear();
        config.from_number.clear();
        config.websocket_url = Some("https://clawdtalk.example".into());

        let status = clawdtalk_bridge_status(Some(&config));
        assert!(status.configured);
        assert!(status.api_key_configured);
        assert!(status.websocket_configured);
        assert!(status.bridge_ready);
        assert!(status.outbound_start_ready);
        assert!(!status.call_control_ready);

        let mut config = test_config();
        config.api_key.clear();
        config.websocket_url = Some("https://clawdtalk.example".into());

        let status = clawdtalk_bridge_status(Some(&config));
        assert!(status.configured);
        assert!(!status.api_key_configured);
        assert!(!status.bridge_ready);
        assert!(!status.outbound_start_ready);
        assert!(!status.call_control_ready);
    }

    #[tokio::test]
    async fn websocket_bridge_roundtrips_transcript_and_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket
                .send(WsMessage::Text(
                    serde_json::json!({
                        "event": "message",
                        "call_id": "clk_roundtrip",
                        "text": "What is my next meeting?",
                        "sequence": 7
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            match socket.next().await.unwrap().unwrap() {
                WsMessage::Text(text) => serde_json::from_str::<serde_json::Value>(&text).unwrap(),
                other => panic!("expected text response, got {other:?}"),
            }
        });

        let mut config = test_config();
        config.websocket_url = Some(endpoint);
        let channel = std::sync::Arc::new(ClawdTalkChannel::new(config));
        let (tx, mut rx) = mpsc::channel(1);
        let channel_for_listen = std::sync::Arc::clone(&channel);
        let listen_task = tokio::spawn(async move { channel_for_listen.listen(tx).await });

        let message = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.content, "What is my next meeting?");
        assert_eq!(message.reply_target, "clawdtalk-call:clk_roundtrip");

        channel
            .send(&SendMessage::new(
                "Your next meeting is at 3 PM.",
                message.reply_target,
            ))
            .await
            .unwrap();

        let response = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        listen_task.abort();
        assert_eq!(
            response,
            serde_json::json!({
                "type": "response",
                "call_id": "clk_roundtrip",
                "text": "Your next meeting is at 3 PM."
            })
        );
    }

    #[test]
    fn websocket_response_shape_matches_clawdtalk_contract() {
        let payload = serde_json::to_value(ClawdTalkSocketResponse::Response {
            call_id: "clk_7xK9mP".into(),
            text: "You have three meetings tomorrow.".into(),
        })
        .unwrap();

        assert_eq!(
            payload,
            serde_json::json!({
                "type": "response",
                "call_id": "clk_7xK9mP",
                "text": "You have three meetings tomorrow."
            })
        );
    }

    #[test]
    fn destination_allowed_exact_match() {
        let channel = ClawdTalkChannel::new(test_config());
        assert!(channel.is_destination_allowed("+15559876543"));
        assert!(!channel.is_destination_allowed("+14449876543"));
    }

    #[test]
    fn destination_allowed_wildcard() {
        let mut config = test_config();
        config.allowed_destinations = vec!["*".to_string()];
        let channel = ClawdTalkChannel::new(config);
        assert!(channel.is_destination_allowed("+15559876543"));
        assert!(channel.is_destination_allowed("+14449876543"));
    }

    #[test]
    fn destination_allowed_empty_means_all() {
        let mut config = test_config();
        config.allowed_destinations = vec![];
        let channel = ClawdTalkChannel::new(config);
        assert!(channel.is_destination_allowed("+15559876543"));
        assert!(channel.is_destination_allowed("+14449876543"));
    }

    #[test]
    fn webhook_event_deserializes() {
        let json = r#"{
            "data": {
                "event_type": "call.initiated",
                "payload": {
                    "call_control_id": "call-123",
                    "call_leg_id": "leg-123",
                    "call_session_id": "session-123",
                    "direction": "incoming",
                    "from": "+15551112222",
                    "to": "+15553334444",
                    "state": "ringing"
                }
            }
        }"#;

        let event: TelnyxWebhookEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.data.event_type, "call.initiated");
        assert_eq!(
            event.data.payload.call_control_id,
            Some("call-123".to_string())
        );
        assert_eq!(event.data.payload.from, Some("+15551112222".to_string()));
    }
}
