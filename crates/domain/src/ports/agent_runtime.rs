//! Port: agent runtime — execute LLM turns + tool loops.
//!
//! Abstracts the `agent::run()` / `run_tool_call_loop()` infrastructure
//! so the application core can orchestrate without depending on concrete
//! provider implementations.

use crate::domain::dialogue_state::{DialogueSlot, FocusEntity};
use crate::domain::message::ChatMessage;
use anyhow::Result;
use async_trait::async_trait;

/// Structured facts extracted from a tool invocation.
#[derive(Debug, Clone, Default)]
pub struct AgentToolFact {
    /// Tool name that produced the fact.
    pub tool_name: String,
    /// Entities surfaced by the tool arguments.
    pub focus_entities: Vec<FocusEntity>,
    /// Structured slots surfaced by the tool arguments.
    pub slots: Vec<DialogueSlot>,
}

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
    /// Structured tool facts extracted from tool-call arguments.
    pub tool_facts: Vec<AgentToolFact>,
    /// Extracted tool context summary (for history display).
    pub tool_summary: String,
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
    ) -> Result<AgentTurnResult>;

    /// Check if a provider supports vision/multimodal.
    fn supports_vision(&self) -> bool {
        false
    }
}
