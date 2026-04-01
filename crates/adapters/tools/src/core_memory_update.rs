//! Core memory update tool — MemGPT pattern.
//!
//! Allows the agent to edit its own core memory blocks that are
//! always present in the system prompt. Labels:
//! - `persona`: agent identity and behavior
//! - `user_knowledge`: what the agent knows about the user
//! - `task_state`: current task context
//! - `domain`: domain expertise

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::config::ToolOperation;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::memory::UnifiedMemoryPort;

/// Tool for agents to edit their core memory blocks (always in prompt).
pub struct CoreMemoryUpdateTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    security: Arc<SecurityPolicy>,
    agent_id: String,
}

impl CoreMemoryUpdateTool {
    pub fn new(
        memory: Arc<dyn UnifiedMemoryPort>,
        security: Arc<SecurityPolicy>,
        agent_id: String,
    ) -> Self {
        Self {
            memory,
            security,
            agent_id,
        }
    }
}

#[async_trait]
impl Tool for CoreMemoryUpdateTool {
    fn name(&self) -> &str {
        "core_memory_update"
    }

    fn description(&self) -> &str {
        "Update your core memory blocks. These blocks are ALWAYS present in your context. \
         Use 'persona' for your identity/behavior, 'user_knowledge' for what you know about \
         the user, 'task_state' for current task context, 'domain' for domain expertise. \
         Action 'replace' overwrites the block; 'append' adds to it."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "label": {
                    "type": "string",
                    "enum": ["persona", "user_knowledge", "task_state", "domain"],
                    "description": "Which core memory block to update"
                },
                "action": {
                    "type": "string",
                    "enum": ["replace", "append"],
                    "description": "Whether to replace the entire block or append to it"
                },
                "content": {
                    "type": "string",
                    "description": "The new content (for replace) or text to append"
                }
            },
            "required": ["label", "action", "content"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'label' parameter"))?;

        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter"))?;

        // Validate label
        if !["persona", "user_knowledge", "task_state", "domain"].contains(&label) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Invalid label '{label}'. Must be: persona, user_knowledge, task_state, domain"
                )),
            });
        }

        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "core_memory_update")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let result = match action {
            "replace" => {
                self.memory
                    .update_core_block(&self.agent_id, label, content.to_string())
                    .await
            }
            "append" => {
                self.memory
                    .append_core_block(&self.agent_id, label, content)
                    .await
            }
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Invalid action '{action}'. Must be: replace, append"
                    )),
                });
            }
        };

        match result {
            Ok(()) => Ok(ToolResult {
                success: true,
                output: format!("Core memory '{label}' updated ({action})"),
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to update core memory: {e}")),
            }),
        }
    }
}
