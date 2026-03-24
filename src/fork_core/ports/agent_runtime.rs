//! Port: agent runtime — execute LLM turns + tool loops.
//!
//! Abstracts the `agent::run()` / `run_tool_call_loop()` infrastructure
//! so the application core can orchestrate without depending on concrete
//! provider implementations.

use crate::providers::ChatMessage;
use anyhow::Result;
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
}

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
    ///
    /// Returns the final response and updated history.
    async fn execute_turn(
        &self,
        history: Vec<ChatMessage>,
        provider_name: &str,
        model: &str,
        temperature: f64,
        max_iterations: usize,
    ) -> Result<AgentTurnResult>;
}
