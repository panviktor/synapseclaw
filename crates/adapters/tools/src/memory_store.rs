use super::traits::{Tool, ToolResult};
use crate::memory_facts;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::config::ToolOperation;
use synapse_domain::domain::memory::MemoryCategory;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::tool::{
    ToolArgumentPolicy, ToolContract, ToolExecution, ToolNonReplayableReason, ToolRuntimeRole,
};

/// Let the agent store memories — its own brain writes
pub struct MemoryStoreTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    security: Arc<SecurityPolicy>,
}

impl MemoryStoreTool {
    pub fn new(memory: Arc<dyn UnifiedMemoryPort>, security: Arc<SecurityPolicy>) -> Self {
        Self { memory, security }
    }

    async fn execute_action(&self, args: &serde_json::Value) -> anyhow::Result<ToolExecution> {
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'key' parameter"))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter"))?;

        let category = match args.get("category").and_then(|v| v.as_str()) {
            Some("core") | None => MemoryCategory::Core,
            Some("daily") => MemoryCategory::Daily,
            Some("conversation") => MemoryCategory::Conversation,
            Some(other) => MemoryCategory::Custom(other.to_string()),
        };

        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "memory_store")
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

        match self.memory.store(key, content, &category, None).await {
            Ok(()) => Ok(ToolExecution {
                result: ToolResult {
                    success: true,
                    output: format!("Stored memory: {key}"),
                    error: None,
                },
                facts: vec![memory_facts::build_memory_entry_fact(
                    self.name(),
                    "store",
                    key,
                    Some(&category),
                )],
            }),
            Err(e) => Ok(ToolExecution {
                result: ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to store memory: {e}")),
                },
                facts: Vec::new(),
            }),
        }
    }
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn description(&self) -> &str {
        "Store a fact, preference, or note in long-term memory. Use category 'core' for permanent facts, 'daily' for session notes, 'conversation' for chat context, or a custom category name."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Unique key for this memory (e.g. 'user_lang', 'project_stack')"
                },
                "content": {
                    "type": "string",
                    "description": "The information to remember"
                },
                "category": {
                    "type": "string",
                    "description": "Memory category: 'core' (permanent), 'daily' (session), 'conversation' (chat), or a custom category name. Defaults to 'core'."
                }
            },
            "required": ["key", "content"]
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::MemoryMutation)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(self.runtime_role(), ToolNonReplayableReason::MutatesState)
            .with_arguments(vec![
                ToolArgumentPolicy::sensitive("key").user_private(),
                ToolArgumentPolicy::sensitive("content").user_private(),
                ToolArgumentPolicy::replayable("category").with_values([
                    "core",
                    "daily",
                    "conversation",
                ]),
            ])
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action(&args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.execute_action(&args).await
    }
}
