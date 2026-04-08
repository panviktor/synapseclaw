//! Adapter: wraps `run_tool_call_loop` as AgentRuntimePort.
//!
//! Since `synapse_providers::ChatMessage` is now a re-export of
//! `synapse_domain::domain::message::ChatMessage`, no conversions are needed.

use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::ports::agent_runtime::{AgentRuntimePort, AgentTurnResult};
use synapse_infra::approval::ApprovalManager;
use synapse_providers::{ChatMessage, Provider};

/// Wraps the existing agent loop infrastructure behind `AgentRuntimePort`.
pub struct ChannelAgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub tools_registry: Arc<Vec<Box<dyn Tool>>>,
    pub observer: Arc<dyn synapse_observability::Observer>,
    pub approval_manager: Arc<ApprovalManager>,
    pub channel_name: String,
    pub multimodal: synapse_domain::config::schema::MultimodalConfig,
    pub excluded_tools: Arc<Vec<String>>,
    pub dedup_exempt_tools: Arc<Vec<String>>,
    pub hooks: Option<Arc<crate::hooks::HookRunner>>,
    pub activated_tools: Option<Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
    pub message_timeout_secs: u64,
    pub max_tool_iterations: usize,
}

#[async_trait]
impl AgentRuntimePort for ChannelAgentRuntime {
    async fn execute_turn(
        &self,
        mut history: Vec<ChatMessage>,
        provider_name: &str,
        model: &str,
        temperature: f64,
        max_iterations: usize,
        timeout_secs: u64,
        on_delta: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> Result<AgentTurnResult> {
        // Compute timeout budget (scale by max iterations)
        let iterations = max_iterations.max(1) as u64;
        let scale = iterations.min(5);
        let budget_secs = if timeout_secs > 0 {
            timeout_secs.saturating_mul(scale)
        } else {
            0
        };

        let fut = Box::pin(crate::agent::loop_::run_tool_call_loop(
            self.provider.as_ref(),
            &mut history,
            &self.tools_registry,
            self.observer.as_ref(),
            provider_name,
            model,
            temperature,
            true, // silent (channel mode)
            Some(&*self.approval_manager as &dyn synapse_domain::ports::approval::ApprovalPort),
            &self.channel_name,
            &self.multimodal,
            max_iterations,
            None,     // cancellation_token
            on_delta, // streaming deltas
            self.hooks.as_deref(),
            &self.excluded_tools,
            &self.dedup_exempt_tools,
            self.activated_tools.as_ref(),
            None, // run_ctx
        ));

        // Apply timeout if configured
        let loop_result = if budget_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(budget_secs), fut).await {
                Ok(result) => result?,
                Err(_) => {
                    anyhow::bail!("Agent execution timed out after {budget_secs}s");
                }
            }
        } else {
            fut.await?
        };

        let response = loop_result.response;
        let tool_names = loop_result.tool_names;
        let tool_facts = loop_result.tool_facts;
        let tools_used = !tool_names.is_empty();
        let tool_summary = format_tool_summary(&tool_names);

        Ok(AgentTurnResult {
            response,
            history,
            tools_used,
            tool_names,
            tool_facts,
            tool_summary,
        })
    }

    fn supports_vision(&self) -> bool {
        self.provider.supports_vision()
    }
}

fn format_tool_summary(tool_names: &[String]) -> String {
    if tool_names.is_empty() {
        String::new()
    } else {
        format!("[Used tools: {}]", tool_names.join(", "))
    }
}
