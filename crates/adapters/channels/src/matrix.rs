use crate::outbound_media::{local_media_path, resolve_outbound_media_uri};
use crate::realtime_audio_ingress::{
    get_realtime_audio_ingress, pcm16_wav_bytes, register_realtime_audio_ingress,
};
use crate::realtime_call_ledger::{
    load_persisted_transport_call_sessions, replace_persisted_transport_call_sessions,
};
use crate::realtime_calls::{MatrixStatusAuthMode, MatrixStatusAuthSource};
use crate::realtime_turn_engine::{
    DeepgramFluxSessionConfig, DeepgramFluxTurnEngine, RealtimePcm16StreamBatcher,
    RealtimeTurnEngine, RealtimeTurnEngineStatus, RealtimeTurnEvent,
};
use crate::traits::{Channel, ChannelMessage, SendMessage};
use crate::transcription::TranscriptionManager;
use anyhow::Context;
use async_trait::async_trait;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use futures_util::StreamExt;
use livekit::{
    e2ee::{
        EncryptionType as LiveKitEncryptionType, E2eeOptions as LiveKitE2eeOptions,
        key_provider::{
            KeyProvider as LiveKitKeyProvider, KeyProviderOptions as LiveKitKeyProviderOptions,
        },
    },
    options::TrackPublishOptions as LiveKitTrackPublishOptions,
    prelude::{
        ConnectionState as LiveKitConnectionState, LocalAudioTrack as LiveKitLocalAudioTrack,
        LocalTrack as LiveKitLocalTrack, RemoteAudioTrack as LiveKitRemoteAudioTrack,
        RemoteTrack as LiveKitRemoteTrack, Room as LiveKitRoom, RoomEvent as LiveKitRoomEvent,
        RoomOptions as LiveKitRoomOptions, TrackKind as LiveKitTrackKind,
        TrackSource as LiveKitTrackSource,
    },
    webrtc::{
        audio_source::native::NativeAudioSource as LiveKitNativeAudioSource,
        audio_stream::native::NativeAudioStream as LiveKitNativeAudioStream,
        prelude::{
            AudioFrame as LiveKitAudioFrame, AudioSourceOptions as LiveKitAudioSourceOptions,
            RtcAudioSource,
        },
    },
};
use matrix_sdk_base::crypto::CollectStrategy;
use matrix_sdk::{
    attachment::{AttachmentConfig, AttachmentInfo, BaseAudioInfo},
    authentication::matrix::MatrixSession,
    config::SyncSettings,
    encryption::verification::{SasState, VerificationRequestState},
    media::{MediaFormat, MediaRequestParameters},
    ruma::{
        api::client::{receipt::create_receipt, uiaa},
        events::{
            AnyToDeviceEvent, AnyToDeviceEventContent,
            call::member::{
                ActiveFocus, ActiveLivekitFocus, Application, CallApplicationContent,
                CallMemberEventContent, CallMemberStateKey, CallScope, Focus, LeaveReason,
                LivekitFocus,
            },
            direct::DirectUserIdentifier,
            reaction::{OriginalSyncReactionEvent, ReactionEventContent},
            receipt::ReceiptThread,
            relation::{Annotation, Reference, Thread},
            room::{
                message::{
                    AudioMessageEventContent, LocationMessageEventContent, MessageType,
                    OriginalSyncRoomMessageEvent, Relation, RoomMessageEventContent,
                },
                MediaSource,
            },
            rtc::{
                decline::{RtcDeclineEventContent, SyncRtcDeclineEvent},
                notification::{
                    NotificationType, RtcNotificationEventContent, SyncRtcNotificationEvent,
                },
            },
            AnySyncStateEvent, Mentions,
        },
        MilliSecondsSinceUnixEpoch, OwnedEventId, OwnedRoomId, OwnedUserId, UInt,
    },
    Client as MatrixSdkClient, LoopCtrl, Room, RoomState, SessionMeta, SessionTokens,
};
use parking_lot::RwLock as ParkingRwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use synapse_domain::application::services::media_artifact_delivery::artifact_delivery_uri;
use synapse_domain::application::services::media_artifact_delivery::tts_provider_output_format;
use synapse_domain::application::services::realtime_call_prompt_service::{
    default_realtime_call_answer_greeting, resolve_realtime_call_objective,
    resolve_realtime_call_prompt,
};
use synapse_domain::application::services::realtime_call_session_service::{
    active_realtime_call_sessions, cleanup_stale_realtime_call_sessions,
    record_realtime_call_ended, record_realtime_call_state,
    record_realtime_call_state_with_context, record_realtime_outbound_call_started,
    trim_recent_realtime_call_sessions,
};
use synapse_domain::config::schema::TtsConfig;
use synapse_domain::domain::channel::{InboundMediaAttachment, InboundMediaKind};
use synapse_domain::ports::provider::{MediaArtifact, MediaArtifactKind};
use synapse_domain::ports::realtime_call::{
    RealtimeCallActionResult, RealtimeCallAnswerRequest, RealtimeCallDirection,
    RealtimeCallHangupRequest, RealtimeCallKind, RealtimeCallOrigin, RealtimeCallRuntimePort,
    RealtimeCallSessionSnapshot, RealtimeCallSpeakRequest, RealtimeCallStartRequest,
    RealtimeCallStartResult, RealtimeCallState,
};
use tokio::sync::{mpsc, Mutex, OnceCell, RwLock};

/// Default maximum media download size (50 MB).
const DEFAULT_MAX_MEDIA_DOWNLOAD_BYTES: usize = 50 * 1024 * 1024;
const MAX_MATRIX_CALL_CONTROL_EVENTS: usize = 50;
const MAX_MATRIX_CALL_RECENT_SESSIONS: usize = 50;
const MATRIX_CALL_IDLE_TIMEOUT_SECS: i64 = 5 * 60;
const MATRIX_CALL_REPLY_TARGET_PREFIX: &str = "matrix-call:";
const MATRIX_CALL_REPLY_TARGET_SEPARATOR: &str = "||";
const MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE: u32 = 48_000;
const MATRIX_CALL_TRANSCRIPT_CHANNELS: u32 = 1;
const MATRIX_CALL_DEBUG_CAPTURE_MAX_SECS: usize = 30;
const MATRIX_CALL_DEBUG_CAPTURE_MIN_SECS: usize = 2;
const MATRIX_CALL_DEBUG_CAPTURE_KEEP_FILES: usize = 8;
const MATRIX_CALL_ENCRYPTION_EVENT_TYPE: &str = "io.element.call.encryption_keys";
const MATRIX_CALL_ENCRYPTION_KEY_INDEX: i32 = 0;
const MATRIX_CALL_ENCRYPTION_KEY_LEN: usize = 16;

/// Filename for persisted session credentials (access_token + device_id).
const MATRIX_SESSION_FILE: &str = "session.json";

/// Persisted Matrix session credentials, saved after login_username().
/// Allows subsequent runs to restore_session() without re-reading the password.
#[derive(Serialize, Deserialize)]
struct SavedSession {
    access_token: String,
    device_id: String,
    user_id: String,
}

/// Matrix channel for Matrix Client-Server API.
/// Uses matrix-sdk for reliable sync and encrypted-room decryption.
#[derive(Clone)]
pub struct MatrixChannel {
    homeserver: String,
    access_token: Option<String>,
    room_id: String,
    allowed_users: Vec<String>,
    session_owner_hint: Option<String>,
    session_device_id_hint: Option<String>,
    synapseclaw_dir: Option<PathBuf>,
    resolved_room_id_cache: Arc<RwLock<Option<String>>>,
    sdk_client: Arc<OnceCell<MatrixSdkClient>>,
    http_client: Client,
    reaction_events: Arc<RwLock<HashMap<String, String>>>,
    voice_mode: Arc<AtomicBool>,
    transcription: Option<synapse_domain::config::schema::TranscriptionConfig>,
    tts: Option<TtsConfig>,
    voice_transcriptions: Arc<Mutex<std::collections::HashMap<String, String>>>,
    password: Option<String>,
    max_media_bytes: usize,
}

impl std::fmt::Debug for MatrixChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatrixChannel")
            .field("homeserver", &self.homeserver)
            .field("room_id", &self.room_id)
            .field("allowed_users", &self.allowed_users)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixCallControlStatus {
    pub configured: bool,
    pub auth_mode: MatrixStatusAuthMode,
    pub auth_source: MatrixStatusAuthSource,
    pub widget_support_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_room_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_accessible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_encrypted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtc_bootstrap: Option<MatrixRtcBootstrapStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_engine: Option<RealtimeTurnEngineStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_calls: Vec<RealtimeCallSessionSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_sessions: Vec<RealtimeCallSessionSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_events: Vec<MatrixCallControlEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatrixCallControlEventKind {
    Configured,
    RoomReady,
    Error,
    OutgoingRing,
    IncomingRing,
    CallDeclined,
    CallEnded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixCallControlEvent {
    pub at: String,
    pub kind: MatrixCallControlEventKind,
    pub call_id: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatrixRtcFocusSource {
    #[default]
    Unknown,
    RtcTransports,
    WellKnown,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatrixRtcAuthorizerApi {
    #[default]
    Unknown,
    GetToken,
    SfuGet,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatrixRtcBootstrapStatus {
    pub focus_source: MatrixRtcFocusSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transports_api_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transports_api_supported: Option<bool>,
    pub authorizer_api: MatrixRtcAuthorizerApi,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorizer_healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openid_token_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorizer_grant_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub livekit_service_url: Option<String>,
    pub media_bootstrap_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_probe_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MatrixRtcTransportsResponse {
    #[serde(default)]
    rtc_transports: Vec<MatrixRtcTransportDescriptor>,
}

#[derive(Debug, Clone, Deserialize)]
struct MatrixRtcTransportDescriptor {
    #[serde(rename = "type")]
    transport_type: String,
    #[serde(default)]
    livekit_service_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MatrixOpenIdToken {
    access_token: String,
    token_type: String,
    matrix_server_name: String,
    expires_in: u64,
}

#[derive(Debug, Clone, Serialize)]
struct MatrixRtcAuthorizerMember {
    id: String,
    claimed_user_id: String,
    claimed_device_id: String,
}

#[derive(Debug, Clone, Serialize)]
struct MatrixRtcAuthorizerRequest {
    room_id: String,
    slot_id: String,
    openid_token: MatrixOpenIdToken,
    member: MatrixRtcAuthorizerMember,
}

#[derive(Debug, Clone, Serialize)]
struct MatrixRtcLegacyAuthorizerRequest {
    room: String,
    openid_token: MatrixOpenIdToken,
    device_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MatrixRtcAuthorizerGrantResponse {
    url: String,
    jwt: String,
}

#[derive(Debug, Clone)]
struct MatrixRtcAuthorizerGrant {
    api: MatrixRtcAuthorizerApi,
    livekit_service_url: String,
    jwt: String,
}

#[derive(Clone)]
struct MatrixMediaSession {
    room: Arc<LiveKitRoom>,
    media_room_id: String,
    e2ee_key_provider: LiveKitKeyProvider,
    own_user_id: String,
    own_device_id: String,
    own_member_id: String,
    local_media_key: Vec<u8>,
    local_key_index: i32,
    speak_gate: Arc<Mutex<()>>,
    ingress_started: Arc<AtomicBool>,
    playback_epoch: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MatrixCallEncryptionKeyContent {
    index: i32,
    key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MatrixCallEncryptionMemberContent {
    #[serde(default)]
    id: String,
    claimed_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MatrixCallEncryptionSessionContent {
    application: String,
    call_id: String,
    scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MatrixCallEncryptionToDeviceContent {
    keys: MatrixCallEncryptionKeyContent,
    member: MatrixCallEncryptionMemberContent,
    room_id: String,
    session: MatrixCallEncryptionSessionContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent_ts: Option<u64>,
}

#[derive(Debug, Clone)]
struct WavPcm16Payload {
    sample_rate: u32,
    channels: u32,
    duration: Duration,
    waveform: Option<Vec<f32>>,
    samples: Vec<i16>,
}

fn is_livekit_matrix_rtc_transport(transport: &MatrixRtcTransportDescriptor) -> bool {
    matches!(
        transport.transport_type.as_str(),
        "livekit" | "livekit_multi_sfu"
    ) && transport
        .livekit_service_url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

pub fn matrix_call_control_status(
    config: Option<&synapse_domain::config::schema::MatrixConfig>,
) -> MatrixCallControlStatus {
    let mut status = matrix_call_control_status_slot().write();
    load_matrix_recent_sessions_from_ledger(&mut status);
    refresh_matrix_call_active_sessions(&mut status);
    match config {
        Some(config) => apply_matrix_call_config_to_status(&mut status, config),
        None => {
            status.configured = false;
            status.room_reference = None;
            status.resolved_room_id = None;
            status.room_accessible = None;
            status.room_encrypted = None;
        }
    }
    status.clone()
}

pub fn matrix_recent_call_sessions() -> Vec<RealtimeCallSessionSnapshot> {
    let mut status = matrix_call_control_status_slot().write();
    load_matrix_recent_sessions_from_ledger(&mut status);
    refresh_matrix_call_active_sessions(&mut status);
    status.recent_sessions.clone()
}

pub fn matrix_call_session(call_control_id: &str) -> Option<RealtimeCallSessionSnapshot> {
    if let Some(session) = matrix_call_control_status_slot()
        .read()
        .recent_sessions
        .iter()
        .find(|session| session.call_control_id == call_control_id)
        .cloned()
    {
        return Some(session);
    }
    matrix_recent_call_sessions()
        .into_iter()
        .find(|session| session.call_control_id == call_control_id)
}

fn matrix_call_control_status_slot() -> &'static ParkingRwLock<MatrixCallControlStatus> {
    static STATUS: OnceLock<ParkingRwLock<MatrixCallControlStatus>> = OnceLock::new();
    STATUS.get_or_init(|| {
        ParkingRwLock::new(MatrixCallControlStatus {
            widget_support_enabled: true,
            ..MatrixCallControlStatus::default()
        })
    })
}

fn matrix_call_ledger_dir_slot() -> &'static ParkingRwLock<Option<PathBuf>> {
    static SLOT: OnceLock<ParkingRwLock<Option<PathBuf>>> = OnceLock::new();
    SLOT.get_or_init(|| ParkingRwLock::new(None))
}

fn matrix_media_sessions_slot() -> &'static ParkingRwLock<HashMap<String, MatrixMediaSession>> {
    static SLOT: OnceLock<ParkingRwLock<HashMap<String, MatrixMediaSession>>> = OnceLock::new();
    SLOT.get_or_init(|| ParkingRwLock::new(HashMap::new()))
}

pub(crate) fn configure_matrix_call_ledger_dir(synapseclaw_dir: Option<PathBuf>) {
    *matrix_call_ledger_dir_slot().write() = synapseclaw_dir;
}

fn matrix_call_ledger_dir() -> Option<PathBuf> {
    matrix_call_ledger_dir_slot().read().clone()
}

pub(crate) fn matrix_media_attached() -> bool {
    !matrix_media_sessions_slot().read().is_empty()
}

fn matrix_media_session(call_control_id: &str) -> Option<MatrixMediaSession> {
    matrix_media_sessions_slot()
        .read()
        .get(call_control_id)
        .cloned()
}

fn insert_matrix_media_session(call_control_id: &str, session: MatrixMediaSession) {
    matrix_media_sessions_slot()
        .write()
        .insert(call_control_id.to_string(), session);
}

fn remove_matrix_media_session(call_control_id: &str) -> Option<MatrixMediaSession> {
    matrix_media_sessions_slot().write().remove(call_control_id)
}

fn matrix_media_session_exists(call_control_id: &str) -> bool {
    matrix_media_sessions_slot().read().contains_key(call_control_id)
}

fn update_matrix_call_control_status(update: impl FnOnce(&mut MatrixCallControlStatus)) {
    let mut status = matrix_call_control_status_slot().write();
    load_matrix_recent_sessions_from_ledger(&mut status);
    update(&mut status);
}

fn matrix_status_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn bounded_matrix_status_detail(value: impl AsRef<str>) -> String {
    const MAX_DETAIL_CHARS: usize = 240;
    let trimmed = value.as_ref().trim();
    if trimmed.chars().count() <= MAX_DETAIL_CHARS {
        return trimmed.to_string();
    }
    let mut detail = trimmed.chars().take(MAX_DETAIL_CHARS).collect::<String>();
    detail.push_str("...");
    detail
}

fn push_matrix_call_control_event(
    status: &mut MatrixCallControlStatus,
    kind: MatrixCallControlEventKind,
    call_id: Option<String>,
    detail: Option<String>,
) {
    status.recent_events.push(MatrixCallControlEvent {
        at: matrix_status_timestamp(),
        kind,
        call_id,
        detail: detail.map(bounded_matrix_status_detail),
    });
    if status.recent_events.len() > MAX_MATRIX_CALL_CONTROL_EVENTS {
        let overflow = status.recent_events.len() - MAX_MATRIX_CALL_CONTROL_EVENTS;
        status.recent_events.drain(0..overflow);
    }
}

fn load_matrix_recent_sessions_from_ledger(status: &mut MatrixCallControlStatus) {
    let Some(synapseclaw_dir) = matrix_call_ledger_dir() else {
        return;
    };
    match load_persisted_transport_call_sessions(Some(synapseclaw_dir.as_path()), "matrix") {
        Ok(sessions) => {
            status.recent_sessions = sessions;
        }
        Err(error) => {
            tracing::warn!(error = %error, "failed to load persisted Matrix call sessions");
        }
    }
}

fn persist_matrix_recent_sessions(sessions: &[RealtimeCallSessionSnapshot]) {
    let Some(synapseclaw_dir) = matrix_call_ledger_dir() else {
        return;
    };
    if let Err(error) = replace_persisted_transport_call_sessions(
        Some(synapseclaw_dir.as_path()),
        "matrix",
        sessions,
    ) {
        tracing::warn!(error = %error, "failed to persist Matrix call sessions");
    }
}

fn refresh_matrix_call_active_sessions(status: &mut MatrixCallControlStatus) {
    cleanup_stale_realtime_call_sessions(
        &mut status.recent_sessions,
        chrono::Utc::now(),
        MATRIX_CALL_IDLE_TIMEOUT_SECS,
    );
    status.active_calls = active_realtime_call_sessions(&status.recent_sessions);
}

fn trim_matrix_recent_sessions(status: &mut MatrixCallControlStatus) {
    trim_recent_realtime_call_sessions(
        &mut status.recent_sessions,
        MAX_MATRIX_CALL_RECENT_SESSIONS,
    );
    refresh_matrix_call_active_sessions(status);
    persist_matrix_recent_sessions(&status.recent_sessions);
}

fn apply_matrix_call_config_to_status(
    status: &mut MatrixCallControlStatus,
    config: &synapse_domain::config::schema::MatrixConfig,
) {
    status.configured = true;
    status.auth_mode = if config
        .access_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
    {
        MatrixStatusAuthMode::AccessToken
    } else if config
        .password
        .as_deref()
        .is_some_and(|password| !password.trim().is_empty())
    {
        MatrixStatusAuthMode::Password
    } else {
        MatrixStatusAuthMode::Missing
    };
    status.widget_support_enabled = true;
    status.room_reference = Some(config.room_id.trim().to_string());
    if status.rtc_bootstrap.is_none() {
        status.rtc_bootstrap = Some(MatrixRtcBootstrapStatus::default());
    }
}

fn record_matrix_call_auth_source(source: MatrixStatusAuthSource) {
    update_matrix_call_control_status(|status| {
        status.auth_source = source;
    });
}

fn record_matrix_call_control_config(channel: &MatrixChannel) {
    update_matrix_call_control_status(|status| {
        let was_configured = status.configured;
        status.configured = true;
        status.widget_support_enabled = true;
        status.room_reference = Some(channel.room_id.clone());
        status.turn_engine = Some(DeepgramFluxSessionConfig::status_from_transcription(
            channel.transcription.as_ref(),
        ));
        if !was_configured {
            push_matrix_call_control_event(
                status,
                MatrixCallControlEventKind::Configured,
                None,
                None,
            );
        }
    });
}

fn record_matrix_call_room_ready(room_id: &str, encrypted: bool) {
    update_matrix_call_control_status(|status| {
        status.room_accessible = Some(true);
        status.room_encrypted = Some(encrypted);
        status.resolved_room_id = Some(room_id.to_string());
        status.last_error = None;
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::RoomReady,
            None,
            Some(if encrypted {
                "room_accessible encrypted".to_string()
            } else {
                "room_accessible unencrypted".to_string()
            }),
        );
    });
}

fn record_matrix_call_control_error(detail: &str) {
    let detail = bounded_matrix_status_detail(detail);
    update_matrix_call_control_status(|status| {
        status.last_error = Some(detail.clone());
        if status.room_accessible.is_none() {
            status.room_accessible = Some(false);
        }
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::Error,
            None,
            Some(detail),
        );
    });
}

fn record_matrix_rtc_bootstrap_status(bootstrap: MatrixRtcBootstrapStatus) {
    update_matrix_call_control_status(|status| {
        status.rtc_bootstrap = Some(bootstrap);
    });
}

fn record_matrix_turn_engine_error(detail: &str) {
    let detail = bounded_matrix_status_detail(detail);
    update_matrix_call_control_status(|status| {
        let mut engine = status.turn_engine.clone().unwrap_or_default();
        engine.last_error = Some(detail.clone());
        status.turn_engine = Some(engine);
    });
}

fn clear_matrix_turn_engine_error() {
    update_matrix_call_control_status(|status| {
        if let Some(engine) = status.turn_engine.as_mut() {
            engine.last_error = None;
        }
    });
}

fn record_matrix_incoming_ring_context(call_control_id: &str, room_id: &str, sender: &str) {
    update_matrix_call_control_status(|status| {
        record_realtime_call_state_with_context(
            &mut status.recent_sessions,
            "matrix",
            RealtimeCallKind::Audio,
            call_control_id,
            RealtimeCallDirection::Inbound,
            RealtimeCallState::Ringing,
            RealtimeCallOrigin {
                source: synapse_domain::ports::realtime_call::RealtimeCallTriggerSource::InboundTransport,
                conversation_id: Some(room_id.to_string()),
                channel: Some("matrix".into()),
                recipient: Some(sender.to_string()),
                thread_ref: None,
            },
            Some(room_id),
            None,
        );
        trim_matrix_recent_sessions(status);
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::IncomingRing,
            Some(call_control_id.to_string()),
            None,
        );
    });
}

fn latest_active_matrix_incoming_call_id(
    sessions: &[RealtimeCallSessionSnapshot],
    room_id: &str,
    sender: &str,
) -> Option<String> {
    sessions
        .iter()
        .rev()
        .find(|session| {
            session.direction == RealtimeCallDirection::Inbound
                && !session.state.is_terminal()
                && session.call_session_id.as_deref() == Some(room_id)
                && session.origin.recipient.as_deref() == Some(sender)
        })
        .map(|session| session.call_control_id.clone())
}

fn record_matrix_incoming_membership_context(
    event_id: &str,
    room_id: &str,
    sender: &str,
) -> (String, bool) {
    let mut call_control_id = event_id.to_string();
    let mut is_new = true;

    update_matrix_call_control_status(|status| {
        if let Some(existing_call_id) =
            latest_active_matrix_incoming_call_id(&status.recent_sessions, room_id, sender)
        {
            call_control_id = existing_call_id;
            is_new = false;
        }

        record_realtime_call_state_with_context(
            &mut status.recent_sessions,
            "matrix",
            RealtimeCallKind::Audio,
            &call_control_id,
            RealtimeCallDirection::Inbound,
            RealtimeCallState::Ringing,
            RealtimeCallOrigin {
                source:
                    synapse_domain::ports::realtime_call::RealtimeCallTriggerSource::InboundTransport,
                conversation_id: Some(room_id.to_string()),
                channel: Some("matrix".into()),
                recipient: Some(sender.to_string()),
                thread_ref: None,
            },
            Some(room_id),
            None,
        );
        trim_matrix_recent_sessions(status);
        if is_new {
            push_matrix_call_control_event(
                status,
                MatrixCallControlEventKind::IncomingRing,
                Some(call_control_id.clone()),
                Some("org.matrix.msc3401.call.member".to_string()),
            );
        }
    });

    (call_control_id, is_new)
}

fn record_matrix_outgoing_ring(
    call_control_id: &str,
    call_leg_id: Option<&str>,
    room_id: &str,
    origin: RealtimeCallOrigin,
    objective: Option<String>,
) {
    update_matrix_call_control_status(|status| {
        record_realtime_outbound_call_started(
            &mut status.recent_sessions,
            "matrix",
            RealtimeCallKind::Audio,
            call_control_id,
            call_leg_id,
            Some(room_id),
            origin,
            objective,
        );
        trim_matrix_recent_sessions(status);
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::OutgoingRing,
            Some(call_control_id.to_string()),
            None,
        );
    });
}

fn record_matrix_call_declined(call_control_id: &str) {
    update_matrix_call_control_status(|status| {
        record_realtime_call_ended(
            &mut status.recent_sessions,
            call_control_id,
            Some("remote_declined"),
            None,
            &[],
        );
        trim_matrix_recent_sessions(status);
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::CallDeclined,
            Some(call_control_id.to_string()),
            None,
        );
    });
}

fn record_matrix_call_state(call_control_id: &str, state: RealtimeCallState) {
    update_matrix_call_control_status(|status| {
        let direction = status
            .recent_sessions
            .iter()
            .find(|session| session.call_control_id == call_control_id)
            .map(|session| session.direction)
            .unwrap_or(RealtimeCallDirection::Unknown);
        record_realtime_call_state(
            &mut status.recent_sessions,
            "matrix",
            RealtimeCallKind::Audio,
            call_control_id,
            direction,
            state,
        );
        trim_matrix_recent_sessions(status);
    });
}

fn record_matrix_call_ended_with_reason(call_control_id: &str, end_reason: &str) {
    update_matrix_call_control_status(|status| {
        record_realtime_call_ended(
            &mut status.recent_sessions,
            call_control_id,
            Some(end_reason),
            None,
            &[],
        );
        trim_matrix_recent_sessions(status);
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::CallEnded,
            Some(call_control_id.to_string()),
            Some(end_reason.to_string()),
        );
    });
}

fn record_matrix_call_ended_if_active(call_control_id: &str, end_reason: &str) {
    let active = matrix_call_session(call_control_id)
        .map(|session| !session.state.is_terminal())
        .unwrap_or(false);
    if active {
        record_matrix_call_ended_with_reason(call_control_id, end_reason);
    }
}

fn record_matrix_call_ended_for_sender_room(
    room_id: &str,
    sender: &str,
    end_reason: &str,
) -> Option<String> {
    let mut call_control_id = None;
    update_matrix_call_control_status(|status| {
        let Some(active_call_id) =
            latest_active_matrix_incoming_call_id(&status.recent_sessions, room_id, sender)
        else {
            return;
        };
        record_realtime_call_ended(
            &mut status.recent_sessions,
            &active_call_id,
            Some(end_reason),
            None,
            &[],
        );
        trim_matrix_recent_sessions(status);
        push_matrix_call_control_event(
            status,
            MatrixCallControlEventKind::CallEnded,
            Some(active_call_id.clone()),
            Some(end_reason.to_string()),
        );
        call_control_id = Some(active_call_id);
    });
    call_control_id
}

fn matrix_call_reply_target(sender: &str, room_id: &str) -> String {
    format!(
        "{MATRIX_CALL_REPLY_TARGET_PREFIX}{sender}{MATRIX_CALL_REPLY_TARGET_SEPARATOR}{room_id}"
    )
}

fn parse_matrix_call_reply_target(reply_target: &str) -> Option<(&str, &str)> {
    let reply_target = reply_target.strip_prefix(MATRIX_CALL_REPLY_TARGET_PREFIX)?;
    let (sender, room_id) = reply_target.split_once(MATRIX_CALL_REPLY_TARGET_SEPARATOR)?;
    let sender = sender.trim();
    let room_id = room_id.trim();
    if sender.is_empty() || room_id.is_empty() {
        return None;
    }
    Some((sender, room_id))
}

pub fn matrix_call_session_for_reply_target(
    reply_target: &str,
) -> Option<RealtimeCallSessionSnapshot> {
    let (sender, room_id) = parse_matrix_call_reply_target(reply_target)?;
    matrix_recent_call_sessions()
        .into_iter()
        .rev()
        .find(|session| {
            session.direction == RealtimeCallDirection::Inbound
                && !session.state.is_terminal()
                && session.call_session_id.as_deref() == Some(room_id)
                && session.origin.recipient.as_deref() == Some(sender)
        })
}

pub fn matrix_set_call_state_for_reply_target(
    reply_target: &str,
    state: RealtimeCallState,
) -> bool {
    let Some((sender, room_id)) = parse_matrix_call_reply_target(reply_target) else {
        return false;
    };

    let mut updated = false;
    update_matrix_call_control_status(|status| {
        let call_id = status
            .recent_sessions
            .iter()
            .rev()
            .find(|session| {
                session.direction == RealtimeCallDirection::Inbound
                    && !session.state.is_terminal()
                    && session.call_session_id.as_deref() == Some(room_id)
                    && session.origin.recipient.as_deref() == Some(sender)
            })
            .map(|session| session.call_control_id.clone());

        if let Some(call_id) = call_id {
            record_realtime_call_state(
                &mut status.recent_sessions,
                "matrix",
                RealtimeCallKind::Audio,
                &call_id,
                RealtimeCallDirection::Inbound,
                state,
            );
            trim_matrix_recent_sessions(status);
            updated = true;
        }
    });
    updated
}

#[cfg(test)]
fn matrix_incoming_call_event_text(sender: &str, room_id: &str, call_control_id: &str) -> String {
    format!(
        "[Incoming audio call from {sender} in room {room_id}; call_control_id={call_control_id}. This is a real live call context, not a hypothetical request. Do not say that you cannot answer or place calls. Answer naturally and briefly; the runtime can speak your reply in the call.]"
    )
}

fn matrix_call_member_end_reason(content: &CallMemberEventContent) -> Option<&'static str> {
    match content {
        CallMemberEventContent::Empty(empty) => Some(match empty.leave_reason {
            Some(LeaveReason::LostConnection) => "lost_connection",
            Some(_) | None => "remote_hangup",
        }),
        _ => None,
    }
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct SyncResponse {
    next_batch: String,
    #[serde(default)]
    rooms: Rooms,
}

#[cfg(test)]
#[derive(Debug, Deserialize, Default)]
struct Rooms {
    #[serde(default)]
    join: std::collections::HashMap<String, JoinedRoom>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct JoinedRoom {
    #[serde(default)]
    timeline: Timeline,
}

#[cfg(test)]
#[derive(Debug, Deserialize, Default)]
struct Timeline {
    #[serde(default)]
    events: Vec<TimelineEvent>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct TimelineEvent {
    #[serde(rename = "type")]
    event_type: String,
    sender: String,
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    content: EventContent,
}

#[cfg(test)]
#[derive(Debug, Deserialize, Default)]
struct EventContent {
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    msgtype: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhoAmIResponse {
    user_id: String,
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RoomAliasResponse {
    room_id: String,
}

// --- Outgoing reaction marker: [REACT:emoji:$event_id] ---

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatrixOutgoingReaction {
    emoji: String,
    event_id: String,
}

/// Parse `[REACT:emoji:$event_id]` markers from response text.
/// Returns cleaned text (markers removed) and list of reactions.
fn parse_matrix_reaction_markers(message: &str) -> (String, Vec<MatrixOutgoingReaction>) {
    let mut cleaned = String::with_capacity(message.len());
    let mut reactions = Vec::new();
    let mut rest = message;

    while let Some(start) = rest.find("[REACT:") {
        cleaned.push_str(&rest[..start]);
        let after_prefix = &rest[start + 7..];
        if let Some(end) = after_prefix.find(']') {
            let inner = &after_prefix[..end];
            // Split into emoji and event_id: "👍:$event_id"
            if let Some(colon) = inner.find(':') {
                let emoji = inner[..colon].trim();
                let event_id = inner[colon + 1..].trim();
                if !emoji.is_empty() && !event_id.is_empty() {
                    reactions.push(MatrixOutgoingReaction {
                        emoji: emoji.to_string(),
                        event_id: event_id.to_string(),
                    });
                }
            }
            rest = &after_prefix[end + 1..];
        } else {
            cleaned.push_str(&rest[start..start + 7]);
            rest = after_prefix;
        }
    }
    cleaned.push_str(rest);

    (cleaned.trim().to_string(), reactions)
}

// --- Outgoing location marker: [LOCATION:geo:lat,lon:description] ---

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatrixOutgoingLocation {
    geo_uri: String,
    description: String,
}

/// Parse `[LOCATION:geo_uri:description]` markers from response text.
fn parse_matrix_location_markers(message: &str) -> (String, Vec<MatrixOutgoingLocation>) {
    let mut cleaned = String::with_capacity(message.len());
    let mut locations = Vec::new();
    let mut rest = message;

    while let Some(start) = rest.find("[LOCATION:") {
        cleaned.push_str(&rest[..start]);
        let after_prefix = &rest[start + 10..];
        if let Some(end) = after_prefix.find(']') {
            let inner = &after_prefix[..end];
            // Split: "geo:lat,lon:Description text"
            // geo_uri is "geo:..." up to second ':', description is the rest
            if let Some(geo_end) = inner.find(',').and_then(|comma_pos| {
                // Find the ':' after the comma (end of geo_uri)
                inner[comma_pos..].find(':').map(|p| comma_pos + p)
            }) {
                let geo_uri = inner[..geo_end].trim().to_string();
                let description = inner[geo_end + 1..].trim().to_string();
                if !geo_uri.is_empty() {
                    locations.push(MatrixOutgoingLocation {
                        geo_uri,
                        description: if description.is_empty() {
                            "Shared location".to_string()
                        } else {
                            description
                        },
                    });
                }
            }
            rest = &after_prefix[end + 1..];
        } else {
            cleaned.push_str(&rest[start..start + 10]);
            rest = after_prefix;
        }
    }
    cleaned.push_str(rest);

    (cleaned.trim().to_string(), locations)
}

// --- Outgoing attachment marker types (follows Telegram/Discord pattern) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixOutgoingAttachmentKind {
    Image,
    File,
    Audio,
    Voice,
}

impl MatrixOutgoingAttachmentKind {
    fn from_marker(marker: &str) -> Option<Self> {
        match marker.trim().to_ascii_uppercase().as_str() {
            "IMAGE" | "PHOTO" => Some(Self::Image),
            "DOCUMENT" | "FILE" | "VIDEO" => Some(Self::File),
            "AUDIO" | "MUSIC" => Some(Self::Audio),
            "VOICE" => Some(Self::Voice),
            _ => None,
        }
    }
}

fn matrix_attachment_kind_for_artifact(kind: MediaArtifactKind) -> MatrixOutgoingAttachmentKind {
    match kind {
        MediaArtifactKind::Image => MatrixOutgoingAttachmentKind::Image,
        MediaArtifactKind::Audio | MediaArtifactKind::Music => MatrixOutgoingAttachmentKind::Audio,
        MediaArtifactKind::Voice => MatrixOutgoingAttachmentKind::Voice,
        MediaArtifactKind::Video => MatrixOutgoingAttachmentKind::File,
    }
}

fn matrix_attachment_fallback_file_name(kind: MatrixOutgoingAttachmentKind) -> &'static str {
    match kind {
        MatrixOutgoingAttachmentKind::Image => "image.png",
        MatrixOutgoingAttachmentKind::File => "file.bin",
        MatrixOutgoingAttachmentKind::Audio => "audio.bin",
        MatrixOutgoingAttachmentKind::Voice => "voice.ogg",
    }
}

#[derive(Debug, Clone)]
struct MatrixAudioDetails {
    duration: Option<Duration>,
    waveform: Option<Vec<f32>>,
}

fn matrix_audio_info(kind: MatrixOutgoingAttachmentKind, bytes: &[u8]) -> BaseAudioInfo {
    let details = wav_pcm16_audio_details(bytes);
    BaseAudioInfo {
        duration: details.as_ref().and_then(|details| details.duration),
        size: Some(UInt::new_wrapping(bytes.len() as u64)),
        waveform: if kind == MatrixOutgoingAttachmentKind::Voice {
            details.and_then(|details| details.waveform)
        } else {
            None
        },
    }
}

fn matrix_audio_message_is_voice(content: &AudioMessageEventContent) -> bool {
    content.voice.is_some()
}

fn matrix_audio_transcription_text(is_voice: bool, text: &str) -> String {
    if is_voice {
        format!("[Voice] {text}")
    } else {
        format!("[Audio] {text}")
    }
}

fn wav_pcm16_payload(bytes: &[u8]) -> Option<WavPcm16Payload> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut sample_rate = None::<u32>;
    let mut channels = None::<u16>;
    let mut bits_per_sample = None::<u16>;
    let mut data_range = None::<(usize, usize)>;
    let mut offset = 12usize;

    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
        let data_start = offset + 8;
        let available_size = bytes.len().saturating_sub(data_start);
        let chunk_size = if chunk_size == u32::MAX {
            available_size
        } else {
            (chunk_size as usize).min(available_size)
        };

        match chunk_id {
            b"fmt " if chunk_size >= 16 => {
                let fmt = &bytes[data_start..data_start + chunk_size];
                let audio_format = u16::from_le_bytes(fmt[0..2].try_into().ok()?);
                if audio_format != 1 {
                    return None;
                }
                channels = Some(u16::from_le_bytes(fmt[2..4].try_into().ok()?));
                sample_rate = Some(u32::from_le_bytes(fmt[4..8].try_into().ok()?));
                bits_per_sample = Some(u16::from_le_bytes(fmt[14..16].try_into().ok()?));
            }
            b"data" => {
                data_range = Some((data_start, chunk_size));
                break;
            }
            _ => {}
        }

        let padded_size = chunk_size + (chunk_size % 2);
        let next_offset = data_start.checked_add(padded_size)?;
        if next_offset <= offset || next_offset > bytes.len() {
            break;
        }
        offset = next_offset;
    }

    let sample_rate = sample_rate?;
    let channels = channels?;
    let bits_per_sample = bits_per_sample?;
    let (data_start, data_len) = data_range?;
    if sample_rate == 0 || channels == 0 || bits_per_sample != 16 || data_len == 0 {
        return None;
    }

    let frame_size = usize::from(channels) * 2;
    if frame_size == 0 {
        return None;
    }
    let frame_count = data_len / frame_size;
    if frame_count == 0 {
        return None;
    }

    let duration_ms = ((frame_count as u128 * 1000) / sample_rate as u128) as u64;
    let waveform = pcm16_waveform(bytes, data_start, frame_count, channels, frame_size);

    let samples = bytes[data_start..data_start + data_len]
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return None;
    }

    Some(WavPcm16Payload {
        sample_rate,
        channels: u32::from(channels),
        duration: Duration::from_millis(duration_ms.max(1)),
        waveform,
        samples,
    })
}

fn wav_pcm16_audio_details(bytes: &[u8]) -> Option<MatrixAudioDetails> {
    wav_pcm16_payload(bytes).map(|payload| MatrixAudioDetails {
        duration: Some(payload.duration),
        waveform: payload.waveform,
    })
}

fn pcm16_waveform(
    bytes: &[u8],
    data_start: usize,
    frame_count: usize,
    channels: u16,
    frame_size: usize,
) -> Option<Vec<f32>> {
    const WAVEFORM_POINTS: usize = 64;
    let mut waveform = vec![0.0f32; WAVEFORM_POINTS];
    for frame_index in 0..frame_count {
        let bin = frame_index * WAVEFORM_POINTS / frame_count;
        let frame_start = data_start + frame_index * frame_size;
        let mut peak = 0i16;
        for channel in 0..usize::from(channels) {
            let sample_start = frame_start + channel * 2;
            let Some(sample_bytes) = bytes.get(sample_start..sample_start + 2) else {
                continue;
            };
            let sample = i16::from_le_bytes(sample_bytes.try_into().ok()?).saturating_abs();
            peak = peak.max(sample);
        }
        waveform[bin] = waveform[bin].max(f32::from(peak) / f32::from(i16::MAX));
    }
    Some(waveform)
}

fn matrix_media_artifact_attachments(
    artifacts: &[MediaArtifact],
) -> anyhow::Result<Vec<MatrixOutgoingAttachment>> {
    artifacts
        .iter()
        .map(|artifact| {
            Ok(MatrixOutgoingAttachment {
                kind: matrix_attachment_kind_for_artifact(artifact.kind),
                target: artifact_delivery_uri("matrix", artifact)?.to_string(),
                label: artifact.label.clone(),
                mime_type: artifact.mime_type.clone(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatrixOutgoingAttachment {
    kind: MatrixOutgoingAttachmentKind,
    target: String,
    label: Option<String>,
    mime_type: Option<String>,
}

fn parse_matrix_attachment_markers(message: &str) -> (String, Vec<MatrixOutgoingAttachment>) {
    let mut cleaned = String::with_capacity(message.len());
    let mut attachments = Vec::new();
    let mut cursor = 0usize;

    while cursor < message.len() {
        let Some(open_rel) = message[cursor..].find('[') else {
            cleaned.push_str(&message[cursor..]);
            break;
        };

        let open = cursor + open_rel;
        cleaned.push_str(&message[cursor..open]);

        let Some(close_rel) = message[open..].find(']') else {
            cleaned.push_str(&message[open..]);
            break;
        };

        let close = open + close_rel;
        let marker = &message[open + 1..close];

        let parsed = marker.split_once(':').and_then(|(kind, target)| {
            let kind = MatrixOutgoingAttachmentKind::from_marker(kind)?;
            let target = target.trim();
            if target.is_empty() {
                return None;
            }
            Some(MatrixOutgoingAttachment {
                kind,
                target: target.to_string(),
                label: None,
                mime_type: None,
            })
        });

        if let Some(attachment) = parsed {
            attachments.push(attachment);
        } else {
            cleaned.push_str(&message[open..=close]);
        }

        cursor = close + 1;
    }

    (cleaned.trim().to_string(), attachments)
}

fn is_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        })
        .unwrap_or(false)
}

fn matrix_inbound_media_attachment(
    kind: InboundMediaKind,
    path: &Path,
    label: impl Into<String>,
) -> InboundMediaAttachment {
    InboundMediaAttachment {
        kind,
        uri: path.to_string_lossy().into_owned(),
        mime_type: None,
        label: Some(label.into()),
    }
}

/// Download media from a Matrix media source via the SDK client, save to disk.
///
/// When `size_hint` is provided (from the event's `info.size`), the download is
/// rejected before fetching the payload. The post-download check remains as a
/// safety net because metadata can be spoofed.
async fn download_and_save_matrix_media(
    client: &MatrixSdkClient,
    source: &MediaSource,
    filename: &str,
    save_dir: &Path,
    size_hint: Option<u64>,
    max_bytes: usize,
) -> anyhow::Result<PathBuf> {
    // Pre-download size check using event metadata (avoids buffering large payloads).
    if let Some(size) = size_hint {
        if usize::try_from(size).unwrap_or(usize::MAX) > max_bytes {
            anyhow::bail!(
                "Matrix media metadata size exceeds limit ({size} bytes > {max_bytes} bytes); skipping download",
            );
        }
    }

    let request = MediaRequestParameters {
        source: source.clone(),
        format: MediaFormat::File,
    };

    let data = client.media().get_media_content(&request, false).await?;

    // Post-download safety net (metadata can be spoofed).
    if data.len() > max_bytes {
        anyhow::bail!(
            "Matrix media exceeds size limit ({} bytes > {max_bytes} bytes)",
            data.len(),
        );
    }

    tokio::fs::create_dir_all(save_dir).await?;

    // Sanitize filename: UUID prefix prevents collisions and path traversal.
    let safe_name = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment.bin");
    let local_name = format!("{}_{}", uuid::Uuid::new_v4(), safe_name);
    let local_path = save_dir.join(&local_name);

    tokio::fs::write(&local_path, &data).await?;

    Ok(local_path)
}

impl MatrixChannel {
    fn normalize_optional_field(value: Option<String>) -> Option<String> {
        value
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
    }

    /// Create a new Matrix channel with minimal configuration.
    pub fn new(
        homeserver: String,
        access_token: Option<String>,
        room_id: String,
        allowed_users: Vec<String>,
    ) -> Self {
        Self::new_with_session_hint(homeserver, access_token, room_id, allowed_users, None, None)
    }

    /// Create a new Matrix channel with optional session owner and device ID hints.
    pub fn new_with_session_hint(
        homeserver: String,
        access_token: Option<String>,
        room_id: String,
        allowed_users: Vec<String>,
        owner_hint: Option<String>,
        device_id_hint: Option<String>,
    ) -> Self {
        Self::new_with_session_hint_and_synapseclaw_dir(
            homeserver,
            access_token,
            room_id,
            allowed_users,
            owner_hint,
            device_id_hint,
            None,
        )
    }

    /// Create a new Matrix channel with session hints and an optional workspace directory for media storage.
    pub fn new_with_session_hint_and_synapseclaw_dir(
        homeserver: String,
        access_token: Option<String>,
        room_id: String,
        allowed_users: Vec<String>,
        owner_hint: Option<String>,
        device_id_hint: Option<String>,
        synapseclaw_dir: Option<PathBuf>,
    ) -> Self {
        let homeserver = homeserver.trim_end_matches('/').to_string();
        let access_token = access_token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let room_id = room_id.trim().to_string();
        let allowed_users = allowed_users
            .into_iter()
            .map(|user| user.trim().to_string())
            .filter(|user| !user.is_empty())
            .collect();

        let channel = Self {
            homeserver,
            access_token,
            room_id,
            allowed_users,
            session_owner_hint: Self::normalize_optional_field(owner_hint),
            session_device_id_hint: Self::normalize_optional_field(device_id_hint),
            synapseclaw_dir,
            resolved_room_id_cache: Arc::new(RwLock::new(None)),
            sdk_client: Arc::new(OnceCell::new()),
            http_client: Client::new(),
            reaction_events: Arc::new(RwLock::new(HashMap::new())),
            voice_mode: Arc::new(AtomicBool::new(false)),
            transcription: None,
            tts: None,
            voice_transcriptions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            password: None,
            max_media_bytes: DEFAULT_MAX_MEDIA_DOWNLOAD_BYTES,
        };
        configure_matrix_call_ledger_dir(channel.synapseclaw_dir.clone());
        record_matrix_call_control_config(&channel);
        channel
    }

    /// Set optional password for automatic login and cross-signing bootstrap.
    pub fn with_password(mut self, password: Option<String>) -> Self {
        self.password = password;
        self
    }

    /// Enable audio transcription with the given configuration.
    pub fn with_transcription(
        mut self,
        config: synapse_domain::config::schema::TranscriptionConfig,
    ) -> Self {
        if config.enabled {
            self.transcription = Some(config);
        }
        record_matrix_call_control_config(&self);
        self
    }

    pub fn with_tts(mut self, config: TtsConfig) -> Self {
        if config.enabled {
            self.tts = Some(config);
        }
        record_matrix_call_control_config(&self);
        self
    }

    /// Set maximum media download size in megabytes. `0` means no limit.
    pub fn with_max_media_download_mb(mut self, mb: Option<u32>) -> Self {
        self.max_media_bytes = match mb {
            Some(0) => usize::MAX,
            Some(v) => v as usize * 1024 * 1024,
            None => DEFAULT_MAX_MEDIA_DOWNLOAD_BYTES,
        };
        self
    }

    fn encode_path_segment(value: &str) -> String {
        fn should_encode(byte: u8) -> bool {
            !matches!(
                byte,
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
            )
        }

        let mut encoded = String::with_capacity(value.len());
        for byte in value.bytes() {
            if should_encode(byte) {
                use std::fmt::Write;
                let _ = write!(&mut encoded, "%{byte:02X}");
            } else {
                encoded.push(byte as char);
            }
        }

        encoded
    }

    /// Returns the Bearer token for HTTP API calls.
    /// Prefers the live SDK client session, falls back to config access_token,
    /// then to persisted session.json.
    async fn auth_header_value(&self) -> anyhow::Result<String> {
        // 1. Try live SDK client session (most up-to-date after password login).
        if let Some(client) = self.sdk_client.get() {
            if let Some(token) = client.access_token() {
                return Ok(format!("Bearer {token}"));
            }
        }
        // 2. Config access_token.
        if let Some(ref token) = self.access_token {
            return Ok(format!("Bearer {token}"));
        }
        // 3. Persisted session.json.
        if let Some(saved) = self.load_saved_session().await {
            return Ok(format!("Bearer {}", saved.access_token));
        }
        anyhow::bail!(
            "Matrix access_token is not available; configure access_token or password and restart"
        )
    }

    async fn current_session_identity(&self) -> anyhow::Result<(String, String)> {
        if let Some(client) = self.sdk_client.get() {
            if let (Some(user_id), Some(device_id)) = (client.user_id(), client.device_id()) {
                return Ok((user_id.to_string(), device_id.to_string()));
            }
        }

        if let Some(saved) = self.load_saved_session().await {
            return Ok((saved.user_id, saved.device_id));
        }

        if let (Some(user_id), Some(device_id)) = (
            self.session_owner_hint.as_deref(),
            self.session_device_id_hint.as_deref(),
        ) {
            return Ok((user_id.to_string(), device_id.to_string()));
        }

        anyhow::bail!(
            "Matrix session identity is unavailable; persist a session or configure user_id/device_id hints"
        )
    }

    async fn current_call_member_state(
        &self,
        room_id: &str,
        focus_url: &str,
    ) -> anyhow::Result<(CallMemberStateKey, CallMemberEventContent)> {
        let (user_id, device_id) = self.current_session_identity().await?;
        let user_id: OwnedUserId = user_id.parse()?;
        let member_id = format!("{device_id}_m.call");
        let state_key = CallMemberStateKey::new(user_id, Some(member_id), true);
        let focus_url = focus_url.trim_end_matches('/').to_string();
        let content = CallMemberEventContent::new(
            Application::Call(CallApplicationContent::new(String::new(), CallScope::Room)),
            device_id.into(),
            ActiveFocus::Livekit(ActiveLivekitFocus::new()),
            vec![Focus::Livekit(LivekitFocus::new(
                room_id.to_string(),
                focus_url,
            ))],
            None,
            None,
        );
        Ok((state_key, content))
    }

    async fn announce_call_membership(
        &self,
        room: &Room,
        room_id: &str,
        focus_url: &str,
        call_control_id: &str,
    ) -> anyhow::Result<OwnedEventId> {
        let (state_key, content) = self.current_call_member_state(room_id, focus_url).await?;
        let response = room
            .send_state_event_for_key(&state_key, content)
            .await
            .map_err(|error| {
                anyhow::anyhow!("failed to publish Matrix call membership: {error}")
            })?;
        tracing::info!(
            call_control_id = %call_control_id,
            room_id = %room_id,
            state_key = %state_key.as_ref(),
            membership_event_id = %response.event_id,
            "Matrix call membership published"
        );
        Ok(response.event_id)
    }

    async fn clear_call_membership_for_room(
        &self,
        room: &Room,
        call_control_id: &str,
    ) -> anyhow::Result<()> {
        let (user_id, device_id) = self.current_session_identity().await?;
        let user_id: OwnedUserId = user_id.parse()?;
        let state_key = CallMemberStateKey::new(user_id, Some(format!("{device_id}_m.call")), true);
        room.send_state_event_for_key(&state_key, CallMemberEventContent::new_empty(None))
            .await
            .map_err(|error| anyhow::anyhow!("failed to clear Matrix call membership: {error}"))?;
        tracing::info!(
            call_control_id = %call_control_id,
            room_id = %room.room_id(),
            state_key = %state_key.as_ref(),
            "Matrix call membership cleared"
        );
        Ok(())
    }

    async fn clear_call_membership_for_call(&self, call_control_id: &str) -> anyhow::Result<()> {
        let room = self.room_for_call_control_id(call_control_id).await?;
        self.clear_call_membership_for_room(&room, call_control_id).await
    }

    async fn request_openid_token(&self) -> anyhow::Result<MatrixOpenIdToken> {
        let (user_id, _) = self.current_session_identity().await?;
        let encoded_user_id = Self::encode_path_segment(&user_id);
        let url = format!(
            "{}/_matrix/client/v3/user/{}/openid/request_token",
            self.homeserver, encoded_user_id
        );
        let resp = self
            .http_client
            .post(&url)
            .header("Authorization", self.auth_header_value().await?)
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Matrix OpenID request failed: {err}");
        }

        let token = resp.json::<MatrixOpenIdToken>().await?;
        if token.access_token.trim().is_empty()
            || token.matrix_server_name.trim().is_empty()
            || token.token_type.trim().is_empty()
            || token.expires_in == 0
        {
            anyhow::bail!("Matrix OpenID response was missing token metadata");
        }

        Ok(token)
    }

    fn matrix_rtc_member_id(user_id: &str, device_id: &str) -> String {
        format!("{user_id}|{device_id}|synapseclaw")
    }

    fn matrix_rtc_authorizer_request(
        room_id: &str,
        openid_token: MatrixOpenIdToken,
        user_id: String,
        device_id: String,
    ) -> MatrixRtcAuthorizerRequest {
        MatrixRtcAuthorizerRequest {
            room_id: room_id.to_string(),
            slot_id: "m.call#ROOM".to_string(),
            openid_token,
            member: MatrixRtcAuthorizerMember {
                id: Self::matrix_rtc_member_id(&user_id, &device_id),
                claimed_user_id: user_id,
                claimed_device_id: device_id,
            },
        }
    }

    fn matrix_rtc_legacy_authorizer_request(
        room_id: &str,
        openid_token: MatrixOpenIdToken,
        device_id: String,
    ) -> MatrixRtcLegacyAuthorizerRequest {
        MatrixRtcLegacyAuthorizerRequest {
            room: room_id.to_string(),
            openid_token,
            device_id,
        }
    }

    async fn probe_matrix_rtc_transports(
        &self,
    ) -> anyhow::Result<(Option<String>, Option<String>)> {
        const TRANSPORT_PATHS: [&str; 2] = [
            "/_matrix/client/v1/rtc/transports",
            "/_matrix/client/unstable/org.matrix.msc4143/rtc/transports",
        ];

        let auth_header = self.auth_header_value().await?;
        let mut saw_404 = false;

        for path in TRANSPORT_PATHS {
            let url = format!("{}{}", self.homeserver, path);
            let resp = self
                .http_client
                .get(&url)
                .header("Authorization", auth_header.clone())
                .send()
                .await?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                saw_404 = true;
                continue;
            }

            if !resp.status().is_success() {
                let err = resp.text().await.unwrap_or_default();
                anyhow::bail!("Matrix RTC transports probe failed on {path}: {err}");
            }

            let body = resp.json::<MatrixRtcTransportsResponse>().await?;
            let focus_url = body
                .rtc_transports
                .into_iter()
                .find(is_livekit_matrix_rtc_transport)
                .and_then(|transport| transport.livekit_service_url)
                .map(|value| value.trim_end_matches('/').to_string());
            return Ok((focus_url, Some(path.to_string())));
        }

        if saw_404 {
            return Ok((None, None));
        }

        Ok((None, None))
    }

    async fn well_known_matrix_rtc_focus_url(&self) -> anyhow::Result<Option<String>> {
        let client = self.matrix_client().await?;
        let foci = client.rtc_foci().await?;
        let focus_url = foci.into_iter().find_map(|focus| {
            match focus {
            matrix_sdk::ruma::api::client::discovery::discover_homeserver::RtcFocusInfo::LiveKit(
                info,
            ) => Some(info.service_url.trim_end_matches('/').to_string()),
            _ => None,
        }
        });
        Ok(focus_url)
    }

    async fn probe_matrix_rtc_authorizer_api(
        &self,
        focus_url: &str,
    ) -> anyhow::Result<MatrixRtcAuthorizerApi> {
        let focus_url = focus_url.trim_end_matches('/');
        let candidates = [
            (
                MatrixRtcAuthorizerApi::GetToken,
                format!("{focus_url}/get_token"),
            ),
            (
                MatrixRtcAuthorizerApi::SfuGet,
                format!("{focus_url}/sfu/get"),
            ),
        ];

        for (api, url) in candidates {
            let resp = self
                .http_client
                .post(&url)
                .header("Content-Type", "application/json")
                .body("{}")
                .send()
                .await?;
            if resp.status() != reqwest::StatusCode::NOT_FOUND {
                return Ok(api);
            }
        }

        Ok(MatrixRtcAuthorizerApi::Unknown)
    }

    async fn request_matrix_rtc_authorizer_grant_with_api(
        &self,
        focus_url: &str,
        room_id: &str,
        openid_token: MatrixOpenIdToken,
        api: MatrixRtcAuthorizerApi,
    ) -> anyhow::Result<MatrixRtcAuthorizerGrant> {
        let focus_url = focus_url.trim_end_matches('/');
        let (_, device_id) = self.current_session_identity().await?;
        let endpoint = match api {
            MatrixRtcAuthorizerApi::GetToken => format!("{focus_url}/get_token"),
            MatrixRtcAuthorizerApi::SfuGet => format!("{focus_url}/sfu/get"),
            MatrixRtcAuthorizerApi::Unknown => {
                anyhow::bail!("Matrix RTC authorizer api is unknown")
            }
        };

        let response = match api {
            MatrixRtcAuthorizerApi::GetToken => {
                let (user_id, device_id) = self.current_session_identity().await?;
                let body =
                    Self::matrix_rtc_authorizer_request(room_id, openid_token, user_id, device_id);
                self.http_client.post(&endpoint).json(&body).send().await?
            }
            MatrixRtcAuthorizerApi::SfuGet => {
                let body =
                    Self::matrix_rtc_legacy_authorizer_request(room_id, openid_token, device_id);
                self.http_client.post(&endpoint).json(&body).send().await?
            }
            MatrixRtcAuthorizerApi::Unknown => unreachable!(),
        };

        if !response.status().is_success() {
            let status = response.status();
            let err = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Matrix RTC authorizer {:?} request failed with status {}: {}",
                api,
                status,
                err
            );
        }

        let grant = response.json::<MatrixRtcAuthorizerGrantResponse>().await?;
        if grant.url.trim().is_empty() || grant.jwt.trim().is_empty() {
            anyhow::bail!("Matrix RTC authorizer {:?} returned an empty grant", api);
        }

        Ok(MatrixRtcAuthorizerGrant {
            api,
            livekit_service_url: grant.url.trim_end_matches('/').to_string(),
            jwt: grant.jwt,
        })
    }

    async fn request_matrix_rtc_authorizer_grant(
        &self,
        focus_url: &str,
        room_id: &str,
        preferred_api: MatrixRtcAuthorizerApi,
        openid_token: MatrixOpenIdToken,
    ) -> anyhow::Result<MatrixRtcAuthorizerGrant> {
        let apis = match preferred_api {
            MatrixRtcAuthorizerApi::GetToken => {
                vec![
                    MatrixRtcAuthorizerApi::GetToken,
                    MatrixRtcAuthorizerApi::SfuGet,
                ]
            }
            MatrixRtcAuthorizerApi::SfuGet => vec![MatrixRtcAuthorizerApi::SfuGet],
            MatrixRtcAuthorizerApi::Unknown => {
                vec![
                    MatrixRtcAuthorizerApi::GetToken,
                    MatrixRtcAuthorizerApi::SfuGet,
                ]
            }
        };

        let mut last_error = None;
        for api in apis {
            match self
                .request_matrix_rtc_authorizer_grant_with_api(
                    focus_url,
                    room_id,
                    openid_token.clone(),
                    api,
                )
                .await
            {
                Ok(grant) => return Ok(grant),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Matrix RTC authorizer grant failed")))
    }

    async fn probe_matrix_rtc_bootstrap(&self, room_id: &str) -> MatrixRtcBootstrapStatus {
        let mut bootstrap = MatrixRtcBootstrapStatus::default();

        let focus_from_transports = match self.probe_matrix_rtc_transports().await {
            Ok((focus_url, path)) => {
                bootstrap.transports_api_supported = Some(path.is_some());
                bootstrap.transports_api_path = path;
                focus_url
            }
            Err(error) => {
                bootstrap.transports_api_supported = Some(false);
                bootstrap.last_probe_error = Some(bounded_matrix_status_detail(error.to_string()));
                None
            }
        };

        let focus_url = if let Some(focus_url) = focus_from_transports {
            bootstrap.focus_source = MatrixRtcFocusSource::RtcTransports;
            Some(focus_url)
        } else {
            match self.well_known_matrix_rtc_focus_url().await {
                Ok(Some(focus_url)) => {
                    bootstrap.focus_source = MatrixRtcFocusSource::WellKnown;
                    Some(focus_url)
                }
                Ok(None) => None,
                Err(error) => {
                    if bootstrap.last_probe_error.is_none() {
                        bootstrap.last_probe_error =
                            Some(bounded_matrix_status_detail(error.to_string()));
                    }
                    None
                }
            }
        };

        bootstrap.focus_url = focus_url.clone();

        if let Some(focus_url) = focus_url.as_deref() {
            match self
                .http_client
                .get(format!("{focus_url}/healthz"))
                .send()
                .await
            {
                Ok(resp) => {
                    bootstrap.authorizer_healthy = Some(resp.status().is_success());
                }
                Err(error) => {
                    bootstrap.authorizer_healthy = Some(false);
                    if bootstrap.last_probe_error.is_none() {
                        bootstrap.last_probe_error =
                            Some(bounded_matrix_status_detail(error.to_string()));
                    }
                }
            }

            match self.probe_matrix_rtc_authorizer_api(focus_url).await {
                Ok(api) => bootstrap.authorizer_api = api,
                Err(error) => {
                    if bootstrap.last_probe_error.is_none() {
                        bootstrap.last_probe_error =
                            Some(bounded_matrix_status_detail(error.to_string()));
                    }
                }
            }
        }

        let openid_token = match self.request_openid_token().await {
            Ok(token) => {
                bootstrap.openid_token_ready = Some(true);
                Some(token)
            }
            Err(error) => {
                bootstrap.openid_token_ready = Some(false);
                if bootstrap.last_probe_error.is_none() {
                    bootstrap.last_probe_error =
                        Some(bounded_matrix_status_detail(error.to_string()));
                }
                None
            }
        };

        if let (Some(focus_url), Some(openid_token)) = (focus_url.as_deref(), openid_token) {
            match self
                .request_matrix_rtc_authorizer_grant(
                    focus_url,
                    room_id,
                    bootstrap.authorizer_api,
                    openid_token,
                )
                .await
            {
                Ok(grant) => {
                    bootstrap.authorizer_api = grant.api;
                    bootstrap.authorizer_grant_ready = Some(true);
                    bootstrap.livekit_service_url = Some(grant.livekit_service_url);
                    if grant.jwt.trim().is_empty() && bootstrap.last_probe_error.is_none() {
                        bootstrap.last_probe_error =
                            Some("Matrix RTC authorizer returned an empty JWT".to_string());
                    }
                }
                Err(error) => {
                    bootstrap.authorizer_grant_ready = Some(false);
                    if bootstrap.last_probe_error.is_none() {
                        bootstrap.last_probe_error =
                            Some(bounded_matrix_status_detail(error.to_string()));
                    }
                }
            }
        }

        bootstrap.media_bootstrap_ready = bootstrap.focus_url.is_some()
            && bootstrap.authorizer_healthy == Some(true)
            && bootstrap.openid_token_ready == Some(true)
            && bootstrap.authorizer_grant_ready == Some(true)
            && bootstrap.authorizer_api != MatrixRtcAuthorizerApi::Unknown;

        bootstrap
    }

    fn matrix_store_dir(&self) -> Option<PathBuf> {
        self.synapseclaw_dir
            .as_ref()
            .map(|dir| dir.join("state").join("matrix"))
    }

    fn session_file_path(&self) -> Option<PathBuf> {
        self.matrix_store_dir().map(|d| d.join(MATRIX_SESSION_FILE))
    }

    async fn load_saved_session(&self) -> Option<SavedSession> {
        let path = self.session_file_path()?;
        let data = tokio::fs::read_to_string(&path).await.ok()?;
        serde_json::from_str(&data).ok()
    }

    async fn save_session(&self, session: &SavedSession) -> anyhow::Result<()> {
        let path = self.session_file_path().ok_or_else(|| {
            anyhow::anyhow!("Matrix store directory not configured; cannot persist session")
        })?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let data = serde_json::to_string_pretty(session)?;
        // Write to a temp file with restricted permissions, then rename atomically
        // to avoid a window where the token is world-readable.
        let tmp_path = path.with_extension("json.tmp");
        #[cfg(unix)]
        {
            let mut opts = tokio::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true).mode(0o600);
            let mut file = opts.open(&tmp_path).await?;
            tokio::io::AsyncWriteExt::write_all(&mut file, data.as_bytes()).await?;
            tokio::io::AsyncWriteExt::flush(&mut file).await?;
        }
        #[cfg(not(unix))]
        {
            tokio::fs::write(&tmp_path, &data).await?;
        }
        tokio::fs::rename(&tmp_path, &path).await?;
        Ok(())
    }

    fn media_save_dir(&self) -> Option<PathBuf> {
        self.synapseclaw_dir
            .as_ref()
            .map(|dir| dir.join("workspace").join("matrix_files"))
    }

    fn live_call_debug_dir(&self) -> Option<PathBuf> {
        self.synapseclaw_dir
            .as_ref()
            .map(|dir| dir.join("workspace").join("matrix_call_debug"))
    }

    #[cfg(test)]
    fn is_user_allowed(&self, sender: &str) -> bool {
        Self::is_sender_allowed(&self.allowed_users, sender)
    }

    fn is_sender_allowed(allowed_users: &[String], sender: &str) -> bool {
        if allowed_users.iter().any(|u| u == "*") {
            return true;
        }

        allowed_users.iter().any(|u| u.eq_ignore_ascii_case(sender))
    }

    #[cfg(test)]
    fn is_supported_message_type(msgtype: &str) -> bool {
        matches!(
            msgtype,
            "m.text" | "m.notice" | "m.image" | "m.file" | "m.audio"
        )
    }

    fn has_non_empty_body(body: &str) -> bool {
        !body.trim().is_empty()
    }

    fn cache_event_id(
        event_id: &str,
        recent_order: &mut std::collections::VecDeque<String>,
        recent_lookup: &mut std::collections::HashSet<String>,
    ) -> bool {
        const MAX_RECENT_EVENT_IDS: usize = 2048;

        if recent_lookup.contains(event_id) {
            return true;
        }

        let event_id_owned = event_id.to_string();
        recent_lookup.insert(event_id_owned.clone());
        recent_order.push_back(event_id_owned);

        if recent_order.len() > MAX_RECENT_EVENT_IDS {
            if let Some(evicted) = recent_order.pop_front() {
                recent_lookup.remove(&evicted);
            }
        }

        false
    }

    async fn target_room_id(&self) -> anyhow::Result<String> {
        if self.room_id.starts_with('!') {
            return Ok(self.room_id.clone());
        }

        if let Some(cached) = self.resolved_room_id_cache.read().await.clone() {
            return Ok(cached);
        }

        let resolved = self.resolve_room_id().await?;
        *self.resolved_room_id_cache.write().await = Some(resolved.clone());
        Ok(resolved)
    }

    async fn resolve_room_reference(&self, configured: &str) -> anyhow::Result<String> {
        let configured = configured.trim();

        if configured.starts_with('!') {
            return Ok(configured.to_string());
        }

        if configured.starts_with('#') {
            let encoded_alias = Self::encode_path_segment(configured);
            let url = format!(
                "{}/_matrix/client/v3/directory/room/{}",
                self.homeserver, encoded_alias
            );

            let resp = self
                .http_client
                .get(&url)
                .header("Authorization", self.auth_header_value().await?)
                .send()
                .await?;

            if !resp.status().is_success() {
                let err = resp.text().await.unwrap_or_default();
                anyhow::bail!("Matrix room alias resolution failed for '{configured}': {err}");
            }

            let resolved: RoomAliasResponse = resp.json().await?;
            return Ok(resolved.room_id);
        }

        anyhow::bail!(
            "Matrix room reference must start with '!' (room ID) or '#' (room alias), got: {configured}"
        )
    }

    async fn get_my_identity(&self) -> anyhow::Result<WhoAmIResponse> {
        let url = format!("{}/_matrix/client/v3/account/whoami", self.homeserver);
        let resp = self
            .http_client
            .get(&url)
            .header("Authorization", self.auth_header_value().await?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Matrix whoami failed: {err}");
        }

        Ok(resp.json().await?)
    }

    async fn get_my_user_id(&self) -> anyhow::Result<String> {
        Ok(self.get_my_identity().await?.user_id)
    }

    async fn matrix_client(&self) -> anyhow::Result<MatrixSdkClient> {
        let client = self
            .sdk_client
            .get_or_try_init(|| async {
                // ── Build SDK client with E2EE + persistent store ──
                let encryption_settings = matrix_sdk::encryption::EncryptionSettings {
                    auto_enable_cross_signing: true,
                    auto_enable_backups: true,
                    backup_download_strategy:
                        matrix_sdk::encryption::BackupDownloadStrategy::AfterDecryptionFailure,
                };

                let mut client_builder = MatrixSdkClient::builder()
                    .homeserver_url(&self.homeserver)
                    .with_encryption_settings(encryption_settings);

                if let Some(store_dir) = self.matrix_store_dir() {
                    tokio::fs::create_dir_all(&store_dir).await.map_err(|error| {
                        anyhow::anyhow!(
                            "Matrix failed to initialize persistent store directory at '{}': {error}",
                            store_dir.display()
                        )
                    })?;
                    client_builder = client_builder.sqlite_store(&store_dir, None);
                }

                let client = client_builder.build().await?;

                // ── Auth: session.json → access_token → password → error ──
                let saved = self.load_saved_session().await;
                let resolved_user_id: String;
                let mut password_authenticated_session = false;

                // Determine whether to use the saved session or fall through
                // to config credentials. If config has fresh credentials and the
                // saved session's user_id doesn't match the configured owner hint,
                // prefer config to avoid stale session.json overriding intentional
                // config changes.
                let use_saved_session = if let Some(ref saved) = saved {
                    let config_has_credentials =
                        self.access_token.is_some() || self.password.is_some();
                    let saved_matches_config = self
                        .session_owner_hint
                        .as_ref()
                        .map_or(true, |hint| hint == &saved.user_id);

                    if config_has_credentials && !saved_matches_config {
                        tracing::warn!(
                            "Matrix: session.json user_id ({}) does not match configured owner hint — ignoring saved session and using config credentials",
                            synapse_domain::domain::util::redact(&saved.user_id)
                        );
                        false
                    } else {
                        if config_has_credentials {
                            tracing::debug!(
                                "Matrix: config credentials present but session.json user matches — restoring saved session"
                            );
                        }
                        true
                    }
                } else {
                    false
                };

                if use_saved_session {
                    let saved = saved.as_ref().unwrap();
                    // Path 1: Restore from persisted session.json (previous login).
                    let user_id: OwnedUserId = saved.user_id.parse()?;
                    let session = MatrixSession {
                        meta: SessionMeta {
                            user_id,
                            device_id: saved.device_id.clone().into(),
                        },
                        tokens: SessionTokens {
                            access_token: saved.access_token.clone(),
                            refresh_token: None,
                        },
                    };
                    client.restore_session(session).await?;
                    record_matrix_call_auth_source(MatrixStatusAuthSource::SessionStore);
                    resolved_user_id = saved.user_id.clone();
                    tracing::info!(
                        "Matrix: restored session from session.json (device_id={})",
                        synapse_domain::domain::util::redact(&saved.device_id)
                    );
                } else if self.access_token.is_some() {
                    // Path 2: Restore from config access_token + device_id (legacy flow).
                    let identity = self.get_my_identity().await;
                    let whoami = match identity {
                        Ok(whoami) => Some(whoami),
                        Err(error) => {
                            if self.session_owner_hint.is_some()
                                && self.session_device_id_hint.is_some()
                            {
                                tracing::warn!(
                                    "Matrix whoami failed; falling back to configured session hints: {error}"
                                );
                                None
                            } else {
                                return Err(error);
                            }
                        }
                    };

                    let user_id_str = if let Some(whoami) = whoami.as_ref() {
                        whoami.user_id.clone()
                    } else {
                        self.session_owner_hint.clone().ok_or_else(|| {
                            anyhow::anyhow!(
                                "Matrix session restore requires user_id when whoami is unavailable"
                            )
                        })?
                    };

                    let device_id_str = match (
                        whoami.as_ref().and_then(|w| w.device_id.clone()),
                        self.session_device_id_hint.as_ref(),
                    ) {
                        (Some(whoami_did), _) => whoami_did,
                        (None, Some(hinted)) => hinted.clone(),
                        (None, None) => {
                            return Err(anyhow::anyhow!(
                                "Matrix E2EE requires device_id. Set channels.matrix.device_id or use password-based login."
                            ));
                        }
                    };

                    let user_id: OwnedUserId = user_id_str.parse()?;
                    let session = MatrixSession {
                        meta: SessionMeta {
                            user_id,
                            device_id: device_id_str.clone().into(),
                        },
                        tokens: SessionTokens {
                            access_token: self.access_token.clone().unwrap_or_default(),
                            refresh_token: None,
                        },
                    };
                    client.restore_session(session).await?;
                    record_matrix_call_auth_source(MatrixStatusAuthSource::AccessToken);
                    resolved_user_id = user_id_str.clone();

                    // Persist session.json so future runs use Path 1.
                    if let Err(e) = self
                        .save_session(&SavedSession {
                            access_token: self.access_token.clone().unwrap_or_default(),
                            device_id: device_id_str,
                            user_id: user_id_str,
                        })
                        .await
                    {
                        tracing::warn!("Matrix: failed to persist session.json: {e}");
                    }
                } else if let Some(ref pw) = self.password {
                    // Path 3: Login with password (simplest setup, no access_token needed).
                    let user_id_str = self.session_owner_hint.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Matrix password-based login requires user_id in config"
                        )
                    })?;

                    let response = client
                        .matrix_auth()
                        .login_username(user_id_str, pw)
                        .initial_device_display_name("SynapseClaw")
                        .send()
                        .await?;

                    password_authenticated_session = true;
                    record_matrix_call_auth_source(MatrixStatusAuthSource::Password);
                    resolved_user_id = response.user_id.to_string();
                    tracing::info!(
                        "Matrix: logged in with password (device_id={})",
                        synapse_domain::domain::util::redact(response.device_id.as_str())
                    );

                    // Persist session.json so future runs use Path 1 (no password needed).
                    if let Err(e) = self
                        .save_session(&SavedSession {
                            access_token: response.access_token,
                            device_id: response.device_id.to_string(),
                            user_id: resolved_user_id.clone(),
                        })
                        .await
                    {
                        tracing::warn!("Matrix: failed to persist session.json: {e}");
                    }
                } else {
                    record_matrix_call_auth_source(MatrixStatusAuthSource::Unknown);
                    return Err(anyhow::anyhow!(
                        "Matrix channel requires either access_token or password in config"
                    ));
                }

                // ── E2EE initialization ──
                client
                    .encryption()
                    .wait_for_e2ee_initialization_tasks()
                    .await;

                // ── Cross-signing bootstrap with password (UIA) ──
                // Always attempt bootstrap_cross_signing (not _if_needed) so keys
                // are actually uploaded to the server. The _if_needed variant checks
                // the local store, which may have stale keys from auto_enable that
                // were never uploaded (server required UIA).
                if password_authenticated_session {
                    let pw = self.password.as_ref().expect("password branch set the flag");
                    match client
                        .encryption()
                        .bootstrap_cross_signing(None)
                        .await
                    {
                        Ok(()) => {
                            tracing::info!("Matrix: cross-signing bootstrap successful (no UIA needed)");
                        }
                        Err(e) => {
                            if let Some(response) = e.as_uiaa_response() {
                                let mut password_auth = uiaa::Password::new(
                                    uiaa::UserIdentifier::UserIdOrLocalpart(
                                        resolved_user_id.clone(),
                                    ),
                                    pw.clone(),
                                );
                                password_auth.session = response.session.clone();
                                match client
                                    .encryption()
                                    .bootstrap_cross_signing(Some(uiaa::AuthData::Password(
                                        password_auth,
                                    )))
                                    .await
                                {
                                    Ok(()) => {
                                        tracing::info!(
                                            "Matrix: cross-signing bootstrap successful (with password)"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Matrix: cross-signing bootstrap with password failed: {e}"
                                        );
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "Matrix: cross-signing bootstrap failed (non-UIA): {e}"
                                );
                            }
                        }
                    }
                }

                Ok::<MatrixSdkClient, anyhow::Error>(client)
            })
            .await?;

        Ok(client.clone())
    }

    async fn resolve_room_id(&self) -> anyhow::Result<String> {
        self.resolve_room_reference(&self.room_id).await
    }

    async fn ensure_room_accessible(&self, room_id: &str) -> anyhow::Result<()> {
        let encoded_room = Self::encode_path_segment(room_id);
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/joined_members",
            self.homeserver, encoded_room
        );

        let resp = self
            .http_client
            .get(&url)
            .header("Authorization", self.auth_header_value().await?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Matrix room access check failed for '{room_id}': {err}");
        }

        Ok(())
    }

    async fn room_is_encrypted(&self, room_id: &str) -> anyhow::Result<bool> {
        let encoded_room = Self::encode_path_segment(room_id);
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.encryption",
            self.homeserver, encoded_room
        );

        let resp = self
            .http_client
            .get(&url)
            .header("Authorization", self.auth_header_value().await?)
            .send()
            .await?;

        if resp.status().is_success() {
            return Ok(true);
        }

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }

        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("Matrix room encryption check failed for '{room_id}': {err}");
    }

    async fn ensure_room_supported(&self, room_id: &str) -> anyhow::Result<()> {
        self.ensure_room_accessible(room_id).await?;

        let encrypted = self.room_is_encrypted(room_id).await?;
        if encrypted {
            tracing::info!(
                "Matrix room {} is encrypted; E2EE decryption is enabled via matrix-sdk.",
                room_id
            );
        }

        record_matrix_call_room_ready(room_id, encrypted);

        Ok(())
    }

    async fn joined_target_room(&self) -> anyhow::Result<(Room, String)> {
        let target_room_id = self.target_room_id().await?;
        let room = self.joined_room_by_id(&target_room_id).await?;
        Ok((room, target_room_id))
    }

    async fn joined_room_by_id(&self, room_id: &str) -> anyhow::Result<Room> {
        let client = self.matrix_client().await?;
        self.ensure_room_supported(room_id).await?;

        let target_room: OwnedRoomId = room_id.parse()?;
        let mut room = client.get_room(&target_room);
        if room.is_none() {
            let _ = client.sync_once(SyncSettings::new()).await;
            room = client.get_room(&target_room);
        }

        let Some(room) = room else {
            anyhow::bail!("Matrix room '{}' not found in joined rooms", room_id);
        };

        if room.state() != RoomState::Joined {
            anyhow::bail!("Matrix room '{}' is not in joined state", room_id);
        }

        Ok(room)
    }

    async fn find_or_create_direct_room_for_user(
        &self,
        user_id: &OwnedUserId,
    ) -> anyhow::Result<String> {
        let client = self.matrix_client().await?;
        if let Some(room) = client.get_dm_room(user_id.as_ref()) {
            if room.state() == RoomState::Joined {
                return Ok(room.room_id().to_string());
            }
        }

        let room = client.create_dm(user_id.as_ref()).await?;
        Ok(room.room_id().to_string())
    }

    async fn room_is_allowed_call_target(&self, room: &Room, sender: &OwnedUserId) -> bool {
        if room.room_id().as_str() == self.room_id {
            return true;
        }

        if let Ok(target_room_id) = self.target_room_id().await {
            if room.room_id().as_str() == target_room_id {
                return true;
            }
        }

        let sender_ref: &matrix_sdk::ruma::UserId = sender.as_ref();
        room.is_direct().await.unwrap_or(false)
            && room
                .direct_targets()
                .contains(<&DirectUserIdentifier>::from(sender_ref))
    }

    async fn room_for_call_control_id(&self, call_control_id: &str) -> anyhow::Result<Room> {
        if let Some(room_id) = matrix_call_session(call_control_id)
            .and_then(|session| session.call_session_id)
            .filter(|room_id| !room_id.trim().is_empty())
        {
            return self.joined_room_by_id(&room_id).await;
        }
        let (room, _) = self.joined_target_room().await?;
        Ok(room)
    }

    async fn call_target_room_and_mentions(
        &self,
        to: &str,
    ) -> anyhow::Result<(Room, String, Mentions)> {
        let normalized = to.trim();
        if normalized.eq_ignore_ascii_case("room")
            || normalized == self.room_id
            || normalized.starts_with('!')
            || normalized.starts_with('#')
        {
            let target_room_id =
                if normalized.eq_ignore_ascii_case("room") || normalized == self.room_id {
                    self.target_room_id().await?
                } else {
                    self.resolve_room_reference(normalized).await?
                };
            let room = self.joined_room_by_id(&target_room_id).await?;
            return Ok((room, target_room_id, Mentions::with_room_mention()));
        }

        let user_id: OwnedUserId = normalized.parse().map_err(|_| {
            anyhow::anyhow!(
                "Matrix call start requires `to` to be a Matrix user id like `@user:example.com`, `room`, `!room:id`, or `#alias:server`"
            )
        })?;
        let target_room_id = self.find_or_create_direct_room_for_user(&user_id).await?;
        let room = self.joined_room_by_id(&target_room_id).await?;
        Ok((room, target_room_id, Mentions::with_user_ids([user_id])))
    }

    pub fn from_call_runtime_config(config: synapse_domain::config::schema::MatrixConfig) -> Self {
        Self::from_call_runtime_config_with_support(config, None, None, None)
    }

    pub fn from_call_runtime_config_with_synapseclaw_dir(
        config: synapse_domain::config::schema::MatrixConfig,
        synapseclaw_dir: Option<PathBuf>,
    ) -> Self {
        Self::from_call_runtime_config_with_support(config, synapseclaw_dir, None, None)
    }

    pub fn from_call_runtime_config_with_support(
        config: synapse_domain::config::schema::MatrixConfig,
        synapseclaw_dir: Option<PathBuf>,
        tts: Option<TtsConfig>,
        transcription: Option<synapse_domain::config::schema::TranscriptionConfig>,
    ) -> Self {
        let mut channel = MatrixChannel::new_with_session_hint_and_synapseclaw_dir(
            config.homeserver,
            config.access_token,
            config.room_id,
            config.allowed_users,
            config.user_id,
            config.device_id,
            synapseclaw_dir,
        )
        .with_password(config.password)
        .with_max_media_download_mb(config.max_media_download_mb);
        if let Some(transcription) = transcription {
            channel = channel.with_transcription(transcription);
        }
        if let Some(tts) = tts {
            channel = channel.with_tts(tts);
        }
        channel
    }

    fn media_call_room_id(&self, call_control_id: &str) -> anyhow::Result<String> {
        let session = matrix_call_session(call_control_id)
            .ok_or_else(|| anyhow::anyhow!("unknown Matrix call_control_id `{call_control_id}`"))?;
        if session.state.is_terminal() {
            anyhow::bail!(
                "cannot attach Matrix media to terminal call `{call_control_id}` in state {:?}",
                session.state
            );
        }
        session
            .call_session_id
            .or(session.origin.conversation_id)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Matrix call `{call_control_id}` has no resolved room_id for media attach"
                )
            })
    }

    fn media_tts_config(&self) -> anyhow::Result<&TtsConfig> {
        let tts = self.tts.as_ref().ok_or_else(|| {
            anyhow::anyhow!("speech_synthesis lane is not configured for Matrix call media")
        })?;
        if !tts.enabled {
            anyhow::bail!("speech_synthesis lane is disabled for Matrix call media");
        }
        let provider_format = tts_provider_output_format(tts);
        let normalized = provider_format.trim().to_ascii_lowercase();
        if !matches!(normalized.as_str(), "wav" | "wave") {
            anyhow::bail!(
                "Matrix call media currently requires WAV PCM16 TTS output; resolved provider format is `{provider_format}`"
            );
        }
        Ok(tts)
    }

    fn live_turn_engine_config(&self) -> anyhow::Result<DeepgramFluxSessionConfig> {
        DeepgramFluxSessionConfig::from_transcription(self.transcription.as_ref())
            .context("Matrix live calls require a ready Deepgram Flux turn engine")
    }

    fn media_call_context(
        &self,
        call_control_id: &str,
    ) -> anyhow::Result<(String, String, String)> {
        let session = matrix_call_session(call_control_id)
            .ok_or_else(|| anyhow::anyhow!("unknown Matrix call_control_id `{call_control_id}`"))?;
        let sender = session
            .origin
            .recipient
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Matrix call `{call_control_id}` is missing caller identity for media ingress"
                )
            })?;
        let room_id = session
            .call_session_id
            .clone()
            .or(session.origin.conversation_id.clone())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Matrix call `{call_control_id}` is missing room_id"))?;
        let reply_target = matrix_call_reply_target(&sender, &room_id);
        Ok((sender, room_id, reply_target))
    }

    async fn send_matrix_call_media_key_to_user(
        &self,
        client: &MatrixSdkClient,
        target_user_id: &OwnedUserId,
        room_id: &str,
        own_user_id: &str,
        own_device_id: &str,
        own_member_id: &str,
        key_index: i32,
        key: &[u8],
    ) -> anyhow::Result<()> {
        let devices = client.encryption().get_user_devices(target_user_id).await?;
        let recipient_devices_owned: Vec<_> = devices.devices().collect();
        if recipient_devices_owned.is_empty() {
            anyhow::bail!("no E2EE-capable recipient devices known for {}", target_user_id);
        }
        let recipient_devices: Vec<_> = recipient_devices_owned.iter().collect();

        let content = serde_json::json!({
            "keys": {
                "index": key_index,
                "key": STANDARD_NO_PAD.encode(key),
            },
            "member": {
                "id": own_member_id,
                "claimed_device_id": own_device_id,
            },
            "room_id": room_id,
            "session": {
                "application": "m.call",
                "call_id": "",
                "scope": "m.room",
            },
            "sent_ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        let content = matrix_sdk::ruma::serde::Raw::new(&content)?.cast_unchecked::<AnyToDeviceEventContent>();
        let failures = client
            .encryption()
            .encrypt_and_send_raw_to_device(
                recipient_devices,
                MATRIX_CALL_ENCRYPTION_EVENT_TYPE,
                content,
                CollectStrategy::AllDevices,
            )
            .await?;
        if !failures.is_empty() {
            tracing::warn!(
                recipient = %target_user_id,
                failures = failures.len(),
                "Matrix call media key delivery had recipient failures"
            );
        } else {
            tracing::info!(
                recipient = %target_user_id,
                room_id = %room_id,
                sender_user_id = %own_user_id,
                sender_device_id = %own_device_id,
                "Matrix call media key delivered to recipient devices"
            );
        }
        Ok(())
    }

    async fn send_matrix_call_media_key_for_call(
        &self,
        client: &MatrixSdkClient,
        call_control_id: &str,
        session: &MatrixMediaSession,
    ) {
        let Ok((sender, room_id, _)) = self.media_call_context(call_control_id) else {
            return;
        };
        let Ok(target_user_id) = sender.parse::<OwnedUserId>() else {
            tracing::warn!(
                call_control_id = %call_control_id,
                sender = %sender,
                "failed to parse Matrix call recipient user id for media key delivery"
            );
            return;
        };
        if let Err(error) = self
            .send_matrix_call_media_key_to_user(
                client,
                &target_user_id,
                &room_id,
                &session.own_user_id,
                &session.own_device_id,
                &session.own_member_id,
                session.local_key_index,
                &session.local_media_key,
            )
            .await
        {
            tracing::warn!(
                call_control_id = %call_control_id,
                recipient = %target_user_id,
                error = %error,
                "failed to deliver Matrix call local media key"
            );
        }
    }

    async fn handle_matrix_call_encryption_to_device_event(
        &self,
        event: serde_json::Value,
    ) {
        let Some(event_type) = event.get("type").and_then(|value| value.as_str()) else {
            return;
        };
        if event_type != MATRIX_CALL_ENCRYPTION_EVENT_TYPE {
            return;
        }
        let Some(sender) = event.get("sender").and_then(|value| value.as_str()) else {
            return;
        };
        let Some(content_value) = event.get("content") else {
            return;
        };
        let Ok(content) = serde_json::from_value::<MatrixCallEncryptionToDeviceContent>(
            content_value.clone(),
        ) else {
            tracing::warn!(
                sender = %sender,
                content = %content_value,
                "failed to parse Matrix call encryption to-device content"
            );
            return;
        };
        let member_id = if content.member.id.trim().is_empty() {
            matrix_call_member_id(sender, &content.member.claimed_device_id)
        } else {
            content.member.id.clone()
        };
        let Ok(key) = decode_matrix_call_media_key(content.keys.key.trim()) else {
            tracing::warn!(
                sender = %sender,
                room_id = %content.room_id,
                key_len = content.keys.key.len(),
                "failed to decode Matrix call encryption key payload"
            );
            return;
        };

        let sessions: Vec<(String, MatrixMediaSession)> = matrix_media_sessions_slot()
            .read()
            .iter()
            .filter(|(call_control_id, session)| {
                session.media_room_id == content.room_id
                    && matrix_call_state_accepts_live_audio_stream(call_control_id)
            })
            .map(|(call_control_id, session)| (call_control_id.clone(), session.clone()))
            .collect();

        if sessions.is_empty() {
            tracing::debug!(
                sender = %sender,
                room_id = %content.room_id,
                "received Matrix call encryption key without an active matching media session"
            );
            return;
        }

        for (call_control_id, session) in sessions {
            matrix_call_apply_media_key(
                &session.e2ee_key_provider,
                sender,
                &content.member.claimed_device_id,
                &member_id,
                content.keys.index,
                &key,
            );
            tracing::info!(
                call_control_id = %call_control_id,
                sender = %sender,
                room_id = %content.room_id,
                claimed_device_id = %content.member.claimed_device_id,
                member_id = %member_id,
                key_index = content.keys.index,
                "applied Matrix call media decryption key to LiveKit key provider"
            );
        }
    }

    async fn emit_turn_engine_transcript(
        tx: mpsc::Sender<ChannelMessage>,
        call_control_id: String,
        sender: String,
        reply_target: String,
        turn_index: u64,
        transcript: String,
    ) -> anyhow::Result<()> {
        let Some(text) = sanitize_live_call_transcript(&transcript) else {
            tracing::debug!(
                call_control_id = %call_control_id,
                turn_index,
                "dropped unstable Matrix Flux transcript turn"
            );
            return Ok(());
        };

        if !matrix_call_state_accepts_turn_delivery(&call_control_id) {
            tracing::debug!(
                call_control_id = %call_control_id,
                turn_index,
                "dropped Matrix Flux transcript while call was not in an active delivery state"
            );
            return Ok(());
        }

        tracing::info!(
            call_control_id = %call_control_id,
            turn_index,
            transcript_chars = text.chars().count(),
            transcript = %bounded_live_call_transcript_log(&text),
            "Matrix live call transcript accepted"
        );

        tx.send(ChannelMessage {
            id: format!("matrix-call-turn:{call_control_id}:{turn_index}"),
            sender,
            reply_target,
            content: text,
            channel: "matrix".to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            thread_ts: None,
            media_attachments: Vec::new(),
        })
        .await
        .map_err(|error| anyhow::anyhow!("failed to enqueue Matrix call turn: {error}"))
    }

    async fn handle_turn_engine_event(
        tx: &mpsc::Sender<ChannelMessage>,
        call_control_id: &str,
        sender: &str,
        reply_target: &str,
        event: RealtimeTurnEvent,
    ) -> anyhow::Result<()> {
        match event {
            RealtimeTurnEvent::Update {
                turn_index,
                transcript,
                languages,
                end_of_turn_confidence,
            } => {
                tracing::info!(
                    call_control_id = %call_control_id,
                    turn_index,
                    languages = ?languages,
                    confidence = end_of_turn_confidence.unwrap_or_default(),
                    transcript_chars = transcript.as_ref().map(|value| value.chars().count()).unwrap_or(0),
                    transcript = %transcript.as_deref().map(bounded_live_call_transcript_log).unwrap_or_default(),
                    "Matrix Flux update"
                );
            }
            RealtimeTurnEvent::StartOfTurn {
                turn_index,
                transcript,
            } => {
                tracing::info!(
                    call_control_id = %call_control_id,
                    turn_index,
                    transcript_chars = transcript.as_ref().map(|value| value.chars().count()).unwrap_or(0),
                    "Matrix Flux detected start of turn"
                );
                interrupt_matrix_call_playback(call_control_id);
            }
            RealtimeTurnEvent::EagerEndOfTurn {
                turn_index,
                transcript,
                languages,
            } => {
                tracing::info!(
                    call_control_id = %call_control_id,
                    turn_index,
                    languages = ?languages,
                    transcript_chars = transcript.chars().count(),
                    transcript = %bounded_live_call_transcript_log(&transcript),
                    "Matrix Flux detected eager end of turn"
                );
            }
            RealtimeTurnEvent::TurnResumed {
                turn_index,
                transcript,
            } => {
                tracing::info!(
                    call_control_id = %call_control_id,
                    turn_index,
                    transcript_chars = transcript.as_ref().map(|value| value.chars().count()).unwrap_or(0),
                    "Matrix Flux resumed a previously eager turn"
                );
                interrupt_matrix_call_playback(call_control_id);
            }
            RealtimeTurnEvent::EndOfTurn {
                turn_index,
                transcript,
                languages,
            } => {
                tracing::info!(
                    call_control_id = %call_control_id,
                    turn_index,
                    languages = ?languages,
                    transcript_chars = transcript.chars().count(),
                    "Matrix Flux detected end of turn"
                );
                Self::emit_turn_engine_transcript(
                    tx.clone(),
                    call_control_id.to_string(),
                    sender.to_string(),
                    reply_target.to_string(),
                    turn_index,
                    transcript,
                )
                .await?;
            }
            RealtimeTurnEvent::Error { description, .. } => {
                record_matrix_turn_engine_error(&description);
                tracing::warn!(
                    call_control_id = %call_control_id,
                    error = %description,
                    "Matrix Flux turn engine reported an error"
                );
            }
            RealtimeTurnEvent::Closed => {
                tracing::info!(
                    call_control_id = %call_control_id,
                    "Matrix Flux turn engine closed"
                );
            }
        }

        Ok(())
    }

    fn spawn_remote_audio_ingress(
        &self,
        call_control_id: &str,
        audio_track: LiveKitRemoteAudioTrack,
        session: MatrixMediaSession,
    ) {
        let Some(ingress) = get_realtime_audio_ingress("matrix") else {
            session.ingress_started.store(false, Ordering::Release);
            return;
        };
        let turn_engine_config = match self.live_turn_engine_config() {
            Ok(config) => config,
            Err(error) => {
                record_matrix_turn_engine_error(&error.to_string());
                session.ingress_started.store(false, Ordering::Release);
                return;
            }
        };
        if ingress.transcription.is_none() {
            session.ingress_started.store(false, Ordering::Release);
            return;
        };
        let Ok((sender, _room_id, reply_target)) = self.media_call_context(call_control_id) else {
            session.ingress_started.store(false, Ordering::Release);
            return;
        };

        let tx = ingress.tx.clone();
        let transcription_config = ingress.transcription.clone();
        let call_control_id = call_control_id.to_string();
        let debug_dump_dir = self.live_call_debug_dir();
        tokio::spawn(async move {
            let result = async {
                let engine = DeepgramFluxTurnEngine::new(turn_engine_config.clone());
                let session_handle = engine
                    .connect(
                        MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE,
                        MATRIX_CALL_TRANSCRIPT_CHANNELS,
                    )
                    .await
                    .context("failed to connect Matrix call turn engine")?;
                tracing::info!(
                    call_control_id = %call_control_id,
                    provider = engine.provider_name(),
                    sample_rate = MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE,
                    channels = MATRIX_CALL_TRANSCRIPT_CHANNELS,
                    "Matrix live turn engine connected"
                );
                clear_matrix_turn_engine_error();
                let (control, mut event_rx) = session_handle.split();
                let mut stream = LiveKitNativeAudioStream::new(
                    audio_track.rtc_track(),
                    MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as i32,
                    MATRIX_CALL_TRANSCRIPT_CHANNELS as i32,
                );
                let mut batcher = RealtimePcm16StreamBatcher::new(
                    MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE,
                    MATRIX_CALL_TRANSCRIPT_CHANNELS,
                );
                let mut frame_index = 0usize;
                let mut chunk_index = 0usize;
                let mut total_frame_samples = 0usize;
                let mut total_chunk_samples = 0usize;
                let mut waits_without_frames = 0usize;
                let mut debug_capture = Vec::new();
                let max_debug_samples =
                    (MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as usize) * MATRIX_CALL_DEBUG_CAPTURE_MAX_SECS;
                let tx_for_events = tx.clone();
                let call_for_events = call_control_id.clone();
                let sender_for_events = sender.clone();
                let reply_target_for_events = reply_target.clone();
                let event_task = tokio::spawn(async move {
                    while let Some(event) = event_rx.recv().await {
                        if let Err(error) = Self::handle_turn_engine_event(
                            &tx_for_events,
                            &call_for_events,
                            &sender_for_events,
                            &reply_target_for_events,
                            event,
                        )
                        .await
                        {
                            tracing::warn!(
                                call_control_id = %call_for_events,
                                error = %error,
                                "Matrix call turn engine event handling failed"
                            );
                        }
                    }
                });
                tracing::info!(
                    call_control_id = %call_control_id,
                    "Matrix remote audio ingress started"
                );

                loop {
                    let next_frame = tokio::time::timeout(Duration::from_secs(10), stream.next()).await;
                    let Some(frame) = (match next_frame {
                        Ok(frame) => frame,
                        Err(_) => {
                            if !matrix_media_session_exists(&call_control_id)
                                || !matrix_call_state_accepts_live_audio_stream(&call_control_id)
                            {
                                tracing::info!(
                                    call_control_id = %call_control_id,
                                    waits_without_frames,
                                    frames_received = frame_index,
                                    chunks_sent = chunk_index,
                                    "Matrix remote audio ingress stopping after call became inactive"
                                );
                                break;
                            }
                            waits_without_frames = waits_without_frames.saturating_add(1);
                            tracing::info!(
                                call_control_id = %call_control_id,
                                waits_without_frames,
                                frames_received = frame_index,
                                chunks_sent = chunk_index,
                                "Matrix remote audio ingress still waiting for frames"
                            );
                            continue;
                        }
                    }) else {
                        break;
                    };

                    if !matrix_call_state_accepts_live_audio_stream(&call_control_id) {
                        tracing::info!(
                            call_control_id = %call_control_id,
                            frame_index,
                            "Matrix remote audio ingress stopping because call no longer accepts live audio"
                        );
                        break;
                    }
                    waits_without_frames = 0;
                    append_bounded_debug_samples(
                        &mut debug_capture,
                        &frame.data,
                        max_debug_samples,
                    );
                    frame_index = frame_index.saturating_add(1);
                    total_frame_samples = total_frame_samples.saturating_add(frame.data.len());
                    let (peak, nonzero_samples) = pcm16_signal_stats(&frame.data);
                    if frame_index <= 5 || frame_index % 100 == 0 {
                        tracing::info!(
                            call_control_id = %call_control_id,
                            frame_index,
                            frame_samples = frame.data.len(),
                            frame_ms = ((frame.data.len() as u64) * 1000)
                                / MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as u64,
                            peak_sample = peak,
                            nonzero_samples,
                            buffered_samples = batcher.buffered_samples(),
                            "Matrix remote audio frame received"
                        );
                    }
                    for chunk in batcher.push_frame(&frame.data) {
                        chunk_index = chunk_index.saturating_add(1);
                        total_chunk_samples = total_chunk_samples.saturating_add(chunk.len());
                        if chunk_index <= 5 || chunk_index % 20 == 0 {
                            let (peak, nonzero_samples) = pcm16_signal_stats(&chunk);
                            tracing::info!(
                                call_control_id = %call_control_id,
                                chunk_index,
                                chunk_samples = chunk.len(),
                                chunk_ms = ((chunk.len() as u64) * 1000)
                                    / MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as u64,
                                peak_sample = peak,
                                nonzero_samples,
                                buffered_samples = batcher.buffered_samples(),
                                "Matrix remote audio chunk forwarded to Flux"
                            );
                        }
                        if control.send_audio(chunk).await.is_err() {
                            anyhow::bail!("Matrix call turn engine audio sender is closed");
                        }
                    }
                }

                if let Some(chunk) = batcher.finish() {
                    chunk_index = chunk_index.saturating_add(1);
                    total_chunk_samples = total_chunk_samples.saturating_add(chunk.len());
                    let (peak, nonzero_samples) = pcm16_signal_stats(&chunk);
                    tracing::info!(
                        call_control_id = %call_control_id,
                        chunk_index,
                        chunk_samples = chunk.len(),
                        chunk_ms = ((chunk.len() as u64) * 1000)
                            / MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as u64,
                        peak_sample = peak,
                        nonzero_samples,
                        "Matrix remote audio final chunk forwarded to Flux"
                    );
                    let _ = control.send_audio(chunk).await;
                }
                tracing::info!(
                    call_control_id = %call_control_id,
                    frames_received = frame_index,
                    frame_samples = total_frame_samples,
                    chunks_sent = chunk_index,
                    chunk_samples = total_chunk_samples,
                    "Matrix remote audio ingress stream ended"
                );

                let min_debug_samples =
                    (MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as usize) * MATRIX_CALL_DEBUG_CAPTURE_MIN_SECS;
                if debug_capture.len() >= min_debug_samples {
                    let wav = pcm16_wav_bytes(
                        MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE,
                        MATRIX_CALL_TRANSCRIPT_CHANNELS as u16,
                        &debug_capture,
                    );
                    match persist_matrix_live_call_debug_artifact(
                        debug_dump_dir.as_deref(),
                        &call_control_id,
                        &wav,
                    )
                    .await
                    {
                        Ok(Some(path)) => {
                            tracing::info!(
                                call_control_id = %call_control_id,
                                path = %path.display(),
                                captured_ms = ((debug_capture.len() as u64) * 1000)
                                    / MATRIX_CALL_TRANSCRIPT_SAMPLE_RATE as u64,
                                "Matrix live call debug WAV captured"
                            );
                        }
                        Ok(None) => {
                            tracing::debug!(
                                call_control_id = %call_control_id,
                                "Matrix live call debug WAV skipped because workspace dir is not configured"
                            );
                        }
                        Err(error) => {
                            tracing::warn!(
                                call_control_id = %call_control_id,
                                error = %error,
                                "Failed to persist Matrix live call debug WAV"
                            );
                        }
                    }

                    if let Some(transcription) = transcription_config.as_ref() {
                        match transcribe_matrix_live_call_debug_wav(&wav, transcription).await {
                            Ok(transcript) => {
                                let trimmed = transcript.trim();
                                tracing::info!(
                                    call_control_id = %call_control_id,
                                    provider = %transcription.default_provider,
                                    transcript_chars = trimmed.chars().count(),
                                    transcript = %bounded_live_call_transcript_log(trimmed),
                                    "Matrix live call debug batch transcription completed"
                                );
                            }
                            Err(error) => {
                                tracing::warn!(
                                    call_control_id = %call_control_id,
                                    provider = %transcription.default_provider,
                                    error = %error,
                                    "Matrix live call debug batch transcription failed"
                                );
                            }
                        }
                    }
                } else {
                    tracing::info!(
                        call_control_id = %call_control_id,
                        captured_samples = debug_capture.len(),
                        "Matrix live call debug capture skipped because inbound audio was too short"
                    );
                }

                let _ = control.close().await;
                let _ = event_task.await;

                Ok::<(), anyhow::Error>(())
            }
            .await;

            session.ingress_started.store(false, Ordering::Release);
            if let Err(error) = result {
                tracing::warn!(
                    call_control_id = %call_control_id,
                    error = %error,
                    "Matrix remote audio ingress stopped with error"
                );
            }
        });
    }

    async fn close_media_session(&self, call_control_id: &str) {
        if let Err(error) = self.clear_call_membership_for_call(call_control_id).await {
            tracing::debug!(
                call_control_id = %call_control_id,
                error = %error,
                "failed to clear Matrix call membership while closing media session"
            );
        }
        let Some(session) = remove_matrix_media_session(call_control_id) else {
            return;
        };
        if let Err(error) = session.room.close().await {
            tracing::debug!(
                call_control_id = %call_control_id,
                error = %error,
                "failed to close Matrix media session"
            );
        }
    }

    async fn ensure_media_session(
        &self,
        call_control_id: &str,
    ) -> anyhow::Result<MatrixMediaSession> {
        if let Some(session) = matrix_media_session(call_control_id) {
            return Ok(session);
        }

        self.media_tts_config()?;
        self.live_turn_engine_config()?;

        let room_id = self.media_call_room_id(call_control_id)?;
        let bootstrap = self.probe_matrix_rtc_bootstrap(&room_id).await;
        record_matrix_rtc_bootstrap_status(bootstrap.clone());
        if !bootstrap.media_bootstrap_ready {
            anyhow::bail!(
                "MatrixRTC bootstrap is not ready for room `{room_id}`{}",
                bootstrap
                    .last_probe_error
                    .as_deref()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            );
        }

        let focus_url = bootstrap
            .focus_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("MatrixRTC bootstrap returned no focus URL"))?;
        let openid_token = self.request_openid_token().await?;
        let grant = self
            .request_matrix_rtc_authorizer_grant(
                focus_url,
                &room_id,
                bootstrap.authorizer_api,
                openid_token,
            )
            .await?;
        let client = self.matrix_client().await?;

        let own_user_id = client
            .user_id()
            .ok_or_else(|| anyhow::anyhow!("Matrix session is missing own user_id"))?
            .to_string();
        let own_device_id = client
            .device_id()
            .ok_or_else(|| anyhow::anyhow!("Matrix session is missing own device_id"))?
            .to_string();
        let own_member_id = matrix_call_member_id(&own_user_id, &own_device_id);
        let e2ee_key_provider = LiveKitKeyProvider::new(LiveKitKeyProviderOptions {
            ratchet_window_size: 10,
            key_ring_size: 256,
            key_derivation_algorithm: livekit::e2ee::key_provider::KeyDerivationAlgorithm::HKDF,
            ..LiveKitKeyProviderOptions::default()
        });
        let local_media_key = matrix_call_generate_local_media_key();
        matrix_call_apply_media_key(
            &e2ee_key_provider,
            &own_user_id,
            &own_device_id,
            &own_member_id,
            MATRIX_CALL_ENCRYPTION_KEY_INDEX,
            &local_media_key,
        );

        let mut room_options = LiveKitRoomOptions::default();
        room_options.encryption = Some(LiveKitE2eeOptions {
            encryption_type: LiveKitEncryptionType::Gcm,
            key_provider: e2ee_key_provider.clone(),
        });
        let (room, mut rx) = LiveKitRoom::connect(
            &grant.livekit_service_url,
            &grant.jwt,
            room_options,
        )
        .await
        .map_err(|error| anyhow::anyhow!("failed to join Matrix LiveKit room: {error}"))?;
        let room = Arc::new(room);
        let session = MatrixMediaSession {
            room: Arc::clone(&room),
            media_room_id: room_id.clone(),
            e2ee_key_provider,
            own_user_id,
            own_device_id,
            own_member_id,
            local_media_key,
            local_key_index: MATRIX_CALL_ENCRYPTION_KEY_INDEX,
            speak_gate: Arc::new(Mutex::new(())),
            ingress_started: Arc::new(AtomicBool::new(false)),
            playback_epoch: Arc::new(AtomicU64::new(0)),
        };
        let joined_room = self.joined_room_by_id(&room_id).await?;
        self.announce_call_membership(&joined_room, &room_id, focus_url, call_control_id)
            .await?;
        insert_matrix_media_session(call_control_id, session.clone());
        record_matrix_call_state(call_control_id, RealtimeCallState::Connected);
        record_matrix_call_state(call_control_id, RealtimeCallState::Listening);
        self.send_matrix_call_media_key_for_call(&client, call_control_id, &session)
            .await;

        let call_control_id = call_control_id.to_string();
        let channel = self.clone();
        let session_for_events = session.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    LiveKitRoomEvent::Connected { .. }
                    | LiveKitRoomEvent::Reconnected
                    | LiveKitRoomEvent::ConnectionStateChanged(LiveKitConnectionState::Connected) =>
                    {
                        record_matrix_call_state(&call_control_id, RealtimeCallState::Listening);
                    }
                    LiveKitRoomEvent::TrackSubscribed {
                        track,
                        publication,
                        participant,
                    } => {
                        if let LiveKitRemoteTrack::Audio(audio_track) = track {
                            let source = publication.source();
                            let encryption_type = publication.encryption_type();
                            tracing::info!(
                                call_control_id = %call_control_id,
                                participant_identity = %participant.identity(),
                                publication_sid = %publication.sid(),
                                publication_source = ?source,
                                publication_encryption = ?encryption_type,
                                track_source = ?audio_track.source(),
                                track_name = %audio_track.name(),
                                "Matrix LiveKit remote audio track subscribed"
                            );
                            if !matrix_livekit_remote_audio_track_is_preferred(
                                source,
                                encryption_type,
                            ) {
                                tracing::info!(
                                    call_control_id = %call_control_id,
                                    participant_identity = %participant.identity(),
                                    publication_sid = %publication.sid(),
                                    publication_source = ?source,
                                    publication_encryption = ?encryption_type,
                                    "Matrix remote audio track ignored because it is not the preferred microphone track"
                                );
                                continue;
                            }
                            record_matrix_call_state(
                                &call_control_id,
                                RealtimeCallState::Listening,
                            );
                            if session_for_events
                                .ingress_started
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                            {
                                channel.spawn_remote_audio_ingress(
                                    &call_control_id,
                                    audio_track,
                                    session_for_events.clone(),
                                );
                            }
                        }
                    }
                    LiveKitRoomEvent::TrackPublished { publication, participant } => {
                        let should_subscribe = matrix_livekit_remote_publication_should_subscribe(
                            publication.kind(),
                            publication.source(),
                        );
                        tracing::info!(
                            call_control_id = %call_control_id,
                            participant_identity = %participant.identity(),
                            publication_sid = %publication.sid(),
                            publication_kind = ?publication.kind(),
                            publication_source = ?publication.source(),
                            publication_encryption = ?publication.encryption_type(),
                            publication_name = %publication.name(),
                            should_subscribe,
                            "Matrix LiveKit remote track published"
                        );
                        if should_subscribe {
                            publication.set_subscribed(true);
                        }
                    }
                    LiveKitRoomEvent::TrackSubscriptionFailed {
                        participant,
                        track_sid,
                        error,
                    } => {
                        tracing::warn!(
                            call_control_id = %call_control_id,
                            participant_identity = %participant.identity(),
                            track_sid = %track_sid,
                            error = %error,
                            "Matrix LiveKit remote track subscription failed"
                        );
                    }
                    LiveKitRoomEvent::Disconnected { .. }
                    | LiveKitRoomEvent::ConnectionStateChanged(
                        LiveKitConnectionState::Disconnected,
                    ) => {
                        if let Err(error) = channel
                            .clear_call_membership_for_call(&call_control_id)
                            .await
                        {
                            tracing::debug!(
                                call_control_id = %call_control_id,
                                error = %error,
                                "failed to clear Matrix call membership after disconnect"
                            );
                        }
                        remove_matrix_media_session(&call_control_id);
                        record_matrix_call_ended_if_active(&call_control_id, "remote_hangup");
                        break;
                    }
                    LiveKitRoomEvent::Reconnecting
                    | LiveKitRoomEvent::ConnectionStateChanged(
                        LiveKitConnectionState::Reconnecting,
                    ) => {
                        record_matrix_call_state(&call_control_id, RealtimeCallState::Connected);
                    }
                    _ => {}
                }
            }
            remove_matrix_media_session(&call_control_id);
        });

        Ok(session)
    }

    async fn speak_into_media_session(
        &self,
        session: &MatrixMediaSession,
        call_control_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let _guard = session.speak_gate.lock().await;
        let playback_epoch = session.playback_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let tts = self.media_tts_config()?;
        let manager = super::tts::TtsManager::new(tts)?;
        let audio = manager.synthesize(text).await?;
        if session.playback_epoch.load(Ordering::Acquire) != playback_epoch {
            record_matrix_call_state(call_control_id, RealtimeCallState::Listening);
            return Ok(());
        }
        let payload = wav_pcm16_payload(&audio).ok_or_else(|| {
            anyhow::anyhow!(
                "Matrix call media requires 16-bit PCM WAV synthesis; provider output was not a compatible WAV stream"
            )
        })?;
        if payload.channels == 0 || payload.channels > 2 {
            anyhow::bail!(
                "Matrix call media currently supports mono or stereo WAV; got {} channels",
                payload.channels
            );
        }

        let source = LiveKitNativeAudioSource::new(
            LiveKitAudioSourceOptions::default(),
            payload.sample_rate,
            payload.channels,
            1_000,
        );
        let track = LiveKitLocalAudioTrack::create_audio_track(
            "synapseclaw-tts",
            RtcAudioSource::Native(source.clone()),
        );
        let publication = session
            .room
            .local_participant()
            .publish_track(
                LiveKitLocalTrack::Audio(track),
                LiveKitTrackPublishOptions {
                    source: LiveKitTrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| {
                anyhow::anyhow!("failed to publish Matrix LiveKit audio track: {error}")
            })?;

        record_matrix_call_state(call_control_id, RealtimeCallState::Speaking);

        const FRAME_DURATION_MS: u32 = 20;
        const TRACK_PREROLL_MS: u64 = 180;
        const TRACK_POSTROLL_MS: u64 = 180;
        let samples_per_channel =
            (payload.sample_rate.saturating_mul(FRAME_DURATION_MS) / 1000).max(1);
        let chunk_samples = samples_per_channel as usize * payload.channels as usize;
        let mut frame_count = 0usize;
        let total_audio_ms = if payload.sample_rate == 0 || payload.channels == 0 {
            0u64
        } else {
            ((payload.samples.len() as u64 / payload.channels as u64) * 1000)
                / payload.sample_rate as u64
        };
        tracing::info!(
            call_control_id = %call_control_id,
            sample_rate = payload.sample_rate,
            channels = payload.channels,
            audio_ms = total_audio_ms,
            "published Matrix LiveKit audio track for speech playback"
        );
        tokio::time::sleep(Duration::from_millis(TRACK_PREROLL_MS)).await;
        for chunk in payload
            .samples
            .chunks(chunk_samples.max(payload.channels as usize))
        {
            if chunk.is_empty() {
                continue;
            }
            if session.playback_epoch.load(Ordering::Acquire) != playback_epoch {
                break;
            }
            let frame = LiveKitAudioFrame {
                data: chunk.to_vec().into(),
                num_channels: payload.channels,
                sample_rate: payload.sample_rate,
                samples_per_channel: (chunk.len() / payload.channels as usize) as u32,
            };
            source.capture_frame(&frame).await.map_err(|error| {
                anyhow::anyhow!("failed to push Matrix LiveKit audio frame: {error}")
            })?;
            frame_count += 1;
            tokio::time::sleep(Duration::from_millis(FRAME_DURATION_MS as u64)).await;
        }
        tracing::info!(
            call_control_id = %call_control_id,
            frames = frame_count,
            audio_ms = total_audio_ms,
            "completed Matrix LiveKit speech playback frames"
        );
        tokio::time::sleep(Duration::from_millis(TRACK_POSTROLL_MS)).await;

        if let Err(error) = session
            .room
            .local_participant()
            .unpublish_track(&publication.sid())
            .await
        {
            tracing::warn!(
                call_control_id = %call_control_id,
                error = %error,
                "failed to unpublish Matrix LiveKit audio track after playback"
            );
        }
        if session.playback_epoch.load(Ordering::Acquire) == playback_epoch {
            record_matrix_call_state(call_control_id, RealtimeCallState::Listening);
        }
        Ok(())
    }

    async fn wait_for_media_ingress_ready(
        &self,
        session: &MatrixMediaSession,
        call_control_id: &str,
        timeout: Duration,
    ) -> bool {
        if session.ingress_started.load(Ordering::Acquire) {
            return true;
        }
        let started = tokio::time::Instant::now();
        while started.elapsed() < timeout {
            if session.ingress_started.load(Ordering::Acquire) {
                tracing::info!(
                    call_control_id = %call_control_id,
                    waited_ms = started.elapsed().as_millis() as u64,
                    "Matrix media ingress became ready before greeting playback"
                );
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tracing::info!(
            call_control_id = %call_control_id,
            waited_ms = timeout.as_millis() as u64,
            "Matrix media ingress was not ready before greeting playback timeout"
        );
        false
    }

    #[cfg(test)]
    fn sync_filter_for_room(room_id: &str, timeline_limit: usize) -> String {
        let timeline_limit = timeline_limit.max(1);
        serde_json::json!({
            "room": {
                "rooms": [room_id],
                "timeline": {
                    "limit": timeline_limit
                }
            }
        })
        .to_string()
    }

    async fn log_e2ee_diagnostics(&self, client: &MatrixSdkClient) {
        match client.encryption().get_own_device().await {
            Ok(Some(device)) => {
                if device.is_verified() {
                    tracing::info!(
                        "Matrix device '{}' is verified for E2EE.",
                        device.device_id()
                    );
                } else {
                    tracing::warn!(
                        "Matrix device '{}' is not verified. Some clients may label bot messages as unverified until you sign/verify this device from a trusted session.",
                        device.device_id()
                    );
                }
            }
            Ok(None) => {
                tracing::warn!(
                    "Matrix own-device metadata is unavailable; verify/signing status cannot be determined."
                );
            }
            Err(error) => {
                tracing::warn!("Matrix own-device verification check failed: {error}");
            }
        }

        if client.encryption().backups().are_enabled().await {
            tracing::info!("Matrix room-key backup is enabled for this device.");
        } else {
            tracing::warn!(
                "Matrix room-key backup is not enabled for this device; `matrix_sdk_crypto::backups` warnings about missing backup keys may appear until recovery is configured."
            );
        }
    }
}

fn matrix_call_state_accepts_live_audio_stream(call_control_id: &str) -> bool {
    matrix_call_session(call_control_id)
        .map(|session| {
            !session.state.is_terminal()
                && !matches!(
                    session.state,
                    RealtimeCallState::Created | RealtimeCallState::Ringing
                )
        })
        .unwrap_or(false)
}

fn matrix_call_member_id(user_id: &str, device_id: &str) -> String {
    format!("{user_id}:{device_id}")
}

fn matrix_call_legacy_rtc_backend_identity(user_id: &str, device_id: &str) -> String {
    format!("{user_id}:{device_id}")
}

fn matrix_call_hashed_rtc_backend_identity(user_id: &str, device_id: &str, member_id: &str) -> String {
    let canonical = serde_json::to_string(&[user_id, device_id, member_id]).unwrap_or_else(|_| {
        format!(r#"["{user_id}","{device_id}","{member_id}"]"#)
    });
    let digest = Sha256::digest(canonical.as_bytes());
    STANDARD_NO_PAD.encode(digest)
}

fn matrix_call_generate_local_media_key() -> Vec<u8> {
    rand::random::<[u8; MATRIX_CALL_ENCRYPTION_KEY_LEN]>().to_vec()
}

fn decode_matrix_call_media_key(encoded: &str) -> anyhow::Result<Vec<u8>> {
    for engine in [STANDARD_NO_PAD, STANDARD, URL_SAFE_NO_PAD, URL_SAFE] {
        if let Ok(decoded) = engine.decode(encoded.as_bytes()) {
            return Ok(decoded);
        }
    }
    anyhow::bail!("unsupported Matrix call media key encoding")
}

fn matrix_call_apply_media_key(
    key_provider: &LiveKitKeyProvider,
    user_id: &str,
    device_id: &str,
    member_id: &str,
    key_index: i32,
    key: &[u8],
) {
    let legacy_identity = matrix_call_legacy_rtc_backend_identity(user_id, device_id);
    let hashed_identity = matrix_call_hashed_rtc_backend_identity(user_id, device_id, member_id);
    let legacy_identity = livekit::prelude::ParticipantIdentity::from(legacy_identity);
    let hashed_identity = livekit::prelude::ParticipantIdentity::from(hashed_identity);

    key_provider.set_key(&legacy_identity, key_index, key.to_vec());
    if hashed_identity.as_str() != legacy_identity.as_str() {
        key_provider.set_key(&hashed_identity, key_index, key.to_vec());
    }
}

fn matrix_livekit_remote_publication_should_subscribe(
    kind: LiveKitTrackKind,
    source: LiveKitTrackSource,
) -> bool {
    match kind {
        LiveKitTrackKind::Audio => matches!(
            source,
            LiveKitTrackSource::Microphone
                | LiveKitTrackSource::ScreenshareAudio
                | LiveKitTrackSource::Unknown
        ),
        LiveKitTrackKind::Video => false,
    }
}

fn matrix_livekit_remote_audio_track_is_preferred(
    source: LiveKitTrackSource,
    encryption_type: LiveKitEncryptionType,
) -> bool {
    matches!(source, LiveKitTrackSource::Microphone)
        && matches!(
            encryption_type,
            LiveKitEncryptionType::None
                | LiveKitEncryptionType::Gcm
                | LiveKitEncryptionType::Custom
        )
}

fn matrix_call_state_accepts_turn_delivery(call_control_id: &str) -> bool {
    matrix_call_session(call_control_id)
        .map(|session| {
            !session.state.is_terminal()
                && !matches!(
                    session.state,
                    RealtimeCallState::Created | RealtimeCallState::Ringing
                )
        })
        .unwrap_or(false)
}

fn interrupt_matrix_call_playback(call_control_id: &str) {
    if let Some(session) = matrix_media_session(call_control_id) {
        session.playback_epoch.fetch_add(1, Ordering::AcqRel);
    }
    record_matrix_call_state(call_control_id, RealtimeCallState::Listening);
}

fn sanitize_live_call_transcript(text: &str) -> Option<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }

    let mut visible = 0usize;
    let mut alnum = 0usize;
    let mut replacement = 0usize;
    let mut control = 0usize;

    for ch in normalized.chars() {
        if ch.is_whitespace() {
            continue;
        }
        visible = visible.saturating_add(1);
        if ch == '\u{fffd}' {
            replacement = replacement.saturating_add(1);
        }
        if ch.is_control() {
            control = control.saturating_add(1);
        }
        if ch.is_alphanumeric() {
            alnum = alnum.saturating_add(1);
        }
    }

    if replacement > 0 || control > 0 || alnum == 0 {
        return None;
    }

    if visible >= 6 && alnum.saturating_mul(3) < visible {
        return None;
    }

    Some(normalized.to_string())
}

fn pcm16_signal_stats(samples: &[i16]) -> (i16, usize) {
    let mut peak = 0i16;
    let mut nonzero = 0usize;
    for sample in samples {
        if *sample != 0 {
            nonzero = nonzero.saturating_add(1);
        }
        let abs = sample.saturating_abs();
        if abs > peak {
            peak = abs;
        }
    }
    (peak, nonzero)
}

fn append_bounded_debug_samples(dst: &mut Vec<i16>, src: &[i16], max_samples: usize) {
    if dst.len() >= max_samples || src.is_empty() {
        return;
    }
    let remaining = max_samples.saturating_sub(dst.len());
    dst.extend_from_slice(&src[..src.len().min(remaining)]);
}

fn matrix_debug_call_dump_name(call_control_id: &str) -> String {
    let safe = call_control_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{safe}.wav")
}

async fn persist_matrix_live_call_debug_artifact(
    dir: Option<&Path>,
    call_control_id: &str,
    wav: &[u8],
) -> anyhow::Result<Option<PathBuf>> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    tokio::fs::create_dir_all(dir).await?;
    prune_old_matrix_live_call_debug_artifacts(dir, MATRIX_CALL_DEBUG_CAPTURE_KEEP_FILES).await?;
    let path = dir.join(matrix_debug_call_dump_name(call_control_id));
    tokio::fs::write(&path, wav)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(Some(path))
}

async fn prune_old_matrix_live_call_debug_artifacts(
    dir: &Path,
    keep: usize,
) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("wav") {
            continue;
        }
        let metadata = entry.metadata().await?;
        let modified = metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        files.push((modified, path));
    }
    files.sort_by_key(|(modified, _)| *modified);
    let remove_count = files.len().saturating_sub(keep.saturating_sub(1));
    for (_, path) in files.into_iter().take(remove_count) {
        let _ = tokio::fs::remove_file(&path).await;
    }
    Ok(())
}

async fn transcribe_matrix_live_call_debug_wav(
    wav: &[u8],
    transcription: &synapse_domain::config::schema::TranscriptionConfig,
) -> anyhow::Result<String> {
    let manager = TranscriptionManager::new(transcription)?;
    manager.transcribe(wav, "matrix-live-debug.wav").await
}

fn bounded_live_call_transcript_log(text: &str) -> String {
    const MAX_LOG_CHARS: usize = 200;
    let mut bounded = text.chars().take(MAX_LOG_CHARS).collect::<String>();
    if text.chars().count() > MAX_LOG_CHARS {
        bounded.push_str("...");
    }
    bounded
}

/// Handle an incoming SAS verification request: accept the request, wait for
/// the other side to start SAS, accept it, log emojis, and auto-confirm.
///
/// Flow: receive request → accept → wait for other side to start SAS →
/// accept SAS → keys exchanged (emojis shown) → confirm → done.
async fn handle_verification_request(
    client: MatrixSdkClient,
    sender: OwnedUserId,
    flow_id: String,
) {
    let Some(request) = client
        .encryption()
        .get_verification_request(&sender, &flow_id)
        .await
    else {
        tracing::warn!("Matrix verification request not found for flow {flow_id}");
        return;
    };

    tracing::info!(
        "Matrix verification request received from {}, accepting...",
        sender
    );

    // Force a fresh device key query from the server for the sender.
    // The crypto store may have stale keys from a previous session, causing
    // MAC verification to fail even though emojis match. This ensures the
    // SAS captures the sender's current Ed25519/Curve25519 device keys.
    match client.encryption().request_user_identity(&sender).await {
        Ok(_) => {
            tracing::debug!("Matrix verification: refreshed device keys for {}", sender);
        }
        Err(error) => {
            tracing::warn!(
                "Matrix verification: failed to refresh device keys for {}: {error}",
                sender
            );
        }
    }

    if let Err(error) = request.accept().await {
        tracing::warn!("Matrix verification accept failed: {error}");
        return;
    }

    // Wait for the request to reach Ready, then wait for the other side to
    // start the SAS flow (Transitioned state). The bot is the responder —
    // it should NOT call start_sas(), as the initiating client (Element)
    // will send m.key.verification.start.
    let mut changes = request.changes();
    let sas = loop {
        match request.state() {
            VerificationRequestState::Transitioned { verification } => {
                if let Some(sas) = verification.sas() {
                    break sas;
                }
                tracing::warn!("Matrix verification transitioned but not to SAS");
                return;
            }
            VerificationRequestState::Done | VerificationRequestState::Cancelled(_) => {
                tracing::warn!("Matrix verification request ended before SAS started");
                return;
            }
            _ => {}
        }
        if changes.next().await.is_none() {
            return;
        }
    };

    // Log the other device's keys for diagnostics — if these don't match
    // what the other side actually has, MAC verification will fail.
    let other_dev = sas.other_device();
    tracing::info!(
        "Matrix SAS verification initiated by {} (device {}), accepting SAS...",
        sender,
        other_dev.device_id()
    );
    tracing::debug!(
        "Matrix SAS: other device ed25519={:?} curve25519={:?}",
        other_dev.ed25519_key().map(|k| k.to_base64()),
        other_dev.curve25519_key().map(|k| k.to_base64()),
    );

    // Subscribe to SAS state changes BEFORE calling accept, so we don't
    // miss any state transitions that fire immediately after accept.
    let mut sas_changes = sas.changes();

    // Accept the SAS — sends m.key.verification.accept back to the initiator.
    if let Err(error) = sas.accept().await {
        tracing::warn!("Matrix SAS accept failed: {error}");
        return;
    }

    // Listen for SAS state changes: KeysExchanged → confirm → Done.
    while let Some(state) = sas_changes.next().await {
        tracing::debug!("Matrix SAS state change: {:?}", state);
        match state {
            SasState::KeysExchanged { emojis, decimals } => {
                if let Some(emojis) = emojis {
                    let emoji_display: Vec<String> = emojis
                        .emojis
                        .iter()
                        .map(|e| format!("{} ({})", e.symbol, e.description))
                        .collect();
                    tracing::info!(
                        "Matrix SAS verification emojis with {} — confirm these match in your client:\n  {}",
                        sender,
                        emoji_display.join("  ")
                    );
                } else {
                    let (d1, d2, d3) = decimals;
                    tracing::info!(
                        "Matrix SAS verification decimals with {}: {} {} {}",
                        sender,
                        d1,
                        d2,
                        d3
                    );
                }

                // Design decision: auto-confirm SAS for bot accounts.
                // Bots cannot perform interactive emoji comparison with a
                // human operator. Since the verification request is only
                // processed for allowlisted senders (checked earlier in the
                // listen() handler), auto-confirming is the intended behavior.
                // The emojis are logged above so an operator can audit them
                // after the fact if needed.

                // Brief delay before auto-confirming: let the sync loop
                // fully process the key exchange on both sides before
                // sending our MAC. Without this, Element may receive the
                // MAC before it has finished processing the key material.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                tracing::info!("Matrix SAS: auto-confirming emojis on bot side (bot accounts cannot do interactive verification)...");
                if let Err(error) = sas.confirm().await {
                    tracing::warn!("Matrix SAS verification confirm failed: {error}");
                    return;
                }
            }
            SasState::Done { .. } => {
                tracing::info!(
                    "Matrix SAS verification with {} completed successfully. Device {} is now verified.",
                    sender,
                    sas.other_device().device_id()
                );
                return;
            }
            SasState::Cancelled(info) => {
                tracing::warn!(
                    "Matrix SAS verification with {} cancelled: {}",
                    sender,
                    info.reason()
                );
                return;
            }
            _ => {}
        }
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name(&self) -> &str {
        "matrix"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let content = message.content.as_str();
        let (after_reactions, reaction_markers) = parse_matrix_reaction_markers(content);
        let (after_locations, location_markers) = parse_matrix_location_markers(&after_reactions);
        let (cleaned_content, mut parsed_attachments) =
            parse_matrix_attachment_markers(&after_locations);
        parsed_attachments.extend(matrix_media_artifact_attachments(&message.media_artifacts)?);

        let client = self.matrix_client().await?;
        let target_room_id = if let Some((_, room)) = message.recipient.split_once("||") {
            room.to_string()
        } else {
            self.target_room_id().await?
        };
        let target_room: OwnedRoomId = target_room_id.parse()?;

        let mut room = client.get_room(&target_room);
        if room.is_none() {
            let _ = client.sync_once(SyncSettings::new()).await;
            room = client.get_room(&target_room);
        }

        let Some(room) = room else {
            anyhow::bail!("Matrix room '{}' not found in joined rooms", target_room_id);
        };

        if room.state() != RoomState::Joined {
            anyhow::bail!("Matrix room '{}' is not in joined state", target_room_id);
        }

        // Stop typing notification before sending the response
        if let Err(error) = room.typing_notice(false).await {
            tracing::warn!("Matrix failed to stop typing notification: {error}");
        }

        // Send reaction markers — the LLM decided to react to a message.
        for reaction in &reaction_markers {
            if let Ok(eid) = reaction.event_id.parse::<OwnedEventId>() {
                let content =
                    ReactionEventContent::new(Annotation::new(eid, reaction.emoji.clone()));
                if let Err(error) = room.send(content).await {
                    tracing::warn!("Matrix reaction send failed: {error}");
                }
            }
        }

        // Send location markers.
        for location in &location_markers {
            let content = RoomMessageEventContent::new(MessageType::Location(
                LocationMessageEventContent::new(
                    location.description.clone(),
                    location.geo_uri.clone(),
                ),
            ));
            if let Err(error) = room.send(content).await {
                tracing::warn!("Matrix location send failed: {error}");
            }
        }

        // Send each attachment via room.send_attachment() which auto-encrypts
        // media for E2EE rooms (unlike client.media().upload() which uploads plain).
        for attachment in &parsed_attachments {
            let target = attachment.target.trim();
            if let Some(path) = local_media_path(target) {
                if !path.exists() || !path.is_file() {
                    anyhow::bail!("Matrix outgoing attachment not found or not a file: {target}");
                }

                // Security: restrict uploads to the workspace/media directory to
                // prevent [IMAGE:path] markers in bot responses from exfiltrating
                // arbitrary host files via Matrix media uploads.
                if let Some(ref zdir) = self.synapseclaw_dir {
                    let allowed_dir = zdir.join("workspace");
                    match path.canonicalize() {
                        Ok(canonical) => {
                            if !canonical.starts_with(&allowed_dir) {
                                anyhow::bail!(
                                    "Matrix outgoing attachment path '{}' is outside workspace directory '{}' — refusing upload",
                                    canonical.display(),
                                    allowed_dir.display()
                                );
                            }
                        }
                        Err(err) => {
                            anyhow::bail!(
                                "Matrix outgoing attachment path '{}' could not be canonicalized: {err}",
                                target
                            );
                        }
                    }
                } else {
                    anyhow::bail!(
                        "Matrix cannot deliver outgoing attachment without synapseclaw_dir path validation: {target}"
                    );
                }
            }

            let upload = resolve_outbound_media_uri(
                &self.http_client,
                target,
                attachment.label.as_deref(),
                attachment.mime_type.as_deref(),
                matrix_attachment_fallback_file_name(attachment.kind),
            )
            .await?;

            let mime = upload
                .mime_type
                .parse()
                .unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM);
            let filename = upload.file_name;
            let bytes = upload.bytes;

            let config = match attachment.kind {
                MatrixOutgoingAttachmentKind::Voice => AttachmentConfig::new().info(
                    AttachmentInfo::Voice(matrix_audio_info(attachment.kind, &bytes)),
                ),
                MatrixOutgoingAttachmentKind::Audio => AttachmentConfig::new().info(
                    AttachmentInfo::Audio(matrix_audio_info(attachment.kind, &bytes)),
                ),
                _ => AttachmentConfig::new(),
            };

            room.send_attachment(&filename, &mime, bytes, config)
                .await
                .map_err(|error| {
                    anyhow::anyhow!("Matrix media send failed for '{}': {error}", filename)
                })?;
        }

        // Send remaining text (if any) after attachments/reactions, with threading support.
        let send_text = |text: &str| {
            let mut content = RoomMessageEventContent::text_markdown(text);
            if let Some(ref thread_ts) = message.thread_ts {
                if let Ok(thread_root) = thread_ts.parse::<OwnedEventId>() {
                    content.relates_to = Some(Relation::Thread(Thread::plain(
                        thread_root.clone(),
                        thread_root,
                    )));
                }
            }
            content
        };

        if !cleaned_content.is_empty() {
            room.send(send_text(&cleaned_content)).await?;
        } else if parsed_attachments.is_empty()
            && reaction_markers.is_empty()
            && location_markers.is_empty()
        {
            // No markers were found — send original content as text.
            room.send(send_text(content)).await?;
        }

        Ok(())
    }

    async fn listen(&self, tx: mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        // Initialize SDK client first — this may login with password and
        // persist session.json, which is needed for subsequent HTTP calls.
        let client = self.matrix_client().await.map_err(|error| {
            record_matrix_call_control_error(&error.to_string());
            error
        })?;

        let target_room_id = self.target_room_id().await.map_err(|error| {
            record_matrix_call_control_error(&error.to_string());
            error
        })?;
        self.ensure_room_supported(&target_room_id)
            .await
            .map_err(|error| {
                record_matrix_call_control_error(&error.to_string());
                error
            })?;

        let target_room: OwnedRoomId = target_room_id.parse()?;
        let my_user_id: OwnedUserId = match self.get_my_user_id().await {
            Ok(user_id) => user_id.parse()?,
            Err(error) => {
                if let Some(hinted) = self.session_owner_hint.as_ref() {
                    tracing::warn!(
                        "Matrix whoami failed while resolving listener user_id; using configured user_id hint: {error}"
                    );
                    hinted.parse()?
                } else {
                    return Err(error);
                }
            }
        };

        self.log_e2ee_diagnostics(&client).await;

        // Force a fresh device key query for allowed users BEFORE the initial
        // sync. The SDK captures device data from the local store when it first
        // processes a verification request event. If the store has stale keys
        // (e.g. from a previous session), SAS MAC verification will fail even
        // though emojis match. Querying keys now ensures the store is up-to-date
        // before any verification events are processed during sync.
        for user_str in &self.allowed_users {
            if let Ok(user_id) = <&matrix_sdk::ruma::UserId>::try_from(user_str.as_str()) {
                match client.encryption().request_user_identity(user_id).await {
                    Ok(_) => {
                        tracing::debug!("Matrix: refreshed device keys for {user_str}");
                    }
                    Err(error) => {
                        tracing::debug!(
                            "Matrix: could not refresh device keys for {user_str}: {error}"
                        );
                    }
                }
            }
        }

        // Register the verification handler BEFORE the initial sync so that
        // verification requests arriving during the first sync are handled.
        let verification_client = client.clone();
        let allowed_users_for_verification = self.allowed_users.clone();
        client.add_event_handler(move |event: OriginalSyncRoomMessageEvent, _room: Room| {
            let ver_client = verification_client.clone();
            let allowed_users = allowed_users_for_verification.clone();

            async move {
                if !matches!(&event.content.msgtype, MessageType::VerificationRequest(_)) {
                    return;
                }

                let sender = event.sender.to_string();
                if !MatrixChannel::is_sender_allowed(&allowed_users, &sender) {
                    tracing::warn!(
                        "Matrix verification request from non-allowed user {sender}, ignoring"
                    );
                    return;
                }

                let flow_id = event.event_id.to_string();
                let sender_id = event.sender.clone();

                tokio::spawn(async move {
                    handle_verification_request(ver_client, sender_id, flow_id).await;
                });
            }
        });

        let _ = client.sync_once(SyncSettings::new()).await;

        tracing::info!(
            "Matrix channel listening on room {} (configured as {})...",
            target_room_id,
            self.room_id
        );

        let recent_event_cache = Arc::new(Mutex::new((
            std::collections::VecDeque::new(),
            std::collections::HashSet::new(),
        )));

        let tx_handler = tx.clone();
        let my_user_id_for_handler = my_user_id.clone();
        let allowed_users_for_handler = self.allowed_users.clone();
        let dedupe_for_handler = Arc::clone(&recent_event_cache);
        let voice_mode_for_handler = Arc::clone(&self.voice_mode);
        let media_save_dir_for_handler = self.media_save_dir();
        let transcription_for_handler = self.transcription.clone();
        let voice_cache_for_handler = Arc::clone(&self.voice_transcriptions);
        let max_media_bytes_for_handler = self.max_media_bytes;

        register_realtime_audio_ingress("matrix", tx.clone(), self.transcription.clone());

        let matrix_for_to_device = self.clone();
        client.add_event_handler(
            move |event: matrix_sdk::ruma::serde::Raw<AnyToDeviceEvent>| {
                let matrix = matrix_for_to_device.clone();
                async move {
                    let Ok(raw_value) = event.deserialize_as::<serde_json::Value>() else {
                        return;
                    };
                    matrix
                        .handle_matrix_call_encryption_to_device_event(raw_value)
                        .await;
                }
            },
        );

        client.add_event_handler(move |event: OriginalSyncRoomMessageEvent, room: Room| {
            let tx = tx_handler.clone();
            let my_user_id = my_user_id_for_handler.clone();
            let allowed_users = allowed_users_for_handler.clone();
            let dedupe = Arc::clone(&dedupe_for_handler);
            let voice_mode = Arc::clone(&voice_mode_for_handler);
            let media_save_dir = media_save_dir_for_handler.clone();
            let transcription_config = transcription_for_handler.clone();
            let voice_cache = Arc::clone(&voice_cache_for_handler);
            let max_media_bytes = max_media_bytes_for_handler;

            async move {
                if false
                /* multi-room: room_id filter disabled */
                {
                    return;
                }

                if event.sender == my_user_id {
                    return;
                }

                let sender = event.sender.to_string();
                if !MatrixChannel::is_sender_allowed(&allowed_users, &sender) {
                    return;
                }

                // Deduplicate early — before downloading media or transcribing
                // to avoid repeated I/O and billable external calls on redelivery.
                //
                // Design tradeoff: caching the event_id before processing means
                // that if media download or transcription fails, the event won't
                // be retried. This is acceptable because Matrix sync does not
                // redeliver events on handler failure — only on sync gaps (where
                // the SDK replays from the sync token). Early dedupe prevents
                // duplicate media downloads and duplicate billable transcription
                // calls, which outweighs the theoretical loss of retry capability.
                let event_id = event.event_id.to_string();
                {
                    let mut guard = dedupe.lock().await;
                    let (recent_order, recent_lookup) = &mut *guard;
                    if MatrixChannel::cache_event_id(&event_id, recent_order, recent_lookup) {
                        return;
                    }
                }

                let mut media_attachments = Vec::new();
                let body = match &event.content.msgtype {
                    MessageType::Text(content) => content.body.clone(),
                    MessageType::Notice(content) => content.body.clone(),
                    MessageType::Image(content) => {
                        let Some(ref save_dir) = media_save_dir else {
                            tracing::warn!("Matrix image received but no synapseclaw_dir configured for media storage");
                            return;
                        };
                        let filename = content.filename().to_string();
                        let source = content.source.clone();
                        let size_hint = content.info.as_ref().and_then(|i| i.size.map(u64::from));
                        let sdk_client = room.client();
                        match download_and_save_matrix_media(&sdk_client, &source, &filename, save_dir, size_hint, max_media_bytes).await {
                            Ok(local_path) => {
                                if is_image_extension(&local_path) {
                                    media_attachments.push(matrix_inbound_media_attachment(
                                        InboundMediaKind::Image,
                                        &local_path,
                                        filename.clone(),
                                    ));
                                    format!("[IMAGE:{}]", local_path.display())
                                } else {
                                    media_attachments.push(matrix_inbound_media_attachment(
                                        InboundMediaKind::File,
                                        &local_path,
                                        filename.clone(),
                                    ));
                                    format!("[Document: {}] {}", filename, local_path.display())
                                }
                            }
                            Err(error) => {
                                tracing::warn!("Matrix image download failed: {error}");
                                return;
                            }
                        }
                    }
                    MessageType::File(content) => {
                        let Some(ref save_dir) = media_save_dir else {
                            tracing::warn!("Matrix file received but no synapseclaw_dir configured for media storage");
                            return;
                        };
                        let filename = content.filename().to_string();
                        let source = content.source.clone();
                        let size_hint = content.info.as_ref().and_then(|i| i.size.map(u64::from));
                        let sdk_client = room.client();
                        match download_and_save_matrix_media(&sdk_client, &source, &filename, save_dir, size_hint, max_media_bytes).await {
                            Ok(local_path) => {
                                media_attachments.push(matrix_inbound_media_attachment(
                                    InboundMediaKind::File,
                                    &local_path,
                                    filename.clone(),
                                ));
                                format!("[Document: {}] {}", filename, local_path.display())
                            }
                            Err(error) => {
                                tracing::warn!("Matrix file download failed: {error}");
                                return;
                            }
                        }
                    }
                    MessageType::Audio(content) => {
                        let filename = content.filename().to_string();
                        let source = content.source.clone();
                        let is_voice_message = matrix_audio_message_is_voice(content);
                        let size_hint = content.info.as_ref().and_then(|i| i.size.map(u64::from));
                        let sdk_client = room.client();

                        // Pre-download size check for audio.
                        if let Some(size) = size_hint {
                            if usize::try_from(size).unwrap_or(usize::MAX) > max_media_bytes {
                                tracing::warn!("Matrix audio exceeds size limit ({size} bytes); skipping");
                                return;
                            }
                        }

                        // Try transcription first if enabled.
                        if let Some(ref config) = transcription_config {
                            let request = MediaRequestParameters {
                                source: source.clone(),
                                format: MediaFormat::File,
                            };
                            match sdk_client.media().get_media_content(&request, false).await {
                                Ok(audio_data) => {
                                    match super::transcription::transcribe_audio(audio_data, &filename, config).await {
                                        Ok(text) => {
                                            let event_id = event.event_id.to_string();
                                            let mut cache = voice_cache.lock().await;
                                            if cache.len() >= 100 {
                                                cache.clear();
                                            }
                                            cache.insert(event_id, text.clone());
                                            if is_voice_message {
                                                voice_mode.store(true, Ordering::Relaxed);
                                            }
                                            matrix_audio_transcription_text(is_voice_message, &text)
                                        }
                                        Err(error) => {
                                            tracing::debug!("Matrix audio transcription failed, falling back to file save: {error}");
                                            let Some(ref save_dir) = media_save_dir else {
                                                tracing::warn!("Matrix audio received but no synapseclaw_dir configured for media storage");
                                                return;
                                            };
                                            match download_and_save_matrix_media(&sdk_client, &source, &filename, save_dir, size_hint, max_media_bytes).await {
                                                Ok(local_path) => {
                                                    media_attachments.push(matrix_inbound_media_attachment(
                                                        InboundMediaKind::Audio,
                                                        &local_path,
                                                        filename.clone(),
                                                    ));
                                                    format!("[Document: {}] {}", filename, local_path.display())
                                                }
                                                Err(dl_error) => {
                                                    tracing::warn!("Matrix audio download failed: {dl_error}");
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!("Matrix audio media fetch failed: {error}");
                                    return;
                                }
                            }
                        } else {
                            // No transcription — save as document.
                            let Some(ref save_dir) = media_save_dir else {
                                tracing::warn!("Matrix audio received but no synapseclaw_dir configured for media storage");
                                return;
                            };
                            match download_and_save_matrix_media(&sdk_client, &source, &filename, save_dir, size_hint, max_media_bytes).await {
                                Ok(local_path) => {
                                    media_attachments.push(matrix_inbound_media_attachment(
                                        InboundMediaKind::Audio,
                                        &local_path,
                                        filename.clone(),
                                    ));
                                    format!("[Document: {}] {}", filename, local_path.display())
                                }
                                Err(error) => {
                                    tracing::warn!("Matrix audio download failed: {error}");
                                    return;
                                }
                            }
                        }
                    }
                    MessageType::Video(content) => {
                        format!("[video: {}]", content.body)
                    }
                    MessageType::Location(content) => {
                        format!("[Location: {}] {}", content.geo_uri, content.body)
                    }
                    _ => return,
                };

                // Prepend reply context if this message is a reply to another.
                // Fetch the original message text so the LLM has context even
                // when the replied-to message is outside the current conversation.
                let body = if let Some(matrix_sdk::ruma::events::room::message::Relation::Reply { in_reply_to }) = &event.content.relates_to {
                    let target_eid = &in_reply_to.event_id;
                    // Check voice transcription cache first.
                    let cached_voice = {
                        let cache = voice_cache.lock().await;
                        cache.get(target_eid.as_str()).cloned()
                    };
                    let original_text = if let Some(transcript) = cached_voice {
                        transcript
                    } else {
                        // Fetch from server.
                        match room.event(target_eid, None).await {
                            Ok(timeline_event) => {
                                serde_json::to_value(timeline_event.raw())
                                    .ok()
                                    .and_then(|v| {
                                        v.get("content")?
                                            .get("body")?
                                            .as_str()
                                            .map(|s| s.to_string())
                                    })
                                    .unwrap_or_default()
                            }
                            Err(_) => String::new(),
                        }
                    };
                    if original_text.is_empty() {
                        format!("[Reply to {}] {}", target_eid, body)
                    } else {
                        let preview = if original_text.chars().count() > 200 {
                            format!("{}…", original_text.chars().take(200).collect::<String>())
                        } else {
                            original_text
                        };
                        format!("[Reply to {}: \"{}\"] {}", target_eid, preview, body)
                    }
                } else {
                    body
                };

                if !MatrixChannel::has_non_empty_body(&body) {
                    return;
                }

                let thread_ts = match &event.content.relates_to {
                    Some(Relation::Thread(thread)) => Some(thread.event_id.to_string()),
                    _ => None,
                };

                // Mark message as read + start typing indicator.
                if let Ok(eid) = event_id.parse::<OwnedEventId>() {
                    let _ = room
                        .send_single_receipt(
                            create_receipt::v3::ReceiptType::Read,
                            ReceiptThread::Unthreaded,
                            eid,
                        )
                        .await;
                }
                let _ = room.typing_notice(true).await;
                let msg = ChannelMessage {
                    id: event_id,
                    sender: sender.clone(),
                    reply_target: format!("{}||{}", sender, room.room_id()),
                    content: body,
                    channel: "matrix".to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    thread_ts,
                    media_attachments,
                };

                let _ = tx.send(msg).await;
            }
        });

        // Reaction event handler — delivers emoji reactions to the agent.
        let tx_reaction = tx.clone();
        let target_room_for_reaction = target_room.clone();
        let my_user_id_for_reaction = my_user_id.clone();
        let allowed_users_for_reaction = self.allowed_users.clone();
        let dedupe_for_reaction = Arc::clone(&recent_event_cache);

        client.add_event_handler(move |event: OriginalSyncReactionEvent, room: Room| {
            let tx = tx_reaction.clone();
            let target_room = target_room_for_reaction.clone();
            let my_user_id = my_user_id_for_reaction.clone();
            let allowed_users = allowed_users_for_reaction.clone();
            let dedupe = Arc::clone(&dedupe_for_reaction);

            async move {
                if room.room_id().as_str() != target_room.as_str() {
                    return;
                }
                if event.sender == my_user_id {
                    return;
                }
                let sender = event.sender.to_string();
                if !MatrixChannel::is_sender_allowed(&allowed_users, &sender) {
                    return;
                }

                let event_id = event.event_id.to_string();
                {
                    let mut guard = dedupe.lock().await;
                    let (recent_order, recent_lookup) = &mut *guard;
                    if MatrixChannel::cache_event_id(&event_id, recent_order, recent_lookup) {
                        return;
                    }
                }

                let emoji = &event.content.relates_to.key;
                let target_event_id = &event.content.relates_to.event_id;

                // Fetch the original message to provide context.
                let (original_author, original_text, thread_ts) =
                    match room.event(target_event_id, None).await {
                        Ok(timeline_event) => {
                            let raw = timeline_event.raw();
                            match raw.deserialize() {
                                Ok(any_event) => {
                                    let author = any_event.sender().to_string();
                                    let json_value = serde_json::to_value(raw).ok();
                                    // Extract body from the raw JSON content.
                                    let text = json_value
                                        .as_ref()
                                        .and_then(|v| {
                                            v.get("content")?
                                                .get("body")?
                                                .as_str()
                                                .map(|s| s.to_string())
                                        })
                                        .unwrap_or_default();
                                    // Extract thread root from m.relates_to so the
                                    // reaction response lands in the same thread.
                                    let thread = json_value.as_ref().and_then(|v| {
                                        let rel = v.get("content")?.get("m.relates_to")?;
                                        if rel.get("rel_type")?.as_str()? == "m.thread" {
                                            rel.get("event_id")?.as_str().map(|s| s.to_string())
                                        } else {
                                            None
                                        }
                                    });
                                    (author, text, thread)
                                }
                                Err(_) => (String::new(), String::new(), None),
                            }
                        }
                        Err(_) => (String::new(), String::new(), None),
                    };

                let is_own_message = original_author == my_user_id.as_str();
                let author_label = if is_own_message {
                    "your message"
                } else {
                    "their message"
                };
                let preview = if original_text.chars().count() > 100 {
                    format!("{}…", original_text.chars().take(100).collect::<String>())
                } else {
                    original_text.clone()
                };
                let body = format!("[Reaction: {emoji} on {author_label}: \"{preview}\"]");

                let msg = ChannelMessage {
                    id: event_id,
                    sender: sender.clone(),
                    reply_target: sender,
                    content: body,
                    channel: "matrix".to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    thread_ts,
                    media_attachments: Vec::new(),
                };

                let _ = tx.send(msg).await;
            }
        });

        let my_user_id_for_rtc = my_user_id.clone();
        let allowed_users_for_rtc = self.allowed_users.clone();
        let matrix_for_rtc = self.clone();
        let dedupe_for_rtc = Arc::clone(&recent_event_cache);
        client.add_event_handler(move |event: SyncRtcNotificationEvent, room: Room| {
            let my_user_id = my_user_id_for_rtc.clone();
            let allowed_users = allowed_users_for_rtc.clone();
            let matrix = matrix_for_rtc.clone();
            let dedupe = Arc::clone(&dedupe_for_rtc);

            async move {
                if event.sender().as_str() == my_user_id.as_str() {
                    return;
                }
                let sender = event.sender().to_string();
                if !MatrixChannel::is_sender_allowed(&allowed_users, &sender) {
                    return;
                }
                let Ok(sender_user_id) = sender.parse::<OwnedUserId>() else {
                    return;
                };
                if !matrix
                    .room_is_allowed_call_target(&room, &sender_user_id)
                    .await
                {
                    return;
                }
                let Some(original) = event.as_original() else {
                    return;
                };
                if !matches!(original.content.notification_type, NotificationType::Ring) {
                    return;
                }
                let event_id = original.event_id.to_string();
                {
                    let mut guard = dedupe.lock().await;
                    let (recent_order, recent_lookup) = &mut *guard;
                    if MatrixChannel::cache_event_id(&event_id, recent_order, recent_lookup) {
                        return;
                    }
                }
                let room_id = room.room_id().to_string();
                record_matrix_incoming_ring_context(&event_id, &room_id, &sender);
                tracing::info!(
                    sender = %sender,
                    room_id = %room_id,
                    event_id = %event_id,
                    "recorded inbound Matrix ring without enqueuing a synthetic chat turn"
                );
            }
        });

        let my_user_id_for_rtc_decline = my_user_id.clone();
        let allowed_users_for_rtc_decline = self.allowed_users.clone();
        let matrix_for_rtc_decline = self.clone();
        let dedupe_for_rtc_decline = Arc::clone(&recent_event_cache);
        client.add_event_handler(move |event: SyncRtcDeclineEvent, room: Room| {
            let my_user_id = my_user_id_for_rtc_decline.clone();
            let allowed_users = allowed_users_for_rtc_decline.clone();
            let matrix = matrix_for_rtc_decline.clone();
            let dedupe = Arc::clone(&dedupe_for_rtc_decline);

            async move {
                if event.sender().as_str() == my_user_id.as_str() {
                    return;
                }
                let sender = event.sender().to_string();
                if !MatrixChannel::is_sender_allowed(&allowed_users, &sender) {
                    return;
                }
                let Ok(sender_user_id) = sender.parse::<OwnedUserId>() else {
                    return;
                };
                if !matrix
                    .room_is_allowed_call_target(&room, &sender_user_id)
                    .await
                {
                    return;
                }
                let Some(original) = event.as_original() else {
                    return;
                };
                let event_id = original.event_id.to_string();
                {
                    let mut guard = dedupe.lock().await;
                    let (recent_order, recent_lookup) = &mut *guard;
                    if MatrixChannel::cache_event_id(&event_id, recent_order, recent_lookup) {
                        return;
                    }
                }
                record_matrix_call_declined(&original.content.relates_to.event_id.to_string());
            }
        });

        let my_user_id_for_call_member = my_user_id.clone();
        let allowed_users_for_call_member = self.allowed_users.clone();
        let matrix_for_call_member = self.clone();
        let dedupe_for_call_member = Arc::clone(&recent_event_cache);
        client.add_event_handler(move |event: AnySyncStateEvent, room: Room| {
            let my_user_id = my_user_id_for_call_member.clone();
            let allowed_users = allowed_users_for_call_member.clone();
            let matrix = matrix_for_call_member.clone();
            let dedupe = Arc::clone(&dedupe_for_call_member);

            async move {
                let AnySyncStateEvent::CallMember(member_event) = event else {
                    return;
                };
                let Some(original) = member_event.as_original() else {
                    return;
                };
                if original.sender.as_str() == my_user_id.as_str() {
                    return;
                }

                let sender = original.sender.to_string();
                if !MatrixChannel::is_sender_allowed(&allowed_users, &sender) {
                    return;
                }
                let Ok(sender_user_id) = sender.parse::<OwnedUserId>() else {
                    return;
                };
                if !matrix
                    .room_is_allowed_call_target(&room, &sender_user_id)
                    .await
                {
                    return;
                }

                let event_id = original.event_id.to_string();
                {
                    let mut guard = dedupe.lock().await;
                    let (recent_order, recent_lookup) = &mut *guard;
                    if MatrixChannel::cache_event_id(&event_id, recent_order, recent_lookup) {
                        return;
                    }
                }

                let room_id = room.room_id().to_string();
                let active_memberships = original
                    .content
                    .active_memberships(Some(original.origin_server_ts));
                let has_call_membership = active_memberships.iter().any(|membership| {
                    membership.is_call() && matches!(membership.application(), Application::Call(_))
                });

                if has_call_membership {
                    let (call_control_id, is_new) =
                        record_matrix_incoming_membership_context(&event_id, &room_id, &sender);
                    if !is_new {
                        return;
                    }
                    tracing::info!(
                        sender = %sender,
                        room_id = %room_id,
                        call_control_id = %call_control_id,
                        "recorded inbound Matrix call membership without enqueuing a synthetic chat turn"
                    );
                    let matrix = matrix.clone();
                    let call_control_id_for_answer = call_control_id.clone();
                    tokio::spawn(async move {
                        if let Err(error) = matrix
                            .answer(RealtimeCallAnswerRequest {
                                call_control_id: call_control_id_for_answer.clone(),
                            })
                            .await
                        {
                            tracing::warn!(
                                call_control_id = %call_control_id_for_answer,
                                error = %error,
                                "failed to auto-answer inbound Matrix call"
                            );
                        } else {
                            tracing::info!(
                                call_control_id = %call_control_id_for_answer,
                                "auto-answered inbound Matrix call"
                            );
                        }
                    });
                    return;
                }

                if let Some(end_reason) = matrix_call_member_end_reason(&original.content) {
                    if let Some(call_control_id) =
                        record_matrix_call_ended_for_sender_room(&room_id, &sender, end_reason)
                    {
                        let matrix = matrix.clone();
                        tokio::spawn(async move {
                            matrix.close_media_session(&call_control_id).await;
                        });
                    }
                }
            }
        });

        let sync_settings = SyncSettings::new().timeout(std::time::Duration::from_secs(30));
        client
            .sync_with_result_callback(sync_settings, |sync_result| {
                let tx = tx.clone();
                async move {
                    if tx.is_closed() {
                        return Ok::<LoopCtrl, matrix_sdk::Error>(LoopCtrl::Break);
                    }

                    if let Err(error) = sync_result {
                        tracing::warn!("Matrix sync error: {error}, retrying...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }

                    Ok::<LoopCtrl, matrix_sdk::Error>(LoopCtrl::Continue)
                }
            })
            .await?;

        Ok(())
    }

    async fn start_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        let client = self.matrix_client().await?;
        let target_room_id = self.target_room_id().await?;
        let target_room: OwnedRoomId = target_room_id.parse()?;
        if let Some(room) = client.get_room(&target_room) {
            room.typing_notice(true).await?;
        }
        Ok(())
    }

    async fn stop_typing(&self, _recipient: &str) -> anyhow::Result<()> {
        let client = self.matrix_client().await?;
        let target_room_id = self.target_room_id().await?;
        let target_room: OwnedRoomId = target_room_id.parse()?;
        if let Some(room) = client.get_room(&target_room) {
            room.typing_notice(false).await?;
        }
        Ok(())
    }

    async fn health_check(&self) -> bool {
        if let Err(error) = self.matrix_client().await {
            record_matrix_call_control_error(&error.to_string());
            return false;
        }

        let room_id = match self.target_room_id().await {
            Ok(room_id) => room_id,
            Err(error) => {
                record_matrix_call_control_error(&error.to_string());
                return false;
            }
        };

        if let Err(error) = self.ensure_room_supported(&room_id).await {
            record_matrix_call_control_error(&error.to_string());
            return false;
        }

        let bootstrap = self.probe_matrix_rtc_bootstrap(&room_id).await;
        record_matrix_rtc_bootstrap_status(bootstrap);

        match self.matrix_client().await {
            Ok(_) => true,
            Err(error) => {
                record_matrix_call_control_error(&error.to_string());
                false
            }
        }
    }

    async fn add_reaction(
        &self,
        _channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        let client = self.matrix_client().await?;
        let target_room_id = self.target_room_id().await?;
        let target_room: OwnedRoomId = target_room_id.parse()?;

        let room = client
            .get_room(&target_room)
            .ok_or_else(|| anyhow::anyhow!("Matrix room not found for reaction"))?;

        let event_id: OwnedEventId = message_id
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid event ID for reaction: {}", message_id))?;

        let reaction = ReactionEventContent::new(Annotation::new(event_id, emoji.to_string()));
        let response = room.send(reaction).await?;

        let key = format!("{}:{}", message_id, emoji);
        self.reaction_events
            .write()
            .await
            .insert(key, response.event_id.to_string());

        Ok(())
    }

    async fn remove_reaction(
        &self,
        _channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> anyhow::Result<()> {
        let key = format!("{}:{}", message_id, emoji);
        let reaction_event_id = self.reaction_events.write().await.remove(&key);

        if let Some(reaction_event_id) = reaction_event_id {
            let client = self.matrix_client().await?;
            let target_room_id = self.target_room_id().await?;
            let target_room: OwnedRoomId = target_room_id.parse()?;

            let room = client
                .get_room(&target_room)
                .ok_or_else(|| anyhow::anyhow!("Matrix room not found for reaction removal"))?;

            let event_id: OwnedEventId = reaction_event_id
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid reaction event ID: {}", reaction_event_id))?;

            room.redact(&event_id, None, None).await?;
        }

        Ok(())
    }

    async fn pin_message(&self, _channel_id: &str, message_id: &str) -> anyhow::Result<()> {
        let room_id = self.target_room_id().await?;
        let encoded_room = Self::encode_path_segment(&room_id);

        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.pinned_events",
            self.homeserver, encoded_room
        );
        let resp = self
            .http_client
            .get(&url)
            .header("Authorization", self.auth_header_value().await?)
            .send()
            .await?;

        let mut pinned: Vec<String> = if resp.status().is_success() {
            let body: serde_json::Value = resp.json().await?;
            body.get("pinned")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let msg_id = message_id.to_string();
        if pinned.contains(&msg_id) {
            return Ok(());
        }
        pinned.push(msg_id);

        let put_url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.pinned_events",
            self.homeserver, encoded_room
        );
        let body = serde_json::json!({ "pinned": pinned });
        let resp = self
            .http_client
            .put(&put_url)
            .header("Authorization", self.auth_header_value().await?)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Matrix pin_message failed: {err}");
        }

        Ok(())
    }

    async fn unpin_message(&self, _channel_id: &str, message_id: &str) -> anyhow::Result<()> {
        let room_id = self.target_room_id().await?;
        let encoded_room = Self::encode_path_segment(&room_id);

        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.pinned_events",
            self.homeserver, encoded_room
        );
        let resp = self
            .http_client
            .get(&url)
            .header("Authorization", self.auth_header_value().await?)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(());
        }

        let body: serde_json::Value = resp.json().await?;
        let mut pinned: Vec<String> = body
            .get("pinned")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let msg_id = message_id.to_string();
        let original_len = pinned.len();
        pinned.retain(|id| id != &msg_id);

        if pinned.len() == original_len {
            return Ok(());
        }

        let put_url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/m.room.pinned_events",
            self.homeserver, encoded_room
        );
        let body = serde_json::json!({ "pinned": pinned });
        let resp = self
            .http_client
            .put(&put_url)
            .header("Authorization", self.auth_header_value().await?)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await.unwrap_or_default();
            anyhow::bail!("Matrix unpin_message failed: {err}");
        }

        Ok(())
    }

    async fn fetch_message(&self, message_id: &str) -> anyhow::Result<Option<String>> {
        let client = self.matrix_client().await?;
        let target_room_id = self.target_room_id().await?;
        let target_room: OwnedRoomId = target_room_id.parse()?;
        let room = client
            .get_room(&target_room)
            .ok_or_else(|| anyhow::anyhow!("Matrix room not found"))?;
        let event_id: OwnedEventId = message_id
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid event ID: {message_id}"))?;

        match room.event(&event_id, None).await {
            Ok(timeline_event) => {
                let raw = timeline_event.raw();
                let text = serde_json::to_value(raw)
                    .ok()
                    .and_then(|v| {
                        v.get("content")?
                            .get("body")?
                            .as_str()
                            .map(|s| s.to_string())
                    })
                    .filter(|s| !s.is_empty());
                Ok(text)
            }
            Err(_) => Ok(None),
        }
    }
}

#[async_trait]
impl RealtimeCallRuntimePort for MatrixChannel {
    fn channel_name(&self) -> &'static str {
        "matrix"
    }

    fn supports_call_kind(&self, kind: RealtimeCallKind) -> bool {
        matches!(kind, RealtimeCallKind::Audio)
    }

    async fn start_audio_call(
        &self,
        request: RealtimeCallStartRequest,
    ) -> anyhow::Result<RealtimeCallStartResult> {
        self.media_tts_config()?;
        self.live_turn_engine_config()?;
        let (room, target_room_id, mentions) =
            self.call_target_room_and_mentions(&request.to).await?;
        let bootstrap = self.probe_matrix_rtc_bootstrap(&target_room_id).await;
        record_matrix_rtc_bootstrap_status(bootstrap.clone());
        if !bootstrap.media_bootstrap_ready {
            anyhow::bail!(
                "MatrixRTC bootstrap is not ready for room `{target_room_id}`{}",
                bootstrap
                    .last_probe_error
                    .as_deref()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            );
        }
        let focus_url = bootstrap
            .focus_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("MatrixRTC bootstrap returned no focus URL"))?;
        let membership_event_id = self
            .announce_call_membership(&room, &target_room_id, focus_url, "outgoing_ring")
            .await?;
        let mut content = RtcNotificationEventContent::new(
            MilliSecondsSinceUnixEpoch::now(),
            Duration::from_secs(30),
            NotificationType::Ring,
        );
        content.mentions = Some(mentions);
        content.relates_to = Some(Reference::new(membership_event_id.clone()));

        let send_response = match room.send(content).await {
            Ok(response) => response,
            Err(error) => {
                let _ = self
                    .clear_call_membership_for_room(&room, "outgoing_ring_send_failed")
                    .await;
                return Err(error.into());
            }
        };
        let call_control_id = send_response.event_id.to_string();
        let objective = resolve_realtime_call_objective(&request);
        let _prompt = resolve_realtime_call_prompt(&request);

        record_matrix_outgoing_ring(
            &call_control_id,
            Some(membership_event_id.as_str()),
            &target_room_id,
            request.origin.clone(),
            objective.clone(),
        );

        Ok(RealtimeCallStartResult {
            channel: "matrix".into(),
            call_control_id,
            call_leg_id: membership_event_id.to_string(),
            call_session_id: target_room_id,
            state: RealtimeCallState::Ringing,
            origin: request.origin,
            objective,
        })
    }

    async fn speak(
        &self,
        request: RealtimeCallSpeakRequest,
    ) -> anyhow::Result<RealtimeCallActionResult> {
        let session = self.ensure_media_session(&request.call_control_id).await?;
        self.speak_into_media_session(&session, &request.call_control_id, &request.text)
            .await?;
        Ok(RealtimeCallActionResult {
            channel: "matrix".into(),
            call_control_id: request.call_control_id,
            status: "spoken".into(),
            state: RealtimeCallState::Listening,
        })
    }

    async fn answer(
        &self,
        request: RealtimeCallAnswerRequest,
    ) -> anyhow::Result<RealtimeCallActionResult> {
        let session = self.ensure_media_session(&request.call_control_id).await?;
        self.wait_for_media_ingress_ready(
            &session,
            &request.call_control_id,
            Duration::from_secs(3),
        )
        .await;
        if let Err(error) = self
            .speak_into_media_session(
                &session,
                &request.call_control_id,
                default_realtime_call_answer_greeting(),
            )
            .await
        {
            tracing::warn!(
                call_control_id = %request.call_control_id,
                error = %error,
                "failed to deliver initial Matrix call answer greeting"
            );
        } else {
            tracing::info!(
                call_control_id = %request.call_control_id,
                "delivered initial Matrix call answer greeting"
            );
        }
        Ok(RealtimeCallActionResult {
            channel: "matrix".into(),
            call_control_id: request.call_control_id,
            status: "answered".into(),
            state: RealtimeCallState::Listening,
        })
    }

    async fn hangup(
        &self,
        request: RealtimeCallHangupRequest,
    ) -> anyhow::Result<RealtimeCallActionResult> {
        self.close_media_session(&request.call_control_id).await;
        let room = self
            .room_for_call_control_id(&request.call_control_id)
            .await?;
        let event_id: OwnedEventId = request
            .call_control_id
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid Matrix call_control_id event id"))?;
        room.send(RtcDeclineEventContent::new(event_id)).await?;
        record_matrix_call_ended_with_reason(&request.call_control_id, "operator_hangup");

        Ok(RealtimeCallActionResult {
            channel: "matrix".into(),
            call_control_id: request.call_control_id,
            status: "declined".into(),
            state: RealtimeCallState::Ended,
        })
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<RealtimeCallSessionSnapshot>> {
        Ok(matrix_recent_call_sessions())
    }

    async fn get_session(
        &self,
        call_control_id: &str,
    ) -> anyhow::Result<Option<RealtimeCallSessionSnapshot>> {
        Ok(matrix_call_session(call_control_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn matrix_call_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("matrix call test lock poisoned")
    }

    fn reset_matrix_call_status() {
        configure_matrix_call_ledger_dir(None);
        *matrix_call_control_status_slot().write() = MatrixCallControlStatus::default();
        matrix_media_sessions_slot().write().clear();
    }

    fn make_channel() -> MatrixChannel {
        MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("syt_test_token".to_string()),
            "!room:matrix.org".to_string(),
            vec!["@user:matrix.org".to_string()],
        )
    }

    fn make_channel_with_synapseclaw_dir() -> MatrixChannel {
        MatrixChannel::new_with_session_hint_and_synapseclaw_dir(
            "https://matrix.org".to_string(),
            Some("syt_test_token".to_string()),
            "!room:matrix.org".to_string(),
            vec!["@user:matrix.org".to_string()],
            None,
            None,
            Some(PathBuf::from("/tmp/synapseclaw")),
        )
    }

    #[test]
    fn creates_with_correct_fields() {
        let ch = make_channel();
        assert_eq!(ch.homeserver, "https://matrix.org");
        assert_eq!(ch.access_token.as_deref(), Some("syt_test_token"));
        assert_eq!(ch.room_id, "!room:matrix.org");
        assert_eq!(ch.allowed_users.len(), 1);
    }

    #[test]
    fn strips_trailing_slash() {
        let ch = MatrixChannel::new(
            "https://matrix.org/".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
        );
        assert_eq!(ch.homeserver, "https://matrix.org");
    }

    #[test]
    fn no_trailing_slash_unchanged() {
        let ch = MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
        );
        assert_eq!(ch.homeserver, "https://matrix.org");
    }

    #[test]
    fn multiple_trailing_slashes_strip_all() {
        let ch = MatrixChannel::new(
            "https://matrix.org//".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
        );
        assert_eq!(ch.homeserver, "https://matrix.org");
    }

    #[test]
    fn trims_access_token() {
        let ch = MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("  syt_test_token  ".to_string()),
            "!r:m".to_string(),
            vec![],
        );
        assert_eq!(ch.access_token.as_deref(), Some("syt_test_token"));
    }

    #[test]
    fn session_hints_are_normalized() {
        let ch = MatrixChannel::new_with_session_hint(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
            Some("  @bot:matrix.org ".to_string()),
            Some("  DEVICE123  ".to_string()),
        );

        assert_eq!(ch.session_owner_hint.as_deref(), Some("@bot:matrix.org"));
        assert_eq!(ch.session_device_id_hint.as_deref(), Some("DEVICE123"));
    }

    #[test]
    fn empty_session_hints_are_ignored() {
        let ch = MatrixChannel::new_with_session_hint(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
            Some("   ".to_string()),
            Some(String::new()),
        );

        assert!(ch.session_owner_hint.is_none());
        assert!(ch.session_device_id_hint.is_none());
    }

    #[test]
    fn matrix_store_dir_is_derived_from_synapseclaw_dir() {
        let ch = MatrixChannel::new_with_session_hint_and_synapseclaw_dir(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
            None,
            None,
            Some(PathBuf::from("/tmp/synapseclaw")),
        );

        assert_eq!(
            ch.matrix_store_dir(),
            Some(PathBuf::from("/tmp/synapseclaw/state/matrix"))
        );
    }

    #[test]
    fn matrix_store_dir_absent_without_synapseclaw_dir() {
        let ch = MatrixChannel::new_with_session_hint(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
            None,
            None,
        );

        assert!(ch.matrix_store_dir().is_none());
    }

    #[test]
    fn call_runtime_config_uses_synapseclaw_dir_when_provided() {
        let ch = MatrixChannel::from_call_runtime_config_with_synapseclaw_dir(
            synapse_domain::config::schema::MatrixConfig {
                homeserver: "https://matrix.org".to_string(),
                access_token: Some("tok".to_string()),
                user_id: None,
                device_id: None,
                room_id: "!r:m".to_string(),
                allowed_users: vec![],
                password: None,
                max_media_download_mb: None,
            },
            Some(PathBuf::from("/tmp/synapseclaw")),
        );

        assert_eq!(
            ch.matrix_store_dir(),
            Some(PathBuf::from("/tmp/synapseclaw/state/matrix"))
        );
    }

    #[test]
    fn encode_path_segment_encodes_room_refs() {
        assert_eq!(
            MatrixChannel::encode_path_segment("#ops:matrix.example.com"),
            "%23ops%3Amatrix.example.com"
        );
        assert_eq!(
            MatrixChannel::encode_path_segment("!room:matrix.example.com"),
            "%21room%3Amatrix.example.com"
        );
    }

    #[test]
    fn supported_message_type_detection() {
        assert!(MatrixChannel::is_supported_message_type("m.text"));
        assert!(MatrixChannel::is_supported_message_type("m.notice"));
        assert!(MatrixChannel::is_supported_message_type("m.image"));
        assert!(MatrixChannel::is_supported_message_type("m.file"));
        assert!(MatrixChannel::is_supported_message_type("m.audio"));
        assert!(!MatrixChannel::is_supported_message_type("m.video"));
        assert!(!MatrixChannel::is_supported_message_type("m.location"));
    }

    #[test]
    fn body_presence_detection() {
        assert!(MatrixChannel::has_non_empty_body("hello"));
        assert!(MatrixChannel::has_non_empty_body("  hello  "));
        assert!(!MatrixChannel::has_non_empty_body(""));
        assert!(!MatrixChannel::has_non_empty_body("   \n\t  "));
    }

    #[test]
    fn send_content_uses_markdown_formatting() {
        let content = RoomMessageEventContent::text_markdown("**hello**");
        let value = serde_json::to_value(content).unwrap();

        assert_eq!(value["msgtype"], "m.text");
        assert_eq!(value["body"], "**hello**");
        assert_eq!(value["format"], "org.matrix.custom.html");
        assert!(value["formatted_body"]
            .as_str()
            .unwrap_or_default()
            .contains("<strong>hello</strong>"));
    }

    #[test]
    fn sync_filter_for_room_targets_requested_room() {
        let filter = MatrixChannel::sync_filter_for_room("!room:matrix.org", 0);
        let value: serde_json::Value = serde_json::from_str(&filter).unwrap();

        assert_eq!(value["room"]["rooms"][0], "!room:matrix.org");
        assert_eq!(value["room"]["timeline"]["limit"], 1);
    }

    #[test]
    fn event_id_cache_deduplicates_and_evicts_old_entries() {
        let mut recent_order = std::collections::VecDeque::new();
        let mut recent_lookup = std::collections::HashSet::new();

        assert!(!MatrixChannel::cache_event_id(
            "$first:event",
            &mut recent_order,
            &mut recent_lookup
        ));
        assert!(MatrixChannel::cache_event_id(
            "$first:event",
            &mut recent_order,
            &mut recent_lookup
        ));

        for i in 0..2050 {
            let event_id = format!("$event-{i}:matrix");
            MatrixChannel::cache_event_id(&event_id, &mut recent_order, &mut recent_lookup);
        }

        assert!(!MatrixChannel::cache_event_id(
            "$first:event",
            &mut recent_order,
            &mut recent_lookup
        ));
    }

    #[test]
    fn trims_room_id_and_allowed_users() {
        let ch = MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "  !room:matrix.org  ".to_string(),
            vec![
                "  @user:matrix.org  ".to_string(),
                "   ".to_string(),
                "@other:matrix.org".to_string(),
            ],
        );

        assert_eq!(ch.room_id, "!room:matrix.org");
        assert_eq!(ch.allowed_users.len(), 2);
        assert!(ch.allowed_users.contains(&"@user:matrix.org".to_string()));
        assert!(ch.allowed_users.contains(&"@other:matrix.org".to_string()));
    }

    #[test]
    fn matrix_rtc_transports_accept_new_and_legacy_livekit_types() {
        let new_transport = MatrixRtcTransportDescriptor {
            transport_type: "livekit".to_string(),
            livekit_service_url: Some("https://rtc.example.com".to_string()),
        };
        let legacy_transport = MatrixRtcTransportDescriptor {
            transport_type: "livekit_multi_sfu".to_string(),
            livekit_service_url: Some("https://rtc.example.com".to_string()),
        };
        let unsupported = MatrixRtcTransportDescriptor {
            transport_type: "something_else".to_string(),
            livekit_service_url: Some("https://rtc.example.com".to_string()),
        };

        assert!(is_livekit_matrix_rtc_transport(&new_transport));
        assert!(is_livekit_matrix_rtc_transport(&legacy_transport));
        assert!(!is_livekit_matrix_rtc_transport(&unsupported));
    }

    #[test]
    fn matrix_rtc_authorizer_request_matches_current_shape() {
        let body = MatrixChannel::matrix_rtc_authorizer_request(
            "!room:matrix.org",
            MatrixOpenIdToken {
                access_token: "token".to_string(),
                token_type: "Bearer".to_string(),
                matrix_server_name: "matrix.org".to_string(),
                expires_in: 3600,
            },
            "@bot:matrix.org".to_string(),
            "DEVICE123".to_string(),
        );

        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["room_id"], "!room:matrix.org");
        assert_eq!(value["slot_id"], "m.call#ROOM");
        assert_eq!(value["member"]["claimed_user_id"], "@bot:matrix.org");
        assert_eq!(value["member"]["claimed_device_id"], "DEVICE123");
        assert_eq!(
            value["member"]["id"],
            MatrixChannel::matrix_rtc_member_id("@bot:matrix.org", "DEVICE123")
        );
        assert_eq!(value["openid_token"]["access_token"], "token");
    }

    #[test]
    fn matrix_rtc_legacy_authorizer_request_matches_current_shape() {
        let body = MatrixChannel::matrix_rtc_legacy_authorizer_request(
            "!room:matrix.org",
            MatrixOpenIdToken {
                access_token: "token".to_string(),
                token_type: "Bearer".to_string(),
                matrix_server_name: "matrix.org".to_string(),
                expires_in: 3600,
            },
            "DEVICE123".to_string(),
        );

        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["room"], "!room:matrix.org");
        assert_eq!(value["device_id"], "DEVICE123");
        assert_eq!(value["openid_token"]["matrix_server_name"], "matrix.org");
    }

    #[test]
    fn wildcard_allows_anyone() {
        let ch = MatrixChannel::new(
            "https://m.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec!["*".to_string()],
        );
        assert!(ch.is_user_allowed("@anyone:matrix.org"));
        assert!(ch.is_user_allowed("@hacker:evil.org"));
    }

    #[test]
    fn specific_user_allowed() {
        let ch = make_channel();
        assert!(ch.is_user_allowed("@user:matrix.org"));
    }

    #[test]
    fn unknown_user_denied() {
        let ch = make_channel();
        assert!(!ch.is_user_allowed("@stranger:matrix.org"));
        assert!(!ch.is_user_allowed("@evil:hacker.org"));
    }

    #[test]
    fn user_case_insensitive() {
        let ch = MatrixChannel::new(
            "https://m.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec!["@User:Matrix.org".to_string()],
        );
        assert!(ch.is_user_allowed("@user:matrix.org"));
        assert!(ch.is_user_allowed("@USER:MATRIX.ORG"));
    }

    #[test]
    fn empty_allowlist_denies_all() {
        let ch = MatrixChannel::new(
            "https://m.org".to_string(),
            Some("tok".to_string()),
            "!r:m".to_string(),
            vec![],
        );
        assert!(!ch.is_user_allowed("@anyone:matrix.org"));
    }

    #[test]
    fn name_returns_matrix() {
        let ch = make_channel();
        assert_eq!(ch.name(), "matrix");
    }

    #[test]
    fn sync_response_deserializes_empty() {
        let json = r#"{"next_batch":"s123","rooms":{"join":{}}}"#;
        let resp: SyncResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.next_batch, "s123");
        assert!(resp.rooms.join.is_empty());
    }

    #[test]
    fn sync_response_deserializes_with_events() {
        let json = r#"{
            "next_batch": "s456",
            "rooms": {
                "join": {
                    "!room:matrix.org": {
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.message",
                                    "event_id": "$event:matrix.org",
                                    "sender": "@user:matrix.org",
                                    "content": {
                                        "msgtype": "m.text",
                                        "body": "Hello!"
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        }"#;
        let resp: SyncResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.next_batch, "s456");
        let room = resp.rooms.join.get("!room:matrix.org").unwrap();
        assert_eq!(room.timeline.events.len(), 1);
        assert_eq!(room.timeline.events[0].sender, "@user:matrix.org");
        assert_eq!(
            room.timeline.events[0].event_id.as_deref(),
            Some("$event:matrix.org")
        );
        assert_eq!(
            room.timeline.events[0].content.body.as_deref(),
            Some("Hello!")
        );
        assert_eq!(
            room.timeline.events[0].content.msgtype.as_deref(),
            Some("m.text")
        );
    }

    #[test]
    fn sync_response_ignores_non_text_events() {
        let json = r#"{
            "next_batch": "s789",
            "rooms": {
                "join": {
                    "!room:m": {
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.member",
                                    "sender": "@user:m",
                                    "content": {}
                                }
                            ]
                        }
                    }
                }
            }
        }"#;
        let resp: SyncResponse = serde_json::from_str(json).unwrap();
        let room = resp.rooms.join.get("!room:m").unwrap();
        assert_eq!(room.timeline.events[0].event_type, "m.room.member");
        assert!(room.timeline.events[0].content.body.is_none());
    }

    #[test]
    fn whoami_response_deserializes() {
        let json = r#"{"user_id":"@bot:matrix.org"}"#;
        let resp: WhoAmIResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.user_id, "@bot:matrix.org");
    }

    #[test]
    fn event_content_defaults() {
        let json = r#"{"type":"m.room.message","sender":"@u:m","content":{}}"#;
        let event: TimelineEvent = serde_json::from_str(json).unwrap();
        assert!(event.content.body.is_none());
        assert!(event.content.msgtype.is_none());
    }

    #[test]
    fn event_content_supports_notice_msgtype() {
        let json = r#"{
            "type":"m.room.message",
            "sender":"@u:m",
            "event_id":"$notice:m",
            "content":{"msgtype":"m.notice","body":"Heads up"}
        }"#;
        let event: TimelineEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.content.msgtype.as_deref(), Some("m.notice"));
        assert_eq!(event.content.body.as_deref(), Some("Heads up"));
        assert_eq!(event.event_id.as_deref(), Some("$notice:m"));
    }

    #[tokio::test]
    async fn invalid_room_reference_fails_fast() {
        let ch = MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "room_without_prefix".to_string(),
            vec![],
        );

        let err = ch.resolve_room_id().await.unwrap_err();
        assert!(err
            .to_string()
            .contains("must start with '!' (room ID) or '#' (room alias)"));
    }

    #[tokio::test]
    async fn target_room_id_keeps_canonical_room_id_without_lookup() {
        let ch = MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "!canonical:matrix.org".to_string(),
            vec![],
        );

        let room_id = ch.target_room_id().await.unwrap();
        assert_eq!(room_id, "!canonical:matrix.org");
    }

    #[tokio::test]
    async fn target_room_id_uses_cached_alias_resolution() {
        let ch = MatrixChannel::new(
            "https://matrix.org".to_string(),
            Some("tok".to_string()),
            "#ops:matrix.org".to_string(),
            vec![],
        );

        *ch.resolved_room_id_cache.write().await = Some("!cached:matrix.org".to_string());
        let room_id = ch.target_room_id().await.unwrap();
        assert_eq!(room_id, "!cached:matrix.org");
    }

    #[test]
    fn sync_response_missing_rooms_defaults() {
        let json = r#"{"next_batch":"s0"}"#;
        let resp: SyncResponse = serde_json::from_str(json).unwrap();
        assert!(resp.rooms.join.is_empty());
    }

    // --- Media support tests ---

    #[test]
    fn parse_matrix_attachment_markers_multiple() {
        let input = "Here is an image\n[IMAGE:/tmp/photo.png]\nAnd a file [FILE:/tmp/doc.pdf]";
        let (cleaned, attachments) = parse_matrix_attachment_markers(input);
        assert_eq!(cleaned, "Here is an image\n\nAnd a file");
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].kind, MatrixOutgoingAttachmentKind::Image);
        assert_eq!(attachments[0].target, "/tmp/photo.png");
        assert_eq!(attachments[1].kind, MatrixOutgoingAttachmentKind::File);
        assert_eq!(attachments[1].target, "/tmp/doc.pdf");
    }

    #[test]
    fn parse_matrix_attachment_markers_invalid_kept_as_text() {
        let input = "Hello [NOT_A_MARKER:foo] world";
        let (cleaned, attachments) = parse_matrix_attachment_markers(input);
        assert_eq!(cleaned, "Hello [NOT_A_MARKER:foo] world");
        assert!(attachments.is_empty());
    }

    #[test]
    fn parse_matrix_attachment_markers_empty_target() {
        let input = "[IMAGE:] some text";
        let (cleaned, attachments) = parse_matrix_attachment_markers(input);
        assert_eq!(cleaned, "[IMAGE:] some text");
        assert!(attachments.is_empty());
    }

    #[test]
    fn parse_matrix_attachment_markers_no_markers() {
        let input = "Just plain text";
        let (cleaned, attachments) = parse_matrix_attachment_markers(input);
        assert_eq!(cleaned, "Just plain text");
        assert!(attachments.is_empty());
    }

    #[test]
    fn matrix_media_artifacts_preserve_label_and_mime() {
        let mut artifact = MediaArtifact::new(MediaArtifactKind::Voice, "/tmp/voice.wav");
        artifact.label = Some("assistant-voice.wav".to_string());
        artifact.mime_type = Some("audio/wav".to_string());

        let attachments = matrix_media_artifact_attachments(&[artifact]).unwrap();

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].kind, MatrixOutgoingAttachmentKind::Voice);
        assert_eq!(attachments[0].target, "/tmp/voice.wav");
        assert_eq!(attachments[0].label.as_deref(), Some("assistant-voice.wav"));
        assert_eq!(attachments[0].mime_type.as_deref(), Some("audio/wav"));
    }

    #[test]
    fn matrix_msc3245_voice_marker_is_detected() {
        use matrix_sdk::ruma::{
            events::room::message::{AudioMessageEventContent, UnstableVoiceContentBlock},
            mxc_uri,
        };

        let mut content = AudioMessageEventContent::plain(
            "voice.ogg".to_string(),
            mxc_uri!("mxc://notareal.hs/voice").to_owned(),
        );

        assert!(!matrix_audio_message_is_voice(&content));
        assert_eq!(
            matrix_audio_transcription_text(false, "hello"),
            "[Audio] hello"
        );

        content.voice = Some(UnstableVoiceContentBlock::new());

        assert!(matrix_audio_message_is_voice(&content));
        assert_eq!(
            matrix_audio_transcription_text(true, "hello"),
            "[Voice] hello"
        );
        let serialized = serde_json::to_value(&content).unwrap();
        assert!(serialized.get("org.matrix.msc3245.voice").is_some());
    }

    #[test]
    fn outgoing_attachment_kind_from_marker_all_variants() {
        assert_eq!(
            MatrixOutgoingAttachmentKind::from_marker("IMAGE"),
            Some(MatrixOutgoingAttachmentKind::Image)
        );
        assert_eq!(
            MatrixOutgoingAttachmentKind::from_marker("photo"),
            Some(MatrixOutgoingAttachmentKind::Image)
        );
        assert_eq!(
            MatrixOutgoingAttachmentKind::from_marker("DOCUMENT"),
            Some(MatrixOutgoingAttachmentKind::File)
        );
        assert_eq!(
            MatrixOutgoingAttachmentKind::from_marker("file"),
            Some(MatrixOutgoingAttachmentKind::File)
        );
        assert_eq!(
            MatrixOutgoingAttachmentKind::from_marker("Audio"),
            Some(MatrixOutgoingAttachmentKind::Audio)
        );
        assert_eq!(
            MatrixOutgoingAttachmentKind::from_marker("VOICE"),
            Some(MatrixOutgoingAttachmentKind::Voice)
        );
        assert_eq!(MatrixOutgoingAttachmentKind::from_marker("unknown"), None);
    }

    #[test]
    fn matrix_voice_audio_info_extracts_duration_size_and_waveform() {
        let wav = tiny_pcm16_wav_with_placeholder_sizes();
        let info = matrix_audio_info(MatrixOutgoingAttachmentKind::Voice, &wav);

        assert_eq!(info.size, Some(matrix_sdk::ruma::UInt::new_wrapping(52)));
        assert_eq!(info.duration, Some(std::time::Duration::from_millis(1)));
        let waveform = info.waveform.expect("voice waveform should be present");
        assert_eq!(waveform.len(), 64);
        assert!(waveform.iter().any(|value| *value > 0.0));
    }

    #[test]
    fn matrix_audio_info_omits_waveform_for_plain_audio() {
        let wav = tiny_pcm16_wav_with_placeholder_sizes();
        let info = matrix_audio_info(MatrixOutgoingAttachmentKind::Audio, &wav);

        assert_eq!(info.size, Some(matrix_sdk::ruma::UInt::new_wrapping(52)));
        assert_eq!(info.duration, Some(std::time::Duration::from_millis(1)));
        assert!(info.waveform.is_none());
    }

    #[test]
    fn wav_pcm16_payload_extracts_samples_and_metadata() {
        let wav = tiny_pcm16_wav_with_placeholder_sizes();
        let payload = wav_pcm16_payload(&wav).expect("pcm16 wav payload");

        assert_eq!(payload.sample_rate, 48_000);
        assert_eq!(payload.channels, 1);
        assert_eq!(payload.duration, std::time::Duration::from_millis(1));
        assert_eq!(payload.samples.len(), 4);
        assert_eq!(payload.samples[1], i16::MAX);
        assert_eq!(payload.samples[3], i16::MIN);
    }

    fn tiny_pcm16_wav_with_placeholder_sizes() -> Vec<u8> {
        let mut wav = vec![
            b'R', b'I', b'F', b'F', 0xff, 0xff, 0xff, 0xff, b'W', b'A', b'V', b'E', b'f', b'm',
            b't', b' ', 16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0xbb, 0, 0, 0, 0x77, 1, 0, 2, 0, 16, 0,
            b'd', b'a', b't', b'a', 0xff, 0xff, 0xff, 0xff,
        ];
        wav.extend_from_slice(&[0, 0, 0xff, 0x7f, 0, 0, 0x00, 0x80]);
        wav
    }

    #[test]
    fn is_image_extension_known() {
        assert!(is_image_extension(Path::new("photo.png")));
        assert!(is_image_extension(Path::new("photo.JPG")));
        assert!(is_image_extension(Path::new("photo.jpeg")));
        assert!(is_image_extension(Path::new("photo.gif")));
        assert!(is_image_extension(Path::new("photo.webp")));
        assert!(is_image_extension(Path::new("photo.bmp")));
    }

    #[test]
    fn is_image_extension_unknown() {
        assert!(!is_image_extension(Path::new("file.pdf")));
        assert!(!is_image_extension(Path::new("audio.ogg")));
        assert!(!is_image_extension(Path::new("noext")));
    }

    #[test]
    fn media_save_dir_derived_from_synapseclaw_dir() {
        let ch = make_channel_with_synapseclaw_dir();
        assert_eq!(
            ch.media_save_dir(),
            Some(PathBuf::from("/tmp/synapseclaw/workspace/matrix_files"))
        );
    }

    #[test]
    fn media_save_dir_absent_without_synapseclaw_dir() {
        let ch = make_channel();
        assert!(ch.media_save_dir().is_none());
    }

    #[test]
    fn with_transcription_enabled() {
        let config = synapse_domain::config::schema::TranscriptionConfig {
            enabled: true,
            ..Default::default()
        };
        let ch = make_channel().with_transcription(config);
        assert!(ch.transcription.is_some());
    }

    #[test]
    fn with_transcription_disabled() {
        let config = synapse_domain::config::schema::TranscriptionConfig {
            enabled: false,
            ..Default::default()
        };
        let ch = make_channel().with_transcription(config);
        assert!(ch.transcription.is_none());
    }

    #[test]
    fn with_tts_enabled() {
        let config = TtsConfig {
            enabled: true,
            default_provider: "groq".into(),
            default_format: "wav".into(),
            default_voice: "hannah".into(),
            groq: Some(synapse_domain::config::schema::GroqTtsConfig {
                api_key: Some("key".into()),
                model: "canopylabs/orpheus-v1-english".into(),
                response_format: "wav".into(),
            }),
            ..Default::default()
        };
        let ch = make_channel().with_tts(config);
        assert!(ch.tts.is_some());
    }

    #[test]
    fn with_password_stores_value() {
        let ch = make_channel().with_password(Some("hunter2".into()));
        assert_eq!(ch.password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn with_password_none_clears() {
        let ch = make_channel()
            .with_password(Some("hunter2".into()))
            .with_password(None);
        assert!(ch.password.is_none());
    }

    #[test]
    fn default_max_media_download_size_is_50mb() {
        assert_eq!(DEFAULT_MAX_MEDIA_DOWNLOAD_BYTES, 50 * 1024 * 1024);
        let ch = make_channel();
        assert_eq!(ch.max_media_bytes, DEFAULT_MAX_MEDIA_DOWNLOAD_BYTES);
    }

    #[test]
    fn with_max_media_download_mb_custom() {
        let ch = make_channel().with_max_media_download_mb(Some(100));
        assert_eq!(ch.max_media_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn with_max_media_download_mb_zero_means_no_limit() {
        let ch = make_channel().with_max_media_download_mb(Some(0));
        assert_eq!(ch.max_media_bytes, usize::MAX);
    }

    #[test]
    fn with_max_media_download_mb_none_uses_default() {
        let ch = make_channel().with_max_media_download_mb(None);
        assert_eq!(ch.max_media_bytes, DEFAULT_MAX_MEDIA_DOWNLOAD_BYTES);
    }

    #[test]
    fn outgoing_ring_persists_target_room_identity() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();

        record_matrix_outgoing_ring(
            "$mx-outgoing",
            Some("$mx-membership"),
            "!dm:matrix.org",
            RealtimeCallOrigin::cli_request(),
            Some("Morning briefing".into()),
        );

        let session =
            matrix_call_session("$mx-outgoing").expect("session present after outgoing ring");
        assert_eq!(session.direction, RealtimeCallDirection::Outbound);
        assert_eq!(session.state, RealtimeCallState::Ringing);
        assert_eq!(session.call_leg_id.as_deref(), Some("$mx-membership"));
        assert_eq!(session.call_session_id.as_deref(), Some("!dm:matrix.org"));
        assert_eq!(session.objective.as_deref(), Some("Morning briefing"));
    }

    #[test]
    fn incoming_ring_context_records_sender_and_room() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();

        record_matrix_incoming_ring_context("$mx-incoming", "!dm:matrix.org", "@user:matrix.org");

        let session = matrix_call_session("$mx-incoming").expect("incoming session recorded");
        assert_eq!(session.direction, RealtimeCallDirection::Inbound);
        assert_eq!(session.state, RealtimeCallState::Ringing);
        assert_eq!(session.call_session_id.as_deref(), Some("!dm:matrix.org"));
        assert_eq!(
            session.origin.recipient.as_deref(),
            Some("@user:matrix.org")
        );
        assert_eq!(
            session.origin.conversation_id.as_deref(),
            Some("!dm:matrix.org")
        );
    }

    #[test]
    fn incoming_membership_context_reuses_active_session_for_same_sender_and_room() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();

        let (first_call_id, first_is_new) = record_matrix_incoming_membership_context(
            "$mx-member-1",
            "!dm:matrix.org",
            "@user:matrix.org",
        );
        assert!(first_is_new);
        let (second_call_id, second_is_new) = record_matrix_incoming_membership_context(
            "$mx-member-2",
            "!dm:matrix.org",
            "@user:matrix.org",
        );

        assert_eq!(first_call_id, "$mx-member-1");
        assert_eq!(second_call_id, first_call_id);
        assert!(!second_is_new);
        assert_eq!(matrix_recent_call_sessions().len(), 1);
    }

    #[test]
    fn reply_target_state_update_marks_matrix_call_thinking() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();
        record_matrix_incoming_ring_context("$mx-thinking", "!dm:matrix.org", "@user:matrix.org");

        assert!(matrix_set_call_state_for_reply_target(
            "matrix-call:@user:matrix.org||!dm:matrix.org",
            RealtimeCallState::Thinking,
        ));

        let session = matrix_call_session("$mx-thinking").expect("session recorded");
        assert_eq!(session.state, RealtimeCallState::Thinking);
        assert!(!matrix_set_call_state_for_reply_target(
            "matrix-call:@other:matrix.org||!dm:matrix.org",
            RealtimeCallState::Thinking,
        ));
    }

    #[test]
    fn session_for_reply_target_resolves_active_call_context() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();
        record_matrix_incoming_ring_context("$mx-session", "!dm:matrix.org", "@user:matrix.org");

        let session =
            matrix_call_session_for_reply_target("matrix-call:@user:matrix.org||!dm:matrix.org")
                .expect("reply target should resolve active session");
        assert_eq!(session.call_control_id, "$mx-session");
    }

    #[test]
    fn incoming_call_event_text_contains_sender_room_and_call_id() {
        let text =
            matrix_incoming_call_event_text("@user:matrix.org", "!dm:matrix.org", "$mx-call");
        assert!(text.contains("@user:matrix.org"));
        assert!(text.contains("!dm:matrix.org"));
        assert!(text.contains("$mx-call"));
    }

    #[test]
    fn sanitize_live_call_transcript_rejects_replacement_noise() {
        assert_eq!(sanitize_live_call_transcript(" \u{fffd}\u{fffd}\u{fffd} "), None);
    }

    #[test]
    fn sanitize_live_call_transcript_normalizes_whitespace() {
        assert_eq!(
            sanitize_live_call_transcript("  hello   world  ").as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn append_bounded_debug_samples_respects_cap() {
        let mut dst = vec![1i16, 2];
        append_bounded_debug_samples(&mut dst, &[3, 4, 5], 4);
        assert_eq!(dst, vec![1, 2, 3, 4]);
    }

    #[test]
    fn matrix_debug_call_dump_name_sanitizes_path_unsafe_bytes() {
        assert_eq!(
            matrix_debug_call_dump_name("$call/id:unsafe"),
            "_call_id_unsafe.wav"
        );
    }

    #[test]
    fn ending_by_sender_and_room_closes_latest_active_inbound_session() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();
        record_matrix_incoming_membership_context(
            "$mx-member-3",
            "!dm:matrix.org",
            "@user:matrix.org",
        );

        let ended = record_matrix_call_ended_for_sender_room(
            "!dm:matrix.org",
            "@user:matrix.org",
            "remote_hangup",
        );

        assert_eq!(ended.as_deref(), Some("$mx-member-3"));
        let session = matrix_call_session("$mx-member-3").expect("session recorded");
        assert_eq!(session.state, RealtimeCallState::Ended);
        assert_eq!(session.end_reason.as_deref(), Some("remote_hangup"));
    }

    #[test]
    fn persisted_session_end_update_loads_existing_matrix_call() {
        let _guard = matrix_call_test_lock();
        reset_matrix_call_status();

        {
            let mut status = matrix_call_control_status_slot().write();
            status.recent_sessions = vec![RealtimeCallSessionSnapshot {
                channel: "matrix".into(),
                kind: RealtimeCallKind::Audio,
                direction: RealtimeCallDirection::Outbound,
                origin: RealtimeCallOrigin::cli_request(),
                objective: Some("Smoke".into()),
                call_control_id: "$mx-call".into(),
                call_leg_id: None,
                call_session_id: Some("!room:matrix.org".into()),
                state: RealtimeCallState::Ringing,
                created_at: "2026-04-21T10:00:00Z".into(),
                updated_at: "2026-04-21T10:00:00Z".into(),
                ended_at: None,
                end_reason: None,
                summary: None,
                decisions: Vec::new(),
                message_count: 0,
                interruption_count: 0,
                last_sequence: None,
            }];
        }

        record_matrix_call_ended_with_reason("$mx-call", "operator_hangup");

        let session = matrix_call_session("$mx-call").expect("session present after end update");
        assert_eq!(session.state, RealtimeCallState::Ended);
        assert_eq!(session.end_reason.as_deref(), Some("operator_hangup"));
        assert!(session.ended_at.is_some());
    }
}
