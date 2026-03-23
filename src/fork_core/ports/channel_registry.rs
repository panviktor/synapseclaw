//! Port: channel adapter registry with long-lived cached instances.
//!
//! The core emits `OutboundIntent`; the registry resolves a cached channel
//! adapter, checks capabilities, applies degradation policy, and delivers.

use crate::channels::Channel;
use crate::fork_core::domain::channel::{ChannelCapability, OutboundIntent};
use async_trait::async_trait;
use std::sync::Arc;

/// Port for resolving and delivering through channel adapters.
///
/// Implementations cache adapter instances so that stateful channels
/// (Matrix) reuse their authenticated SDK client across deliveries.
#[async_trait]
pub trait ChannelRegistryPort: Send + Sync {
    /// Resolve a cached channel adapter by config section name
    /// (e.g. "telegram", "matrix").
    fn resolve(&self, channel_name: &str) -> anyhow::Result<Arc<dyn Channel>>;

    /// Declared capabilities for a channel (without building the adapter).
    fn capabilities(&self, channel_name: &str) -> Vec<ChannelCapability>;

    /// Resolve adapter, check required capabilities, apply degradation
    /// policy, and deliver the intent via `channel.send()`.
    async fn deliver(&self, intent: &OutboundIntent) -> anyhow::Result<()>;
}
