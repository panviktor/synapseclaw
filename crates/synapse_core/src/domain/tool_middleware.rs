//! Tool middleware domain types.
//!
//! Phase 4.1 Slice 3: types for intercepting tool calls with before/after hooks.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Reason a tool call was blocked by middleware.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolBlock {
    /// Too many calls in the time window.
    RateLimited {
        tool: String,
        limit: u32,
        window_secs: u64,
    },
    /// Input validation failed.
    ValidationFailed { tool: String, reason: String },
    /// Human approval required before execution.
    ApprovalRequired { tool: String, prompt: String },
    /// Explicitly denied by policy.
    Denied { tool: String, reason: String },
}

impl fmt::Display for ToolBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited {
                tool,
                limit,
                window_secs,
            } => write!(
                f,
                "tool '{tool}' rate-limited: max {limit} calls per {window_secs}s"
            ),
            Self::ValidationFailed { tool, reason } => {
                write!(f, "tool '{tool}' validation failed: {reason}")
            }
            Self::ApprovalRequired { tool, prompt } => {
                write!(f, "tool '{tool}' requires approval: {prompt}")
            }
            Self::Denied { tool, reason } => {
                write!(f, "tool '{tool}' denied: {reason}")
            }
        }
    }
}

/// Context passed to middleware hooks.
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    /// Pipeline run ID (if in a pipeline).
    pub run_id: Option<String>,
    /// Pipeline name (if in a pipeline).
    pub pipeline_name: Option<String>,
    /// Pipeline step ID (if in a pipeline).
    pub step_id: Option<String>,
    /// Agent executing the tool.
    pub agent_id: String,
    /// Tool being called.
    pub tool_name: String,
    /// Tool arguments (JSON).
    pub args: Value,
    /// How many times this tool has been called in this run.
    pub call_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_block_display() {
        let b = ToolBlock::RateLimited {
            tool: "web_search".into(),
            limit: 5,
            window_secs: 60,
        };
        assert!(b.to_string().contains("web_search"));
        assert!(b.to_string().contains("5"));

        let b2 = ToolBlock::Denied {
            tool: "shell".into(),
            reason: "not allowed".into(),
        };
        assert!(b2.to_string().contains("denied"));
    }

    #[test]
    fn tool_block_serialization() {
        let b = ToolBlock::RateLimited {
            tool: "search".into(),
            limit: 10,
            window_secs: 60,
        };
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("rate_limited"));
        let b2: ToolBlock = serde_json::from_str(&json).unwrap();
        assert!(matches!(b2, ToolBlock::RateLimited { limit: 10, .. }));
    }
}
