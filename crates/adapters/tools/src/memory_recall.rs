use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;
use synapse_domain::domain::memory::MemoryQuery;
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::domain::tool_fact::TypedToolFact;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::tool::ToolExecution;

fn is_autosave_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    normalized == "assistant_resp" || normalized.starts_with("assistant_resp_")
}

/// Let the agent search its own memory
pub struct MemoryRecallTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    agent_id: String,
}

impl MemoryRecallTool {
    pub fn new(memory: Arc<dyn UnifiedMemoryPort>, agent_id: String) -> Self {
        Self { memory, agent_id }
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

        let search_limit = limit.saturating_mul(3).max(limit.saturating_add(4));
        let result = self
            .memory
            .hybrid_search(&MemoryQuery {
                text: query.to_string(),
                embedding: None,
                agent_id: self.agent_id.clone(),
                categories: Vec::new(),
                include_shared: true,
                time_range: None,
                limit: search_limit,
            })
            .await;

        match result {
            Ok(result) => {
                let entries = result
                    .episodes
                    .into_iter()
                    .filter_map(|scored| {
                        let mut entry = scored.entry;
                        if entry.key.trim().is_empty()
                            || is_autosave_key(&entry.key)
                            || synapse_domain::domain::util::should_skip_autosave_content(
                                &entry.content,
                            )
                            || entry.content.contains("<tool_result")
                        {
                            return None;
                        }
                        entry.score = Some(scored.score as f64);
                        Some(
                            synapse_domain::application::services::retrieval_service::MemorySearchMatch {
                                entry,
                            },
                        )
                    })
                    .take(limit)
                    .collect::<Vec<_>>();

                if entries.is_empty() {
                    return Ok((
                        ToolResult {
                            success: true,
                            output: "No memories found matching that query.".into(),
                            error: None,
                        },
                        entries,
                    ));
                }

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
    ) -> Vec<TypedToolFact> {
        if entries.is_empty() {
            return Vec::new();
        }

        vec![TypedToolFact::focus(
            self.name().to_string(),
            entries
                .iter()
                .take(3)
                .map(|hit| FocusEntity {
                    kind: hit.entry.category.to_string(),
                    name: hit.entry.key.clone(),
                    metadata: Some(hit.entry.content.chars().take(120).collect()),
                })
                .collect(),
            Vec::new(),
        )]
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
