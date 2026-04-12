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
use synapse_domain::domain::turn_defaults::TurnDefaultSource;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::tool::{Tool, ToolExecution, ToolResult};
use synapse_domain::ports::turn_defaults_context::TurnDefaultsContextPort;

pub struct MessageSendTool {
    context: Arc<dyn ConversationContextPort>,
    turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
    channel_registry: Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>,
}

impl MessageSendTool {
    pub fn new(
        context: Arc<dyn ConversationContextPort>,
        turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
        channel_registry: Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>,
    ) -> Self {
        Self {
            context,
            turn_defaults_context,
            channel_registry,
        }
    }

    fn parse_explicit_target_object(
        obj: &serde_json::Value,
    ) -> Result<(ConversationDeliveryTarget, DeliveryTargetKind), String> {
        let channel = obj.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let recipient = obj.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
        let thread_ref = obj
            .get("thread_ref")
            .and_then(|v| v.as_str())
            .map(String::from);
        if channel.is_empty() || recipient.is_empty() {
            Err("Explicit target requires both 'channel' and 'recipient'".to_string())
        } else {
            let target = ConversationDeliveryTarget::Explicit {
                channel: channel.to_string(),
                recipient: recipient.to_string(),
                thread_ref,
            };
            Ok((target.clone(), DeliveryTargetKind::Explicit(target)))
        }
    }

    fn resolve_target(
        &self,
        args: &serde_json::Value,
    ) -> Result<(ConversationDeliveryTarget, DeliveryTargetKind), String> {
        match args.get("target") {
            Some(serde_json::Value::String(s)) if s == "current_conversation" => self
                .context
                .get_current()
                .map(|ctx| {
                    let target = ctx.to_explicit_target();
                    (target, DeliveryTargetKind::CurrentConversation)
                })
                .ok_or_else(|| {
                    "No current conversation context available. \
                     Use an explicit target with channel and recipient."
                        .to_string()
                }),
            Some(serde_json::Value::String(_)) => Err(
                "Invalid target. Use 'current_conversation', omit target for a resolved default, \
                 or provide {channel, recipient}."
                    .into(),
            ),
            Some(obj) if obj.is_object() => Self::parse_explicit_target_object(obj),
            None => self
                .turn_defaults_context
                .get_current()
                .and_then(|defaults| defaults.delivery_target)
                .map(|resolved| {
                    let kind = match resolved.source {
                        TurnDefaultSource::DialogueState => {
                            DeliveryTargetKind::Explicit(resolved.target.clone())
                        }
                        TurnDefaultSource::UserProfile => {
                            DeliveryTargetKind::UserProfile(resolved.target.clone())
                        }
                        TurnDefaultSource::ConfiguredChannel => {
                            DeliveryTargetKind::ConfiguredDefault(resolved.target.clone())
                        }
                    };
                    (resolved.target, kind)
                })
                .ok_or_else(|| {
                    "No explicit target provided and no resolved delivery default is available."
                        .to_string()
                }),
            _ => Err(
                "Invalid target. Use 'current_conversation', omit target for a resolved default, \
                 or provide {channel, recipient}."
                    .into(),
            ),
        }
    }
}

#[async_trait]
impl Tool for MessageSendTool {
    fn name(&self) -> &str {
        "message_send"
    }

    fn description(&self) -> &str {
        "Send a message to a conversation or external channel target. Use \
         target='current_conversation' to reply here, omit target when a resolved \
         runtime default already exists, or specify an explicit channel and recipient. \
         Prefer this tool whenever the user asks to send, deliver, post, or report \
         something externally. Do not inspect workspace files or construct shell/Python \
         scripts to discover or perform messaging if this tool can satisfy the request."
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
                    "description": "Where to send. Use 'current_conversation' for here, omit target when a resolved runtime default already exists, or provide explicit target. Omitting target is preferred over file or shell discovery when the destination is already configured.",
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
            "required": ["content"]
        })
    }

    fn runtime_role(&self) -> Option<synapse_domain::ports::tool::ToolRuntimeRole> {
        Some(synapse_domain::ports::tool::ToolRuntimeRole::DirectDelivery)
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_with_facts(args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if content.trim().is_empty() {
            return Ok(ToolExecution {
                result: ToolResult {
                    output: "Message content cannot be empty".into(),
                    success: false,
                    error: None,
                },
                facts: Vec::new(),
            });
        }

        // Resolve target
        let (target, fact_target) = match self.resolve_target(&args) {
            Ok(target) => target,
            Err(output) => {
                return Ok(ToolExecution {
                    result: ToolResult {
                        output,
                        success: false,
                        error: None,
                    },
                    facts: Vec::new(),
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
                    Ok(_) => Ok(ToolExecution {
                        result: ToolResult {
                            output: format!("Message sent to {channel}:{recipient}"),
                            success: true,
                            error: None,
                        },
                        facts: vec![TypedToolFact {
                            tool_id: self.name().to_string(),
                            payload: ToolFactPayload::Delivery(DeliveryFact {
                                target: fact_target,
                                content_bytes: Some(content.len()),
                            }),
                        }],
                    }),
                    Err(e) => Ok(ToolExecution {
                        result: ToolResult {
                            output: format!("Delivery failed: {e}"),
                            success: false,
                            error: None,
                        },
                        facts: Vec::new(),
                    }),
                }
            }
            _ => Ok(ToolExecution {
                result: ToolResult {
                    output: "Unexpected target state".into(),
                    success: false,
                    error: None,
                },
                facts: Vec::new(),
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
    use synapse_domain::domain::turn_defaults::{
        ResolvedDeliveryTarget, ResolvedTurnDefaults, TurnDefaultSource,
    };
    use synapse_domain::ports::channel_registry::ChannelRegistryPort;
    use synapse_domain::ports::turn_defaults_context::{
        InMemoryTurnDefaultsContext, TurnDefaultsContextPort,
    };

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
        let turn_defaults = Arc::new(InMemoryTurnDefaultsContext::new());
        let registry = Arc::new(TestRegistry::default());
        let tool = MessageSendTool::new(context, turn_defaults, registry.clone());

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

    #[tokio::test]
    async fn uses_user_profile_target_when_target_omitted() {
        let context = Arc::new(TestContext::default());
        let turn_defaults = Arc::new(InMemoryTurnDefaultsContext::new());
        turn_defaults.set_current(Some(ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!profile:example.com".into(),
                    thread_ref: None,
                },
                source: TurnDefaultSource::UserProfile,
            }),
        }));
        let registry = Arc::new(TestRegistry::default());
        let tool = MessageSendTool::new(context, turn_defaults, registry.clone());

        let execution = tool
            .execute_with_facts(serde_json::json!({
                "content": "hello"
            }))
            .await
            .unwrap();

        assert!(execution.result.success);
        match &execution.facts[0].payload {
            ToolFactPayload::Delivery(DeliveryFact {
                target:
                    DeliveryTargetKind::UserProfile(ConversationDeliveryTarget::Explicit {
                        recipient,
                        ..
                    }),
                ..
            }) => assert_eq!(recipient, "!profile:example.com"),
            other => panic!("unexpected fact: {other:?}"),
        }
    }

    #[tokio::test]
    async fn uses_recent_delivery_target_before_user_profile_target() {
        let context = Arc::new(TestContext::default());
        let turn_defaults = Arc::new(InMemoryTurnDefaultsContext::new());
        turn_defaults.set_current(Some(ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!recent:example.com".into(),
                    thread_ref: Some("$thread".into()),
                },
                source: TurnDefaultSource::DialogueState,
            }),
        }));
        let registry = Arc::new(TestRegistry::default());
        let tool = MessageSendTool::new(context, turn_defaults, registry.clone());

        let execution = tool
            .execute_with_facts(serde_json::json!({
                "content": "hello"
            }))
            .await
            .unwrap();

        assert!(execution.result.success);
        let delivered = registry.delivered.lock().unwrap();
        assert_eq!(delivered[0].target_recipient, "!recent:example.com");
        match &execution.facts[0].payload {
            ToolFactPayload::Delivery(DeliveryFact {
                target:
                    DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                        recipient, ..
                    }),
                ..
            }) => assert_eq!(recipient, "!recent:example.com"),
            other => panic!("unexpected fact: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_null_target() {
        let context = Arc::new(TestContext::default());
        let turn_defaults = Arc::new(InMemoryTurnDefaultsContext::new());
        let registry = Arc::new(TestRegistry::default());
        let tool = MessageSendTool::new(context, turn_defaults, registry);

        let execution = tool
            .execute_with_facts(serde_json::json!({
                "content": "hello",
                "target": null,
            }))
            .await
            .unwrap();

        assert!(!execution.result.success);
        assert!(execution.result.output.contains("Invalid target"));
    }

    #[tokio::test]
    async fn rejects_stringified_explicit_target_object() {
        let context = Arc::new(TestContext::default());
        let turn_defaults = Arc::new(InMemoryTurnDefaultsContext::new());
        let registry = Arc::new(TestRegistry::default());
        let tool = MessageSendTool::new(context, turn_defaults, registry);

        let execution = tool
            .execute_with_facts(serde_json::json!({
                "content": "hello",
                "target": "{\"channel\":\"matrix\",\"recipient\":\"!room:example.com\"}"
            }))
            .await
            .unwrap();

        assert!(!execution.result.success);
        assert!(execution.result.output.contains("Invalid target"));
    }
}
