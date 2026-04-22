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

use crate::application::services::model_preset_resolution::resolve_effective_model_lanes;
use crate::application::services::route_switch_preflight::RouteSwitchPreflight;
use crate::config::schema::{CapabilityLane, Config};
use crate::domain::channel::{
    ChannelCapability, ConversationIdentity, InboundEnvelope, InboundMediaAttachment,
};
use crate::domain::memory::SkillStatus;

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
    /// Show runtime capability readiness graph.
    ShowDoctor,
    /// Show governed skill runtime status.
    ShowSkills,
    /// Show governed skill items that are blocked or unavailable.
    ShowBlockedSkills,
    /// Show governed skill candidates awaiting review.
    ShowSkillCandidates,
    /// Create a memory-backed user-authored skill from explicit command text.
    CreateUserSkill {
        name: String,
        body: String,
        metadata: RuntimeUserSkillCreateMetadata,
    },
    /// Update a memory-backed user-authored or learned skill from explicit command text.
    UpdateUserSkill {
        skill: String,
        body: String,
        metadata: RuntimeUserSkillCreateMetadata,
    },
    /// Show runtime tool replay contract inventory used by skill replay/eval.
    ShowSkillTools,
    /// Show compact skill use traces recorded after live/replay skill execution.
    ShowSkillTraces,
    /// Show or apply operator-approved skill catalog cleanup recommendations.
    ShowSkillHealth { apply: bool },
    /// Show a compact diff/review view for a generated skill patch candidate.
    ShowSkillDiff(String),
    /// Apply a generated skill patch candidate after replay/eval gates pass.
    ApplySkillPatch(String),
    /// Show applied patch/rollback version records for generated skill patches.
    ShowSkillVersions(Option<String>),
    /// Roll back a generated skill patch by apply record or rollback snapshot.
    RollbackSkillPatch(String),
    /// Evaluate or apply generated patch auto-promotion policy.
    AutoPromoteSkills { apply: bool },
    /// Review learned/generated skills without or with applying deterministic decisions.
    ReviewSkills { apply: bool },
    /// Promote a learned/generated skill to active.
    PromoteSkill(String),
    /// Move a learned/generated skill back to candidate status.
    DemoteSkill(String),
    /// Reject a learned/generated skill by marking it deprecated.
    RejectSkill(String),
    /// Compact the current conversation session without changing route.
    CompactSession,
    /// Switch to a specific model.
    SetModel(String),
    /// Start a new conversation session.
    NewSession,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeUserSkillCreateMetadata {
    pub task_family: Option<String>,
    pub tool_pattern: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSkillStatusView {
    All,
    Blocked,
    Candidates,
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
    let command_tail = trimmed
        .strip_prefix(command_token)
        .unwrap_or("")
        .trim_start();

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
        "/doctor" => Some(RuntimeCommand::ShowDoctor),
        "/skills" => parse_skills_runtime_command(command_tail),
        "/compact" => Some(RuntimeCommand::CompactSession),
        "/new" => Some(RuntimeCommand::NewSession),
        _ => None,
    }
}

fn parse_skills_runtime_command(raw_args: &str) -> Option<RuntimeCommand> {
    let args = raw_args.split_whitespace().collect::<Vec<_>>();
    let Some(subcommand) = args.first() else {
        return Some(RuntimeCommand::ShowSkills);
    };
    let normalized = subcommand.trim().to_lowercase();
    match normalized.as_str() {
        "" | "status" | "list" => Some(RuntimeCommand::ShowSkills),
        "blocked" | "blockers" => Some(RuntimeCommand::ShowBlockedSkills),
        "candidate" | "candidates" | "queue" => Some(RuntimeCommand::ShowSkillCandidates),
        "create" => parse_skill_create_command_raw(
            raw_args.strip_prefix(subcommand).unwrap_or("").trim_start(),
        ),
        "update" | "edit" => parse_skill_update_command_raw(
            raw_args.strip_prefix(subcommand).unwrap_or("").trim_start(),
        ),
        "tools" | "tool-contracts" | "replay-tools" => Some(RuntimeCommand::ShowSkillTools),
        "trace" | "traces" | "use-traces" | "usage" => Some(RuntimeCommand::ShowSkillTraces),
        "health" | "hygiene" | "cleanup" => Some(RuntimeCommand::ShowSkillHealth {
            apply: args
                .iter()
                .skip(1)
                .any(|arg| matches!(arg.trim().to_lowercase().as_str(), "--apply" | "apply")),
        }),
        "diff" => parse_skill_ref_command(&args, RuntimeCommand::ShowSkillDiff),
        "apply" => parse_skill_ref_command(&args, RuntimeCommand::ApplySkillPatch),
        "version" | "versions" | "history" => Some(RuntimeCommand::ShowSkillVersions(
            parse_optional_skill_ref(&args),
        )),
        "rollback" | "revert" => parse_skill_ref_command(&args, RuntimeCommand::RollbackSkillPatch),
        "autopromote" | "auto-promote" => Some(RuntimeCommand::AutoPromoteSkills {
            apply: args
                .iter()
                .skip(1)
                .any(|arg| matches!(arg.trim().to_lowercase().as_str(), "--apply" | "apply")),
        }),
        "review" => Some(RuntimeCommand::ReviewSkills {
            apply: args
                .iter()
                .skip(1)
                .any(|arg| matches!(arg.trim().to_lowercase().as_str(), "--apply" | "apply")),
        }),
        "promote" => parse_skill_ref_command(&args, RuntimeCommand::PromoteSkill),
        "demote" => parse_skill_ref_command(&args, RuntimeCommand::DemoteSkill),
        "reject" | "deprecate" => parse_skill_ref_command(&args, RuntimeCommand::RejectSkill),
        _ => Some(RuntimeCommand::ShowSkills),
    }
}

fn parse_skill_create_command_raw(raw_args: &str) -> Option<RuntimeCommand> {
    let (name, body) = raw_args.split_once("::")?;
    let (name, metadata) = parse_skill_create_header(name.trim())?;
    let body = body.trim().to_string();
    if name.is_empty() || body.is_empty() {
        return None;
    }
    Some(RuntimeCommand::CreateUserSkill {
        name,
        body,
        metadata,
    })
}

fn parse_skill_update_command_raw(raw_args: &str) -> Option<RuntimeCommand> {
    let (skill, body) = raw_args.split_once("::")?;
    let (skill, metadata) = parse_skill_create_header(skill.trim())?;
    let body = body.trim().to_string();
    if skill.is_empty() || body.is_empty() {
        return None;
    }
    Some(RuntimeCommand::UpdateUserSkill {
        skill,
        body,
        metadata,
    })
}

fn parse_skill_create_header(raw_header: &str) -> Option<(String, RuntimeUserSkillCreateMetadata)> {
    let mut name_parts = Vec::new();
    let mut metadata = RuntimeUserSkillCreateMetadata::default();
    let mut args = raw_header.split_whitespace().peekable();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--task-family=") {
            metadata.task_family = non_empty_trimmed(value);
            continue;
        }
        if let Some(value) = arg.strip_prefix("--tools=") {
            metadata
                .tool_pattern
                .extend(parse_skill_metadata_list(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--tool-pattern=") {
            metadata
                .tool_pattern
                .extend(parse_skill_metadata_list(value));
            continue;
        }
        if let Some(value) = arg.strip_prefix("--tags=") {
            metadata.tags.extend(parse_skill_metadata_list(value));
            continue;
        }

        match arg {
            "--task-family" => {
                metadata.task_family = non_empty_trimmed(args.next()?);
            }
            "--tools" | "--tool-pattern" => {
                metadata
                    .tool_pattern
                    .extend(parse_skill_metadata_list(args.next()?));
            }
            "--tags" => {
                metadata
                    .tags
                    .extend(parse_skill_metadata_list(args.next()?));
            }
            value if value.starts_with("--") => return None,
            value => name_parts.push(value),
        }
    }
    let name = name_parts.join(" ").trim().to_string();
    (!name.is_empty()).then_some((name, metadata))
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_skill_metadata_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .flat_map(|part| part.split("->"))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_skill_ref_command(
    args: &[&str],
    build: impl FnOnce(String) -> RuntimeCommand,
) -> Option<RuntimeCommand> {
    let skill_ref = args
        .iter()
        .skip(1)
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if skill_ref.is_empty() {
        return None;
    }
    Some(build(skill_ref))
}

fn parse_optional_skill_ref(args: &[&str]) -> Option<String> {
    let skill_ref = args
        .iter()
        .skip(1)
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    (!skill_ref.is_empty()).then_some(skill_ref)
}

// ── Conversation key ─────────────────────────────────────────────

/// Construct the canonical conversation identity for an inbound envelope.
pub fn conversation_identity(envelope: &InboundEnvelope, agent_id: &str) -> ConversationIdentity {
    ConversationIdentity::from_envelope(agent_id, envelope)
}

pub fn conversation_key_for_agent(envelope: &InboundEnvelope, agent_id: &str) -> String {
    conversation_identity(envelope, agent_id).conversation_key()
}

pub fn conversation_scope_key_prefix_for_agent(
    envelope: &InboundEnvelope,
    agent_id: &str,
) -> String {
    conversation_identity(envelope, agent_id).conversation_scope_key_prefix()
}

/// Construct the canonical raw-autosave memory key for an inbound envelope.
///
/// Preference order:
/// - stable upstream event/message id when available
/// - otherwise a bounded fallback derived from receipt timestamp and content size
pub fn autosave_memory_key_for_agent(envelope: &InboundEnvelope, agent_id: &str) -> String {
    conversation_identity(envelope, agent_id)
        .autosave_memory_key(envelope.received_at, envelope.content.chars().count())
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

pub fn provider_facing_content(
    content: &str,
    media_attachments: &[InboundMediaAttachment],
) -> String {
    let mut normalized = content.to_string();
    for marker in media_attachments
        .iter()
        .filter_map(InboundMediaAttachment::marker)
    {
        if normalized.contains(&marker) {
            continue;
        }
        if !normalized.trim().is_empty() {
            normalized.push('\n');
        }
        normalized.push_str(&marker);
    }
    normalized
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
    /// Continuation turn: enrichment driven by `ContinuationPolicy` config.
    Continuation,
    /// Not first turn: no enrichment.
    None,
}

/// Decide how to enrich the conversation context for an inbound message.
///
/// Business rules:
/// - If there's already history for this conversation: no enrichment
/// - If the message is in a thread: seed from parent conversation summary
/// - Otherwise: load relevant memory context
pub fn decide_history_enrichment_for_agent(
    has_prior_history: bool,
    envelope: &InboundEnvelope,
    agent_id: &str,
) -> HistoryEnrichment {
    if has_prior_history {
        return HistoryEnrichment::Continuation;
    }

    let identity = conversation_identity(envelope, agent_id);
    match &envelope.thread_ref {
        Some(tid) => HistoryEnrichment::ThreadSeeding {
            parent_key: identity.parent_conversation_key(),
            thread_id: tid.clone(),
        },
        None => HistoryEnrichment::MemoryContext {
            conversation_key: identity.conversation_key(),
        },
    }
}

// ── Thread context: parent conversation excerpt ─────────────────

/// Extract the last N non-system turns from a parent conversation,
/// capped by a total character budget.
///
/// Each individual message is truncated to `budget / max_turns` characters
/// to prevent a single huge message from consuming the entire budget.
/// Returns a formatted string like `user: ...\nassistant: ...\n`.
pub fn smart_truncate_parent_turns(
    turns: &[crate::domain::message::ChatMessage],
    max_turns: usize,
    max_total_chars: usize,
) -> String {
    let non_system: Vec<_> = turns.iter().filter(|t| t.role != "system").collect();

    let per_msg_limit = max_total_chars / max_turns.max(1);
    let mut result = String::new();
    let mut budget = max_total_chars;

    for turn in non_system.iter().rev().take(max_turns).rev() {
        let content = if turn.content.chars().count() > per_msg_limit {
            format!(
                "{}...",
                turn.content.chars().take(per_msg_limit).collect::<String>()
            )
        } else {
            turn.content.clone()
        };

        let line = format!("{}: {}\n", turn.role, content);
        if line.len() > budget {
            break;
        }
        budget -= line.len();
        result.push_str(&line);
    }

    result
}

// ── Auto-save policy ─────────────────────────────────────────────

/// Minimum message length (in chars) to trigger auto-save.
pub const AUTOSAVE_MIN_MESSAGE_CHARS: usize =
    crate::application::services::memory_quality_governor::AUTOSAVE_MIN_CONTENT_CHARS;

/// Decide whether to auto-save an inbound message to memory.
///
/// Business rules:
/// - Auto-save must be enabled in config
/// - Message must be at least AUTOSAVE_MIN_MESSAGE_CHARS characters
/// - Content must not match skip patterns (e.g. single-word commands)
pub fn should_autosave(auto_save_enabled: bool, content: &str) -> bool {
    auto_save_enabled
        && matches!(
            crate::application::services::memory_quality_governor::assess_autosave_write(
                content,
                AUTOSAVE_MIN_MESSAGE_CHARS,
            ),
            crate::application::services::memory_quality_governor::AutosaveWriteVerdict::Write
        )
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
    should_autosave(auto_save_enabled, content)
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
    /// Display runtime capability readiness graph (no state change).
    ShowDoctor,
    /// Display governed skill runtime status (no state change).
    ShowSkills { view: RuntimeSkillStatusView },
    /// Create a memory-backed user-authored skill.
    CreateUserSkill {
        name: String,
        body: String,
        metadata: RuntimeUserSkillCreateMetadata,
    },
    /// Update a memory-backed user-authored or learned skill.
    UpdateUserSkill {
        skill: String,
        body: String,
        metadata: RuntimeUserSkillCreateMetadata,
    },
    /// Review learned/generated skill state (optionally applying decisions).
    ReviewSkills { apply: bool },
    /// Display runtime tool replay contract inventory (no state change).
    ShowSkillTools,
    /// Display compact skill use trace inventory (no state change).
    ShowSkillTraces,
    /// Display or apply skill catalog cleanup guidance.
    ShowSkillHealth { apply: bool },
    /// Display compact patch candidate diff (no state change).
    ShowSkillDiff { candidate: String },
    /// Apply generated skill patch candidate after replay/eval gates pass.
    ApplySkillPatch { candidate: String },
    /// Display generated skill patch version records.
    ShowSkillVersions { skill: Option<String> },
    /// Roll back generated skill patch from a saved rollback snapshot.
    RollbackSkillPatch { rollback: String },
    /// Evaluate or apply generated patch auto-promotion policy.
    AutoPromoteSkills { apply: bool },
    /// Update learned/generated skill lifecycle state.
    UpdateSkillStatus { skill: String, status: SkillStatus },
    /// Compact current conversation history without clearing route/session state.
    CompactSession {
        /// Adapter may fill this after execution for shared presentation.
        compacted: bool,
    },
    /// Switch to a specific model, optionally with inferred provider from routes.
    SwitchModel {
        model: String,
        /// If a lane or catalog alias matched, the coupled provider.
        inferred_provider: Option<String>,
        /// If a lane or catalog alias matched a capability lane, preserve it in route state.
        lane: Option<CapabilityLane>,
        /// Ordered lane candidate index when the selector matched a lane candidate.
        candidate_index: Option<usize>,
        /// Whether provider-facing context was compacted before the switch.
        compacted: bool,
    },
    /// Model switch was not applied because the target route cannot safely fit current context.
    SwitchModelBlocked {
        model: String,
        provider: String,
        lane: Option<CapabilityLane>,
        preflight: RouteSwitchPreflight,
        compacted: bool,
    },
    /// Clear conversation history and route overrides for this sender.
    ClearSession,
}

/// Determine the effect of a runtime command.
///
/// Resolves lane/catalog routes (model selector → provider) but leaves
/// provider validation to the adapter (requires infrastructure).
pub fn command_effect(command: &RuntimeCommand, config: &Config) -> CommandEffect {
    command_effect_with_alias_resolver(command, config, |value| {
        crate::config::model_catalog::route_alias(value).map(|route| ResolvedModelCommandRoute {
            provider: route.provider,
            model: route.model,
            lane: route.capability,
            candidate_index: None,
        })
    })
}

fn command_effect_with_alias_resolver(
    command: &RuntimeCommand,
    config: &Config,
    alias_resolver: impl Fn(&str) -> Option<ResolvedModelCommandRoute>,
) -> CommandEffect {
    match command {
        RuntimeCommand::ShowProviders => CommandEffect::ShowProviders,
        RuntimeCommand::SetProvider(raw) => CommandEffect::SwitchProvider {
            provider: raw.clone(),
        },
        RuntimeCommand::ShowModel => CommandEffect::ShowModel,
        RuntimeCommand::ShowDoctor => CommandEffect::ShowDoctor,
        RuntimeCommand::ShowSkills => CommandEffect::ShowSkills {
            view: RuntimeSkillStatusView::All,
        },
        RuntimeCommand::ShowBlockedSkills => CommandEffect::ShowSkills {
            view: RuntimeSkillStatusView::Blocked,
        },
        RuntimeCommand::ShowSkillCandidates => CommandEffect::ShowSkills {
            view: RuntimeSkillStatusView::Candidates,
        },
        RuntimeCommand::CreateUserSkill {
            name,
            body,
            metadata,
        } => CommandEffect::CreateUserSkill {
            name: name.clone(),
            body: body.clone(),
            metadata: metadata.clone(),
        },
        RuntimeCommand::UpdateUserSkill {
            skill,
            body,
            metadata,
        } => CommandEffect::UpdateUserSkill {
            skill: skill.clone(),
            body: body.clone(),
            metadata: metadata.clone(),
        },
        RuntimeCommand::ShowSkillTools => CommandEffect::ShowSkillTools,
        RuntimeCommand::ShowSkillTraces => CommandEffect::ShowSkillTraces,
        RuntimeCommand::ShowSkillHealth { apply } => {
            CommandEffect::ShowSkillHealth { apply: *apply }
        }
        RuntimeCommand::ShowSkillDiff(candidate) => CommandEffect::ShowSkillDiff {
            candidate: candidate.clone(),
        },
        RuntimeCommand::ApplySkillPatch(candidate) => CommandEffect::ApplySkillPatch {
            candidate: candidate.clone(),
        },
        RuntimeCommand::ShowSkillVersions(skill) => CommandEffect::ShowSkillVersions {
            skill: skill.clone(),
        },
        RuntimeCommand::RollbackSkillPatch(rollback) => CommandEffect::RollbackSkillPatch {
            rollback: rollback.clone(),
        },
        RuntimeCommand::AutoPromoteSkills { apply } => {
            CommandEffect::AutoPromoteSkills { apply: *apply }
        }
        RuntimeCommand::ReviewSkills { apply } => CommandEffect::ReviewSkills { apply: *apply },
        RuntimeCommand::PromoteSkill(skill) => CommandEffect::UpdateSkillStatus {
            skill: skill.clone(),
            status: SkillStatus::Active,
        },
        RuntimeCommand::DemoteSkill(skill) => CommandEffect::UpdateSkillStatus {
            skill: skill.clone(),
            status: SkillStatus::Candidate,
        },
        RuntimeCommand::RejectSkill(skill) => CommandEffect::UpdateSkillStatus {
            skill: skill.clone(),
            status: SkillStatus::Deprecated,
        },
        RuntimeCommand::CompactSession => CommandEffect::CompactSession { compacted: false },
        RuntimeCommand::SetModel(raw) => {
            let model = normalize_model_selector(raw);
            let matched =
                resolve_model_command_route_with_alias_resolver(&model, config, alias_resolver);
            match matched {
                Some(route) => CommandEffect::SwitchModel {
                    model: route.model,
                    inferred_provider: Some(route.provider),
                    lane: route.lane,
                    candidate_index: route.candidate_index,
                    compacted: false,
                },
                None => CommandEffect::SwitchModel {
                    model,
                    inferred_provider: None,
                    lane: None,
                    candidate_index: None,
                    compacted: false,
                },
            }
        }
        RuntimeCommand::NewSession => CommandEffect::ClearSession,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModelCommandRoute {
    pub provider: String,
    pub model: String,
    pub lane: Option<CapabilityLane>,
    pub candidate_index: Option<usize>,
}

pub fn resolve_model_command_route(
    selector: &str,
    config: &Config,
) -> Option<ResolvedModelCommandRoute> {
    resolve_model_command_route_with_alias_resolver(selector, config, |value| {
        crate::config::model_catalog::route_alias(value).map(|route| ResolvedModelCommandRoute {
            provider: route.provider,
            model: route.model,
            lane: route.capability,
            candidate_index: None,
        })
    })
}

fn resolve_model_command_route_with_alias_resolver(
    selector: &str,
    config: &Config,
    alias_resolver: impl Fn(&str) -> Option<ResolvedModelCommandRoute>,
) -> Option<ResolvedModelCommandRoute> {
    let selector = normalize_model_selector(selector);
    if selector.is_empty() {
        return None;
    }

    let effective_lanes = resolve_effective_model_lanes(config);
    if let Ok(requested_lane) = selector.parse::<CapabilityLane>() {
        if let Some((lane, candidate)) = effective_lanes
            .iter()
            .find(|lane| lane.lane == requested_lane)
            .and_then(|lane| {
                lane.candidates
                    .first()
                    .map(|candidate| (lane.lane, candidate))
            })
        {
            return Some(ResolvedModelCommandRoute {
                provider: candidate.provider.clone(),
                model: candidate.model.clone(),
                lane: Some(lane),
                candidate_index: Some(0),
            });
        }
    }

    for lane in &effective_lanes {
        if let Some((index, candidate)) = lane
            .candidates
            .iter()
            .enumerate()
            .find(|(_, candidate)| model_selector_matches_candidate(&selector, candidate))
        {
            return Some(ResolvedModelCommandRoute {
                provider: candidate.provider.clone(),
                model: candidate.model.clone(),
                lane: Some(lane.lane),
                candidate_index: Some(index),
            });
        }
    }

    alias_resolver(&selector)
}

fn model_selector_matches_candidate(
    selector: &str,
    candidate: &crate::config::schema::ModelLaneCandidateConfig,
) -> bool {
    candidate.model.eq_ignore_ascii_case(selector)
        || provider_model_selector(candidate.provider.as_str(), candidate.model.as_str())
            .eq_ignore_ascii_case(selector)
}

fn provider_model_selector(provider: &str, model: &str) -> String {
    format!("{}:{}", provider.trim(), model.trim())
}

fn normalize_model_selector(value: &str) -> String {
    value.trim().trim_matches('`').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        ModelCandidateProfileConfig, ModelLaneCandidateConfig, ModelLaneConfig,
    };
    use crate::domain::channel::SourceKind;

    const TEST_PROVIDER: &str = "test-provider";
    const TEST_MAIN_MODEL: &str = "test-main-model";
    const TEST_VISION_PROVIDER: &str = "test-vision-provider";
    const TEST_VISION_MODEL: &str = "test-vision-model";
    const TEST_ALIAS: &str = "test-alias";
    const TEST_ALIAS_PROVIDER: &str = "test-alias-provider";
    const TEST_ALIAS_MODEL: &str = "test-alias-model";

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
        let cmd = parse_runtime_command("/models test-provider", &caps_with_runtime());
        assert_eq!(
            cmd,
            Some(RuntimeCommand::SetProvider("test-provider".into()))
        );
    }

    #[test]
    fn parse_model_show() {
        let cmd = parse_runtime_command("/model", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowModel));
    }

    #[test]
    fn parse_model_set() {
        let cmd = parse_runtime_command("/model test-model", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::SetModel("test-model".into())));
    }

    #[test]
    fn parse_doctor_show() {
        let cmd = parse_runtime_command("/doctor", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowDoctor));
    }

    #[test]
    fn parse_skills_status() {
        let cmd = parse_runtime_command("/skills status", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ShowSkills));
    }

    #[test]
    fn parse_skills_blocked_and_candidates() {
        assert_eq!(
            parse_runtime_command("/skills blocked", &caps_with_runtime()),
            Some(RuntimeCommand::ShowBlockedSkills)
        );
        assert_eq!(
            parse_runtime_command("/skills candidates", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillCandidates)
        );
    }

    #[test]
    fn parse_skills_tools() {
        assert_eq!(
            parse_runtime_command("/skills tools", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillTools)
        );
        assert_eq!(
            parse_runtime_command("/skills replay-tools", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillTools)
        );
    }

    #[test]
    fn parse_skills_traces() {
        assert_eq!(
            parse_runtime_command("/skills traces", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillTraces)
        );
        assert_eq!(
            parse_runtime_command("/skills usage", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillTraces)
        );
    }

    #[test]
    fn parse_skills_health() {
        assert_eq!(
            parse_runtime_command("/skills health", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillHealth { apply: false })
        );
        assert_eq!(
            parse_runtime_command("/skills cleanup", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillHealth { apply: false })
        );
        assert_eq!(
            parse_runtime_command("/skills health --apply", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillHealth { apply: true })
        );
    }

    #[test]
    fn parse_skills_diff_with_spaced_candidate_ref() {
        assert_eq!(
            parse_runtime_command("/skills diff patch Matrix Upgrade", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillDiff("patch Matrix Upgrade".into()))
        );
    }

    #[test]
    fn parse_skills_apply_with_spaced_candidate_ref() {
        assert_eq!(
            parse_runtime_command("/skills apply patch Matrix Upgrade", &caps_with_runtime()),
            Some(RuntimeCommand::ApplySkillPatch(
                "patch Matrix Upgrade".into()
            ))
        );
    }

    #[test]
    fn parse_skills_versions_and_rollback_with_spaced_refs() {
        assert_eq!(
            parse_runtime_command("/skills versions Matrix Upgrade", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillVersions(Some(
                "Matrix Upgrade".into()
            )))
        );
        assert_eq!(
            parse_runtime_command("/skills versions", &caps_with_runtime()),
            Some(RuntimeCommand::ShowSkillVersions(None))
        );
        assert_eq!(
            parse_runtime_command(
                "/skills rollback apply Matrix Upgrade",
                &caps_with_runtime()
            ),
            Some(RuntimeCommand::RollbackSkillPatch(
                "apply Matrix Upgrade".into()
            ))
        );
    }

    #[test]
    fn parse_skills_autopromote_apply() {
        assert_eq!(
            parse_runtime_command("/skills autopromote", &caps_with_runtime()),
            Some(RuntimeCommand::AutoPromoteSkills { apply: false })
        );
        assert_eq!(
            parse_runtime_command("/skills auto-promote --apply", &caps_with_runtime()),
            Some(RuntimeCommand::AutoPromoteSkills { apply: true })
        );
    }

    #[test]
    fn parse_skills_review_apply() {
        let cmd = parse_runtime_command("/skills review --apply", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::ReviewSkills { apply: true }));
    }

    #[test]
    fn parse_skills_create_user_skill() {
        let cmd = parse_runtime_command(
            "/skills create Matrix release check :: Find local repo and compare tags",
            &caps_with_runtime(),
        );
        assert_eq!(
            cmd,
            Some(RuntimeCommand::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "Find local repo and compare tags".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            })
        );
    }

    #[test]
    fn parse_skills_create_user_skill_metadata() {
        let cmd = parse_runtime_command(
            "/skills create Matrix release check --task-family=release-audit --tools=repo_discovery,git_operations --tags=matrix,release :: Find local repo and compare tags",
            &caps_with_runtime(),
        );
        assert_eq!(
            cmd,
            Some(RuntimeCommand::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "Find local repo and compare tags".into(),
                metadata: RuntimeUserSkillCreateMetadata {
                    task_family: Some("release-audit".into()),
                    tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                    tags: vec!["matrix".into(), "release".into()],
                },
            })
        );
    }

    #[test]
    fn parse_skills_create_preserves_markdown_body() {
        let cmd = parse_runtime_command(
            "/skills create Matrix release check :: # Matrix release check\n\n1. Find local repo.\n2. Compare tags.",
            &caps_with_runtime(),
        );
        assert_eq!(
            cmd,
            Some(RuntimeCommand::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "# Matrix release check\n\n1. Find local repo.\n2. Compare tags.".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            })
        );
    }

    #[test]
    fn parse_skills_update_user_skill() {
        let cmd = parse_runtime_command(
            "/skills update Matrix release check --task-family=release-audit --tools=repo_discovery,git_operations --tags=matrix :: Updated procedure",
            &caps_with_runtime(),
        );
        assert_eq!(
            cmd,
            Some(RuntimeCommand::UpdateUserSkill {
                skill: "Matrix release check".into(),
                body: "Updated procedure".into(),
                metadata: RuntimeUserSkillCreateMetadata {
                    task_family: Some("release-audit".into()),
                    tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                    tags: vec!["matrix".into()],
                },
            })
        );
    }

    #[test]
    fn parse_skills_create_requires_explicit_body_delimiter() {
        assert_eq!(
            parse_runtime_command(
                "/skills create Matrix release check Find local repo",
                &caps_with_runtime()
            ),
            None
        );
    }

    #[test]
    fn parse_skills_status_update_with_spaced_name() {
        let cmd = parse_runtime_command(
            "/skills promote Matrix Upgrade Recipe",
            &caps_with_runtime(),
        );
        assert_eq!(
            cmd,
            Some(RuntimeCommand::PromoteSkill("Matrix Upgrade Recipe".into()))
        );
    }

    #[test]
    fn parse_compact_session() {
        let cmd = parse_runtime_command("/compact", &caps_with_runtime());
        assert_eq!(cmd, Some(RuntimeCommand::CompactSession));
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
            conversation_id: String::new(),
            event_ref: None,
            reply_ref: String::new(),
            thread_ref: None,
            media_attachments: Vec::new(),
            content: String::new(),
            received_at: 0,
        };
        assert_eq!(
            conversation_key_for_agent(&env, "test-agent"),
            "conversation:test-agent:telegram:user123:user123"
        );
    }

    #[test]
    fn key_with_thread() {
        let env = InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "slack".into(),
            actor_id: "user456".into(),
            conversation_id: String::new(),
            event_ref: None,
            reply_ref: String::new(),
            thread_ref: Some("thread789".into()),
            media_attachments: Vec::new(),
            content: String::new(),
            received_at: 0,
        };
        assert_eq!(
            conversation_key_for_agent(&env, "test-agent"),
            "conversation:test-agent:slack:user456:thread789:user456"
        );
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

    #[test]
    fn consolidation_uses_same_gate_as_autosave() {
        assert!(!should_consolidate_memory(true, "/model cheap"));
        assert!(!should_consolidate_memory(true, "[GENERATE:IMAGE] poster"));
        assert!(should_consolidate_memory(
            true,
            "Это достаточно длинное и осмысленное сообщение для consolidation."
        ));
    }

    // ── history enrichment tests ─────────────────────────────────

    fn envelope(thread: Option<&str>) -> InboundEnvelope {
        InboundEnvelope {
            source_kind: SourceKind::Channel,
            source_adapter: "telegram".into(),
            actor_id: "user1".into(),
            conversation_id: String::new(),
            event_ref: None,
            reply_ref: String::new(),
            thread_ref: thread.map(String::from),
            media_attachments: Vec::new(),
            content: String::new(),
            received_at: 0,
        }
    }

    #[test]
    fn provider_facing_content_appends_typed_media_markers_once() {
        let attachments = vec![crate::domain::channel::InboundMediaAttachment::new(
            crate::domain::channel::InboundMediaKind::Audio,
            "file:///tmp/voice.ogg",
        )];

        assert_eq!(
            provider_facing_content("listen", &attachments),
            "listen\n[AUDIO:file:///tmp/voice.ogg]"
        );
        assert_eq!(
            provider_facing_content("listen\n[AUDIO:file:///tmp/voice.ogg]", &attachments),
            "listen\n[AUDIO:file:///tmp/voice.ogg]"
        );
    }

    #[test]
    fn autosave_key_prefers_event_ref() {
        let mut env = envelope(None);
        env.event_ref = Some("telegram_123_456".into());
        assert_eq!(
            autosave_memory_key_for_agent(&env, "test-agent"),
            "channel:conversation:test-agent:telegram:user1:user1:telegram_123_456"
        );
    }

    #[test]
    fn autosave_key_falls_back_to_received_at_and_length() {
        let mut env = envelope(None);
        env.content = "hello there".into();
        env.received_at = 42;
        assert_eq!(
            autosave_memory_key_for_agent(&env, "test-agent"),
            "channel:conversation:test-agent:telegram:user1:user1:recv42:len11"
        );
    }

    #[test]
    fn enrichment_core_blocks_only_when_has_history() {
        assert_eq!(
            decide_history_enrichment_for_agent(true, &envelope(None), "test-agent"),
            HistoryEnrichment::Continuation
        );
    }

    #[test]
    fn enrichment_thread_seeding_for_threaded_first_message() {
        let env = envelope(Some("thread123"));
        let result = decide_history_enrichment_for_agent(false, &env, "test-agent");
        assert_eq!(
            result,
            HistoryEnrichment::ThreadSeeding {
                parent_key: "conversation:test-agent:telegram:user1:user1".into(),
                thread_id: "thread123".into(),
            }
        );
    }

    #[test]
    fn enrichment_memory_for_first_standalone_message() {
        let env = envelope(None);
        let result = decide_history_enrichment_for_agent(false, &env, "test-agent");
        assert_eq!(
            result,
            HistoryEnrichment::MemoryContext {
                conversation_key: "conversation:test-agent:telegram:user1:user1".into(),
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

    #[test]
    fn autosave_skips_structured_control_turns() {
        assert!(!should_autosave(true, "/model cheap"));
        assert!(!should_autosave(true, "[GENERATE:IMAGE] album cover"));
    }

    #[test]
    fn autosave_skips_low_information_repetition() {
        assert!(!should_autosave(
            true,
            "echo echo echo echo echo echo echo echo echo echo echo echo echo"
        ));
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

    fn command_test_config() -> Config {
        let mut config = Config::default();
        config.model_lanes = vec![
            ModelLaneConfig {
                lane: CapabilityLane::Reasoning,
                candidates: vec![ModelLaneCandidateConfig {
                    provider: TEST_PROVIDER.into(),
                    model: TEST_MAIN_MODEL.into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig::default(),
                }],
            },
            ModelLaneConfig {
                lane: CapabilityLane::MultimodalUnderstanding,
                candidates: vec![ModelLaneCandidateConfig {
                    provider: TEST_VISION_PROVIDER.into(),
                    model: TEST_VISION_MODEL.into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: ModelCandidateProfileConfig::default(),
                }],
            },
        ];
        config
    }

    fn test_alias_resolver(value: &str) -> Option<ResolvedModelCommandRoute> {
        value
            .eq_ignore_ascii_case(TEST_ALIAS)
            .then(|| ResolvedModelCommandRoute {
                provider: TEST_ALIAS_PROVIDER.into(),
                model: TEST_ALIAS_MODEL.into(),
                lane: Some(CapabilityLane::CheapReasoning),
                candidate_index: None,
            })
    }

    fn command_effect_for_test(command: &RuntimeCommand) -> CommandEffect {
        command_effect_with_alias_resolver(command, &command_test_config(), test_alias_resolver)
    }

    #[test]
    fn effect_show_providers() {
        let cmd = RuntimeCommand::ShowProviders;
        assert_eq!(command_effect_for_test(&cmd), CommandEffect::ShowProviders);
    }

    #[test]
    fn effect_switch_provider() {
        let cmd = RuntimeCommand::SetProvider(TEST_PROVIDER.into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchProvider {
                provider: TEST_PROVIDER.into()
            }
        );
    }

    #[test]
    fn effect_show_model() {
        let cmd = RuntimeCommand::ShowModel;
        assert_eq!(command_effect_for_test(&cmd), CommandEffect::ShowModel);
    }

    #[test]
    fn effect_show_doctor() {
        let cmd = RuntimeCommand::ShowDoctor;
        assert_eq!(command_effect_for_test(&cmd), CommandEffect::ShowDoctor);
    }

    #[test]
    fn effect_show_skills_status_views() {
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkills),
            CommandEffect::ShowSkills {
                view: RuntimeSkillStatusView::All
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowBlockedSkills),
            CommandEffect::ShowSkills {
                view: RuntimeSkillStatusView::Blocked
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkillCandidates),
            CommandEffect::ShowSkills {
                view: RuntimeSkillStatusView::Candidates
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkillTools),
            CommandEffect::ShowSkillTools
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkillTraces),
            CommandEffect::ShowSkillTraces
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkillHealth { apply: true }),
            CommandEffect::ShowSkillHealth { apply: true }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkillDiff("patch-a".into())),
            CommandEffect::ShowSkillDiff {
                candidate: "patch-a".into()
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ApplySkillPatch("patch-a".into())),
            CommandEffect::ApplySkillPatch {
                candidate: "patch-a".into()
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ShowSkillVersions(Some("skill-a".into()))),
            CommandEffect::ShowSkillVersions {
                skill: Some("skill-a".into())
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::RollbackSkillPatch("apply-a".into())),
            CommandEffect::RollbackSkillPatch {
                rollback: "apply-a".into()
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::AutoPromoteSkills { apply: true }),
            CommandEffect::AutoPromoteSkills { apply: true }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "Find local repo and compare tags".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            }),
            CommandEffect::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "Find local repo and compare tags".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::UpdateUserSkill {
                skill: "Matrix release check".into(),
                body: "Updated procedure".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            }),
            CommandEffect::UpdateUserSkill {
                skill: "Matrix release check".into(),
                body: "Updated procedure".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            }
        );
    }

    #[test]
    fn effect_skills_review_and_status_updates() {
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::ReviewSkills { apply: true }),
            CommandEffect::ReviewSkills { apply: true }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::PromoteSkill("skill-a".into())),
            CommandEffect::UpdateSkillStatus {
                skill: "skill-a".into(),
                status: SkillStatus::Active
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::DemoteSkill("skill-a".into())),
            CommandEffect::UpdateSkillStatus {
                skill: "skill-a".into(),
                status: SkillStatus::Candidate
            }
        );
        assert_eq!(
            command_effect_for_test(&RuntimeCommand::RejectSkill("skill-a".into())),
            CommandEffect::UpdateSkillStatus {
                skill: "skill-a".into(),
                status: SkillStatus::Deprecated
            }
        );
    }

    #[test]
    fn effect_compact_session() {
        let cmd = RuntimeCommand::CompactSession;
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::CompactSession { compacted: false }
        );
    }

    #[test]
    fn effect_switch_model_by_lane_selector() {
        let cmd = RuntimeCommand::SetModel("reasoning".into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchModel {
                model: TEST_MAIN_MODEL.into(),
                inferred_provider: Some(TEST_PROVIDER.into()),
                lane: Some(CapabilityLane::Reasoning),
                candidate_index: Some(0),
                compacted: false,
            }
        );
    }

    #[test]
    fn effect_switch_model_by_name() {
        let cmd = RuntimeCommand::SetModel(TEST_MAIN_MODEL.into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchModel {
                model: TEST_MAIN_MODEL.into(),
                inferred_provider: Some(TEST_PROVIDER.into()),
                lane: Some(CapabilityLane::Reasoning),
                candidate_index: Some(0),
                compacted: false,
            }
        );
    }

    #[test]
    fn effect_switch_model_unknown() {
        let cmd = RuntimeCommand::SetModel("custom-model".into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchModel {
                model: "custom-model".into(),
                inferred_provider: None,
                lane: None,
                candidate_index: None,
                compacted: false,
            }
        );
    }

    #[test]
    fn effect_switch_model_by_catalog_alias_when_lane_missing() {
        let cmd = RuntimeCommand::SetModel(TEST_ALIAS.into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchModel {
                model: TEST_ALIAS_MODEL.into(),
                inferred_provider: Some(TEST_ALIAS_PROVIDER.into()),
                lane: Some(CapabilityLane::CheapReasoning),
                candidate_index: None,
                compacted: false,
            }
        );
    }

    #[test]
    fn effect_prefers_lane_candidate_over_catalog_alias() {
        let cmd = RuntimeCommand::SetModel(TEST_MAIN_MODEL.into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchModel {
                model: TEST_MAIN_MODEL.into(),
                inferred_provider: Some(TEST_PROVIDER.into()),
                lane: Some(CapabilityLane::Reasoning),
                candidate_index: Some(0),
                compacted: false,
            }
        );
    }

    #[test]
    fn effect_switch_model_preserves_lane_candidate() {
        let cmd = RuntimeCommand::SetModel("multimodal_understanding".into());
        assert_eq!(
            command_effect_for_test(&cmd),
            CommandEffect::SwitchModel {
                model: TEST_VISION_MODEL.into(),
                inferred_provider: Some(TEST_VISION_PROVIDER.into()),
                lane: Some(CapabilityLane::MultimodalUnderstanding),
                candidate_index: Some(0),
                compacted: false,
            }
        );
    }

    #[test]
    fn effect_new_session() {
        let cmd = RuntimeCommand::NewSession;
        assert_eq!(command_effect_for_test(&cmd), CommandEffect::ClearSession);
    }

    #[test]
    fn smart_truncate_takes_last_n_non_system() {
        use crate::domain::message::ChatMessage;
        let turns = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("first"),
            ChatMessage::assistant("reply1"),
            ChatMessage::user("second"),
            ChatMessage::assistant("reply2"),
            ChatMessage::user("third"),
        ];
        let result = smart_truncate_parent_turns(&turns, 3, 5000);
        assert!(result.contains("reply2"));
        assert!(result.contains("second"));
        assert!(result.contains("third"));
        assert!(!result.contains("first"));
        assert!(!result.contains("sys"));
    }

    #[test]
    fn smart_truncate_respects_budget() {
        use crate::domain::message::ChatMessage;
        let turns = vec![
            ChatMessage::user("a".repeat(1000)),
            ChatMessage::assistant("b".repeat(1000)),
            ChatMessage::user("c".repeat(1000)),
        ];
        let result = smart_truncate_parent_turns(&turns, 3, 500);
        // Budget is 500 chars, per-msg limit = 166
        assert!(result.len() <= 600); // some overhead from "user: " prefix
        assert!(result.contains("..."));
    }

    #[test]
    fn smart_truncate_empty_returns_empty() {
        let result = smart_truncate_parent_turns(&[], 3, 2000);
        assert!(result.is_empty());
    }
}
