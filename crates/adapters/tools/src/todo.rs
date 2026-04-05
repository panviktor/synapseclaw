//! Todo tool — session-scoped task ledger for multi-step planning.
//!
//! Gives the agent a bounded task scratchpad instead of keeping plans
//! implicit in chat history. Items are session-scoped (keyed by
//! conversation_ref) and in-memory only.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::tool::{Tool, ToolResult};

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
            .map(|c| c.conversation_ref)
            .unwrap_or_else(|| "default".to_string())
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
                let items = store.entry(key).or_default();
                if items.len() >= MAX_ITEMS {
                    return Ok(ToolResult {
                        output: format!(
                            "Task list full ({MAX_ITEMS} max). Complete or remove items first."
                        ),
                        success: false,
                        error: None,
                    });
                }
                let id = items.last().map_or(1, |i| i.id + 1);
                items.push(TodoItem {
                    id,
                    text: text.clone(),
                    status: "pending".into(),
                });
                Ok(ToolResult {
                    output: format!("Added task #{id}: {text}"),
                    success: true,
                    error: None,
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
                        Ok(ToolResult {
                            output: out,
                            success: true,
                            error: None,
                        })
                    }
                    _ => Ok(ToolResult {
                        output: "No tasks. Use action=add to create one.".into(),
                        success: true,
                        error: None,
                    }),
                }
            }
            "update" => {
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let mut store = self.store.write();
                if let Some(items) = store.get_mut(&key) {
                    if let Some(item) = items.iter_mut().find(|i| i.id == id) {
                        if !text.is_empty() {
                            item.text = text.to_string();
                        }
                        return Ok(ToolResult {
                            output: format!("Updated task #{id}"),
                            success: true,
                            error: None,
                        });
                    }
                }
                Ok(ToolResult {
                    output: format!("Task #{id} not found"),
                    success: false,
                    error: None,
                })
            }
            "complete" => {
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let mut store = self.store.write();
                if let Some(items) = store.get_mut(&key) {
                    if let Some(item) = items.iter_mut().find(|i| i.id == id) {
                        item.status = "done".into();
                        return Ok(ToolResult {
                            output: format!("✅ Task #{id} completed"),
                            success: true,
                            error: None,
                        });
                    }
                }
                Ok(ToolResult {
                    output: format!("Task #{id} not found"),
                    success: false,
                    error: None,
                })
            }
            "remove" => {
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let mut store = self.store.write();
                if let Some(items) = store.get_mut(&key) {
                    let before = items.len();
                    items.retain(|i| i.id != id);
                    if items.len() < before {
                        return Ok(ToolResult {
                            output: format!("Removed task #{id}"),
                            success: true,
                            error: None,
                        });
                    }
                }
                Ok(ToolResult {
                    output: format!("Task #{id} not found"),
                    success: false,
                    error: None,
                })
            }
            "clear" => {
                let mut store = self.store.write();
                store.remove(&key);
                Ok(ToolResult {
                    output: "All tasks cleared".into(),
                    success: true,
                    error: None,
                })
            }
            _ => Ok(ToolResult {
                output: format!(
                    "Unknown action: {action}. Use: add, list, update, complete, remove, clear"
                ),
                success: false,
                error: None,
            }),
        }
    }
}
