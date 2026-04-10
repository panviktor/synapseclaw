//! Port: agent runtime — execute LLM turns + tool execution loops.
//!
//! Abstracts the `agent::run()` / `run_tool_call_loop()` infrastructure
//! so the application core can orchestrate without depending on concrete
//! provider implementations.

use crate::domain::message::ChatMessage;
use crate::domain::tool_fact::TypedToolFact;
use crate::domain::tool_repair::ToolRepairTrace;
use crate::ports::provider::ProviderCapabilities;
use async_trait::async_trait;

/// Result of an agent execution turn.
#[derive(Debug, Clone)]
pub struct AgentTurnResult {
    /// The final assistant response text.
    pub response: String,
    /// Updated conversation history (includes tool call/result turns).
    pub history: Vec<ChatMessage>,
    /// Whether tools were executed during this turn.
    pub tools_used: bool,
    /// Structured tool names used during this turn.
    pub tool_names: Vec<String>,
    /// Structured tool facts emitted during the turn.
    pub tool_facts: Vec<TypedToolFact>,
    /// Extracted tool context summary (for history display).
    pub tool_summary: String,
    /// Most recent structured tool self-repair trace emitted during the turn.
    pub last_tool_repair: Option<ToolRepairTrace>,
    /// Distinct structured tool self-repair traces emitted during the turn.
    pub tool_repairs: Vec<ToolRepairTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRuntimeErrorKind {
    Timeout,
    ContextLimitExceeded,
    CapabilityMismatch,
    AuthFailure,
    RuntimeFailure,
}

#[derive(Debug, Clone)]
pub struct AgentRuntimeError {
    pub kind: AgentRuntimeErrorKind,
    pub detail: String,
}

impl AgentRuntimeError {
    pub fn new(kind: AgentRuntimeErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for AgentRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{:?}", self.kind)
        } else {
            write!(
                f,
                "{}: {}",
                match self.kind {
                    AgentRuntimeErrorKind::Timeout => "timeout",
                    AgentRuntimeErrorKind::ContextLimitExceeded => "context_limit_exceeded",
                    AgentRuntimeErrorKind::CapabilityMismatch => "capability_mismatch",
                    AgentRuntimeErrorKind::AuthFailure => "auth_failure",
                    AgentRuntimeErrorKind::RuntimeFailure => "runtime_failure",
                },
                self.detail
            )
        }
    }
}

impl std::error::Error for AgentRuntimeError {}

/// Port for executing agent turns (LLM + tool loop).
#[async_trait]
pub trait AgentRuntimePort: Send + Sync {
    /// Execute one agent turn: send history to LLM, run tool loop, return result.
    ///
    /// Parameters:
    /// - `history`: conversation history up to this point
    /// - `provider_name`: which provider to use
    /// - `model`: which model to use
    /// - `temperature`: sampling temperature
    /// - `max_iterations`: tool loop iteration cap
    /// - `timeout_secs`: hard timeout for the entire turn (0 = no timeout)
    /// - `on_delta`: optional channel for streaming deltas
    ///
    /// Returns the final response and updated history.
    async fn execute_turn(
        &self,
        history: Vec<ChatMessage>,
        provider_name: &str,
        model: &str,
        temperature: f64,
        max_iterations: usize,
        timeout_secs: u64,
        on_delta: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> std::result::Result<AgentTurnResult, AgentRuntimeError>;

    /// Return provider capability metadata for the requested provider route.
    ///
    /// Adapters that can switch providers per-turn should override this to
    /// resolve the effective provider rather than reporting only startup-time
    /// defaults.
    fn capabilities_for(&self, _provider_name: &str) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Check if a specific provider route supports vision/multimodal input.
    fn supports_vision_for(&self, provider_name: &str) -> bool {
        self.capabilities_for(provider_name).vision
    }

    /// Check if a specific provider+model route supports vision/multimodal input.
    ///
    /// Adapters may use model-profile metadata here because capabilities can differ
    /// for the same provider across models or gateways.
    fn supports_vision_for_route(&self, provider_name: &str, _model: &str) -> bool {
        self.supports_vision_for(provider_name)
    }

    /// Check if a provider supports vision/multimodal.
    fn supports_vision(&self) -> bool {
        self.supports_vision_for("")
    }
}
