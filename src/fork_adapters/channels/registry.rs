//! Cached channel registry — long-lived adapter instances for outbound delivery.
//!
//! Implements `ChannelRegistryPort` by caching channel adapters built via
//! `build_channel_by_id`.  Stateful channels (Matrix) keep their authenticated
//! SDK client alive across deliveries instead of re-initialising per message.

use crate::channels::{build_channel_by_id, Channel, SendMessage};
use crate::config::Config;
use crate::fork_core::domain::channel::{ChannelCapability, DegradationPolicy, OutboundIntent};
use crate::fork_core::ports::channel_registry::ChannelRegistryPort;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Concrete `ChannelRegistryPort` that caches adapters for the daemon lifetime.
pub struct CachedChannelRegistry {
    config: Config,
    cache: RwLock<HashMap<String, Arc<dyn Channel>>>,
}

impl CachedChannelRegistry {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            cache: RwLock::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    fn inject(&self, name: &str, ch: Arc<dyn Channel>) {
        self.cache.write().insert(name.to_string(), ch);
    }
}

#[async_trait]
impl ChannelRegistryPort for CachedChannelRegistry {
    fn resolve(&self, channel_name: &str) -> anyhow::Result<Arc<dyn Channel>> {
        // Fast path: read lock
        {
            let cache = self.cache.read();
            if let Some(ch) = cache.get(channel_name) {
                return Ok(Arc::clone(ch));
            }
        }
        // Slow path: build adapter, cache under write lock
        let ch = build_channel_by_id(&self.config, channel_name)?;
        let mut cache = self.cache.write();
        // Double-check: another thread may have inserted while we upgraded
        let entry = cache
            .entry(channel_name.to_string())
            .or_insert_with(|| Arc::clone(&ch));
        Ok(Arc::clone(entry))
    }

    fn capabilities(&self, channel_name: &str) -> Vec<ChannelCapability> {
        // Hardcoded per channel for now.  Phase 4.1 moves this to adapters.
        match channel_name {
            "telegram" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::RichFormatting,
                ChannelCapability::EditMessage,
            ],
            "discord" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
                ChannelCapability::EditMessage,
            ],
            "slack" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
            ],
            #[cfg(feature = "channel-matrix")]
            "matrix" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
            ],
            _ => vec![],
        }
    }

    async fn deliver(&self, intent: &OutboundIntent) -> anyhow::Result<()> {
        let text = intent.content.as_text();
        if text.is_empty() {
            tracing::debug!(
                channel = %intent.target_channel,
                "ChannelRegistry: skipping empty intent"
            );
            return Ok(());
        }

        let channel = self.resolve(&intent.target_channel)?;

        // Capability check
        let available = self.capabilities(&intent.target_channel);
        for required in &intent.required_capabilities {
            if !available.contains(required) {
                match intent.degradation_policy {
                    DegradationPolicy::Drop => {
                        tracing::info!(
                            channel = %intent.target_channel,
                            missing = ?required,
                            "ChannelRegistry: dropping intent (missing capability)"
                        );
                        return Ok(());
                    }
                    DegradationPolicy::PlainText => {
                        // Continue — send as plain text
                    }
                }
            }
        }

        let msg = SendMessage::new(text, intent.target_recipient.as_str())
            .in_thread(intent.thread_ref.clone());
        channel.send(&msg).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::traits::ChannelMessage;

    /// Minimal test channel that records sends.
    struct MockChannel {
        name: String,
        sent: std::sync::Mutex<Vec<String>>,
    }

    impl MockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                sent: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn sent_messages(&self) -> Vec<String> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
            self.sent.lock().unwrap().push(message.content.clone());
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn resolve_unknown_channel_returns_error() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        assert!(registry.resolve("nonexistent").is_err());
    }

    #[test]
    fn capabilities_known_channels() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        let tg = registry.capabilities("telegram");
        assert!(tg.contains(&ChannelCapability::SendText));
        assert!(tg.contains(&ChannelCapability::RichFormatting));

        #[cfg(feature = "channel-matrix")]
        {
            let mx = registry.capabilities("matrix");
            assert!(mx.contains(&ChannelCapability::SendText));
            assert!(mx.contains(&ChannelCapability::Threads));
        }

        assert!(registry.capabilities("unknown").is_empty());
    }

    #[test]
    fn cache_reuse_returns_same_arc() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let a = registry.resolve("test").unwrap();
        let b = registry.resolve("test").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn deliver_sends_through_mock_channel() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let intent = OutboundIntent::notify("test", "recipient-1", "hello world".into());
        registry.deliver(&intent).await.unwrap();

        assert_eq!(mock.sent_messages(), vec!["hello world"]);
    }

    #[tokio::test]
    async fn deliver_skips_empty_content() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let intent = OutboundIntent::notify("test", "recipient-1", String::new());
        registry.deliver(&intent).await.unwrap();

        assert!(mock.sent_messages().is_empty());
    }

    #[tokio::test]
    async fn deliver_drops_on_missing_capability_with_drop_policy() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let mut intent = OutboundIntent::notify("test", "r", "msg".into());
        // Require Attachments, which "test" doesn't have
        intent.required_capabilities = vec![ChannelCapability::Attachments];
        intent.degradation_policy = DegradationPolicy::Drop;

        registry.deliver(&intent).await.unwrap();
        assert!(mock.sent_messages().is_empty());
    }

    #[tokio::test]
    async fn deliver_continues_on_missing_capability_with_plaintext_policy() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(config);
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let mut intent = OutboundIntent::notify("test", "r", "msg".into());
        intent.required_capabilities = vec![ChannelCapability::Attachments];
        intent.degradation_policy = DegradationPolicy::PlainText;

        registry.deliver(&intent).await.unwrap();
        assert_eq!(mock.sent_messages(), vec!["msg"]);
    }
}
