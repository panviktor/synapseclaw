//! Turn context formatting — adapter-layer prompt string generation.
//!
//! Converts domain `TurnMemoryContext` into prompt-injectable strings.
//! This module owns XML/markdown formatting, string truncation,
//! and prompt-specific layout — keeping the domain layer formatting-free.

use std::fmt::Write;
use synapse_domain::application::services::turn_context::{PromptBudget, TurnMemoryContext};

/// Formatted prompt strings ready for injection.
pub struct FormattedTurnContext {
    /// Core memory blocks for system prompt (highest priority).
    pub core_blocks_system: String,
    /// Recall + skills + entities as ephemeral user-prefix.
    pub enrichment_prefix: String,
}

/// Format `TurnMemoryContext` into prompt-injectable strings.
///
/// - `core_blocks_system`: XML-tagged blocks for system prompt.
/// - `enrichment_prefix`: combined recall/skills/entities for user message prefix.
///
/// Respects `budget.enrichment_total_max_chars` as a hard cap.
pub fn format_turn_context(ctx: &TurnMemoryContext, budget: &PromptBudget) -> FormattedTurnContext {
    // ── Core blocks → system prompt ──
    let mut core_blocks_system = String::new();
    for block in &ctx.core_blocks {
        if block.content.trim().is_empty() {
            continue;
        }
        let _ = writeln!(core_blocks_system, "<{}>", block.label);
        let _ = writeln!(core_blocks_system, "{}", block.content.trim());
        let _ = writeln!(core_blocks_system, "</{}>", block.label);
    }

    // ── Enrichment prefix → user message ──
    let mut enrichment = String::new();
    let max_chars = budget.enrichment_total_max_chars;

    // Recall entries
    if !ctx.recalled_entries.is_empty() {
        let header = "[Memory context]\n";
        // Only add if at least one entry fits within budget (header + entry)
        let mut section = String::from(header);
        let mut added = false;
        for entry in &ctx.recalled_entries {
            let content = truncate_chars(&entry.content, budget.recall_entry_max_chars);
            let line = format!("- {}: {content}\n", entry.key);
            if enrichment.chars().count() + section.chars().count() + line.chars().count()
                > max_chars
            {
                break;
            }
            section.push_str(&line);
            added = true;
        }
        if added {
            section.push('\n');
            enrichment.push_str(&section);
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
        if enrichment.chars().count() + block.chars().count() > max_chars {
            break;
        }
        enrichment.push_str(&block);
    }

    // Entities
    for entity in &ctx.entities {
        if let Some(ref summary) = entity.summary {
            let block = format!(
                "<entity name=\"{}\" type=\"{}\">\n{}\n</entity>\n",
                entity.name, entity.entity_type, summary
            );
            if enrichment.chars().count() + block.chars().count() > max_chars {
                break;
            }
            enrichment.push_str(&block);
        }
    }

    FormattedTurnContext {
        core_blocks_system,
        enrichment_prefix: enrichment,
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

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::memory::{CoreMemoryBlock, MemoryCategory, MemoryEntry};

    #[test]
    fn empty_context_produces_empty_strings() {
        let ctx = TurnMemoryContext::default();
        let budget = PromptBudget::default();
        let fmt = format_turn_context(&ctx, &budget);
        assert!(fmt.core_blocks_system.is_empty());
        assert!(fmt.enrichment_prefix.is_empty());
    }

    #[test]
    fn core_blocks_formatted_as_xml() {
        let ctx = TurnMemoryContext {
            core_blocks: vec![CoreMemoryBlock {
                agent_id: "a".to_string(),
                label: "persona".to_string(),
                content: "I am helpful".to_string(),
                max_tokens: 500,
                updated_at: chrono::Utc::now(),
            }],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.contains("<persona>"));
        assert!(fmt.core_blocks_system.contains("I am helpful"));
        assert!(fmt.core_blocks_system.contains("</persona>"));
    }

    #[test]
    fn recall_entries_formatted() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![MemoryEntry {
                id: String::new(),
                key: "fact1".into(),
                content: "User likes Rust".into(),
                category: MemoryCategory::Core,
                score: Some(0.9),
                timestamp: String::new(),
                session_id: None,
            }],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.contains("[Memory context]"));
        assert!(fmt.enrichment_prefix.contains("fact1"));
        assert!(fmt.enrichment_prefix.contains("User likes Rust"));
    }

    #[test]
    fn enrichment_total_budget_respected() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![MemoryEntry {
                id: String::new(),
                key: "k".into(),
                content: "x".repeat(100),
                category: MemoryCategory::Core,
                score: Some(0.9),
                timestamp: String::new(),
                session_id: None,
            }],
            ..Default::default()
        };
        let budget = PromptBudget {
            enrichment_total_max_chars: 50,
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &budget);
        // The entry is too large for the 50-char budget after the header
        // so it should be cut short
        assert!(fmt.enrichment_prefix.chars().count() <= 55); // header + tolerance
    }
}
