//! Structured user profile tool.
//!
//! This is the explicit runtime path for stable user defaults and preferences.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::application::services::user_profile_service::{
    self, ProfileFieldPatch, UserProfilePatch,
};
use synapse_domain::domain::config::ToolOperation;
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::agent_runtime::AgentToolFact;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::tool::{Tool, ToolResult};
use synapse_domain::ports::user_profile_context::UserProfileContextPort;
use synapse_domain::ports::user_profile_store::UserProfileStorePort;

pub struct UserProfileTool {
    store: Arc<dyn UserProfileStorePort>,
    security: Arc<SecurityPolicy>,
    conversation_context: Option<Arc<dyn ConversationContextPort>>,
    profile_context: Option<Arc<dyn UserProfileContextPort>>,
}

impl UserProfileTool {
    pub fn new(
        store: Arc<dyn UserProfileStorePort>,
        security: Arc<SecurityPolicy>,
        conversation_context: Option<Arc<dyn ConversationContextPort>>,
        profile_context: Option<Arc<dyn UserProfileContextPort>>,
    ) -> Self {
        Self {
            store,
            security,
            conversation_context,
            profile_context,
        }
    }

    fn resolve_user_key(&self) -> Option<String> {
        if let Some(key) = self
            .profile_context
            .as_ref()
            .and_then(|port| port.get_current_key())
        {
            return Some(key);
        }

        self.conversation_context.as_ref().and_then(|port| {
            port.get_current()
                .map(|ctx| format!("channel:{}:{}", ctx.source_adapter, ctx.actor_id))
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileAction {
    Get,
    Upsert,
    Clear,
    Delete,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DeliveryTargetInput {
    Keyword(String),
    Explicit {
        channel: String,
        recipient: String,
        thread_ref: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct UserProfileArgs {
    #[serde(default = "default_action")]
    action: ProfileAction,
    preferred_language: Option<String>,
    timezone: Option<String>,
    default_city: Option<String>,
    communication_style: Option<String>,
    known_environments: Option<Vec<String>>,
    default_delivery_target: Option<DeliveryTargetInput>,
    #[serde(default)]
    clear_fields: Vec<String>,
}

fn default_action() -> ProfileAction {
    ProfileAction::Get
}

#[async_trait]
impl Tool for UserProfileTool {
    fn name(&self) -> &str {
        "user_profile"
    }

    fn description(&self) -> &str {
        "Get or update the structured user profile for stable defaults like preferred language, timezone, default city, communication style, known environments, and default delivery target. Use this when the user explicitly states a durable preference or corrects an existing default."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["get", "upsert", "clear", "delete"],
                    "description": "Profile operation"
                },
                "preferred_language": { "type": "string" },
                "timezone": { "type": "string", "description": "IANA timezone like Europe/Berlin" },
                "default_city": { "type": "string" },
                "communication_style": { "type": "string" },
                "known_environments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Replace the full known environments list"
                },
                "default_delivery_target": {
                    "description": "Use 'current_conversation' to snapshot this chat, or provide explicit channel/recipient",
                    "oneOf": [
                        {
                            "type": "string",
                            "enum": ["current_conversation"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "channel": { "type": "string" },
                                "recipient": { "type": "string" },
                                "thread_ref": { "type": "string" }
                            },
                            "required": ["channel", "recipient"]
                        }
                    ]
                },
                "clear_fields": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": [
                            "preferred_language",
                            "timezone",
                            "default_city",
                            "communication_style",
                            "known_environments",
                            "default_delivery_target"
                        ]
                    },
                    "description": "Fields to clear when action is upsert or clear"
                }
            }
        })
    }

    fn extract_facts(
        &self,
        _args: &serde_json::Value,
        _result: Option<&ToolResult>,
    ) -> Vec<AgentToolFact> {
        Vec::new()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let args: UserProfileArgs = serde_json::from_value(args)?;

        if let Err(error) = self
            .security
            .enforce_tool_operation(ToolOperation::Act, "user_profile")
        {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error),
            });
        }

        let Some(user_key) = self.resolve_user_key() else {
            return Ok(ToolResult {
                success: false,
                output: "No current user profile context is available.".into(),
                error: None,
            });
        };

        match args.action {
            ProfileAction::Get => {
                let output = match self.store.load(&user_key) {
                    Some(profile) => format!(
                        "User profile for {user_key}:\n{}",
                        user_profile_service::format_profile_projection(&profile)
                    ),
                    None => format!("User profile for {user_key} is empty."),
                };
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }
            ProfileAction::Delete => {
                let removed = self.store.remove(&user_key)?;
                Ok(ToolResult {
                    success: removed,
                    output: if removed {
                        format!("Deleted user profile for {user_key}")
                    } else {
                        format!("No user profile found for {user_key}")
                    },
                    error: None,
                })
            }
            ProfileAction::Upsert | ProfileAction::Clear => {
                let patch = build_patch(
                    &args,
                    self.conversation_context
                        .as_ref()
                        .and_then(|port| port.get_current())
                        .as_ref(),
                    matches!(args.action, ProfileAction::Clear),
                )?;
                if patch.is_noop() {
                    return Ok(ToolResult {
                        success: false,
                        output: "No profile changes were provided.".into(),
                        error: None,
                    });
                }

                let updated = user_profile_service::apply_patch(self.store.load(&user_key), &patch);
                match updated {
                    Some(profile) => {
                        self.store.upsert(&user_key, profile.clone())?;
                        Ok(ToolResult {
                            success: true,
                            output: format!(
                                "Updated user profile for {user_key}:\n{}",
                                user_profile_service::format_profile_projection(&profile)
                            ),
                            error: None,
                        })
                    }
                    None => {
                        let _ = self.store.remove(&user_key)?;
                        Ok(ToolResult {
                            success: true,
                            output: format!(
                                "User profile for {user_key} is now empty and was removed."
                            ),
                            error: None,
                        })
                    }
                }
            }
        }
    }
}

fn build_patch(
    args: &UserProfileArgs,
    current_conversation: Option<
        &synapse_domain::domain::conversation_target::CurrentConversationContext,
    >,
    clear_only: bool,
) -> anyhow::Result<UserProfilePatch> {
    let mut patch = UserProfilePatch::default();
    let clear =
        |field: &str, args: &UserProfileArgs| args.clear_fields.iter().any(|item| item == field);

    patch.preferred_language = if clear_only || clear("preferred_language", args) {
        ProfileFieldPatch::Clear
    } else if let Some(value) = args.preferred_language.as_ref() {
        ProfileFieldPatch::Set(value.clone())
    } else {
        ProfileFieldPatch::Keep
    };

    patch.timezone = if clear_only || clear("timezone", args) {
        ProfileFieldPatch::Clear
    } else if let Some(value) = args.timezone.as_ref() {
        ProfileFieldPatch::Set(value.clone())
    } else {
        ProfileFieldPatch::Keep
    };

    patch.default_city = if clear_only || clear("default_city", args) {
        ProfileFieldPatch::Clear
    } else if let Some(value) = args.default_city.as_ref() {
        ProfileFieldPatch::Set(value.clone())
    } else {
        ProfileFieldPatch::Keep
    };

    patch.communication_style = if clear_only || clear("communication_style", args) {
        ProfileFieldPatch::Clear
    } else if let Some(value) = args.communication_style.as_ref() {
        ProfileFieldPatch::Set(value.clone())
    } else {
        ProfileFieldPatch::Keep
    };

    patch.known_environments = if clear_only || clear("known_environments", args) {
        ProfileFieldPatch::Clear
    } else if let Some(values) = args.known_environments.as_ref() {
        ProfileFieldPatch::Set(values.clone())
    } else {
        ProfileFieldPatch::Keep
    };

    patch.default_delivery_target = if clear_only || clear("default_delivery_target", args) {
        ProfileFieldPatch::Clear
    } else if let Some(target) = args.default_delivery_target.as_ref() {
        ProfileFieldPatch::Set(match target {
                DeliveryTargetInput::Keyword(value) if value == "current_conversation" => {
                    current_conversation
                        .map(|ctx| ctx.to_explicit_target())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "default_delivery_target='current_conversation' requires live conversation context"
                            )
                        })?
                }
                DeliveryTargetInput::Keyword(_) => {
                    anyhow::bail!(
                        "default_delivery_target must be 'current_conversation' or an explicit object"
                    );
                }
                DeliveryTargetInput::Explicit {
                    channel,
                    recipient,
                    thread_ref,
                } => ConversationDeliveryTarget::Explicit {
                    channel: channel.clone(),
                    recipient: recipient.clone(),
                    thread_ref: thread_ref.clone(),
                },
            })
    } else {
        ProfileFieldPatch::Keep
    };

    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::RwLock;
    use synapse_domain::domain::conversation_target::CurrentConversationContext;
    use synapse_domain::ports::conversation_context::ConversationContextPort;
    use synapse_domain::ports::user_profile_context::InMemoryUserProfileContext;
    use synapse_domain::ports::user_profile_store::InMemoryUserProfileStore;

    #[derive(Default)]
    struct TestConversationContext {
        inner: RwLock<Option<CurrentConversationContext>>,
    }

    impl ConversationContextPort for TestConversationContext {
        fn get_current(&self) -> Option<CurrentConversationContext> {
            self.inner.read().clone()
        }

        fn set_current(&self, ctx: Option<CurrentConversationContext>) {
            *self.inner.write() = ctx;
        }
    }

    fn security() -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy::default())
    }

    #[tokio::test]
    async fn upserts_profile_for_current_web_user() {
        let store = Arc::new(InMemoryUserProfileStore::new());
        let profile_context = Arc::new(InMemoryUserProfileContext::new());
        profile_context.set_current_key(Some("web:abc".into()));
        let tool = UserProfileTool::new(store.clone(), security(), None, Some(profile_context));

        let result = tool
            .execute(json!({
                "action": "upsert",
                "preferred_language": "ru",
                "timezone": "Europe/Berlin"
            }))
            .await
            .unwrap();

        assert!(result.success);
        let profile = store.load("web:abc").unwrap();
        assert_eq!(profile.preferred_language.as_deref(), Some("ru"));
        assert_eq!(profile.timezone.as_deref(), Some("Europe/Berlin"));
    }

    #[tokio::test]
    async fn snapshots_current_conversation_delivery_target() {
        let store = Arc::new(InMemoryUserProfileStore::new());
        let profile_context = Arc::new(InMemoryUserProfileContext::new());
        profile_context.set_current_key(Some("web:abc".into()));
        let conversation_context = Arc::new(TestConversationContext::default());
        conversation_context.set_current(Some(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_ref: "matrix_room".into(),
            reply_ref: "!room:example.com".into(),
            thread_ref: Some("$thread".into()),
            actor_id: "alice".into(),
        }));
        let tool = UserProfileTool::new(
            store.clone(),
            security(),
            Some(conversation_context),
            Some(profile_context),
        );

        let result = tool
            .execute(json!({
                "action": "upsert",
                "default_delivery_target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(result.success);
        match store.load("web:abc").unwrap().default_delivery_target {
            Some(ConversationDeliveryTarget::Explicit {
                channel,
                recipient,
                thread_ref,
            }) => {
                assert_eq!(channel, "matrix");
                assert_eq!(recipient, "!room:example.com");
                assert_eq!(thread_ref.as_deref(), Some("$thread"));
            }
            other => panic!("unexpected target: {other:?}"),
        }
    }
}
