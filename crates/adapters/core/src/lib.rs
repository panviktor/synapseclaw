#![allow(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::field_reassign_with_default,
    clippy::map_unwrap_or,
    clippy::new_without_default,
    clippy::unnecessary_cast,
    clippy::unnecessary_map_or,
    clippy::should_implement_trait
)]
//! Infrastructure adapters — implementations of `synapse_domain` ports.
//!
//! Design rule: `synapse_domain` owns *what* happens; `synapse_adapters` owns *how*.

// ── Port implementations (synapse_domain ports → concrete adapters) ──
pub mod channels;
pub mod commands;
pub mod inbound;
pub mod ipc;
pub mod memory_adapters;
pub mod middleware;
pub mod pipeline;
pub mod routing;
pub mod runtime;
pub mod storage;

// ── Infrastructure adapters (moved from src/ top-level) ──
pub use synapse_infra::approval;
pub use synapse_providers::auth;
pub mod cost;
pub use synapse_cron as cron;
pub mod daemon;
pub mod doctor;
pub mod gateway;
pub mod health;
pub mod heartbeat;
pub mod hooks;
pub mod integrations;
pub use synapse_observability as observability;
pub mod onboard;
pub use synapse_providers as providers;
pub use synapse_providers::proxy;
pub mod service;
pub mod tools;
pub mod tunnel;
pub mod workspace;
pub mod workspace_io;

// ── Deferred modules (moved from src/ in Phase 4.1H2) ──
pub use synapse_infra::identity;
pub use synapse_providers::multimodal;
pub mod skills;

// ── ChatMessage ────────────────────────────────────────────────────────────
//
// `providers::ChatMessage` is now a re-export of `synapse_domain::domain::message::ChatMessage`.
// No conversion helpers needed — the types are identical.

/// Build an `InboundEnvelope` from an upstream `ChannelMessage`.
///
/// This conversion lived in synapse_domain before the workspace crate extraction.
/// Now it's adapter logic — synapse_domain must not depend on upstream channel types.
pub(crate) fn envelope_from_channel_message(
    msg: &crate::channels::traits::ChannelMessage,
) -> synapse_domain::domain::channel::InboundEnvelope {
    use synapse_domain::domain::channel::{InboundEnvelope, SourceKind};
    InboundEnvelope {
        source_kind: SourceKind::Channel,
        source_adapter: msg.channel.clone(),
        actor_id: msg.sender.clone(),
        conversation_ref: if let Some(ref thread) = msg.thread_ts {
            format!("{}_{}_{}", msg.channel, thread, msg.sender)
        } else {
            format!("{}_{}", msg.channel, msg.sender)
        },
        reply_ref: msg.reply_target.clone(),
        thread_ref: msg.thread_ts.clone(),
        content: msg.content.clone(),
        received_at: msg.timestamp,
    }
}
pub mod agent;
pub use synapse_infra::config_io;
