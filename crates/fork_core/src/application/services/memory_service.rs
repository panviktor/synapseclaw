//! Memory service — owns memory tier policy and recall formatting.
//!
//! Phase 4.0 Slice 6: extracts memory business logic into fork_core.
//!
//! Business rules this service owns:
//! - autosave policy (when to persist inbound messages)
//! - recall context formatting (relevance filter, truncation, budget)
//! - consolidation policy (when to run LLM-driven extraction)
//! - tier selection (which category for which operation)

use crate::domain::memory::{MemoryCategory, MemoryEntry, RecallConfig};
use crate::ports::memory::MemoryTiersPort;
use anyhow::Result;

/// Minimum message length (chars) to trigger autosave.
pub const AUTOSAVE_MIN_CHARS: usize = 20;

// ── Autosave policy ──────────────────────────────────────────────

/// Decide whether to auto-save an inbound message.
pub fn should_autosave(auto_save_enabled: bool, content: &str) -> bool {
    auto_save_enabled && content.chars().count() >= AUTOSAVE_MIN_CHARS
}

/// Generate an autosave key for an inbound message.
pub fn autosave_key(conversation_key: &str) -> String {
    format!(
        "user_msg_{}",
        &uuid::Uuid::new_v4().to_string()[..8]
    )
}

// ── Recall formatting ────────────────────────────────────────────

/// Build a memory context string from recalled entries.
///
/// Filters by relevance, truncates per entry and total, formats for prompt injection.
pub fn format_recall_context(entries: &[MemoryEntry], config: &RecallConfig) -> String {
    let mut context = String::new();
    let mut included = 0usize;
    let mut used_chars = 0usize;

    for entry in entries
        .iter()
        .filter(|e| e.score.map_or(true, |s| s >= config.min_relevance_score))
    {
        if included >= config.max_entries {
            break;
        }

        let content = if entry.content.chars().count() > config.entry_max_chars {
            let truncated: String = entry.content.chars().take(config.entry_max_chars).collect();
            format!("{truncated}…")
        } else {
            entry.content.clone()
        };

        let line = format!("- {}: {content}\n", entry.key);
        let line_chars = line.chars().count();
        if used_chars + line_chars > config.total_max_chars {
            break;
        }

        if included == 0 {
            context.push_str("[Memory context]\n");
        }
        context.push_str(&line);
        included += 1;
        used_chars += line_chars;
    }

    context
}

/// Recall and format memory context for a conversation turn.
///
/// Uses tier-aware recall: searches long-term memory, formats for prompt injection.
pub async fn recall_context(
    mem: &dyn MemoryTiersPort,
    query: &str,
    session_id: Option<&str>,
    config: &RecallConfig,
) -> String {
    let entries = match mem
        .recall(query, config.max_entries + 2, None, session_id)
        .await
    {
        Ok(e) => e,
        Err(_) => return String::new(),
    };

    format_recall_context(&entries, config)
}

// ── Consolidation policy ─────────────────────────────────────────

/// Decide whether to run memory consolidation after a turn.
pub fn should_consolidate(auto_save_enabled: bool, user_message: &str) -> bool {
    auto_save_enabled && user_message.chars().count() >= AUTOSAVE_MIN_CHARS
}

/// Run memory consolidation (fire-and-forget).
///
/// Delegates to MemoryTiersPort which handles LLM extraction internally.
/// Extracts facts → Core tier, journal → Daily tier.
pub async fn consolidate_turn(
    mem: &dyn MemoryTiersPort,
    user_message: &str,
    assistant_response: &str,
) -> Result<()> {
    mem.consolidate_turn(user_message, assistant_response).await
}

// ── Tier selection ───────────────────────────────────────────────

/// Determine the appropriate category for an autosave entry.
pub fn autosave_category() -> MemoryCategory {
    MemoryCategory::Conversation
}

/// Determine the appropriate category for a consolidation-extracted fact.
pub fn consolidation_fact_category() -> MemoryCategory {
    MemoryCategory::Core
}

/// Determine the appropriate category for a daily journal entry.
pub fn consolidation_journal_category() -> MemoryCategory {
    MemoryCategory::Daily
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_enabled_long_enough() {
        assert!(should_autosave(true, "This message is definitely long enough to save"));
    }

    #[test]
    fn autosave_disabled() {
        assert!(!should_autosave(false, "This message is long enough but disabled"));
    }

    #[test]
    fn autosave_too_short() {
        assert!(!should_autosave(true, "short"));
    }

    #[test]
    fn autosave_key_unique() {
        let k1 = autosave_key("session1");
        let k2 = autosave_key("session1");
        assert_ne!(k1, k2); // UUID-based
    }

    #[test]
    fn format_recall_empty() {
        let entries = vec![];
        let config = RecallConfig::default();
        assert_eq!(format_recall_context(&entries, &config), "");
    }

    #[test]
    fn format_recall_basic() {
        let entries = vec![MemoryEntry {
            key: "fact1".into(),
            content: "The user likes rust".into(),
            category: MemoryCategory::Core,
            score: Some(0.9),
            timestamp: String::new(),
            session_id: None,
        }];
        let config = RecallConfig::default();
        let result = format_recall_context(&entries, &config);
        assert!(result.contains("[Memory context]"));
        assert!(result.contains("fact1"));
        assert!(result.contains("The user likes rust"));
    }

    #[test]
    fn format_recall_filters_low_relevance() {
        let entries = vec![MemoryEntry {
            key: "noise".into(),
            content: "irrelevant".into(),
            category: MemoryCategory::Core,
            score: Some(0.1),
            timestamp: String::new(),
            session_id: None,
        }];
        let config = RecallConfig {
            min_relevance_score: 0.5,
            ..Default::default()
        };
        assert_eq!(format_recall_context(&entries, &config), "");
    }

    #[test]
    fn format_recall_truncates_long_entry() {
        let long_content = "a".repeat(1000);
        let entries = vec![MemoryEntry {
            key: "long".into(),
            content: long_content,
            category: MemoryCategory::Core,
            score: Some(0.9),
            timestamp: String::new(),
            session_id: None,
        }];
        let config = RecallConfig {
            entry_max_chars: 100,
            ..Default::default()
        };
        let result = format_recall_context(&entries, &config);
        // Entry should be truncated
        assert!(result.len() < 200);
        assert!(result.contains("…"));
    }

    #[test]
    fn format_recall_respects_max_entries() {
        let entries: Vec<MemoryEntry> = (0..10)
            .map(|i| MemoryEntry {
                key: format!("fact{i}"),
                content: format!("content {i}"),
                category: MemoryCategory::Core,
                score: Some(0.9),
                timestamp: String::new(),
                session_id: None,
            })
            .collect();
        let config = RecallConfig {
            max_entries: 3,
            ..Default::default()
        };
        let result = format_recall_context(&entries, &config);
        assert!(result.contains("fact0"));
        assert!(result.contains("fact2"));
        assert!(!result.contains("fact3"));
    }

    #[test]
    fn consolidation_policy() {
        assert!(should_consolidate(true, "A sufficiently long user message"));
        assert!(!should_consolidate(false, "A sufficiently long user message"));
        assert!(!should_consolidate(true, "short"));
    }

    #[test]
    fn tier_categories() {
        assert_eq!(autosave_category(), MemoryCategory::Conversation);
        assert_eq!(consolidation_fact_category(), MemoryCategory::Core);
        assert_eq!(consolidation_journal_category(), MemoryCategory::Daily);
    }
}
