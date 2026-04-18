//! Dynamic user profile tool.
//!
//! Stores durable user facts as arbitrary key/value data. The tool does not
//! expose fixed profile fields; consumers may agree on data keys when needed.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use synapse_domain::application::services::user_profile_service::{
    self, ProfileFactPatch, UserProfilePatch,
};
use synapse_domain::domain::config::ToolOperation;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    ProfileOperation, ToolFactPayload, TypedToolFact, UserProfileFact,
};
use synapse_domain::domain::user_profile::normalize_fact_key;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::tool::{
    Tool, ToolArgumentPolicy, ToolContract, ToolNonReplayableReason, ToolResult, ToolRuntimeRole,
};
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
#[serde(deny_unknown_fields)]
struct UserProfileArgs {
    #[serde(default = "default_action")]
    action: ProfileAction,
    #[serde(default)]
    facts: BTreeMap<String, Value>,
    #[serde(default)]
    clear_keys: Vec<String>,
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
        "Get or update durable user facts as an arbitrary key/value profile. Use only when the user explicitly states a durable preference, default, identity fact, or correction. Keys are dynamic; do not assume a fixed profile schema."
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
                "facts": {
                    "type": "object",
                    "additionalProperties": true,
                    "description": "Arbitrary profile facts to set. Example: {\"preferred_response_language\":\"ru\", \"project_alias\":\"Borealis\"}. Structured values are allowed."
                },
                "clear_keys": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Dynamic fact keys to clear. With action=clear and no keys, all current facts are cleared."
                }
            }
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::ProfileMutation)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(self.runtime_role(), ToolNonReplayableReason::MutatesState)
            .with_arguments(vec![
                ToolArgumentPolicy::replayable("action")
                    .with_values(["get", "upsert", "clear", "delete"]),
                ToolArgumentPolicy::sensitive("facts").user_private(),
                ToolArgumentPolicy::sensitive("clear_keys").user_private(),
            ])
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        if matches!(result, Some(result) if !result.success) {
            return Vec::new();
        }

        let Ok(args) = serde_json::from_value::<UserProfileArgs>(args.clone()) else {
            return Vec::new();
        };

        if matches!(args.action, ProfileAction::Get) {
            return Vec::new();
        }

        let mut facts = Vec::new();
        match args.action {
            ProfileAction::Get => {}
            ProfileAction::Upsert => {
                for (key, value) in &args.facts {
                    collect_profile_fact(&mut facts, self.name(), key, Some(value), false);
                }
                for key in &args.clear_keys {
                    collect_profile_fact(&mut facts, self.name(), key, None, true);
                }
            }
            ProfileAction::Clear | ProfileAction::Delete => {
                let keys = if args.clear_keys.is_empty() {
                    self.resolve_user_key()
                        .and_then(|key| self.store.load(&key))
                        .map(|profile| profile.iter().map(|(key, _)| key.clone()).collect())
                        .unwrap_or_default()
                } else {
                    args.clear_keys.clone()
                };
                for key in &keys {
                    collect_profile_fact(&mut facts, self.name(), key, None, true);
                }
            }
        }

        facts
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
                let current = self.store.load(&user_key);
                let patch = build_patch(
                    &args,
                    current.as_ref(),
                    matches!(args.action, ProfileAction::Clear),
                );
                if patch.is_noop() {
                    return Ok(ToolResult {
                        success: false,
                        output: "No profile changes were provided.".into(),
                        error: None,
                    });
                }

                let updated = user_profile_service::apply_patch(current, &patch);
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

fn collect_profile_fact(
    facts: &mut Vec<TypedToolFact>,
    tool_name: &str,
    key: &str,
    value: Option<&Value>,
    clear: bool,
) {
    let Some(key) = normalize_fact_key(key) else {
        return;
    };
    let (operation, value) = if clear {
        (ProfileOperation::Clear, None)
    } else {
        let Some(value) = value else {
            return;
        };
        (ProfileOperation::Set, Some(render_fact_value(value)))
    };

    facts.push(TypedToolFact {
        tool_id: tool_name.to_string(),
        payload: ToolFactPayload::UserProfile(UserProfileFact {
            key,
            operation,
            value,
        }),
    });
}

fn build_patch(
    args: &UserProfileArgs,
    current_profile: Option<&synapse_domain::domain::user_profile::UserProfile>,
    clear_only: bool,
) -> UserProfilePatch {
    let mut patch = UserProfilePatch::default();

    if clear_only && args.clear_keys.is_empty() {
        if let Some(profile) = current_profile {
            for (key, _) in profile.iter() {
                patch.clear(key);
            }
        }
        return patch;
    }

    for (key, value) in &args.facts {
        patch.set(key, value.clone());
    }
    for key in &args.clear_keys {
        if let Some(key) = normalize_fact_key(key) {
            patch.facts.insert(key, ProfileFactPatch::Clear);
        }
    }

    patch
}

fn render_fact_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::RwLock;
    use synapse_domain::domain::conversation_target::{
        ConversationDeliveryTarget, CurrentConversationContext,
    };
    use synapse_domain::domain::user_profile::DELIVERY_TARGET_PREFERENCE_KEY;
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
    async fn upserts_dynamic_facts_for_current_web_user() {
        let store = Arc::new(InMemoryUserProfileStore::new());
        let profile_context = Arc::new(InMemoryUserProfileContext::new());
        profile_context.set_current_key(Some("web:abc".into()));
        let tool = UserProfileTool::new(store.clone(), security(), None, Some(profile_context));

        let result = tool
            .execute(json!({
                "action": "upsert",
                "facts": {
                    "response_locale": "ru",
                    "workspace_anchor": "Borealis"
                }
            }))
            .await
            .unwrap();

        assert!(result.success);
        let profile = store.load("web:abc").unwrap();
        assert_eq!(profile.get_text("response_locale").as_deref(), Some("ru"));
        assert_eq!(
            profile.get_text("workspace_anchor").as_deref(),
            Some("Borealis")
        );
    }

    #[tokio::test]
    async fn stores_structured_delivery_target_as_dynamic_fact() {
        let store = Arc::new(InMemoryUserProfileStore::new());
        let profile_context = Arc::new(InMemoryUserProfileContext::new());
        profile_context.set_current_key(Some("web:abc".into()));
        let conversation_context = Arc::new(TestConversationContext::default());
        conversation_context.set_current(Some(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "matrix_room".into(),
            reply_ref: "!room:example.com".into(),
            thread_ref: Some("$thread".into()),
            actor_id: "alice".into(),
        }));
        let target = conversation_context
            .get_current()
            .unwrap()
            .to_explicit_target();
        let tool = UserProfileTool::new(
            store.clone(),
            security(),
            Some(conversation_context),
            Some(profile_context),
        );

        let mut facts = serde_json::Map::new();
        facts.insert(
            DELIVERY_TARGET_PREFERENCE_KEY.to_string(),
            serde_json::to_value(target).unwrap(),
        );

        let result = tool
            .execute(json!({
                "action": "upsert",
                "facts": facts
            }))
            .await
            .unwrap();

        assert!(result.success);
        match store
            .load("web:abc")
            .unwrap()
            .get_delivery_target(DELIVERY_TARGET_PREFERENCE_KEY)
        {
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
