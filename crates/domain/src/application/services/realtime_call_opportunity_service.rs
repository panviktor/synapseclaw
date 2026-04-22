use crate::domain::message::ChatMessage;
use crate::ports::memory::UnifiedMemoryPort;
use std::collections::HashMap;
use std::sync::OnceLock;

const MAX_FAST_PATH_CHARS: usize = 180;
const MAX_FAST_PATH_WORDS: usize = 28;
const MIN_CURRENT_CONVERSATION_SIMILARITY: f64 = 0.83;
const MIN_DECISION_MARGIN: f64 = 0.05;
const MAX_HANDOFF_SUMMARY_CHARS: usize = 360;
const MAX_HANDOFF_TURNS: usize = 4;
const MAX_HANDOFF_TURN_CHARS: usize = 220;

const CURRENT_CONVERSATION_CALL_PROTOTYPES: &[&str] = &[
    "The user wants the assistant to switch this chat into a live voice call with the same user right now.",
    "The user is asking the assistant to call them now in the current conversation instead of continuing by text.",
    "The user wants to continue this conversation as a live audio call with the assistant.",
];

const THIRD_PARTY_CALL_PROTOTYPES: &[&str] = &[
    "The user wants the assistant to call a third party such as a restaurant, store, office, or another phone number.",
    "The user is asking the assistant to place an external phone call to someone else.",
];

const CALL_DISCUSSION_ONLY_PROTOTYPES: &[&str] = &[
    "The user is only discussing calls as a topic and is not asking to start a live call right now.",
    "The user mentions calls, but does not want the assistant to begin a live call in this conversation.",
];

static PROTOTYPE_CACHE: OnceLock<tokio::sync::Mutex<HashMap<String, CachedPrototypeEmbeddings>>> =
    OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeCallOpportunityDecision {
    NoFastPath,
    StartCurrentConversation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeCallOpportunityAssessment {
    pub decision: RealtimeCallOpportunityDecision,
    pub current_conversation_similarity: Option<f64>,
    pub third_party_similarity: Option<f64>,
    pub discussion_only_similarity: Option<f64>,
}

impl Default for RealtimeCallOpportunityAssessment {
    fn default() -> Self {
        Self {
            decision: RealtimeCallOpportunityDecision::NoFastPath,
            current_conversation_similarity: None,
            third_party_similarity: None,
            discussion_only_similarity: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeCallHandoffInput<'a> {
    pub existing_summary: Option<&'a str>,
    pub recent_turns: &'a [ChatMessage],
}

#[derive(Debug, Clone)]
struct CachedPrototypeEmbeddings {
    current_conversation: Vec<Vec<f32>>,
    third_party: Vec<Vec<f32>>,
    discussion_only: Vec<Vec<f32>>,
}

pub async fn assess_realtime_call_opportunity(
    memory: &dyn UnifiedMemoryPort,
    cache_key: Option<&str>,
    message: &str,
) -> RealtimeCallOpportunityAssessment {
    let trimmed = message.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_FAST_PATH_CHARS
        || trimmed.split_whitespace().count() > MAX_FAST_PATH_WORDS
    {
        return RealtimeCallOpportunityAssessment::default();
    }

    let query_embedding = match memory.embed_query(trimmed).await {
        Ok(embedding) if !embedding.is_empty() => embedding,
        _ => return RealtimeCallOpportunityAssessment::default(),
    };

    let cached = match cached_prototype_embeddings(memory, cache_key).await {
        Some(cached) => cached,
        None => return RealtimeCallOpportunityAssessment::default(),
    };

    let current_conversation_similarity =
        best_similarity(&query_embedding, &cached.current_conversation);
    let third_party_similarity = best_similarity(&query_embedding, &cached.third_party);
    let discussion_only_similarity = best_similarity(&query_embedding, &cached.discussion_only);

    let decision = if let Some(current) = current_conversation_similarity {
        let third_party = third_party_similarity.unwrap_or(f64::NEG_INFINITY);
        let discussion_only = discussion_only_similarity.unwrap_or(f64::NEG_INFINITY);
        if current >= MIN_CURRENT_CONVERSATION_SIMILARITY
            && (current - third_party) >= MIN_DECISION_MARGIN
            && (current - discussion_only) >= MIN_DECISION_MARGIN
        {
            RealtimeCallOpportunityDecision::StartCurrentConversation
        } else {
            RealtimeCallOpportunityDecision::NoFastPath
        }
    } else {
        RealtimeCallOpportunityDecision::NoFastPath
    };

    RealtimeCallOpportunityAssessment {
        decision,
        current_conversation_similarity,
        third_party_similarity,
        discussion_only_similarity,
    }
}

pub fn build_realtime_call_handoff(input: RealtimeCallHandoffInput<'_>) -> Option<String> {
    let mut sections =
        vec!["Continue the existing chat as a live voice call. Do not restart cold.".to_string()];

    if let Some(summary) = bounded_text(
        input.existing_summary.unwrap_or(""),
        MAX_HANDOFF_SUMMARY_CHARS,
    ) {
        sections.push(format!("Recent chat summary:\n{summary}"));
    }

    let recent_turns = input
        .recent_turns
        .iter()
        .rev()
        .filter(|turn| matches!(turn.role.as_str(), "user" | "assistant"))
        .filter_map(|turn| {
            bounded_text(&turn.content, MAX_HANDOFF_TURN_CHARS).map(|content| {
                let role = match turn.role.as_str() {
                    "user" => "User",
                    "assistant" => "Assistant",
                    _ => "Turn",
                };
                format!("{role}: {content}")
            })
        })
        .take(MAX_HANDOFF_TURNS)
        .collect::<Vec<_>>();

    if !recent_turns.is_empty() {
        let mut ordered = recent_turns;
        ordered.reverse();
        sections.push(format!("Recent chat turns:\n- {}", ordered.join("\n- ")));
    }

    (sections.len() > 1).then(|| sections.join("\n\n"))
}

async fn cached_prototype_embeddings(
    memory: &dyn UnifiedMemoryPort,
    cache_key: Option<&str>,
) -> Option<CachedPrototypeEmbeddings> {
    let key = prototype_cache_key(memory, cache_key);
    let cache = PROTOTYPE_CACHE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
    {
        let guard = cache.lock().await;
        if let Some(cached) = guard.get(&key) {
            return Some(cached.clone());
        }
    }

    let current_conversation =
        embed_prototype_set(memory, CURRENT_CONVERSATION_CALL_PROTOTYPES).await?;
    let third_party = embed_prototype_set(memory, THIRD_PARTY_CALL_PROTOTYPES).await?;
    let discussion_only = embed_prototype_set(memory, CALL_DISCUSSION_ONLY_PROTOTYPES).await?;

    let cached = CachedPrototypeEmbeddings {
        current_conversation,
        third_party,
        discussion_only,
    };

    let mut guard = cache.lock().await;
    let entry = guard.entry(key).or_insert_with(|| cached.clone());
    Some(entry.clone())
}

async fn embed_prototype_set(
    memory: &dyn UnifiedMemoryPort,
    prototypes: &[&str],
) -> Option<Vec<Vec<f32>>> {
    let mut embeddings = Vec::with_capacity(prototypes.len());
    for prototype in prototypes {
        let embedding = memory.embed_document(prototype).await.ok()?;
        if embedding.is_empty() {
            return None;
        }
        embeddings.push(embedding);
    }
    Some(embeddings)
}

fn prototype_cache_key(memory: &dyn UnifiedMemoryPort, override_key: Option<&str>) -> String {
    if let Some(value) = override_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return value.to_string();
    }
    let profile = memory.embedding_profile();
    format!(
        "profile={};provider={};model={};dims={};metric={:?};normalized={}",
        profile.profile_id,
        profile.provider_family,
        profile.model_id,
        profile.dimensions,
        profile.distance_metric,
        profile.normalize_output
    )
}

fn bounded_text(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut bounded = trimmed.chars().take(limit).collect::<String>();
    if trimmed.chars().count() > limit {
        bounded.push_str("...");
    }
    Some(bounded)
}

fn best_similarity(query: &[f32], prototypes: &[Vec<f32>]) -> Option<f64> {
    prototypes
        .iter()
        .filter_map(|prototype| cosine_similarity(query, prototype))
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (&left_value, &right_value) in left.iter().zip(right.iter()) {
        let left_value = left_value as f64;
        let right_value = right_value as f64;
        dot += left_value * right_value;
        left_norm += left_value * left_value;
        right_norm += right_value * right_value;
    }
    if left_norm <= f64::EPSILON || right_norm <= f64::EPSILON {
        return None;
    }
    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingDistanceMetric, EmbeddingProfile,
        Entity, HybridSearchResult, MemoryCategory, MemoryEntry, MemoryError, MemoryId,
        MemoryQuery, Reflection, SearchResult, SessionId, Skill, SkillUpdate, TemporalFact,
        Visibility,
    };
    use crate::ports::memory::UnifiedMemoryPort;
    use async_trait::async_trait;

    #[derive(Default)]
    struct TestSemanticMemory;

    #[async_trait]
    impl crate::ports::memory::WorkingMemoryPort for TestSemanticMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
            Ok(Vec::new())
        }

        async fn update_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: String,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn append_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl crate::ports::memory::EpisodicMemoryPort for TestSemanticMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Ok("episode".into())
        }

        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(Vec::new())
        }

        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(Vec::new())
        }

        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl crate::ports::memory::SemanticMemoryPort for TestSemanticMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
            Ok("entity".into())
        }

        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
            Ok(None)
        }

        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
            Ok("fact".into())
        }

        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
            Ok(Vec::new())
        }

        async fn traverse(
            &self,
            _: &MemoryId,
            _: usize,
        ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
            Ok(Vec::new())
        }

        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl crate::ports::memory::SkillMemoryPort for TestSemanticMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
            Ok("skill".into())
        }

        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(Vec::new())
        }

        async fn update_skill(
            &self,
            _: &MemoryId,
            _: SkillUpdate,
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl crate::ports::memory::ReflectionPort for TestSemanticMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
            Ok("reflection".into())
        }

        async fn get_relevant_reflections(
            &self,
            _: &MemoryQuery,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(Vec::new())
        }

        async fn get_failure_patterns(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl crate::ports::memory::ConsolidationPort for TestSemanticMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
            Ok(ConsolidationReport::default())
        }

        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
            Ok(0)
        }

        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl UnifiedMemoryPort for TestSemanticMemory {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(test_embedding(text))
        }

        fn embedding_profile(&self) -> EmbeddingProfile {
            EmbeddingProfile {
                profile_id: "test:test:6".into(),
                provider_family: "test".into(),
                model_id: "semantic-gate".into(),
                dimensions: 6,
                distance_metric: EmbeddingDistanceMetric::Cosine,
                normalize_output: false,
                query_prefix: None,
                document_prefix: None,
                supports_multilingual: true,
                supports_code: false,
                recommended_chunk_chars: 256,
                recommended_top_k: 8,
            }
        }

        async fn store(
            &self,
            _: &str,
            _: &str,
            _: &MemoryCategory,
            _: Option<&str>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(HybridSearchResult::default())
        }

        async fn recall(
            &self,
            _: &str,
            _: usize,
            _: Option<&str>,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(Vec::new())
        }

        async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }

        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> {
            Ok(false)
        }

        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> {
            Ok(None)
        }

        async fn list(
            &self,
            _: Option<&MemoryCategory>,
            _: Option<&str>,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(Vec::new())
        }

        async fn count(&self) -> Result<usize, MemoryError> {
            Ok(0)
        }

        fn name(&self) -> &str {
            "test-semantic-memory"
        }

        async fn health_check(&self) -> bool {
            true
        }

        async fn promote_visibility(
            &self,
            _: &MemoryId,
            _: &Visibility,
            _: &[AgentId],
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn test_embedding(text: &str) -> Vec<f32> {
        let lower = text.to_ascii_lowercase();
        let current = contains_any(
            &lower,
            &[
                "current conversation",
                "call them now",
                "live audio call",
                "switch this chat",
            ],
        ) as i32 as f32;
        let driving =
            contains_any(&lower, &["driving", "instead of continuing by text"]) as i32 as f32;
        let third_party = contains_any(
            &lower,
            &["third party", "restaurant", "store", "another phone"],
        ) as i32 as f32;
        let discussion =
            contains_any(&lower, &["discussing calls", "mentions calls", "topic"]) as i32 as f32;
        let russian = contains_any(&lower, &["звон", "голос", "чат"]) as i32 as f32;
        let external_action =
            contains_any(&lower, &["someone else", "contact someone else"]) as i32 as f32;
        vec![
            current,
            driving,
            third_party,
            discussion,
            russian,
            external_action,
        ]
    }

    fn contains_any(text: &str, needles: &[&str]) -> bool {
        needles.iter().any(|needle| text.contains(needle))
    }

    #[tokio::test]
    async fn semantic_gate_accepts_current_conversation_call_request() {
        let memory = TestSemanticMemory;
        let assessment = assess_realtime_call_opportunity(
            &memory,
            Some("test"),
            "Please switch this chat into a live voice call with me now.",
        )
        .await;
        assert_eq!(
            assessment.decision,
            RealtimeCallOpportunityDecision::StartCurrentConversation
        );
    }

    #[tokio::test]
    async fn semantic_gate_rejects_third_party_call_request() {
        let memory = TestSemanticMemory;
        let assessment = assess_realtime_call_opportunity(
            &memory,
            Some("test-third-party"),
            "Call the restaurant and ask whether they have a table at seven.",
        )
        .await;
        assert_eq!(
            assessment.decision,
            RealtimeCallOpportunityDecision::NoFastPath
        );
    }

    #[tokio::test]
    async fn semantic_gate_rejects_discussion_without_live_call_request() {
        let memory = TestSemanticMemory;
        let assessment = assess_realtime_call_opportunity(
            &memory,
            Some("test-discussion"),
            "We should discuss whether phone calls are better than chat for this workflow.",
        )
        .await;
        assert_eq!(
            assessment.decision,
            RealtimeCallOpportunityDecision::NoFastPath
        );
    }

    #[test]
    fn handoff_uses_summary_and_recent_turns() {
        let handoff = build_realtime_call_handoff(RealtimeCallHandoffInput {
            existing_summary: Some("The user is choosing a restaurant near Alexanderplatz."),
            recent_turns: &[
                ChatMessage::user("Can you help me pick a quiet place?"),
                ChatMessage::assistant("Yes. Do you want Italian or Japanese?"),
                ChatMessage::user("Italian, not too expensive."),
            ],
        })
        .expect("handoff");

        assert!(handoff.contains("Recent chat summary"));
        assert!(handoff.contains("User: Can you help me pick a quiet place?"));
        assert!(handoff.contains("Assistant: Yes. Do you want Italian or Japanese?"));
        assert!(handoff.contains("Italian, not too expensive."));
    }
}
