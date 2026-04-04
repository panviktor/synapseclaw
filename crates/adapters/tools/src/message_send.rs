//! Message send tool — deliver a message to the current or another conversation.
//!
//! Uses ConversationContextPort to resolve "current_conversation" targets.
//! Delivers via ChannelRegistryPort → channel adapter.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::tool::{Tool, ToolResult};

pub struct MessageSendTool {
    context: Arc<dyn ConversationContextPort>,
    channel_registry: Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>,
}

impl MessageSendTool {
    pub fn new(
        context: Arc<dyn ConversationContextPort>,
        channel_registry: Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>,
    ) -> Self {
        Self {
            context,
            channel_registry,
        }
    }
}

#[async_trait]
impl Tool for MessageSendTool {
    fn name(&self) -> &str {
        "message_send"
    }

    fn description(&self) -> &str {
        "Send a message to a conversation. Use target='current_conversation' to reply here, \
         or specify an explicit channel and recipient. This is the preferred way to send \
         proactive messages instead of constructing shell commands."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Message text to send"
                },
                "target": {
                    "description": "Where to send. Use 'current_conversation' for here, or provide explicit target.",
                    "oneOf": [
                        {
                            "type": "string",
                            "enum": ["current_conversation"],
                            "description": "Send to the current conversation"
                        },
                        {
                            "type": "object",
                            "properties": {
                                "channel": { "type": "string", "description": "Channel adapter name (telegram, matrix, slack, etc.)" },
                                "recipient": { "type": "string", "description": "Chat ID, room ID, or channel ID" },
                                "thread_ref": { "type": "string", "description": "Optional thread ID" }
                            },
                            "required": ["channel", "recipient"]
                        }
                    ]
                }
            },
            "required": ["content", "target"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if content.trim().is_empty() {
            return Ok(ToolResult { output: "Message content cannot be empty".into(), success: false, error: None });
        }

        // Resolve target
        let target = match args.get("target") {
            Some(serde_json::Value::String(s)) if s == "current_conversation" => {
                match self.context.get_current() {
                    Some(ctx) => ctx.to_explicit_target(),
                    None => {
                        return Ok(ToolResult { output: "No current conversation context available. \
                                     Use an explicit target with channel and recipient."
                                .into(), success: false, error: None });
                    }
                }
            }
            Some(obj) if obj.is_object() => {
                let channel = obj
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let recipient = obj
                    .get("recipient")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let thread_ref = obj
                    .get("thread_ref")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if channel.is_empty() || recipient.is_empty() {
                    return Ok(ToolResult { output: "Explicit target requires both 'channel' and 'recipient'".into(), success: false, error: None });
                }
                ConversationDeliveryTarget::Explicit {
                    channel: channel.to_string(),
                    recipient: recipient.to_string(),
                    thread_ref,
                }
            }
            _ => {
                return Ok(ToolResult { output: "Invalid target. Use 'current_conversation' or {channel, recipient}."
                        .into(), success: false, error: None });
            }
        };

        // Deliver via channel registry
        match &target {
            ConversationDeliveryTarget::Explicit {
                channel,
                recipient,
                thread_ref: _,
            } => {
                let intent = synapse_domain::domain::channel::OutboundIntent::notify(
                    channel.as_str(),
                    recipient.as_str(),
                    content.clone(),
                );
                match self.channel_registry.deliver(&intent).await {
                    Ok(_) => Ok(ToolResult { output: format!("Message sent to {channel}:{recipient}"), success: true, error: None }),
                    Err(e) => Ok(ToolResult { output: format!("Delivery failed: {e}"), success: false, error: None }),
                }
            }
            _ => Ok(ToolResult { output: "Unexpected target state".into(), success: false, error: None }),
        }
    }
}
