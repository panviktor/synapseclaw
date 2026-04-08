//! Conversation delivery targets — first-class "here" abstraction.
//!
//! Lets tools and services target "the current conversation" without
//! manually discovering room IDs, chat IDs, or channel adapters.

use serde::{Deserialize, Serialize};

/// Runtime projection of where the current message came from.
///
/// Created from `InboundEnvelope` at the start of each turn and made
/// available to tools via `ConversationContextPort`.
#[derive(Debug, Clone)]
pub struct CurrentConversationContext {
    /// Channel adapter name: "telegram", "matrix", "slack", "web", etc.
    pub source_adapter: String,
    /// History/conversation key for session lookup.
    pub conversation_ref: String,
    /// Platform-specific reply target (chat_id, room_id, etc.).
    pub reply_ref: String,
    /// Thread ID for threaded replies (optional).
    pub thread_ref: Option<String>,
    /// User/actor who sent the message.
    pub actor_id: String,
}

/// Where to deliver a message — either "here" or an explicit target.
///
/// Used by cron delivery, `message_send` tool, standing orders, and
/// any future proactive flow that needs to target a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationDeliveryTarget {
    /// Deliver to wherever the current message came from.
    /// Resolved at call time from `ConversationContextPort`.
    CurrentConversation,
    /// Deliver to an explicit channel + recipient.
    Explicit {
        channel: String,
        recipient: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_ref: Option<String>,
    },
}

impl CurrentConversationContext {
    /// Convert to an explicit delivery target (snapshot for async use like cron).
    pub fn to_explicit_target(&self) -> ConversationDeliveryTarget {
        ConversationDeliveryTarget::Explicit {
            channel: self.source_adapter.clone(),
            recipient: self.reply_ref.clone(),
            thread_ref: self.thread_ref.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_conversation_serde_roundtrip() {
        let target = ConversationDeliveryTarget::CurrentConversation;
        let json = serde_json::to_string(&target).unwrap();
        assert!(json.contains("current_conversation"));
        let parsed: ConversationDeliveryTarget = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            ConversationDeliveryTarget::CurrentConversation
        ));
    }

    #[test]
    fn explicit_target_serde_roundtrip() {
        let target = ConversationDeliveryTarget::Explicit {
            channel: "matrix".into(),
            recipient: "!room:example.com".into(),
            thread_ref: Some("$event123".into()),
        };
        let json = serde_json::to_string(&target).unwrap();
        let parsed: ConversationDeliveryTarget = serde_json::from_str(&json).unwrap();
        match parsed {
            ConversationDeliveryTarget::Explicit {
                channel,
                recipient,
                thread_ref,
            } => {
                assert_eq!(channel, "matrix");
                assert_eq!(recipient, "!room:example.com");
                assert_eq!(thread_ref, Some("$event123".into()));
            }
            _ => panic!("expected Explicit"),
        }
    }

    #[test]
    fn context_to_explicit_snapshot() {
        let ctx = CurrentConversationContext {
            source_adapter: "telegram".into(),
            conversation_ref: "tg_12345".into(),
            reply_ref: "12345".into(),
            thread_ref: None,
            actor_id: "user1".into(),
        };
        let target = ctx.to_explicit_target();
        match target {
            ConversationDeliveryTarget::Explicit {
                channel,
                recipient,
                thread_ref,
            } => {
                assert_eq!(channel, "telegram");
                assert_eq!(recipient, "12345");
                assert!(thread_ref.is_none());
            }
            _ => panic!("expected Explicit"),
        }
    }
}
