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

// ── Core modules (remain in synapse_adapters) ──
pub mod agent;
pub(crate) mod agent_runtime_factory;
pub(crate) mod channel_runtime_support;
pub mod channels;
pub mod commands;
pub mod cost;
pub mod daemon;
pub mod doctor;
pub mod gateway;
pub mod health;
pub mod heartbeat;
pub mod hooks;
pub(crate) mod inbound_runtime_config;
pub(crate) mod inbound_runtime_ports;
pub(crate) mod inbound_runtime_summary;
pub mod integrations;
pub mod ipc;
pub mod memory_adapters;
pub(crate) mod message_routing_service;
pub mod middleware;
pub mod pipeline;
pub mod routing;
pub mod runtime;
pub(crate) mod runtime_adapter_contract;
pub(crate) mod runtime_history_hygiene;
pub mod runtime_routes;
pub mod runtime_system_prompt;
pub(crate) mod runtime_tool_notifications;
pub(crate) mod runtime_tool_observer;
pub mod scoped_instruction_context;
pub mod service;
pub mod skills;
pub mod storage;
pub mod tools;
pub mod tunnel;

// ── ChatMessage ────────────────────────────────────────────────────────────
//
// `synapse_providers::ChatMessage` is now a re-export of `synapse_domain::domain::message::ChatMessage`.
// No conversion helpers needed — the types are identical.

/// Build an `InboundEnvelope` from an upstream `ChannelMessage`.
///
/// This conversion lived in synapse_domain before the workspace crate extraction.
/// Now it's adapter logic — synapse_domain must not depend on upstream channel types.
pub(crate) fn envelope_from_channel_message(
    msg: &crate::channels::traits::ChannelMessage,
) -> synapse_domain::domain::channel::InboundEnvelope {
    use synapse_domain::domain::channel::{InboundEnvelope, SourceKind};
    let mut media_attachments = msg.media_attachments.clone();
    for attachment in inbound_media_attachments_from_content(&msg.content) {
        if !media_attachments
            .iter()
            .any(|existing| existing.kind == attachment.kind && existing.uri == attachment.uri)
        {
            media_attachments.push(attachment);
        }
    }
    InboundEnvelope {
        source_kind: SourceKind::Channel,
        source_adapter: msg.channel.clone(),
        actor_id: msg.sender.clone(),
        conversation_id: msg.reply_target.clone(),
        event_ref: Some(msg.id.clone()),
        reply_ref: msg.reply_target.clone(),
        thread_ref: msg.thread_ts.clone(),
        media_attachments,
        content: msg.content.clone(),
        received_at: msg.timestamp,
    }
}

fn inbound_media_attachments_from_content(
    content: &str,
) -> Vec<synapse_domain::domain::channel::InboundMediaAttachment> {
    use std::collections::HashSet;
    use synapse_domain::domain::channel::{InboundMediaAttachment, InboundMediaKind};

    let mut seen = HashSet::new();
    let mut attachments = Vec::new();
    for (label, kind) in [
        ("IMAGE", InboundMediaKind::Image),
        ("AUDIO", InboundMediaKind::Audio),
        ("MUSIC", InboundMediaKind::Audio),
        ("VIDEO", InboundMediaKind::Video),
        ("FILE", InboundMediaKind::File),
    ] {
        let prefix = format!("[{label}:");
        let mut rest = content;
        while let Some(start) = rest.find(&prefix) {
            let after_prefix = &rest[start + prefix.len()..];
            let Some(end) = after_prefix.find(']') else {
                break;
            };
            let uri = after_prefix[..end].trim();
            if !uri.is_empty() && seen.insert((label, uri.to_string())) {
                attachments.push(InboundMediaAttachment::new(kind, uri));
            }
            rest = &after_prefix[end + 1..];
        }
    }
    for line in content.lines() {
        let trimmed = line.trim();
        for prefix in ["[Document:", "[Attachment:", "[File:"] {
            if !trimmed.starts_with(prefix) {
                continue;
            }
            let Some(end) = trimmed.find(']') else {
                continue;
            };
            let uri = trimmed[end + 1..].trim();
            if uri.is_empty() || uri.contains('\n') {
                continue;
            }
            if seen.insert(("FILE", uri.to_string())) {
                attachments.push(InboundMediaAttachment::new(InboundMediaKind::File, uri));
            }
        }
    }
    attachments
}
