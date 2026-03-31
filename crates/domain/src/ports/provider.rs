//! Port: provider value objects — domain-owned types for LLM interaction.
//!
//! These types model the contract between the application core and LLM
//! providers. The `Provider` trait itself stays in `synapse_adapters`
//! because its streaming methods depend on `reqwest` and `futures_util`,
//! but all data types it operates on are domain-owned.

use crate::domain::message::ChatMessage;
use crate::ports::tool::ToolSpec;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

// ── Value Objects ────────────────────────────────────────────────────

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Raw token counts from a single LLM API response.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Tokens served from the provider's prompt cache (Anthropic `cache_read_input_tokens`,
    /// OpenAI `prompt_tokens_details.cached_tokens`).
    pub cached_input_tokens: Option<u64>,
}

/// An LLM response that may contain text, tool calls, or both.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// Text content of the response (may be empty if only tool calls).
    pub text: Option<String>,
    /// Tool calls requested by the LLM.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage reported by the provider, if available.
    pub usage: Option<TokenUsage>,
    /// Raw reasoning/thinking content from thinking models (e.g. DeepSeek-R1,
    /// Kimi K2.5, GLM-4.7). Preserved as an opaque pass-through so it can be
    /// sent back in subsequent API requests — some providers reject tool-call
    /// history that omits this field.
    pub reasoning_content: Option<String>,
}

impl ChatResponse {
    /// True when the LLM wants to invoke at least one tool.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Convenience: return text content or empty string.
    pub fn text_or_empty(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

/// Request payload for provider chat calls.
#[derive(Debug, Clone, Copy)]
pub struct ChatRequest<'a> {
    pub messages: &'a [ChatMessage],
    pub tools: Option<&'a [ToolSpec]>,
}

/// A tool result to feed back to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: String,
}

/// A message in a multi-turn conversation, including tool interactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ConversationMessage {
    /// Regular chat message (system, user, assistant).
    Chat(ChatMessage),
    /// Tool calls from the assistant (stored for history fidelity).
    AssistantToolCalls {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
        /// Raw reasoning content from thinking models, preserved for round-trip
        /// fidelity with provider APIs that require it.
        reasoning_content: Option<String>,
    },
    /// Results of tool executions, fed back to the LLM.
    ToolResults(Vec<ToolResultMessage>),
}

/// Provider capabilities declaration.
///
/// Describes what features a provider supports, enabling intelligent
/// adaptation of tool calling modes and request formatting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Whether the provider supports native tool calling via API primitives.
    pub native_tool_calling: bool,
    /// Whether the provider supports vision / image inputs.
    pub vision: bool,
    /// Whether the provider supports prompt caching.
    pub prompt_caching: bool,
}

/// Structured error returned when a requested capability is not supported.
#[derive(Debug, Clone)]
pub struct ProviderCapabilityError {
    pub provider: String,
    pub capability: String,
    pub message: String,
}

impl std::fmt::Display for ProviderCapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "provider_capability_error provider={} capability={} message={}",
            self.provider, self.capability, self.message
        )
    }
}

impl std::error::Error for ProviderCapabilityError {}

// ── Pure Functions ───────────────────────────────────────────────────

/// Build tool instructions text for prompt-guided tool calling.
///
/// Generates a formatted text block describing available tools and how to
/// invoke them using XML-style tags. This is used as a fallback when the
/// provider doesn't support native tool calling.
pub fn build_tool_instructions_text(tools: &[ToolSpec]) -> String {
    let mut instructions = String::new();

    instructions.push_str("## Tool Use Protocol\n\n");
    instructions.push_str("To use a tool, wrap a JSON object in <tool_call></tool_call> tags:\n\n");
    instructions.push_str("<tool_call>\n");
    instructions.push_str(r#"{"name": "tool_name", "arguments": {"param": "value"}}"#);
    instructions.push_str("\n</tool_call>\n\n");
    instructions.push_str("You may use multiple tool calls in a single response. ");
    instructions.push_str("After tool execution, results appear in <tool_result> tags. ");
    instructions
        .push_str("Continue reasoning with the results until you can give a final answer.\n\n");
    instructions.push_str("### Available Tools\n\n");

    for tool in tools {
        writeln!(&mut instructions, "**{}**: {}", tool.name, tool.description)
            .expect("writing to String cannot fail");

        let parameters =
            serde_json::to_string(&tool.parameters).unwrap_or_else(|_| "{}".to_string());
        writeln!(&mut instructions, "Parameters: `{parameters}`")
            .expect("writing to String cannot fail");
        instructions.push('\n');
    }

    instructions
}
