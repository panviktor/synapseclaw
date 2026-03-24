//! Adapter: wraps `run_tool_call_loop` as AgentRuntimePort.

use crate::fork_core::ports::agent_runtime::{AgentRuntimePort, AgentTurnResult};
use crate::providers::{ChatMessage, Provider};
use crate::approval::ApprovalManager;
use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Wraps the existing agent loop infrastructure behind `AgentRuntimePort`.
///
/// Holds all the shared state that `run_tool_call_loop` needs.
pub struct ChannelAgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub tools_registry: Arc<Vec<Box<dyn Tool>>>,
    pub observer: Arc<dyn crate::observability::Observer>,
    pub approval_manager: Arc<ApprovalManager>,
    pub channel_name: String,
    pub multimodal: crate::config::MultimodalConfig,
    pub excluded_tools: Arc<Vec<String>>,
    pub dedup_exempt_tools: Arc<Vec<String>>,
    pub hooks: Option<Arc<crate::hooks::HookRunner>>,
    pub activated_tools: Option<Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
}

#[async_trait]
impl AgentRuntimePort for ChannelAgentRuntime {
    async fn execute_turn(
        &self,
        mut history: Vec<ChatMessage>,
        _provider_name: &str,
        _model: &str,
        temperature: f64,
        max_iterations: usize,
    ) -> Result<AgentTurnResult> {
        let history_before = history.len();

        let response = Box::pin(crate::agent::loop_::run_tool_call_loop(
            self.provider.as_ref(),
            &mut history,
            &self.tools_registry,
            self.observer.as_ref(),
            _provider_name,
            _model,
            temperature,
            true,  // silent (channel mode)
            Some(&*self.approval_manager),
            &self.channel_name,
            &self.multimodal,
            max_iterations,
            None, // cancellation_token — managed by dispatch loop
            None, // on_delta — streaming handled separately
            self.hooks.as_deref(),
            &self.excluded_tools,
            &self.dedup_exempt_tools,
            self.activated_tools.as_ref(),
            None, // run_ctx
        ))
        .await?;

        let tools_used = history.len() > history_before + 1; // more than just the assistant turn

        Ok(AgentTurnResult {
            response,
            history,
            tools_used,
        })
    }
}
