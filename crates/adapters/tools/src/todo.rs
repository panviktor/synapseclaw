//! Todo tool — session-scoped task ledger for multi-step planning.
//!
//! Gives the agent a bounded task scratchpad instead of keeping plans
//! implicit in chat history. Items are session-scoped (keyed by
//! conversation_id) and in-memory only.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::{
    domain::dialogue_state::FocusEntity,
    domain::tool_fact::TypedToolFact,
    ports::tool::{Tool, ToolExecution, ToolResult},
};

/// Maximum items per session.
const MAX_ITEMS: usize = 20;

#[derive(Debug, Clone, serde::Serialize)]
struct TodoItem {
    id: u32,
    text: String,
    status: String, // pending, in_progress, done
}

pub struct TodoTool {
    store: Arc<RwLock<HashMap<String, Vec<TodoItem>>>>,
    context: Option<Arc<dyn ConversationContextPort>>,
}

impl TodoTool {
    pub fn new(context: Option<Arc<dyn ConversationContextPort>>) -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            context,
        }
    }

    fn session_key(&self) -> String {
        self.context
            .as_ref()
            .and_then(|c| c.get_current())
            .map(|c| c.conversation_id)
            .unwrap_or_else(|| "default".to_string())
    }

    fn build_fact(
        &self,
        _action: &str,
        _session_key: &str,
        item: Option<&TodoItem>,
        _count: usize,
    ) -> TypedToolFact {
        let mut focus_entities = Vec::new();

        if let Some(item) = item {
            focus_entities.push(FocusEntity {
                kind: "todo_item".into(),
                name: item.id.to_string(),
                metadata: Some(item.status.clone()),
            });
        }

        TypedToolFact::focus(self.name().to_string(), focus_entities, Vec::new())
    }

    async fn execute_action(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("list");
        let key = self.session_key();

        match action {
            "add" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(untitled)")
                    .to_string();
                let mut store = self.store.write();
                let items = store.entry(key.clone()).or_default();
                if items.len() >= MAX_ITEMS {
                    return Ok(ToolExecution {
                        result: ToolResult {
                            output: format!(
                                "Task list full ({MAX_ITEMS} max). Complete or remove items first."
                            ),
                            success: false,
                            error: None,
                        },
                        facts: Vec::new(),
                    });
                }
                let id = items.last().map_or(1, |i| i.id + 1);
                items.push(TodoItem {
                    id,
                    text: text.clone(),
                    status: "pending".into(),
                });
                let item = items.last().expect("just pushed task").clone();
                Ok(ToolExecution {
                    result: ToolResult {
                        output: format!("Added task #{id}: {text}"),
                        success: true,
                        error: None,
                    },
                    facts: vec![self.build_fact(action, &key, Some(&item), items.len())],
                })
            }
            "list" => {
                let store = self.store.read();
                let items = store.get(&key);
                match items {
                    Some(items) if !items.is_empty() => {
                        let mut out = String::from("Tasks:\n");
                        for item in items {
                            let marker = match item.status.as_str() {
                                "done" => "✅",
                                "in_progress" => "🔄",
                                _ => "⬜",
                            };
                            out.push_str(&format!(
                                "  {marker} #{} [{}] {}\n",
                                item.id, item.status, item.text
                            ));
                        }
                        let focus_entities = items
                            .iter()
                            .take(3)
                            .map(|item| FocusEntity {
                                kind: "todo_item".into(),
                                name: item.id.to_string(),
                                metadata: Some(item.status.clone()),
                            })
                            .collect();
                        Ok(ToolExecution {
                            result: ToolResult {
                                output: out,
                                success: true,
                                error: None,
                            },
                            facts: vec![TypedToolFact::focus(
                                self.name().to_string(),
                                focus_entities,
                                Vec::new(),
                            )],
                        })
                    }
                    _ => Ok(ToolExecution {
                        result: ToolResult {
                            output: "No tasks. Use action=add to create one.".into(),
                            success: true,
                            error: None,
                        },
                        facts: vec![self.build_fact(action, &key, None, 0)],
                    }),
                }
            }
            "update" => {
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut store = self.store.write();
                if let Some(items) = store.get_mut(&key) {
                    if let Some(index) = items.iter().position(|item| item.id == id) {
                        if !text.is_empty() {
                            items[index].text = text.to_string();
                        }
                        let item = items[index].clone();
                        return Ok(ToolExecution {
                            result: ToolResult {
                                output: format!("Updated task #{id}"),
                                success: true,
                                error: None,
                            },
                            facts: vec![self.build_fact(action, &key, Some(&item), items.len())],
                        });
                    }
                }
                Ok(ToolExecution {
                    result: ToolResult {
                        output: format!("Task #{id} not found"),
                        success: false,
                        error: None,
                    },
                    facts: Vec::new(),
                })
            }
            "complete" => {
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let mut store = self.store.write();
                if let Some(items) = store.get_mut(&key) {
                    if let Some(index) = items.iter().position(|item| item.id == id) {
                        items[index].status = "done".into();
                        let item = items[index].clone();
                        return Ok(ToolExecution {
                            result: ToolResult {
                                output: format!("✅ Task #{id} completed"),
                                success: true,
                                error: None,
                            },
                            facts: vec![self.build_fact(action, &key, Some(&item), items.len())],
                        });
                    }
                }
                Ok(ToolExecution {
                    result: ToolResult {
                        output: format!("Task #{id} not found"),
                        success: false,
                        error: None,
                    },
                    facts: Vec::new(),
                })
            }
            "remove" => {
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let mut store = self.store.write();
                if let Some(items) = store.get_mut(&key) {
                    let before = items.len();
                    items.retain(|i| i.id != id);
                    if items.len() < before {
                        return Ok(ToolExecution {
                            result: ToolResult {
                                output: format!("Removed task #{id}"),
                                success: true,
                                error: None,
                            },
                            facts: vec![TypedToolFact::focus(
                                self.name().to_string(),
                                vec![FocusEntity {
                                    kind: "todo_item".into(),
                                    name: id.to_string(),
                                    metadata: Some("removed".into()),
                                }],
                                Vec::new(),
                            )],
                        });
                    }
                }
                Ok(ToolExecution {
                    result: ToolResult {
                        output: format!("Task #{id} not found"),
                        success: false,
                        error: None,
                    },
                    facts: Vec::new(),
                })
            }
            "clear" => {
                let mut store = self.store.write();
                store.remove(&key);
                Ok(ToolExecution {
                    result: ToolResult {
                        output: "All tasks cleared".into(),
                        success: true,
                        error: None,
                    },
                    facts: vec![self.build_fact(action, &key, None, 0)],
                })
            }
            _ => Ok(ToolExecution {
                result: ToolResult {
                    output: format!(
                        "Unknown action: {action}. Use: add, list, update, complete, remove, clear"
                    ),
                    success: false,
                    error: None,
                },
                facts: Vec::new(),
            }),
        }
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage a session-scoped task list for multi-step planning. \
         Use to externalize plans instead of keeping them implicit in chat. \
         Actions: add, list, update, complete, remove, clear."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "update", "complete", "remove", "clear"],
                    "description": "Operation to perform"
                },
                "text": {
                    "type": "string",
                    "description": "Task text (for add/update)"
                },
                "id": {
                    "type": "integer",
                    "description": "Task ID (for update/complete/remove)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action(args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.execute_action(args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_with_facts_adds_todo_item_fact() {
        let tool = TodoTool::new(None);
        let exec = tool
            .execute_with_facts(json!({"action": "add", "text": "Ship release"}))
            .await
            .unwrap();

        assert!(exec.result.success);
        assert_eq!(exec.facts.len(), 1);
        assert_eq!(exec.facts[0].focus_entities()[0].kind, "todo_item");
        assert_eq!(
            exec.facts[0].focus_entities()[0].metadata.as_deref(),
            Some("pending")
        );
    }

    #[tokio::test]
    async fn execute_with_facts_lists_todo_entities() {
        let tool = TodoTool::new(None);
        let _ = tool
            .execute_with_facts(json!({"action": "add", "text": "One"}))
            .await
            .unwrap();
        let exec = tool
            .execute_with_facts(json!({"action": "list"}))
            .await
            .unwrap();

        assert!(exec.result.success);
        assert!(exec.facts[0]
            .focus_entities()
            .iter()
            .any(|entity| entity.kind == "todo_item" && entity.name == "1"));
    }
}
