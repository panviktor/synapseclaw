//! Knowledge management tool — Phase 4.3 stub.
//!
//! The old SQLite-based KnowledgeGraph has been replaced by SemanticMemoryPort
//! (entities + bitemporal facts in SurrealDB). This tool will be rewritten
//! to use SemanticMemoryPort in a follow-up slice.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::ports::memory::UnifiedMemoryPort;

/// Tool for managing a knowledge graph (Phase 4.3: uses SemanticMemoryPort).
pub struct KnowledgeTool {
    memory: Arc<dyn UnifiedMemoryPort>,
}

impl KnowledgeTool {
    pub fn new(memory: Arc<dyn UnifiedMemoryPort>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for KnowledgeTool {
    fn name(&self) -> &str {
        "knowledge"
    }

    fn description(&self) -> &str {
        "Manage the knowledge graph: search entities, add facts, explore relationships. (Phase 4.3: being migrated to SurrealDB)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "add_entity", "add_fact"],
                    "description": "Action to perform"
                },
                "query": {
                    "type": "string",
                    "description": "Search query or entity name"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("search");

        match action {
            "search" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                match self.memory.find_entity(query).await {
                    Ok(Some(entity)) => Ok(ToolResult {
                        success: true,
                        output: format!(
                            "Found entity: {} ({})\nSummary: {}",
                            entity.name,
                            entity.entity_type,
                            entity.summary.as_deref().unwrap_or("none")
                        ),
                        error: None,
                    }),
                    Ok(None) => Ok(ToolResult {
                        success: true,
                        output: format!("No entity found matching: {query}"),
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Search failed: {e}")),
                    }),
                }
            }
            _ => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Action '{action}' not yet implemented in Phase 4.3 stub"
                )),
            }),
        }
    }
}
