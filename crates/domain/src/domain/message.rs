//! Core-owned chat message type.
//!
//! Replaces `providers::ChatMessage` inside the synapse_domain boundary.
//! Adapters convert between this and the upstream `ChatMessage`.

/// A single turn in a conversation history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    /// Role: "system", "user", "assistant".
    pub role: String,
    /// Content of the turn.
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}
