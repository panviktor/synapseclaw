//! Channel adapters — messaging platform implementations.
//!
//! Each channel implements `synapse_domain::ports::channel::Channel`.
//! This crate contains the platform-specific logic; the orchestration
//! (start_channels, message dispatch loop) lives in `synapse_adapters` core.

// Channel implementations
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
pub mod outbound_media;
pub mod qq;
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
pub use registry::CachedChannelRegistry;

// Re-export channel implementations for direct use
pub use bluesky::BlueskyChannel;
pub use clawdtalk::ClawdTalkChannel;
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
