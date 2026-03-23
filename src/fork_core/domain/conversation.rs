//! Conversation domain types for the fork-owned application core.
//!
//! A `ConversationSession` is a durable session object shared by web chat
//! first and extensible to channels/IPC later.  `ConversationEvent` is
//! an event-oriented transcript entry (not just user/assistant text).

use std::fmt;

/// What kind of conversation this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationKind {
    /// Web dashboard chat (WebSocket).
    Web,
    /// Human messaging channel (Telegram, Matrix, etc.).
    Channel,
    /// Inter-agent IPC session.
    Ipc,
}

impl fmt::Display for ConversationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Web => write!(f, "web"),
            Self::Channel => write!(f, "channel"),
            Self::Ipc => write!(f, "ipc"),
        }
    }
}

/// A durable conversation/session record.
#[derive(Debug, Clone)]
pub struct ConversationSession {
    /// Unique session key (e.g. "web:token:id", "telegram_user123").
    pub key: String,
    /// Conversation kind.
    pub kind: ConversationKind,
    /// User-visible label (auto-generated or manual).
    pub label: Option<String>,
    /// Rolling summary of the conversation.
    pub summary: Option<String>,
    /// Current high-level goal.
    pub current_goal: Option<String>,
    /// Creation timestamp (unix seconds).
    pub created_at: u64,
    /// Last activity timestamp (unix seconds).
    pub last_active: u64,
    /// Total message count.
    pub message_count: u32,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
}

/// Type of transcript event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventType {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    Error,
    Interrupted,
    System,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Assistant => write!(f, "assistant"),
            Self::ToolCall => write!(f, "tool_call"),
            Self::ToolResult => write!(f, "tool_result"),
            Self::Error => write!(f, "error"),
            Self::Interrupted => write!(f, "interrupted"),
            Self::System => write!(f, "system"),
        }
    }
}

/// A single event in a conversation transcript.
///
/// Event-oriented: captures tool calls, results, errors — not just
/// user/assistant text turns.
#[derive(Debug, Clone)]
pub struct ConversationEvent {
    /// Event type.
    pub event_type: EventType,
    /// Who produced this event (user ID, "assistant", tool name).
    pub actor: String,
    /// Event content (message text, tool output, error description).
    pub content: String,
    /// Tool name (for ToolCall/ToolResult events).
    pub tool_name: Option<String>,
    /// Run ID linking events to a single agent execution.
    pub run_id: Option<String>,
    /// Input tokens consumed (for LLM events).
    pub input_tokens: Option<u32>,
    /// Output tokens produced (for LLM events).
    pub output_tokens: Option<u32>,
    /// Event timestamp (unix seconds).
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_kind_display() {
        assert_eq!(ConversationKind::Web.to_string(), "web");
        assert_eq!(ConversationKind::Channel.to_string(), "channel");
        assert_eq!(ConversationKind::Ipc.to_string(), "ipc");
    }

    #[test]
    fn event_type_display() {
        assert_eq!(EventType::User.to_string(), "user");
        assert_eq!(EventType::ToolCall.to_string(), "tool_call");
        assert_eq!(EventType::Interrupted.to_string(), "interrupted");
    }

    #[test]
    fn conversation_session_basic() {
        let session = ConversationSession {
            key: "web:abc:default".into(),
            kind: ConversationKind::Web,
            label: Some("Test session".into()),
            summary: None,
            current_goal: None,
            created_at: 1_711_234_567,
            last_active: 1_711_234_600,
            message_count: 5,
            input_tokens: 1000,
            output_tokens: 500,
        };
        assert_eq!(session.key, "web:abc:default");
        assert_eq!(session.kind, ConversationKind::Web);
        assert_eq!(session.message_count, 5);
    }

    #[test]
    fn conversation_event_basic() {
        let event = ConversationEvent {
            event_type: EventType::ToolCall,
            actor: "assistant".into(),
            content: "shell: ls -la".into(),
            tool_name: Some("shell".into()),
            run_id: Some("run-1".into()),
            input_tokens: None,
            output_tokens: None,
            timestamp: 1_711_234_567,
        };
        assert_eq!(event.event_type, EventType::ToolCall);
        assert_eq!(event.tool_name, Some("shell".into()));
    }
}
