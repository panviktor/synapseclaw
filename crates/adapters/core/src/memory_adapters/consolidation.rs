//! LLM-driven memory consolidation.
//!
//! After each conversation turn, extracts structured information:
//! - `history_entry`: A timestamped summary for the daily conversation log.
//! - `memory_update`: New facts, preferences, or decisions worth remembering
//!   long-term (or `null` if nothing new was learned).
//!
//! This two-phase approach replaces the naive raw-message auto-save with
//! semantic extraction, similar to Nanobot's `save_memory` tool call pattern.

use super::entity_extractor;
use synapse_memory::{MemoryCategory, UnifiedMemoryPort};
use synapse_providers::traits::Provider;

/// Output of consolidation extraction.
#[derive(Debug, serde::Deserialize)]
pub struct ConsolidationResult {
    /// Brief timestamped summary for the conversation history log.
    pub history_entry: String,
    /// New facts/preferences/decisions to store long-term, or None.
    pub memory_update: Option<String>,
}

/// Summary of what consolidation produced (returned to caller for IPC broadcast).
#[derive(Debug, Default)]
pub struct ConsolidationOutcome {
    pub entities_extracted: usize,
}

const CONSOLIDATION_SYSTEM_PROMPT: &str = r#"You are a memory consolidation engine. Given a conversation turn, extract:
1. "history_entry": A brief summary of what happened in this turn (1-2 sentences). Include the key topic or action.
2. "memory_update": Any NEW facts, preferences, decisions, or commitments worth remembering long-term. Return null if nothing new was learned.

Respond ONLY with valid JSON: {"history_entry": "...", "memory_update": "..." or null}
Do not include any text outside the JSON object."#;

/// Run two-phase LLM-driven consolidation on a conversation turn.
///
/// Phase 1: Write a history entry to the Daily memory category.
/// Phase 2: Write a memory update to the Core category (if the LLM identified new facts).
///
/// This function is designed to be called fire-and-forget via `tokio::spawn`.
pub async fn consolidate_turn(
    provider: &dyn Provider,
    model: &str,
    memory: &dyn UnifiedMemoryPort,
    user_message: &str,
    assistant_response: &str,
    agent_id: &str,
) -> anyhow::Result<ConsolidationOutcome> {
    let turn_text = format!("User: {user_message}\nAssistant: {assistant_response}");

    // Truncate very long turns to avoid wasting tokens on consolidation.
    // Use char-boundary-safe slicing to prevent panic on multi-byte UTF-8 (e.g. CJK text).
    let truncated = if turn_text.len() > 4000 {
        let end = turn_text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 4000)
            .last()
            .unwrap_or(0);
        format!("{}…", &turn_text[..end])
    } else {
        turn_text.clone()
    };

    tracing::info!(agent_id, "memory.consolidation.start");

    let raw = provider
        .chat_with_system(Some(CONSOLIDATION_SYSTEM_PROMPT), &truncated, model, 0.1)
        .await?;

    let result: ConsolidationResult = parse_consolidation_response(&raw, &turn_text);

    tracing::info!(
        agent_id,
        has_memory_update = result.memory_update.is_some(),
        "memory.consolidation.extracted"
    );

    // Phase 1: Write history entry to Daily category.
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let history_key = format!("daily_{date}_{}", uuid::Uuid::new_v4());
    memory
        .store(
            &history_key,
            &result.history_entry,
            &MemoryCategory::Daily,
            None,
        )
        .await?;

    tracing::info!(key = %history_key, "memory.consolidation.daily_stored");

    // Phase 2: Write memory update to Core category (if present).
    if let Some(ref update) = result.memory_update {
        if !update.trim().is_empty() {
            let mem_key = format!("core_{}", uuid::Uuid::new_v4());
            memory
                .store(&mem_key, update, &MemoryCategory::Core, None)
                .await?;
            tracing::info!(key = %mem_key, "memory.consolidation.core_stored");
        }
    }

    // Phase 3: Entity extraction — populate knowledge graph.
    // Best-effort: errors logged but don't fail consolidation.
    let mut outcome = ConsolidationOutcome::default();

    tracing::info!(agent_id, "memory.consolidation.entity_extraction_start");

    match entity_extractor::extract_entities(provider, model, &truncated).await {
        Ok(extraction) => {
            tracing::info!(
                entities = extraction.entities.len(),
                relationships = extraction.relationships.len(),
                "memory.consolidation.entities_extracted"
            );
            if !extraction.entities.is_empty() || !extraction.relationships.is_empty() {
                outcome.entities_extracted = extraction.entities.len();
                if let Err(e) =
                    entity_extractor::store_extraction(memory, &extraction, agent_id).await
                {
                    tracing::debug!("Entity storage failed: {e}");
                }
            }
        }
        Err(e) => {
            tracing::debug!("Entity extraction skipped: {e}");
        }
    }

    Ok(outcome)
}

/// Parse the LLM's consolidation response, with fallback for malformed JSON.
fn parse_consolidation_response(raw: &str, fallback_text: &str) -> ConsolidationResult {
    // Try to extract JSON from the response (LLM may wrap in markdown code blocks).
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned).unwrap_or_else(|_| {
        // Fallback: use truncated turn text as history entry.
        // Use char-boundary-safe slicing to prevent panic on multi-byte UTF-8.
        let summary = if fallback_text.len() > 200 {
            let end = fallback_text
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= 200)
                .last()
                .unwrap_or(0);
            format!("{}…", &fallback_text[..end])
        } else {
            fallback_text.to_string()
        };
        ConsolidationResult {
            history_entry: summary,
            memory_update: None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json_response() {
        let raw = r#"{"history_entry": "User asked about Rust.", "memory_update": "User prefers Rust over Go."}"#;
        let result = parse_consolidation_response(raw, "fallback");
        assert_eq!(result.history_entry, "User asked about Rust.");
        assert_eq!(
            result.memory_update.as_deref(),
            Some("User prefers Rust over Go.")
        );
    }

    #[test]
    fn parse_json_with_null_memory() {
        let raw = r#"{"history_entry": "Routine greeting.", "memory_update": null}"#;
        let result = parse_consolidation_response(raw, "fallback");
        assert_eq!(result.history_entry, "Routine greeting.");
        assert!(result.memory_update.is_none());
    }

    #[test]
    fn parse_json_wrapped_in_code_block() {
        let raw =
            "```json\n{\"history_entry\": \"Discussed deployment.\", \"memory_update\": null}\n```";
        let result = parse_consolidation_response(raw, "fallback");
        assert_eq!(result.history_entry, "Discussed deployment.");
    }

    #[test]
    fn fallback_on_malformed_response() {
        let raw = "I'm sorry, I can't do that.";
        let result = parse_consolidation_response(raw, "User: hello\nAssistant: hi");
        assert_eq!(result.history_entry, "User: hello\nAssistant: hi");
        assert!(result.memory_update.is_none());
    }

    #[test]
    fn fallback_truncates_long_text() {
        let long_text = "x".repeat(500);
        let result = parse_consolidation_response("invalid", &long_text);
        // 200 bytes + "…" (3 bytes in UTF-8) = 203
        assert!(result.history_entry.len() <= 203);
    }

    #[test]
    fn fallback_truncates_cjk_text_without_panic() {
        // Each CJK character is 3 bytes in UTF-8; byte index 200 may land
        // inside a character. This must not panic.
        let cjk_text = "二手书项目".repeat(50); // 250 chars = 750 bytes
        let result = parse_consolidation_response("invalid", &cjk_text);
        assert!(result
            .history_entry
            .is_char_boundary(result.history_entry.len()));
        assert!(result.history_entry.ends_with('…'));
    }
}
