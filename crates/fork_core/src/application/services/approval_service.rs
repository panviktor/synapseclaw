//! Approval service — owns tool approval policy and quarantine review.
//!
//! Phase 4.0 Slice 4: extracts business logic from approval_manager.rs
//! and gateway/ipc.rs quarantine endpoints.
//!
//! Business rules this service owns:
//! - tool approval decision (auto-approve, always-ask, autonomy level)
//! - session allowlist semantics (always_ask overrides session allowlist)
//! - quarantine review decisions (promote vs dismiss)
//! - audit trail recording

use crate::domain::approval::{
    ApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalRisk, ApprovalOrigin,
    ApprovalStatus, QuarantineItem,
};
use crate::ports::approval::{ApprovalPort, QuarantinePort};
use anyhow::Result;
use std::sync::Arc;

// ── Tool approval policy ─────────────────────────────────────────

/// Re-export from security — single source of truth for autonomy levels.
pub use crate::domain::config::AutonomyLevel;

/// Check if a tool needs approval based on autonomy config.
///
/// Business rules:
/// - ReadOnly → always deny (no tools at all)
/// - Full → never needs approval
/// - Supervised → needs approval unless in auto_approve list
/// - always_ask overrides everything (even session allowlist)
pub fn check_needs_approval(
    tool_name: &str,
    autonomy: AutonomyLevel,
    auto_approve: &[String],
    always_ask: &[String],
    session_allowlist: &[String],
    non_interactive: bool,
) -> bool {
    match autonomy {
        AutonomyLevel::ReadOnly => false, // blocked elsewhere, no prompt needed
        AutonomyLevel::Full => {
            // Even full autonomy respects always_ask
            always_ask.iter().any(|t| t == tool_name)
        }
        AutonomyLevel::Supervised => {
            // always_ask overrides everything
            if always_ask.iter().any(|t| t == tool_name) {
                return true;
            }
            // Non-interactive shell: skip outer approval (shell's own policy guards)
            if non_interactive && tool_name == "shell" {
                return false;
            }
            // auto_approve skips
            if auto_approve.iter().any(|t| t == tool_name) {
                return false;
            }
            // Session allowlist (prior "Always" responses)
            if session_allowlist.iter().any(|t| t == tool_name) {
                return false;
            }
            true // supervised = ask by default
        }
    }
}

/// Check if a tool is in the session allowlist BUT not in always_ask.
///
/// always_ask overrides session allowlist — operator must approve every time.
pub fn is_session_allowed(
    tool_name: &str,
    session_allowlist: &[String],
    always_ask: &[String],
) -> bool {
    if always_ask.iter().any(|t| t == tool_name) {
        return false;
    }
    session_allowlist.iter().any(|t| t == tool_name)
}

// ── Quarantine review ────────────────────────────────────────────

/// Promote a quarantine item — business validation before delegating to port.
pub async fn promote_quarantine_item(
    port: &dyn QuarantinePort,
    message_id: i64,
    to_agent: &str,
) -> Result<i64> {
    port.promote_message(message_id, to_agent).await
}

/// Dismiss a quarantine item.
pub async fn dismiss_quarantine_item(
    port: &dyn QuarantinePort,
    message_id: i64,
) -> Result<()> {
    port.dismiss_message(message_id).await
}

/// Quarantine an agent — set trust level and move messages.
pub async fn quarantine_agent(
    port: &dyn QuarantinePort,
    agent_id: &str,
) -> Result<u64> {
    port.quarantine_agent(agent_id).await
}

// ── Decision recording policy ─────────────────────────────────────

/// Determine if a decision should add the tool to the session allowlist.
///
/// Business rule: "Always" response → add to allowlist for this session.
pub fn should_add_to_allowlist(response: &ApprovalResponse) -> bool {
    *response == ApprovalResponse::Always
}

// ── Approval request creation ────────────────────────────────────

/// Create an approval request.
pub fn create_approval_request(
    tool_name: &str,
    action_summary: &str,
    risk: ApprovalRisk,
    origin: ApprovalOrigin,
    conversation_key: Option<&str>,
    run_id: Option<&str>,
) -> ApprovalRequest {
    ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        origin,
        requested_by: "system".into(),
        tool_name: tool_name.into(),
        action_summary: action_summary.into(),
        risk,
        conversation_key: conversation_key.map(String::from),
        run_id: run_id.map(String::from),
        status: ApprovalStatus::Pending,
        created_at: chrono::Utc::now().timestamp() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_autonomy_no_approval_needed() {
        assert!(!check_needs_approval("shell", AutonomyLevel::Full, &[], &[], &[], false));
    }

    #[test]
    fn full_autonomy_always_ask_overrides() {
        assert!(check_needs_approval("shell", AutonomyLevel::Full, &[], &["shell".into()], &[], false));
    }

    #[test]
    fn supervised_auto_approve_skips() {
        assert!(!check_needs_approval("file_read", AutonomyLevel::Supervised, &["file_read".into()], &[], &[], false));
    }

    #[test]
    fn supervised_default_asks() {
        assert!(check_needs_approval("shell", AutonomyLevel::Supervised, &[], &[], &[], false));
    }

    #[test]
    fn supervised_always_ask_overrides_auto_approve() {
        assert!(check_needs_approval("shell", AutonomyLevel::Supervised, &["shell".into()], &["shell".into()], &[], false));
    }

    #[test]
    fn read_only_no_prompt_needed() {
        // ReadOnly blocks elsewhere — needs_approval returns false (no prompt)
        assert!(!check_needs_approval("file_read", AutonomyLevel::ReadOnly, &["file_read".into()], &[], &[], false));
    }

    #[test]
    fn session_allowlist_skips_approval() {
        assert!(!check_needs_approval("shell", AutonomyLevel::Supervised, &[], &[], &["shell".into()], false));
    }

    #[test]
    fn non_interactive_shell_skips() {
        assert!(!check_needs_approval("shell", AutonomyLevel::Supervised, &[], &[], &[], true));
    }

    #[test]
    fn non_interactive_non_shell_still_asks() {
        assert!(check_needs_approval("file_write", AutonomyLevel::Supervised, &[], &[], &[], true));
    }

    #[test]
    fn session_allowlist_respects_always_ask() {
        assert!(!is_session_allowed(
            "shell",
            &["shell".into()],
            &["shell".into()]
        ));
    }

    #[test]
    fn session_allowlist_works_without_always_ask() {
        assert!(is_session_allowed("shell", &["shell".into()], &[]));
    }

    #[test]
    fn session_allowlist_missing_tool() {
        assert!(!is_session_allowed("shell", &[], &[]));
    }

    #[test]
    fn create_request_has_pending_status() {
        let req = create_approval_request(
            "shell",
            "rm -rf /tmp",
            ApprovalRisk::High,
            ApprovalOrigin::Runtime,
            Some("web:abc:123"),
            Some("run-1"),
        );
        assert_eq!(req.status, ApprovalStatus::Pending);
        assert_eq!(req.tool_name, "shell");
        assert!(!req.id.is_empty());
    }
}
