//! Channel domain types for the fork-owned application core.
//!
//! Phase 4.0 introduces capability-driven channel behavior. This module
//! defines the canonical `OutboundIntent` — the core says *what* must happen,
//! and the adapter decides *how* to express it on a specific transport.

use std::fmt;

use crate::ports::provider::MediaArtifact;

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

pub fn web_channel_capabilities() -> Vec<ChannelCapability> {
    vec![
        ChannelCapability::SendText,
        ChannelCapability::ReceiveText,
        ChannelCapability::Threads,
        ChannelCapability::Attachments,
        ChannelCapability::Typing,
        ChannelCapability::RuntimeCommands,
        ChannelCapability::ToolContextDisplay,
    ]
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
    pub conversation_id: String,
    /// Stable upstream event/message identifier when the source provides one.
    pub event_ref: Option<String>,
    /// Platform-specific reply target (e.g. Telegram chat ID, Matrix room ID).
    pub reply_ref: String,
    /// Optional thread reference for threaded replies.
    pub thread_ref: Option<String>,
    /// Typed inbound media references supplied by transport adapters.
    pub media_attachments: Vec<InboundMediaAttachment>,
    /// Message text content.
    pub content: String,
    /// Unix timestamp (seconds).
    pub received_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundMediaKind {
    Image,
    Audio,
    Video,
    File,
}

impl InboundMediaKind {
    pub fn marker_label(self) -> &'static str {
        match self {
            Self::Image => "IMAGE",
            Self::Audio => "AUDIO",
            Self::Video => "VIDEO",
            Self::File => "FILE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMediaAttachment {
    pub kind: InboundMediaKind,
    pub uri: String,
    pub mime_type: Option<String>,
    pub label: Option<String>,
}

impl InboundMediaAttachment {
    pub fn new(kind: InboundMediaKind, uri: impl Into<String>) -> Self {
        Self {
            kind,
            uri: uri.into(),
            mime_type: None,
            label: None,
        }
    }

    pub fn marker(&self) -> Option<String> {
        let uri = self.uri.trim();
        (!uri.is_empty()).then(|| format!("[{}:{}]", self.kind.marker_label(), uri))
    }
}

/// Stable identity for an inbound conversation turn.
///
/// Transport adapters may carry very different delivery metadata, but runtime
/// policy must derive history, route, memory, and profile keys from this single
/// shape rather than from ad-hoc web/channel strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationIdentity {
    pub agent_id: String,
    pub transport: String,
    pub conversation_id: String,
    pub actor_id: String,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub reply_target: String,
}

impl ConversationIdentity {
    pub fn from_envelope(agent_id: impl Into<String>, envelope: &InboundEnvelope) -> Self {
        let transport = if envelope.source_adapter.trim().is_empty() {
            envelope.source_kind.to_string()
        } else {
            envelope.source_adapter.clone()
        };
        let conversation_id = if !envelope.conversation_id.trim().is_empty() {
            envelope.conversation_id.clone()
        } else if !envelope.reply_ref.trim().is_empty() {
            envelope.reply_ref.clone()
        } else {
            envelope.actor_id.clone()
        };
        Self {
            agent_id: agent_id.into(),
            transport,
            conversation_id,
            actor_id: envelope.actor_id.clone(),
            thread_id: envelope.thread_ref.clone(),
            message_id: envelope.event_ref.clone(),
            reply_target: envelope.reply_ref.clone(),
        }
    }

    pub fn conversation_key(&self) -> String {
        let mut parts = vec![
            "conversation".to_string(),
            key_component(&self.agent_id),
            key_component(&self.transport),
            key_component(&self.conversation_id),
        ];
        if let Some(thread_id) = self
            .thread_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            parts.push(key_component(thread_id));
        }
        parts.push(key_component(&self.actor_id));
        parts.join(":")
    }

    pub fn conversation_scope_key_prefix(&self) -> String {
        let parts = [
            "conversation".to_string(),
            key_component(&self.agent_id),
            key_component(&self.transport),
            key_component(&self.conversation_id),
        ];
        format!("{}:", parts.join(":"))
    }

    pub fn actor_profile_key(&self) -> String {
        [
            "user".to_string(),
            key_component(&self.agent_id),
            key_component(&self.transport),
            key_component(&self.actor_id),
        ]
        .join(":")
    }

    pub fn parent_conversation_key(&self) -> String {
        let mut parent = self.clone();
        parent.thread_id = None;
        parent.conversation_key()
    }

    pub fn autosave_memory_key(&self, received_at: u64, content_chars: usize) -> String {
        if let Some(message_id) = self
            .message_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return format!(
                "channel:{}:{}",
                self.conversation_key(),
                key_component(message_id)
            );
        }

        format!(
            "channel:{}:recv{}:len{}",
            self.conversation_key(),
            received_at,
            content_chars
        )
    }
}

fn key_component(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "_".to_string();
    }
    trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '@') {
                ch
            } else {
                '_'
            }
        })
        .collect()
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
    /// Typed inbound media references supplied by transport adapters.
    ///
    /// Transport metadata belongs here, not in provider-facing prompt prose.
    /// Legacy text markers remain supported by the adapter-core normalizer for
    /// compatibility while channel adapters are migrated.
    pub media_attachments: Vec<InboundMediaAttachment>,
}

/// Message to send through a channel adapter.
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub content: String,
    pub recipient: String,
    pub subject: Option<String>,
    /// Platform thread identifier for threaded replies (e.g. Slack `thread_ts`).
    pub thread_ts: Option<String>,
    pub media_artifacts: Vec<MediaArtifact>,
}

impl SendMessage {
    /// Create a new message with content and recipient.
    pub fn new(content: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            recipient: recipient.into(),
            subject: None,
            thread_ts: None,
            media_artifacts: Vec::new(),
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
            media_artifacts: Vec::new(),
        }
    }

    /// Set the thread identifier for threaded replies.
    pub fn in_thread(mut self, thread_ts: Option<String>) -> Self {
        self.thread_ts = thread_ts;
        self
    }

    pub fn with_media_artifacts(mut self, media_artifacts: Vec<MediaArtifact>) -> Self {
        self.media_artifacts = media_artifacts;
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
