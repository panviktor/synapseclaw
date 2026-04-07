//! Message send tool — deliver a message to the current or another conversation.
//!
//! Uses ConversationContextPort to resolve "current_conversation" targets.
//! Delivers via ChannelRegistryPort → channel adapter.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::domain::tool_fact::{
    DeliveryFact, DeliveryTargetKind, ToolFactPayload, TypedToolFact,
};
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

    fn resolve_target(
        &self,
        args: &serde_json::Value,
    ) -> Result<ConversationDeliveryTarget, String> {
        match args.get("target") {
            Some(serde_json::Value::String(s)) if s == "current_conversation" => self
                .context
                .get_current()
                .map(|ctx| ctx.to_explicit_target())
                .ok_or_else(|| {
                    "No current conversation context available. \
                     Use an explicit target with channel and recipient."
                        .to_string()
                }),
            Some(obj) if obj.is_object() => {
                let channel = obj.get("channel").and_then(|v| v.as_str()).unwrap_or("");
                let recipient = obj.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
                let thread_ref = obj
                    .get("thread_ref")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if channel.is_empty() || recipient.is_empty() {
                    Err("Explicit target requires both 'channel' and 'recipient'".to_string())
                } else {
                    Ok(ConversationDeliveryTarget::Explicit {
                        channel: channel.to_string(),
                        recipient: recipient.to_string(),
                        thread_ref,
                    })
                }
            }
            _ => Err("Invalid target. Use 'current_conversation' or {channel, recipient}.".into()),
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

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        if matches!(result, Some(result) if !result.success) {
            return Vec::new();
        }

        let target = match args.get("target") {
            Some(serde_json::Value::String(s)) if s == "current_conversation" => {
                DeliveryTargetKind::CurrentConversation
            }
            Some(obj) if obj.is_object() => {
                let channel = obj.get("channel").and_then(|v| v.as_str()).unwrap_or("");
                let recipient = obj.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
                let thread_ref = obj
                    .get("thread_ref")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if channel.is_empty() || recipient.is_empty() {
                    return Vec::new();
                }
                DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                    channel: channel.to_string(),
                    recipient: recipient.to_string(),
                    thread_ref,
                })
            }
            _ => return Vec::new(),
        };

        vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Delivery(DeliveryFact {
                target,
                content_bytes: args.get("content").and_then(|v| v.as_str()).map(str::len),
            }),
        }]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if content.trim().is_empty() {
            return Ok(ToolResult {
                output: "Message content cannot be empty".into(),
                success: false,
                error: None,
            });
        }

        // Resolve target
        let target = match self.resolve_target(&args) {
            Ok(target) => target,
            Err(output) => {
                return Ok(ToolResult {
                    output,
                    success: false,
                    error: None,
                });
            }
        };

        // Deliver via channel registry
        match &target {
            ConversationDeliveryTarget::Explicit {
                channel,
                recipient,
                thread_ref,
            } => {
                let intent = synapse_domain::domain::channel::OutboundIntent::notify_in_thread(
                    channel.as_str(),
                    recipient.as_str(),
                    thread_ref.clone(),
                    content.clone(),
                );
                match self.channel_registry.deliver(&intent).await {
                    Ok(_) => Ok(ToolResult {
                        output: format!("Message sent to {channel}:{recipient}"),
                        success: true,
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        output: format!("Delivery failed: {e}"),
                        success: false,
                        error: None,
                    }),
                }
            }
            _ => Ok(ToolResult {
                output: "Unexpected target state".into(),
                success: false,
                error: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use synapse_domain::domain::channel::{ChannelCapability, OutboundIntent};
    use synapse_domain::domain::conversation_target::CurrentConversationContext;
    use synapse_domain::ports::channel_registry::ChannelRegistryPort;

    #[derive(Default)]
    struct TestContext {
        inner: parking_lot::RwLock<Option<CurrentConversationContext>>,
    }

    impl ConversationContextPort for TestContext {
        fn get_current(&self) -> Option<CurrentConversationContext> {
            self.inner.read().clone()
        }

        fn set_current(&self, ctx: Option<CurrentConversationContext>) {
            *self.inner.write() = ctx;
        }
    }

    #[derive(Default)]
    struct TestRegistry {
        delivered: Mutex<Vec<OutboundIntent>>,
    }

    #[async_trait]
    impl ChannelRegistryPort for TestRegistry {
        fn has_channel(&self, _channel_name: &str) -> bool {
            true
        }

        fn capabilities(&self, _channel_name: &str) -> Vec<ChannelCapability> {
            vec![ChannelCapability::SendText, ChannelCapability::Threads]
        }

        async fn deliver(&self, intent: &OutboundIntent) -> anyhow::Result<()> {
            self.delivered.lock().unwrap().push(intent.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn preserves_thread_ref_on_explicit_delivery() {
        let context = Arc::new(TestContext::default());
        let registry = Arc::new(TestRegistry::default());
        let tool = MessageSendTool::new(context, registry.clone());

        let result = tool
            .execute(serde_json::json!({
                "content": "hello",
                "target": {
                    "channel": "matrix",
                    "recipient": "!room:example.com",
                    "thread_ref": "$thread"
                }
            }))
            .await
            .unwrap();

        assert!(result.success);
        let delivered = registry.delivered.lock().unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].thread_ref.as_deref(), Some("$thread"));
    }
}
