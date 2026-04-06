//! Precedent search tool — search prior successful execution patterns.
//!
//! Uses the shared retrieval backbone so recipe/precedent lookup is consistent
//! with the rest of Phase 4.8 retrieval.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::application::services::retrieval_service;
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::ports::agent_runtime::AgentToolFact;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;
use synapse_domain::ports::tool::{Tool, ToolExecution, ToolResult};

pub struct PrecedentSearchTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    store: Arc<dyn RunRecipeStorePort>,
    agent_id: String,
}

impl PrecedentSearchTool {
    pub fn new(
        memory: Arc<dyn UnifiedMemoryPort>,
        store: Arc<dyn RunRecipeStorePort>,
        agent_id: String,
    ) -> Self {
        Self {
            memory,
            store,
            agent_id,
        }
    }

    async fn execute_query(
        &self,
        args: &serde_json::Value,
    ) -> anyhow::Result<(ToolResult, Vec<retrieval_service::RunRecipeSearchMatch>)> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .min(5) as usize;

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

        let hits = retrieval_service::search_run_recipes(
            self.memory.as_ref(),
            self.store.as_ref(),
            &self.agent_id,
            query,
            limit,
        )
        .await;

        if hits.is_empty() {
            return Ok((
                ToolResult {
                    output: format!("No successful precedents found matching '{query}'"),
                    success: true,
                    error: None,
                },
                hits,
            ));
        }

        let mut output = format!("Found {} precedent(s) matching '{query}':\n\n", hits.len());
        for (index, hit) in hits.iter().enumerate() {
            output.push_str(&format!(
                "{}. **{}** (score {}, successes {})\n",
                index + 1,
                hit.task_family,
                hit.score,
                hit.success_count
            ));
            output.push_str(&format!("   Summary: {}\n", hit.summary));
            output.push_str(&format!("   Sample request: {}\n", hit.sample_request));
            if !hit.tool_pattern.is_empty() {
                output.push_str(&format!(
                    "   Tool pattern: {}\n",
                    hit.tool_pattern.join(", ")
                ));
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
        hits: &[retrieval_service::RunRecipeSearchMatch],
    ) -> Vec<AgentToolFact> {
        if hits.is_empty() {
            return Vec::new();
        }

        vec![AgentToolFact {
            tool_name: self.name().to_string(),
            focus_entities: hits
                .iter()
                .take(3)
                .map(|hit| FocusEntity {
                    kind: "run_recipe".into(),
                    name: hit.task_family.clone(),
                    metadata: Some(format!(
                        "successes={};score={};tools={}",
                        hit.success_count,
                        hit.score,
                        hit.tool_pattern.join(",")
                    )),
                })
                .collect(),
            slots: Vec::new(),
        }]
    }
}

#[async_trait]
impl Tool for PrecedentSearchTool {
    fn name(&self) -> &str {
        "precedent_search"
    }

    fn description(&self) -> &str {
        "Search prior successful execution patterns and reusable recipes. \
         Use when the user asks to do something like before, repeat a prior \
         approach, or find the last successful pattern for a task."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Task description or request to match against prior successful work"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 3, max 5)"
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
            result,
            facts: self.build_result_facts(&hits),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use synapse_domain::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, Entity, HybridSearchResult, MemoryCategory,
        MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, SearchResult, SessionId,
        Skill, SkillUpdate, TemporalFact, Visibility,
    };
    use synapse_domain::domain::run_recipe::RunRecipe;
    use synapse_domain::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        WorkingMemoryPort,
    };
    use synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore;

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

    #[tokio::test]
    async fn finds_semantically_similar_precedent() {
        let store = InMemoryRunRecipeStore::new();
        store
            .upsert(RunRecipe {
                agent_id: "agent".into(),
                task_family: "deploy".into(),
                sample_request: "deploy the latest release to production".into(),
                summary: "Check staging first, then ship the release".into(),
                tool_pattern: vec!["shell".into(), "git".into()],
                success_count: 3,
                updated_at: 10,
            })
            .unwrap();

        let tool = PrecedentSearchTool::new(Arc::new(TestMemory), Arc::new(store), "agent".into());

        let result = tool
            .execute(serde_json::json!({"query": "ship it to prod"}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("deploy"));
        assert!(result.output.contains("Tool pattern: shell, git"));
    }

    #[tokio::test]
    async fn execute_with_facts_emits_recipe_entities() {
        let store = InMemoryRunRecipeStore::new();
        store
            .upsert(RunRecipe {
                agent_id: "agent".into(),
                task_family: "deploy".into(),
                sample_request: "deploy the latest release to production".into(),
                summary: "Check staging first, then ship the release".into(),
                tool_pattern: vec!["shell".into(), "git".into()],
                success_count: 3,
                updated_at: 10,
            })
            .unwrap();

        let tool = PrecedentSearchTool::new(Arc::new(TestMemory), Arc::new(store), "agent".into());
        let execution = tool
            .execute_with_facts(serde_json::json!({"query": "ship it to prod"}))
            .await
            .unwrap();

        assert!(execution.result.success);
        assert_eq!(execution.facts.len(), 1);
        assert_eq!(execution.facts[0].tool_name, "precedent_search");
        assert!(execution.facts[0]
            .focus_entities
            .iter()
            .any(|entity| entity.kind == "run_recipe" && entity.name == "deploy"));
    }

    fn test_embedding(text: &str) -> Vec<f32> {
        let lowered = text.to_lowercase();
        let mut vec = vec![0.0f32; 4];
        for token in lowered.split(|c: char| !c.is_alphanumeric()) {
            match token {
                "deploy" | "release" | "rollout" | "ship" => vec[0] += 1.0,
                "prod" | "production" => vec[1] += 1.0,
                "weather" | "forecast" => vec[2] += 1.0,
                "berlin" | "city" => vec[3] += 1.0,
                _ => {}
            }
        }
        vec
    }
}
