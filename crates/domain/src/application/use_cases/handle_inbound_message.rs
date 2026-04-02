//! Use case: HandleInboundMessage — full orchestration of an inbound message.
//!
//! Phase 4.0 Slice 2: replaces the monolithic `process_channel_message` in
//! channels/mod.rs with a port-driven orchestrator in synapse_domain.
//!
//! All 24 behaviors from the original function are accounted for here.

use crate::application::services::inbound_message_service::{
    self, CommandEffect, HistoryEnrichment, MessageClassification,
};
use crate::domain::channel::{ChannelCapability, InboundEnvelope};
use crate::domain::message::ChatMessage;
use crate::ports::agent_runtime::AgentRuntimePort;
use crate::ports::channel_output::ChannelOutputPort;
use crate::ports::channel_registry::ChannelRegistryPort;
use crate::ports::conversation_history::ConversationHistoryPort;
use crate::ports::hooks::{HookOutcome, HooksPort};
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::route_selection::RouteSelectionPort;
use crate::ports::session_summary::SessionSummaryPort;
use anyhow::Result;
use std::sync::Arc;

/// Max chars for hook-modified outbound content.
const HOOK_MAX_OUTBOUND_CHARS: usize = 20_000;

/// Configuration for the inbound message handler.
#[derive(Clone)]
pub struct InboundMessageConfig {
    pub system_prompt: String,
    pub default_provider: String,
    pub default_model: String,
    pub temperature: f64,
    pub max_tool_iterations: usize,
    pub auto_save_memory: bool,
    pub model_routes: Vec<(String, String, String)>,
    pub thread_root_max_chars: usize,
    /// Max recent parent turns to inject when seeding a thread (default: 3).
    pub thread_parent_recent_turns: usize,
    /// Total char budget for parent turns excerpt (default: 2000).
    pub thread_parent_max_chars: usize,
    /// Query classification config for route override.
    /// Optional query classifier: message -> model hint.
    #[allow(clippy::type_complexity)]
    pub query_classifier: Option<std::sync::Arc<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    /// Timeout for agent execution in seconds (0 = no timeout).
    pub message_timeout_secs: u64,
    /// Min relevance for memory recall.
    pub min_relevance_score: f64,
    /// Whether to show ack reactions (👀).
    pub ack_reactions: bool,
    /// Agent identity for memory operations (core blocks, consolidation).
    #[allow(dead_code)]
    pub agent_id: String,
}

impl std::fmt::Debug for InboundMessageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundMessageConfig")
            .field("default_provider", &self.default_provider)
            .field("default_model", &self.default_model)
            .field(
                "query_classifier",
                &self.query_classifier.as_ref().map(|_| "<fn>"),
            )
            .finish()
    }
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
    pub memory: Option<Arc<dyn UnifiedMemoryPort>>,
}

/// Result of handling an inbound message.
#[derive(Debug, Clone)]
pub enum HandleResult {
    /// Runtime command — adapter formats and sends the response.
    Command {
        effect: CommandEffect,
        conversation_key: String,
    },
    /// Agent produced a response — adapter delivers it.
    Response {
        conversation_key: String,
        response_text: String,
        tool_summary: String,
        tools_used: bool,
    },
    /// Cancelled by hook.
    Cancelled { reason: String },
    /// Command without available channel.
    CommandNoChannel,
}

/// Handle an inbound message through the full business flow.
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
            let effect = inbound_message_service::command_effect(&cmd, &config.model_routes);

            // Apply state changes
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
                CommandEffect::ShowProviders | CommandEffect::ShowModel => {}
            }

            Ok(HandleResult::Command {
                effect,
                conversation_key,
            })
        }

        MessageClassification::RegularMessage => {
            handle_regular_message(envelope, &content, &conversation_key, caps, config, ports).await
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
    // ── 3. Resolve route (with query classification override) ─────
    let mut route = ports.routes.get_route(conversation_key);

    // #4: Query classification override
    if config.query_classifier.is_some() {
        if let Some(hint) = config.query_classifier.as_ref().and_then(|f| f(content)) {
            if let Some((provider, model, _)) = config
                .model_routes
                .iter()
                .find(|(_, _, h)| h.eq_ignore_ascii_case(&hint))
            {
                tracing::info!(
                    target: "query_classification",
                    %hint, %provider, %model,
                    "Channel message classified — overriding route"
                );
                route.provider = provider.clone();
                route.model = model.clone();
            }
        }
    }

    // ── #5: Auto-save memory on inbound ──────────────────────────
    if let Some(ref mem) = ports.memory {
        if inbound_message_service::should_autosave(config.auto_save_memory, content)
            && !mem.should_skip_autosave(content)
        {
            let autosave_key = format!("channel:{conversation_key}:user");
            let _ = mem
                .store(
                    &autosave_key,
                    content,
                    &crate::domain::memory::MemoryCategory::Conversation,
                    Some(conversation_key),
                )
                .await;
        }
    }

    // ── 4. Check prior history ───────────────────────────────────
    let has_prior = ports.history.has_history(conversation_key);

    // ── 5. Build initial history ─────────────────────────────────
    let mut history = vec![ChatMessage::system(config.system_prompt.clone())];

    if let Some(hints) = ports
        .channel_registry
        .delivery_hints(&envelope.source_adapter)
    {
        history.push(ChatMessage::system(hints));
    }

    // #6: Add prior turns (with #23 vision normalization)
    let prior_turns = ports.history.get_history(conversation_key);
    let supports_vision = ports.agent_runtime.supports_vision();
    for turn in prior_turns {
        if !supports_vision && turn.content.contains("[IMAGE:") {
            // Strip image markers from history for non-vision providers
            // Simple approach: remove [IMAGE:...] blocks
            let cleaned = strip_image_markers(&turn.content);
            history.push(ChatMessage {
                content: cleaned,
                ..turn
            });
        } else {
            history.push(turn);
        }
    }

    // ── #7/#8: Enrich context for first turn ─────────────────────
    let enrichment = inbound_message_service::decide_history_enrichment(has_prior, envelope);
    match enrichment {
        HistoryEnrichment::ThreadSeeding {
            parent_key,
            thread_id,
        } => {
            if let Some(ref summary_port) = ports.session_summary {
                if let Some(summary) = summary_port.load_summary(&parent_key) {
                    history.push(ChatMessage::system(format!(
                        "[Thread context — parent conversation summary]\n{summary}"
                    )));
                }
            }
            if let Ok(Some(root_text)) = ports.channel_output.fetch_message_text(&thread_id).await {
                let truncated = truncate_chars(&root_text, config.thread_root_max_chars);
                history.push(ChatMessage::system(format!(
                    "[Thread root message]\n{truncated}"
                )));
            }

            // Recent parent turns for immediate context
            let parent_turns = ports.history.get_history(&parent_key);
            if !parent_turns.is_empty() {
                let recent = inbound_message_service::smart_truncate_parent_turns(
                    &parent_turns,
                    config.thread_parent_recent_turns,
                    config.thread_parent_max_chars,
                );
                if !recent.is_empty() {
                    history.push(ChatMessage::system(format!(
                        "[Recent parent conversation]\n{recent}"
                    )));
                }
            }
        }
        HistoryEnrichment::MemoryContext {
            conversation_key: conv_key,
        } => {
            // #8: Memory context enrichment (core blocks + recall)
            if let Some(ref mem) = ports.memory {
                let mut full_context = String::new();

                // Core memory blocks (MemGPT: always in context)
                if let Ok(blocks) = mem.get_core_blocks(&config.agent_id).await {
                    for block in &blocks {
                        if !block.content.trim().is_empty() {
                            full_context.push_str(&format!(
                                "<{}>\n{}\n</{}>\n",
                                block.label,
                                block.content.trim(),
                                block.label
                            ));
                        }
                    }
                }

                // Keyword/vector recall
                let recall_cfg = crate::domain::memory::RecallConfig {
                    min_relevance_score: config.min_relevance_score,
                    ..Default::default()
                };
                let recall = crate::application::services::memory_service::recall_context(
                    mem.as_ref(),
                    content,
                    Some(&conv_key),
                    &recall_cfg,
                )
                .await;
                if !recall.is_empty() {
                    full_context.push_str(&recall);
                }

                if !full_context.is_empty() {
                    history.push(ChatMessage::user(format!("{full_context}\n{content}")));
                    // Skip the normal user turn append below
                    ports
                        .history
                        .append_turn(conversation_key, ChatMessage::user(content));
                    return execute_agent_turn(
                        envelope,
                        content,
                        conversation_key,
                        caps,
                        config,
                        ports,
                        &route,
                        history,
                    )
                    .await;
                }
            }
        }
        HistoryEnrichment::None => {}
    }

    // ── 7. Append user turn ──────────────────────────────────────
    ports
        .history
        .append_turn(conversation_key, ChatMessage::user(content));
    history.push(ChatMessage::user(content));

    execute_agent_turn(
        envelope,
        content,
        conversation_key,
        caps,
        config,
        ports,
        &route,
        history,
    )
    .await
}

/// Execute the agent turn and handle the response.
async fn execute_agent_turn(
    envelope: &InboundEnvelope,
    content: &str,
    conversation_key: &str,
    caps: &[ChannelCapability],
    config: &InboundMessageConfig,
    ports: &InboundMessagePorts,
    route: &crate::ports::route_selection::RouteSelection,
    history: Vec<ChatMessage>,
) -> Result<HandleResult> {
    // ── #11: Ack reaction + typing ───────────────────────────────
    if config.ack_reactions {
        let _ = ports
            .channel_output
            .add_reaction(&envelope.reply_ref, &envelope.conversation_ref, "👀")
            .await;
    }
    let _ = ports.channel_output.start_typing(&envelope.reply_ref).await;

    // ── #10: Streaming decision ──────────────────────────────────
    let use_streaming = ports.channel_output.supports_streaming();
    let (delta_tx, delta_rx) = if use_streaming {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Spawn draft updater task if streaming
    let draft_handle = if let Some(mut rx) = delta_rx {
        let output = ports.channel_output.clone();
        let recipient = envelope.reply_ref.clone();
        let thread = envelope.thread_ref.clone();
        Some(tokio::spawn(async move {
            let mut accumulated = String::new();
            let mut draft_id: Option<String> = None;

            while let Some(delta) = rx.recv().await {
                accumulated.push_str(&delta);
                match &draft_id {
                    None => {
                        if let Ok(Some(id)) = output
                            .send_draft(&recipient, &accumulated, thread.as_deref())
                            .await
                        {
                            draft_id = Some(id);
                        }
                    }
                    Some(id) => {
                        let _ = output.update_draft(&recipient, id, &accumulated).await;
                    }
                }
            }
            (accumulated, draft_id)
        }))
    } else {
        None
    };

    // ── #12: Execute agent turn (with #22 timeout) ───────────────
    let result = ports
        .agent_runtime
        .execute_turn(
            history,
            &route.provider,
            &route.model,
            config.temperature,
            config.max_tool_iterations,
            config.message_timeout_secs,
            delta_tx,
        )
        .await;

    // ── Stop typing ──────────────────────────────────────────────
    let _ = ports.channel_output.stop_typing(&envelope.reply_ref).await;

    // Collect draft state
    let draft_state = if let Some(handle) = draft_handle {
        handle.await.ok()
    } else {
        None
    };

    match result {
        Ok(turn_result) => {
            // ── #13: Hook: on_message_sending (with 20k limit) ───
            let mut response_text = match ports
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
                    ports.history.rollback_last_turn(conversation_key, content);
                    // Cancel draft if active
                    if let Some((_, Some(ref id))) = draft_state {
                        let _ = ports
                            .channel_output
                            .cancel_draft(&envelope.reply_ref, id)
                            .await;
                    }
                    return Ok(HandleResult::Cancelled { reason });
                }
            };

            // Enforce hook max outbound chars
            if response_text.chars().count() > HOOK_MAX_OUTBOUND_CHARS {
                response_text = truncate_chars(&response_text, HOOK_MAX_OUTBOUND_CHARS);
            }

            // ── #14: Finalize draft or send ──────────────────────
            if let Some((_, Some(ref draft_id))) = draft_state {
                let _ = ports
                    .channel_output
                    .finalize_draft(&envelope.reply_ref, draft_id, &response_text)
                    .await;
            } else {
                let _ = ports
                    .channel_output
                    .send_message(
                        &envelope.reply_ref,
                        &response_text,
                        envelope.thread_ref.as_deref(),
                    )
                    .await;
            }

            // ── #15: Tool context summary for history ────────────
            let tool_summary = &turn_result.tool_summary;
            let history_response =
                if inbound_message_service::should_include_tool_summary(tool_summary, caps) {
                    format!("{tool_summary}\n{response_text}")
                } else {
                    response_text.clone()
                };

            // ── #16: Persist assistant turn ──────────────────────
            ports
                .history
                .append_turn(conversation_key, ChatMessage::assistant(&history_response));

            // ── #18: Memory consolidation (fire-and-forget) ──────
            if let Some(ref mem) = ports.memory {
                if inbound_message_service::should_consolidate_memory(
                    config.auto_save_memory,
                    content,
                ) {
                    let mem = Arc::clone(mem);
                    let user_msg = content.to_string();
                    let asst_resp = response_text.clone();
                    tokio::spawn(async move {
                        let _ = mem.consolidate_turn(&user_msg, &asst_resp).await;
                    });
                }
            }

            // ── #19: Skill reflection (fire-and-forget) ──────
            // Multi-criteria gate: only reflect on high-value turns to avoid wasting LLM calls.
            if let Some(ref mem) = ports.memory {
                let resp_lower = response_text.to_lowercase();
                let should_reflect = response_text.len() > 200
                    && content.chars().count() >= 30
                    && (turn_result.tools_used
                        || resp_lower.contains("error")
                        || resp_lower.contains("failed"));

                if should_reflect {
                    let mem = Arc::clone(mem);
                    let user_msg = content.to_string();
                    let asst_resp = response_text.clone();
                    // Parse tool names from summary: "[Used tools: foo, bar]" → vec!["foo", "bar"]
                    let tools: Vec<String> = turn_result
                        .tool_summary
                        .trim_start_matches("[Used tools: ")
                        .trim_end_matches(']')
                        .split(", ")
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect();
                    tokio::spawn(async move {
                        let _ = mem.reflect_on_turn(&user_msg, &asst_resp, &tools).await;
                    });
                }
            }

            // ── #11: Swap ack reaction to done ───────────────────
            if config.ack_reactions {
                let _ = ports
                    .channel_output
                    .remove_reaction(&envelope.reply_ref, &envelope.conversation_ref, "👀")
                    .await;
                let _ = ports
                    .channel_output
                    .add_reaction(&envelope.reply_ref, &envelope.conversation_ref, "✅")
                    .await;
            }

            Ok(HandleResult::Response {
                conversation_key: conversation_key.to_string(),
                response_text,
                tool_summary: turn_result.tool_summary,
                tools_used: turn_result.tools_used,
            })
        }
        Err(e) => {
            // Cancel draft if active
            if let Some((_, Some(ref draft_id))) = draft_state {
                let _ = ports
                    .channel_output
                    .cancel_draft(&envelope.reply_ref, draft_id)
                    .await;
            }

            let err_str = e.to_string();

            // ── #20: Context overflow recovery ───────────────────
            if err_str.contains("context_length_exceeded")
                || err_str.contains("context window")
                || err_str.contains("maximum context length")
            {
                let compacted = ports.history.compact_history(conversation_key, 6);
                if compacted {
                    // Reinject summary if available
                    if let Some(ref summary_port) = ports.session_summary {
                        if let Some(summary) = summary_port.load_summary(conversation_key) {
                            ports.history.prepend_turn(
                                conversation_key,
                                ChatMessage::system(format!(
                                    "[Previous conversation summary]\n{summary}"
                                )),
                            );
                        }
                    }
                }
                let msg = if compacted {
                    "⚠️ Context window exceeded. I compacted history and preserved a summary. Please try again."
                } else {
                    "⚠️ Context window exceeded. Try `/new` to start a fresh conversation."
                };
                let _ = ports
                    .channel_output
                    .send_message(&envelope.reply_ref, msg, envelope.thread_ref.as_deref())
                    .await;
                // Don't rollback — keep the user turn for retry
                return Ok(HandleResult::Response {
                    conversation_key: conversation_key.to_string(),
                    response_text: msg.to_string(),
                    tool_summary: String::new(),
                    tools_used: false,
                });
            }

            // ── #22: Timeout detection ───────────────────────────
            if err_str.contains("deadline has elapsed") || err_str.contains("timed out") {
                ports.history.rollback_last_turn(conversation_key, content);
                let msg = "⏱️ Request timed out. Try a simpler question or `/new`.";
                let _ = ports
                    .channel_output
                    .send_message(&envelope.reply_ref, msg, envelope.thread_ref.as_deref())
                    .await;
                return Ok(HandleResult::Response {
                    conversation_key: conversation_key.to_string(),
                    response_text: msg.to_string(),
                    tool_summary: String::new(),
                    tools_used: false,
                });
            }

            // ── #21: Generic error ───────────────────────────────
            ports.history.rollback_last_turn(conversation_key, content);
            let safe_err = err_str.replace("Bearer ", "Bearer [REDACTED]");
            let msg = format!("⚠️ {safe_err}");
            let _ = ports
                .channel_output
                .send_message(&envelope.reply_ref, &msg, envelope.thread_ref.as_deref())
                .await;

            // Return as Response so caller doesn't double-send error
            Ok(HandleResult::Response {
                conversation_key: conversation_key.to_string(),
                response_text: msg,
                tool_summary: String::new(),
                tools_used: false,
            })
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

/// Strip [IMAGE:...] markers from text for non-vision providers.
fn strip_image_markers(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            // Check if this is [IMAGE: prefix
            let mut lookahead = String::from(ch);
            let mut is_image = false;
            for _ in 0..6 {
                if let Some(&next) = chars.peek() {
                    lookahead.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if lookahead.starts_with("[IMAGE:") {
                // Skip until closing ]
                is_image = true;
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            if !is_image {
                result.push_str(&lookahead);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::channel::SourceKind;
    use crate::ports::hooks::NoOpHooks;
    use crate::ports::route_selection::RouteSelection;
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
            _h: Vec<ChatMessage>,
            _p: &str,
            _m: &str,
            _t: f64,
            _mi: usize,
            _to: u64,
            _delta: Option<tokio::sync::mpsc::Sender<String>>,
        ) -> Result<crate::ports::agent_runtime::AgentTurnResult> {
            Ok(crate::ports::agent_runtime::AgentTurnResult {
                response: self.response.clone(),
                history: vec![],
                tools_used: false,
                tool_summary: String::new(),
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
        fn has_channel(&self, _n: &str) -> bool {
            false
        }
        fn capabilities(&self, _n: &str) -> Vec<ChannelCapability> {
            vec![
                ChannelCapability::SendText,
                ChannelCapability::RuntimeCommands,
            ]
        }
        async fn deliver(&self, _i: &crate::domain::channel::OutboundIntent) -> Result<()> {
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
            thread_parent_recent_turns: 3,
            thread_parent_max_chars: 2000,
            query_classifier: None,
            message_timeout_secs: 60,
            min_relevance_score: 0.5,
            ack_reactions: false,
            agent_id: "test-agent".into(),
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
            memory: None,
        }
    }

    #[tokio::test]
    async fn handle_regular_message_returns_response() {
        let env = test_envelope("Hello");
        let caps = vec![ChannelCapability::SendText];
        let result = handle(&env, &caps, &test_config(), &test_ports("Hi!"))
            .await
            .unwrap();
        match result {
            HandleResult::Response { response_text, .. } => assert_eq!(response_text, "Hi!"),
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_regular_message_persists_turns() {
        let env = test_envelope("Hello");
        let caps = vec![ChannelCapability::SendText];
        let ports = test_ports("Hi!");
        handle(&env, &caps, &test_config(), &ports).await.unwrap();
        assert_eq!(ports.history.get_history("telegram_user1").len(), 2);
    }

    #[tokio::test]
    async fn handle_command_clears_session() {
        let env = test_envelope("/new");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let ports = test_ports("");
        ports
            .history
            .append_turn("telegram_user1", ChatMessage::user("old"));
        let result = handle(&env, &caps, &test_config(), &ports).await.unwrap();
        assert!(matches!(
            result,
            HandleResult::Command {
                effect: CommandEffect::ClearSession,
                ..
            }
        ));
        assert!(!ports.history.has_history("telegram_user1"));
    }

    #[test]
    fn strip_image_markers_removes_blocks() {
        let text = "Hello [IMAGE:abc123] world";
        assert_eq!(strip_image_markers(text), "Hello  world");
    }

    #[test]
    fn strip_image_markers_preserves_normal_brackets() {
        let text = "Hello [world] test";
        assert_eq!(strip_image_markers(text), "Hello [world] test");
    }

    #[test]
    fn truncate_chars_within_limit() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_over_limit() {
        let result = truncate_chars("hello world", 5);
        assert_eq!(result, "hello…");
    }
}
