//! Use case: HandleInboundMessage — full orchestration of an inbound message.
//!
//! Phase 4.0 Slice 2: replaces the monolithic `process_channel_message` in
//! channels/mod.rs with a port-driven orchestrator in fork_core.
//!
//! The orchestrator owns the business flow:
//! 1. Hook: on_message_received
//! 2. Classify message (command vs regular)
//! 3. If command → resolve effect → delegate to adapter for execution
//! 4. If regular → resolve route → enrich context → run agent → deliver response
//!
//! All infrastructure calls go through injected ports.

use crate::fork_core::application::services::inbound_message_service::{
    self, CommandEffect, HistoryEnrichment, MessageClassification, RuntimeCommand,
};
use crate::fork_core::domain::channel::{ChannelCapability, InboundEnvelope};
use crate::fork_core::ports::agent_runtime::{AgentRuntimePort, AgentTurnResult};
use crate::fork_core::ports::channel_output::ChannelOutputPort;
use crate::fork_core::ports::channel_registry::ChannelRegistryPort;
use crate::fork_core::ports::conversation_history::ConversationHistoryPort;
use crate::fork_core::ports::hooks::{HookOutcome, HooksPort};
use crate::fork_core::ports::route_selection::RouteSelectionPort;
use crate::fork_core::ports::session_summary::SessionSummaryPort;
use crate::providers::ChatMessage;
use anyhow::Result;
use std::sync::Arc;

/// Configuration for the inbound message handler.
#[derive(Debug, Clone)]
pub struct InboundMessageConfig {
    /// Base system prompt.
    pub system_prompt: String,
    /// Default provider name.
    pub default_provider: String,
    /// Default model name.
    pub default_model: String,
    /// Sampling temperature.
    pub temperature: f64,
    /// Max tool loop iterations.
    pub max_tool_iterations: usize,
    /// Whether auto-save memory is enabled.
    pub auto_save_memory: bool,
    /// Model routes: (provider, model, hint) tuples.
    pub model_routes: Vec<(String, String, String)>,
    /// Max chars for thread root text seeding.
    pub thread_root_max_chars: usize,
}

/// Ports required by the inbound message handler.
pub struct InboundMessagePorts {
    pub history: Arc<dyn ConversationHistoryPort>,
    pub routes: Arc<dyn RouteSelectionPort>,
    pub hooks: Arc<dyn HooksPort>,
    pub channel_output: Arc<dyn ChannelOutputPort>,
    pub agent_runtime: Arc<dyn AgentRuntimePort>,
    pub channel_registry: Arc<dyn ChannelRegistryPort>,
    pub session_summary: Option<Arc<dyn SessionSummaryPort>>,
}

/// Result of handling an inbound message — tells the adapter what happened.
#[derive(Debug, Clone)]
pub enum HandleResult {
    /// Message was a runtime command; adapter should apply the effect.
    Command {
        effect: CommandEffect,
        conversation_key: String,
    },
    /// Message was processed; agent produced a response.
    Response {
        conversation_key: String,
        response_text: String,
        tools_used: bool,
    },
    /// Message was cancelled by a hook.
    Cancelled { reason: String },
    /// Message was a command but channel output was unavailable.
    CommandNoChannel,
}

/// Handle an inbound message through the full business flow.
///
/// This is the primary use case entry point.  Adapters construct an
/// `InboundEnvelope`, call this function, and act on the `HandleResult`.
pub async fn handle(
    envelope: &InboundEnvelope,
    caps: &[ChannelCapability],
    config: &InboundMessageConfig,
    ports: &InboundMessagePorts,
) -> Result<HandleResult> {
    let conversation_key = inbound_message_service::conversation_key(envelope);

    // ── 1. Hook: on_message_received ─────────────────────────────
    let content = match ports
        .hooks
        .on_message_received(
            &envelope.source_adapter,
            &envelope.actor_id,
            envelope.content.clone(),
        )
        .await
    {
        HookOutcome::Continue(c) => c,
        HookOutcome::Cancel(reason) => return Ok(HandleResult::Cancelled { reason }),
    };

    // ── 2. Classify message ──────────────────────────────────────
    let classification = inbound_message_service::classify_message(&content, caps);

    match classification {
        MessageClassification::Command(cmd) => {
            let effect =
                inbound_message_service::command_effect(&cmd, &config.model_routes);

            // Apply state changes for commands that modify session state
            match &effect {
                CommandEffect::ClearSession => {
                    ports.history.clear_history(&conversation_key);
                    ports.routes.clear_route(&conversation_key);
                }
                CommandEffect::SwitchProvider { provider } => {
                    let mut route = ports.routes.get_route(&conversation_key);
                    route.provider = provider.clone();
                    ports.routes.set_route(&conversation_key, route);
                }
                CommandEffect::SwitchModel {
                    model,
                    inferred_provider,
                } => {
                    let mut route = ports.routes.get_route(&conversation_key);
                    route.model = model.clone();
                    if let Some(p) = inferred_provider {
                        route.provider = p.clone();
                    }
                    ports.routes.set_route(&conversation_key, route);
                }
                CommandEffect::ShowProviders | CommandEffect::ShowModel => {
                    // Read-only — no state change
                }
            }

            Ok(HandleResult::Command {
                effect,
                conversation_key,
            })
        }

        MessageClassification::RegularMessage => {
            handle_regular_message(
                envelope,
                &content,
                &conversation_key,
                caps,
                config,
                ports,
            )
            .await
        }
    }
}

/// Handle a regular (non-command) message through the conversation flow.
async fn handle_regular_message(
    envelope: &InboundEnvelope,
    content: &str,
    conversation_key: &str,
    caps: &[ChannelCapability],
    config: &InboundMessageConfig,
    ports: &InboundMessagePorts,
) -> Result<HandleResult> {
    // ── 3. Resolve route ─────────────────────────────────────────
    let route = ports.routes.get_route(conversation_key);

    // ── 4. Check prior history ───────────────────────────────────
    let has_prior = ports.history.has_history(conversation_key);

    // ── 5. Build initial history ─────────────────────────────────
    let mut history = vec![ChatMessage::system(config.system_prompt.clone())];

    // Add delivery hints from channel registry
    if let Some(hints) = ports.channel_registry.delivery_hints(&envelope.source_adapter) {
        history.push(ChatMessage::system(hints));
    }

    // Add prior turns
    let prior_turns = ports.history.get_history(conversation_key);
    history.extend(prior_turns);

    // ── 6. Enrich context for first turn ─────────────────────────
    let enrichment = inbound_message_service::decide_history_enrichment(has_prior, envelope);
    match enrichment {
        HistoryEnrichment::ThreadSeeding {
            parent_key,
            thread_id,
        } => {
            // Load parent summary
            if let Some(ref summary_port) = ports.session_summary {
                if let Some(summary) = summary_port.load_summary(&parent_key) {
                    let seeding = format!(
                        "[Thread context — parent conversation summary]\n{summary}"
                    );
                    history.push(ChatMessage::system(seeding));
                }
            }
            // Fetch thread root text
            if let Ok(Some(root_text)) =
                ports.channel_output.fetch_message_text(&thread_id).await
            {
                let truncated = if root_text.chars().count() > config.thread_root_max_chars {
                    let s: String = root_text.chars().take(config.thread_root_max_chars).collect();
                    format!("{s}…")
                } else {
                    root_text
                };
                history.push(ChatMessage::system(format!(
                    "[Thread root message]\n{truncated}"
                )));
            }
        }
        HistoryEnrichment::MemoryContext { .. } => {
            // Memory enrichment is handled by the adapter for now —
            // requires MemoryPort which is Slice 6.
        }
        HistoryEnrichment::None => {}
    }

    // ── 7. Append user turn ──────────────────────────────────────
    ports
        .history
        .append_turn(conversation_key, ChatMessage::user(content));
    history.push(ChatMessage::user(content));

    // ── 8. Start typing indicator ────────────────────────────────
    let _ = ports
        .channel_output
        .start_typing(&envelope.reply_ref)
        .await;

    // ── 9. Execute agent turn ────────────────────────────────────
    let result = ports
        .agent_runtime
        .execute_turn(
            history,
            &route.provider,
            &route.model,
            config.temperature,
            config.max_tool_iterations,
        )
        .await;

    // ── 10. Stop typing ──────────────────────────────────────────
    let _ = ports
        .channel_output
        .stop_typing(&envelope.reply_ref)
        .await;

    match result {
        Ok(turn_result) => {
            // ── 11. Hook: on_message_sending ─────────────────────
            let response_text = match ports
                .hooks
                .on_message_sending(
                    &envelope.source_adapter,
                    &envelope.reply_ref,
                    turn_result.response.clone(),
                )
                .await
            {
                HookOutcome::Continue(text) => text,
                HookOutcome::Cancel(reason) => {
                    // Rollback user turn since we won't respond
                    ports
                        .history
                        .rollback_last_turn(conversation_key, content);
                    return Ok(HandleResult::Cancelled { reason });
                }
            };

            // ── 12. Persist assistant turn ───────────────────────
            ports.history.append_turn(
                conversation_key,
                ChatMessage::assistant(&response_text),
            );

            Ok(HandleResult::Response {
                conversation_key: conversation_key.to_string(),
                response_text,
                tools_used: turn_result.tools_used,
            })
        }
        Err(e) => {
            // Rollback orphan user turn
            ports
                .history
                .rollback_last_turn(conversation_key, content);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork_core::domain::channel::SourceKind;
    use crate::fork_core::ports::hooks::NoOpHooks;
    use crate::fork_core::ports::route_selection::RouteSelection;
    use async_trait::async_trait;
    use std::sync::Mutex;

    // ── Mock implementations ─────────────────────────────────────

    struct MockHistory {
        turns: Mutex<std::collections::HashMap<String, Vec<ChatMessage>>>,
    }

    impl MockHistory {
        fn new() -> Self {
            Self {
                turns: Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    impl ConversationHistoryPort for MockHistory {
        fn has_history(&self, key: &str) -> bool {
            self.turns
                .lock()
                .unwrap()
                .get(key)
                .is_some_and(|v| !v.is_empty())
        }
        fn get_history(&self, key: &str) -> Vec<ChatMessage> {
            self.turns
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .unwrap_or_default()
        }
        fn append_turn(&self, key: &str, turn: ChatMessage) {
            self.turns
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_default()
                .push(turn);
        }
        fn clear_history(&self, key: &str) {
            self.turns.lock().unwrap().remove(key);
        }
        fn compact_history(&self, _key: &str, _keep: usize) -> bool {
            false
        }
        fn rollback_last_turn(&self, key: &str, _expected: &str) -> bool {
            if let Some(turns) = self.turns.lock().unwrap().get_mut(key) {
                turns.pop();
                return true;
            }
            false
        }
        fn prepend_turn(&self, key: &str, turn: ChatMessage) {
            self.turns
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_default()
                .insert(0, turn);
        }
    }

    struct MockRoutes {
        default_provider: String,
        default_model: String,
    }

    impl RouteSelectionPort for MockRoutes {
        fn get_route(&self, _key: &str) -> RouteSelection {
            RouteSelection {
                provider: self.default_provider.clone(),
                model: self.default_model.clone(),
            }
        }
        fn set_route(&self, _key: &str, _route: RouteSelection) {}
        fn clear_route(&self, _key: &str) {}
    }

    struct MockRuntime {
        response: String,
    }

    #[async_trait]
    impl AgentRuntimePort for MockRuntime {
        async fn execute_turn(
            &self,
            _history: Vec<ChatMessage>,
            _provider: &str,
            _model: &str,
            _temp: f64,
            _max_iter: usize,
        ) -> Result<AgentTurnResult> {
            Ok(AgentTurnResult {
                response: self.response.clone(),
                history: vec![],
                tools_used: false,
            })
        }
    }

    struct MockChannelOutput;

    #[async_trait]
    impl ChannelOutputPort for MockChannelOutput {
        async fn send_message(&self, _r: &str, _t: &str, _th: Option<&str>) -> Result<()> {
            Ok(())
        }
        async fn start_typing(&self, _r: &str) -> Result<()> {
            Ok(())
        }
        async fn stop_typing(&self, _r: &str) -> Result<()> {
            Ok(())
        }
        async fn add_reaction(&self, _r: &str, _m: &str, _e: &str) -> Result<()> {
            Ok(())
        }
        async fn remove_reaction(&self, _r: &str, _m: &str, _e: &str) -> Result<()> {
            Ok(())
        }
        async fn fetch_message_text(&self, _m: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn supports_streaming(&self) -> bool {
            false
        }
    }

    struct MockRegistry;

    #[async_trait]
    impl ChannelRegistryPort for MockRegistry {
        fn resolve(&self, _n: &str) -> Result<Arc<dyn crate::channels::traits::Channel>> {
            anyhow::bail!("mock")
        }
        fn capabilities(&self, _n: &str) -> Vec<ChannelCapability> {
            vec![ChannelCapability::SendText, ChannelCapability::RuntimeCommands]
        }
        async fn deliver(
            &self,
            _i: &crate::fork_core::domain::channel::OutboundIntent,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn test_config() -> InboundMessageConfig {
        InboundMessageConfig {
            system_prompt: "You are helpful.".into(),
            default_provider: "openrouter".into(),
            default_model: "default-model".into(),
            temperature: 0.7,
            max_tool_iterations: 5,
            auto_save_memory: false,
            model_routes: vec![],
            thread_root_max_chars: 500,
        }
    }

    fn test_envelope(content: &str) -> InboundEnvelope {
        InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user1".into(),
            conversation_ref: "telegram_user1".into(),
            reply_ref: "chat123".into(),
            thread_ref: None,
            content: content.into(),
            received_at: 0,
        }
    }

    fn test_ports(response: &str) -> InboundMessagePorts {
        InboundMessagePorts {
            history: Arc::new(MockHistory::new()),
            routes: Arc::new(MockRoutes {
                default_provider: "openrouter".into(),
                default_model: "default-model".into(),
            }),
            hooks: Arc::new(NoOpHooks),
            channel_output: Arc::new(MockChannelOutput),
            agent_runtime: Arc::new(MockRuntime {
                response: response.into(),
            }),
            channel_registry: Arc::new(MockRegistry),
            session_summary: None,
        }
    }

    #[tokio::test]
    async fn handle_regular_message_returns_response() {
        let env = test_envelope("Hello");
        let caps = vec![ChannelCapability::SendText];
        let config = test_config();
        let ports = test_ports("Hi there!");

        let result = handle(&env, &caps, &config, &ports).await.unwrap();
        match result {
            HandleResult::Response {
                response_text,
                tools_used,
                ..
            } => {
                assert_eq!(response_text, "Hi there!");
                assert!(!tools_used);
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_regular_message_persists_turns() {
        let env = test_envelope("Hello");
        let caps = vec![ChannelCapability::SendText];
        let config = test_config();
        let ports = test_ports("Hi!");

        handle(&env, &caps, &config, &ports).await.unwrap();

        // Should have user + assistant turns
        let history = ports.history.get_history("telegram_user1");
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn handle_command_returns_effect() {
        let env = test_envelope("/new");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let config = test_config();
        let ports = test_ports("");

        // Pre-populate history
        ports
            .history
            .append_turn("telegram_user1", ChatMessage::user("old msg"));

        let result = handle(&env, &caps, &config, &ports).await.unwrap();
        match result {
            HandleResult::Command { effect, .. } => {
                assert_eq!(effect, CommandEffect::ClearSession);
            }
            other => panic!("expected Command, got {other:?}"),
        }

        // History should be cleared
        assert!(!ports.history.has_history("telegram_user1"));
    }

    #[tokio::test]
    async fn handle_command_without_capability_is_regular() {
        let env = test_envelope("/new");
        // No RuntimeCommands capability
        let caps = vec![ChannelCapability::SendText];
        let config = test_config();
        let ports = test_ports("I can help with that!");

        let result = handle(&env, &caps, &config, &ports).await.unwrap();
        // Should be treated as regular message since no RuntimeCommands capability
        assert!(matches!(result, HandleResult::Response { .. }));
    }
}
