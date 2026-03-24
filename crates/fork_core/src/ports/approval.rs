//! Port: approval — tool approval and quarantine operations.
//!
//! Phase 4.0 Slice 4.

use crate::domain::approval::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalRisk, ApprovalOrigin,
    QuarantineItem,
};
use anyhow::Result;
use async_trait::async_trait;

/// Port for tool approval workflow.
#[async_trait]
pub trait ApprovalPort: Send + Sync {
    /// Check if a tool needs approval given the current autonomy config.
    fn needs_approval(&self, tool_name: &str) -> bool;

    /// Request approval (blocking with timeout for interactive, instant for non-interactive).
    async fn request_approval(
        &self,
        tool_name: &str,
        arguments: &str,
    ) -> Result<ApprovalResponse>;

    /// Record an approval decision for audit.
    fn record_decision(&self, decision: &ApprovalDecision);

    /// Check session allowlist (tools approved with "Always" in this session).
    fn is_session_allowed(&self, tool_name: &str) -> bool;

    /// Add tool to session allowlist.
    fn add_session_allowlist(&self, tool_name: &str);
}

/// Port for quarantine operations (IPC message lane management).
#[async_trait]
pub trait QuarantinePort: Send + Sync {
    /// Quarantine an agent — set trust_level=4, move pending messages.
    async fn quarantine_agent(&self, agent_id: &str) -> Result<u64>;

    /// Promote a quarantine message to the recipient's normal inbox.
    async fn promote_message(&self, message_id: i64, to_agent: &str) -> Result<i64>;

    /// Dismiss a quarantine message (soft-delete).
    async fn dismiss_message(&self, message_id: i64) -> Result<()>;

    /// List quarantine items for review.
    async fn list_quarantine(&self, limit: u32) -> Result<Vec<QuarantineItem>>;
}
