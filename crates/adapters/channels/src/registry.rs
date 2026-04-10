//! Cached channel registry — long-lived adapter instances for outbound delivery.
//!
//! Implements `ChannelRegistryPort` by caching channel adapters built via
//! `build_channel_by_id`.  Stateful channels (Matrix) keep their authenticated
//! SDK client alive across deliveries instead of re-initialising per message.

use crate::{Channel, SendMessage};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use synapse_domain::config::schema::Config;
use synapse_domain::domain::channel::{ChannelCapability, DegradationPolicy, OutboundIntent};
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::ports::channel_registry::ChannelRegistryPort;

/// Builder function type for creating channels from config.
pub type ChannelBuilderFn = dyn Fn(&Config, &str) -> anyhow::Result<Arc<dyn Channel>> + Send + Sync;

/// Concrete `ChannelRegistryPort` that caches adapters for the daemon lifetime.
pub struct CachedChannelRegistry {
    config: Config,
    cache: RwLock<HashMap<String, Arc<dyn Channel>>>,
    builder: Arc<ChannelBuilderFn>,
}

impl CachedChannelRegistry {
    fn push_target(
        targets: &mut Vec<ConversationDeliveryTarget>,
        target: ConversationDeliveryTarget,
    ) {
        if !targets.iter().any(|existing| existing == &target) {
            targets.push(target);
        }
    }

    pub fn new(config: Config, builder: Arc<ChannelBuilderFn>) -> Self {
        Self {
            config,
            cache: RwLock::new(HashMap::new()),
            builder,
        }
    }

    #[cfg(test)]
    fn inject(&self, name: &str, ch: Arc<dyn Channel>) {
        self.cache.write().insert(name.to_string(), ch);
    }

    /// Resolve a channel adapter by name, building and caching if needed.
    ///
    /// This is an inherent method (not part of the `ChannelRegistryPort` trait)
    /// so that callers with a concrete `CachedChannelRegistry` can still
    /// obtain the underlying `Arc<dyn Channel>` for direct use.
    pub fn resolve(&self, channel_name: &str) -> anyhow::Result<Arc<dyn Channel>> {
        // Fast path: read lock
        {
            let cache = self.cache.read();
            if let Some(ch) = cache.get(channel_name) {
                return Ok(Arc::clone(ch));
            }
        }
        // Slow path: build adapter, cache under write lock
        let ch = (self.builder)(&self.config, channel_name)?;
        let mut cache = self.cache.write();
        // Double-check: another thread may have inserted while we upgraded
        let entry = cache
            .entry(channel_name.to_string())
            .or_insert_with(|| Arc::clone(&ch));
        Ok(Arc::clone(entry))
    }

    /// Per-channel formatting instructions for the system prompt.
    ///
    /// This is adapter metadata — the core asks "how should I format?" and the
    /// adapter returns transport-specific instructions.  New channels just add
    /// a match arm here.
    pub fn delivery_hints_impl(&self, channel_name: &str) -> Option<String> {
        match channel_name {
            "telegram" => Some(
                "Format replies using Telegram HTML (bold=<b>, italic=<i>, \
                 code=<code>, pre=<pre>). Keep messages under 4096 characters. \
                 Use concise formatting."
                    .to_string(),
            ),
            "matrix" => Some(
                "Format replies using Markdown (bold=**, italic=*, \
                 code=```, headings=#). Matrix supports full CommonMark."
                    .to_string(),
            ),
            "discord" => Some(
                "Format replies using Discord Markdown (bold=**, italic=*, \
                 code=```, spoiler=||). Keep messages under 2000 characters."
                    .to_string(),
            ),
            "slack" => Some(
                "Format replies using Slack mrkdwn (bold=*, italic=_, \
                 code=```, link=<url|text>). Keep messages under 4000 characters."
                    .to_string(),
            ),
            "mattermost" => {
                Some("Format replies using Markdown (bold=**, italic=*, code=```).".to_string())
            }
            _ => None,
        }
    }

    fn configured_delivery_target_impl(&self) -> Option<ConversationDeliveryTarget> {
        let mut targets = Vec::new();

        let heartbeat_channel = self
            .config
            .heartbeat
            .target
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let heartbeat_recipient = self
            .config
            .heartbeat
            .to
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let (Some(channel), Some(recipient)) = (heartbeat_channel, heartbeat_recipient) {
            Self::push_target(
                &mut targets,
                ConversationDeliveryTarget::Explicit {
                    channel: channel.to_ascii_lowercase(),
                    recipient: recipient.to_string(),
                    thread_ref: None,
                },
            );
        }

        #[cfg(feature = "channel-matrix")]
        if let Some(matrix) = &self.config.channels_config.matrix {
            let room_id = matrix.room_id.trim();
            if !room_id.is_empty() {
                Self::push_target(
                    &mut targets,
                    ConversationDeliveryTarget::Explicit {
                        channel: "matrix".into(),
                        recipient: room_id.to_string(),
                        thread_ref: None,
                    },
                );
            }
        }

        if let Some(slack) = &self.config.channels_config.slack {
            if let Some(channel_id) = slack.channel_id.as_deref().map(str::trim) {
                if !channel_id.is_empty() && channel_id != "*" {
                    Self::push_target(
                        &mut targets,
                        ConversationDeliveryTarget::Explicit {
                            channel: "slack".into(),
                            recipient: channel_id.to_string(),
                            thread_ref: None,
                        },
                    );
                }
            }
        }

        if let Some(mattermost) = &self.config.channels_config.mattermost {
            if let Some(channel_id) = mattermost.channel_id.as_deref().map(str::trim) {
                if !channel_id.is_empty() {
                    Self::push_target(
                        &mut targets,
                        ConversationDeliveryTarget::Explicit {
                            channel: "mattermost".into(),
                            recipient: channel_id.to_string(),
                            thread_ref: None,
                        },
                    );
                }
            }
        }

        if let Some(signal) = &self.config.channels_config.signal {
            if let Some(group_id) = signal.group_id.as_deref().map(str::trim) {
                if !group_id.is_empty() && !group_id.eq_ignore_ascii_case("dm") {
                    Self::push_target(
                        &mut targets,
                        ConversationDeliveryTarget::Explicit {
                            channel: "signal".into(),
                            recipient: group_id.to_string(),
                            thread_ref: None,
                        },
                    );
                }
            }
        }

        if let Some(irc) = &self.config.channels_config.irc {
            if irc.channels.len() == 1 {
                let channel = irc.channels[0].trim();
                if !channel.is_empty() {
                    Self::push_target(
                        &mut targets,
                        ConversationDeliveryTarget::Explicit {
                            channel: "irc".into(),
                            recipient: channel.to_string(),
                            thread_ref: None,
                        },
                    );
                }
            }
        }

        match targets.len() {
            1 => targets.into_iter().next(),
            _ => None,
        }
    }
}

#[async_trait]
impl ChannelRegistryPort for CachedChannelRegistry {
    fn has_channel(&self, channel_name: &str) -> bool {
        // Check cache first, then try to build (which validates config).
        {
            let cache = self.cache.read();
            if cache.contains_key(channel_name) {
                return true;
            }
        }
        // Attempt to resolve — success means the channel is available.
        self.resolve(channel_name).is_ok()
    }

    fn capabilities(&self, channel_name: &str) -> Vec<ChannelCapability> {
        // Hardcoded per channel for now.  Phase 4.1 moves this to adapters.
        match channel_name {
            "telegram" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::RichFormatting,
                ChannelCapability::EditMessage,
                ChannelCapability::RuntimeCommands,
                ChannelCapability::InterruptOnNewMessage,
            ],
            "discord" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
                ChannelCapability::EditMessage,
                ChannelCapability::RuntimeCommands,
                ChannelCapability::ToolContextDisplay,
            ],
            "slack" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
                ChannelCapability::InterruptOnNewMessage,
                ChannelCapability::ToolContextDisplay,
            ],
            #[cfg(feature = "channel-matrix")]
            "matrix" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
                ChannelCapability::RuntimeCommands,
                ChannelCapability::ToolContextDisplay,
            ],
            "mattermost" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Threads,
                ChannelCapability::Reactions,
                ChannelCapability::RichFormatting,
                ChannelCapability::ToolContextDisplay,
            ],
            "signal" => vec![
                ChannelCapability::SendText,
                ChannelCapability::ReceiveText,
                ChannelCapability::Reactions,
            ],
            _ => vec![],
        }
    }

    fn delivery_hints(&self, channel_name: &str) -> Option<String> {
        self.delivery_hints_impl(channel_name)
    }

    fn configured_delivery_target(&self) -> Option<ConversationDeliveryTarget> {
        self.configured_delivery_target_impl()
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
    use crate::traits::ChannelMessage;

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
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
        assert!(registry.resolve("nonexistent").is_err());
    }

    #[test]
    fn has_channel_returns_true_for_injected() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock);
        assert!(registry.has_channel("test"));
        assert!(!registry.has_channel("nonexistent"));
    }

    #[test]
    fn capabilities_known_channels() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
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
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let a = registry.resolve("test").unwrap();
        let b = registry.resolve("test").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn deliver_sends_through_mock_channel() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let intent = OutboundIntent::notify("test", "recipient-1", "hello world".into());
        registry.deliver(&intent).await.unwrap();

        assert_eq!(mock.sent_messages(), vec!["hello world"]);
    }

    #[tokio::test]
    async fn deliver_skips_empty_content() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let intent = OutboundIntent::notify("test", "recipient-1", String::new());
        registry.deliver(&intent).await.unwrap();

        assert!(mock.sent_messages().is_empty());
    }

    #[tokio::test]
    async fn deliver_drops_on_missing_capability_with_drop_policy() {
        let config = Config::default();
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
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
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|cfg, id| anyhow::bail!("no channels configured for {id}")),
        );
        let mock = Arc::new(MockChannel::new("test"));
        registry.inject("test", mock.clone());

        let mut intent = OutboundIntent::notify("test", "r", "msg".into());
        intent.required_capabilities = vec![ChannelCapability::Attachments];
        intent.degradation_policy = DegradationPolicy::PlainText;

        registry.deliver(&intent).await.unwrap();
        assert_eq!(mock.sent_messages(), vec!["msg"]);
    }

    #[test]
    fn configured_delivery_target_prefers_single_unambiguous_channel_target() {
        let config = Config {
            channels_config: synapse_domain::config::schema::ChannelsConfig {
                slack: Some(synapse_domain::config::schema::SlackConfig {
                    bot_token: "xoxb-test".into(),
                    app_token: None,
                    channel_id: Some("C123".into()),
                    allowed_users: Vec::new(),
                    interrupt_on_new_message: false,
                    mention_only: false,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|_, id| anyhow::bail!("no channels configured for {id}")),
        );

        assert_eq!(
            registry.configured_delivery_target(),
            Some(ConversationDeliveryTarget::Explicit {
                channel: "slack".into(),
                recipient: "C123".into(),
                thread_ref: None,
            })
        );
    }

    #[test]
    fn configured_delivery_target_returns_none_when_multiple_targets_exist() {
        let config = Config {
            channels_config: synapse_domain::config::schema::ChannelsConfig {
                slack: Some(synapse_domain::config::schema::SlackConfig {
                    bot_token: "xoxb-test".into(),
                    app_token: None,
                    channel_id: Some("C123".into()),
                    allowed_users: Vec::new(),
                    interrupt_on_new_message: false,
                    mention_only: false,
                }),
                mattermost: Some(synapse_domain::config::schema::MattermostConfig {
                    url: "https://mattermost.example.com".into(),
                    bot_token: "mm-token".into(),
                    channel_id: Some("channel-1".into()),
                    allowed_users: Vec::new(),
                    thread_replies: None,
                    mention_only: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let registry = CachedChannelRegistry::new(
            config,
            Arc::new(|_, id| anyhow::bail!("no channels configured for {id}")),
        );

        assert_eq!(registry.configured_delivery_target(), None);
    }
}
