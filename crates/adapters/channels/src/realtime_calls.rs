//! Realtime call runtime wiring for channel adapters.
//!
//! Capability profiles say whether a channel may be used for realtime calls.
//! This module is the single adapter-level place that turns a configured
//! channel into an executable call runtime.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use synapse_domain::config::schema::{ChannelsConfig, TtsConfig};
use synapse_domain::domain::channel::ChannelCapability;
use synapse_domain::ports::realtime_call::{
    RealtimeCallRuntimePort, RealtimeCallSessionSnapshot, RealtimeCallState,
};

use crate::capabilities::{
    declared_channel_capability_profile, declared_channel_capability_profiles,
};
use crate::clawdtalk::{
    clawdtalk_bridge_status, clawdtalk_call_session_for_reply_target, clawdtalk_recent_sessions,
    clawdtalk_session, clawdtalk_set_call_state_for_reply_target,
    configure_clawdtalk_call_ledger_dir, ClawdTalkChannel,
};
#[cfg(feature = "channel-matrix")]
use crate::matrix::{
    configure_matrix_call_ledger_dir, matrix_call_control_status, matrix_call_session,
    matrix_call_session_for_reply_target, matrix_media_attached, matrix_recent_call_sessions,
    matrix_set_call_state_for_reply_target, MatrixChannel,
};
use crate::realtime_call_ledger::load_persisted_transport_call_sessions;
use crate::traits::Channel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeCallRuntimeSupport {
    Available,
    ControlOnly,
    Planned,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallRuntimeHealth {
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_attempts: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_calls: Vec<RealtimeCallSessionSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_sessions: Vec<RealtimeCallSessionSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallActionSupport {
    pub start: bool,
    pub answer: bool,
    pub speak: bool,
    pub hangup: bool,
    pub inspect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealtimeCallTransportDetails {
    #[serde(rename = "clawdtalk")]
    ClawdTalk {
        api_key_configured: bool,
        websocket_configured: bool,
        websocket_url: Option<String>,
        api_base_url: Option<String>,
        assistant_configured: bool,
        bridge_ready: bool,
        outbound_start_ready: bool,
        call_control_ready: bool,
    },
    Matrix {
        auth_mode: MatrixStatusAuthMode,
        auth_source: MatrixStatusAuthSource,
        widget_support_enabled: bool,
        room_reference: Option<String>,
        resolved_room_id: Option<String>,
        room_accessible: Option<bool>,
        room_encrypted: Option<bool>,
        rtc_bootstrap: Option<crate::matrix::MatrixRtcBootstrapStatus>,
        turn_engine: Option<crate::realtime_turn_engine::RealtimeTurnEngineStatus>,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixStatusAuthMode {
    AccessToken,
    Password,
    #[default]
    Missing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixStatusAuthSource {
    SessionStore,
    AccessToken,
    Password,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallTransportStatus {
    pub channel: String,
    pub transport_configured: bool,
    pub audio_call_runtime: RealtimeCallRuntimeSupport,
    pub video_call_runtime: RealtimeCallRuntimeSupport,
    pub media_attached: bool,
    pub action_support: RealtimeCallActionSupport,
    pub runtime_selected_by_default: bool,
    pub runtime_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<RealtimeCallTransportDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<RealtimeCallRuntimeHealth>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallStatusReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_channel: Option<String>,
    pub channels: Vec<RealtimeCallTransportStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeCallRuntimeConfigError {
    ConfirmationRequired,
    EmptyArgument {
        name: String,
    },
    NoConfiguredRuntime,
    AmbiguousDefault {
        available: Vec<String>,
    },
    Unavailable {
        channel: String,
        support: RealtimeCallRuntimeSupport,
    },
    MissingConfig {
        channel: String,
        config_key: &'static str,
    },
    MissingRuntimeFactory {
        channel: String,
    },
}

impl fmt::Display for RealtimeCallRuntimeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfirmationRequired => write!(
                f,
                "realtime call actions create external telephony side effects; confirm=true is required"
            ),
            Self::EmptyArgument { name } => write!(f, "{name} cannot be empty"),
            Self::NoConfiguredRuntime => write!(
                f,
                "no configured realtime audio call runtime is available"
            ),
            Self::AmbiguousDefault { available } => write!(
                f,
                "multiple realtime audio call runtimes are configured ({}); specify `channel`",
                available.join(", ")
            ),
            Self::Unavailable { channel, support } => write!(
                f,
                "channel `{channel}` does not have an executable realtime audio call runtime; support={support:?}"
            ),
            Self::MissingConfig {
                channel,
                config_key,
            } => write!(f, "{config_key} is not configured for channel `{channel}`"),
            Self::MissingRuntimeFactory { channel } => write!(
                f,
                "channel `{channel}` declares realtime audio calls but has no runtime factory"
            ),
        }
    }
}

impl Error for RealtimeCallRuntimeConfigError {}

fn clawdtalk_transport_details(
    status: &crate::clawdtalk::ClawdTalkBridgeStatus,
) -> RealtimeCallTransportDetails {
    RealtimeCallTransportDetails::ClawdTalk {
        api_key_configured: status.api_key_configured,
        websocket_configured: status.websocket_configured,
        websocket_url: status.websocket_url.clone(),
        api_base_url: status.api_base_url.clone(),
        assistant_configured: status.assistant_configured,
        bridge_ready: status.bridge_ready,
        outbound_start_ready: status.outbound_start_ready,
        call_control_ready: status.call_control_ready,
    }
}

#[cfg(feature = "channel-matrix")]
fn matrix_transport_details(
    status: &crate::matrix::MatrixCallControlStatus,
) -> RealtimeCallTransportDetails {
    RealtimeCallTransportDetails::Matrix {
        auth_mode: status.auth_mode,
        auth_source: status.auth_source,
        widget_support_enabled: status.widget_support_enabled,
        room_reference: status.room_reference.clone(),
        resolved_room_id: status.resolved_room_id.clone(),
        room_accessible: status.room_accessible,
        room_encrypted: status.room_encrypted,
        rtc_bootstrap: status.rtc_bootstrap.clone(),
        turn_engine: status.turn_engine.clone(),
    }
}

fn runtime_support(
    profile: &synapse_domain::ports::channel_registry::ChannelCapabilityProfile,
    capability: ChannelCapability,
    _channels_config: &ChannelsConfig,
) -> RealtimeCallRuntimeSupport {
    if profile.has(capability) {
        RealtimeCallRuntimeSupport::Available
    } else if profile.plans(capability) {
        RealtimeCallRuntimeSupport::Planned
    } else {
        RealtimeCallRuntimeSupport::Unsupported
    }
}

fn inspectable_call_support(
    channel: &str,
    channels_config: &ChannelsConfig,
) -> RealtimeCallRuntimeSupport {
    let profile = declared_channel_capability_profile(channel);
    runtime_support(
        &profile,
        ChannelCapability::RealtimeAudioCall,
        channels_config,
    )
}

fn transport_configured(channel: &str, channels_config: &ChannelsConfig) -> bool {
    match channel {
        "clawdtalk" => channels_config.clawdtalk.is_some(),
        "matrix" => channels_config.matrix.is_some(),
        "telegram" => channels_config.telegram.is_some(),
        "signal" => channels_config.signal.is_some(),
        _ => false,
    }
}

fn action_support_for_channel(
    channel: &str,
    channels_config: &ChannelsConfig,
) -> RealtimeCallActionSupport {
    match channel {
        "clawdtalk" if channels_config.clawdtalk.is_some() => RealtimeCallActionSupport {
            start: true,
            answer: true,
            speak: true,
            hangup: true,
            inspect: true,
        },
        "matrix" if channels_config.matrix.is_some() => RealtimeCallActionSupport {
            start: true,
            answer: true,
            speak: true,
            hangup: true,
            inspect: true,
        },
        _ => RealtimeCallActionSupport {
            start: false,
            answer: false,
            speak: false,
            hangup: false,
            inspect: false,
        },
    }
}

pub fn normalize_realtime_call_channel(channel: &str) -> String {
    channel.trim().to_ascii_lowercase()
}

pub fn resolve_current_conversation_realtime_call_target(
    channel: &str,
    actor_id: &str,
    reply_ref: &str,
) -> Option<String> {
    let channel = normalize_realtime_call_channel(channel);
    let actor_id = actor_id.trim();
    let reply_ref = reply_ref.trim();

    match channel.as_str() {
        #[cfg(feature = "channel-matrix")]
        "matrix" => {
            if actor_id.starts_with('@') {
                Some(actor_id.to_string())
            } else if !reply_ref.is_empty() {
                Some(reply_ref.to_string())
            } else {
                None
            }
        }
        _ => {
            if !reply_ref.is_empty() {
                Some(reply_ref.to_string())
            } else if !actor_id.is_empty() {
                Some(actor_id.to_string())
            } else {
                None
            }
        }
    }
}

pub fn require_realtime_call_confirmation(
    confirm: bool,
) -> Result<(), RealtimeCallRuntimeConfigError> {
    if confirm {
        Ok(())
    } else {
        Err(RealtimeCallRuntimeConfigError::ConfirmationRequired)
    }
}

pub fn non_empty_realtime_call_arg(
    name: &str,
    value: impl AsRef<str>,
) -> Result<String, RealtimeCallRuntimeConfigError> {
    let trimmed = value.as_ref().trim();
    if trimmed.is_empty() {
        return Err(RealtimeCallRuntimeConfigError::EmptyArgument {
            name: name.to_string(),
        });
    }
    Ok(trimmed.to_string())
}

pub fn ensure_realtime_audio_call_available(
    channel: &str,
    channels_config: &ChannelsConfig,
) -> Result<(), RealtimeCallRuntimeConfigError> {
    let channel = normalize_realtime_call_channel(channel);
    let support = inspectable_call_support(&channel, channels_config);
    if matches!(
        support,
        RealtimeCallRuntimeSupport::Available | RealtimeCallRuntimeSupport::ControlOnly
    ) {
        return Ok(());
    }
    Err(RealtimeCallRuntimeConfigError::Unavailable { channel, support })
}

fn ensure_realtime_audio_call_inspectable(
    channel: &str,
    channels_config: &ChannelsConfig,
) -> Result<(), RealtimeCallRuntimeConfigError> {
    let channel = normalize_realtime_call_channel(channel);
    let support = inspectable_call_support(&channel, channels_config);
    if matches!(
        support,
        RealtimeCallRuntimeSupport::Available | RealtimeCallRuntimeSupport::ControlOnly
    ) {
        return Ok(());
    }
    Err(RealtimeCallRuntimeConfigError::Unavailable { channel, support })
}

pub fn configured_realtime_audio_call_channels(channels_config: &ChannelsConfig) -> Vec<String> {
    declared_channel_capability_profiles()
        .into_iter()
        .filter(|profile| {
            matches!(
                runtime_support(
                    profile,
                    ChannelCapability::RealtimeAudioCall,
                    channels_config
                ),
                RealtimeCallRuntimeSupport::Available | RealtimeCallRuntimeSupport::ControlOnly
            ) && transport_configured(&profile.channel, channels_config)
        })
        .map(|profile| profile.channel)
        .collect()
}

pub fn configured_realtime_audio_call_inspection_channels(
    channels_config: &ChannelsConfig,
) -> Vec<String> {
    declared_channel_capability_profiles()
        .into_iter()
        .filter(|profile| {
            matches!(
                runtime_support(
                    profile,
                    ChannelCapability::RealtimeAudioCall,
                    channels_config
                ),
                RealtimeCallRuntimeSupport::Available | RealtimeCallRuntimeSupport::ControlOnly
            ) && transport_configured(&profile.channel, channels_config)
        })
        .map(|profile| profile.channel)
        .collect()
}

pub fn realtime_call_status_report(channels_config: &ChannelsConfig) -> RealtimeCallStatusReport {
    let default_channel = resolve_realtime_audio_call_channel(None, channels_config).ok();
    let channels = declared_channel_capability_profiles()
        .into_iter()
        .filter(|profile| {
            profile.has(ChannelCapability::RealtimeAudioCall)
                || profile.plans(ChannelCapability::RealtimeAudioCall)
                || profile.has(ChannelCapability::RealtimeVideoCall)
                || profile.plans(ChannelCapability::RealtimeVideoCall)
        })
        .map(|profile| {
            let channel = profile.channel.clone();
            let action_support = action_support_for_channel(&channel, channels_config);
            let (runtime_ready, details, health) = match channel.as_str() {
                "clawdtalk" => {
                    let status = clawdtalk_bridge_status(channels_config.clawdtalk.as_ref());
                    (
                        status.outbound_start_ready || status.call_control_ready,
                        Some(clawdtalk_transport_details(&status)),
                        Some(RealtimeCallRuntimeHealth {
                            ready: status.outbound_start_ready || status.call_control_ready,
                            connected: Some(status.connected),
                            last_error: status.last_error,
                            reconnect_attempts: Some(status.reconnect_attempts),
                            active_calls: status.active_calls,
                            recent_sessions: status.recent_sessions,
                        }),
                    )
                }
                #[cfg(feature = "channel-matrix")]
                "matrix" => {
                    let status = matrix_call_control_status(channels_config.matrix.as_ref());
                    let bootstrap_ready = status
                        .rtc_bootstrap
                        .as_ref()
                        .map(|bootstrap| bootstrap.media_bootstrap_ready)
                        .unwrap_or(false);
                    let turn_engine_ready = status
                        .turn_engine
                        .as_ref()
                        .map(|engine| engine.ready)
                        .unwrap_or(false);
                    (
                        status.room_accessible.unwrap_or(false)
                            && status.widget_support_enabled
                            && bootstrap_ready
                            && turn_engine_ready,
                        Some(matrix_transport_details(&status)),
                        Some(RealtimeCallRuntimeHealth {
                            ready: status.room_accessible.unwrap_or(false)
                                && status.widget_support_enabled
                                && bootstrap_ready
                                && turn_engine_ready,
                            connected: Some(matrix_media_attached()),
                            last_error: status
                                .turn_engine
                                .as_ref()
                                .and_then(|engine| engine.last_error.clone())
                                .or(status.last_error),
                            reconnect_attempts: None,
                            active_calls: status.active_calls,
                            recent_sessions: status.recent_sessions,
                        }),
                    )
                }
                _ => (false, None, None),
            };

            RealtimeCallTransportStatus {
                channel: channel.clone(),
                transport_configured: transport_configured(&channel, channels_config),
                audio_call_runtime: runtime_support(
                    &profile,
                    ChannelCapability::RealtimeAudioCall,
                    channels_config,
                ),
                video_call_runtime: runtime_support(
                    &profile,
                    ChannelCapability::RealtimeVideoCall,
                    channels_config,
                ),
                media_attached: match channel.as_str() {
                    "clawdtalk" => {
                        transport_configured(&channel, channels_config)
                            && health
                                .as_ref()
                                .is_some_and(|health| !health.active_calls.is_empty())
                    }
                    #[cfg(feature = "channel-matrix")]
                    "matrix" => matrix_media_attached(),
                    _ => false,
                },
                action_support,
                runtime_selected_by_default: default_channel.as_deref() == Some(channel.as_str()),
                runtime_ready,
                details,
                health,
            }
        })
        .collect();

    RealtimeCallStatusReport {
        default_channel,
        channels,
    }
}

pub async fn realtime_call_status_report_live(
    channels_config: &ChannelsConfig,
    tts_config: Option<synapse_domain::config::schema::TtsConfig>,
    transcription_config: Option<synapse_domain::config::schema::TranscriptionConfig>,
) -> RealtimeCallStatusReport {
    realtime_call_status_report_live_with_synapseclaw_dir(
        channels_config,
        None,
        tts_config,
        transcription_config,
    )
    .await
}

pub async fn realtime_call_status_report_live_with_synapseclaw_dir(
    channels_config: &ChannelsConfig,
    synapseclaw_dir: Option<PathBuf>,
    tts_config: Option<synapse_domain::config::schema::TtsConfig>,
    transcription_config: Option<synapse_domain::config::schema::TranscriptionConfig>,
) -> RealtimeCallStatusReport {
    const STATUS_PROBE_TIMEOUT_SECS: u64 = 5;
    configure_clawdtalk_call_ledger_dir(synapseclaw_dir.clone());
    #[cfg(feature = "channel-matrix")]
    configure_matrix_call_ledger_dir(synapseclaw_dir.clone());
    let mut report = realtime_call_status_report(channels_config);

    for transport in &mut report.channels {
        match transport.channel.as_str() {
            "clawdtalk" => {
                let Some(config) = channels_config.clawdtalk.clone() else {
                    continue;
                };
                let channel =
                    ClawdTalkChannel::new_with_synapseclaw_dir(config, synapseclaw_dir.clone());
                let health_ok = tokio::time::timeout(
                    std::time::Duration::from_secs(STATUS_PROBE_TIMEOUT_SECS),
                    channel.health_check(),
                )
                .await
                .unwrap_or(false);
                let refreshed = clawdtalk_bridge_status(channels_config.clawdtalk.as_ref());
                transport.runtime_ready &= health_ok;
                transport.details = Some(clawdtalk_transport_details(&refreshed));
                if let Some(health) = transport.health.as_mut() {
                    health.ready = transport.runtime_ready;
                    health.connected = Some(refreshed.connected);
                    health.reconnect_attempts = Some(refreshed.reconnect_attempts);
                    health.active_calls = refreshed.active_calls;
                    health.recent_sessions = refreshed.recent_sessions;
                    if !health_ok && health.last_error.is_none() && transport.transport_configured {
                        health.last_error =
                            Some("clawdtalk health probe failed or timed out".into());
                    }
                }
            }
            #[cfg(feature = "channel-matrix")]
            "matrix" => {
                let Some(config) = channels_config.matrix.clone() else {
                    continue;
                };
                let channel = MatrixChannel::from_call_runtime_config_with_support(
                    config,
                    synapseclaw_dir.clone(),
                    tts_config.clone(),
                    transcription_config.clone(),
                );
                let health_ok = tokio::time::timeout(
                    std::time::Duration::from_secs(STATUS_PROBE_TIMEOUT_SECS),
                    channel.health_check(),
                )
                .await
                .unwrap_or(false);
                let refreshed = matrix_call_control_status(channels_config.matrix.as_ref());
                let bootstrap_ready = refreshed
                    .rtc_bootstrap
                    .as_ref()
                    .map(|bootstrap| bootstrap.media_bootstrap_ready)
                    .unwrap_or(false);
                let turn_engine_ready = refreshed
                    .turn_engine
                    .as_ref()
                    .map(|engine| engine.ready)
                    .unwrap_or(false);
                transport.runtime_ready = health_ok
                    && refreshed.room_accessible.unwrap_or(false)
                    && refreshed.widget_support_enabled
                    && bootstrap_ready
                    && turn_engine_ready;
                transport.details = Some(matrix_transport_details(&refreshed));
                transport.health = Some(RealtimeCallRuntimeHealth {
                    ready: transport.runtime_ready,
                    connected: Some(matrix_media_attached()),
                    last_error: refreshed
                        .turn_engine
                        .as_ref()
                        .and_then(|engine| engine.last_error.clone())
                        .or(refreshed.last_error)
                        .or_else(|| {
                            if transport.transport_configured && !health_ok {
                                Some("matrix health probe failed or timed out".into())
                            } else {
                                None
                            }
                        }),
                    reconnect_attempts: None,
                    active_calls: refreshed.active_calls,
                    recent_sessions: refreshed.recent_sessions,
                });
            }
            _ => {}
        }
    }

    report
}

pub fn resolve_realtime_audio_call_channel(
    requested: Option<&str>,
    channels_config: &ChannelsConfig,
) -> Result<String, RealtimeCallRuntimeConfigError> {
    if let Some(channel) = requested
        .map(normalize_realtime_call_channel)
        .filter(|channel| !channel.is_empty())
    {
        ensure_realtime_audio_call_available(&channel, channels_config)?;
        match channel.as_str() {
            "clawdtalk" => {
                if channels_config.clawdtalk.is_some() {
                    return Ok(channel);
                }
                return Err(RealtimeCallRuntimeConfigError::MissingConfig {
                    channel,
                    config_key: "channels_config.clawdtalk",
                });
            }
            #[cfg(feature = "channel-matrix")]
            "matrix" => {
                if channels_config.matrix.is_some() {
                    return Ok(channel);
                }
                return Err(RealtimeCallRuntimeConfigError::MissingConfig {
                    channel,
                    config_key: "channels_config.matrix",
                });
            }
            _ => {
                return Err(RealtimeCallRuntimeConfigError::MissingRuntimeFactory { channel });
            }
        }
    }

    let available = configured_realtime_audio_call_channels(channels_config);
    match available.as_slice() {
        [] => Err(RealtimeCallRuntimeConfigError::NoConfiguredRuntime),
        [channel] => Ok(channel.clone()),
        _ => Err(RealtimeCallRuntimeConfigError::AmbiguousDefault { available }),
    }
}

pub fn resolve_realtime_audio_call_inspection_channel(
    requested: Option<&str>,
    channels_config: &ChannelsConfig,
) -> Result<String, RealtimeCallRuntimeConfigError> {
    if let Some(channel) = requested
        .map(normalize_realtime_call_channel)
        .filter(|channel| !channel.is_empty())
    {
        ensure_realtime_audio_call_inspectable(&channel, channels_config)?;
        return match channel.as_str() {
            "clawdtalk" if channels_config.clawdtalk.is_none() => {
                Err(RealtimeCallRuntimeConfigError::MissingConfig {
                    channel,
                    config_key: "channels_config.clawdtalk",
                })
            }
            "matrix" if channels_config.matrix.is_none() => {
                Err(RealtimeCallRuntimeConfigError::MissingConfig {
                    channel,
                    config_key: "channels_config.matrix",
                })
            }
            _ => Ok(channel),
        };
    }

    let available = configured_realtime_audio_call_inspection_channels(channels_config);
    match available.as_slice() {
        [] => Err(RealtimeCallRuntimeConfigError::NoConfiguredRuntime),
        [channel] => Ok(channel.clone()),
        _ => Err(RealtimeCallRuntimeConfigError::AmbiguousDefault { available }),
    }
}

pub fn configured_realtime_audio_call_runtime(
    channel: &str,
    channels_config: &ChannelsConfig,
) -> Result<Box<dyn RealtimeCallRuntimePort>, RealtimeCallRuntimeConfigError> {
    configured_realtime_audio_call_runtime_with_support_configs(
        channel,
        channels_config,
        None,
        None,
        None,
    )
}

pub fn configured_realtime_audio_call_runtime_with_synapseclaw_dir(
    channel: &str,
    channels_config: &ChannelsConfig,
    synapseclaw_dir: Option<PathBuf>,
) -> Result<Box<dyn RealtimeCallRuntimePort>, RealtimeCallRuntimeConfigError> {
    configured_realtime_audio_call_runtime_with_support_configs(
        channel,
        channels_config,
        synapseclaw_dir,
        None,
        None,
    )
}

pub fn configured_realtime_audio_call_runtime_with_support_configs(
    channel: &str,
    channels_config: &ChannelsConfig,
    synapseclaw_dir: Option<PathBuf>,
    tts_config: Option<TtsConfig>,
    transcription_config: Option<synapse_domain::config::schema::TranscriptionConfig>,
) -> Result<Box<dyn RealtimeCallRuntimePort>, RealtimeCallRuntimeConfigError> {
    let channel = normalize_realtime_call_channel(channel);
    ensure_realtime_audio_call_available(&channel, channels_config)?;
    match channel.as_str() {
        "clawdtalk" => {
            let Some(config) = channels_config.clawdtalk.clone() else {
                return Err(RealtimeCallRuntimeConfigError::MissingConfig {
                    channel,
                    config_key: "channels_config.clawdtalk",
                });
            };
            Ok(Box::new(ClawdTalkChannel::new_with_synapseclaw_dir(
                config,
                synapseclaw_dir,
            )))
        }
        #[cfg(feature = "channel-matrix")]
        "matrix" => {
            let Some(config) = channels_config.matrix.clone() else {
                return Err(RealtimeCallRuntimeConfigError::MissingConfig {
                    channel,
                    config_key: "channels_config.matrix",
                });
            };
            Ok(Box::new(
                MatrixChannel::from_call_runtime_config_with_support(
                    config,
                    synapseclaw_dir,
                    tts_config,
                    transcription_config,
                ),
            ))
        }
        _ => Err(RealtimeCallRuntimeConfigError::MissingRuntimeFactory { channel }),
    }
}

pub fn list_realtime_audio_call_sessions(
    channel: &str,
    channels_config: &ChannelsConfig,
) -> Result<Vec<RealtimeCallSessionSnapshot>, RealtimeCallRuntimeConfigError> {
    list_realtime_audio_call_sessions_with_synapseclaw_dir(channel, channels_config, None)
}

pub fn list_realtime_audio_call_sessions_with_synapseclaw_dir(
    channel: &str,
    channels_config: &ChannelsConfig,
    synapseclaw_dir: Option<PathBuf>,
) -> Result<Vec<RealtimeCallSessionSnapshot>, RealtimeCallRuntimeConfigError> {
    let channel = normalize_realtime_call_channel(channel);
    ensure_realtime_audio_call_inspectable(&channel, channels_config)?;
    if let Some(dir) = synapseclaw_dir.as_deref() {
        match load_persisted_transport_call_sessions(Some(dir), &channel) {
            Ok(sessions) => return Ok(sessions),
            Err(error) => {
                tracing::warn!(
                    channel = %channel,
                    error = %error,
                    "failed to load persisted realtime call sessions; falling back to process-local state"
                );
            }
        }
    }
    match channel.as_str() {
        "clawdtalk" => Ok(clawdtalk_recent_sessions()),
        #[cfg(feature = "channel-matrix")]
        "matrix" => Ok(matrix_recent_call_sessions()),
        _ => Err(RealtimeCallRuntimeConfigError::MissingRuntimeFactory { channel }),
    }
}

pub fn get_realtime_audio_call_session(
    channel: &str,
    call_control_id: &str,
    channels_config: &ChannelsConfig,
) -> Result<Option<RealtimeCallSessionSnapshot>, RealtimeCallRuntimeConfigError> {
    get_realtime_audio_call_session_with_synapseclaw_dir(
        channel,
        call_control_id,
        channels_config,
        None,
    )
}

pub fn get_realtime_audio_call_session_with_synapseclaw_dir(
    channel: &str,
    call_control_id: &str,
    channels_config: &ChannelsConfig,
    synapseclaw_dir: Option<PathBuf>,
) -> Result<Option<RealtimeCallSessionSnapshot>, RealtimeCallRuntimeConfigError> {
    let channel = normalize_realtime_call_channel(channel);
    ensure_realtime_audio_call_inspectable(&channel, channels_config)?;
    let call_control_id = non_empty_realtime_call_arg("call_control_id", call_control_id)?;
    if let Some(dir) = synapseclaw_dir.as_deref() {
        match load_persisted_transport_call_sessions(Some(dir), &channel) {
            Ok(sessions) => {
                return Ok(sessions
                    .into_iter()
                    .find(|session| session.call_control_id == call_control_id));
            }
            Err(error) => {
                tracing::warn!(
                    channel = %channel,
                    error = %error,
                    "failed to load persisted realtime call session; falling back to process-local state"
                );
            }
        }
    }
    match channel.as_str() {
        "clawdtalk" => Ok(clawdtalk_session(&call_control_id)),
        #[cfg(feature = "channel-matrix")]
        "matrix" => Ok(matrix_call_session(&call_control_id)),
        _ => Err(RealtimeCallRuntimeConfigError::MissingRuntimeFactory { channel }),
    }
}

pub fn get_realtime_audio_call_session_for_reply_target(
    channel: &str,
    reply_target: &str,
    channels_config: &ChannelsConfig,
) -> Result<Option<RealtimeCallSessionSnapshot>, RealtimeCallRuntimeConfigError> {
    let channel = normalize_realtime_call_channel(channel);
    ensure_realtime_audio_call_inspectable(&channel, channels_config)?;
    let reply_target = reply_target.trim();
    if reply_target.is_empty() {
        return Ok(None);
    }
    match channel.as_str() {
        "clawdtalk" => Ok(clawdtalk_call_session_for_reply_target(reply_target)),
        #[cfg(feature = "channel-matrix")]
        "matrix" => Ok(matrix_call_session_for_reply_target(reply_target)),
        _ => Err(RealtimeCallRuntimeConfigError::MissingRuntimeFactory { channel }),
    }
}

pub fn set_realtime_call_state_for_reply_target(
    channel: &str,
    reply_target: &str,
    state: RealtimeCallState,
) -> bool {
    match normalize_realtime_call_channel(channel).as_str() {
        "clawdtalk" => clawdtalk_set_call_state_for_reply_target(reply_target, state),
        #[cfg(feature = "channel-matrix")]
        "matrix" => matrix_set_call_state_for_reply_target(reply_target, state),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clawdtalk::ClawdTalkConfig;
    use crate::realtime_call_ledger::replace_persisted_transport_call_sessions;
    use synapse_domain::config::schema::ChannelsConfig;
    use synapse_domain::ports::realtime_call::{
        RealtimeCallDirection, RealtimeCallKind, RealtimeCallOrigin, RealtimeCallState,
    };

    fn persisted_session(channel: &str, call_control_id: &str) -> RealtimeCallSessionSnapshot {
        RealtimeCallSessionSnapshot {
            channel: channel.into(),
            kind: RealtimeCallKind::Audio,
            direction: RealtimeCallDirection::Outbound,
            origin: RealtimeCallOrigin::cli_request(),
            objective: Some("Persisted check".into()),
            call_control_id: call_control_id.into(),
            call_leg_id: None,
            call_session_id: None,
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
        }
    }

    #[test]
    fn planned_channels_are_not_executable() {
        let error = ensure_realtime_audio_call_available("telegram", &ChannelsConfig::default())
            .expect_err("telegram calls are planned, not available");
        assert!(error.to_string().contains("support=Planned"));
    }

    #[test]
    fn missing_clawdtalk_config_is_explicit() {
        let error =
            match configured_realtime_audio_call_runtime("clawdtalk", &ChannelsConfig::default()) {
                Ok(_) => panic!("clawdtalk config is required"),
                Err(error) => error,
            };
        assert!(error
            .to_string()
            .contains("channels_config.clawdtalk is not configured"));
    }

    #[test]
    fn resolve_runtime_channel_uses_single_configured_runtime() {
        let mut config = ChannelsConfig::default();
        config.clawdtalk = Some(Default::default());
        assert_eq!(
            resolve_realtime_audio_call_channel(None, &config).unwrap(),
            "clawdtalk"
        );
    }

    #[test]
    fn resolve_runtime_channel_uses_matrix_control_runtime_when_it_is_the_only_transport() {
        let mut config = ChannelsConfig::default();
        config.matrix = Some(synapse_domain::config::schema::MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: Some("tok".into()),
            user_id: None,
            device_id: None,
            room_id: "!room:matrix.example.com".into(),
            allowed_users: vec!["@user:matrix.example.com".into()],
            password: None,
            max_media_download_mb: None,
        });
        assert_eq!(
            resolve_realtime_audio_call_channel(None, &config).unwrap(),
            "matrix"
        );
        assert!(configured_realtime_audio_call_runtime("matrix", &config).is_ok());
    }

    #[test]
    fn resolve_runtime_channel_allows_explicit_matrix_selection_when_configured() {
        let mut config = ChannelsConfig::default();
        config.matrix = Some(synapse_domain::config::schema::MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: Some("tok".into()),
            user_id: None,
            device_id: None,
            room_id: "!room:matrix.example.com".into(),
            allowed_users: vec!["@user:matrix.example.com".into()],
            password: None,
            max_media_download_mb: None,
        });

        assert_eq!(
            resolve_realtime_audio_call_channel(Some("matrix"), &config).unwrap(),
            "matrix"
        );
    }

    #[test]
    fn resolve_runtime_channel_requires_runtime_when_nothing_is_configured() {
        let error = resolve_realtime_audio_call_channel(None, &ChannelsConfig::default())
            .expect_err("runtime selection should fail without configured transports");
        assert!(matches!(
            error,
            RealtimeCallRuntimeConfigError::NoConfiguredRuntime
        ));
    }

    #[test]
    fn status_report_includes_planned_and_available_call_transports() {
        let report = realtime_call_status_report(&ChannelsConfig::default());
        let clawdtalk = report
            .channels
            .iter()
            .find(|status| status.channel == "clawdtalk")
            .expect("clawdtalk status");
        assert_eq!(
            clawdtalk.audio_call_runtime,
            RealtimeCallRuntimeSupport::Available
        );
        assert!(!clawdtalk.media_attached);
        assert!(!clawdtalk.action_support.start);
        assert!(!clawdtalk.action_support.answer);
        assert!(!clawdtalk.action_support.speak);
        assert!(!clawdtalk.action_support.hangup);
        assert!(!clawdtalk.action_support.inspect);
        assert_eq!(
            clawdtalk.video_call_runtime,
            RealtimeCallRuntimeSupport::Planned
        );
        let matrix = report
            .channels
            .iter()
            .find(|status| status.channel == "matrix")
            .expect("matrix status");
        assert_eq!(
            matrix.audio_call_runtime,
            RealtimeCallRuntimeSupport::Available
        );
        assert!(!matrix.transport_configured);
        assert!(!matrix.media_attached);
        assert!(!matrix.action_support.start);
        assert!(!matrix.action_support.answer);
        assert!(!matrix.action_support.speak);
        assert!(!matrix.action_support.hangup);
        assert!(!matrix.action_support.inspect);
        assert_eq!(
            matrix.video_call_runtime,
            RealtimeCallRuntimeSupport::Planned
        );
    }

    #[test]
    fn status_report_marks_selected_runtime_when_single_runtime_is_configured() {
        let mut config = ChannelsConfig::default();
        config.clawdtalk = Some(ClawdTalkConfig {
            api_key: "test-key".into(),
            websocket_url: Some("https://clawdtalk.example".into()),
            api_base_url: None,
            assistant_id: None,
            connection_id: "conn-123".into(),
            from_number: "+15551234567".into(),
            allowed_destinations: vec![],
            webhook_secret: None,
            answering_machine_detection_mode: Some("detect".into()),
            speak_voice: "female".into(),
            speak_language: "en-US".into(),
            speak_service_level: "premium".into(),
            ai_voice: "alloy".into(),
            ai_speed: 1.0,
        });
        let report = realtime_call_status_report(&config);
        assert_eq!(report.default_channel.as_deref(), Some("clawdtalk"));
        let clawdtalk = report
            .channels
            .iter()
            .find(|status| status.channel == "clawdtalk")
            .expect("clawdtalk status");
        assert!(clawdtalk.transport_configured);
        assert!(!clawdtalk.media_attached);
        assert!(clawdtalk.action_support.answer);
        assert!(clawdtalk.runtime_selected_by_default);
        assert!(clawdtalk.runtime_ready);
        assert!(clawdtalk.health.is_some());
        assert!(matches!(
            clawdtalk.details,
            Some(RealtimeCallTransportDetails::ClawdTalk {
                bridge_ready: true,
                outbound_start_ready: true,
                call_control_ready: true,
                ..
            })
        ));
    }

    #[test]
    fn status_report_keeps_clawdtalk_not_ready_when_config_is_partial() {
        let mut config = ChannelsConfig::default();
        config.clawdtalk = Some(Default::default());

        let report = realtime_call_status_report(&config);
        let clawdtalk = report
            .channels
            .iter()
            .find(|status| status.channel == "clawdtalk")
            .expect("clawdtalk status");
        assert!(clawdtalk.transport_configured);
        assert!(!clawdtalk.runtime_ready);
        assert_eq!(
            clawdtalk.health.as_ref().map(|health| health.ready),
            Some(false)
        );
        assert!(matches!(
            clawdtalk.details,
            Some(RealtimeCallTransportDetails::ClawdTalk {
                bridge_ready: false,
                outbound_start_ready: false,
                call_control_ready: false,
                ..
            })
        ));
    }

    #[test]
    fn matrix_becomes_available_when_configured_for_runtime() {
        let mut config = ChannelsConfig::default();
        config.matrix = Some(synapse_domain::config::schema::MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: Some("tok".into()),
            user_id: None,
            device_id: None,
            room_id: "!room:matrix.example.com".into(),
            allowed_users: vec!["@user:matrix.example.com".into()],
            password: None,
            max_media_download_mb: None,
        });

        let report = realtime_call_status_report(&config);
        let matrix = report
            .channels
            .iter()
            .find(|status| status.channel == "matrix")
            .expect("matrix status");
        assert_eq!(
            matrix.audio_call_runtime,
            RealtimeCallRuntimeSupport::Available
        );
        assert!(!matrix.media_attached);
        assert!(matrix.action_support.start);
        assert!(matrix.action_support.answer);
        assert!(matrix.action_support.speak);
        assert!(matrix.action_support.hangup);
        assert!(matrix.action_support.inspect);
        assert!(matches!(
            matrix.details,
            Some(RealtimeCallTransportDetails::Matrix {
                auth_mode: MatrixStatusAuthMode::AccessToken,
                auth_source: MatrixStatusAuthSource::Unknown,
                widget_support_enabled: true,
                room_reference: Some(_),
                ..
            })
        ));
        assert_eq!(
            resolve_realtime_audio_call_inspection_channel(None, &config).unwrap(),
            "matrix"
        );
        assert!(list_realtime_audio_call_sessions("matrix", &config).is_ok());
    }

    #[test]
    fn reply_target_state_dispatch_is_channel_specific() {
        assert!(!set_realtime_call_state_for_reply_target(
            "matrix",
            "matrix-call:anything",
            RealtimeCallState::Thinking,
        ));
    }

    #[test]
    fn inspection_uses_persisted_sessions_when_synapseclaw_dir_is_provided() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = ChannelsConfig::default();
        config.matrix = Some(synapse_domain::config::schema::MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: Some("tok".into()),
            user_id: None,
            device_id: None,
            room_id: "!room:matrix.example.com".into(),
            allowed_users: vec!["@user:matrix.example.com".into()],
            password: None,
            max_media_download_mb: None,
        });

        replace_persisted_transport_call_sessions(
            Some(dir.path()),
            "matrix",
            &[persisted_session("matrix", "$call-1")],
        )
        .unwrap();

        let sessions = list_realtime_audio_call_sessions_with_synapseclaw_dir(
            "matrix",
            &config,
            Some(dir.path().to_path_buf()),
        )
        .unwrap();
        let session = get_realtime_audio_call_session_with_synapseclaw_dir(
            "matrix",
            "$call-1",
            &config,
            Some(dir.path().to_path_buf()),
        )
        .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].call_control_id, "$call-1");
        assert_eq!(
            session.as_ref().map(|call| call.call_control_id.as_str()),
            Some("$call-1")
        );
    }

    #[test]
    fn current_conversation_target_prefers_matrix_user_id() {
        assert_eq!(
            resolve_current_conversation_realtime_call_target(
                "matrix",
                "@victor:matrix.example.com",
                "@victor:matrix.example.com||!room:matrix.example.com",
            )
            .as_deref(),
            Some("@victor:matrix.example.com")
        );
    }

    #[test]
    fn current_conversation_target_falls_back_to_reply_ref_for_non_matrix() {
        assert_eq!(
            resolve_current_conversation_realtime_call_target(
                "clawdtalk",
                "+15550001111",
                "+15550002222",
            )
            .as_deref(),
            Some("+15550002222")
        );
    }
}
