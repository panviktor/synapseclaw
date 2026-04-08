//! Standing order tool — subscribe to proactive system reports.
//!
//! "After restart, report here" → StandingOrder with RestartReport kind
//! bound to the current conversation.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::domain::standing_order::{StandingOrder, StandingOrderKind};
use synapse_domain::domain::tool_fact::TypedToolFact;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::standing_order_store::StandingOrderStorePort;
use synapse_domain::ports::tool::{Tool, ToolExecution, ToolResult};

pub struct StandingOrderTool {
    context: Option<Arc<dyn ConversationContextPort>>,
    store: Arc<dyn StandingOrderStorePort>,
    created_by: String,
}

impl StandingOrderTool {
    pub fn new(
        context: Option<Arc<dyn ConversationContextPort>>,
        store: Arc<dyn StandingOrderStorePort>,
        created_by: impl Into<String>,
    ) -> Self {
        Self {
            context,
            store,
            created_by: created_by.into(),
        }
    }

    async fn execute_action(
        &self,
        args: &serde_json::Value,
    ) -> anyhow::Result<(ToolResult, Vec<TypedToolFact>)> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("list");

        match action {
            "subscribe" => {
                let kind_str = args
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("restart_report");
                let kind = match kind_str {
                    "heartbeat_report" => StandingOrderKind::HeartbeatReport,
                    _ => StandingOrderKind::RestartReport,
                };

                let (channel, recipient, thread) = match self
                    .context
                    .as_ref()
                    .and_then(|c| c.get_current())
                {
                    Some(ctx) => (ctx.source_adapter, ctx.reply_ref, ctx.thread_ref),
                    None => {
                        return Ok((
                            ToolResult {
                                output: "No current conversation context. Standing orders must be created from a live conversation.".into(),
                                success: false,
                                error: None,
                            },
                            Vec::new(),
                        ));
                    }
                };

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let id = format!(
                    "so_{:x}",
                    now.wrapping_mul(6364136223846793005).wrapping_add(1)
                );

                let order = StandingOrder {
                    id: id.clone(),
                    kind,
                    delivery_channel: channel.clone(),
                    delivery_recipient: recipient.clone(),
                    delivery_thread: thread.clone(),
                    enabled: true,
                    created_by: self.created_by.clone(),
                    created_at: now,
                };

                self.store.upsert(order)?;

                Ok((
                    ToolResult {
                        output: format!(
                            "Standing order {id} created: {kind_str} → {channel}:{recipient}"
                        ),
                        success: true,
                        error: None,
                    },
                    vec![TypedToolFact::focus(
                        self.name().to_string(),
                        vec![FocusEntity {
                            kind: "standing_order".into(),
                            name: id,
                            metadata: Some(kind_str.to_string()),
                        }],
                        Vec::new(),
                    )],
                ))
            }
            "list" => {
                let orders = self.store.list();
                if orders.is_empty() {
                    return Ok((
                        ToolResult {
                            output: "No standing orders.".into(),
                            success: true,
                            error: None,
                        },
                        Vec::new(),
                    ));
                }
                let mut out = String::from("Standing orders:\n");
                for o in &orders {
                    let kind = match &o.kind {
                        StandingOrderKind::RestartReport => "restart_report",
                        StandingOrderKind::HeartbeatReport => "heartbeat_report",
                        StandingOrderKind::ScheduledPrompt { .. } => "scheduled",
                        StandingOrderKind::CustomEvent { .. } => "custom",
                    };
                    let status = if o.enabled { "✅" } else { "⏸️" };
                    out.push_str(&format!(
                        "  {status} {} [{}] → {}:{}\n",
                        o.id, kind, o.delivery_channel, o.delivery_recipient
                    ));
                }
                let facts = vec![TypedToolFact::focus(
                    self.name().to_string(),
                    orders
                        .iter()
                        .take(3)
                        .map(|order| FocusEntity {
                            kind: "standing_order".into(),
                            name: order.id.clone(),
                            metadata: Some(match &order.kind {
                                StandingOrderKind::RestartReport => "restart_report".into(),
                                StandingOrderKind::HeartbeatReport => "heartbeat_report".into(),
                                StandingOrderKind::ScheduledPrompt { .. } => "scheduled".into(),
                                StandingOrderKind::CustomEvent { .. } => "custom".into(),
                            }),
                        })
                        .collect(),
                    Vec::new(),
                )];
                Ok((
                    ToolResult {
                        output: out,
                        success: true,
                        error: None,
                    },
                    facts,
                ))
            }
            "cancel" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if self.store.remove(id)? {
                    Ok((
                        ToolResult {
                            output: format!("Standing order {id} cancelled"),
                            success: true,
                            error: None,
                        },
                        vec![TypedToolFact::focus(
                            self.name().to_string(),
                            vec![FocusEntity {
                                kind: "standing_order".into(),
                                name: id.to_string(),
                                metadata: Some("cancelled".into()),
                            }],
                            Vec::new(),
                        )],
                    ))
                } else {
                    Ok((
                        ToolResult {
                            output: format!("Standing order {id} not found"),
                            success: false,
                            error: None,
                        },
                        Vec::new(),
                    ))
                }
            }
            _ => Ok((
                ToolResult {
                    output: format!("Unknown action: {action}. Use: subscribe, list, cancel"),
                    success: false,
                    error: None,
                },
                Vec::new(),
            )),
        }
    }
}

#[async_trait]
impl Tool for StandingOrderTool {
    fn name(&self) -> &str {
        "standing_order"
    }

    fn description(&self) -> &str {
        "Subscribe to proactive system reports. Use 'subscribe' to create a standing order \
         that triggers on restart, heartbeat, or custom events. The agent will automatically \
         deliver reports to the target conversation."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["subscribe", "list", "cancel"],
                    "description": "Operation to perform"
                },
                "kind": {
                    "type": "string",
                    "enum": ["restart_report", "heartbeat_report"],
                    "description": "What triggers the order (for subscribe)"
                },
                "id": {
                    "type": "string",
                    "description": "Order ID (for cancel)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let (result, _) = self.execute_action(&args).await?;
        Ok(result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        let (result, facts) = self.execute_action(&args).await?;
        Ok(ToolExecution { result, facts })
    }
}
