//! Use case: HandleInboundMessage — full orchestration of an inbound message.
//!
//! Phase 4.0 Slice 2: replaces the monolithic `process_channel_message` in
//! channels/mod.rs with a port-driven orchestrator in synapse_domain.
//!
//! All 24 behaviors from the original function are accounted for here.

use crate::application::services::channel_presentation::{
    self, ChannelPresentationMode, CompactProgressSurface,
};
use crate::application::services::dialogue_state_service::{self, DialogueStateStore};
use crate::application::services::history_compaction::{
    compact_provider_history_for_session_hygiene, SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
};
use crate::application::services::inbound_message_service::{
    self, CommandEffect, HistoryEnrichment, MessageClassification,
};
use crate::application::services::provider_context_budget::provider_context_input_for_history;
use crate::application::services::runtime_assumptions::{
    apply_tool_repair_assumption_challenges, build_runtime_assumptions,
    challenge_runtime_assumption_ledger, merge_runtime_assumption_ledger,
    RuntimeAssumptionChallenge, RuntimeAssumptionInput, RuntimeAssumptionInvalidation,
    RuntimeAssumptionKind, RuntimeAssumptionReplacementPath,
};
use crate::application::services::runtime_watchdog::{
    build_runtime_watchdog_digest, format_runtime_watchdog_context, RuntimeWatchdogInput,
};
use crate::application::services::turn_interpretation::TurnInterpretation;
use crate::application::services::turn_markup::{
    contains_image_attachment_marker, strip_image_attachment_markers,
};
use crate::config::schema::ModelRouteConfig;
use crate::domain::channel::{ChannelCapability, InboundEnvelope};
use crate::domain::message::ChatMessage;
use crate::ports::agent_runtime::{AgentRuntimeErrorKind, AgentRuntimePort};
use crate::ports::channel_output::ChannelOutputPort;
use crate::ports::channel_registry::ChannelRegistryPort;
use crate::ports::conversation_history::ConversationHistoryPort;
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::hooks::{HookOutcome, HooksPort};
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::model_profile_catalog::ModelProfileCatalogPort;
use crate::ports::route_selection::{RouteAdmissionState, RouteSelectionPort};
use crate::ports::run_recipe_store::RunRecipeStorePort;
use crate::ports::scoped_instruction_context::{
    ScopedInstructionContextPort, ScopedInstructionRequest,
};
use crate::ports::session_summary::SessionSummaryPort;
use crate::ports::turn_defaults_context::TurnDefaultsContextPort;
use crate::ports::user_profile_store::UserProfileStorePort;
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
    pub model_routes: Vec<ModelRouteConfig>,
    pub model_lanes: Vec<crate::config::schema::ModelLaneConfig>,
    pub model_preset: Option<String>,
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
    /// Prompt budget for turn context assembly.
    pub prompt_budget: crate::application::services::turn_context::PromptBudget,
    /// What to load on continuation turns (turn N>1).
    pub continuation_policy: crate::application::services::turn_context::ContinuationPolicy,
    /// Human-facing rendering policy for messaging channels.
    pub presentation_mode: ChannelPresentationMode,
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
    /// SSE event sender for publishing learning reports to web dashboard.
    pub event_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
    /// Current conversation context for tools that need "here".
    pub conversation_context:
        Option<Arc<dyn crate::ports::conversation_context::ConversationContextPort>>,
    pub model_profile_catalog: Option<Arc<dyn ModelProfileCatalogPort>>,
    /// Resolved typed defaults for the current turn.
    pub turn_defaults_context: Option<Arc<dyn TurnDefaultsContextPort>>,
    /// Progressive scoped project instructions loaded on demand.
    pub scoped_instruction_context: Option<Arc<dyn ScopedInstructionContextPort>>,
    pub conversation_store: Option<Arc<dyn ConversationStorePort>>,
    /// Dialogue state store for session-scoped working memory.
    pub dialogue_state_store: Option<Arc<DialogueStateStore>>,
    pub run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    pub user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
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
                    // Clear semantics are adapter lifecycle-specific. Channel
                    // command hosts clear the concrete in-memory route/history
                    // backend after command dispatch.
                }
                CommandEffect::SwitchProvider { .. } => {
                    // Provider validation/canonicalization is adapter-owned because it
                    // depends on concrete provider initialization. The adapter runtime
                    // command host applies the route mutation only after validation.
                }
                CommandEffect::SwitchModel { .. } => {
                    // Model route mutation and target-window preflight need the
                    // concrete runtime route backend, provider catalog, and
                    // compaction lifecycle, so adapters own the side effect.
                }
                CommandEffect::SwitchModelBlocked { .. }
                | CommandEffect::ShowProviders
                | CommandEffect::ShowModel => {}
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
    let current_conversation = crate::domain::conversation_target::CurrentConversationContext {
        source_adapter: envelope.source_adapter.clone(),
        conversation_ref: envelope.conversation_ref.clone(),
        reply_ref: envelope.reply_ref.clone(),
        thread_ref: envelope.thread_ref.clone(),
        actor_id: envelope.actor_id.clone(),
    };
    let dialogue_state = ports
        .dialogue_state_store
        .as_ref()
        .and_then(|store| store.get(conversation_key));
    let user_profile = ports
        .user_profile_store
        .as_ref()
        .and_then(|store| store.load(&channel_user_profile_key(envelope)));

    // ── 3. Resolve route (with query classification override) ─────
    let mut route = ports.routes.get_route(conversation_key);

    // #4: Query classification override
    if config.query_classifier.is_some() {
        if let Some(hint) = config.query_classifier.as_ref().and_then(|f| f(content)) {
            if let Some(route_match) = config
                .model_routes
                .iter()
                .find(|route| route.hint.eq_ignore_ascii_case(&hint))
            {
                tracing::info!(
                    target: "query_classification",
                    %hint,
                    provider = %route_match.provider,
                    model = %route_match.model,
                    "Channel message classified — overriding route"
                );
                route.provider = route_match.provider.clone();
                route.model = route_match.model.clone();
                route.lane = route_match.capability;
                route.candidate_index = None;
                route.clear_runtime_diagnostics();
            }
        }
    }

    let routing_config = build_model_routing_config(config);
    let current_route_profile =
        crate::application::services::model_lane_resolution::resolve_route_selection_profile(
            &routing_config,
            &route,
            ports.model_profile_catalog.as_deref(),
        );
    let current_supports_vision = ports
        .agent_runtime
        .supports_vision_for_route(&route.provider, &route.model);
    let capability_route_override =
        crate::application::services::turn_model_routing::resolve_turn_route_override(
            &routing_config,
            content,
            &route.provider,
            &route.model,
            &current_route_profile,
            current_supports_vision,
            ports.model_profile_catalog.as_deref(),
        );
    if let Some(route_override) = capability_route_override.as_ref() {
        tracing::info!(
            target: "capability_routing",
            lane = ?route_override.lane,
            provider = %route_override.provider,
            model = %route_override.model,
            "Channel turn capability route override"
        );
        route.provider = route_override.provider.clone();
        route.model = route_override.model.clone();
        route.lane = Some(route_override.lane);
        route.candidate_index = route_override.candidate_index;
        route.clear_runtime_diagnostics();
    }

    // ── #5: Auto-save memory on inbound ──────────────────────────
    if let Some(ref mem) = ports.memory {
        if inbound_message_service::should_autosave(config.auto_save_memory, content) {
            let _ = mem
                .store(
                    &inbound_message_service::autosave_memory_key(envelope),
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
    let interpretation =
        crate::application::services::turn_interpretation::build_turn_interpretation(
            ports.memory.as_ref().map(|memory| memory.as_ref()),
            content,
            user_profile,
            Some(&current_conversation),
            dialogue_state.as_ref(),
            ports.channel_registry.configured_delivery_target(),
        )
        .await;
    if let Some(interpretation) = interpretation.as_ref() {
        if let Some(block) =
            crate::application::services::turn_interpretation::format_turn_interpretation_for_turn(
                content,
                interpretation,
            )
        {
            history.push(ChatMessage::system(block));
        }
    }
    let resolved_turn_defaults =
        crate::application::services::turn_defaults_resolution::resolve_turn_defaults(
            interpretation.as_ref(),
            ports.channel_registry.configured_delivery_target(),
        );
    if let (Some(loader), Some(plan)) = (
        ports.scoped_instruction_context.as_ref(),
        crate::application::services::scoped_instruction_resolution::build_scoped_instruction_plan(
            content,
            interpretation.as_ref(),
        ),
    ) {
        let snippets = loader
            .load_scoped_instructions(ScopedInstructionRequest {
                session_id: Some(conversation_key.to_string()),
                path_hints: plan.hints.into_iter().map(|hint| hint.path).collect(),
                max_files: plan.max_files,
                max_total_chars: plan.max_total_chars,
            })
            .await
            .unwrap_or_default();
        if let Some(block) =
            crate::application::services::scoped_instruction_resolution::format_scoped_instruction_block(&snippets)
        {
            history.push(ChatMessage::system(block));
        }
    }

    // #6: Add prior turns (with #23 vision normalization)
    let prior_turns = ports.history.get_history(conversation_key);
    let supports_vision = capability_route_override
        .as_ref()
        .is_some_and(|override_route| {
            override_route.lane == crate::config::schema::CapabilityLane::MultimodalUnderstanding
        })
        || ports
            .agent_runtime
            .supports_vision_for_route(&route.provider, &route.model);
    for turn in prior_turns {
        if !supports_vision && contains_image_attachment_marker(&turn.content) {
            // Strip image markers from history for non-vision providers
            let cleaned = strip_image_attachment_markers(&turn.content);
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
            // Inject core blocks before thread context
            if let Some(ref mem) = ports.memory {
                if let Ok(blocks) = mem.get_core_blocks(&config.agent_id).await {
                    let mut core_ctx = String::new();
                    for block in &blocks {
                        if !block.content.trim().is_empty() {
                            core_ctx.push_str(&format!(
                                "<{}>\n{}\n</{}>\n",
                                block.label,
                                block.content.trim(),
                                block.label
                            ));
                        }
                    }
                    if !core_ctx.is_empty() {
                        history.push(ChatMessage::system(core_ctx));
                    }
                }
            }
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
            // #8: Memory context enrichment via unified assembler
            if let Some(ref mem) = ports.memory {
                use crate::application::services::turn_context as tc;
                let recent_admission_reasons =
                    crate::application::services::route_admission_history::recent_route_admission_reasons(
                        &route.recent_admissions,
                    );
                let recent_admission_repair =
                    crate::application::services::route_admission_history::latest_route_admission_repair(
                        &route.recent_admissions,
                    );
                let turn_ctx = tc::assemble_turn_context(
                    mem.as_ref(),
                    ports.run_recipe_store.as_ref().map(|store| store.as_ref()),
                    ports
                        .conversation_store
                        .as_ref()
                        .map(|store| store.as_ref()),
                    content,
                    &config.agent_id,
                    Some(&conv_key),
                    interpretation.as_ref(),
                    &route.recent_tool_repairs,
                    &recent_admission_reasons,
                    recent_admission_repair,
                    &config.prompt_budget,
                    None, // first turn → full context
                )
                .await;
                let formatted = tc::format_turn_context(&turn_ctx, &config.prompt_budget);

                if !formatted.core_blocks_system.is_empty()
                    || !formatted.enrichment_prefix.is_empty()
                {
                    if !formatted.core_blocks_system.is_empty() {
                        history.push(ChatMessage::system(formatted.core_blocks_system));
                    }
                    if !formatted.resolution_system.is_empty() {
                        history.push(ChatMessage::system(formatted.resolution_system));
                    }
                    // Enrichment as ephemeral user-prefix (raw content stored in history)
                    if !formatted.enrichment_prefix.is_empty() {
                        history.push(ChatMessage::user(format!(
                            "{}{content}",
                            formatted.enrichment_prefix
                        )));
                    } else {
                        history.push(ChatMessage::user(content));
                    }
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
                        resolved_turn_defaults.clone(),
                        interpretation.clone(),
                        route.clone(),
                        history,
                    )
                    .await;
                }
            }
        }
        HistoryEnrichment::Continuation => {
            // Continuation turn — use ContinuationPolicy from config
            if let Some(ref mem) = ports.memory {
                use crate::application::services::turn_context as tc;
                let continuation = config.continuation_policy.clone();
                let recent_admission_reasons =
                    crate::application::services::route_admission_history::recent_route_admission_reasons(
                        &route.recent_admissions,
                    );
                let recent_admission_repair =
                    crate::application::services::route_admission_history::latest_route_admission_repair(
                        &route.recent_admissions,
                    );
                let turn_ctx = tc::assemble_turn_context(
                    mem.as_ref(),
                    ports.run_recipe_store.as_ref().map(|store| store.as_ref()),
                    ports
                        .conversation_store
                        .as_ref()
                        .map(|store| store.as_ref()),
                    content,
                    &config.agent_id,
                    Some(conversation_key),
                    interpretation.as_ref(),
                    &route.recent_tool_repairs,
                    &recent_admission_reasons,
                    recent_admission_repair,
                    &config.prompt_budget,
                    Some(&continuation),
                )
                .await;
                let formatted = tc::format_turn_context(&turn_ctx, &config.prompt_budget);

                if !formatted.core_blocks_system.is_empty() {
                    history.push(ChatMessage::system(formatted.core_blocks_system));
                }
                if !formatted.resolution_system.is_empty() {
                    history.push(ChatMessage::system(formatted.resolution_system));
                }
                if !formatted.enrichment_prefix.is_empty() {
                    history.push(ChatMessage::user(format!(
                        "{}{content}",
                        formatted.enrichment_prefix
                    )));
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
                        resolved_turn_defaults.clone(),
                        interpretation.clone(),
                        route.clone(),
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
        resolved_turn_defaults,
        interpretation,
        route,
        history,
    )
    .await
}

fn build_model_routing_config(config: &InboundMessageConfig) -> crate::config::schema::Config {
    let mut routing = crate::config::schema::Config::default();
    routing.default_provider = Some(config.default_provider.clone());
    routing.default_model = Some(config.default_model.clone());
    routing.model_preset = config.model_preset.clone();
    routing.model_lanes = config.model_lanes.clone();
    routing.model_routes = config.model_routes.iter().cloned().collect();
    routing
}

fn format_blocked_turn_admission_response(
    decision: &crate::application::services::turn_admission::CandidateAdmissionDecision,
    user_message: &str,
) -> String {
    use crate::domain::turn_admission::{
        AdmissionRepairHint, CandidateAdmissionReason, ContextPressureState, TurnIntentCategory,
    };

    let base = if let Some(AdmissionRepairHint::RefreshCapabilityMetadata(lane)) =
        decision.recommended_action
    {
        format!(
            "Capability metadata for `{}` is stale or low-confidence on the current route. Refresh model profiles or switch to a compatible lane and try again.",
            blocked_lane_name(lane)
        )
    } else {
        match (decision.snapshot.intent, decision.snapshot.pressure_state) {
            (_, ContextPressureState::OverflowRisk) => match decision.recommended_action {
                Some(AdmissionRepairHint::StartFreshHandoff) => {
                    "This turn is too large for the current route's safe context budget. Start a fresh handoff or switch to a larger-context model.".into()
                }
                _ => {
                    "This turn is too large for the current route's safe context budget. Compact the session first or switch to a larger-context model.".into()
                }
            },
            (TurnIntentCategory::MultimodalUnderstanding, _) => {
                "The current route cannot handle image-aware input. Switch to a multimodal route and try again.".into()
            }
            (TurnIntentCategory::ImageGeneration, _) => {
                "The current route cannot generate images. Switch to an image-generation lane and try again.".into()
            }
            (TurnIntentCategory::AudioGeneration, _) => {
                "The current route cannot generate audio. Switch to an audio-capable lane and try again.".into()
            }
            (TurnIntentCategory::VideoGeneration, _) => {
                "The current route cannot generate video. Switch to a video-capable lane and try again.".into()
            }
            (TurnIntentCategory::MusicGeneration, _) => {
                "The current route cannot generate music. Switch to a music-capable lane and try again.".into()
            }
            (_, _) if decision.reasons.iter().any(|reason| {
                matches!(
                    reason,
                    CandidateAdmissionReason::CapabilityMetadataUnknown(_)
                        | CandidateAdmissionReason::CapabilityMetadataLowConfidence(_)
                        | CandidateAdmissionReason::CapabilityMetadataStale(_)
                )
            }) => {
                "The current route has incomplete capability metadata for this turn. Refresh model profiles or switch to a compatible lane and try again.".into()
            }
            _ => "The current route cannot safely execute this turn. Switch to a compatible lane or start a fresh handoff.".into(),
        }
    };

    if let Some(packet) =
        crate::application::services::session_handoff::build_session_handoff_packet(
            crate::application::services::session_handoff::SessionHandoffInput {
                user_message,
                interpretation: None,
                recent_admission_repair: decision.recommended_action,
                recent_admission_reasons: &decision.reasons,
                recalled_entries: &[],
                session_matches: &[],
                run_recipes: &[],
            },
        )
    {
        format!(
            "{base}\n\n{}",
            crate::application::services::session_handoff::format_session_handoff_packet(&packet)
        )
    } else {
        base
    }
}

fn blocked_lane_name(lane: crate::config::schema::CapabilityLane) -> &'static str {
    lane.as_str()
}

/// Execute the agent turn and handle the response.
async fn execute_agent_turn(
    envelope: &InboundEnvelope,
    content: &str,
    conversation_key: &str,
    caps: &[ChannelCapability],
    config: &InboundMessageConfig,
    ports: &InboundMessagePorts,
    resolved_turn_defaults: crate::domain::turn_defaults::ResolvedTurnDefaults,
    interpretation: Option<TurnInterpretation>,
    mut route: crate::ports::route_selection::RouteSelection,
    mut history: Vec<ChatMessage>,
) -> Result<HandleResult> {
    let janitor_now_unix = chrono::Utc::now().timestamp();
    let cleaned_route_traces =
        crate::application::services::runtime_trace_janitor::run_runtime_trace_janitor(
            crate::application::services::runtime_trace_janitor::RuntimeTraceJanitorInput {
                tool_repairs: &route.recent_tool_repairs,
                assumptions: &route.assumptions,
                calibration_records: &route.calibrations,
                now_unix: janitor_now_unix,
                ..Default::default()
            },
        );
    route.recent_tool_repairs = cleaned_route_traces.tool_repairs;
    route.last_tool_repair = route.recent_tool_repairs.last().cloned();
    route.assumptions = cleaned_route_traces.assumptions;
    route.calibrations = cleaned_route_traces.calibration_records;

    struct ConversationContextGuard {
        port: Option<Arc<dyn crate::ports::conversation_context::ConversationContextPort>>,
    }

    impl Drop for ConversationContextGuard {
        fn drop(&mut self) {
            if let Some(port) = &self.port {
                port.set_current(None);
            }
        }
    }

    struct TurnDefaultsGuard {
        port: Option<Arc<dyn TurnDefaultsContextPort>>,
    }

    impl Drop for TurnDefaultsGuard {
        fn drop(&mut self) {
            if let Some(port) = &self.port {
                port.set_current(None);
            }
        }
    }

    let current_conversation = crate::domain::conversation_target::CurrentConversationContext {
        source_adapter: envelope.source_adapter.clone(),
        conversation_ref: envelope.conversation_ref.clone(),
        reply_ref: envelope.reply_ref.clone(),
        thread_ref: envelope.thread_ref.clone(),
        actor_id: envelope.actor_id.clone(),
    };

    let routing_config = build_model_routing_config(config);
    let route_profile =
        crate::application::services::model_lane_resolution::resolve_route_selection_profile(
            &routing_config,
            &route,
            ports.model_profile_catalog.as_deref(),
        );
    let route_capabilities = ports.agent_runtime.capabilities_for(&route.provider);
    let admission_decision = crate::application::services::turn_admission::assess_turn_admission(
        crate::application::services::turn_admission::TurnAdmissionInput {
            config: Some(&routing_config),
            user_message: content,
            execution_guidance: None,
            tool_specs: &[],
            current_provider: &route.provider,
            current_model: &route.model,
            current_lane: route.lane,
            current_profile: &route_profile,
            provider_capabilities: &route_capabilities,
            provider_context: provider_context_input_for_history(&history)
                .with_target_model_profile(&route_profile),
            calibration_records: &route.calibrations,
            catalog: ports.model_profile_catalog.as_deref(),
        },
    );
    let observed_assumptions = build_runtime_assumptions(RuntimeAssumptionInput {
        user_message: content,
        interpretation: interpretation.as_ref(),
        recent_admission_repair: admission_decision.recommended_action,
        recent_admission_reasons: &admission_decision.reasons,
    });
    route.assumptions = merge_runtime_assumption_ledger(&route.assumptions, &observed_assumptions);

    tracing::info!(
        target: "turn_admission",
        provider = %route.provider,
        model = %route.model,
        lane = ?route.lane,
        intent = %crate::domain::turn_admission::turn_intent_name(admission_decision.snapshot.intent),
        pressure = %crate::domain::turn_admission::context_pressure_state_name(
            admission_decision.snapshot.pressure_state
        ),
        action = %crate::domain::turn_admission::turn_admission_action_name(
            admission_decision.snapshot.action
        ),
        reasons = ?admission_decision.reasons,
        "Channel turn admission decision"
    );

    if let Some(route_override) = admission_decision.route_override.as_ref() {
        route.provider = route_override.provider.clone();
        route.model = route_override.model.clone();
        route.lane = Some(route_override.lane);
        route.candidate_index = route_override.candidate_index;
    }
    let observed_at_unix = chrono::Utc::now().timestamp();
    let admission_state = RouteAdmissionState {
        observed_at_unix,
        snapshot: admission_decision.snapshot.clone(),
        reasons: admission_decision.reasons.clone(),
        recommended_action: admission_decision.recommended_action,
    };
    route.recent_admissions =
        crate::application::services::route_admission_history::append_route_admission_state(
            &route.recent_admissions,
            Some(admission_state.clone()),
            observed_at_unix,
        );
    route.last_admission = Some(admission_state);
    let runtime_watchdog_digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
        last_admission: route.last_admission.as_ref(),
        recent_admissions: &route.recent_admissions,
        last_tool_repair: route.last_tool_repair.as_ref(),
        recent_tool_repairs: &route.recent_tool_repairs,
        context_cache: route.context_cache.as_ref(),
        assumptions: &route.assumptions,
        subsystem_observations: &[],
        now_unix: observed_at_unix,
    });
    route.last_tool_repair = None;
    route.recent_tool_repairs.clear();
    ports.routes.set_route(conversation_key, route.clone());

    if admission_decision.snapshot.action
        == crate::domain::turn_admission::TurnAdmissionAction::Block
    {
        return Ok(HandleResult::Response {
            conversation_key: conversation_key.to_string(),
            response_text: format_blocked_turn_admission_response(&admission_decision, content),
            tool_summary: String::new(),
            tools_used: false,
        });
    }

    if admission_decision.requires_compaction {
        let stored_compacted = ports
            .history
            .compact_history(conversation_key, SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS);
        let provider_history_compacted = compact_provider_history_for_session_hygiene(
            &mut history,
            SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
        );
        tracing::info!(
            conversation_key,
            stored_compacted,
            provider_history_compacted,
            "Compacted channel session before agent execution"
        );
    }
    if let Some(block) = format_runtime_watchdog_context(&runtime_watchdog_digest) {
        history.push(ChatMessage::system(block));
    }

    // ── Set current conversation context for tools that need "here" ──
    let _conversation_context_guard = if let Some(ctx_port) = ports.conversation_context.clone() {
        ctx_port.set_current(Some(current_conversation.clone()));
        ConversationContextGuard {
            port: Some(ctx_port),
        }
    } else {
        ConversationContextGuard { port: None }
    };
    let _turn_defaults_guard = if let Some(defaults_port) = ports.turn_defaults_context.clone() {
        defaults_port.set_current(Some(resolved_turn_defaults));
        TurnDefaultsGuard {
            port: Some(defaults_port),
        }
    } else {
        TurnDefaultsGuard { port: None }
    };

    // ── #11: Ack reaction + typing ───────────────────────────────
    if config.ack_reactions {
        if let Some(message_id) = envelope.event_ref.as_deref() {
            let _ = ports
                .channel_output
                .add_reaction(&envelope.reply_ref, message_id, "👀")
                .await;
        }
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

    let compact_progress_handle = match channel_presentation::compact_progress_surface(
        config.presentation_mode,
        caps,
        use_streaming,
    ) {
        CompactProgressSurface::None => None,
        CompactProgressSurface::StatusMessage => {
            let output = ports.channel_output.clone();
            let recipient = envelope.reply_ref.clone();
            let thread = envelope.thread_ref.clone();
            Some(tokio::spawn(async move {
                tokio::time::sleep(channel_presentation::compact_progress_delay()).await;
                output
                    .send_message(
                        &recipient,
                        channel_presentation::compact_progress_text(),
                        thread.as_deref(),
                    )
                    .await
                    .is_ok()
            }))
        }
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

    if let Some(handle) = &compact_progress_handle {
        handle.abort();
    }

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

            route.last_tool_repair = turn_result.last_tool_repair.clone();
            route.recent_tool_repairs =
                crate::application::services::tool_repair::append_tool_repair_traces(
                    &route.recent_tool_repairs,
                    &turn_result.tool_repairs,
                    chrono::Utc::now().timestamp(),
                );
            route.assumptions = apply_tool_repair_assumption_challenges(
                &route.assumptions,
                &turn_result.tool_repairs,
            );
            route.calibrations =
                crate::application::services::runtime_calibration::append_tool_fact_calibration_observations(
                    &route.calibrations,
                    &turn_result.tool_facts,
                    chrono::Utc::now().timestamp(),
                )
                .records;
            ports.routes.set_route(conversation_key, route);

            if let Some(ref store) = ports.dialogue_state_store {
                let existing = store.get(conversation_key);
                if dialogue_state_service::should_materialize_state(
                    existing.as_ref(),
                    &turn_result.tool_facts,
                ) {
                    let mut state = existing.unwrap_or_default();
                    dialogue_state_service::update_state_from_turn(
                        &mut state,
                        content,
                        &turn_result.tool_facts,
                        &response_text,
                    );
                    store.set(conversation_key, state);
                }
            }

            // ── #18/#19: Post-turn learning (via orchestrator, fire-and-forget) ──
            if let Some(ref mem) = ports.memory {
                let mem = Arc::clone(mem);
                let input = crate::application::services::post_turn_orchestrator::PostTurnInput {
                    agent_id: config.agent_id.clone(),
                    user_message: content.to_string(),
                    assistant_response: response_text.clone(),
                    tools_used: turn_result.tool_names.clone(),
                    tool_facts: turn_result.tool_facts.clone(),
                    run_recipe_store: ports.run_recipe_store.clone(),
                    user_profile_store: ports.user_profile_store.clone(),
                    user_profile_key: Some(channel_user_profile_key(envelope)),
                    auto_save_enabled: config.auto_save_memory,
                    event_tx: ports.event_tx.clone(),
                };
                tokio::spawn(async move {
                    // Orchestrator handles tracing internally
                    crate::application::services::post_turn_orchestrator::execute_post_turn_learning(
                        mem.as_ref(), input,
                    ).await;
                });
            }

            // ── #11: Swap ack reaction to done ───────────────────
            if config.ack_reactions {
                if let Some(message_id) = envelope.event_ref.as_deref() {
                    let _ = ports
                        .channel_output
                        .remove_reaction(&envelope.reply_ref, message_id, "👀")
                        .await;
                    let _ = ports
                        .channel_output
                        .add_reaction(&envelope.reply_ref, message_id, "✅")
                        .await;
                }
            }

            Ok(HandleResult::Response {
                conversation_key: conversation_key.to_string(),
                response_text,
                tool_summary: turn_result.tool_summary,
                tools_used: turn_result.tools_used,
            })
        }
        Err(e) => {
            if let Some(challenge) = agent_runtime_error_assumption_challenge(&e.kind) {
                route.assumptions =
                    challenge_runtime_assumption_ledger(&route.assumptions, challenge);
                ports.routes.set_route(conversation_key, route.clone());
            }
            // Cancel draft if active
            if let Some((_, Some(ref draft_id))) = draft_state {
                let _ = ports
                    .channel_output
                    .cancel_draft(&envelope.reply_ref, draft_id)
                    .await;
            }

            // ── #20: Context overflow recovery ───────────────────
            if matches!(e.kind, AgentRuntimeErrorKind::ContextLimitExceeded) {
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
            if matches!(e.kind, AgentRuntimeErrorKind::Timeout) {
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
            let msg = format!("⚠️ {e}");
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

fn agent_runtime_error_assumption_challenge(
    kind: &AgentRuntimeErrorKind,
) -> Option<RuntimeAssumptionChallenge<'static>> {
    match kind {
        AgentRuntimeErrorKind::ContextLimitExceeded => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::ContextWindow,
            value: "context_limit_exceeded",
            invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
            replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
        }),
        AgentRuntimeErrorKind::CapabilityMismatch => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::RouteCapability,
            value: "capability_mismatch",
            invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
            replacement_path: RuntimeAssumptionReplacementPath::SwitchRoute,
        }),
        AgentRuntimeErrorKind::AuthFailure => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::RouteCapability,
            value: "auth_failure",
            invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        }),
        AgentRuntimeErrorKind::Timeout | AgentRuntimeErrorKind::RuntimeFailure => None,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

fn channel_user_profile_key(envelope: &InboundEnvelope) -> String {
    format!("channel:{}:{}", envelope.source_adapter, envelope.actor_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{ModelCandidateProfileConfig, ModelRouteConfig};
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
        fn compact_history(&self, key: &str, keep: usize) -> bool {
            let mut turns = self.turns.lock().unwrap();
            turns
                .get_mut(key)
                .is_some_and(|history| compact_provider_history_for_session_hygiene(history, keep))
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
                lane: None,
                candidate_index: None,
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
            }
        }
        fn set_route(&self, _key: &str, _route: RouteSelection) {}
        fn clear_route(&self, _key: &str) {}
    }

    struct RecordingRoutes {
        default_provider: String,
        default_model: String,
        route: Mutex<Option<RouteSelection>>,
        set_count: Mutex<usize>,
    }

    impl RecordingRoutes {
        fn new(default_provider: &str, default_model: &str) -> Self {
            Self {
                default_provider: default_provider.to_string(),
                default_model: default_model.to_string(),
                route: Mutex::new(None),
                set_count: Mutex::new(0),
            }
        }

        fn set_count(&self) -> usize {
            *self.set_count.lock().unwrap()
        }

        fn current_route(&self) -> Option<RouteSelection> {
            self.route.lock().unwrap().clone()
        }

        fn default_route(&self) -> RouteSelection {
            RouteSelection {
                provider: self.default_provider.clone(),
                model: self.default_model.clone(),
                lane: None,
                candidate_index: None,
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
            }
        }
    }

    impl RouteSelectionPort for RecordingRoutes {
        fn get_route(&self, _key: &str) -> RouteSelection {
            self.route
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| self.default_route())
        }

        fn set_route(&self, _key: &str, route: RouteSelection) {
            *self.route.lock().unwrap() = Some(route);
            *self.set_count.lock().unwrap() += 1;
        }

        fn clear_route(&self, _key: &str) {
            *self.route.lock().unwrap() = None;
        }
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
        ) -> std::result::Result<
            crate::ports::agent_runtime::AgentTurnResult,
            crate::ports::agent_runtime::AgentRuntimeError,
        > {
            Ok(crate::ports::agent_runtime::AgentTurnResult {
                response: self.response.clone(),
                history: vec![],
                tools_used: false,
                tool_names: vec![],
                tool_facts: vec![],
                tool_summary: String::new(),
                last_tool_repair: None,
                tool_repairs: vec![],
            })
        }
    }

    struct MockFailingRuntime {
        error: crate::ports::agent_runtime::AgentRuntimeError,
    }

    #[async_trait]
    impl AgentRuntimePort for MockFailingRuntime {
        async fn execute_turn(
            &self,
            _h: Vec<ChatMessage>,
            _p: &str,
            _m: &str,
            _t: f64,
            _mi: usize,
            _to: u64,
            _delta: Option<tokio::sync::mpsc::Sender<String>>,
        ) -> std::result::Result<
            crate::ports::agent_runtime::AgentTurnResult,
            crate::ports::agent_runtime::AgentRuntimeError,
        > {
            Err(self.error.clone())
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

    #[derive(Default)]
    struct RecordingChannelOutput {
        reactions: Mutex<Vec<(String, String, String, &'static str)>>,
    }

    #[async_trait]
    impl ChannelOutputPort for RecordingChannelOutput {
        async fn send_message(&self, _r: &str, _t: &str, _th: Option<&str>) -> Result<()> {
            Ok(())
        }
        async fn start_typing(&self, _r: &str) -> Result<()> {
            Ok(())
        }
        async fn stop_typing(&self, _r: &str) -> Result<()> {
            Ok(())
        }
        async fn add_reaction(&self, r: &str, m: &str, e: &str) -> Result<()> {
            self.reactions.lock().unwrap().push((
                r.to_string(),
                m.to_string(),
                e.to_string(),
                "add",
            ));
            Ok(())
        }
        async fn remove_reaction(&self, r: &str, m: &str, e: &str) -> Result<()> {
            self.reactions.lock().unwrap().push((
                r.to_string(),
                m.to_string(),
                e.to_string(),
                "remove",
            ));
            Ok(())
        }
        async fn fetch_message_text(&self, _m: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn supports_streaming(&self) -> bool {
            false
        }
    }

    #[test]
    fn provider_context_input_breaks_system_history_into_artifacts() {
        let history = vec![
            ChatMessage::system(
                concat!(
                    "bootstrap prelude\n",
                    "[core-memory]\nremember this\n",
                    "[runtime-interpretation]\nstate\n",
                    "[scoped-context]\nsubtree\n",
                    "[execution-guidance]\nreply from state\n",
                )
                .to_string(),
            ),
            ChatMessage::assistant("prior reply"),
            ChatMessage::user("current question"),
        ];

        let input = provider_context_input_for_history(&history);

        assert_eq!(input.bootstrap_chars, "bootstrap prelude\n".chars().count());
        assert!(input.core_memory_chars > 0);
        assert!(input.runtime_interpretation_chars > 0);
        assert!(input.scoped_context_chars > 0);
        assert!(input.resolution_chars > 0);
        assert_eq!(input.prior_chat_messages, 1);
        assert_eq!(input.current_turn_messages, 1);
    }

    #[test]
    fn session_hygiene_compaction_preserves_system_and_recent_turns() {
        let mut history = vec![
            ChatMessage::system("bootstrap"),
            ChatMessage::system("[runtime-interpretation]\nstate"),
        ];
        for idx in 0..10 {
            history.push(ChatMessage::user(format!("user {idx}")));
            history.push(ChatMessage::assistant(format!("assistant {idx}")));
        }

        assert!(compact_provider_history_for_session_hygiene(
            &mut history,
            4
        ));

        assert_eq!(history[0].role, "system");
        assert_eq!(history[1].role, "system");
        let non_system = history
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            non_system,
            vec!["user 8", "assistant 8", "user 9", "assistant 9"]
        );
    }

    #[test]
    fn session_hygiene_compaction_drops_leading_orphan_assistant() {
        let mut history = vec![ChatMessage::system("bootstrap")];
        for idx in 0..10 {
            history.push(ChatMessage::user(format!("user {idx}")));
            history.push(ChatMessage::assistant(format!("assistant {idx}")));
        }
        history.push(ChatMessage::user("current"));

        assert!(compact_provider_history_for_session_hygiene(
            &mut history,
            12
        ));

        let non_system = history
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| (message.role.as_str(), message.content.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(non_system.first(), Some(&("user", "user 5")));
        assert_eq!(non_system.last(), Some(&("user", "current")));
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
            model_lanes: vec![],
            model_preset: None,
            thread_root_max_chars: 500,
            thread_parent_recent_turns: 3,
            thread_parent_max_chars: 2000,
            query_classifier: None,
            message_timeout_secs: 60,
            min_relevance_score: 0.5,
            ack_reactions: false,
            agent_id: "test-agent".into(),
            prompt_budget: crate::application::services::turn_context::PromptBudget::default(),
            continuation_policy:
                crate::application::services::turn_context::ContinuationPolicy::default(),
            presentation_mode: ChannelPresentationMode::Compact,
        }
    }

    fn config_with_route_window(
        hint: &str,
        provider: &str,
        model: &str,
        context_window_tokens: usize,
    ) -> InboundMessageConfig {
        let mut config = test_config();
        config.model_routes = vec![ModelRouteConfig {
            hint: hint.to_string(),
            capability: None,
            provider: provider.to_string(),
            model: model.to_string(),
            api_key: None,
            profile: ModelCandidateProfileConfig {
                context_window_tokens: Some(context_window_tokens),
                ..Default::default()
            },
        }];
        config
    }

    fn test_envelope(content: &str) -> InboundEnvelope {
        InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user1".into(),
            conversation_ref: "telegram_user1".into(),
            event_ref: None,
            reply_ref: "chat123".into(),
            thread_ref: None,
            content: content.into(),
            received_at: 0,
        }
    }

    fn test_ports(response: &str) -> InboundMessagePorts {
        test_ports_with_state(
            Arc::new(MockHistory::new()),
            Arc::new(MockRoutes {
                default_provider: "openrouter".into(),
                default_model: "default-model".into(),
            }),
            response,
        )
    }

    fn test_ports_with_state(
        history: Arc<dyn ConversationHistoryPort>,
        routes: Arc<dyn RouteSelectionPort>,
        response: &str,
    ) -> InboundMessagePorts {
        InboundMessagePorts {
            history,
            routes,
            hooks: Arc::new(NoOpHooks),
            channel_output: Arc::new(MockChannelOutput),
            agent_runtime: Arc::new(MockRuntime {
                response: response.into(),
            }),
            channel_registry: Arc::new(MockRegistry),
            session_summary: None,
            memory: None,
            event_tx: None,
            conversation_context: None,
            model_profile_catalog: None,
            turn_defaults_context: None,
            scoped_instruction_context: None,
            conversation_store: None,
            dialogue_state_store: None,
            run_recipe_store: None,
            user_profile_store: None,
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
    async fn channel_provider_switch_waits_for_adapter_validation() {
        let env = test_envelope("/models grok");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let routes = Arc::new(RecordingRoutes::new("openrouter", "default-model"));
        let ports = test_ports_with_state(Arc::new(MockHistory::new()), routes.clone(), "");

        let result = handle(&env, &caps, &test_config(), &ports).await.unwrap();

        match result {
            HandleResult::Command {
                effect: CommandEffect::SwitchProvider { provider },
                ..
            } => assert_eq!(provider, "grok"),
            other => panic!("expected provider switch command, got {other:?}"),
        }
        assert_eq!(routes.set_count(), 0);
        assert!(routes.current_route().is_none());
    }

    #[tokio::test]
    async fn ack_reactions_use_event_ref_not_conversation_ref() {
        let output = Arc::new(RecordingChannelOutput::default());
        let ports = InboundMessagePorts {
            history: Arc::new(MockHistory::new()),
            routes: Arc::new(MockRoutes {
                default_provider: "openrouter".into(),
                default_model: "default-model".into(),
            }),
            hooks: Arc::new(NoOpHooks),
            channel_output: output.clone(),
            agent_runtime: Arc::new(MockRuntime {
                response: "Hi!".into(),
            }),
            channel_registry: Arc::new(MockRegistry),
            session_summary: None,
            memory: None,
            event_tx: None,
            conversation_context: None,
            model_profile_catalog: None,
            turn_defaults_context: None,
            scoped_instruction_context: None,
            conversation_store: None,
            dialogue_state_store: None,
            run_recipe_store: None,
            user_profile_store: None,
        };
        let mut config = test_config();
        config.ack_reactions = true;
        let caps = vec![ChannelCapability::SendText];
        let mut env = test_envelope("Hello");
        env.event_ref = Some("telegram_42_99".into());
        env.conversation_ref = "telegram_user1".into();

        handle(&env, &caps, &config, &ports).await.unwrap();

        let reactions = output.reactions.lock().unwrap().clone();
        assert_eq!(
            reactions,
            vec![
                (
                    "chat123".into(),
                    "telegram_42_99".into(),
                    "👀".into(),
                    "add"
                ),
                (
                    "chat123".into(),
                    "telegram_42_99".into(),
                    "👀".into(),
                    "remove"
                ),
                (
                    "chat123".into(),
                    "telegram_42_99".into(),
                    "✅".into(),
                    "add"
                ),
            ]
        );
    }

    #[tokio::test]
    async fn handle_command_defers_clear_session_to_adapter() {
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
        assert!(ports.history.has_history("telegram_user1"));
    }

    #[tokio::test]
    async fn channel_model_switch_waits_for_adapter_preflight() {
        let env = test_envelope("/model tiny");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let history = Arc::new(MockHistory::new());
        history.append_turn("telegram_user1", ChatMessage::user("x".repeat(20_000)));
        let routes = Arc::new(RecordingRoutes::new("openrouter", "large-model"));
        let ports = test_ports_with_state(history, routes.clone(), "");
        let config = config_with_route_window("tiny", "openrouter", "tiny-model", 1_000);

        let result = handle(&env, &caps, &config, &ports).await.unwrap();

        match result {
            HandleResult::Command {
                effect:
                    CommandEffect::SwitchModel {
                        model,
                        inferred_provider,
                        lane,
                        compacted,
                    },
                ..
            } => {
                assert_eq!(model, "tiny-model");
                assert_eq!(inferred_provider, Some("openrouter".to_string()));
                assert_eq!(lane, None);
                assert!(!compacted);
            }
            other => panic!("expected model switch command, got {other:?}"),
        }
        assert_eq!(routes.set_count(), 0);
        assert!(routes.current_route().is_none());
    }

    #[tokio::test]
    async fn channel_model_switch_defers_safe_downshift_mutation_to_adapter() {
        let env = test_envelope("/model compact");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let history = Arc::new(MockHistory::new());
        for idx in 0..20 {
            history.append_turn(
                "telegram_user1",
                ChatMessage::user(format!("{idx}: {}", "x".repeat(1_000))),
            );
        }
        let routes = Arc::new(RecordingRoutes::new("openrouter", "large-model"));
        let ports = test_ports_with_state(history.clone(), routes.clone(), "");
        let config = config_with_route_window("compact", "openrouter", "compact-model", 8_000);

        let result = handle(&env, &caps, &config, &ports).await.unwrap();

        match result {
            HandleResult::Command {
                effect:
                    CommandEffect::SwitchModel {
                        model,
                        inferred_provider,
                        compacted,
                        ..
                    },
                ..
            } => {
                assert_eq!(model, "compact-model");
                assert_eq!(inferred_provider, Some("openrouter".to_string()));
                assert!(!compacted);
            }
            other => panic!("expected switched model, got {other:?}"),
        }

        assert_eq!(routes.set_count(), 0);
        assert!(routes.current_route().is_none());
        assert_eq!(history.get_history("telegram_user1").len(), 20);
    }

    #[tokio::test]
    async fn context_limit_recovery_uses_typed_runtime_error() {
        let env = test_envelope("Hello");
        let caps = vec![ChannelCapability::SendText];
        let ports = InboundMessagePorts {
            history: Arc::new(MockHistory::new()),
            routes: Arc::new(MockRoutes {
                default_provider: "openrouter".into(),
                default_model: "default-model".into(),
            }),
            hooks: Arc::new(NoOpHooks),
            channel_output: Arc::new(MockChannelOutput),
            agent_runtime: Arc::new(MockFailingRuntime {
                error: crate::ports::agent_runtime::AgentRuntimeError::new(
                    crate::ports::agent_runtime::AgentRuntimeErrorKind::ContextLimitExceeded,
                    "too many tokens",
                ),
            }),
            channel_registry: Arc::new(MockRegistry),
            session_summary: None,
            memory: None,
            event_tx: None,
            conversation_context: None,
            model_profile_catalog: None,
            turn_defaults_context: None,
            scoped_instruction_context: None,
            conversation_store: None,
            dialogue_state_store: None,
            run_recipe_store: None,
            user_profile_store: None,
        };

        let result = handle(&env, &caps, &test_config(), &ports).await.unwrap();
        match result {
            HandleResult::Response { response_text, .. } => {
                assert!(response_text.contains("Context window exceeded"));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn strip_image_markers_removes_blocks() {
        let text = "Hello [IMAGE:abc123] world";
        assert_eq!(strip_image_attachment_markers(text), "Hello  world");
    }

    #[test]
    fn strip_image_markers_preserves_normal_brackets() {
        let text = "Hello [world] test";
        assert_eq!(strip_image_attachment_markers(text), "Hello [world] test");
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

    #[test]
    fn blocked_turn_response_mentions_stale_or_low_confidence_metadata() {
        let response = format_blocked_turn_admission_response(
            &crate::application::services::turn_admission::CandidateAdmissionDecision {
                snapshot: crate::domain::turn_admission::TurnAdmissionSnapshot {
                    intent: crate::domain::turn_admission::TurnIntentCategory::ImageGeneration,
                    pressure_state: crate::domain::turn_admission::ContextPressureState::Warning,
                    action: crate::domain::turn_admission::TurnAdmissionAction::Block,
                },
                required_lane: Some(crate::config::schema::CapabilityLane::ImageGeneration),
                route_override: None,
                reasons: vec![
                    crate::domain::turn_admission::CandidateAdmissionReason::CapabilityMetadataLowConfidence(
                        crate::config::schema::CapabilityLane::ImageGeneration,
                    ),
                ],
                recommended_action: Some(
                    crate::domain::turn_admission::AdmissionRepairHint::RefreshCapabilityMetadata(
                        crate::config::schema::CapabilityLane::ImageGeneration,
                    ),
                ),
                condensation_plan: None,
                requires_compaction: false,
            },
            "Generate an image for the current task.",
        );

        assert!(response.contains("image_generation"));
        assert!(response.contains("stale or low-confidence"));
        assert!(response.contains("Refresh model profiles"));
    }

    #[test]
    fn blocked_turn_response_mentions_safe_context_budget_on_overflow_risk() {
        let response = format_blocked_turn_admission_response(
            &crate::application::services::turn_admission::CandidateAdmissionDecision {
                snapshot: crate::domain::turn_admission::TurnAdmissionSnapshot {
                    intent: crate::domain::turn_admission::TurnIntentCategory::Reply,
                    pressure_state: crate::domain::turn_admission::ContextPressureState::OverflowRisk,
                    action: crate::domain::turn_admission::TurnAdmissionAction::Block,
                },
                required_lane: None,
                route_override: None,
                reasons: vec![
                    crate::domain::turn_admission::CandidateAdmissionReason::CandidateWindowExceeded,
                ],
                recommended_action: Some(
                    crate::domain::turn_admission::AdmissionRepairHint::StartFreshHandoff,
                ),
                condensation_plan: None,
                requires_compaction: false,
            },
            "Continue the current task after compaction.",
        );

        assert!(response.contains("safe context budget"));
        assert!(response.contains("fresh handoff"));
    }
}
