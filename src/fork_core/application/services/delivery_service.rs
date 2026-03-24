//! Delivery service — owns outbound delivery policy.
//!
//! Phase 4.0 Slice 1: moves heartbeat/cron delivery decisions out of
//! daemon/scheduler and into the fork-owned application core.
//!
//! Business rules this service owns:
//! - heartbeat delivery target resolution (explicit config vs auto-detect)
//! - auto-detect channel selection via capabilities (replaces hardcoded priority)
//! - cron delivery mode gating and field validation
//! - deadman alert target resolution
//! - announcement formatting policy

use crate::config::Config;
use crate::fork_core::domain::channel::{ChannelCapability, OutboundIntent};
use crate::fork_core::ports::channel_registry::ChannelRegistryPort;
use anyhow::{bail, Result};
use std::sync::Arc;

/// Resolved delivery target: channel name + recipient.
#[derive(Debug, Clone)]
pub struct DeliveryTarget {
    pub channel: String,
    pub recipient: String,
}

/// Delivery service — owns all outbound delivery policy.
///
/// Stateless: holds only the registry reference.  All config-dependent
/// decisions take `&Config` as a parameter so the service can be shared.
pub struct DeliveryService {
    registry: Arc<dyn ChannelRegistryPort>,
}

impl DeliveryService {
    pub fn new(registry: Arc<dyn ChannelRegistryPort>) -> Self {
        Self { registry }
    }

    // ── Heartbeat delivery target resolution ─────────────────────

    /// Resolve the heartbeat delivery target from config.
    ///
    /// Priority: explicit `heartbeat.target` + `heartbeat.to` > auto-detect.
    /// Both fields must be set together or neither.
    pub fn resolve_heartbeat_target(&self, config: &Config) -> Result<Option<DeliveryTarget>> {
        let channel = config
            .heartbeat
            .target
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let target = config
            .heartbeat
            .to
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());

        match (channel, target) {
            (Some(ch), Some(to)) => {
                self.validate_channel(ch)?;
                Ok(Some(DeliveryTarget {
                    channel: ch.to_string(),
                    recipient: to.to_string(),
                }))
            }
            (Some(_), None) => bail!("heartbeat.to is required when heartbeat.target is set"),
            (None, Some(_)) => bail!("heartbeat.target is required when heartbeat.to is set"),
            (None, None) => Ok(self.auto_detect_delivery_channel(config)),
        }
    }

    /// Resolve the deadman alert delivery target.
    ///
    /// Priority: explicit `deadman_channel` + `deadman_to` >
    ///           heartbeat delivery target (passed in) > skip.
    pub fn resolve_deadman_target(
        &self,
        config: &Config,
        heartbeat_target: &Option<DeliveryTarget>,
    ) -> Option<DeliveryTarget> {
        // Explicit deadman config
        if let Some(ch) = &config.heartbeat.deadman_channel {
            let to = config
                .heartbeat
                .deadman_to
                .as_deref()
                .or(config.heartbeat.to.as_deref())
                .unwrap_or_default();
            return Some(DeliveryTarget {
                channel: ch.clone(),
                recipient: to.to_string(),
            });
        }

        // Fall back to heartbeat target
        heartbeat_target.clone()
    }

    // ── Auto-detect via capabilities ─────────────────────────────

    /// Auto-detect the best delivery channel by checking which configured
    /// channels have `SendText` capability and can provide a recipient.
    ///
    /// Uses the capability registry instead of hardcoded channel priority.
    /// Channels that can auto-extract a recipient (matrix, telegram via
    /// `allowed_users[0]`) are preferred over those that require explicit config.
    fn auto_detect_delivery_channel(&self, config: &Config) -> Option<DeliveryTarget> {
        // Candidate channels that support auto-detection of recipient
        let candidates = [
            ("matrix", Self::extract_matrix_recipient(config)),
            ("telegram", Self::extract_telegram_recipient(config)),
        ];

        for (name, recipient) in &candidates {
            if let Some(rcpt) = recipient {
                let caps = self.registry.capabilities(name);
                if caps.contains(&ChannelCapability::SendText) {
                    return Some(DeliveryTarget {
                        channel: name.to_string(),
                        recipient: rcpt.clone(),
                    });
                }
            }
        }

        None
    }

    /// Validate that a channel is available in the registry.
    fn validate_channel(&self, channel: &str) -> Result<()> {
        self.registry
            .resolve(&channel.to_ascii_lowercase())
            .map(|_| ())
            .map_err(|e| {
                anyhow::anyhow!(
                    "heartbeat.target is set to '{channel}' but channel is not available: {e}"
                )
            })
    }

    // ── Recipient extraction helpers ─────────────────────────────

    fn extract_matrix_recipient(config: &Config) -> Option<String> {
        config
            .channels_config
            .matrix
            .as_ref()
            .and_then(|mx| mx.allowed_users.first().cloned())
            .filter(|s| !s.is_empty())
    }

    fn extract_telegram_recipient(config: &Config) -> Option<String> {
        config
            .channels_config
            .telegram
            .as_ref()
            .and_then(|tg| tg.allowed_users.first().cloned())
            .filter(|s| !s.is_empty())
    }

    // ── Delivery execution ───────────────────────────────────────

    /// Deliver an announcement text to a target via the registry.
    pub async fn deliver(&self, target: &DeliveryTarget, text: &str) -> Result<()> {
        let intent = OutboundIntent::notify(&target.channel, &target.recipient, text.to_string());
        self.registry.deliver(&intent).await
    }

    // ── Cron delivery ────────────────────────────────────────────

    /// Validate cron job delivery config and deliver if mode is "announce".
    ///
    /// Returns Ok(()) if delivery mode is not "announce" (nothing to do).
    pub async fn deliver_cron_output(
        &self,
        delivery: &crate::cron::DeliveryConfig,
        output: &str,
    ) -> Result<()> {
        if !delivery.mode.eq_ignore_ascii_case("announce") {
            return Ok(());
        }

        let channel = delivery
            .channel
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("delivery.channel is required for announce mode"))?;
        let target = delivery
            .to
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("delivery.to is required for announce mode"))?;

        self.deliver(
            &DeliveryTarget {
                channel: channel.to_string(),
                recipient: target.to_string(),
            },
            output,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork_core::domain::channel::ChannelCapability;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Test-only mock registry that records deliveries and returns configurable capabilities.
    struct MockRegistry {
        capabilities: Vec<(String, Vec<ChannelCapability>)>,
        deliveries: Mutex<Vec<OutboundIntent>>,
    }

    impl MockRegistry {
        fn new(capabilities: Vec<(String, Vec<ChannelCapability>)>) -> Self {
            Self {
                capabilities,
                deliveries: Mutex::new(vec![]),
            }
        }

        fn delivered(&self) -> Vec<OutboundIntent> {
            self.deliveries.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ChannelRegistryPort for MockRegistry {
        fn resolve(
            &self,
            channel_name: &str,
        ) -> Result<Arc<dyn crate::channels::traits::Channel>> {
            // For validation: succeed if we have caps for this channel
            if self
                .capabilities
                .iter()
                .any(|(name, _)| name == channel_name)
            {
                // Return a minimal mock — we only need resolve to not error
                Ok(Arc::new(NullChannel))
            } else {
                bail!("channel '{channel_name}' not found")
            }
        }

        fn capabilities(&self, channel_name: &str) -> Vec<ChannelCapability> {
            self.capabilities
                .iter()
                .find(|(name, _)| name == channel_name)
                .map(|(_, caps)| caps.clone())
                .unwrap_or_default()
        }

        async fn deliver(&self, intent: &OutboundIntent) -> Result<()> {
            self.deliveries.lock().unwrap().push(intent.clone());
            Ok(())
        }
    }

    /// Null channel for resolve() validation.
    struct NullChannel;

    #[async_trait]
    impl crate::channels::traits::Channel for NullChannel {
        fn name(&self) -> &str {
            "null"
        }

        async fn send(
            &self,
            _msg: &crate::channels::traits::SendMessage,
        ) -> Result<()> {
            Ok(())
        }

        async fn listen(
            &self,
            _tx: tokio::sync::mpsc::Sender<crate::channels::traits::ChannelMessage>,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn test_config_with_matrix_and_telegram() -> Config {
        let mut config = Config::default();
        config.channels_config.matrix = Some(crate::config::MatrixConfig {
            homeserver: "https://matrix.example.com".into(),
            access_token: None,
            user_id: None,
            device_id: None,
            room_id: "!room:example.com".into(),
            allowed_users: vec!["!room:example.com".into()],
            password: None,
            max_media_download_mb: None,
        });
        config.channels_config.telegram = Some(crate::config::TelegramConfig {
            bot_token: "token".into(),
            allowed_users: vec!["123456".into()],
            stream_mode: crate::config::StreamMode::default(),
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
        });
        config
    }

    fn registry_with_matrix_and_telegram() -> Arc<MockRegistry> {
        Arc::new(MockRegistry::new(vec![
            (
                "matrix".into(),
                vec![ChannelCapability::SendText, ChannelCapability::Threads],
            ),
            (
                "telegram".into(),
                vec![ChannelCapability::SendText, ChannelCapability::Typing],
            ),
        ]))
    }

    // ── resolve_heartbeat_target tests ───────────────────────────

    #[test]
    fn resolve_explicit_target() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let mut config = test_config_with_matrix_and_telegram();
        config.heartbeat.target = Some("matrix".into());
        config.heartbeat.to = Some("!room:example.com".into());

        let target = svc.resolve_heartbeat_target(&config).unwrap().unwrap();
        assert_eq!(target.channel, "matrix");
        assert_eq!(target.recipient, "!room:example.com");
    }

    #[test]
    fn resolve_only_target_no_to_is_error() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let mut config = test_config_with_matrix_and_telegram();
        config.heartbeat.target = Some("matrix".into());
        config.heartbeat.to = None;

        let err = svc.resolve_heartbeat_target(&config).unwrap_err();
        assert!(err.to_string().contains("heartbeat.to is required"));
    }

    #[test]
    fn resolve_only_to_no_target_is_error() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let mut config = test_config_with_matrix_and_telegram();
        config.heartbeat.target = None;
        config.heartbeat.to = Some("!room:example.com".into());

        let err = svc.resolve_heartbeat_target(&config).unwrap_err();
        assert!(err.to_string().contains("heartbeat.target is required"));
    }

    #[test]
    fn resolve_auto_detect_picks_matrix_first() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let config = test_config_with_matrix_and_telegram();

        let target = svc.resolve_heartbeat_target(&config).unwrap().unwrap();
        assert_eq!(target.channel, "matrix");
        assert_eq!(target.recipient, "!room:example.com");
    }

    #[test]
    fn resolve_auto_detect_falls_to_telegram_when_no_matrix() {
        let registry = Arc::new(MockRegistry::new(vec![(
            "telegram".into(),
            vec![ChannelCapability::SendText],
        )]));
        let svc = DeliveryService::new(registry);
        let mut config = Config::default();
        config.channels_config.telegram = Some(crate::config::TelegramConfig {
            bot_token: "token".into(),
            allowed_users: vec!["123456".into()],
            stream_mode: crate::config::StreamMode::default(),
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
        });

        let target = svc.resolve_heartbeat_target(&config).unwrap().unwrap();
        assert_eq!(target.channel, "telegram");
        assert_eq!(target.recipient, "123456");
    }

    #[test]
    fn resolve_auto_detect_returns_none_when_no_channels() {
        let registry = Arc::new(MockRegistry::new(vec![]));
        let svc = DeliveryService::new(registry);
        let config = Config::default();

        assert!(svc.resolve_heartbeat_target(&config).unwrap().is_none());
    }

    #[test]
    fn resolve_invalid_explicit_channel_is_error() {
        let registry = Arc::new(MockRegistry::new(vec![]));
        let svc = DeliveryService::new(registry);
        let mut config = Config::default();
        config.heartbeat.target = Some("nonexistent".into());
        config.heartbeat.to = Some("someone".into());

        let err = svc.resolve_heartbeat_target(&config).unwrap_err();
        assert!(err.to_string().contains("not available"));
    }

    // ── deadman target resolution ────────────────────────────────

    #[test]
    fn deadman_explicit_overrides_heartbeat() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let mut config = test_config_with_matrix_and_telegram();
        config.heartbeat.deadman_channel = Some("telegram".into());
        config.heartbeat.deadman_to = Some("999".into());

        let hb = Some(DeliveryTarget {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
        });
        let dm = svc.resolve_deadman_target(&config, &hb).unwrap();
        assert_eq!(dm.channel, "telegram");
        assert_eq!(dm.recipient, "999");
    }

    #[test]
    fn deadman_falls_back_to_heartbeat_target() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let config = test_config_with_matrix_and_telegram();

        let hb = Some(DeliveryTarget {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
        });
        let dm = svc.resolve_deadman_target(&config, &hb).unwrap();
        assert_eq!(dm.channel, "matrix");
    }

    #[test]
    fn deadman_returns_none_when_no_targets() {
        let registry = Arc::new(MockRegistry::new(vec![]));
        let svc = DeliveryService::new(registry);
        let config = Config::default();

        assert!(svc.resolve_deadman_target(&config, &None).is_none());
    }

    // ── deliver tests ────────────────────────────────────────────

    #[tokio::test]
    async fn deliver_sends_notify_intent() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry.clone());
        let target = DeliveryTarget {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
        };

        svc.deliver(&target, "test message").await.unwrap();

        let delivered = registry.delivered();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].target_channel, "matrix");
        assert_eq!(delivered[0].content.as_text(), "test message");
    }

    // ── cron delivery tests ──────────────────────────────────────

    #[tokio::test]
    async fn cron_delivery_skips_non_announce_mode() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry.clone());
        let delivery = crate::cron::DeliveryConfig {
            mode: "silent".into(),
            channel: None,
            to: None,
            ..Default::default()
        };

        svc.deliver_cron_output(&delivery, "output").await.unwrap();
        assert!(registry.delivered().is_empty());
    }

    #[tokio::test]
    async fn cron_delivery_requires_channel() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let delivery = crate::cron::DeliveryConfig {
            mode: "announce".into(),
            channel: None,
            to: Some("target".into()),
            ..Default::default()
        };

        let err = svc.deliver_cron_output(&delivery, "output").await.unwrap_err();
        assert!(err.to_string().contains("delivery.channel is required"));
    }

    #[tokio::test]
    async fn cron_delivery_requires_to() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let delivery = crate::cron::DeliveryConfig {
            mode: "announce".into(),
            channel: Some("matrix".into()),
            to: None,
            ..Default::default()
        };

        let err = svc.deliver_cron_output(&delivery, "output").await.unwrap_err();
        assert!(err.to_string().contains("delivery.to is required"));
    }

    #[tokio::test]
    async fn cron_delivery_sends_to_target() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry.clone());
        let delivery = crate::cron::DeliveryConfig {
            mode: "announce".into(),
            channel: Some("telegram".into()),
            to: Some("123456".into()),
            ..Default::default()
        };

        svc.deliver_cron_output(&delivery, "cron output").await.unwrap();
        let delivered = registry.delivered();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].target_channel, "telegram");
        assert_eq!(delivered[0].target_recipient, "123456");
        assert_eq!(delivered[0].content.as_text(), "cron output");
    }
}
