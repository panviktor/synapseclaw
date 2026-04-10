//! Port: channel adapter registry with long-lived cached instances.
//!
//! The core emits `OutboundIntent`; the registry resolves a cached channel
//! adapter, checks capabilities, applies degradation policy, and delivers.

use crate::domain::channel::{ChannelCapability, OutboundIntent};
use crate::domain::conversation_target::ConversationDeliveryTarget;
use async_trait::async_trait;

/// Port for resolving and delivering through channel adapters.
///
/// Implementations cache adapter instances so that stateful channels
/// (Matrix) reuse their authenticated SDK client across deliveries.
#[async_trait]
pub trait ChannelRegistryPort: Send + Sync {
    /// Check if a channel is available in the registry.
    fn has_channel(&self, channel_name: &str) -> bool;

    /// Declared capabilities for a channel (without building the adapter).
    fn capabilities(&self, channel_name: &str) -> Vec<ChannelCapability>;

    /// Resolve adapter, check required capabilities, apply degradation
    /// policy, and deliver the intent via `channel.send()`.
    async fn deliver(&self, intent: &OutboundIntent) -> anyhow::Result<()>;

    /// Per-channel formatting instructions for the system prompt.
    ///
    /// Returns transport-specific rendering hints (Telegram HTML, Matrix
    /// CommonMark, Discord Markdown, etc.).  The core asks "how should I
    /// format?" and the adapter returns the instructions.
    fn delivery_hints(&self, channel_name: &str) -> Option<String> {
        let _ = channel_name;
        None
    }

    /// Return a single explicit proactive delivery target when runtime config
    /// exposes one unambiguous destination. This is used for turn-level
    /// defaults and should avoid heuristics based on workspace files.
    fn configured_delivery_target(&self) -> Option<ConversationDeliveryTarget> {
        None
    }
}
