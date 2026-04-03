//! Turn context assembly — unified memory enrichment for all paths.
//!
//! Owns the policy of *what* memory data to load for a given turn.
//! Returns structured data; formatting into prompt strings is the
//! adapter layer's responsibility (hexagonal boundary).

use crate::domain::memory::{CoreMemoryBlock, Entity, MemoryEntry, MemoryQuery, Skill};
use crate::ports::memory::UnifiedMemoryPort;

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
    user_message: &str,
    agent_id: &str,
    session_id: Option<&str>,
    budget: &PromptBudget,
    continuation: Option<&ContinuationPolicy>,
) -> TurnMemoryContext {
    let mut ctx = TurnMemoryContext::default();

    // Core blocks: always loaded regardless of continuation policy.
    ctx.core_blocks = mem
        .get_core_blocks(&agent_id.to_string())
        .await
        .unwrap_or_default();

    match continuation {
        Some(ContinuationPolicy::CoreOnly) => return ctx,
        Some(ContinuationPolicy::CorePlusRecall {
            recall_max_entries: n,
        }) => {
            load_recall(mem, user_message, session_id, budget, *n, &mut ctx).await;
            return ctx;
        }
        Some(ContinuationPolicy::Full) | None => {
            // Full context: recall + skills + entities
        }
    }

    // Episodic recall
    load_recall(
        mem,
        user_message,
        session_id,
        budget,
        budget.recall_max_entries,
        &mut ctx,
    )
    .await;

    // Skills (independent of recall)
    let query = MemoryQuery {
        text: user_message.to_string(),
        embedding: None,
        agent_id: agent_id.to_string(),
        include_shared: false,
        time_range: None,
        limit: budget.skills_max_count,
    };
    if let Ok(skills) = mem.find_skills(&query).await {
        let mut chars = 0usize;
        for skill in skills {
            if skill.content.trim().is_empty() {
                continue;
            }
            let len = skill.content.chars().count();
            if chars + len > budget.skills_total_max_chars {
                break;
            }
            chars += len;
            ctx.skills.push(skill);
        }
    }

    // Entities (independent of recall)
    let query = MemoryQuery {
        text: user_message.to_string(),
        embedding: None,
        agent_id: agent_id.to_string(),
        include_shared: false,
        time_range: None,
        limit: budget.entities_max_count,
    };
    if let Ok(entities) = mem.search_entities(&query).await {
        let mut chars = 0usize;
        for entity in entities {
            let summary_len = entity
                .summary
                .as_ref()
                .map_or(0, |s| s.chars().count());
            if summary_len == 0 {
                continue;
            }
            if chars + summary_len > budget.entities_total_max_chars {
                break;
            }
            chars += summary_len;
            ctx.entities.push(entity);
        }
    }

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
            continue;
        }
        // Skip entries that look like autosave metadata
        if crate::domain::util::should_skip_autosave_content(&entry.content) {
            continue;
        }
        // Skip tool_result leaks
        if entry.content.contains("<tool_result") {
            continue;
        }
        // Relevance gate
        if let Some(score) = entry.score {
            if score < budget.recall_min_relevance {
                continue;
            }
        }
        let entry_chars = entry.content.chars().count().min(budget.recall_entry_max_chars);
        if total_chars + entry_chars > budget.recall_total_max_chars {
            break;
        }
        total_chars += entry_chars;
        count += 1;
        ctx.recalled_entries.push(entry);
    }
}

// ── Domain-level formatting ──────────────────────────────────────

/// Formatted prompt strings from turn context.
#[derive(Debug, Default)]
pub struct FormattedTurnContext {
    /// Core memory blocks for system prompt.
    pub core_blocks_system: String,
    /// Recall + skills + entities as enrichment prefix.
    pub enrichment_prefix: String,
}

/// Format `TurnMemoryContext` into prompt-injectable strings.
///
/// Domain-level formatting — both web and channel paths can use this.
/// The adapter layer (`turn_context_fmt`) can override with richer formatting
/// if needed, but this function provides the canonical format.
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
                let truncated: String = entry.content.chars().take(budget.recall_entry_max_chars).collect();
                format!("{truncated}…")
            } else {
                entry.content.clone()
            };
            let line = format!("- {}: {content}\n", entry.key);
            if result.enrichment_prefix.chars().count() + section.chars().count() + line.chars().count() > max_chars {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
