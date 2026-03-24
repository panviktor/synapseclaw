//! Adapter: wraps existing `ApprovalManager` as `ApprovalPort`.

use crate::approval::ApprovalManager;
use crate::fork_core::domain::approval::{ApprovalDecision, ApprovalResponse};
use crate::fork_core::ports::approval::ApprovalPort;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Wraps the existing `ApprovalManager` behind `ApprovalPort`.
pub struct ApprovalManagerAdapter {
    manager: Arc<ApprovalManager>,
}

impl ApprovalManagerAdapter {
    pub fn new(manager: Arc<ApprovalManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ApprovalPort for ApprovalManagerAdapter {
    fn needs_approval(&self, tool_name: &str) -> bool {
        self.manager.needs_approval(tool_name)
    }

    async fn request_approval(
        &self,
        tool_name: &str,
        _arguments: &str,
    ) -> Result<ApprovalResponse> {
        if self.manager.is_non_interactive() {
            return Ok(ApprovalResponse::No);
        }
        // CLI interactive path uses prompt_cli — not reachable through this adapter
        // (channels always use non-interactive)
        Ok(ApprovalResponse::No)
    }

    fn record_decision(&self, decision: &ApprovalDecision) {
        let args = serde_json::json!({"summary": decision.request_id});
        let response = match decision.response {
            ApprovalResponse::Yes => crate::approval::ApprovalResponse::Yes,
            ApprovalResponse::No => crate::approval::ApprovalResponse::No,
            ApprovalResponse::Always => crate::approval::ApprovalResponse::Always,
        };
        self.manager
            .record_decision(&decision.request_id, &args, response, &decision.channel);
    }

    fn is_session_allowed(&self, tool_name: &str) -> bool {
        self.manager.session_allowlist().contains(tool_name)
    }

    fn add_session_allowlist(&self, tool_name: &str) {
        // record_decision with Always adds to allowlist internally
        let args = serde_json::json!({});
        self.manager.record_decision(
            tool_name,
            &args,
            crate::approval::ApprovalResponse::Always,
            "session",
        );
    }
}
