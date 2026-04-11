use super::traits::{Tool, ToolResult};
use crate::memory_facts;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::config::ToolOperation;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::tool::ToolExecution;

/// Let the agent forget/delete a memory entry
pub struct MemoryForgetTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    security: Arc<SecurityPolicy>,
    agent_id: String,
}

impl MemoryForgetTool {
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

    async fn execute_action(&self, args: &serde_json::Value) -> anyhow::Result<ToolExecution> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'key' parameter"))?;

        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "memory_forget")
        {
            return Ok(ToolExecution {
                result: ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(error),
                },
                facts: Vec::new(),
            });
        }

        match self.memory.forget(key, &self.agent_id).await {
            Ok(true) => Ok(ToolExecution {
                result: ToolResult {
                    success: true,
                    output: format!("Forgot memory: {key}"),
                    error: None,
                },
                facts: vec![memory_facts::build_memory_entry_fact(
                    self.name(),
                    "forget",
                    key,
                    None,
                )],
            }),
            Ok(false) => Ok(ToolExecution {
                result: ToolResult {
                    success: true,
                    output: format!("No memory found with key: {key}"),
                    error: None,
                },
                facts: Vec::new(),
            }),
            Err(e) => Ok(ToolExecution {
                result: ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to forget memory: {e}")),
                },
                facts: Vec::new(),
            }),
        }
    }
}

#[async_trait]
impl Tool for MemoryForgetTool {
    fn name(&self) -> &str {
        "memory_forget"
    }

    fn description(&self) -> &str {
        "Remove a memory by key. Use to delete outdated facts or sensitive data. Returns whether the memory was found and removed."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key of the memory to forget"
                }
            },
            "required": ["key"]
        })
    }

    fn runtime_role(&self) -> Option<synapse_domain::ports::tool::ToolRuntimeRole> {
        Some(synapse_domain::ports::tool::ToolRuntimeRole::MemoryMutation)
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action(&args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.execute_action(&args).await
    }
}
