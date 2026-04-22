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

    /// Declared capabilities for a channel plus transport identity.
    fn capability_profile(&self, channel_name: &str) -> ChannelCapabilityProfile {
        ChannelCapabilityProfile::new(
            channel_name.trim().to_ascii_lowercase(),
            self.capabilities(channel_name),
        )
    }

    /// Declared profiles for channels known to this registry.
    fn capability_profiles(&self) -> Vec<ChannelCapabilityProfile>;

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

#[derive(Debug, Clone)]
pub struct ChannelCapabilityProfile {
    pub channel: String,
    pub capabilities: Vec<ChannelCapability>,
    pub planned_capabilities: Vec<ChannelCapability>,
}

impl ChannelCapabilityProfile {
    pub fn new(channel: impl Into<String>, capabilities: Vec<ChannelCapability>) -> Self {
        Self {
            channel: channel.into(),
            capabilities,
            planned_capabilities: Vec::new(),
        }
    }

    pub fn with_planned_capabilities(
        mut self,
        planned_capabilities: Vec<ChannelCapability>,
    ) -> Self {
        self.planned_capabilities = planned_capabilities;
        self
    }

    pub fn has(&self, capability: ChannelCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn plans(&self, capability: ChannelCapability) -> bool {
        self.planned_capabilities.contains(&capability)
    }
}
