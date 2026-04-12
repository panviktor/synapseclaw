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
    pub memory_update: Option<ConsolidatedMemoryUpdate>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum ConsolidatedMemoryUpdate {
    Classified {
        class: ConsolidatedMemoryClass,
        text: String,
        #[serde(default)]
        confidence: Option<f32>,
    },
    LegacyText(String),
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidatedMemoryClass {
    Preference,
    TaskState,
    FactAnchor,
    GenericDialogue,
}

/// Summary of what consolidation produced (returned to caller for IPC broadcast).
#[derive(Debug, Default)]
pub struct ConsolidationOutcome {
    pub entities_extracted: usize,
}

const CONSOLIDATION_SYSTEM_PROMPT: &str = r#"You are a memory consolidation engine. Given a conversation turn, extract:
1. "history_entry": A brief summary of what happened in this turn (1-2 sentences). Include the key topic or action.
2. "memory_update": a typed object only for NEW durable user/project/runtime facts, preferences, decisions, or commitments worth remembering long-term; otherwise null.

memory_update format:
{"class":"preference|task_state|fact_anchor","text":"durable user/project/runtime-scoped update","confidence":0.0-1.0}

Use null for generic world knowledge, abstract philosophy, broad opinions, or ordinary dialogue that belongs only in history_entry.

Respond ONLY with valid JSON: {"history_entry": "...", "memory_update": {...} or null}
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
    let mut outcome = ConsolidationOutcome::default();
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

    let result = match parse_consolidation_response(&raw) {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(
                agent_id,
                error = %error,
                "memory.consolidation.invalid_response"
            );
            return Ok(outcome);
        }
    };

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

    // Phase 2: Evaluate memory update via AUDN-lite mutation service.
    if let Some(ref update) = result.memory_update {
        let update_text = update.text();
        if !update_text.trim().is_empty() {
            use synapse_domain::application::services::memory_mutation as mutation;
            use synapse_domain::domain::memory_mutation::{
                MutationCandidate, MutationSource, MutationThresholds,
            };

            let candidate = MutationCandidate {
                category: MemoryCategory::Core,
                text: update_text.to_string(),
                confidence: update.confidence().unwrap_or(0.7).clamp(0.0, 1.0),
                source: MutationSource::Consolidation,
                write_class: Some(update.write_class()),
            };
            let thresholds = MutationThresholds::default();
            let decision =
                mutation::evaluate_candidate(memory, candidate, agent_id, &thresholds).await;

            match mutation::apply_decision_with_event(memory, &decision, agent_id).await {
                Ok(event) => {
                    tracing::info!(
                        kind = ?event.kind,
                        reason = %decision.reason,
                        "memory.consolidation.core_event"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "memory.consolidation.core_mutation_failed"
                    );
                }
            }
        }
    }

    // Phase 3: Entity extraction — populate knowledge graph.
    // Best-effort: errors logged but don't fail consolidation.
    if !result
        .memory_update
        .as_ref()
        .is_some_and(ConsolidatedMemoryUpdate::allows_graph_extraction)
    {
        tracing::info!(
            agent_id,
            "memory.consolidation.entity_extraction_skipped_no_durable_update"
        );
        return Ok(outcome);
    }

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

/// Parse the LLM's consolidation response.
fn parse_consolidation_response(raw: &str) -> anyhow::Result<ConsolidationResult> {
    // Try to extract JSON from the response (LLM may wrap in markdown code blocks).
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned).map_err(|error| {
        anyhow::anyhow!("consolidation response is not valid JSON object: {error}")
    })
}

impl ConsolidatedMemoryUpdate {
    fn text(&self) -> &str {
        match self {
            Self::Classified { text, .. } | Self::LegacyText(text) => text,
        }
    }

    fn confidence(&self) -> Option<f32> {
        match self {
            Self::Classified { confidence, .. } => *confidence,
            Self::LegacyText(_) => None,
        }
    }

    fn write_class(&self) -> synapse_domain::domain::memory_mutation::MutationWriteClass {
        use synapse_domain::domain::memory_mutation::MutationWriteClass;
        match self {
            Self::Classified { class, .. } => match class {
                ConsolidatedMemoryClass::Preference => MutationWriteClass::Preference,
                ConsolidatedMemoryClass::TaskState => MutationWriteClass::TaskState,
                ConsolidatedMemoryClass::FactAnchor => MutationWriteClass::FactAnchor,
                ConsolidatedMemoryClass::GenericDialogue => MutationWriteClass::GenericDialogue,
            },
            Self::LegacyText(_) => MutationWriteClass::GenericDialogue,
        }
    }

    fn allows_graph_extraction(&self) -> bool {
        matches!(
            self,
            Self::Classified {
                class: ConsolidatedMemoryClass::Preference
                    | ConsolidatedMemoryClass::TaskState
                    | ConsolidatedMemoryClass::FactAnchor,
                ..
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json_response() {
        let raw = r#"{"history_entry": "User asked about Rust.", "memory_update": "User prefers Rust over Go."}"#;
        let result = parse_consolidation_response(raw).unwrap();
        assert_eq!(result.history_entry, "User asked about Rust.");
        assert_eq!(
            result
                .memory_update
                .as_ref()
                .map(ConsolidatedMemoryUpdate::text),
            Some("User prefers Rust over Go.")
        );
    }

    #[test]
    fn parse_typed_memory_update_response() {
        let raw = r#"{"history_entry": "User discussed deployment.", "memory_update": {"class":"task_state","text":"Atlas deployment is blocked on SSO login verification.","confidence":0.82}}"#;
        let result = parse_consolidation_response(raw).unwrap();
        let update = result.memory_update.as_ref().unwrap();
        assert_eq!(
            update.write_class(),
            synapse_domain::domain::memory_mutation::MutationWriteClass::TaskState
        );
        assert_eq!(
            update.text(),
            "Atlas deployment is blocked on SSO login verification."
        );
        assert_eq!(update.confidence(), Some(0.82));
        assert!(update.allows_graph_extraction());
    }

    #[test]
    fn generic_dialogue_memory_update_does_not_allow_graph_extraction() {
        let raw = r#"{"history_entry": "User discussed abstract reflection.", "memory_update": {"class":"generic_dialogue","text":"The dialogue explored abstract reflection without durable task facts.","confidence":0.62}}"#;
        let result = parse_consolidation_response(raw).unwrap();
        let update = result.memory_update.as_ref().unwrap();

        assert_eq!(
            update.write_class(),
            synapse_domain::domain::memory_mutation::MutationWriteClass::GenericDialogue
        );
        assert!(!update.allows_graph_extraction());
    }

    #[test]
    fn legacy_memory_update_does_not_allow_graph_extraction() {
        let raw = r#"{"history_entry": "User discussed a preference.", "memory_update": "User prefers concise reports."}"#;
        let result = parse_consolidation_response(raw).unwrap();
        let update = result.memory_update.as_ref().unwrap();

        assert_eq!(
            update.write_class(),
            synapse_domain::domain::memory_mutation::MutationWriteClass::GenericDialogue
        );
        assert!(!update.allows_graph_extraction());
    }

    #[test]
    fn parse_json_with_null_memory() {
        let raw = r#"{"history_entry": "Routine greeting.", "memory_update": null}"#;
        let result = parse_consolidation_response(raw).unwrap();
        assert_eq!(result.history_entry, "Routine greeting.");
        assert!(result.memory_update.is_none());
    }

    #[test]
    fn parse_json_wrapped_in_code_block() {
        let raw =
            "```json\n{\"history_entry\": \"Discussed deployment.\", \"memory_update\": null}\n```";
        let result = parse_consolidation_response(raw).unwrap();
        assert_eq!(result.history_entry, "Discussed deployment.");
    }

    #[test]
    fn rejects_malformed_response() {
        let raw = "I'm sorry, I can't do that.";
        let err = parse_consolidation_response(raw).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"));
    }
}
