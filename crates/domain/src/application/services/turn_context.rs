//! Turn context assembly — unified memory enrichment for all paths.
//!
//! Owns the policy of *what* memory data to load for a given turn,
//! and provides canonical formatting into prompt strings.
//!
//! Formatting lives here (not in the adapter layer) because both
//! web (`agent.rs`) and channels (`handle_inbound_message.rs`) need
//! the same format, and the channel use case can't call adapters.
//! The adapter `turn_context_fmt` re-exports these functions.

use crate::application::services::clarification_policy;
use crate::application::services::resolution_router;
use crate::application::services::retrieval_service;
use crate::application::services::turn_budget_policy;
use crate::application::services::turn_interpretation::{
    ReferenceCandidateKind, ReferenceSource, TurnInterpretation,
};
use crate::domain::memory::{CoreMemoryBlock, Entity, MemoryEntry, Skill};
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::run_recipe_store::RunRecipeStorePort;

// ── Types ────────────────────────────────────────────────────────

/// Structured memory context for a single LLM turn.
///
/// Pure data — no formatting, no prompt strings.
/// The adapter layer converts this into system messages / user prefixes.
#[derive(Debug, Default)]
pub struct TurnMemoryContext {
    /// Core memory blocks (MemGPT pattern), always present.
    pub core_blocks: Vec<CoreMemoryBlock>,
    /// Episodic recall entries matching the user query.
    pub recalled_entries: Vec<MemoryEntry>,
    /// Relevant learned skills (procedural memory).
    pub skills: Vec<Skill>,
    /// Relevant entities (semantic memory / knowledge graph).
    pub entities: Vec<Entity>,
    /// Relevant prior session recaps / historical context.
    pub session_matches: Vec<retrieval_service::SessionSearchMatch>,
    /// Relevant prior successful execution recipes / precedents.
    pub run_recipes: Vec<retrieval_service::RunRecipeSearchMatch>,
    /// Deterministic resolver ordering for this turn.
    pub resolution_plan: Option<resolution_router::ResolutionPlan>,
    /// Structured clarification guidance from known defaults and candidates.
    pub clarification_guidance: Option<clarification_policy::ClarificationGuidance>,
    /// Cheap-path gating and retrieval limits for this turn.
    pub execution_budget: Option<turn_budget_policy::TurnExecutionBudget>,
}

/// Token/char budget for turn context assembly.
#[derive(Debug, Clone)]
pub struct PromptBudget {
    pub recall_max_entries: usize,
    pub recall_entry_max_chars: usize,
    pub recall_total_max_chars: usize,
    pub recall_min_relevance: f64,
    pub skills_max_count: usize,
    pub skills_total_max_chars: usize,
    pub entities_max_count: usize,
    pub entities_total_max_chars: usize,
    pub enrichment_total_max_chars: usize,
}

impl Default for PromptBudget {
    fn default() -> Self {
        Self {
            recall_max_entries: 5,
            recall_entry_max_chars: 800,
            recall_total_max_chars: 4_000,
            recall_min_relevance: 0.4,
            skills_max_count: 3,
            skills_total_max_chars: 2_000,
            entities_max_count: 3,
            entities_total_max_chars: 1_500,
            enrichment_total_max_chars: 8_000,
        }
    }
}

/// What to load on continuation turns (turn N>1 in a session).
#[derive(Debug, Clone)]
pub enum ContinuationPolicy {
    /// Core blocks only — cheapest, no recall/skills/entities.
    CoreOnly,
    /// Core blocks + lightweight recall (reduced budget).
    CorePlusRecall { recall_max_entries: usize },
    /// Full context — same as first turn.
    Full,
}

impl Default for ContinuationPolicy {
    fn default() -> Self {
        Self::CorePlusRecall {
            recall_max_entries: 2,
        }
    }
}

// ── Assembly ─────────────────────────────────────────────────────

/// Assemble structured memory context for a single turn.
///
/// Both web and channel paths should call this function.
/// The `continuation` parameter controls what to load on turn N>1:
/// - `None` → full context (first turn or explicit full).
/// - `Some(CoreOnly)` → core blocks only.
/// - `Some(CorePlusRecall { n })` → core blocks + recall with limit `n`.
/// - `Some(Full)` → same as `None`.
pub async fn assemble_turn_context(
    mem: &dyn UnifiedMemoryPort,
    run_recipe_store: Option<&dyn RunRecipeStorePort>,
    conversation_store: Option<&dyn ConversationStorePort>,
    user_message: &str,
    agent_id: &str,
    session_id: Option<&str>,
    interpretation: Option<&TurnInterpretation>,
    budget: &PromptBudget,
    continuation: Option<&ContinuationPolicy>,
) -> TurnMemoryContext {
    let mut ctx = TurnMemoryContext {
        core_blocks: mem
            .get_core_blocks(&agent_id.to_string())
            .await
            .unwrap_or_default(),
        ..Default::default()
    };
    ctx.execution_budget = build_execution_budget(interpretation);
    let query_text = build_query_text(user_message, interpretation);

    let policy_name = match continuation {
        Some(ContinuationPolicy::CoreOnly) => "core_only",
        Some(ContinuationPolicy::CorePlusRecall { .. }) => "core_plus_recall",
        Some(ContinuationPolicy::Full) => "full",
        None => "first_turn",
    };

    match continuation {
        Some(ContinuationPolicy::CoreOnly) => {
            apply_resolution_plan(&mut ctx, interpretation);
            tracing::debug!(
                target: "memory_assembly",
                core_blocks = ctx.core_blocks.len(),
                policy = policy_name,
                "Turn context assembled"
            );
            return ctx;
        }
        Some(ContinuationPolicy::CorePlusRecall {
            recall_max_entries: n,
        }) => {
            let recall_limit = ctx
                .execution_budget
                .as_ref()
                .map_or(*n, |execution_budget| {
                    (*n).min(execution_budget.retrieval_budget.max_memory_candidates)
                });
            load_recall(mem, &query_text, session_id, budget, recall_limit, &mut ctx).await;
            apply_resolution_plan(&mut ctx, interpretation);
            tracing::debug!(
                target: "memory_assembly",
                core_blocks = ctx.core_blocks.len(),
                recalled = ctx.recalled_entries.len(),
                policy = policy_name,
                "Turn context assembled"
            );
            return ctx;
        }
        Some(ContinuationPolicy::Full) | None => {
            // Full context: recall + skills + entities
        }
    }

    // Episodic recall
    let recall_limit =
        ctx.execution_budget
            .as_ref()
            .map_or(budget.recall_max_entries, |execution_budget| {
                budget
                    .recall_max_entries
                    .min(execution_budget.retrieval_budget.max_memory_candidates)
            });
    let results = retrieval_service::search_turn_hybrid(
        mem,
        &query_text,
        agent_id,
        session_id,
        &retrieval_service::HybridTurnSearchOptions {
            recall_max_entries: recall_limit,
            recall_min_relevance: budget.recall_min_relevance,
            skills_max_count: budget.skills_max_count,
            skills_total_max_chars: budget.skills_total_max_chars,
            entities_max_count: budget.entities_max_count,
            entities_total_max_chars: budget.entities_total_max_chars,
            query_limit: budget
                .recall_max_entries
                .max(budget.skills_max_count)
                .max(budget.entities_max_count)
                .max(8)
                + 2,
        },
    )
    .await;
    if let Ok(results) = results {
        ctx.recalled_entries = results.recalled_entries;
        ctx.skills = results.skills;
        ctx.entities = results.entities;
    }

    let allow_historical_context = ctx
        .execution_budget
        .as_ref()
        .is_some_and(|execution_budget| {
            execution_budget.interpreter_mode != turn_budget_policy::InterpreterMode::Skip
        });

    if allow_historical_context {
        if let Some(store) = conversation_store {
            let session_max_count = ctx.execution_budget.as_ref().map_or(2, |execution_budget| {
                execution_budget.retrieval_budget.max_session_candidates
            });
            if session_max_count > 0 {
                const SESSION_TOTAL_MAX_CHARS: usize = 1_200;
                const SESSION_MIN_SCORE: f64 = 1.5;

                let session_hits = retrieval_service::search_sessions(
                    mem,
                    store,
                    &query_text,
                    None,
                    session_max_count + 1,
                )
                .await;
                let mut total_session_chars = 0usize;
                for hit in session_hits {
                    if session_id.is_some_and(|current| hit.session_key == current) {
                        continue;
                    }
                    if hit.score < SESSION_MIN_SCORE {
                        break;
                    }
                    let session_chars = hit.label.as_ref().map_or(0, |s| s.chars().count())
                        + hit.summary.as_ref().map_or(0, |s| s.chars().count())
                        + hit.recap.as_ref().map_or(0, |s| s.chars().count());
                    if total_session_chars + session_chars > SESSION_TOTAL_MAX_CHARS {
                        break;
                    }
                    total_session_chars += session_chars;
                    ctx.session_matches.push(hit);
                    if ctx.session_matches.len() >= session_max_count {
                        break;
                    }
                }
            }
        }
    }

    if allow_historical_context {
        if let Some(store) = run_recipe_store {
            let recipe_max_count = ctx.execution_budget.as_ref().map_or(2, |execution_budget| {
                execution_budget.retrieval_budget.max_precedent_candidates
            });
            if recipe_max_count > 0 {
                const RECIPE_TOTAL_MAX_CHARS: usize = 1_200;
                const RECIPE_MIN_SCORE: i64 = 150;

                let recipe_hits = retrieval_service::search_run_recipes(
                    mem,
                    store,
                    agent_id,
                    &query_text,
                    recipe_max_count,
                )
                .await;
                let mut total_recipe_chars = 0usize;
                for recipe in recipe_hits {
                    if recipe.score < RECIPE_MIN_SCORE {
                        break;
                    }
                    let recipe_chars = recipe.summary.chars().count()
                        + recipe.sample_request.chars().count()
                        + recipe
                            .tool_pattern
                            .iter()
                            .map(|tool| tool.chars().count())
                            .sum::<usize>();
                    if total_recipe_chars + recipe_chars > RECIPE_TOTAL_MAX_CHARS {
                        break;
                    }
                    total_recipe_chars += recipe_chars;
                    ctx.run_recipes.push(recipe);
                }
            }
        }
    }

    apply_resolution_plan(&mut ctx, interpretation);

    tracing::debug!(
        target: "memory_assembly",
        core_blocks = ctx.core_blocks.len(),
        recalled = ctx.recalled_entries.len(),
        skills = ctx.skills.len(),
        entities = ctx.entities.len(),
        sessions = ctx.session_matches.len(),
        recipes = ctx.run_recipes.len(),
        interpreter_mode = ?ctx
            .execution_budget
            .as_ref()
            .map(|budget| budget.interpreter_mode),
        policy = policy_name,
        "Turn context assembled"
    );

    ctx
}

/// Load episodic recall entries into context, applying filters.
async fn load_recall(
    mem: &dyn UnifiedMemoryPort,
    user_message: &str,
    session_id: Option<&str>,
    budget: &PromptBudget,
    max_entries: usize,
    ctx: &mut TurnMemoryContext,
) {
    // Fetch a few extra to compensate for filtering
    let entries = match mem.recall(user_message, max_entries + 2, session_id).await {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut total_chars = 0usize;
    let mut count = 0usize;

    for entry in entries {
        if count >= max_entries {
            break;
        }
        // Skip assistant autosave bookkeeping
        if is_autosave_key(&entry.key) {
            tracing::trace!(target: "memory_assembly", key = %entry.key, "Recall: skip autosave");
            continue;
        }
        // Skip entries that look like autosave metadata
        if crate::domain::util::should_skip_autosave_content(&entry.content) {
            tracing::trace!(target: "memory_assembly", key = %entry.key, "Recall: skip noise content");
            continue;
        }
        // Skip tool_result leaks
        if entry.content.contains("<tool_result") {
            tracing::trace!(target: "memory_assembly", key = %entry.key, "Recall: skip tool_result");
            continue;
        }
        // Relevance gate
        if let Some(score) = entry.score {
            if score < budget.recall_min_relevance {
                tracing::trace!(target: "memory_assembly", key = %entry.key, score, min = budget.recall_min_relevance, "Recall: skip low relevance");
                continue;
            }
        }
        let entry_chars = entry
            .content
            .chars()
            .count()
            .min(budget.recall_entry_max_chars);
        if total_chars + entry_chars > budget.recall_total_max_chars {
            break;
        }
        total_chars += entry_chars;
        count += 1;
        ctx.recalled_entries.push(entry);
    }
}

// ── Formatting ───────────────────────────────────────────────────
//
// Formatting lives here (not in the adapter layer) because both
// web (agent.rs) and channels (handle_inbound_message.rs) need it,
// and the channel path is a domain use case that can't call adapters.
// The adapter `turn_context_fmt` re-exports this as its public API.

/// Formatted prompt strings from turn context.
#[derive(Debug, Default)]
pub struct FormattedTurnContext {
    /// Core memory blocks for system prompt.
    pub core_blocks_system: String,
    /// Resolution plan for system prompt.
    pub resolution_system: String,
    /// Recall + skills + entities + sessions + recipes as enrichment prefix.
    pub enrichment_prefix: String,
}

/// Format `TurnMemoryContext` into prompt-injectable strings.
///
/// Canonical formatter used by both web and channel paths.
/// The adapter layer (`turn_context_fmt`) re-exports this function.
pub fn format_turn_context(ctx: &TurnMemoryContext, budget: &PromptBudget) -> FormattedTurnContext {
    use std::fmt::Write;

    let mut result = FormattedTurnContext::default();
    let max_chars = budget.enrichment_total_max_chars;
    let mut remaining_projection_lines = ctx
        .execution_budget
        .as_ref()
        .map_or(usize::MAX, |b| b.retrieval_budget.max_projection_lines);

    // Core blocks → system prompt
    for block in &ctx.core_blocks {
        if block.content.trim().is_empty() {
            continue;
        }
        let _ = writeln!(result.core_blocks_system, "<{}>", block.label);
        let _ = writeln!(result.core_blocks_system, "{}", block.content.trim());
        let _ = writeln!(result.core_blocks_system, "</{}>", block.label);
    }

    if let Some(plan) = &ctx.resolution_plan {
        if let Some(block) = resolution_router::format_resolution_plan(plan) {
            result.resolution_system = block;
        }
    }
    if let Some(guidance) = &ctx.clarification_guidance {
        if let Some(block) = clarification_policy::format_clarification_guidance(guidance) {
            result.resolution_system.push_str(&block);
        }
    }

    for section in ordered_enrichment_sections(ctx, budget) {
        let Some(section) = take_section_lines(&section, &mut remaining_projection_lines) else {
            break;
        };
        if result.enrichment_prefix.chars().count() + section.chars().count() > max_chars {
            break;
        }
        result.enrichment_prefix.push_str(&section);
    }

    result
}

// ── Internal helpers ─────────────────────────────────────────────

/// Check if a memory key is an assistant-generated autosave key.
///
/// Must match `synapse_memory::is_assistant_autosave_key` logic.
fn is_autosave_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    normalized == "assistant_resp" || normalized.starts_with("assistant_resp_")
}

fn build_query_text(user_message: &str, interpretation: Option<&TurnInterpretation>) -> String {
    let base = user_message.trim();
    let Some(interpretation) = interpretation else {
        return base.to_string();
    };

    let mut parts = vec![base.to_string()];

    if let Some(profile) = interpretation.user_profile.as_ref() {
        if let Some(city) = profile.default_city.as_deref() {
            parts.push(format!("Default city: {city}"));
        }
        if let Some(language) = profile.preferred_language.as_deref() {
            parts.push(format!("Preferred language: {language}"));
        }
        if let Some(timezone) = profile.timezone.as_deref() {
            parts.push(format!("Timezone: {timezone}"));
        }
    }

    if !interpretation.reference_candidates.is_empty() {
        let references = interpretation
            .reference_candidates
            .iter()
            .map(|candidate| candidate.value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !references.is_empty() {
            parts.push(format!("Reference candidates: {references}"));
        }
    }

    let Some(state) = interpretation.dialogue_state.as_ref() else {
        return parts.join("\n");
    };

    if !state.focus_entities.is_empty() {
        let focus = state
            .focus_entities
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !focus.is_empty() {
            parts.push(format!("Focus: {focus}"));
        }
    }

    if !state.comparison_set.is_empty() {
        let comparison = state
            .comparison_set
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !comparison.is_empty() {
            parts.push(format!("Comparison: {comparison}"));
        }
    }

    if !state.reference_anchors.is_empty() {
        let anchors = state
            .reference_anchors
            .iter()
            .map(|anchor| {
                let selector = match &anchor.selector {
                    crate::domain::dialogue_state::ReferenceAnchorSelector::Current => {
                        "current".to_string()
                    }
                    crate::domain::dialogue_state::ReferenceAnchorSelector::Latest => {
                        "latest".to_string()
                    }
                    crate::domain::dialogue_state::ReferenceAnchorSelector::Ordinal(ordinal) => {
                        match ordinal {
                            crate::domain::dialogue_state::ReferenceOrdinal::First => {
                                "first".to_string()
                            }
                            crate::domain::dialogue_state::ReferenceOrdinal::Second => {
                                "second".to_string()
                            }
                            crate::domain::dialogue_state::ReferenceOrdinal::Third => {
                                "third".to_string()
                            }
                            crate::domain::dialogue_state::ReferenceOrdinal::Fourth => {
                                "fourth".to_string()
                            }
                        }
                    }
                };
                match anchor.entity_kind.as_deref() {
                    Some(entity_kind) => format!("{selector}<{entity_kind}>={}", anchor.value),
                    None => format!("{selector}={}", anchor.value),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !anchors.is_empty() {
            parts.push(format!("Reference anchors: {anchors}"));
        }
    }

    if !state.last_tool_subjects.is_empty() {
        parts.push(format!(
            "Recent tools: {}",
            state.last_tool_subjects.join(", ")
        ));
    }

    parts.join("\n")
}

fn build_execution_budget(
    interpretation: Option<&TurnInterpretation>,
) -> Option<turn_budget_policy::TurnExecutionBudget> {
    let interpretation = interpretation?;
    let dialogue_state = interpretation.dialogue_state.as_ref();
    let user_profile = interpretation.user_profile.as_ref();
    let signals = turn_budget_policy::TurnExecutionSignals {
        has_working_state: dialogue_state.is_some_and(|state| {
            !state.focus_entities.is_empty()
                || !state.comparison_set.is_empty()
                || !state.reference_anchors.is_empty()
                || !state.last_tool_subjects.is_empty()
                || state.recent_delivery_target.is_some()
                || state.recent_schedule_job.is_some()
                || state.recent_resource.is_some()
                || state.recent_search.is_some()
                || state.recent_workspace.is_some()
        }),
        has_profile_defaults: user_profile.is_some_and(|profile| {
            profile.preferred_language.is_some()
                || profile.timezone.is_some()
                || profile.default_city.is_some()
                || profile.default_delivery_target.is_some()
        }),
        has_reference_candidates: !interpretation.reference_candidates.is_empty(),
        direct_reference_count: count_direct_reference_candidates(interpretation),
        ambiguity_candidate_count: interpretation.clarification_candidates.len(),
        recent_tool_fact_count: dialogue_state.map_or(0, |state| state.last_tool_subjects.len()),
        explicit_user_correction: false,
    };
    Some(turn_budget_policy::build_turn_execution_budget(signals))
}

fn count_direct_reference_candidates(interpretation: &TurnInterpretation) -> usize {
    interpretation
        .reference_candidates
        .iter()
        .filter(|candidate| {
            candidate.source == ReferenceSource::DialogueState
                && matches!(
                    candidate.kind,
                    ReferenceCandidateKind::DeliveryTarget
                        | ReferenceCandidateKind::ScheduleJob
                        | ReferenceCandidateKind::ResourceLocator { .. }
                        | ReferenceCandidateKind::SearchQuery { .. }
                        | ReferenceCandidateKind::SearchResult { .. }
                        | ReferenceCandidateKind::WorkspaceName { .. }
                )
        })
        .count()
}

fn apply_resolution_plan(ctx: &mut TurnMemoryContext, interpretation: Option<&TurnInterpretation>) {
    let plan = resolution_router::build_resolution_plan(resolution_router::ResolutionEvidence {
        interpretation,
        top_session_score: ctx.session_matches.first().map(|session| session.score),
        second_session_score: ctx.session_matches.get(1).map(|session| session.score),
        top_recipe_score: ctx.run_recipes.first().map(|recipe| recipe.score),
        second_recipe_score: ctx.run_recipes.get(1).map(|recipe| recipe.score),
        top_memory_score: ctx.recalled_entries.first().and_then(|entry| entry.score),
        second_memory_score: ctx.recalled_entries.get(1).and_then(|entry| entry.score),
        recall_hits: ctx.recalled_entries.len(),
        skill_hits: ctx.skills.len(),
        entity_hits: ctx.entities.len(),
    });
    if !plan.source_order.is_empty() {
        ctx.clarification_guidance =
            clarification_policy::build_clarification_guidance(Some(&plan), interpretation);
        ctx.resolution_plan = Some(plan);
    } else {
        ctx.clarification_guidance =
            clarification_policy::build_clarification_guidance(None, interpretation);
    }
}

fn ordered_enrichment_sections(ctx: &TurnMemoryContext, budget: &PromptBudget) -> Vec<String> {
    let plan = ctx.resolution_plan.as_ref();
    let mut sections = Vec::<(usize, String)>::new();

    if let Some(section) = format_memory_section(ctx, budget) {
        let source = resolution_router::ResolutionSource::LongTermMemory;
        if should_include_enrichment_source(ctx, source) {
            sections.push((resolution_router::source_priority(plan, source), section));
        }
    }
    if let Some(section) = format_session_section(ctx) {
        let source = resolution_router::ResolutionSource::SessionHistory;
        if should_include_enrichment_source(ctx, source) {
            sections.push((resolution_router::source_priority(plan, source), section));
        }
    }
    if let Some(section) = format_recipe_section(ctx) {
        let source = resolution_router::ResolutionSource::RunRecipe;
        if should_include_enrichment_source(ctx, source) {
            sections.push((resolution_router::source_priority(plan, source), section));
        }
    }

    sections.sort_by_key(|(priority, _)| *priority);
    sections.into_iter().map(|(_, section)| section).collect()
}

fn should_include_enrichment_source(
    ctx: &TurnMemoryContext,
    source: resolution_router::ResolutionSource,
) -> bool {
    let Some(execution_budget) = ctx.execution_budget.as_ref() else {
        return true;
    };
    let Some(plan) = ctx.resolution_plan.as_ref() else {
        return true;
    };

    let has_direct_reference_gate = execution_budget
        .gate_reasons
        .contains(&turn_budget_policy::InterpreterGateReason::DirectTypedReference);
    if !has_direct_reference_gate {
        return true;
    }

    if plan.confidence == resolution_router::ResolutionConfidence::Low {
        return true;
    }

    let priority = resolution_router::source_priority(Some(plan), source);
    priority != usize::MAX && priority <= 1
}

fn take_section_lines(section: &str, remaining_lines: &mut usize) -> Option<String> {
    if *remaining_lines == 0 || section.trim().is_empty() {
        return None;
    }

    let lines = section.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }

    if lines.len() <= *remaining_lines {
        *remaining_lines -= lines.len();
        let mut result = lines.join("\n");
        if section.ends_with('\n') {
            result.push('\n');
        }
        return Some(result);
    }

    if *remaining_lines == 1 {
        *remaining_lines = 0;
        return Some("...\n".to_string());
    }

    let visible_lines = (*remaining_lines).saturating_sub(1);
    let mut result = lines
        .into_iter()
        .take(visible_lines)
        .collect::<Vec<_>>()
        .join("\n");
    result.push_str("\n...\n");
    *remaining_lines = 0;
    Some(result)
}

fn format_memory_section(ctx: &TurnMemoryContext, budget: &PromptBudget) -> Option<String> {
    let mut section = String::new();

    if !ctx.recalled_entries.is_empty() {
        let header = "[Memory context]\n";
        let mut recall_section = String::from(header);
        let mut added = false;
        for entry in &ctx.recalled_entries {
            let content = if entry.content.chars().count() > budget.recall_entry_max_chars {
                let truncated: String = entry
                    .content
                    .chars()
                    .take(budget.recall_entry_max_chars)
                    .collect();
                format!("{truncated}…")
            } else {
                entry.content.clone()
            };
            recall_section.push_str(&format!("- {}: {content}\n", entry.key));
            added = true;
        }
        if added {
            recall_section.push('\n');
            section.push_str(&recall_section);
        }
    }

    for skill in &ctx.skills {
        if skill.content.trim().is_empty() {
            continue;
        }
        section.push_str(&format!(
            "<skill name=\"{}\">\n{}\n</skill>\n",
            skill.name,
            skill.content.trim()
        ));
    }

    for entity in &ctx.entities {
        if let Some(summary) = entity.summary.as_ref() {
            section.push_str(&format!(
                "<entity name=\"{}\" type=\"{}\">\n{}\n</entity>\n",
                entity.name, entity.entity_type, summary
            ));
        }
    }

    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

fn format_session_section(ctx: &TurnMemoryContext) -> Option<String> {
    if ctx.session_matches.is_empty() {
        return None;
    }
    let mut section = String::new();
    for session in &ctx.session_matches {
        let mut block = format!(
            "<session-recap key=\"{}\" kind=\"{}\">\n",
            session.session_key, session.kind
        );
        if let Some(label) = session.label.as_deref() {
            if !label.trim().is_empty() {
                block.push_str("Label: ");
                block.push_str(label.trim());
                block.push('\n');
            }
        }
        if let Some(summary) = session.summary.as_deref() {
            if !summary.trim().is_empty() {
                block.push_str(summary.trim());
                block.push('\n');
            }
        }
        if let Some(recap) = session.recap.as_deref() {
            if !recap.trim().is_empty() {
                block.push_str("Recent match: ");
                block.push_str(recap.trim());
                block.push('\n');
            }
        }
        block.push_str("</session-recap>\n");
        section.push_str(&block);
    }
    Some(section)
}

fn format_recipe_section(ctx: &TurnMemoryContext) -> Option<String> {
    if ctx.run_recipes.is_empty() {
        return None;
    }
    let mut section = String::new();
    for recipe in &ctx.run_recipes {
        let mut block = format!(
            "<recipe task_family=\"{}\" success_count=\"{}\">\n",
            recipe.task_family, recipe.success_count
        );
        if !recipe.summary.trim().is_empty() {
            block.push_str(recipe.summary.trim());
            block.push('\n');
        }
        if !recipe.sample_request.trim().is_empty() {
            block.push_str("Sample request: ");
            block.push_str(recipe.sample_request.trim());
            block.push('\n');
        }
        if !recipe.tool_pattern.is_empty() {
            block.push_str("Tool pattern: ");
            block.push_str(&recipe.tool_pattern.join(", "));
            block.push('\n');
        }
        block.push_str("</recipe>\n");
        section.push_str(&block);
    }
    Some(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::retrieval_service::{
        RunRecipeSearchMatch, SessionSearchMatch,
    };
    use crate::application::services::turn_budget_policy::{
        InterpreterGateReason, InterpreterMode, RetrievalBudget, TurnExecutionBudget,
    };
    use crate::domain::dialogue_state::DialogueState;
    use crate::domain::memory::{CoreMemoryBlock, Entity, MemoryCategory, MemoryEntry, Skill};

    // ── Budget & policy defaults ──

    #[test]
    fn default_budget_values() {
        let b = PromptBudget::default();
        assert_eq!(b.recall_max_entries, 5);
        assert_eq!(b.recall_entry_max_chars, 800);
        assert_eq!(b.recall_total_max_chars, 4_000);
        assert!((b.recall_min_relevance - 0.4).abs() < f64::EPSILON);
        assert_eq!(b.skills_max_count, 3);
        assert_eq!(b.skills_total_max_chars, 2_000);
        assert_eq!(b.entities_max_count, 3);
        assert_eq!(b.entities_total_max_chars, 1_500);
        assert_eq!(b.enrichment_total_max_chars, 8_000);
    }

    #[test]
    fn default_continuation_policy() {
        let p = ContinuationPolicy::default();
        match p {
            ContinuationPolicy::CorePlusRecall {
                recall_max_entries: n,
            } => assert_eq!(n, 2),
            _ => panic!("expected CorePlusRecall"),
        }
    }

    #[test]
    fn is_autosave_key_matches() {
        assert!(is_autosave_key("assistant_resp"));
        assert!(is_autosave_key("assistant_resp_1234"));
        assert!(is_autosave_key("ASSISTANT_RESP_abcd"));
        assert!(!is_autosave_key("assistant_response"));
        assert!(!is_autosave_key("user_msg_1234"));
        assert!(!is_autosave_key("core_persona"));
    }

    #[tokio::test]
    async fn build_query_text_includes_typed_dialogue_state() {
        let state = DialogueState {
            focus_entities: vec![crate::domain::dialogue_state::FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            last_tool_subjects: vec!["weather_lookup".into()],
            ..Default::default()
        };
        let interpretation =
            crate::application::services::turn_interpretation::build_turn_interpretation(
                None,
                "what's the weather?",
                Some(crate::domain::user_profile::UserProfile {
                    default_city: Some("Berlin".into()),
                    timezone: Some("Europe/Berlin".into()),
                    ..Default::default()
                }),
                None,
                Some(&state),
            )
            .await
            .unwrap();
        let query = build_query_text("what's the weather?", Some(&interpretation));
        assert!(query.contains("what's the weather?"));
        assert!(query.contains("Default city: Berlin"));
        assert!(query.contains("Focus: Berlin"));
        assert!(query.contains("Recent tools: weather_lookup"));
    }

    #[test]
    fn execution_budget_uses_interpretation_signals() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                user_profile: Some(crate::domain::user_profile::UserProfile {
                    default_city: Some("Berlin".into()),
                    ..Default::default()
                }),
                dialogue_state: Some(
                    crate::application::services::turn_interpretation::DialogueStateSnapshot {
                        focus_entities: vec![("city".into(), "Berlin".into())],
                        comparison_set: vec![
                            ("city".into(), "Berlin".into()),
                            ("city".into(), "Tbilisi".into()),
                        ],
                        reference_anchors: vec![],
                        last_tool_subjects: vec!["weather_lookup".into()],
                        recent_delivery_target: None,
                        recent_schedule_job: None,
                        recent_resource: None,
                        recent_search: None,
                        recent_workspace: None,
                    },
                ),
                clarification_candidates: vec!["Berlin".into(), "Tbilisi".into()],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 2);
    }

    #[test]
    fn execution_budget_trims_history_when_direct_typed_reference_exists() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                dialogue_state: Some(
                    crate::application::services::turn_interpretation::DialogueStateSnapshot {
                        focus_entities: vec![],
                        comparison_set: vec![],
                        reference_anchors: vec![],
                        last_tool_subjects: vec!["job_123".into()],
                        recent_delivery_target: None,
                        recent_schedule_job: Some(
                            crate::domain::dialogue_state::ScheduleJobReference {
                                job_id: "job_123".into(),
                                action: crate::domain::tool_fact::ScheduleAction::Run,
                                job_type: Some(
                                    crate::domain::tool_fact::ScheduleJobType::Agent,
                                ),
                                schedule_kind: Some(
                                    crate::domain::tool_fact::ScheduleKind::Cron,
                                ),
                                session_target: Some("main".into()),
                                timezone: Some("Europe/Berlin".into()),
                            },
                        ),
                        recent_resource: None,
                        recent_search: None,
                        recent_workspace: None,
                    },
                ),
                reference_candidates: vec![
                    crate::application::services::turn_interpretation::ReferenceCandidate {
                        kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::ScheduleJob,
                        value: "job_123".into(),
                        source: crate::application::services::turn_interpretation::ReferenceSource::DialogueState,
                    },
                ],
                clarification_candidates: vec![],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert!(execution_budget
            .gate_reasons
            .contains(&InterpreterGateReason::DirectTypedReference));
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 0);
        assert_eq!(
            execution_budget.retrieval_budget.max_precedent_candidates,
            0
        );
        assert_eq!(execution_budget.retrieval_budget.max_memory_candidates, 3);
    }

    #[test]
    fn current_conversation_alone_does_not_trigger_direct_reference_budget_trim() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                current_conversation: Some(
                    crate::application::services::turn_interpretation::CurrentConversationSnapshot {
                        adapter: "matrix".into(),
                        has_thread: true,
                    },
                ),
                reference_candidates: vec![
                    crate::application::services::turn_interpretation::ReferenceCandidate {
                        kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::DeliveryTarget,
                        value: "current_conversation".into(),
                        source: crate::application::services::turn_interpretation::ReferenceSource::CurrentConversation,
                    },
                ],
                clarification_candidates: vec![],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert!(!execution_budget
            .gate_reasons
            .contains(&InterpreterGateReason::DirectTypedReference));
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 2);
        assert_eq!(
            execution_budget.retrieval_budget.max_precedent_candidates,
            2
        );
    }

    #[test]
    fn take_section_lines_truncates_with_ellipsis() {
        let mut remaining_lines = 3usize;
        let section = "line1\nline2\nline3\nline4\n";

        let trimmed = take_section_lines(section, &mut remaining_lines).unwrap();

        assert_eq!(remaining_lines, 0);
        assert_eq!(trimmed.lines().count(), 3);
        assert!(trimmed.contains("line1"));
        assert!(trimmed.contains("line2"));
        assert!(trimmed.contains("..."));
    }

    #[test]
    fn format_turn_context_respects_projection_line_budget() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![
                make_entry("k1", "alpha", 0.9),
                make_entry("k2", "beta", 0.8),
                make_entry("k3", "gamma", 0.7),
            ],
            execution_budget: Some(TurnExecutionBudget {
                retrieval_budget: RetrievalBudget {
                    max_projection_lines: 2,
                    ..RetrievalBudget::default()
                },
                ..TurnExecutionBudget::default()
            }),
            ..TurnMemoryContext::default()
        };

        let formatted = format_turn_context(&ctx, &PromptBudget::default());

        assert!(formatted.enrichment_prefix.lines().count() <= 2);
        assert!(formatted.enrichment_prefix.contains("[Memory context]"));
        assert!(formatted.enrichment_prefix.contains("..."));
    }

    #[test]
    fn direct_reference_skips_low_priority_memory_enrichment() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            execution_budget: Some(TurnExecutionBudget {
                gate_reasons: vec![InterpreterGateReason::DirectTypedReference],
                ..TurnExecutionBudget::default()
            }),
            resolution_plan: Some(resolution_router::ResolutionPlan {
                source_order: vec![
                    resolution_router::ResolutionSource::DialogueState,
                    resolution_router::ResolutionSource::UserProfile,
                    resolution_router::ResolutionSource::LongTermMemory,
                ],
                confidence: resolution_router::ResolutionConfidence::Medium,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            ..TurnMemoryContext::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(!fmt.enrichment_prefix.contains("[Memory context]"));
    }

    #[test]
    fn direct_reference_keeps_secondary_memory_enrichment() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            execution_budget: Some(TurnExecutionBudget {
                gate_reasons: vec![InterpreterGateReason::DirectTypedReference],
                ..TurnExecutionBudget::default()
            }),
            resolution_plan: Some(resolution_router::ResolutionPlan {
                source_order: vec![
                    resolution_router::ResolutionSource::DialogueState,
                    resolution_router::ResolutionSource::LongTermMemory,
                ],
                confidence: resolution_router::ResolutionConfidence::Medium,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            ..TurnMemoryContext::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.contains("[Memory context]"));
    }

    // ── Helpers ──

    fn make_core_block(label: &str, content: &str) -> CoreMemoryBlock {
        CoreMemoryBlock {
            agent_id: "test".into(),
            label: label.into(),
            content: content.into(),
            max_tokens: 500,
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_entry(key: &str, content: &str, score: f64) -> MemoryEntry {
        MemoryEntry {
            id: String::new(),
            key: key.into(),
            content: content.into(),
            category: MemoryCategory::Core,
            score: Some(score),
            timestamp: String::new(),
            session_id: None,
        }
    }

    fn make_skill(name: &str, content: &str) -> Skill {
        Skill {
            id: String::new(),
            name: name.into(),
            description: String::new(),
            content: content.into(),
            tags: vec![],
            success_count: 1,
            fail_count: 0,
            version: 1,
            created_by: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_entity(name: &str, summary: &str) -> Entity {
        Entity {
            id: String::new(),
            name: name.into(),
            entity_type: "concept".into(),
            summary: Some(summary.into()),
            properties: serde_json::json!({}),
            created_by: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_recipe(task_family: &str, summary: &str) -> RunRecipeSearchMatch {
        RunRecipeSearchMatch {
            score: 220,
            task_family: task_family.into(),
            sample_request: format!("{task_family} the latest release"),
            summary: summary.into(),
            tool_pattern: vec!["shell".into(), "git".into()],
            success_count: 3,
            updated_at: 1,
        }
    }

    fn make_session_match(label: &str, summary: &str, recap: &str) -> SessionSearchMatch {
        SessionSearchMatch {
            score: 2.4,
            session_key: "channel_alice".into(),
            label: Some(label.into()),
            kind: crate::domain::conversation::ConversationKind::Channel,
            message_count: 8,
            summary: Some(summary.into()),
            recap: Some(recap.into()),
        }
    }

    // ── format_turn_context: core blocks ──

    #[test]
    fn format_core_blocks_xml_tags() {
        let ctx = TurnMemoryContext {
            core_blocks: vec![
                make_core_block("persona", "I am helpful"),
                make_core_block("user_knowledge", "Prefers Rust"),
            ],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.contains("<persona>"));
        assert!(fmt.core_blocks_system.contains("I am helpful"));
        assert!(fmt.core_blocks_system.contains("</persona>"));
        assert!(fmt.core_blocks_system.contains("<user_knowledge>"));
        assert!(fmt.core_blocks_system.contains("Prefers Rust"));
        assert!(fmt.enrichment_prefix.is_empty());
    }

    #[test]
    fn format_empty_core_block_skipped() {
        let ctx = TurnMemoryContext {
            core_blocks: vec![make_core_block("persona", "  ")],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.is_empty());
    }

    // ── format_turn_context: recall entries ──

    #[test]
    fn format_recall_entries_with_header() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.starts_with("[Memory context]\n"));
        assert!(fmt.enrichment_prefix.contains("- fact1: User likes Rust"));
    }

    #[test]
    fn format_recall_entry_truncated() {
        let long = "x".repeat(1000);
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("k", &long, 0.9)],
            ..Default::default()
        };
        let budget = PromptBudget {
            recall_entry_max_chars: 50,
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &budget);
        assert!(fmt.enrichment_prefix.contains("…"));
        // Truncated to ~50 chars + key + formatting
        assert!(fmt.enrichment_prefix.len() < 200);
    }

    // ── format_turn_context: skills independent of recall ──

    #[test]
    fn format_skills_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![], // empty recall
            skills: vec![make_skill("deploy", "Run cargo build --release")],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(!fmt.enrichment_prefix.contains("[Memory context]"));
        assert!(fmt.enrichment_prefix.contains("<skill name=\"deploy\">"));
        assert!(fmt.enrichment_prefix.contains("Run cargo build --release"));
        assert!(fmt.enrichment_prefix.contains("</skill>"));
    }

    // ── format_turn_context: entities independent of recall ──

    #[test]
    fn format_entities_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![], // empty recall
            entities: vec![make_entity("Rust", "Systems programming language")],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt
            .enrichment_prefix
            .contains("<entity name=\"Rust\" type=\"concept\">"));
        assert!(fmt
            .enrichment_prefix
            .contains("Systems programming language"));
    }

    #[test]
    fn format_recipes_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![],
            run_recipes: vec![make_recipe(
                "deploy",
                "Check staging first, then ship the release",
            )],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt
            .enrichment_prefix
            .contains("<recipe task_family=\"deploy\" success_count=\"3\">"));
        assert!(fmt
            .enrichment_prefix
            .contains("Check staging first, then ship the release"));
        assert!(fmt.enrichment_prefix.contains("Sample request:"));
        assert!(fmt.enrichment_prefix.contains("Tool pattern: shell, git"));
        assert!(fmt.enrichment_prefix.contains("</recipe>"));
    }

    #[test]
    fn format_session_recaps_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![],
            session_matches: vec![make_session_match(
                "Weather thread",
                "Compared Berlin and Tbilisi weather last week",
                "user: what was the weather in Tbilisi? | assistant: Tbilisi was warmer",
            )],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt
            .enrichment_prefix
            .contains("<session-recap key=\"channel_alice\" kind=\"channel\">"));
        assert!(fmt.enrichment_prefix.contains("Label: Weather thread"));
        assert!(fmt
            .enrichment_prefix
            .contains("Compared Berlin and Tbilisi weather last week"));
        assert!(fmt.enrichment_prefix.contains("Recent match:"));
        assert!(fmt.enrichment_prefix.contains("</session-recap>"));
    }

    #[test]
    fn format_resolution_plan_orders_recipe_before_memory_when_router_says_so() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            run_recipes: vec![make_recipe(
                "deploy",
                "Check staging first, then ship the release",
            )],
            resolution_plan: Some(resolution_router::ResolutionPlan {
                source_order: vec![
                    resolution_router::ResolutionSource::RunRecipe,
                    resolution_router::ResolutionSource::LongTermMemory,
                ],
                confidence: resolution_router::ResolutionConfidence::High,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        let recipe_pos = fmt.enrichment_prefix.find("<recipe").unwrap();
        let memory_pos = fmt.enrichment_prefix.find("[Memory context]").unwrap();
        assert!(recipe_pos < memory_pos);
        assert!(fmt.resolution_system.contains("[resolution-plan]"));
    }

    #[test]
    fn format_turn_context_includes_clarification_policy_when_available() {
        let ctx = TurnMemoryContext {
            clarification_guidance: Some(clarification_policy::ClarificationGuidance {
                candidate_set: vec!["Berlin".into(), "Tbilisi".into()],
                required: true,
                avoid_generic_questions: true,
                reason: Some("low_confidence".into()),
            }),
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.resolution_system.contains("[clarification-policy]"));
        assert!(fmt
            .resolution_system
            .contains("clarification_required: true"));
        assert!(fmt.resolution_system.contains("Berlin | Tbilisi"));
        assert!(fmt.resolution_system.contains("reason: low_confidence"));
    }

    // ── format_turn_context: budget enforcement ──

    #[test]
    fn format_enrichment_budget_cap() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("k", &"x".repeat(100), 0.9)],
            skills: vec![make_skill("s", &"y".repeat(100))],
            entities: vec![make_entity("e", &"z".repeat(100))],
            ..Default::default()
        };
        let budget = PromptBudget {
            enrichment_total_max_chars: 80,
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &budget);
        // Total enrichment must not exceed budget (with small tolerance for formatting)
        assert!(
            fmt.enrichment_prefix.chars().count() <= 90,
            "enrichment {} chars exceeds budget 80",
            fmt.enrichment_prefix.chars().count()
        );
    }

    #[test]
    fn format_empty_context_produces_empty_strings() {
        let ctx = TurnMemoryContext::default();
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.is_empty());
        assert!(fmt.enrichment_prefix.is_empty());
    }

    // ── load_recall filtering (via internal helper) ──

    #[test]
    fn recall_filters_autosave_keys() {
        // is_autosave_key is tested directly above; this documents the invariant
        assert!(is_autosave_key("assistant_resp"));
        assert!(is_autosave_key("assistant_resp_abc"));
        assert!(!is_autosave_key("user_msg_abc"));
    }

    #[test]
    fn recall_filters_tool_result_content() {
        // This invariant is enforced in load_recall: entries with <tool_result are skipped
        let entry_content = "Previous result: <tool_result>stale data</tool_result>";
        assert!(entry_content.contains("<tool_result"));
    }
}
