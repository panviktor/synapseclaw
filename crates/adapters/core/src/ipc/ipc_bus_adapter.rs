//! IPC bus adapter — wraps `gateway::ipc::IpcDb` behind `IpcBusPort`.
//!
//! Phase 4.0: extracts IPC DB operations from gateway handlers into
//! a clean port implementation.

use crate::gateway::ipc::IpcDb;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::domain::ipc::IpcMessage;
use synapse_domain::ports::ipc_bus::IpcBusPort;

/// Adapter that wraps `IpcDb` to implement `IpcBusPort`.
pub struct IpcBusAdapter {
    db: Arc<IpcDb>,
}

impl IpcBusAdapter {
    pub fn new(db: Arc<IpcDb>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl IpcBusPort for IpcBusAdapter {
    async fn send_message(
        &self,
        from_agent: &str,
        to_agent: &str,
        kind: &str,
        payload: &str,
        session_id: Option<&str>,
        from_trust_level: i32,
        priority: i32,
    ) -> Result<i64> {
        let id = self
            .db
            .insert_message(
                from_agent,
                to_agent,
                kind,
                payload,
                u8::try_from(from_trust_level).unwrap_or(0),
                session_id,
                priority,
                None, // message_ttl_secs — use default
            )
            .await
            .map_err(|e| anyhow::anyhow!("IPC insert error: {e:?}"))?;
        Ok(id)
    }

    async fn fetch_inbox(
        &self,
        agent_id: &str,
        include_quarantine: bool,
        limit: u32,
    ) -> Result<Vec<IpcMessage>> {
        let rows = self
            .db
            .fetch_inbox(agent_id, include_quarantine, limit)
            .await;
        Ok(rows
            .into_iter()
            .map(|r| IpcMessage {
                id: r.id,
                from_agent: r.from_agent,
                to_agent: r.to_agent,
                kind: r.kind,
                payload: r.payload,
                session_id: r.session_id,
                from_trust_level: i32::from(r.from_trust_level),
                priority: r.priority,
                created_at: r.created_at,
                promoted: r.quarantined == Some(false),
                read: false,
                blocked: false,
            })
            .collect())
    }

    async fn ack_messages(&self, _agent_id: &str, message_ids: &[i64]) -> Result<u64> {
        self.db.ack_messages(message_ids).await;
        Ok(message_ids.len() as u64)
    }

    async fn session_has_request(&self, session_id: &str, from_agent: &str) -> Result<bool> {
        Ok(self
            .db
            .session_has_request_for(session_id, from_agent)
            .await)
    }

    async fn get_agent_trust_level(&self, agent_id: &str) -> Option<i32> {
        self.db
            .agent_detail(agent_id, 0)
            .await
            .and_then(|info| info.agent.trust_level.map(i32::from))
    }
}
