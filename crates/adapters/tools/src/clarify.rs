//! Clarify tool — structured clarification requests.
//!
//! Instead of free-form "which city?", the agent uses this tool to present
//! a structured question with optional choices and a recommendation.
//! The tool returns formatted output that the agent includes in its response.

use async_trait::async_trait;
use serde_json::json;
use synapse_domain::ports::tool::{Tool, ToolResult};

pub struct ClarifyTool;

impl ClarifyTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ClarifyTool {
    fn name(&self) -> &str {
        "clarify"
    }

    fn description(&self) -> &str {
        "Ask the user a structured clarifying question before proceeding. \
         Use this instead of guessing when critical information is missing. \
         Supports optional multiple-choice options and a recommendation."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The clarifying question to ask"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of choices (max 5). Omit for open-ended questions."
                },
                "recommendation": {
                    "type": "string",
                    "description": "Optional recommended choice or default suggestion"
                },
                "context": {
                    "type": "string",
                    "description": "Why this clarification is needed (shown to user)"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("Could you clarify?");

        let options: Vec<&str> = args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).take(5).collect())
            .unwrap_or_default();

        let recommendation = args.get("recommendation").and_then(|v| v.as_str());
        let context = args.get("context").and_then(|v| v.as_str());

        let mut output = String::new();

        if let Some(ctx) = context {
            output.push_str(&format!("*{ctx}*\n\n"));
        }

        output.push_str(&format!("**{question}**\n"));

        if !options.is_empty() {
            output.push('\n');
            for (i, opt) in options.iter().enumerate() {
                output.push_str(&format!("{}. {opt}\n", i + 1));
            }
        }

        if let Some(rec) = recommendation {
            output.push_str(&format!("\n💡 Suggestion: {rec}"));
        }

        Ok(ToolResult {
            output,
            success: true,
            error: None,
        })
    }
}
