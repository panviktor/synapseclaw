//! Port: IPC bus — send/receive inter-agent messages.
//!
//! Phase 4.0 Slice 5: abstracts the IPC broker DB behind a port.

use crate::fork_core::domain::ipc::IpcMessage;
use anyhow::Result;
use async_trait::async_trait;

/// Port for IPC message operations.
#[async_trait]
pub trait IpcBusPort: Send + Sync {
    /// Send a message (after ACL validation passes).
    async fn send_message(
        &self,
        from_agent: &str,
        to_agent: &str,
        kind: &str,
        payload: &str,
        session_id: Option<&str>,
        from_trust_level: i32,
        priority: i32,
    ) -> Result<i64>; // returns message seq

    /// Fetch inbox messages for an agent.
    async fn fetch_inbox(
        &self,
        agent_id: &str,
        include_quarantine: bool,
        limit: u32,
    ) -> Result<Vec<IpcMessage>>;

    /// Acknowledge (mark as read) messages by IDs.
    async fn ack_messages(&self, agent_id: &str, message_ids: &[i64]) -> Result<u64>;

    /// Check if a session has a prior task/query from a specific agent.
    async fn session_has_request(
        &self,
        session_id: &str,
        from_agent: &str,
    ) -> Result<bool>;

    /// Get agent trust level (None if agent not found).
    async fn get_agent_trust_level(&self, agent_id: &str) -> Option<i32>;
}
