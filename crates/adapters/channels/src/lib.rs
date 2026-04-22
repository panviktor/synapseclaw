//! Channel adapters — messaging platform implementations.
//!
//! Each channel implements `synapse_domain::ports::channel::Channel`.
//! This crate contains the platform-specific logic; the orchestration
//! (start_channels, message dispatch loop) lives in `synapse_adapters` core.

// Channel implementations
pub mod bluesky;
pub mod capabilities;
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
pub mod outbound_media;
pub mod qq;
pub mod realtime_audio_ingress;
pub mod realtime_call_ledger;
pub mod realtime_calls;
pub mod realtime_turn_engine;
pub mod reddit;
pub mod registry;
pub mod session_backend;
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

// Inbound adapters (co-located with channels)
pub mod inbound;

// Re-export key types for convenience
pub use capabilities::{
    declared_channel_capabilities, declared_channel_capability_profile,
    declared_channel_capability_profiles, planned_channel_capabilities,
};
pub use realtime_calls::{
    configured_realtime_audio_call_channels, configured_realtime_audio_call_inspection_channels,
    configured_realtime_audio_call_runtime,
    configured_realtime_audio_call_runtime_with_support_configs,
    configured_realtime_audio_call_runtime_with_synapseclaw_dir,
    ensure_realtime_audio_call_available, get_realtime_audio_call_session,
    get_realtime_audio_call_session_for_reply_target,
    get_realtime_audio_call_session_with_synapseclaw_dir, list_realtime_audio_call_sessions,
    list_realtime_audio_call_sessions_with_synapseclaw_dir, non_empty_realtime_call_arg,
    normalize_realtime_call_channel, realtime_call_status_report, realtime_call_status_report_live,
    realtime_call_status_report_live_with_synapseclaw_dir, require_realtime_call_confirmation,
    resolve_current_conversation_realtime_call_target, resolve_realtime_audio_call_channel,
    resolve_realtime_audio_call_inspection_channel, set_realtime_call_state_for_reply_target,
    RealtimeCallRuntimeHealth, RealtimeCallRuntimeSupport, RealtimeCallStatusReport,
    RealtimeCallTransportDetails, RealtimeCallTransportStatus,
};
pub use registry::CachedChannelRegistry;

// Re-export channel implementations for direct use
pub use bluesky::BlueskyChannel;
pub use clawdtalk::{
    clawdtalk_bridge_status, clawdtalk_call_session_for_reply_target, clawdtalk_recent_sessions,
    clawdtalk_session, clawdtalk_set_call_state_for_reply_target, ClawdTalkBridgeStatus,
    ClawdTalkChannel,
};
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
pub use matrix::{
    matrix_call_control_status, matrix_call_session, matrix_call_session_for_reply_target,
    matrix_recent_call_sessions, MatrixCallControlStatus, MatrixChannel,
};
pub use mattermost::MattermostChannel;
pub use mochat::MochatChannel;
pub use nextcloud_talk::NextcloudTalkChannel;
#[cfg(feature = "channel-nostr")]
pub use nostr::NostrChannel;
pub use notion::NotionChannel;
pub use qq::QQChannel;
pub use reddit::RedditChannel;
pub use session_backend::SessionBackend;
pub use signal::SignalChannel;
pub use slack::SlackChannel;
pub use telegram::TelegramChannel;
pub use traits::{Channel, ChannelMessage, SendMessage};
#[allow(unused_imports)]
pub use tts::{TtsManager, TtsProvider};
pub use twitter::TwitterChannel;
pub use wati::WatiChannel;
pub use webhook::WebhookChannel;
pub use wecom::WeComChannel;
pub use whatsapp::WhatsAppChannel;
#[cfg(feature = "whatsapp-web")]
pub use whatsapp_web::WhatsAppWebChannel;
