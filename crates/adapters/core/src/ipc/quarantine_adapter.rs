//! Quarantine adapter — wraps `gateway::ipc::IpcDb` behind `QuarantinePort`.
//!
//! Phase 4.0: exposes quarantine operations through a clean port.

use crate::gateway::ipc::IpcDb;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::domain::approval::QuarantineItem;
use synapse_domain::ports::approval::QuarantinePort;

/// Adapter that wraps `IpcDb` quarantine operations.
pub struct QuarantineAdapter {
    db: Arc<IpcDb>,
}

impl QuarantineAdapter {
    pub fn new(db: Arc<IpcDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl QuarantinePort for QuarantineAdapter {
    async fn quarantine_agent(&self, agent_id: &str) -> Result<u64> {
        let count = self.db.quarantine_pending_messages(agent_id).await;
        Ok(count as u64)
    }

    async fn promote_message(&self, message_id: i64, to_agent: &str) -> Result<i64> {
        let original = self
            .db
            .get_message(message_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Message {message_id} not found"))?;

        let promoted_payload = serde_json::json!({
            "original_id": message_id,
            "original_kind": original.kind,
            "payload": original.payload,
        })
        .to_string();

        let new_id = self
            .db
            .insert_promoted_message(
                &original.from_agent,
                to_agent,
                "promoted_quarantine",
                &promoted_payload,
                original.from_trust_level,
                original.session_id.as_deref(),
                original.priority,
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Promote insert error: {e:?}"))?;

        Ok(new_id)
    }

    async fn dismiss_message(&self, message_id: i64) -> Result<()> {
        self.db
            .dismiss_message(message_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    async fn list_quarantine(&self, limit: u32) -> Result<Vec<QuarantineItem>> {
        let rows = self
            .db
            .list_messages_admin(
                None,
                None,
                None,
                Some(true),
                None,
                None,
                None,
                None,
                limit,
                0,
            )
            .await;
        Ok(rows
            .into_iter()
            .map(|r| QuarantineItem {
                message_id: r.id,
                from_agent: r.from_agent,
                to_agent: r.to_agent,
                from_trust_level: i32::from(r.from_trust_level),
                original_kind: r.kind,
                payload: r.payload,
                created_at: r.created_at,
                promoted: r.promoted,
                dismissed: r.blocked,
            })
            .collect())
    }
}
