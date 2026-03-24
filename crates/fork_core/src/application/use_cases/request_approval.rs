//! Use case: RequestApproval — request operator approval for a tool action.
//!
//! Phase 4.0 Slice 4.

use crate::application::services::approval_service::{self, AutonomyLevel};
use crate::domain::approval::{
    ApprovalDecision, ApprovalOrigin, ApprovalRequest, ApprovalResponse, ApprovalRisk,
};
use crate::ports::approval::ApprovalPort;
use anyhow::Result;

/// Execute the approval workflow for a tool call.
///
/// Returns the approval response (Yes/No/Always) after checking:
/// 1. Session allowlist (instant approve if previously "Always")
/// 2. Auto-approve config (instant approve)
/// 3. Delegate to ApprovalPort for interactive/non-interactive decision
pub async fn execute(
    port: &dyn ApprovalPort,
    tool_name: &str,
    arguments: &str,
) -> Result<ApprovalResponse> {
    // Port.needs_approval() delegates to approval_service::check_needs_approval
    // with full context (autonomy, auto_approve, always_ask, session_allowlist, non_interactive)
    if !port.needs_approval(tool_name) {
        return Ok(ApprovalResponse::Yes);
    }

    // Request approval via port (interactive prompt or auto-deny)
    let response = port.request_approval(tool_name, arguments).await?;

    // If "Always", add to session allowlist
    if response == ApprovalResponse::Always {
        port.add_session_allowlist(tool_name);
    }

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockApprovalPort {
        response: ApprovalResponse,
        needs: bool,
        session_allowed: Mutex<Vec<String>>,
    }

    impl MockApprovalPort {
        fn needs(response: ApprovalResponse) -> Self {
            Self { response, needs: true, session_allowed: Mutex::new(vec![]) }
        }
        fn auto_approved() -> Self {
            Self { response: ApprovalResponse::No, needs: false, session_allowed: Mutex::new(vec![]) }
        }
    }

    #[async_trait]
    impl ApprovalPort for MockApprovalPort {
        fn needs_approval(&self, tool_name: &str) -> bool {
            // Check session allowlist first (mirrors real behavior)
            if self.session_allowed.lock().unwrap().contains(&tool_name.to_string()) {
                return false;
            }
            self.needs
        }
        async fn request_approval(&self, _tool: &str, _args: &str) -> Result<ApprovalResponse> {
            Ok(self.response)
        }
        fn record_decision(&self, _decision: &ApprovalDecision) {}
        fn is_session_allowed(&self, tool_name: &str) -> bool {
            self.session_allowed.lock().unwrap().contains(&tool_name.to_string())
        }
        fn add_session_allowlist(&self, tool_name: &str) {
            self.session_allowed.lock().unwrap().push(tool_name.to_string());
        }
    }

    #[tokio::test]
    async fn auto_approved_tool_passes() {
        let port = MockApprovalPort::auto_approved();
        let result = execute(&port, "shell", "ls").await.unwrap();
        assert_eq!(result, ApprovalResponse::Yes);
    }

    #[tokio::test]
    async fn supervised_delegates_to_port() {
        let port = MockApprovalPort::needs(ApprovalResponse::Yes);
        let result = execute(&port, "shell", "ls").await.unwrap();
        assert_eq!(result, ApprovalResponse::Yes);
    }

    #[tokio::test]
    async fn supervised_denied_by_port() {
        let port = MockApprovalPort::needs(ApprovalResponse::No);
        let result = execute(&port, "shell", "ls").await.unwrap();
        assert_eq!(result, ApprovalResponse::No);
    }

    #[tokio::test]
    async fn always_adds_to_session() {
        let port = MockApprovalPort::needs(ApprovalResponse::Always);
        let result = execute(&port, "shell", "ls").await.unwrap();
        assert_eq!(result, ApprovalResponse::Always);
        assert!(port.is_session_allowed("shell"));
    }

    #[tokio::test]
    async fn session_allowed_skips_port() {
        let port = MockApprovalPort::needs(ApprovalResponse::No);
        port.add_session_allowlist("shell");
        let result = execute(&port, "shell", "ls").await.unwrap();
        assert_eq!(result, ApprovalResponse::Yes);
    }
}
