//! Turn context formatting — adapter-layer prompt string generation.
//!
//! Thin wrapper over the domain-level `turn_context::format_turn_context`.
//! Exists as an extension point if adapter-specific formatting diverges
//! in the future; today it delegates directly.

pub use synapse_domain::application::services::turn_context::{
    FormattedTurnContext, PromptBudget, TurnMemoryContext,
};

/// Format `TurnMemoryContext` into prompt-injectable strings.
///
/// Delegates to the domain-level formatter. Adapter layer can override
/// this function if web-specific formatting needs diverge from channels.
pub fn format_turn_context(ctx: &TurnMemoryContext, budget: &PromptBudget) -> FormattedTurnContext {
    synapse_domain::application::services::turn_context::format_turn_context(ctx, budget)
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
        // Header "[Memory context]\n" alone is 17 chars; with 50 budget, the 100-char
        // entry doesn't fit → either empty or header-only (no entry added)
        assert!(fmt.enrichment_prefix.chars().count() <= 55);
    }
}
