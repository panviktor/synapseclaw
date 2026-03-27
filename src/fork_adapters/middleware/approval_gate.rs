//! Approval gate middleware — human-in-the-loop for dangerous tools.
//!
//! Phase 4.1 Slice 3: blocks specified tools until human approval is received.
//! Integrates with existing ApprovalPort from Phase 4.0.

use async_trait::async_trait;
use fork_core::domain::tool_middleware::{ToolBlock, ToolCallContext};
use fork_core::ports::tool_middleware::ToolMiddlewarePort;
use serde_json::Value;
use std::collections::HashSet;

/// Approval gate middleware: requires human approval for specified tools.
///
/// Tools in the `gated_tools` set will be blocked with
/// `ToolBlock::ApprovalRequired`. The pipeline runner or agent loop
/// can then request approval via `ApprovalPort` and retry.
pub struct ApprovalGateMiddleware {
    /// Tools that require approval before execution.
    gated_tools: HashSet<String>,
    /// Prompt template: {tool} is replaced with the tool name.
    prompt_template: String,
    /// Tools that have been approved (run_id:tool_name).
    approved: std::sync::Mutex<HashSet<String>>,
}

impl ApprovalGateMiddleware {
    /// Create with a set of gated tools.
    pub fn new(gated_tools: HashSet<String>) -> Self {
        Self {
            gated_tools,
            prompt_template: "Tool '{tool}' requires human approval to execute.".into(),
            approved: std::sync::Mutex::new(HashSet::new()),
        }
    }

    /// Set a custom prompt template ({tool} is replaced).
    pub fn with_prompt(mut self, template: String) -> Self {
        self.prompt_template = template;
        self
    }

    /// Mark a tool as approved for a specific run + step.
    pub fn approve(&self, run_id: &str, step_id: &str, tool_name: &str) {
        let key = format!("{run_id}:{step_id}:{tool_name}");
        self.approved.lock().unwrap().insert(key);
    }

    /// Check if a tool has been approved for this run + step.
    fn is_approved(&self, ctx: &ToolCallContext) -> bool {
        let run_id = ctx.run_id.as_deref().unwrap_or(&ctx.agent_id);
        let step_id = ctx.step_id.as_deref().unwrap_or("_");
        let key = format!("{run_id}:{step_id}:{}", ctx.tool_name);
        self.approved.lock().unwrap().contains(&key)
    }
}

#[async_trait]
impl ToolMiddlewarePort for ApprovalGateMiddleware {
    async fn before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock> {
        if !self.gated_tools.contains(&ctx.tool_name) {
            return Ok(());
        }

        if self.is_approved(ctx) {
            return Ok(());
        }

        let prompt = self.prompt_template.replace("{tool}", &ctx.tool_name);

        Err(ToolBlock::ApprovalRequired {
            tool: ctx.tool_name.clone(),
            prompt,
        })
    }

    async fn after(&self, _ctx: &ToolCallContext, _result: &mut Value) -> Result<(), ToolBlock> {
        Ok(())
    }

    fn name(&self) -> &str {
        "approval_gate"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(tool: &str, run_id: &str) -> ToolCallContext {
        ToolCallContext {
            run_id: Some(run_id.into()),
            pipeline_name: None,
            step_id: Some("step1".into()),
            agent_id: "agent".into(),
            tool_name: tool.into(),
            args: json!({}),
            call_count: 0,
        }
    }

    #[tokio::test]
    async fn ungated_tool_passes() {
        let mw = ApprovalGateMiddleware::new(HashSet::from(["shell".into()]));
        assert!(mw.before(&ctx("web_search", "run-1")).await.is_ok());
    }

    #[tokio::test]
    async fn gated_tool_blocks() {
        let mw = ApprovalGateMiddleware::new(HashSet::from(["shell".into()]));
        let err = mw.before(&ctx("shell", "run-1")).await.unwrap_err();
        assert!(matches!(err, ToolBlock::ApprovalRequired { .. }));
    }

    #[tokio::test]
    async fn approved_tool_passes() {
        let mw = ApprovalGateMiddleware::new(HashSet::from(["shell".into()]));
        mw.approve("run-1", "step1", "shell");
        assert!(mw.before(&ctx("shell", "run-1")).await.is_ok());
    }

    #[tokio::test]
    async fn approval_is_per_run_and_step() {
        let mw = ApprovalGateMiddleware::new(HashSet::from(["shell".into()]));
        mw.approve("run-1", "step1", "shell");

        // run-1:step1 approved
        assert!(mw.before(&ctx("shell", "run-1")).await.is_ok());
        // run-2:step1 NOT approved
        assert!(mw.before(&ctx("shell", "run-2")).await.is_err());

        // run-1:step2 NOT approved (different step)
        let ctx_step2 = ToolCallContext {
            run_id: Some("run-1".into()),
            pipeline_name: None,
            step_id: Some("step2".into()),
            agent_id: "agent".into(),
            tool_name: "shell".into(),
            args: json!({}),
            call_count: 0,
        };
        assert!(mw.before(&ctx_step2).await.is_err());
    }

    #[tokio::test]
    async fn custom_prompt() {
        let mw = ApprovalGateMiddleware::new(HashSet::from(["deploy".into()]))
            .with_prompt("Are you sure you want to run {tool}?".into());

        let err = mw.before(&ctx("deploy", "run-1")).await.unwrap_err();
        if let ToolBlock::ApprovalRequired { prompt, .. } = err {
            assert_eq!(prompt, "Are you sure you want to run deploy?");
        } else {
            panic!("expected ApprovalRequired");
        }
    }
}
