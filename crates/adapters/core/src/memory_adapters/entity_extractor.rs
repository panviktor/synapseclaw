//! LLM-driven entity and relationship extraction from conversation turns.
//!
//! Phase 4.3 Slice 3: extracts entities (people, companies, concepts, tools)
//! and relationships from conversations, then stores them in the knowledge graph
//! via SemanticMemoryPort.

use chrono::Utc;
use synapse_domain::domain::memory::{Entity, MemoryError, TemporalFact};
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_providers::traits::Provider;

const EXTRACTION_PROMPT: &str = r#"Extract entities and relationships from this conversation turn.
Return ONLY valid JSON, no other text.

Format:
{
  "entities": [
    {"name": "exact name", "type": "person|company|concept|tool|place|project", "summary": "one-line description"}
  ],
  "relationships": [
    {"subject": "entity name", "predicate": "verb_phrase", "object": "entity name", "confidence": 0.9}
  ]
}

Rules:
- Merge name variations: "Victor", "the user" → one entity
- predicate: lowercase verb phrase like "works_at", "prefers", "knows_about", "created"
- confidence: 0.0–1.0 based on how explicit the statement is
- Only extract what is clearly stated, do NOT infer
- If nothing worth extracting, return: {"entities": [], "relationships": []}

Conversation turn:
"#;

#[derive(Debug, serde::Deserialize)]
pub struct ExtractionResult {
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub relationships: Vec<ExtractedRelationship>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ExtractedRelationship {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    0.8
}

/// Extract entities and relationships from a conversation turn using LLM.
pub async fn extract_entities(
    provider: &dyn Provider,
    model: &str,
    turn_text: &str,
) -> anyhow::Result<ExtractionResult> {
    let prompt = format!("{EXTRACTION_PROMPT}{turn_text}");

    let raw = provider.chat_with_system(None, &prompt, model, 0.1).await?;

    parse_extraction_response(&raw)
}

fn parse_extraction_response(raw: &str) -> anyhow::Result<ExtractionResult> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned).map_err(|e| anyhow::anyhow!("Entity extraction parse error: {e}"))
}

/// Store extracted entities and relationships into the knowledge graph.
pub async fn store_extraction(
    memory: &dyn UnifiedMemoryPort,
    extraction: &ExtractionResult,
    agent_id: &str,
) -> Result<(), MemoryError> {
    // 1. Upsert entities
    for extracted in &extraction.entities {
        let entity = Entity {
            id: String::new(), // let adapter generate
            name: extracted.name.clone(),
            entity_type: extracted.entity_type.clone(),
            properties: serde_json::Value::Object(Default::default()),
            summary: extracted.summary.clone(),
            created_by: agent_id.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        match memory.upsert_entity(entity).await {
            Ok(_) => {
                tracing::info!(
                    name = %extracted.name,
                    entity_type = %extracted.entity_type,
                    "memory.entity.upserted"
                );
            }
            Err(e) => {
                tracing::debug!("Entity upsert failed for '{}': {e}", extracted.name);
            }
        }
    }

    // 2. Add facts with AUDN deduplication (Add/Update/Delete/Noop)
    for rel in &extraction.relationships {
        let subject_entity = memory.find_entity(&rel.subject).await.ok().flatten();
        let object_entity = memory.find_entity(&rel.object).await.ok().flatten();

        let (subject_id, object_id, subject_name, object_name) =
            match (subject_entity, object_entity) {
                (Some(s), Some(o)) => (s.id.clone(), o.id.clone(), s.name.clone(), o.name.clone()),
                _ => {
                    tracing::debug!(
                        subject = %rel.subject,
                        object = %rel.object,
                        "memory.fact.skipped_entities_not_found"
                    );
                    continue;
                }
            };

        // AUDN: embed fact using entity NAMES (not IDs) for meaningful similarity
        let fact_text = format!(
            "{subject_name} {predicate} {object_name}",
            predicate = rel.predicate
        );
        let mut final_confidence = rel.confidence;
        let mut fact_embedding: Option<Vec<f32>> = None;
        let audn_action = match memory.embed_document(&fact_text).await {
            Ok(embedding) if !embedding.is_empty() => {
                fact_embedding = Some(embedding.clone()); // capture for reuse in add_fact
                match memory.find_similar_facts(&embedding, 5).await {
                    Ok(similar) if !similar.is_empty() => {
                        let (best_fact, best_sim) = &similar[0];
                        if *best_sim > 0.95 {
                            // NOOP: near-exact duplicate
                            tracing::info!(
                                sim = *best_sim,
                                subject = %rel.subject,
                                predicate = %rel.predicate,
                                object = %rel.object,
                                "memory.audn.noop"
                            );
                            continue;
                        } else if *best_sim > 0.85
                            && best_fact.predicate == rel.predicate
                            && best_fact.subject == subject_id
                            && best_fact.object == object_id
                        {
                            // UPDATE: same entities + same predicate → merge confidence
                            final_confidence = best_fact.confidence.max(rel.confidence);
                            let _ = memory.invalidate_fact(&best_fact.id).await;
                            tracing::info!(
                                sim = *best_sim,
                                old_confidence = best_fact.confidence,
                                new_confidence = final_confidence,
                                "memory.audn.update"
                            );
                            "update"
                        } else if *best_sim > 0.85
                            && best_fact.subject == subject_id
                            && best_fact.object == object_id
                            && best_fact.predicate != rel.predicate
                        {
                            // REPLACE: same entities but contradictory predicate
                            let _ = memory.invalidate_fact(&best_fact.id).await;
                            tracing::info!(
                                sim = *best_sim,
                                old_predicate = %best_fact.predicate,
                                new_predicate = %rel.predicate,
                                "memory.audn.replace"
                            );
                            "replace"
                        } else {
                            // ADD: different entities or below threshold
                            "add"
                        }
                    }
                    _ => "add",
                }
            }
            _ => "add", // embedding unavailable — just add
        };

        let fact = TemporalFact {
            id: String::new(),
            subject: subject_id,
            predicate: rel.predicate.clone(),
            object: object_id,
            confidence: final_confidence,
            valid_from: Utc::now(),
            valid_to: None,
            recorded_at: Utc::now(),
            source_episode: None,
            created_by: agent_id.to_string(),
            embedding: fact_embedding, // reuse AUDN embedding — no redundant API call
        };

        match memory.add_fact(fact).await {
            Ok(_) => {
                tracing::info!(
                    subject = %rel.subject,
                    predicate = %rel.predicate,
                    object = %rel.object,
                    confidence = final_confidence,
                    audn = audn_action,
                    "memory.fact.added"
                );
            }
            Err(e) => {
                tracing::debug!(
                    "Fact creation failed for '{}' {} '{}': {e}",
                    rel.subject,
                    rel.predicate,
                    rel.object
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_extraction() {
        let json = r#"{
            "entities": [
                {"name": "Victor", "type": "person", "summary": "the user"},
                {"name": "Rust", "type": "concept", "summary": "programming language"}
            ],
            "relationships": [
                {"subject": "Victor", "predicate": "prefers", "object": "Rust", "confidence": 0.95}
            ]
        }"#;
        let result = parse_extraction_response(json).unwrap();
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.relationships.len(), 1);
        assert_eq!(result.relationships[0].predicate, "prefers");
    }

    #[test]
    fn parse_empty_extraction() {
        let json = r#"{"entities": [], "relationships": []}"#;
        let result = parse_extraction_response(json).unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn parse_code_block_wrapped() {
        let json = "```json\n{\"entities\": [], \"relationships\": []}\n```";
        let result = parse_extraction_response(json).unwrap();
        assert!(result.entities.is_empty());
    }

    #[test]
    fn parse_malformed_returns_error() {
        let result = parse_extraction_response("not json");
        assert!(result.is_err());
    }

    #[test]
    fn default_confidence_is_applied() {
        let json = r#"{
            "entities": [],
            "relationships": [
                {"subject": "A", "predicate": "knows", "object": "B"}
            ]
        }"#;
        let result = parse_extraction_response(json).unwrap();
        assert!((result.relationships[0].confidence - 0.8).abs() < 0.01);
    }
}
