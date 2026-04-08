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

use crate::domain::channel::{ChannelCapability, OutboundIntent};
use crate::domain::config::{AutoDetectCandidate, CronDeliveryConfig, HeartbeatConfig};
use crate::ports::channel_registry::ChannelRegistryPort;
use anyhow::{bail, Result};
use std::sync::Arc;

/// Resolved delivery target: channel name + recipient.
#[derive(Debug, Clone)]
pub struct DeliveryTarget {
    pub channel: String,
    pub recipient: String,
    pub thread_ref: Option<String>,
}

/// Delivery service — owns all outbound delivery policy.
///
/// Stateless: holds only the registry reference.
pub struct DeliveryService {
    registry: Arc<dyn ChannelRegistryPort>,
}

impl DeliveryService {
    pub fn new(registry: Arc<dyn ChannelRegistryPort>) -> Self {
        Self { registry }
    }

    // ── Heartbeat delivery target resolution ─────────────────────

    /// Resolve the heartbeat delivery target.
    ///
    /// Priority: explicit `target` + `to` > auto-detect via candidates.
    /// Both fields must be set together or neither.
    pub fn resolve_heartbeat_target(
        &self,
        heartbeat: &HeartbeatConfig,
        candidates: &[AutoDetectCandidate],
    ) -> Result<Option<DeliveryTarget>> {
        let channel = heartbeat
            .target
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let target = heartbeat
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
                    thread_ref: None,
                }))
            }
            (Some(_), None) => bail!("heartbeat.to is required when heartbeat.target is set"),
            (None, Some(_)) => bail!("heartbeat.target is required when heartbeat.to is set"),
            (None, None) => Ok(self.auto_detect_delivery_channel(candidates)),
        }
    }

    /// Resolve the deadman alert delivery target.
    ///
    /// Priority: explicit `deadman_channel` + `deadman_to` >
    ///           heartbeat delivery target (passed in) > skip.
    pub fn resolve_deadman_target(
        &self,
        heartbeat: &HeartbeatConfig,
        heartbeat_target: &Option<DeliveryTarget>,
    ) -> Option<DeliveryTarget> {
        if let Some(ch) = &heartbeat.deadman_channel {
            let to = heartbeat
                .deadman_to
                .as_deref()
                .or(heartbeat.to.as_deref())
                .unwrap_or_default();
            return Some(DeliveryTarget {
                channel: ch.clone(),
                recipient: to.to_string(),
                thread_ref: None,
            });
        }

        heartbeat_target.clone()
    }

    // ── Auto-detect via capabilities ─────────────────────────────

    /// Auto-detect the best delivery channel from candidates.
    ///
    /// Picks the first candidate with a recipient and `SendText` capability.
    fn auto_detect_delivery_channel(
        &self,
        candidates: &[AutoDetectCandidate],
    ) -> Option<DeliveryTarget> {
        for candidate in candidates {
            if let Some(ref rcpt) = candidate.recipient {
                let caps = self.registry.capabilities(&candidate.channel_name);
                if caps.contains(&ChannelCapability::SendText) {
                    return Some(DeliveryTarget {
                        channel: candidate.channel_name.clone(),
                        recipient: rcpt.clone(),
                        thread_ref: None,
                    });
                }
            }
        }
        None
    }

    /// Validate that a channel is available in the registry.
    fn validate_channel(&self, channel: &str) -> Result<()> {
        if !self.registry.has_channel(&channel.to_ascii_lowercase()) {
            bail!("heartbeat.target is set to '{channel}' but channel is not available");
        }
        Ok(())
    }

    // ── Delivery execution ───────────────────────────────────────

    /// Deliver an announcement text to a target via the registry.
    pub async fn deliver(&self, target: &DeliveryTarget, text: &str) -> Result<()> {
        let intent = OutboundIntent::notify_in_thread(
            &target.channel,
            &target.recipient,
            target.thread_ref.clone(),
            text.to_string(),
        );
        self.registry.deliver(&intent).await
    }

    // ── Cron delivery ────────────────────────────────────────────

    /// Validate cron job delivery config and deliver if mode is "announce".
    ///
    /// Returns Ok(()) if delivery mode is not "announce" (nothing to do).
    pub async fn deliver_cron_output(
        &self,
        delivery: &CronDeliveryConfig,
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
                thread_ref: delivery.thread_ref.clone(),
            },
            output,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::channel::ChannelCapability;
    use async_trait::async_trait;
    use std::sync::Mutex;

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
        fn has_channel(&self, channel_name: &str) -> bool {
            self.capabilities
                .iter()
                .any(|(name, _)| name == channel_name)
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

    fn test_candidates() -> Vec<AutoDetectCandidate> {
        vec![
            AutoDetectCandidate {
                channel_name: "matrix".into(),
                recipient: Some("!room:example.com".into()),
            },
            AutoDetectCandidate {
                channel_name: "telegram".into(),
                recipient: Some("123456".into()),
            },
        ]
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
        let hb = HeartbeatConfig {
            target: Some("matrix".into()),
            to: Some("!room:example.com".into()),
            ..Default::default()
        };

        let target = svc.resolve_heartbeat_target(&hb, &[]).unwrap().unwrap();
        assert_eq!(target.channel, "matrix");
        assert_eq!(target.recipient, "!room:example.com");
        assert_eq!(target.thread_ref, None);
    }

    #[test]
    fn resolve_only_target_no_to_is_error() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let hb = HeartbeatConfig {
            target: Some("matrix".into()),
            to: None,
            ..Default::default()
        };

        let err = svc.resolve_heartbeat_target(&hb, &[]).unwrap_err();
        assert!(err.to_string().contains("heartbeat.to is required"));
    }

    #[test]
    fn resolve_only_to_no_target_is_error() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let hb = HeartbeatConfig {
            target: None,
            to: Some("!room:example.com".into()),
            ..Default::default()
        };

        let err = svc.resolve_heartbeat_target(&hb, &[]).unwrap_err();
        assert!(err.to_string().contains("heartbeat.target is required"));
    }

    #[test]
    fn resolve_auto_detect_picks_matrix_first() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let candidates = test_candidates();

        let target = svc
            .resolve_heartbeat_target(&HeartbeatConfig::default(), &candidates)
            .unwrap()
            .unwrap();
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
        let candidates = vec![AutoDetectCandidate {
            channel_name: "telegram".into(),
            recipient: Some("123456".into()),
        }];

        let target = svc
            .resolve_heartbeat_target(&HeartbeatConfig::default(), &candidates)
            .unwrap()
            .unwrap();
        assert_eq!(target.channel, "telegram");
        assert_eq!(target.recipient, "123456");
    }

    #[test]
    fn resolve_auto_detect_returns_none_when_no_channels() {
        let registry = Arc::new(MockRegistry::new(vec![]));
        let svc = DeliveryService::new(registry);

        assert!(svc
            .resolve_heartbeat_target(&HeartbeatConfig::default(), &[])
            .unwrap()
            .is_none());
    }

    #[test]
    fn resolve_invalid_explicit_channel_is_error() {
        let registry = Arc::new(MockRegistry::new(vec![]));
        let svc = DeliveryService::new(registry);
        let hb = HeartbeatConfig {
            target: Some("nonexistent".into()),
            to: Some("someone".into()),
            ..Default::default()
        };

        let err = svc.resolve_heartbeat_target(&hb, &[]).unwrap_err();
        assert!(err.to_string().contains("not available"));
    }

    // ── deadman target resolution ────────────────────────────────

    #[test]
    fn deadman_explicit_overrides_heartbeat() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let hb = HeartbeatConfig {
            deadman_channel: Some("telegram".into()),
            deadman_to: Some("999".into()),
            ..Default::default()
        };

        let hb_target = Some(DeliveryTarget {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
            thread_ref: None,
        });
        let dm = svc.resolve_deadman_target(&hb, &hb_target).unwrap();
        assert_eq!(dm.channel, "telegram");
        assert_eq!(dm.recipient, "999");
    }

    #[test]
    fn deadman_falls_back_to_heartbeat_target() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);

        let hb_target = Some(DeliveryTarget {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
            thread_ref: None,
        });
        let dm = svc
            .resolve_deadman_target(&HeartbeatConfig::default(), &hb_target)
            .unwrap();
        assert_eq!(dm.channel, "matrix");
    }

    #[test]
    fn deadman_returns_none_when_no_targets() {
        let registry = Arc::new(MockRegistry::new(vec![]));
        let svc = DeliveryService::new(registry);

        assert!(svc
            .resolve_deadman_target(&HeartbeatConfig::default(), &None)
            .is_none());
    }

    // ── deliver tests ────────────────────────────────────────────

    #[tokio::test]
    async fn deliver_sends_notify_intent() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry.clone());
        let target = DeliveryTarget {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
            thread_ref: None,
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
        let delivery = CronDeliveryConfig {
            mode: "silent".into(),
            channel: None,
            to: None,
            thread_ref: None,
        };

        svc.deliver_cron_output(&delivery, "output").await.unwrap();
        assert!(registry.delivered().is_empty());
    }

    #[tokio::test]
    async fn cron_delivery_requires_channel() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let delivery = CronDeliveryConfig {
            mode: "announce".into(),
            channel: None,
            to: Some("target".into()),
            thread_ref: None,
        };

        let err = svc
            .deliver_cron_output(&delivery, "output")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("delivery.channel is required"));
    }

    #[tokio::test]
    async fn cron_delivery_requires_to() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry);
        let delivery = CronDeliveryConfig {
            mode: "announce".into(),
            channel: Some("matrix".into()),
            to: None,
            thread_ref: None,
        };

        let err = svc
            .deliver_cron_output(&delivery, "output")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("delivery.to is required"));
    }

    #[tokio::test]
    async fn cron_delivery_sends_to_target() {
        let registry = registry_with_matrix_and_telegram();
        let svc = DeliveryService::new(registry.clone());
        let delivery = CronDeliveryConfig {
            mode: "announce".into(),
            channel: Some("telegram".into()),
            to: Some("123456".into()),
            thread_ref: None,
        };

        svc.deliver_cron_output(&delivery, "cron output")
            .await
            .unwrap();
        let delivered = registry.delivered();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].target_channel, "telegram");
        assert_eq!(delivered[0].target_recipient, "123456");
        assert_eq!(delivered[0].content.as_text(), "cron output");
    }
}
