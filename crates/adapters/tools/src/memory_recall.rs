use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::domain::memory::MemoryQuery;
use synapse_domain::domain::tool_fact::{SearchDomain, SearchFact, ToolFactPayload, TypedToolFact};
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
}

fn build_result_facts(
    tool_name: &str,
    query: Option<&str>,
    entries: &[synapse_domain::application::services::retrieval_service::MemorySearchMatch],
) -> Vec<TypedToolFact> {
    if entries.is_empty() {
        return Vec::new();
    }

    let mut facts = vec![TypedToolFact {
        tool_id: tool_name.to_string(),
        payload: ToolFactPayload::Search(SearchFact {
            domain: SearchDomain::Memory,
            query: query.map(str::to_string),
            result_count: Some(entries.len()),
            primary_locator: entries
                .iter()
                .find_map(|hit| (!hit.entry.key.trim().is_empty()).then(|| hit.entry.key.clone())),
        }),
    }];

    facts.push(TypedToolFact::focus(
        tool_name.to_string(),
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
    ));

    facts
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
        let query = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let (result, entries) = self.execute_query(&args).await?;
        Ok(ToolExecution {
            result,
            facts: build_result_facts(self.name(), query.as_deref(), &entries),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::application::services::retrieval_service::MemorySearchMatch;
    use synapse_domain::domain::memory::{MemoryCategory, MemoryEntry};
    use synapse_domain::domain::tool_fact::ToolFactPayload;

    fn sample_entry(key: &str, content: &str, category: MemoryCategory) -> MemorySearchMatch {
        MemorySearchMatch {
            entry: MemoryEntry {
                id: format!("id-{key}"),
                key: key.to_string(),
                content: content.to_string(),
                category,
                timestamp: "2026-04-08T00:00:00Z".into(),
                session_id: None,
                score: Some(0.91),
            },
        }
    }

    #[test]
    fn build_result_facts_emits_memory_search_and_focus() {
        let entries = vec![sample_entry(
            "atlas_work_chain",
            "project Atlas; branch release/hotfix-17",
            MemoryCategory::Core,
        )];
        let facts = build_result_facts("memory_recall", Some("atlas hotfix"), &entries);

        assert_eq!(facts.len(), 2);
        match &facts[0].payload {
            ToolFactPayload::Search(search) => {
                assert_eq!(search.domain, SearchDomain::Memory);
                assert_eq!(search.query.as_deref(), Some("atlas hotfix"));
                assert_eq!(search.primary_locator.as_deref(), Some("atlas_work_chain"));
                assert_eq!(search.result_count, Some(1));
            }
            payload => panic!("unexpected payload: {payload:?}"),
        }
        assert!(facts[1]
            .focus_entities()
            .iter()
            .any(|entity| entity.kind == "core" && entity.name == "atlas_work_chain"));
    }
}
