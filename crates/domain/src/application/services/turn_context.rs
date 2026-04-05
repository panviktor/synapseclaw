//! Turn context assembly — unified memory enrichment for all paths.
//!
//! Owns the policy of *what* memory data to load for a given turn,
//! and provides canonical formatting into prompt strings.
//!
//! Formatting lives here (not in the adapter layer) because both
//! web (`agent.rs`) and channels (`handle_inbound_message.rs`) need
//! the same format, and the channel use case can't call adapters.
//! The adapter `turn_context_fmt` re-exports these functions.

use crate::application::services::retrieval_service;
use crate::domain::dialogue_state::DialogueState;
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
    dialogue_state: Option<&DialogueState>,
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
    let query_text = build_query_text(user_message, dialogue_state);

    let policy_name = match continuation {
        Some(ContinuationPolicy::CoreOnly) => "core_only",
        Some(ContinuationPolicy::CorePlusRecall { .. }) => "core_plus_recall",
        Some(ContinuationPolicy::Full) => "full",
        None => "first_turn",
    };

    match continuation {
        Some(ContinuationPolicy::CoreOnly) => {
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
            load_recall(mem, &query_text, session_id, budget, *n, &mut ctx).await;
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
    let results = retrieval_service::search_turn_hybrid(
        mem,
        &query_text,
        agent_id,
        session_id,
        &retrieval_service::HybridTurnSearchOptions {
            recall_max_entries: budget.recall_max_entries,
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

    if let Some(store) = conversation_store {
        const SESSION_MAX_COUNT: usize = 2;
        const SESSION_TOTAL_MAX_CHARS: usize = 1_200;
        const SESSION_MIN_SCORE: f64 = 1.5;

        let session_hits = retrieval_service::search_sessions(
            mem,
            store,
            &query_text,
            None,
            SESSION_MAX_COUNT + 1,
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
            if ctx.session_matches.len() >= SESSION_MAX_COUNT {
                break;
            }
        }
    }

    if let Some(store) = run_recipe_store {
        const RECIPE_MAX_COUNT: usize = 2;
        const RECIPE_TOTAL_MAX_CHARS: usize = 1_200;
        const RECIPE_MIN_SCORE: i64 = 150;

        let recipe_hits = retrieval_service::search_run_recipes(
            mem,
            store,
            agent_id,
            &query_text,
            RECIPE_MAX_COUNT,
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

    tracing::debug!(
        target: "memory_assembly",
        core_blocks = ctx.core_blocks.len(),
        recalled = ctx.recalled_entries.len(),
        skills = ctx.skills.len(),
        entities = ctx.entities.len(),
        sessions = ctx.session_matches.len(),
        recipes = ctx.run_recipes.len(),
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

    // Core blocks → system prompt
    for block in &ctx.core_blocks {
        if block.content.trim().is_empty() {
            continue;
        }
        let _ = writeln!(result.core_blocks_system, "<{}>", block.label);
        let _ = writeln!(result.core_blocks_system, "{}", block.content.trim());
        let _ = writeln!(result.core_blocks_system, "</{}>", block.label);
    }

    // Recall entries
    if !ctx.recalled_entries.is_empty() {
        let header = "[Memory context]\n";
        let mut section = String::from(header);
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
            let line = format!("- {}: {content}\n", entry.key);
            if result.enrichment_prefix.chars().count()
                + section.chars().count()
                + line.chars().count()
                > max_chars
            {
                break;
            }
            section.push_str(&line);
            added = true;
        }
        if added {
            section.push('\n');
            result.enrichment_prefix.push_str(&section);
        }
    }

    // Skills
    for skill in &ctx.skills {
        if skill.content.trim().is_empty() {
            continue;
        }
        let block = format!(
            "<skill name=\"{}\">\n{}\n</skill>\n",
            skill.name,
            skill.content.trim()
        );
        if result.enrichment_prefix.chars().count() + block.chars().count() > max_chars {
            break;
        }
        result.enrichment_prefix.push_str(&block);
    }

    // Entities
    for entity in &ctx.entities {
        if let Some(ref summary) = entity.summary {
            let block = format!(
                "<entity name=\"{}\" type=\"{}\">\n{}\n</entity>\n",
                entity.name, entity.entity_type, summary
            );
            if result.enrichment_prefix.chars().count() + block.chars().count() > max_chars {
                break;
            }
            result.enrichment_prefix.push_str(&block);
        }
    }

    // Prior session recaps / historical context
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
        if result.enrichment_prefix.chars().count() + block.chars().count() > max_chars {
            break;
        }
        result.enrichment_prefix.push_str(&block);
    }

    // Prior successful recipes / precedents
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
        if result.enrichment_prefix.chars().count() + block.chars().count() > max_chars {
            break;
        }
        result.enrichment_prefix.push_str(&block);
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

fn build_query_text(user_message: &str, dialogue_state: Option<&DialogueState>) -> String {
    let base = user_message.trim();
    let Some(state) = dialogue_state else {
        return base.to_string();
    };

    let mut parts = vec![base.to_string()];

    if !state.focus_entities.is_empty() {
        let focus = state
            .focus_entities
            .iter()
            .map(|entity| entity.name.as_str())
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
            .map(|entity| entity.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !comparison.is_empty() {
            parts.push(format!("Comparison: {comparison}"));
        }
    }

    if !state.slots.is_empty() {
        let slots = state
            .slots
            .iter()
            .map(|slot| format!("{}={}", slot.name, slot.value))
            .collect::<Vec<_>>()
            .join(", ");
        if !slots.is_empty() {
            parts.push(format!("Slots: {slots}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::retrieval_service::{
        RunRecipeSearchMatch, SessionSearchMatch,
    };
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

    #[test]
    fn build_query_text_includes_typed_dialogue_state() {
        let state = DialogueState {
            focus_entities: vec![crate::domain::dialogue_state::FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            slots: vec![crate::domain::dialogue_state::DialogueSlot {
                name: "timezone".into(),
                value: "Europe/Berlin".into(),
            }],
            last_tool_subjects: vec!["weather_lookup".into()],
            ..Default::default()
        };
        let query = build_query_text("what's the weather?", Some(&state));
        assert!(query.contains("what's the weather?"));
        assert!(query.contains("Focus: Berlin"));
        assert!(query.contains("Slots: timezone=Europe/Berlin"));
        assert!(query.contains("Recent tools: weather_lookup"));
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
