//! Use case: handle an inbound message from any source.
//!
//! Phase 4.0 Step 7 — first inbound use case.  Converts `InboundEnvelope`
//! to the existing `ChannelMessage` and delegates to `process_channel_message`.
//! This is the strangler-fig entry point: callers work with `InboundEnvelope`,
//! and the internal delegation will gradually be replaced with fork_core logic.

use crate::fork_core::domain::channel::InboundEnvelope;

/// Convert an `InboundEnvelope` back to a `ChannelMessage` for delegation
/// to the existing `process_channel_message` infrastructure.
///
/// This is the temporary bridge between the new canonical input type and
/// the old channel processing path.  As fork_core absorbs more business
/// logic, this function will shrink and eventually disappear.
pub fn to_channel_message(envelope: &InboundEnvelope) -> crate::channels::traits::ChannelMessage {
    crate::channels::traits::ChannelMessage {
        id: uuid::Uuid::new_v4().to_string(),
        sender: envelope.actor_id.clone(),
        reply_target: envelope.reply_ref.clone(),
        content: envelope.content.clone(),
        channel: envelope.source_adapter.clone(),
        timestamp: envelope.received_at,
        thread_ts: envelope.thread_ref.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork_core::domain::channel::SourceKind;

    #[test]
    fn to_channel_message_preserves_fields() {
        let env = InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user123".into(),
            conversation_ref: "telegram_user123".into(),
            reply_ref: "chat456".into(),
            thread_ref: Some("thread789".into()),
            content: "hello world".into(),
            received_at: 1_711_234_567,
        };

        let msg = to_channel_message(&env);
        assert_eq!(msg.sender, "user123");
        assert_eq!(msg.reply_target, "chat456");
        assert_eq!(msg.content, "hello world");
        assert_eq!(msg.channel, "telegram");
        assert_eq!(msg.timestamp, 1_711_234_567);
        assert_eq!(msg.thread_ts, Some("thread789".into()));
    }

    #[test]
    fn to_channel_message_no_thread() {
        let env = InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "cli".into(),
            actor_id: "user".into(),
            conversation_ref: "cli_user".into(),
            reply_ref: "user".into(),
            thread_ref: None,
            content: "test".into(),
            received_at: 0,
        };

        let msg = to_channel_message(&env);
        assert!(msg.thread_ts.is_none());
        assert_eq!(msg.channel, "cli");
    }
}
