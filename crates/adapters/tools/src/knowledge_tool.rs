//! Knowledge management tool — Phase 4.3.
//!
//! Exposes the knowledge graph (SemanticMemoryPort) to agents:
//! search entities, add entities, add facts, traverse relationships.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::memory::Entity;
use synapse_domain::domain::tool_fact::{
    KnowledgeAction, KnowledgeFact, ToolFactPayload, TypedToolFact,
};
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::tool::{
    ToolArgumentPolicy, ToolContract, ToolNonReplayableReason, ToolRuntimeRole,
};

/// Tool for managing a knowledge graph via SemanticMemoryPort.
pub struct KnowledgeTool {
    memory: Arc<dyn UnifiedMemoryPort>,
}

impl KnowledgeTool {
    pub fn new(memory: Arc<dyn UnifiedMemoryPort>) -> Self {
        Self { memory }
    }
}

fn build_result_facts(
    tool_name: &str,
    args: &serde_json::Value,
    result: Option<&ToolResult>,
) -> Vec<TypedToolFact> {
    match result {
        Some(result) if result.success => {}
        _ => return Vec::new(),
    }

    let action = match args.get("action").and_then(|value| value.as_str()) {
        Some("search") => KnowledgeAction::Search,
        Some("add_entity") => KnowledgeAction::AddEntity,
        Some("add_fact") => KnowledgeAction::AddFact,
        Some("get_facts") => KnowledgeAction::GetFacts,
        _ => return Vec::new(),
    };

    let query = args
        .get("query")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let subject = args
        .get("subject")
        .and_then(|value| value.as_str())
        .map(str::to_string);

    vec![TypedToolFact {
        tool_id: tool_name.to_string(),
        payload: ToolFactPayload::Knowledge(KnowledgeFact {
            action,
            subject: subject.or(query),
            predicate: args
                .get("predicate")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            object: args
                .get("object")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            entity_type: args
                .get("entity_type")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        }),
    }]
}

#[async_trait]
impl Tool for KnowledgeTool {
    fn name(&self) -> &str {
        "knowledge"
    }

    fn description(&self) -> &str {
        "Manage the knowledge graph: search entities, add entities, add facts, explore relationships."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "add_entity", "add_fact", "get_facts"],
                    "description": "Action to perform"
                },
                "query": {
                    "type": "string",
                    "description": "Search query or entity name"
                },
                "entity_type": {
                    "type": "string",
                    "description": "Type for add_entity: person, company, concept, tool, place, project"
                },
                "summary": {
                    "type": "string",
                    "description": "Description for add_entity"
                },
                "subject": {
                    "type": "string",
                    "description": "Subject entity name for add_fact"
                },
                "predicate": {
                    "type": "string",
                    "description": "Relationship verb for add_fact (e.g. 'works_at', 'prefers')"
                },
                "object": {
                    "type": "string",
                    "description": "Object entity name for add_fact"
                }
            },
            "required": ["action"]
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::HistoricalLookup)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(self.runtime_role(), ToolNonReplayableReason::MutatesState)
            .with_arguments(vec![
                ToolArgumentPolicy::replayable("action").with_values([
                    "search",
                    "add_entity",
                    "add_fact",
                    "get_facts",
                ]),
                ToolArgumentPolicy::sensitive("query").user_private(),
                ToolArgumentPolicy::replayable("entity_type"),
                ToolArgumentPolicy::sensitive("summary").user_private(),
                ToolArgumentPolicy::sensitive("subject").user_private(),
                ToolArgumentPolicy::sensitive("predicate").user_private(),
                ToolArgumentPolicy::sensitive("object").user_private(),
            ])
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
                    Ok(Some(entity)) => {
                        let mut output =
                            format!("Entity: {} ({})", entity.name, entity.entity_type);
                        if let Some(ref s) = entity.summary {
                            output.push_str(&format!("\nSummary: {s}"));
                        }
                        // Show facts
                        if let Ok(facts) = self.memory.get_current_facts(&entity.id).await {
                            if !facts.is_empty() {
                                output.push_str("\nFacts:");
                                for fact in &facts {
                                    output.push_str(&format!(
                                        "\n  - {} (confidence: {:.0}%)",
                                        fact.predicate,
                                        fact.confidence * 100.0
                                    ));
                                }
                            }
                        }
                        Ok(ToolResult {
                            success: true,
                            output,
                            error: None,
                        })
                    }
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
            "add_entity" => {
                let name = args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'query' (entity name)"))?;
                let entity_type = args
                    .get("entity_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("concept");
                let summary = args.get("summary").and_then(|v| v.as_str());

                let entity = Entity {
                    id: String::new(),
                    name: name.to_string(),
                    entity_type: entity_type.to_string(),
                    properties: serde_json::Value::Object(Default::default()),
                    summary: summary.map(String::from),
                    created_by: "default".to_string(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };

                match self.memory.upsert_entity(entity).await {
                    Ok(id) => Ok(ToolResult {
                        success: true,
                        output: format!(
                            "Entity '{name}' ({entity_type}) created/updated (id: {id})"
                        ),
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to add entity: {e}")),
                    }),
                }
            }
            "add_fact" => {
                let subject = args
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'subject'"))?;
                let predicate = args
                    .get("predicate")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'predicate'"))?;
                let object = args
                    .get("object")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'object'"))?;

                // Resolve entities
                let subj = self.memory.find_entity(subject).await.ok().flatten();
                let obj = self.memory.find_entity(object).await.ok().flatten();

                let (subj_id, obj_id) = match (subj, obj) {
                    (Some(s), Some(o)) => (s.id, o.id),
                    (None, _) => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!(
                                "Subject entity '{subject}' not found. Add it first."
                            )),
                        });
                    }
                    (_, None) => {
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!(
                                "Object entity '{object}' not found. Add it first."
                            )),
                        });
                    }
                };

                let fact = synapse_domain::domain::memory::TemporalFact {
                    id: String::new(),
                    subject: subj_id,
                    predicate: predicate.to_string(),
                    object: obj_id,
                    confidence: 0.9,
                    valid_from: chrono::Utc::now(),
                    valid_to: None,
                    recorded_at: chrono::Utc::now(),
                    source_episode: None,
                    created_by: "default".to_string(),
                    embedding: None, // SurrealDB will embed on insert
                };

                match self.memory.add_fact(fact).await {
                    Ok(_) => Ok(ToolResult {
                        success: true,
                        output: format!("{subject} —[{predicate}]→ {object}"),
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to add fact: {e}")),
                    }),
                }
            }
            "get_facts" => {
                let name = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                match self.memory.find_entity(name).await {
                    Ok(Some(entity)) => match self.memory.get_current_facts(&entity.id).await {
                        Ok(facts) if facts.is_empty() => Ok(ToolResult {
                            success: true,
                            output: format!("No facts found for entity '{name}'"),
                            error: None,
                        }),
                        Ok(facts) => {
                            let mut output = format!("Facts about '{name}':\n");
                            for fact in &facts {
                                output.push_str(&format!(
                                    "- {} → {} (confidence: {:.0}%)\n",
                                    fact.predicate,
                                    fact.object,
                                    fact.confidence * 100.0
                                ));
                            }
                            Ok(ToolResult {
                                success: true,
                                output,
                                error: None,
                            })
                        }
                        Err(e) => Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!("Failed to get facts: {e}")),
                        }),
                    },
                    Ok(None) => Ok(ToolResult {
                        success: true,
                        output: format!("Entity '{name}' not found"),
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed: {e}")),
                    }),
                }
            }
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Unknown action '{other}'. Use: search, add_entity, add_fact, get_facts"
                )),
            }),
        }
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        build_result_facts(self.name(), args, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_facts_emits_add_entity_knowledge_fact() {
        let facts = build_result_facts(
            "knowledge",
            &json!({
                "action": "add_entity",
                "query": "Atlas",
                "entity_type": "project",
                "summary": "Release train"
            }),
            Some(&ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            }),
        );

        assert_eq!(facts.len(), 1);
        match &facts[0].payload {
            ToolFactPayload::Knowledge(fact) => {
                assert_eq!(fact.action, KnowledgeAction::AddEntity);
                assert_eq!(fact.subject.as_deref(), Some("Atlas"));
                assert_eq!(fact.entity_type.as_deref(), Some("project"));
                assert_eq!(fact.predicate, None);
            }
            payload => panic!("unexpected payload: {payload:?}"),
        }
    }

    #[test]
    fn extract_facts_emits_add_fact_knowledge_fact() {
        let facts = build_result_facts(
            "knowledge",
            &json!({
                "action": "add_fact",
                "subject": "Atlas",
                "predicate": "uses",
                "object": "SSO"
            }),
            Some(&ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            }),
        );

        match &facts[0].payload {
            ToolFactPayload::Knowledge(fact) => {
                assert_eq!(fact.action, KnowledgeAction::AddFact);
                assert_eq!(fact.subject.as_deref(), Some("Atlas"));
                assert_eq!(fact.predicate.as_deref(), Some("uses"));
                assert_eq!(fact.object.as_deref(), Some("SSO"));
            }
            payload => panic!("unexpected payload: {payload:?}"),
        }
    }

    #[test]
    fn extract_facts_skips_failed_results() {
        let facts = build_result_facts(
            "knowledge",
            &json!({"action": "search", "query": "Atlas"}),
            Some(&ToolResult {
                success: false,
                output: String::new(),
                error: Some("boom".into()),
            }),
        );

        assert!(facts.is_empty());
    }
}
