//! Fork-owned adapters — infrastructure implementations of `fork_core` ports.
//!
//! Design rule: `fork_core` owns *what* happens; `fork_adapters` owns *how*.

// ── Port implementations (fork_core ports → concrete adapters) ──
pub mod channels;
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

// ── ChatMessage conversion helpers ──────────────────────────────────────────
//
// The upstream `providers::ChatMessage` and fork_core's
// `domain::message::ChatMessage` share the same shape (role + content).
// These helpers live here so every adapter can use them without duplication.

/// Convert an upstream `providers::ChatMessage` to a `fork_core` `ChatMessage`.
pub(crate) fn to_core_message(
    msg: &crate::fork_adapters::providers::ChatMessage,
) -> crate::fork_core::domain::message::ChatMessage {
    crate::fork_core::domain::message::ChatMessage {
        role: msg.role.clone(),
        content: msg.content.clone(),
    }
}

/// Convert a `fork_core` `ChatMessage` to an upstream `providers::ChatMessage`.
pub(crate) fn from_core_message(
    msg: &crate::fork_core::domain::message::ChatMessage,
) -> crate::fork_adapters::providers::ChatMessage {
    crate::fork_adapters::providers::ChatMessage {
        role: msg.role.clone(),
        content: msg.content.clone(),
    }
}

/// Build an `InboundEnvelope` from an upstream `ChannelMessage`.
///
/// This conversion lived in fork_core before the workspace crate extraction.
/// Now it's adapter logic — fork_core must not depend on upstream channel types.
pub(crate) fn envelope_from_channel_message(
    msg: &crate::fork_adapters::channels::traits::ChannelMessage,
) -> crate::fork_core::domain::channel::InboundEnvelope {
    use crate::fork_core::domain::channel::{InboundEnvelope, SourceKind};
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
