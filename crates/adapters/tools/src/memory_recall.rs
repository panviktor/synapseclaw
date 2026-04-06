use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;
use synapse_domain::application::services::retrieval_service;
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::ports::agent_runtime::AgentToolFact;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::tool::ToolExecution;

/// Let the agent search its own memory
pub struct MemoryRecallTool {
    memory: Arc<dyn UnifiedMemoryPort>,
}

impl MemoryRecallTool {
    pub fn new(memory: Arc<dyn UnifiedMemoryPort>) -> Self {
        Self { memory }
    }

    async fn execute_query(
        &self,
        args: &serde_json::Value,
    ) -> anyhow::Result<(
        ToolResult,
        Vec<synapse_domain::application::services::retrieval_service::MemorySearchMatch>,
    )> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;

        #[allow(clippy::cast_possible_truncation)]
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(5, |v| v as usize);

        match retrieval_service::search_memory(self.memory.as_ref(), query, limit, None).await {
            Ok(entries) if entries.is_empty() => Ok((
                ToolResult {
                    success: true,
                    output: "No memories found matching that query.".into(),
                    error: None,
                },
                entries,
            )),
            Ok(entries) => {
                let mut output = format!("Found {} memories:\n", entries.len());
                for hit in &entries {
                    let entry = &hit.entry;
                    let score = entry
                        .score
                        .map_or_else(String::new, |s| format!(" [{s:.0}%]"));
                    let _ = writeln!(
                        output,
                        "- [{}] {}: {}{score}",
                        entry.category, entry.key, entry.content
                    );
                }
                Ok((
                    ToolResult {
                        success: true,
                        output,
                        error: None,
                    },
                    entries,
                ))
            }
            Err(e) => Ok((
                ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Memory recall failed: {e}")),
                },
                Vec::new(),
            )),
        }
    }

    fn build_result_facts(
        &self,
        entries: &[synapse_domain::application::services::retrieval_service::MemorySearchMatch],
    ) -> Vec<AgentToolFact> {
        if entries.is_empty() {
            return Vec::new();
        }

        vec![AgentToolFact {
            tool_name: self.name().to_string(),
            focus_entities: entries
                .iter()
                .take(3)
                .map(|hit| FocusEntity {
                    kind: hit.entry.category.to_string(),
                    name: hit.entry.key.clone(),
                    metadata: Some(hit.entry.content.chars().take(120).collect()),
                })
                .collect(),
            slots: Vec::new(),
        }]
    }
}

#[async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &str {
        "memory_recall"
    }

    fn description(&self) -> &str {
        "Search long-term memory for relevant facts, preferences, or context. Returns scored results ranked by relevance."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords or phrase to search for in memory"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results to return (default: 5)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let (result, _) = self.execute_query(&args).await?;
        Ok(result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        let (result, entries) = self.execute_query(&args).await?;
        Ok(ToolExecution {
            result,
            facts: self.build_result_facts(&entries),
        })
    }
}
