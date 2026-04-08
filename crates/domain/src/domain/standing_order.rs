//! Standing orders — product-native proactive automation.
//!
//! Replaces ad-hoc shell scripts + cron for common proactive patterns:
//! "after restart, report here", "every morning send summary", etc.

use serde::{Deserialize, Serialize};

/// A persistent instruction to perform an action on a trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandingOrder {
    /// Unique identifier.
    pub id: String,
    /// What kind of order.
    pub kind: StandingOrderKind,
    /// Where to deliver the result.
    pub delivery_channel: String,
    /// Platform-specific recipient (chat_id, room_id).
    pub delivery_recipient: String,
    /// Optional thread reference.
    pub delivery_thread: Option<String>,
    /// Whether this order is active.
    pub enabled: bool,
    /// Who created it (agent_id).
    pub created_by: String,
    /// When it was created (unix secs).
    pub created_at: u64,
}

/// What triggers the standing order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StandingOrderKind {
    /// Trigger on runtime/service restart.
    RestartReport,
    /// Trigger on each heartbeat tick.
    HeartbeatReport,
    /// Trigger on a cron-like schedule with a custom prompt.
    ScheduledPrompt {
        prompt: String,
        cron_expression: String,
    },
    /// Trigger on a custom system event.
    CustomEvent { event_name: String, prompt: String },
}

/// System events that can trigger standing orders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemEvent {
    /// The runtime just restarted.
    RuntimeRestarted,
    /// Periodic heartbeat tick.
    HeartbeatTick,
    /// Operator-enqueued custom event.
    OperatorEvent { text: String },
}

impl StandingOrder {
    /// Check if this order should fire for a given system event.
    pub fn matches_event(&self, event: &SystemEvent) -> bool {
        if !self.enabled {
            return false;
        }
        match (&self.kind, event) {
            (StandingOrderKind::RestartReport, SystemEvent::RuntimeRestarted) => true,
            (StandingOrderKind::HeartbeatReport, SystemEvent::HeartbeatTick) => true,
            (
                StandingOrderKind::CustomEvent { event_name, .. },
                SystemEvent::OperatorEvent { text },
            ) => text.contains(event_name.as_str()),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_order(kind: StandingOrderKind) -> StandingOrder {
        StandingOrder {
            id: "test".into(),
            kind,
            delivery_channel: "matrix".into(),
            delivery_recipient: "!room:example.com".into(),
            delivery_thread: None,
            enabled: true,
            created_by: "broker".into(),
            created_at: 0,
        }
    }

    #[test]
    fn restart_report_matches() {
        let order = make_order(StandingOrderKind::RestartReport);
        assert!(order.matches_event(&SystemEvent::RuntimeRestarted));
        assert!(!order.matches_event(&SystemEvent::HeartbeatTick));
    }

    #[test]
    fn heartbeat_report_matches() {
        let order = make_order(StandingOrderKind::HeartbeatReport);
        assert!(order.matches_event(&SystemEvent::HeartbeatTick));
        assert!(!order.matches_event(&SystemEvent::RuntimeRestarted));
    }

    #[test]
    fn disabled_order_never_matches() {
        let mut order = make_order(StandingOrderKind::RestartReport);
        order.enabled = false;
        assert!(!order.matches_event(&SystemEvent::RuntimeRestarted));
    }
}
