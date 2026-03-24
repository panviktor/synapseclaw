//! Use case: ReviewQuarantineItem — promote or dismiss quarantined messages.
//!
//! Phase 4.0 Slice 4.

use crate::fork_core::domain::approval::QuarantineItem;
use crate::fork_core::ports::approval::QuarantinePort;
use anyhow::Result;

/// Promote a quarantined message to the recipient's inbox.
///
/// Returns the new message ID created in the normal inbox.
pub async fn promote(
    port: &dyn QuarantinePort,
    message_id: i64,
    to_agent: &str,
) -> Result<i64> {
    tracing::info!(
        message_id,
        to_agent,
        "Promoting quarantine message to inbox"
    );
    port.promote_message(message_id, to_agent).await
}

/// Dismiss a quarantined message (soft-delete).
pub async fn dismiss(port: &dyn QuarantinePort, message_id: i64) -> Result<()> {
    tracing::info!(message_id, "Dismissing quarantine message");
    port.dismiss_message(message_id).await
}

/// List quarantine items for admin review.
pub async fn list(port: &dyn QuarantinePort, limit: u32) -> Result<Vec<QuarantineItem>> {
    port.list_quarantine(limit).await
}

/// Quarantine an agent — move all pending messages to quarantine lane.
///
/// Returns count of messages moved.
pub async fn quarantine_agent(port: &dyn QuarantinePort, agent_id: &str) -> Result<u64> {
    tracing::info!(agent_id, "Quarantining agent");
    port.quarantine_agent(agent_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockQuarantinePort;

    #[async_trait]
    impl QuarantinePort for MockQuarantinePort {
        async fn quarantine_agent(&self, _agent_id: &str) -> Result<u64> {
            Ok(3) // 3 messages moved
        }
        async fn promote_message(&self, _msg_id: i64, _to: &str) -> Result<i64> {
            Ok(999) // new message id
        }
        async fn dismiss_message(&self, _msg_id: i64) -> Result<()> {
            Ok(())
        }
        async fn list_quarantine(&self, _limit: u32) -> Result<Vec<QuarantineItem>> {
            Ok(vec![QuarantineItem {
                message_id: 1,
                from_agent: "agent-a".into(),
                to_agent: "agent-b".into(),
                from_trust_level: 4,
                original_kind: "task".into(),
                payload: "do something".into(),
                created_at: 1000,
                promoted: false,
                dismissed: false,
            }])
        }
    }

    #[tokio::test]
    async fn promote_returns_new_id() {
        let port = MockQuarantinePort;
        let new_id = promote(&port, 1, "agent-b").await.unwrap();
        assert_eq!(new_id, 999);
    }

    #[tokio::test]
    async fn dismiss_succeeds() {
        let port = MockQuarantinePort;
        dismiss(&port, 1).await.unwrap();
    }

    #[tokio::test]
    async fn list_returns_items() {
        let port = MockQuarantinePort;
        let items = list(&port, 10).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].from_agent, "agent-a");
    }

    #[tokio::test]
    async fn quarantine_agent_returns_count() {
        let port = MockQuarantinePort;
        let count = quarantine_agent(&port, "agent-a").await.unwrap();
        assert_eq!(count, 3);
    }
}
