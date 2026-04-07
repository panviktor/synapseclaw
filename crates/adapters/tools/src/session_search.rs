//! Session search tool — search past conversation sessions.
//!
//! Uses the shared retrieval service so session search logic is not duplicated
//! inside tools.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::application::services::retrieval_service;
use synapse_domain::domain::tool_fact::{SearchDomain, SearchFact, ToolFactPayload, TypedToolFact};
use synapse_domain::ports::conversation_store::ConversationStorePort;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::tool::{Tool, ToolExecution, ToolResult};

pub struct SessionSearchTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    store: Arc<dyn ConversationStorePort>,
}

impl SessionSearchTool {
    pub fn new(memory: Arc<dyn UnifiedMemoryPort>, store: Arc<dyn ConversationStorePort>) -> Self {
        Self { memory, store }
    }

    async fn execute_query(
        &self,
        args: &serde_json::Value,
    ) -> anyhow::Result<(ToolResult, Vec<retrieval_service::SessionSearchMatch>)> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(10) as usize;
        let kind_filter = args.get("kind").and_then(|v| v.as_str());

        if query.trim().is_empty() {
            return Ok((
                ToolResult {
                    output: "Query cannot be empty".into(),
                    success: false,
                    error: None,
                },
                Vec::new(),
            ));
        }

        let hits = retrieval_service::search_sessions(
            self.memory.as_ref(),
            self.store.as_ref(),
            query,
            kind_filter,
            limit,
        )
        .await;

        if hits.is_empty() {
            return Ok((
                ToolResult {
                    output: format!("No sessions found matching '{query}'"),
                    success: true,
                    error: None,
                },
                hits,
            ));
        }

        let mut output = format!("Found {} session(s) matching '{query}':\n\n", hits.len());
        for (index, hit) in hits.iter().enumerate() {
            let label = hit.label.as_deref().unwrap_or("(untitled)");
            let summary = hit
                .summary
                .as_deref()
                .map(|summary| truncate_chars(summary, 150))
                .unwrap_or_else(|| "(no summary)".into());
            let kind = hit.kind.to_string();

            output.push_str(&format!(
                "{}. **{}** ({})\n   {} messages | {}\n",
                index + 1,
                label,
                kind,
                hit.message_count,
                summary
            ));

            if let Some(ref recap) = hit.recap {
                output.push_str(&format!("   Recent transcript match: {}\n", recap));
            }

            output.push('\n');
        }

        Ok((
            ToolResult {
                output,
                success: true,
                error: None,
            },
            hits,
        ))
    }

    fn build_result_facts(
        &self,
        hits: &[retrieval_service::SessionSearchMatch],
    ) -> Vec<TypedToolFact> {
        if hits.is_empty() {
            return Vec::new();
        }

        let primary_locator = hits.first().map(|hit| {
            hit.label
                .clone()
                .filter(|label| !label.trim().is_empty())
                .unwrap_or_else(|| hit.session_key.clone())
        });

        vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Search(SearchFact {
                domain: SearchDomain::Session,
                query: None,
                result_count: Some(hits.len()),
                primary_locator,
            }),
        }]
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}...")
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past conversation sessions semantically. Returns matching sessions \
         with labels, summaries, and transcript-backed recap snippets. Use this \
         when the user references past discussions, previous decisions, or \
         'what we talked about before'."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 5, max 10)"
                },
                "kind": {
                    "type": "string",
                    "enum": ["web", "channel", "ipc"],
                    "description": "Filter by session kind (optional)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let (result, _) = self.execute_query(&args).await?;
        Ok(result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        let (result, hits) = self.execute_query(&args).await?;
        Ok(ToolExecution {
            facts: self.build_result_facts(&hits),
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;
    use synapse_domain::domain::conversation::{
        ConversationEvent, ConversationKind, ConversationSession, EventType,
    };
    use synapse_domain::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, Entity, HybridSearchResult, MemoryCategory,
        MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, SearchResult, SessionId,
        Skill, SkillUpdate, TemporalFact, Visibility,
    };
    use synapse_domain::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        WorkingMemoryPort,
    };

    #[derive(Default)]
    struct TestStore {
        sessions: Vec<ConversationSession>,
        events: HashMap<String, Vec<ConversationEvent>>,
    }

    #[async_trait]
    impl ConversationStorePort for TestStore {
        async fn get_session(&self, key: &str) -> Option<ConversationSession> {
            self.sessions
                .iter()
                .find(|session| session.key == key)
                .cloned()
        }

        async fn list_sessions(&self, _prefix: Option<&str>) -> Vec<ConversationSession> {
            self.sessions.clone()
        }

        async fn upsert_session(&self, _session: &ConversationSession) -> anyhow::Result<()> {
            Ok(())
        }

        async fn delete_session(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn touch_session(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn append_event(
            &self,
            _session_key: &str,
            _event: &ConversationEvent,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn get_events(&self, session_key: &str, _limit: usize) -> Vec<ConversationEvent> {
            self.events.get(session_key).cloned().unwrap_or_default()
        }

        async fn clear_events(&self, _session_key: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn update_label(&self, _key: &str, _label: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn update_goal(&self, _key: &str, _goal: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn increment_message_count(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn add_token_usage(
            &self,
            _key: &str,
            _input: i64,
            _output: i64,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn get_summary(&self, _key: &str) -> Option<String> {
            None
        }

        async fn set_summary(&self, _key: &str, _summary: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct TestMemory;

    #[async_trait]
    impl WorkingMemoryPort for TestMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
            Ok(vec![])
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
    impl EpisodicMemoryPort for TestMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SemanticMemoryPort for TestMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
            Ok(None)
        }
        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
            Ok(vec![])
        }
        async fn traverse(
            &self,
            _: &MemoryId,
            _: usize,
        ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
            Ok(vec![])
        }
        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SkillMemoryPort for TestMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(vec![])
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
    impl ReflectionPort for TestMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_relevant_reflections(
            &self,
            _: &MemoryQuery,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
        async fn get_failure_patterns(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl ConsolidationPort for TestMemory {
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
    impl UnifiedMemoryPort for TestMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(HybridSearchResult::default())
        }
        async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(test_embedding(text))
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
        async fn recall(
            &self,
            _: &str,
            _: usize,
            _: Option<&str>,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
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
            Ok(vec![])
        }
        fn should_skip_autosave(&self, _: &str) -> bool {
            false
        }
        async fn count(&self) -> Result<usize, MemoryError> {
            Ok(0)
        }
        fn name(&self) -> &str {
            "test"
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

    fn session(key: &str, label: &str, summary: &str, last_active: u64) -> ConversationSession {
        ConversationSession {
            key: key.into(),
            kind: ConversationKind::Web,
            label: Some(label.into()),
            summary: Some(summary.into()),
            current_goal: None,
            created_at: 0,
            last_active,
            message_count: 4,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    fn event(event_type: EventType, content: &str) -> ConversationEvent {
        ConversationEvent {
            event_type,
            actor: "user".into(),
            content: content.into(),
            tool_name: None,
            run_id: None,
            input_tokens: None,
            output_tokens: None,
            timestamp: 1,
        }
    }

    #[tokio::test]
    async fn finds_transcript_only_match() {
        let mut store = TestStore::default();
        store
            .sessions
            .push(session("web:one", "Infra", "Deployment notes", 10));
        store.events.insert(
            "web:one".into(),
            vec![
                event(EventType::User, "We should move weather cache to Redis"),
                event(
                    EventType::Assistant,
                    "Agreed, weather cache belongs in Redis.",
                ),
            ],
        );

        let tool = SessionSearchTool::new(Arc::new(TestMemory), Arc::new(store));
        let result = tool
            .execute(serde_json::json!({"query": "weather cache"}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Recent transcript match"));
        assert!(result.output.contains("weather cache"));
    }

    #[tokio::test]
    async fn execute_with_facts_emits_session_entities() {
        let mut store = TestStore::default();
        store.sessions.push(session(
            "web:one",
            "Weather Thread",
            "Compared Berlin and Tbilisi",
            10,
        ));
        store.events.insert(
            "web:one".into(),
            vec![event(
                EventType::Assistant,
                "We compared Berlin and Tbilisi weather forecasts.",
            )],
        );

        let tool = SessionSearchTool::new(Arc::new(TestMemory), Arc::new(store));
        let execution = tool
            .execute_with_facts(serde_json::json!({"query": "weather forecast"}))
            .await
            .unwrap();

        assert!(execution.result.success);
        assert_eq!(execution.facts.len(), 1);
        assert_eq!(execution.facts[0].tool_id, "session_search");
        assert!(execution.facts[0]
            .projected_focus_entities()
            .iter()
            .any(|entity| entity.kind == "session" && entity.name == "Weather Thread"));
    }

    fn test_embedding(text: &str) -> Vec<f32> {
        let lowered = text.to_lowercase();
        let mut vec = vec![0.0f32; 4];
        for token in lowered.split(|c: char| !c.is_alphanumeric()) {
            match token {
                "weather" | "forecast" => vec[0] += 1.0,
                "cache" | "redis" => vec[1] += 1.0,
                "deploy" | "release" | "ship" | "rollout" => vec[2] += 1.0,
                "prod" | "production" => vec[3] += 1.0,
                _ => {}
            }
        }
        vec
    }
}
