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

    // 2. Add facts (relationships)
    for rel in &extraction.relationships {
        // Resolve subject and object entities by name
        let subject = memory.find_entity(&rel.subject).await.ok().flatten();
        let object = memory.find_entity(&rel.object).await.ok().flatten();

        let (subject_id, object_id) = match (subject, object) {
            (Some(s), Some(o)) => (s.id, o.id),
            _ => {
                tracing::debug!(
                    subject = %rel.subject,
                    object = %rel.object,
                    "memory.fact.skipped_entities_not_found"
                );
                continue;
            }
        };

        let fact = TemporalFact {
            id: String::new(),
            subject: subject_id,
            predicate: rel.predicate.clone(),
            object: object_id,
            confidence: rel.confidence,
            valid_from: Utc::now(),
            valid_to: None,
            recorded_at: Utc::now(),
            source_episode: None,
            created_by: agent_id.to_string(),
        };

        match memory.add_fact(fact).await {
            Ok(_) => {
                tracing::info!(
                    subject = %rel.subject,
                    predicate = %rel.predicate,
                    object = %rel.object,
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
