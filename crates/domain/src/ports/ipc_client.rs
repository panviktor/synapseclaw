//! Port: IPC client — client-side broker communication.
//!
//! Abstracts the concrete IpcClient (reqwest-based) so that gateway,
//! channels, and tools can use IPC without depending on concrete HTTP impl.

use anyhow::Result;
use async_trait::async_trait;

/// Result of a broker HTTP call.
pub struct IpcSendResult {
    pub success: bool,
    pub status_code: u16,
    pub body: serde_json::Value,
}

/// Port for IPC client operations (client-side broker communication).
#[async_trait]
pub trait IpcClientPort: Send + Sync {
    /// Generic GET to broker API path. Returns parsed JSON.
    async fn broker_get(&self, path: &str) -> Result<IpcSendResult>;

    /// Generic POST to broker API path with JSON body. Returns parsed JSON.
    async fn broker_post(&self, path: &str, body: &serde_json::Value) -> Result<IpcSendResult>;

    /// Send an IPC message via `/api/ipc/send`. Convenience wrapper over `broker_post`.
    async fn send_message(&self, body: &serde_json::Value) -> Result<IpcSendResult> {
        self.broker_post("/api/ipc/send", body).await
    }

    /// Acknowledge messages by IDs via `/api/ipc/ack`.
    async fn ack_messages(&self, message_ids: &[i64]) -> Result<()> {
        let body = serde_json::json!({ "message_ids": message_ids });
        let result = self.broker_post("/api/ipc/ack", &body).await?;
        if !result.success {
            anyhow::bail!("ack_messages HTTP {}: {}", result.status_code, result.body);
        }
        Ok(())
    }

    /// Peek at inbox messages with optional filters.
    async fn peek_inbox(
        &self,
        from: Option<&str>,
        kinds: Option<&[&str]>,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>> {
        let mut query_parts = vec![format!("peek=true&limit={limit}")];
        if let Some(from) = from {
            query_parts.push(format!("from={from}"));
        }
        if let Some(kinds) = kinds {
            if !kinds.is_empty() {
                query_parts.push(format!("kinds={}", kinds.join(",")));
            }
        }
        let query_string = query_parts.join("&");
        let result = self
            .broker_get(&format!("/api/ipc/inbox?{query_string}"))
            .await?;
        if !result.success {
            anyhow::bail!("peek_inbox HTTP {}: {}", result.status_code, result.body);
        }
        Ok(result.body["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Register the agent's public key with the broker.
    async fn register_public_key(&self) -> Result<()>;

    /// Whether this client has an Ed25519 identity attached.
    fn has_identity(&self) -> bool;

    /// Sign a send body with the agent's Ed25519 identity (if available).
    fn sign_send_body(&self, body: &mut serde_json::Value);

    /// Synchronize sender sequence counter from DB.
    fn sync_sender_seq(&self, db_seq: i64);

}
