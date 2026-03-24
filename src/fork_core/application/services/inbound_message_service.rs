//! Inbound message service — owns message classification and routing policy.
//!
//! Phase 4.0 Slice 2: upgrades the `inbound_message` bridge module into a
//! real application service.  Extracts pure business logic from
//! `channels/mod.rs` into fork_core.
//!
//! Business rules this service owns:
//! - runtime command parsing (/models, /model, /new)
//! - conversation key construction (per-sender, per-thread isolation)
//! - message classification (command vs regular message)
//! - route selection management (provider/model overrides per sender)

use crate::fork_core::domain::channel::{ChannelCapability, InboundEnvelope};

// ── Runtime commands ─────────────────────────────────────────────

/// Runtime commands that can be issued from channel messages.
///
/// These are the fork-owned domain commands.  The adapter layer
/// (channels/mod.rs) handles execution and response formatting;
/// the core owns parsing and classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    /// Show available providers.
    ShowProviders,
    /// Switch to a specific provider.
    SetProvider(String),
    /// Show current model.
    ShowModel,
    /// Switch to a specific model.
    SetModel(String),
    /// Start a new conversation session.
    NewSession,
}

/// Parse a runtime command from message content.
///
/// Returns `None` if:
/// - the channel doesn't have `RuntimeCommands` capability
/// - the content doesn't start with `/`
/// - the command is not recognized
///
/// This is pure domain logic — no adapter or infrastructure dependencies.
pub fn parse_runtime_command(
    content: &str,
    caps: &[ChannelCapability],
) -> Option<RuntimeCommand> {
    if !caps.contains(&ChannelCapability::RuntimeCommands) {
        return None;
    }

    let trimmed = content.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let command_token = parts.next()?;
    // Strip bot mention suffix (e.g. "/models@botname" → "/models")
    let base_command = command_token
        .split('@')
        .next()
        .unwrap_or(command_token)
        .to_ascii_lowercase();

    match base_command.as_str() {
        "/models" => {
            if let Some(provider) = parts.next() {
                Some(RuntimeCommand::SetProvider(provider.trim().to_string()))
            } else {
                Some(RuntimeCommand::ShowProviders)
            }
        }
        "/model" => {
            let model = parts.collect::<Vec<_>>().join(" ").trim().to_string();
            if model.is_empty() {
                Some(RuntimeCommand::ShowModel)
            } else {
                Some(RuntimeCommand::SetModel(model))
            }
        }
        "/new" => Some(RuntimeCommand::NewSession),
        _ => None,
    }
}

// ── Conversation key ─────────────────────────────────────────────

/// Construct the canonical conversation key for an inbound envelope.
///
/// Rules:
/// - Per-sender isolation: each sender gets their own conversation
/// - Per-thread isolation: same sender in different threads = different sessions
/// - Key format: `{adapter}_{thread}_{actor}` or `{adapter}_{actor}`
pub fn conversation_key(envelope: &InboundEnvelope) -> String {
    match &envelope.thread_ref {
        Some(tid) => format!(
            "{}_{}_{}",
            envelope.source_adapter, tid, envelope.actor_id
        ),
        None => format!("{}_{}", envelope.source_adapter, envelope.actor_id),
    }
}

// ── Message classification ───────────────────────────────────────

/// Classification result for an inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageClassification {
    /// A runtime command that should be handled by the command dispatcher.
    Command(RuntimeCommand),
    /// A regular message that should enter the conversation flow.
    RegularMessage,
}

/// Classify an inbound message.
///
/// First checks for runtime commands (if the channel supports them),
/// then falls through to regular message processing.
pub fn classify_message(
    content: &str,
    caps: &[ChannelCapability],
) -> MessageClassification {
    if let Some(cmd) = parse_runtime_command(content, caps) {
        return MessageClassification::Command(cmd);
    }
    MessageClassification::RegularMessage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork_core::domain::channel::SourceKind;

    fn caps_with_runtime() -> Vec<ChannelCapability> {
        vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ]
    }

    fn caps_without_runtime() -> Vec<ChannelCapability> {
        vec![ChannelCapability::SendText]
    }

    // ── parse_runtime_command tests ──────────────────────────────

    #[test]
    fn parse_models_show() {
        let cmd = parse_runtime_command("/models", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowProviders));
    }

    #[test]
    fn parse_models_set_provider() {
        let cmd = parse_runtime_command("/models anthropic", &caps_with_runtime());
        assert_eq!(
            cmd,
            Some(RuntimeCommand::SetProvider("anthropic".into()))
        );
    }

    #[test]
    fn parse_model_show() {
        let cmd = parse_runtime_command("/model", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowModel));
    }

    #[test]
    fn parse_model_set() {
        let cmd = parse_runtime_command("/model claude-3-opus", &caps_with_runtime());
        assert_eq!(
            cmd,
            Some(RuntimeCommand::SetModel("claude-3-opus".into()))
        );
    }

    #[test]
    fn parse_new_session() {
        let cmd = parse_runtime_command("/new", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::NewSession));
    }

    #[test]
    fn parse_unknown_command_returns_none() {
        let cmd = parse_runtime_command("/unknown", &caps_with_runtime());
        assert_eq!(cmd, None);
    }

    #[test]
    fn parse_no_slash_returns_none() {
        let cmd = parse_runtime_command("hello world", &caps_with_runtime());
        assert_eq!(cmd, None);
    }

    #[test]
    fn parse_without_capability_returns_none() {
        let cmd = parse_runtime_command("/models", &caps_without_runtime());
        assert_eq!(cmd, None);
    }

    #[test]
    fn parse_case_insensitive() {
        let cmd = parse_runtime_command("/Models", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowProviders));
    }

    #[test]
    fn parse_strips_bot_mention() {
        let cmd = parse_runtime_command("/models@mybot", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowProviders));
    }

    #[test]
    fn parse_with_leading_whitespace() {
        let cmd = parse_runtime_command("  /new  ", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::NewSession));
    }

    // ── conversation_key tests ───────────────────────────────────

    #[test]
    fn key_without_thread() {
        let env = InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user123".into(),
            conversation_ref: String::new(),
            reply_ref: String::new(),
            thread_ref: None,
            content: String::new(),
            received_at: 0,
        };
        assert_eq!(conversation_key(&env), "telegram_user123");
    }

    #[test]
    fn key_with_thread() {
        let env = InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "slack".into(),
            actor_id: "user456".into(),
            conversation_ref: String::new(),
            reply_ref: String::new(),
            thread_ref: Some("thread789".into()),
            content: String::new(),
            received_at: 0,
        };
        assert_eq!(conversation_key(&env), "slack_thread789_user456");
    }

    // ── classify_message tests ───────────────────────────────────

    #[test]
    fn classify_command() {
        let cls = classify_message("/models", &caps_with_runtime());
        assert_eq!(
            cls,
            MessageClassification::Command(RuntimeCommand::ShowProviders)
        );
    }

    #[test]
    fn classify_regular() {
        let cls = classify_message("hello world", &caps_with_runtime());
        assert_eq!(cls, MessageClassification::RegularMessage);
    }

    #[test]
    fn classify_command_without_capability_is_regular() {
        let cls = classify_message("/models", &caps_without_runtime());
        assert_eq!(cls, MessageClassification::RegularMessage);
    }
}
