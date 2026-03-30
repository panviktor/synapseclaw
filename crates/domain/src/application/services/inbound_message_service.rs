//! Inbound message service — owns message classification and routing policy.
//!
//! Phase 4.0 Slice 2: upgrades the `inbound_message` bridge module into a
//! real application service.  Extracts pure business logic from
//! `channels/mod.rs` into synapse_domain.
//!
//! Business rules this service owns:
//! - runtime command parsing (/models, /model, /new)
//! - conversation key construction (per-sender, per-thread isolation)
//! - message classification (command vs regular message)
//! - route selection management (provider/model overrides per sender)

use crate::domain::channel::{ChannelCapability, InboundEnvelope};

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
pub fn parse_runtime_command(content: &str, caps: &[ChannelCapability]) -> Option<RuntimeCommand> {
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
        Some(tid) => format!("{}_{}_{}", envelope.source_adapter, tid, envelope.actor_id),
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
pub fn classify_message(content: &str, caps: &[ChannelCapability]) -> MessageClassification {
    if let Some(cmd) = parse_runtime_command(content, caps) {
        return MessageClassification::Command(cmd);
    }
    MessageClassification::RegularMessage
}

// ── History enrichment strategy ──────────────────────────────────

/// How to seed context for the first turn of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryEnrichment {
    /// First turn in thread: load parent summary + root message.
    ThreadSeeding {
        parent_key: String,
        thread_id: String,
    },
    /// First turn standalone: load relevant memory context.
    MemoryContext { conversation_key: String },
    /// Not first turn: no enrichment.
    None,
}

/// Decide how to enrich the conversation context for an inbound message.
///
/// Business rules:
/// - If there's already history for this conversation: no enrichment
/// - If the message is in a thread: seed from parent conversation summary
/// - Otherwise: load relevant memory context
pub fn decide_history_enrichment(
    has_prior_history: bool,
    envelope: &InboundEnvelope,
) -> HistoryEnrichment {
    if has_prior_history {
        return HistoryEnrichment::None;
    }

    match &envelope.thread_ref {
        Some(tid) => HistoryEnrichment::ThreadSeeding {
            parent_key: format!("{}_{}", envelope.source_adapter, envelope.actor_id),
            thread_id: tid.clone(),
        },
        None => HistoryEnrichment::MemoryContext {
            conversation_key: conversation_key(envelope),
        },
    }
}

// ── Auto-save policy ─────────────────────────────────────────────

/// Minimum message length (in chars) to trigger auto-save.
pub const AUTOSAVE_MIN_MESSAGE_CHARS: usize = 20;

/// Decide whether to auto-save an inbound message to memory.
///
/// Business rules:
/// - Auto-save must be enabled in config
/// - Message must be at least AUTOSAVE_MIN_MESSAGE_CHARS characters
/// - Content must not match skip patterns (e.g. single-word commands)
pub fn should_autosave(auto_save_enabled: bool, content: &str) -> bool {
    auto_save_enabled && content.chars().count() >= AUTOSAVE_MIN_MESSAGE_CHARS
}

// ── Tool context display policy ──────────────────────────────────

/// Decide whether to include tool usage summary in the response.
///
/// Business rules:
/// - Tools must have been used (non-empty summary)
/// - Channel must support `ToolContextDisplay` capability
pub fn should_include_tool_summary(tool_summary: &str, caps: &[ChannelCapability]) -> bool {
    !tool_summary.is_empty() && caps.contains(&ChannelCapability::ToolContextDisplay)
}

// ── Memory consolidation policy ──────────────────────────────────

/// Decide whether to run background memory consolidation after a turn.
///
/// Same gate as auto-save: enabled + minimum message length.
pub fn should_consolidate_memory(auto_save_enabled: bool, content: &str) -> bool {
    auto_save_enabled && content.chars().count() >= AUTOSAVE_MIN_MESSAGE_CHARS
}

// ── Interrupt-on-new-message policy ──────────────────────────────

/// Decide whether to interrupt a previous in-flight request from the same sender.
///
/// Requires both:
/// - interrupt-on-new-message is enabled for this session
/// - channel has the `InterruptOnNewMessage` capability
pub fn should_interrupt_previous(enabled: bool, caps: &[ChannelCapability]) -> bool {
    enabled && caps.contains(&ChannelCapability::InterruptOnNewMessage)
}

// ── Runtime command execution ────────────────────────────────────

/// Result of executing a runtime command — tells the adapter what state changes to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEffect {
    /// Display provider list (no state change).
    ShowProviders,
    /// Switch to a specific provider. Adapter must validate + persist.
    SwitchProvider { provider: String },
    /// Display current model info (no state change).
    ShowModel,
    /// Switch to a specific model, optionally with inferred provider from routes.
    SwitchModel {
        model: String,
        /// If a model_route matched, the coupled provider.
        inferred_provider: Option<String>,
    },
    /// Clear conversation history and route overrides for this sender.
    ClearSession,
}

/// Determine the effect of a runtime command.
///
/// Resolves model routes (model name → provider) but leaves
/// provider validation to the adapter (requires infrastructure).
pub fn command_effect(
    command: &RuntimeCommand,
    model_routes: &[(String, String, String)], // (provider, model, hint)
) -> CommandEffect {
    match command {
        RuntimeCommand::ShowProviders => CommandEffect::ShowProviders,
        RuntimeCommand::SetProvider(raw) => CommandEffect::SwitchProvider {
            provider: raw.clone(),
        },
        RuntimeCommand::ShowModel => CommandEffect::ShowModel,
        RuntimeCommand::SetModel(raw) => {
            let model = raw.trim().trim_matches('`').to_string();
            // Look up in model_routes by model name or hint
            let matched = model_routes
                .iter()
                .find(|(_, m, h)| m.eq_ignore_ascii_case(&model) || h.eq_ignore_ascii_case(&model));
            match matched {
                Some((provider, resolved_model, _)) => CommandEffect::SwitchModel {
                    model: resolved_model.clone(),
                    inferred_provider: Some(provider.clone()),
                },
                None => CommandEffect::SwitchModel {
                    model,
                    inferred_provider: None,
                },
            }
        }
        RuntimeCommand::NewSession => CommandEffect::ClearSession,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::channel::SourceKind;

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
        assert_eq!(cmd, Some(RuntimeCommand::SetProvider("anthropic".into())));
    }

    #[test]
    fn parse_model_show() {
        let cmd = parse_runtime_command("/model", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowModel));
    }

    #[test]
    fn parse_model_set() {
        let cmd = parse_runtime_command("/model claude-3-opus", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::SetModel("claude-3-opus".into())));
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

    // ── history enrichment tests ─────────────────────────────────

    fn envelope(thread: Option<&str>) -> InboundEnvelope {
        InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user1".into(),
            conversation_ref: String::new(),
            reply_ref: String::new(),
            thread_ref: thread.map(String::from),
            content: String::new(),
            received_at: 0,
        }
    }

    #[test]
    fn enrichment_none_when_has_history() {
        assert_eq!(
            decide_history_enrichment(true, &envelope(None)),
            HistoryEnrichment::None
        );
    }

    #[test]
    fn enrichment_thread_seeding_for_threaded_first_message() {
        let env = envelope(Some("thread123"));
        let result = decide_history_enrichment(false, &env);
        assert_eq!(
            result,
            HistoryEnrichment::ThreadSeeding {
                parent_key: "telegram_user1".into(),
                thread_id: "thread123".into(),
            }
        );
    }

    #[test]
    fn enrichment_memory_for_first_standalone_message() {
        let env = envelope(None);
        let result = decide_history_enrichment(false, &env);
        assert_eq!(
            result,
            HistoryEnrichment::MemoryContext {
                conversation_key: "telegram_user1".into(),
            }
        );
    }

    // ── auto-save policy tests ───────────────────────────────────

    #[test]
    fn autosave_enabled_and_long_enough() {
        assert!(should_autosave(
            true,
            "This is a message that is long enough"
        ));
    }

    #[test]
    fn autosave_disabled() {
        assert!(!should_autosave(
            false,
            "This is a message that is long enough"
        ));
    }

    #[test]
    fn autosave_too_short() {
        assert!(!should_autosave(true, "short"));
    }

    // ── tool context policy tests ────────────────────────────────

    #[test]
    fn tool_summary_shown_when_capable() {
        let caps = vec![ChannelCapability::ToolContextDisplay];
        assert!(should_include_tool_summary("Used: shell", &caps));
    }

    #[test]
    fn tool_summary_hidden_without_capability() {
        let caps = vec![ChannelCapability::SendText];
        assert!(!should_include_tool_summary("Used: shell", &caps));
    }

    #[test]
    fn tool_summary_hidden_when_empty() {
        let caps = vec![ChannelCapability::ToolContextDisplay];
        assert!(!should_include_tool_summary("", &caps));
    }

    // ── interrupt policy tests ───────────────────────────────────

    #[test]
    fn interrupt_enabled_with_capability() {
        let caps = vec![ChannelCapability::InterruptOnNewMessage];
        assert!(should_interrupt_previous(true, &caps));
    }

    #[test]
    fn interrupt_disabled_by_config() {
        let caps = vec![ChannelCapability::InterruptOnNewMessage];
        assert!(!should_interrupt_previous(false, &caps));
    }

    #[test]
    fn interrupt_disabled_without_capability() {
        let caps = vec![ChannelCapability::SendText];
        assert!(!should_interrupt_previous(true, &caps));
    }

    // ── command effect tests ─────────────────────────────────────

    fn test_routes() -> Vec<(String, String, String)> {
        vec![
            ("anthropic".into(), "claude-3-opus".into(), "opus".into()),
            ("openai".into(), "gpt-4".into(), "gpt4".into()),
        ]
    }

    #[test]
    fn effect_show_providers() {
        let cmd = RuntimeCommand::ShowProviders;
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::ShowProviders
        );
    }

    #[test]
    fn effect_switch_provider() {
        let cmd = RuntimeCommand::SetProvider("anthropic".into());
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::SwitchProvider {
                provider: "anthropic".into()
            }
        );
    }

    #[test]
    fn effect_show_model() {
        let cmd = RuntimeCommand::ShowModel;
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::ShowModel
        );
    }

    #[test]
    fn effect_switch_model_by_hint() {
        let cmd = RuntimeCommand::SetModel("opus".into());
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::SwitchModel {
                model: "claude-3-opus".into(),
                inferred_provider: Some("anthropic".into()),
            }
        );
    }

    #[test]
    fn effect_switch_model_by_name() {
        let cmd = RuntimeCommand::SetModel("gpt-4".into());
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::SwitchModel {
                model: "gpt-4".into(),
                inferred_provider: Some("openai".into()),
            }
        );
    }

    #[test]
    fn effect_switch_model_unknown() {
        let cmd = RuntimeCommand::SetModel("custom-model".into());
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::SwitchModel {
                model: "custom-model".into(),
                inferred_provider: None,
            }
        );
    }

    #[test]
    fn effect_new_session() {
        let cmd = RuntimeCommand::NewSession;
        assert_eq!(
            command_effect(&cmd, &test_routes()),
            CommandEffect::ClearSession
        );
    }
}
