//! Approval domain types — tool approval and quarantine.
//!
//! Phase 4.0 Slice 4: first-class approval and quarantine objects.

use std::fmt;

/// Risk level for an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRisk {
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for ApprovalRisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Where the approval request originated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOrigin {
    /// Channel message processing.
    Channel,
    /// Inter-agent IPC.
    Ipc,
    /// Standard operating procedure.
    Sop,
    /// Runtime (agent loop).
    Runtime,
}

/// Operator's response to an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalResponse {
    /// Approve this action.
    Yes,
    /// Deny this action.
    No,
    /// Approve and add to session allowlist.
    Always,
}

/// Status of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

/// A first-class approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub id: String,
    pub origin: ApprovalOrigin,
    pub requested_by: String,
    pub tool_name: String,
    pub action_summary: String,
    pub risk: ApprovalRisk,
    pub conversation_key: Option<String>,
    pub run_id: Option<String>,
    pub status: ApprovalStatus,
    pub created_at: u64,
}

/// A quarantine item — a message held for review.
#[derive(Debug, Clone)]
pub struct QuarantineItem {
    pub message_id: i64,
    pub from_agent: String,
    pub to_agent: String,
    pub from_trust_level: i32,
    pub original_kind: String,
    pub payload: String,
    pub created_at: i64,
    pub promoted: bool,
    pub dismissed: bool,
}

/// Decision record for audit trail.
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    pub request_id: String,
    pub response: ApprovalResponse,
    pub decided_by: String,
    pub channel: String,
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_display() {
        assert_eq!(ApprovalRisk::Low.to_string(), "low");
        assert_eq!(ApprovalRisk::Critical.to_string(), "critical");
    }

    #[test]
    fn approval_request_fields() {
        let req = ApprovalRequest {
            id: "req-1".into(),
            origin: ApprovalOrigin::Runtime,
            requested_by: "agent".into(),
            tool_name: "shell".into(),
            action_summary: "rm -rf /tmp".into(),
            risk: ApprovalRisk::High,
            conversation_key: Some("web:abc:123".into()),
            run_id: Some("run-1".into()),
            status: ApprovalStatus::Pending,
            created_at: 1000,
        };
        assert_eq!(req.status, ApprovalStatus::Pending);
        assert_eq!(req.risk, ApprovalRisk::High);
    }
}
