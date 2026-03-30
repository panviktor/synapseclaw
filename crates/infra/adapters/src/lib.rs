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
pub mod memory;
pub mod middleware;
pub mod pipeline;
pub mod routing;
pub mod runtime;
pub mod storage;

// ── Infrastructure adapters (moved from src/ top-level) ──
pub mod approval;
pub mod auth;
pub mod cost;
pub mod cron;
pub mod daemon;
pub mod doctor;
pub mod gateway;
pub mod health;
pub mod heartbeat;
pub mod hooks;
pub mod integrations;
pub mod observability;
pub mod onboard;
pub mod providers;
pub mod service;
pub mod tools;
pub mod tunnel;
pub mod workspace;
pub mod workspace_io;

// ── Deferred modules (moved from src/ in Phase 4.1H2) ──
pub mod identity;
pub mod multimodal;
pub mod skills;

// ── ChatMessage conversion helpers ──────────────────────────────────────────
//
// The upstream `providers::ChatMessage` and synapse_domain's
// `domain::message::ChatMessage` share the same shape (role + content).
// These helpers live here so every adapter can use them without duplication.

/// Convert an upstream `providers::ChatMessage` to a `synapse_domain` `ChatMessage`.
pub(crate) fn to_core_message(
    msg: &crate::providers::ChatMessage,
) -> synapse_domain::domain::message::ChatMessage {
    synapse_domain::domain::message::ChatMessage {
        role: msg.role.clone(),
        content: msg.content.clone(),
    }
}

/// Convert a `synapse_domain` `ChatMessage` to an upstream `providers::ChatMessage`.
pub(crate) fn from_core_message(
    msg: &synapse_domain::domain::message::ChatMessage,
) -> crate::providers::ChatMessage {
    crate::providers::ChatMessage {
        role: msg.role.clone(),
        content: msg.content.clone(),
    }
}

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
pub mod config_io;
