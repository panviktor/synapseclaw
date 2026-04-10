//! Channel domain types for the fork-owned application core.
//!
//! Phase 4.0 introduces capability-driven channel behavior. This module
//! defines the canonical `OutboundIntent` — the core says *what* must happen,
//! and the adapter decides *how* to express it on a specific transport.

use std::fmt;

/// What the core wants to happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentKind {
    /// Reply to a user in the originating conversation.
    Reply,
    /// Notify a channel/user proactively (heartbeat, scheduled, relay).
    Notify,
    /// Escalation to the operator or approval channel.
    Escalation,
}

impl fmt::Display for IntentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reply => write!(f, "reply"),
            Self::Notify => write!(f, "notify"),
            Self::Escalation => write!(f, "escalation"),
        }
    }
}

/// What to do when a required capability is absent on the target channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DegradationPolicy {
    /// Send as plain text (strip formatting, skip threads).
    #[default]
    PlainText,
    /// Drop the intent silently.
    Drop,
}

/// Content that adapters can render per-platform.
#[derive(Debug, Clone)]
pub enum RenderableContent {
    /// Plain UTF-8 text.
    Text(String),
}

impl RenderableContent {
    /// Extract the text content regardless of variant.
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(s) => s,
        }
    }
}

/// Capabilities a channel adapter can declare.
///
/// Application services check these instead of branching on channel names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelCapability {
    SendText,
    ReceiveText,
    Threads,
    Reactions,
    Typing,
    Attachments,
    RichFormatting,
    EditMessage,
    /// Channel supports runtime /models, /model, /new commands.
    RuntimeCommands,
    /// Channel supports interrupt-on-new-message (cancel previous run).
    InterruptOnNewMessage,
    /// Channel supports displaying tool call context in history.
    ToolContextDisplay,
}

/// The canonical outbound intent — Phase 4.0's first domain object.
///
/// The application core emits intents; adapters translate them into
/// platform-specific API calls.  This replaces ad-hoc channel-name
/// branching for the push-relay use case (first vertical slice).
#[derive(Debug, Clone)]
pub struct OutboundIntent {
    /// What the core wants to happen.
    pub intent_kind: IntentKind,
    /// Target channel adapter name (e.g. "telegram", "matrix").
    pub target_channel: String,
    /// Platform-specific recipient (e.g. Telegram chat ID, Matrix room ID).
    pub target_recipient: String,
    /// Optional thread reference for threaded replies.
    pub thread_ref: Option<String>,
    /// The content to deliver.
    pub content: RenderableContent,
    /// Capabilities required for full-fidelity delivery.
    pub required_capabilities: Vec<ChannelCapability>,
    /// Fallback behavior when capabilities are missing.
    pub degradation_policy: DegradationPolicy,
}

impl OutboundIntent {
    /// Create a simple text notification intent.
    pub fn notify(channel: impl Into<String>, recipient: impl Into<String>, text: String) -> Self {
        Self::notify_in_thread(channel, recipient, None, text)
    }

    /// Create a simple text notification intent, optionally preserving thread/topic context.
    pub fn notify_in_thread(
        channel: impl Into<String>,
        recipient: impl Into<String>,
        thread_ref: Option<String>,
        text: String,
    ) -> Self {
        Self {
            intent_kind: IntentKind::Notify,
            target_channel: channel.into(),
            target_recipient: recipient.into(),
            thread_ref,
            content: RenderableContent::Text(text),
            required_capabilities: vec![ChannelCapability::SendText],
            degradation_policy: DegradationPolicy::default(),
        }
    }
}

/// Where the inbound message originated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// Human messaging channel (Telegram, Matrix, Slack, etc.)
    Channel,
    /// Web dashboard chat (WebSocket).
    Web,
    /// Inter-agent IPC message.
    Ipc,
    /// Cron/scheduler-triggered prompt.
    Cron,
    /// Internal system event.
    System,
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Channel => write!(f, "channel"),
            Self::Web => write!(f, "web"),
            Self::Ipc => write!(f, "ipc"),
            Self::Cron => write!(f, "cron"),
            Self::System => write!(f, "system"),
        }
    }
}

/// Canonical inbound envelope — Phase 4.0's unified input type.
///
/// Every inbound message-like event (channel message, web chat, IPC task,
/// cron prompt) becomes one `InboundEnvelope`. Application services reason
/// on this single type instead of channel-specific message structs.
#[derive(Debug, Clone)]
pub struct InboundEnvelope {
    /// Where this message came from (channel, web, ipc, cron, system).
    pub source_kind: SourceKind,
    /// Specific adapter name (e.g. "telegram", "matrix", "cli", "web").
    pub source_adapter: String,
    /// Who sent the message (user ID, agent ID, "system").
    pub actor_id: String,
    /// Conversation key for history lookup (e.g. "telegram_123456", "web:session:abc").
    pub conversation_ref: String,
    /// Stable upstream event/message identifier when the source provides one.
    pub event_ref: Option<String>,
    /// Platform-specific reply target (e.g. Telegram chat ID, Matrix room ID).
    pub reply_ref: String,
    /// Optional thread reference for threaded replies.
    pub thread_ref: Option<String>,
    /// Message text content.
    pub content: String,
    /// Unix timestamp (seconds).
    pub received_at: u64,
}

// ── Channel message types ────────────────────────────────────────────

/// A message received from a channel adapter.
#[derive(Debug, Clone)]
pub struct ChannelMessage {
    pub id: String,
    pub sender: String,
    pub reply_target: String,
    pub content: String,
    pub channel: String,
    pub timestamp: u64,
    /// Platform thread identifier (e.g. Slack `ts`, Discord thread ID).
    /// When set, replies should be posted as threaded responses.
    pub thread_ts: Option<String>,
}

/// Message to send through a channel adapter.
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
    pub subject: Option<String>,
    /// Platform thread identifier for threaded replies (e.g. Slack `thread_ts`).
    pub thread_ts: Option<String>,
}

impl SendMessage {
    /// Create a new message with content and recipient.
    pub fn new(content: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: None,
            thread_ts: None,
        }
    }

    /// Create a new message with content, recipient, and subject.
    pub fn with_subject(
        content: impl Into<String>,
        recipient: impl Into<String>,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: Some(subject.into()),
            thread_ts: None,
        }
    }

    /// Set the thread identifier for threaded replies.
    pub fn in_thread(mut self, thread_ts: Option<String>) -> Self {
        self.thread_ts = thread_ts;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_builds_correct_intent() {
        let intent = OutboundIntent::notify("telegram", "123456", "Hello".into());
        assert_eq!(intent.intent_kind, IntentKind::Notify);
        assert_eq!(intent.target_channel, "telegram");
        assert_eq!(intent.target_recipient, "123456");
        assert_eq!(intent.content.as_text(), "Hello");
        assert_eq!(
            intent.required_capabilities,
            vec![ChannelCapability::SendText]
        );
        assert_eq!(intent.degradation_policy, DegradationPolicy::PlainText);
        assert!(intent.thread_ref.is_none());
    }

    #[test]
    fn intent_kind_display() {
        assert_eq!(IntentKind::Reply.to_string(), "reply");
        assert_eq!(IntentKind::Notify.to_string(), "notify");
        assert_eq!(IntentKind::Escalation.to_string(), "escalation");
    }

    #[test]
    fn renderable_content_as_text() {
        let content = RenderableContent::Text("test content".into());
        assert_eq!(content.as_text(), "test content");
    }

    #[test]
    fn source_kind_display() {
        assert_eq!(SourceKind::Channel.to_string(), "channel");
        assert_eq!(SourceKind::Web.to_string(), "web");
        assert_eq!(SourceKind::Ipc.to_string(), "ipc");
    }
}
