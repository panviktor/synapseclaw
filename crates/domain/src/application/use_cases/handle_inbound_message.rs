//! Use case: HandleInboundMessage — full orchestration of an inbound message.
//!
//! Phase 4.0 Slice 2: replaces the monolithic `process_channel_message` in
//! channels/mod.rs with a port-driven orchestrator in synapse_domain.
//!
//! All 24 behaviors from the original function are accounted for here.

use crate::application::services::assistant_output_presentation::{
    AssistantOutputPresenter, OutputDeliveryHints, PresentedOutput,
};
use crate::application::services::channel_presentation::{
    self, ChannelPresentationMode, CompactProgressSurface,
};
use crate::application::services::dialogue_state_service::{self, DialogueStateStore};
use crate::application::services::history_compaction::{
    build_compaction_transcript, compact_provider_history_for_session_hygiene,
    session_hygiene_dropped_messages_with_indices, HistoryCompressionPolicy,
    SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
};
use crate::application::services::implicit_memory_recall_service::{
    execute_implicit_memory_recall, ImplicitMemoryRecallInput,
};
use crate::application::services::inbound_message_service::{
    self, CommandEffect, HistoryEnrichment, MessageClassification,
};
use crate::application::services::memory_precompress_handoff::{
    execute_memory_precompress_handoff, is_precompress_preservation_message,
    precompress_preservation_message, MemoryPreCompressHandoffInput,
    MemoryPreCompressHandoffReason,
};
use crate::application::services::provider_context_budget::provider_context_input_for_history;
use crate::application::services::runtime_admission_presentation::format_blocked_turn_admission_response;
use crate::application::services::runtime_assumptions::{
    apply_tool_repair_assumption_challenges, build_runtime_assumptions,
    challenge_runtime_assumption_ledger, merge_runtime_assumption_ledger,
    RuntimeAssumptionChallenge, RuntimeAssumptionInput, RuntimeAssumptionInvalidation,
    RuntimeAssumptionKind, RuntimeAssumptionReplacementPath,
};
use crate::application::services::runtime_decision_trace::{
    build_runtime_decision_trace, build_runtime_decision_trace_id,
    merge_runtime_decision_trace_update, runtime_tool_decisions_from_repairs,
    RuntimeDecisionTraceInput, RuntimeDecisionTraceUpdate, RuntimeTraceRouteRef,
};
use crate::application::services::runtime_usage_insight_service::{
    estimate_usage_cost_microusd, record_runtime_usage, runtime_pricing_status_for_route,
    RuntimePricingStatus, RuntimeUsageRecordInput,
};
use crate::application::services::runtime_error_presentation::{
    format_context_limit_recovery_response, format_runtime_failure_response,
    format_timeout_recovery_response,
};
use crate::application::services::runtime_trace_janitor::{
    append_runtime_decision_trace_for_janitor, append_runtime_handoff_packet,
    append_runtime_watchdog_alerts, RUNTIME_TRACE_JANITOR_TTL_SECS,
};
use crate::application::services::runtime_watchdog::{
    build_runtime_subsystem_observations, build_runtime_watchdog_digest,
    format_runtime_watchdog_context, RuntimeSubsystemObservationInput, RuntimeWatchdogInput,
};
use crate::application::services::session_handoff::{
    build_session_handoff_packet, SessionHandoffInput,
};
use crate::application::services::turn_interpretation::TurnInterpretation;
use crate::application::services::turn_markup::{
    contains_image_attachment_marker, strip_image_attachment_markers,
};
use crate::domain::channel::{ChannelCapability, InboundEnvelope, SourceKind};
use crate::domain::conversation_target::ConversationDeliveryTarget;
use crate::domain::message::ChatMessage;
use crate::domain::tool_fact::{DeliveryTargetKind, OutcomeStatus, ToolFactPayload, TypedToolFact};
use crate::domain::turn_admission::CandidateAdmissionReason;
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
    /// Whether compact progress text may be emitted for long-running turns.
    pub emit_compact_progress: bool,
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
        output: PresentedOutput,
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
    let conversation_key =
        inbound_message_service::conversation_key_for_agent(envelope, &config.agent_id);
    let provider_facing_content = inbound_message_service::provider_facing_content(
        &envelope.content,
        &envelope.media_attachments,
    );

    // ── 1. Hook: on_message_received ─────────────────────────────
    let content = match ports
        .hooks
        .on_message_received(
            &envelope.source_adapter,
            &envelope.actor_id,
            provider_facing_content,
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
            let routing_config = build_model_routing_config(config);
            let effect = inbound_message_service::command_effect(&cmd, &routing_config);

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
                CommandEffect::CompactSession { .. } => {
                    // Session compaction uses the shared runtime-command host:
                    // adapters provide the concrete history target, while
                    // adapter-core owns the compaction algorithm.
                }
                CommandEffect::SwitchModelBlocked { .. }
                | CommandEffect::ShowProviders
                | CommandEffect::ShowModel
                | CommandEffect::ShowDoctor
                | CommandEffect::ShowSkills { .. }
                | CommandEffect::CreateUserSkill { .. }
                | CommandEffect::UpdateUserSkill { .. }
                | CommandEffect::ShowSkillTools
                | CommandEffect::ShowSkillTraces
                | CommandEffect::ShowSkillHealth { .. }
                | CommandEffect::ShowSkillDiff { .. }
                | CommandEffect::ApplySkillPatch { .. }
                | CommandEffect::ShowSkillVersions { .. }
                | CommandEffect::RollbackSkillPatch { .. }
                | CommandEffect::AutoPromoteSkills { .. }
                | CommandEffect::ReviewSkills { .. }
                | CommandEffect::UpdateSkillStatus { .. } => {}
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
        conversation_id: envelope.conversation_id.clone(),
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
        .and_then(|store| store.load(&channel_user_profile_key(&config.agent_id, envelope)));

    // ── 3. Resolve route (with query classification override) ─────
    let mut route = ports.routes.get_route(conversation_key);
    let routing_config = build_model_routing_config(config);

    // #4: Query classification override
    if config.query_classifier.is_some() {
        if let Some(hint) = config.query_classifier.as_ref().and_then(|f| f(content)) {
            if let Some(route_match) =
                inbound_message_service::resolve_model_command_route(&hint, &routing_config)
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
                route.lane = route_match.lane;
                route.candidate_index = route_match.candidate_index;
                route.clear_runtime_diagnostics();
            }
        }
    }

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
                    &inbound_message_service::autosave_memory_key_for_agent(
                        envelope,
                        &config.agent_id,
                    ),
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
    let implicit_memory_recall = execute_implicit_memory_recall(
        ports.memory.as_ref().map(|memory| memory.as_ref()),
        ImplicitMemoryRecallInput {
            agent_id: &config.agent_id,
            user_message: content,
            conversation_key: Some(conversation_key),
            interpretation: interpretation.as_ref(),
            min_relevance_score: config.min_relevance_score,
            now: chrono::Utc::now(),
        },
    )
    .await;
    if let Some(block) = implicit_memory_recall.guidance_block.as_ref() {
        history.push(ChatMessage::system(block.clone()));
    }
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
    let enrichment = inbound_message_service::decide_history_enrichment_for_agent(
        has_prior,
        envelope,
        &config.agent_id,
    );
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
                        implicit_memory_recall.clone(),
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
                        implicit_memory_recall.clone(),
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
        implicit_memory_recall,
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
    routing
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
    implicit_memory_recall: crate::application::services::implicit_memory_recall_service::ImplicitMemoryRecallOutcome,
    mut route: crate::ports::route_selection::RouteSelection,
    mut history: Vec<ChatMessage>,
) -> Result<HandleResult> {
    let janitor_now_unix = chrono::Utc::now().timestamp();
    route.clean_runtime_traces(janitor_now_unix);
    let delivery_hints = OutputDeliveryHints {
        reply_ref: Some(envelope.reply_ref.clone()),
        thread_ref: envelope.thread_ref.clone(),
        already_delivered: false,
    };

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
        conversation_id: envelope.conversation_id.clone(),
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
    let route_before_admission = route.clone();
    let provider_context =
        provider_context_input_for_history(&history).with_target_model_profile(&route_profile);
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
            provider_context,
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
    let runtime_trace_id = build_runtime_decision_trace_id(observed_at_unix, conversation_key);
    let admission_state = RouteAdmissionState {
        observed_at_unix,
        snapshot: admission_decision.snapshot.clone(),
        required_lane: admission_decision.required_lane,
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
    let selected_route_profile =
        crate::application::services::model_lane_resolution::resolve_route_selection_profile(
            &routing_config,
            &route,
            ports.model_profile_catalog.as_deref(),
        );
    let runtime_trace = build_runtime_decision_trace(RuntimeDecisionTraceInput {
        trace_id: runtime_trace_id.clone(),
        observed_at_unix,
        route_before: RuntimeTraceRouteRef::new(
            route_before_admission.provider,
            route_before_admission.model,
            route_before_admission.lane,
            route_before_admission.candidate_index,
        ),
        route_after: RuntimeTraceRouteRef::new(
            route.provider.clone(),
            route.model.clone(),
            route.lane,
            route.candidate_index,
        ),
        admission: &admission_decision,
        model_profile: &selected_route_profile,
        provider_context,
        context_cache: route.context_cache,
    });
    route.runtime_decision_traces = append_runtime_decision_trace_for_janitor(
        &route.runtime_decision_traces,
        runtime_trace,
        observed_at_unix,
    );
    if !implicit_memory_recall.runtime_memory_decisions.is_empty()
        || !implicit_memory_recall.runtime_notes.is_empty()
    {
        route.runtime_decision_traces = merge_runtime_decision_trace_update(
            &route.runtime_decision_traces,
            &runtime_trace_id,
            RuntimeDecisionTraceUpdate {
                memory: implicit_memory_recall.runtime_memory_decisions.clone(),
                notes: implicit_memory_recall.runtime_notes.clone(),
                ..Default::default()
            },
            observed_at_unix,
            RUNTIME_TRACE_JANITOR_TTL_SECS,
        );
    }
    let memory_backend_healthy = if let Some(memory) = ports.memory.as_ref() {
        Some(memory.health_check().await)
    } else {
        None
    };
    let embedding_profile = ports
        .memory
        .as_ref()
        .map(|memory| memory.embedding_profile());
    let channel_available = matches!(envelope.source_kind, SourceKind::Channel)
        .then(|| ports.channel_registry.has_channel(&envelope.source_adapter));
    let subsystem_observations =
        build_runtime_subsystem_observations(RuntimeSubsystemObservationInput {
            memory_backend_healthy,
            embedding_profile: embedding_profile.as_ref(),
            channel_available,
            now_unix: observed_at_unix,
        });
    let runtime_watchdog_digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
        last_admission: route.last_admission.as_ref(),
        recent_admissions: &route.recent_admissions,
        last_tool_repair: route.last_tool_repair.as_ref(),
        recent_tool_repairs: &route.recent_tool_repairs,
        context_cache: route.context_cache.as_ref(),
        assumptions: &route.assumptions,
        calibration_records: &route.calibrations,
        decision_traces: &route.runtime_decision_traces,
        subsystem_observations: &subsystem_observations,
        now_unix: observed_at_unix,
    });
    route.watchdog_alerts = append_runtime_watchdog_alerts(
        &route.watchdog_alerts,
        &runtime_watchdog_digest.alerts,
        observed_at_unix,
    );
    let mut handoff_tool_repairs = route.recent_tool_repairs.clone();
    if let Some(last_tool_repair) = route.last_tool_repair.clone() {
        if !handoff_tool_repairs.contains(&last_tool_repair) {
            handoff_tool_repairs.push(last_tool_repair);
        }
    }
    route.last_tool_repair = None;
    route.recent_tool_repairs.clear();

    if admission_decision.snapshot.action
        == crate::domain::turn_admission::TurnAdmissionAction::Block
    {
        let handoff_packet = build_session_handoff_packet(SessionHandoffInput {
            user_message: content,
            interpretation: interpretation.as_ref(),
            recent_admission_repair: admission_decision.recommended_action,
            recent_admission_reasons: &admission_decision.reasons,
            recalled_entries: &[],
            session_matches: &[],
            run_recipes: &[],
            recent_tool_repairs: &handoff_tool_repairs,
        });
        if let Some(packet) = handoff_packet.as_ref() {
            route.handoff_artifacts =
                append_runtime_handoff_packet(&route.handoff_artifacts, packet, observed_at_unix);
        }
        ports.routes.set_route(conversation_key, route.clone());
        return Ok(HandleResult::Response {
            conversation_key: conversation_key.to_string(),
            output: AssistantOutputPresenter::failure(
                format_blocked_turn_admission_response(
                    &admission_decision,
                    handoff_packet.as_ref(),
                ),
                AgentRuntimeErrorKind::CapabilityMismatch,
                delivery_hints.clone(),
            ),
        });
    }
    ports.routes.set_route(conversation_key, route.clone());

    if admission_decision.requires_compaction {
        let keep_non_system_turns =
            admission_session_hygiene_keep_non_system_turns(&admission_decision);
        let dropped =
            session_hygiene_dropped_messages_with_indices(&history, keep_non_system_turns);
        let dropped_messages = dropped
            .iter()
            .map(|entry| entry.message.clone())
            .collect::<Vec<_>>();
        let dropped_indices = dropped.iter().map(|entry| entry.index).collect::<Vec<_>>();
        let start_index = dropped_indices.first().copied().unwrap_or(0);
        let end_index = dropped_indices
            .last()
            .map(|index| index.saturating_add(1))
            .unwrap_or(start_index);
        if !history.is_empty() {
            let transcript = build_compaction_transcript(
                &dropped_messages,
                HistoryCompressionPolicy::default().max_source_chars,
            );
            let handoff_report = execute_memory_precompress_handoff(
                ports.memory.as_ref().map(|memory| memory.as_ref()),
                MemoryPreCompressHandoffInput {
                    agent_id: &config.agent_id,
                    reason: MemoryPreCompressHandoffReason::ChannelSessionHygiene,
                    start_index,
                    end_index,
                    transcript: &transcript,
                    messages: &dropped_messages,
                    message_indices: &dropped_indices,
                    recent_tool_repairs: &handoff_tool_repairs,
                    run_recipe_store: ports.run_recipe_store.as_ref().map(|store| store.as_ref()),
                    observed_at_unix,
                },
            )
            .await;
            let preservation_message =
                precompress_preservation_message(&handoff_report.preservation_hints);
            if !handoff_report.runtime_memory_decisions.is_empty() {
                route.runtime_decision_traces = merge_runtime_decision_trace_update(
                    &route.runtime_decision_traces,
                    &runtime_trace_id,
                    RuntimeDecisionTraceUpdate {
                        memory: handoff_report.runtime_memory_decisions,
                        ..Default::default()
                    },
                    observed_at_unix,
                    RUNTIME_TRACE_JANITOR_TTL_SECS,
                );
                ports.routes.set_route(conversation_key, route.clone());
            }
            if let Some(message) = preservation_message.as_ref() {
                insert_preservation_message_before_current_user(&mut history, content, message);
            }
            if let Some(message) = preservation_message {
                if ports.history.rollback_last_turn(conversation_key, content) {
                    ports.history.append_turn(conversation_key, message);
                    ports
                        .history
                        .append_turn(conversation_key, ChatMessage::user(content));
                }
            }
        }
        let stored_compacted = ports
            .history
            .compact_history(conversation_key, keep_non_system_turns);
        if stored_compacted {
            route.runtime_decision_traces = merge_runtime_decision_trace_update(
                &route.runtime_decision_traces,
                &runtime_trace_id,
                RuntimeDecisionTraceUpdate {
                    notes: vec![post_compaction_pressure_note(
                        &history,
                    )],
                    ..Default::default()
                },
                observed_at_unix,
                RUNTIME_TRACE_JANITOR_TTL_SECS,
            );
            ports.routes.set_route(conversation_key, route.clone());
        }
        let provider_history_compacted =
            compact_provider_history_for_session_hygiene(&mut history, keep_non_system_turns);
        let precompress_preservation_messages = history
            .iter()
            .filter(|message| is_precompress_preservation_message(message))
            .count();
        tracing::info!(
            conversation_key,
            stored_compacted,
            provider_history_compacted,
            keep_non_system_turns,
            precompress_preservation_messages,
            "Compacted channel session before agent execution"
        );
    }
    if let Some(block) = format_runtime_watchdog_context(&runtime_watchdog_digest) {
        history.push(ChatMessage::system(block));
    }
    let attached_precompress_preservation_messages =
        attach_precompress_preservation_messages_to_current_user(&mut history, content);
    let precompress_preservation_messages = history
        .iter()
        .filter(|message| is_precompress_preservation_message(message))
        .count();
    if precompress_preservation_messages > 0 || attached_precompress_preservation_messages > 0 {
        tracing::info!(
            conversation_key,
            precompress_preservation_messages,
            attached_precompress_preservation_messages,
            provider_history_messages = history.len(),
            "Channel provider history includes pre-compress handoff"
        );
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
        config.emit_compact_progress,
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
    let context_recovery_dropped = session_hygiene_dropped_messages_with_indices(&history, 6);
    let context_recovery_dropped_messages = context_recovery_dropped
        .iter()
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>();
    let context_recovery_dropped_indices = context_recovery_dropped
        .iter()
        .map(|entry| entry.index)
        .collect::<Vec<_>>();
    let context_recovery_history = history.clone();
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
            let response_text_for_delivery = response_text.clone();

            // ── #14: Finalize draft if the transport used a streaming draft.
            let mut response_delivery_hints = delivery_hints.clone();
            let draft_delivered = matches!(draft_state.as_ref(), Some((_, Some(_))));
            if let Some((_, Some(ref draft_id))) = draft_state {
                let _ = ports
                    .channel_output
                    .finalize_draft(&envelope.reply_ref, draft_id, &response_text_for_delivery)
                    .await;
                response_delivery_hints.already_delivered = turn_result.media_artifacts.is_empty();
            }

            // ── #15: Tool context summary for history ────────────
            let tool_summary = &turn_result.tool_summary;
            let history_response =
                if inbound_message_service::should_include_tool_summary(tool_summary, caps) {
                    format!("{tool_summary}\n{response_text_for_delivery}")
                } else {
                    response_text_for_delivery.clone()
                };

            if let Some(usage) = turn_result.usage.as_ref() {
                let default_prices = crate::config::model_catalog::default_pricing_table();
                let (pricing_status, pricing) =
                    runtime_pricing_status_for_route(
                        &default_prices,
                        true,
                        &route.provider,
                        &route.model,
                    );
                let estimated_cost_microusd = match pricing_status {
                    RuntimePricingStatus::Known => estimate_usage_cost_microusd(
                        usage.input_tokens,
                        usage.output_tokens,
                        pricing.as_ref(),
                    ),
                    _ => 0,
                };
                route.usage_ledger = record_runtime_usage(
                    route.usage_ledger,
                    RuntimeUsageRecordInput {
                        provider: &route.provider,
                        model: &route.model,
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cached_input_tokens: usage.cached_input_tokens,
                        pricing_status,
                        estimated_cost_microusd,
                    },
                );
                if let Some(store) = ports.conversation_store.as_ref() {
                    let _ = crate::application::services::conversation_service::add_token_usage(
                        store.as_ref(),
                        conversation_key,
                        usage.input_tokens.unwrap_or(0) as i64,
                        usage.output_tokens.unwrap_or(0) as i64,
                    )
                    .await;
                }
            }

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
            route.runtime_decision_traces = merge_runtime_decision_trace_update(
                &route.runtime_decision_traces,
                &runtime_trace_id,
                RuntimeDecisionTraceUpdate {
                    tools: runtime_tool_decisions_from_repairs(&turn_result.tool_repairs),
                    ..Default::default()
                },
                chrono::Utc::now().timestamp(),
                RUNTIME_TRACE_JANITOR_TTL_SECS,
            );
            route.calibrations =
                crate::application::services::runtime_calibration::append_tool_fact_calibration_observations(
                    &route.calibrations,
                    &turn_result.tool_facts,
                    chrono::Utc::now().timestamp(),
                )
                .records;
            let post_turn_tool_repairs = route.recent_tool_repairs.clone();
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
                        &response_text_for_delivery,
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
                    assistant_response: response_text_for_delivery.clone(),
                    tools_used: turn_result.tool_names.clone(),
                    tool_facts: turn_result.tool_facts.clone(),
                    tool_repairs: post_turn_tool_repairs.clone(),
                    run_recipe_store: ports.run_recipe_store.clone(),
                    user_profile_store: ports.user_profile_store.clone(),
                    user_profile_key: Some(channel_user_profile_key(&config.agent_id, envelope)),
                    auto_save_enabled: config.auto_save_memory,
                    event_tx: ports.event_tx.clone(),
                    runtime_trace_sink: Some(
                        crate::application::services::post_turn_orchestrator::PostTurnRuntimeTraceSink {
                            trace_id: runtime_trace_id.clone(),
                            conversation_key: conversation_key.to_string(),
                            routes: Arc::clone(&ports.routes),
                        },
                    ),
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
                output: AssistantOutputPresenter::success(
                    if (draft_delivered && !turn_result.media_artifacts.is_empty())
                        || voice_reply_delivered_to_current_conversation(
                            &turn_result.tool_facts,
                            envelope,
                        )
                    {
                        String::new()
                    } else {
                        response_text_for_delivery
                    },
                    turn_result.media_artifacts,
                    turn_result.tool_summary,
                    turn_result.tools_used,
                    response_delivery_hints,
                ),
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
                if !context_recovery_history.is_empty() {
                    let transcript = build_compaction_transcript(
                        &context_recovery_dropped_messages,
                        HistoryCompressionPolicy::default().max_source_chars,
                    );
                    let handoff_report = execute_memory_precompress_handoff(
                        ports.memory.as_ref().map(|memory| memory.as_ref()),
                        MemoryPreCompressHandoffInput {
                            agent_id: &config.agent_id,
                            reason: MemoryPreCompressHandoffReason::ChannelSessionHygiene,
                            start_index: context_recovery_dropped_indices
                                .first()
                                .copied()
                                .unwrap_or(0),
                            end_index: context_recovery_dropped_indices
                                .last()
                                .map(|index| index.saturating_add(1))
                                .unwrap_or(0),
                            transcript: &transcript,
                            messages: &context_recovery_dropped_messages,
                            message_indices: &context_recovery_dropped_indices,
                            recent_tool_repairs: &handoff_tool_repairs,
                            run_recipe_store: ports
                                .run_recipe_store
                                .as_ref()
                                .map(|store| store.as_ref()),
                            observed_at_unix,
                        },
                    )
                    .await;
                    let preservation_message =
                        precompress_preservation_message(&handoff_report.preservation_hints);
                    if !handoff_report.runtime_memory_decisions.is_empty() {
                        route.runtime_decision_traces = merge_runtime_decision_trace_update(
                            &route.runtime_decision_traces,
                            &runtime_trace_id,
                            RuntimeDecisionTraceUpdate {
                                memory: handoff_report.runtime_memory_decisions,
                                ..Default::default()
                            },
                            observed_at_unix,
                            RUNTIME_TRACE_JANITOR_TTL_SECS,
                        );
                        ports.routes.set_route(conversation_key, route.clone());
                    }
                    if let Some(message) = preservation_message {
                        if ports.history.rollback_last_turn(conversation_key, content) {
                            ports.history.append_turn(conversation_key, message);
                            ports
                                .history
                                .append_turn(conversation_key, ChatMessage::user(content));
                        }
                    }
                }
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
                let msg = format_context_limit_recovery_response(compacted);
                // Don't rollback — keep the user turn for retry
                return Ok(HandleResult::Response {
                    conversation_key: conversation_key.to_string(),
                    output: AssistantOutputPresenter::failure(
                        msg,
                        AgentRuntimeErrorKind::ContextLimitExceeded,
                        delivery_hints.clone(),
                    ),
                });
            }

            // ── #22: Timeout detection ───────────────────────────
            if matches!(e.kind, AgentRuntimeErrorKind::Timeout) {
                ports.history.rollback_last_turn(conversation_key, content);
                let msg = format_timeout_recovery_response();
                return Ok(HandleResult::Response {
                    conversation_key: conversation_key.to_string(),
                    output: AssistantOutputPresenter::failure(
                        msg,
                        AgentRuntimeErrorKind::Timeout,
                        delivery_hints.clone(),
                    ),
                });
            }

            // ── #21: Generic error ───────────────────────────────
            ports.history.rollback_last_turn(conversation_key, content);
            let msg = format_runtime_failure_response(&e);

            // Return as Response so caller doesn't double-send error
            Ok(HandleResult::Response {
                conversation_key: conversation_key.to_string(),
                output: AssistantOutputPresenter::failure(msg, e.kind, delivery_hints.clone()),
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
        AgentRuntimeErrorKind::MissingResource => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::WorkspaceAnchor,
            value: "missing_resource",
            invalidation: RuntimeAssumptionInvalidation::UserContradiction,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        }),
        AgentRuntimeErrorKind::PolicyBlocked => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::RouteCapability,
            value: "policy_blocked",
            invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        }),
        AgentRuntimeErrorKind::Timeout
        | AgentRuntimeErrorKind::SchemaMismatch
        | AgentRuntimeErrorKind::RuntimeFailure => None,
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

fn post_compaction_pressure_note(
    history: &[ChatMessage],
) -> crate::application::services::runtime_decision_trace::RuntimeTraceNote {
    let context = provider_context_input_for_history(history);
    let assessment =
        crate::application::services::provider_context_budget::assess_provider_context_budget(
            context,
        );
    let basis_points =
        crate::application::services::runtime_usage_insight_service::pressure_basis_points(
            assessment.snapshot.estimated_total_tokens,
            assessment.snapshot.ceiling_total_tokens,
        );
    crate::application::services::runtime_decision_trace::RuntimeTraceNote {
        observed_at_unix: chrono::Utc::now().timestamp(),
        kind: "post_compaction_pressure".into(),
        detail: format!(
            "basis_points={} estimated_tokens={} ceiling_tokens={}",
            basis_points,
            assessment.snapshot.estimated_total_tokens,
            assessment.snapshot.ceiling_total_tokens
        ),
    }
}

fn insert_preservation_message_before_current_user(
    history: &mut Vec<ChatMessage>,
    current_user_content: &str,
    message: &ChatMessage,
) {
    let insert_at = history
        .iter()
        .rposition(|turn| turn.role == "user" && turn.content == current_user_content)
        .unwrap_or(history.len());
    history.insert(insert_at, message.clone());
}

fn attach_precompress_preservation_messages_to_current_user(
    history: &mut Vec<ChatMessage>,
    current_user_content: &str,
) -> usize {
    let mut preservation_messages = Vec::new();
    let mut index = 0;
    while index < history.len() {
        if is_precompress_preservation_message(&history[index]) {
            let message = history.remove(index);
            if !preservation_messages
                .iter()
                .any(|existing: &ChatMessage| existing.content == message.content)
            {
                preservation_messages.push(message);
            }
        } else {
            index += 1;
        }
    }
    if preservation_messages.is_empty() {
        return 0;
    }

    let Some(current_user_index) = history
        .iter()
        .rposition(|turn| turn.role == "user" && turn.content == current_user_content)
    else {
        let insert_at = history.len();
        let count = preservation_messages.len();
        history.splice(insert_at..insert_at, preservation_messages);
        return count;
    };

    let count = preservation_messages.len();
    let handoff = preservation_messages
        .into_iter()
        .map(|message| message.content)
        .collect::<Vec<_>>()
        .join("\n\n");
    let current = history[current_user_index].content.clone();
    history[current_user_index].content = format!("{handoff}\n\n[current-user-message]\n{current}");
    count
}

fn admission_session_hygiene_keep_non_system_turns(
    decision: &crate::application::services::turn_admission::CandidateAdmissionDecision,
) -> usize {
    if decision.reasons.iter().any(|reason| {
        matches!(
            reason,
            CandidateAdmissionReason::ProviderContextOverflowRisk
                | CandidateAdmissionReason::CandidateWindowExceeded
        )
    }) {
        return (SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS / 3).max(2);
    }

    SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS
}

fn channel_user_profile_key(agent_id: &str, envelope: &InboundEnvelope) -> String {
    inbound_message_service::conversation_identity(envelope, agent_id).actor_profile_key()
}

fn voice_reply_delivered_to_current_conversation(
    facts: &[TypedToolFact],
    envelope: &InboundEnvelope,
) -> bool {
    let succeeded = facts.iter().any(|fact| {
        fact.tool_id == "voice_reply"
            && matches!(
                &fact.payload,
                ToolFactPayload::Outcome(ref outcome)
                    if outcome.status == OutcomeStatus::Succeeded
            )
    });
    if !succeeded {
        return false;
    }

    facts.iter().any(|fact| {
        fact.tool_id == "voice_reply"
            && matches!(
                &fact.payload,
                ToolFactPayload::Delivery(delivery)
                    if match &delivery.target {
                        DeliveryTargetKind::CurrentConversation => true,
                        DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                            channel,
                            recipient,
                            thread_ref,
                        }) => {
                            channel == &envelope.source_adapter
                                && recipient == &envelope.reply_ref
                                && thread_ref.as_deref() == envelope.thread_ref.as_deref()
                        }
                        _ => false,
                    }
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        ModelCandidateProfileConfig, ModelLaneCandidateConfig, ModelLaneConfig,
    };
    use crate::domain::channel::SourceKind;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingProfile, Entity, HybridSearchResult,
        MemoryCategory, MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, SearchResult,
        SearchSource, SessionId, Skill, SkillUpdate, TemporalFact, Visibility,
    };
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
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
                runtime_decision_traces: Vec::new(),
                usage_ledger: Default::default(),
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
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
                runtime_decision_traces: Vec::new(),
                usage_ledger: Default::default(),
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
                media_artifacts: vec![],
                usage: None,
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

    #[derive(Default)]
    struct RecordingRuntime {
        history: Mutex<Vec<ChatMessage>>,
        response: String,
    }

    #[async_trait]
    impl AgentRuntimePort for RecordingRuntime {
        async fn execute_turn(
            &self,
            history: Vec<ChatMessage>,
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
            *self.history.lock().unwrap() = history;
            Ok(crate::ports::agent_runtime::AgentTurnResult {
                response: self.response.clone(),
                history: vec![],
                tools_used: false,
                tool_names: vec![],
                tool_facts: vec![],
                tool_summary: String::new(),
                last_tool_repair: None,
                tool_repairs: vec![],
                media_artifacts: vec![],
                usage: None,
            })
        }
    }

    #[derive(Default)]
    struct StubMemory {
        hybrid: HybridSearchResult,
    }

    #[async_trait]
    impl crate::ports::memory::WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> { Ok(vec![]) }
        async fn update_core_block(&self, _: &AgentId, _: &str, _: String) -> Result<(), MemoryError> { Ok(()) }
        async fn append_core_block(&self, _: &AgentId, _: &str, _: &str) -> Result<(), MemoryError> { Ok(()) }
    }
    #[async_trait]
    impl crate::ports::memory::EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> { Ok(vec![]) }
    }
    #[async_trait]
    impl crate::ports::memory::SemanticMemoryPort for StubMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> { Ok(None) }
        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> { Ok(()) }
        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> { Ok(vec![]) }
        async fn traverse(&self, _: &MemoryId, _: usize) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> { Ok(vec![]) }
        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> { Ok(vec![]) }
    }
    #[async_trait]
    impl crate::ports::memory::SkillMemoryPort for StubMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> { Ok(vec![]) }
        async fn update_skill(&self, _: &MemoryId, _: SkillUpdate, _: &AgentId) -> Result<(), MemoryError> { Ok(()) }
        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> { Ok(None) }
    }
    #[async_trait]
    impl crate::ports::memory::ReflectionPort for StubMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn get_relevant_reflections(&self, _: &MemoryQuery) -> Result<Vec<Reflection>, MemoryError> { Ok(vec![]) }
        async fn get_failure_patterns(&self, _: &AgentId, _: usize) -> Result<Vec<Reflection>, MemoryError> { Ok(vec![]) }
    }
    #[async_trait]
    impl crate::ports::memory::ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> { Ok(ConsolidationReport::default()) }
        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> { Ok(0) }
        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> { Ok(0) }
    }
    #[async_trait]
    impl crate::ports::memory::UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> { Ok(self.hybrid.clone()) }
        async fn embed(&self, _: &str) -> Result<Vec<f32>, MemoryError> { Ok(vec![0.1]) }
        async fn store(&self, _: &str, _: &str, _: &MemoryCategory, _: Option<&str>) -> Result<(), MemoryError> { Ok(()) }
        async fn recall(&self, _: &str, _: usize, _: Option<&str>) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> { Ok(()) }
        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> { Ok(false) }
        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> { Ok(None) }
        async fn list(&self, _: Option<&MemoryCategory>, _: Option<&str>, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn count(&self) -> Result<usize, MemoryError> { Ok(0) }
        fn name(&self) -> &str { "stub" }
        async fn health_check(&self) -> bool { true }
        fn embedding_profile(&self) -> EmbeddingProfile { EmbeddingProfile::default() }
        async fn promote_visibility(&self, _: &MemoryId, _: &Visibility, _: &[AgentId], _: &AgentId) -> Result<(), MemoryError> { Ok(()) }
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

    #[test]
    fn precompress_handoff_is_attached_to_current_user_after_core_memory() {
        let preservation = precompress_preservation_message(&[String::from(
            "stable_project_fact project=Atlas branch=release/hotfix-17 staging=https://staging.atlas.local",
        )])
        .expect("preservation message");
        let mut history = vec![
            ChatMessage::system("bootstrap"),
            preservation.clone(),
            ChatMessage::user("old"),
            ChatMessage::assistant("old answer"),
            ChatMessage::system("[core-memory]\nolder project=Legacy"),
            ChatMessage::user("current"),
        ];

        let attached =
            attach_precompress_preservation_messages_to_current_user(&mut history, "current");

        assert_eq!(attached, 1);
        let current = history
            .iter()
            .find(|message| message.role == "user" && message.content.contains("current"))
            .expect("current user");
        assert!(current.content.starts_with("[pre-compress-handoff]\n"));
        assert!(current.content.contains("project=Atlas"));
        assert!(current.content.contains("[current-user-message]\ncurrent"));
        assert_eq!(
            history
                .iter()
                .filter(|message| is_precompress_preservation_message(message))
                .count(),
            0
        );
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
        fn capability_profiles(
            &self,
        ) -> Vec<crate::ports::channel_registry::ChannelCapabilityProfile> {
            vec![self.capability_profile("test")]
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
            emit_compact_progress: true,
        }
    }

    fn config_with_route_window(
        provider: &str,
        model: &str,
        context_window_tokens: usize,
    ) -> InboundMessageConfig {
        let mut config = test_config();
        config.model_lanes = vec![ModelLaneConfig {
            lane: crate::config::schema::CapabilityLane::Reasoning,
            candidates: vec![ModelLaneCandidateConfig {
                provider: provider.to_string(),
                model: model.to_string(),
                api_key: None,
                api_key_env: None,
                dimensions: None,
                profile: ModelCandidateProfileConfig {
                    context_window_tokens: Some(context_window_tokens),
                    ..Default::default()
                },
            }],
        }];
        config
    }

    fn test_envelope(content: &str) -> InboundEnvelope {
        InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user1".into(),
            conversation_id: "telegram_user1".into(),
            event_ref: None,
            reply_ref: "chat123".into(),
            thread_ref: None,
            media_attachments: Vec::new(),
            content: content.into(),
            received_at: 0,
        }
    }

    fn test_conversation_key() -> &'static str {
        "conversation:test-agent:telegram:telegram_user1:user1"
    }

    #[test]
    fn detects_successful_voice_reply_to_current_conversation() {
        let envelope = test_envelope("send this as voice");
        let facts = vec![
            TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Outcome(crate::domain::tool_fact::OutcomeFact {
                    status: OutcomeStatus::Succeeded,
                    duration_ms: Some(10),
                }),
            },
            TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Delivery(crate::domain::tool_fact::DeliveryFact {
                    target: DeliveryTargetKind::CurrentConversation,
                    content_bytes: Some(128),
                }),
            },
        ];

        assert!(voice_reply_delivered_to_current_conversation(
            &facts, &envelope
        ));
    }

    #[test]
    fn rejects_failed_or_mismatched_voice_reply_delivery() {
        let envelope = test_envelope("send this as voice");
        let failed = vec![
            TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Outcome(crate::domain::tool_fact::OutcomeFact {
                    status: OutcomeStatus::ReportedFailure,
                    duration_ms: Some(10),
                }),
            },
            TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Delivery(crate::domain::tool_fact::DeliveryFact {
                    target: DeliveryTargetKind::CurrentConversation,
                    content_bytes: Some(128),
                }),
            },
        ];
        assert!(!voice_reply_delivered_to_current_conversation(
            &failed, &envelope
        ));

        let mismatched_target = vec![
            TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Outcome(crate::domain::tool_fact::OutcomeFact {
                    status: OutcomeStatus::Succeeded,
                    duration_ms: Some(10),
                }),
            },
            TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Delivery(crate::domain::tool_fact::DeliveryFact {
                    target: DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                        channel: "matrix".into(),
                        recipient: "!room:example".into(),
                        thread_ref: None,
                    }),
                    content_bytes: Some(128),
                }),
            },
        ];
        assert!(!voice_reply_delivered_to_current_conversation(
            &mismatched_target,
            &envelope
        ));
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
            HandleResult::Response { output, .. } => assert_eq!(output.text, "Hi!"),
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_regular_message_persists_turns() {
        let env = test_envelope("Hello");
        let caps = vec![ChannelCapability::SendText];
        let ports = test_ports("Hi!");
        handle(&env, &caps, &test_config(), &ports).await.unwrap();
        assert_eq!(ports.history.get_history(test_conversation_key()).len(), 2);
    }

    #[tokio::test]
    async fn implicit_memory_recall_guidance_is_injected_before_runtime() {
        let env = test_envelope("What is our self-hosted Matrix server?");
        let caps = vec![ChannelCapability::SendText];
        let runtime = Arc::new(RecordingRuntime {
            response: "It runs as tuwunel.service.".into(),
            ..Default::default()
        });
        let memory = Arc::new(StubMemory {
            hybrid: HybridSearchResult {
                episodes: vec![SearchResult {
                    entry: MemoryEntry {
                        id: "1".into(),
                        key: "local_infra_matrix_homeserver".into(),
                        content: "Our self-hosted Matrix homeserver runs as tuwunel.service package tuwunel.".into(),
                        category: MemoryCategory::Custom("local_infra".into()),
                        timestamp: "2026-04-20T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.96),
                    },
                    score: 0.96,
                    source: SearchSource::Hybrid,
                }],
                ..Default::default()
            },
        });
        let ports = InboundMessagePorts {
            history: Arc::new(MockHistory::new()),
            routes: Arc::new(RecordingRoutes::new("openrouter", "default-model")),
            hooks: Arc::new(NoOpHooks),
            channel_output: Arc::new(MockChannelOutput),
            agent_runtime: runtime.clone(),
            channel_registry: Arc::new(MockRegistry),
            session_summary: None,
            memory: Some(memory),
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
            HandleResult::Response { .. } => {}
            other => panic!("expected Response, got {other:?}"),
        }

        let history = runtime.history.lock().unwrap().clone();
        let implicit_block = history
            .iter()
            .find(|message| message.role == "system" && message.content.contains("[implicit-memory-recall]"))
            .expect("implicit memory recall block");
        assert!(implicit_block.content.contains("tuwunel.service"));
        assert!(implicit_block.content.contains("avoid broad host inventory"));
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
    async fn ack_reactions_use_event_ref_not_conversation_id() {
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
        env.conversation_id = "telegram_user1".into();

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
            .append_turn(test_conversation_key(), ChatMessage::user("old"));
        let result = handle(&env, &caps, &test_config(), &ports).await.unwrap();
        assert!(matches!(
            result,
            HandleResult::Command {
                effect: CommandEffect::ClearSession,
                ..
            }
        ));
        assert!(ports.history.has_history(test_conversation_key()));
    }

    #[tokio::test]
    async fn channel_model_switch_waits_for_adapter_preflight() {
        let env = test_envelope("/model tiny-model");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let history = Arc::new(MockHistory::new());
        history.append_turn(
            test_conversation_key(),
            ChatMessage::user("x".repeat(20_000)),
        );
        let routes = Arc::new(RecordingRoutes::new("openrouter", "large-model"));
        let ports = test_ports_with_state(history, routes.clone(), "");
        let config = config_with_route_window("openrouter", "tiny-model", 1_000);

        let result = handle(&env, &caps, &config, &ports).await.unwrap();

        match result {
            HandleResult::Command {
                effect:
                    CommandEffect::SwitchModel {
                        model,
                        inferred_provider,
                        lane,
                        candidate_index,
                        compacted,
                    },
                ..
            } => {
                assert_eq!(model, "tiny-model");
                assert_eq!(inferred_provider, Some("openrouter".to_string()));
                assert_eq!(lane, Some(crate::config::schema::CapabilityLane::Reasoning));
                assert_eq!(candidate_index, Some(0));
                assert!(!compacted);
            }
            other => panic!("expected model switch command, got {other:?}"),
        }
        assert_eq!(routes.set_count(), 0);
        assert!(routes.current_route().is_none());
    }

    #[tokio::test]
    async fn channel_model_switch_defers_safe_downshift_mutation_to_adapter() {
        let env = test_envelope("/model compact-model");
        let caps = vec![
            ChannelCapability::SendText,
            ChannelCapability::RuntimeCommands,
        ];
        let history = Arc::new(MockHistory::new());
        for idx in 0..20 {
            history.append_turn(
                test_conversation_key(),
                ChatMessage::user(format!("{idx}: {}", "x".repeat(1_000))),
            );
        }
        let routes = Arc::new(RecordingRoutes::new("openrouter", "large-model"));
        let ports = test_ports_with_state(history.clone(), routes.clone(), "");
        let config = config_with_route_window("openrouter", "compact-model", 8_000);

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
        assert_eq!(history.get_history(test_conversation_key()).len(), 20);
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
            HandleResult::Response { output, .. } => {
                assert!(output.text.contains("Context window exceeded"));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn runtime_error_challenges_cover_typed_resource_and_policy_failures() {
        let missing =
            agent_runtime_error_assumption_challenge(&AgentRuntimeErrorKind::MissingResource)
                .expect("missing resource should challenge workspace assumptions");
        assert_eq!(missing.kind, RuntimeAssumptionKind::WorkspaceAnchor);
        assert_eq!(
            missing.replacement_path,
            RuntimeAssumptionReplacementPath::AskUserClarification
        );

        let blocked =
            agent_runtime_error_assumption_challenge(&AgentRuntimeErrorKind::PolicyBlocked)
                .expect("policy block should challenge route capability assumptions");
        assert_eq!(blocked.kind, RuntimeAssumptionKind::RouteCapability);
        assert_eq!(
            blocked.invalidation,
            RuntimeAssumptionInvalidation::RouteAdmissionFailure
        );
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
}
