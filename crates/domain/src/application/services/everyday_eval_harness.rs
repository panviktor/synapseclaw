//! Everyday intelligence eval harness.
//!
//! This keeps common assistant cases measurable and independent from the chat
//! model. It exercises bounded interpretation, resolution routing, and
//! clarification guidance with deterministic inputs.

use crate::application::services::clarification_policy::{
    self, ClarificationGuidance,
};
use crate::application::services::resolution_router::{self, ResolutionPlan, ResolutionSource};
use crate::application::services::turn_interpretation::{self, DefaultKind, TurnInterpretation};
use crate::domain::conversation_target::CurrentConversationContext;
use crate::domain::dialogue_state::DialogueState;
use crate::domain::user_profile::UserProfile;
use crate::ports::memory::UnifiedMemoryPort;

#[derive(Debug, Clone)]
pub struct EverydayEvalScenario {
    pub id: &'static str,
    pub user_message: &'static str,
    pub profile: Option<UserProfile>,
    pub current_conversation: Option<CurrentConversationContext>,
    pub dialogue_state: Option<DialogueState>,
    pub top_session_score: Option<f64>,
    pub second_session_score: Option<f64>,
    pub top_recipe_score: Option<i64>,
    pub second_recipe_score: Option<i64>,
    pub top_memory_score: Option<f64>,
    pub second_memory_score: Option<f64>,
    pub recall_hits: usize,
    pub skill_hits: usize,
    pub entity_hits: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClarificationShape {
    None,
    DefaultsOnly,
    CandidateSet,
    GenericRisk,
}

#[derive(Debug, Clone)]
pub struct EverydayEvalResult {
    pub scenario_id: &'static str,
    pub interpretation: Option<TurnInterpretation>,
    pub resolution_plan: ResolutionPlan,
    pub clarification_guidance: Option<ClarificationGuidance>,
    pub selected_source: Option<ResolutionSource>,
    pub defaults_used: Vec<DefaultKind>,
    pub used_session_history: bool,
    pub used_run_recipe: bool,
    pub clarification_shape: ClarificationShape,
}

pub async fn evaluate_scenario(
    memory: &dyn UnifiedMemoryPort,
    scenario: &EverydayEvalScenario,
) -> EverydayEvalResult {
    let interpretation = turn_interpretation::build_turn_interpretation(
        Some(memory),
        scenario.user_message,
        scenario.profile.clone(),
        scenario.current_conversation.as_ref(),
        scenario.dialogue_state.as_ref(),
    )
    .await;

    let resolution_plan = resolution_router::build_resolution_plan(
        resolution_router::ResolutionEvidence {
            interpretation: interpretation.as_ref(),
            top_session_score: scenario.top_session_score,
            second_session_score: scenario.second_session_score,
            top_recipe_score: scenario.top_recipe_score,
            second_recipe_score: scenario.second_recipe_score,
            top_memory_score: scenario.top_memory_score,
            second_memory_score: scenario.second_memory_score,
            recall_hits: scenario.recall_hits,
            skill_hits: scenario.skill_hits,
            entity_hits: scenario.entity_hits,
        },
    );
    let clarification_guidance =
        clarification_policy::build_clarification_guidance(
            Some(&resolution_plan),
            interpretation.as_ref(),
        );

    EverydayEvalResult {
        scenario_id: scenario.id,
        selected_source: resolution_plan.source_order.first().copied(),
        defaults_used: interpretation
            .as_ref()
            .map(|i| i.defaults_requested.clone())
            .unwrap_or_default(),
        used_session_history: resolution_plan
            .source_order
            .contains(&ResolutionSource::SessionHistory),
        used_run_recipe: resolution_plan
            .source_order
            .contains(&ResolutionSource::RunRecipe),
        clarification_shape: classify_clarification_shape(
            clarification_guidance.as_ref(),
            &resolution_plan,
        ),
        interpretation,
        resolution_plan,
        clarification_guidance,
    }
}

pub fn default_golden_scenarios() -> Vec<EverydayEvalScenario> {
    vec![
        EverydayEvalScenario {
            id: "weather_uses_default_city",
            user_message: "What's the weather?",
            profile: Some(UserProfile {
                default_city: Some("Berlin".into()),
                ..Default::default()
            }),
            current_conversation: None,
            dialogue_state: None,
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: Some(0.84),
            second_memory_score: Some(0.71),
            recall_hits: 1,
            skill_hits: 0,
            entity_hits: 0,
        },
        EverydayEvalScenario {
            id: "translate_uses_preferred_language",
            user_message: "Translate it to my language",
            profile: Some(UserProfile {
                preferred_language: Some("ru".into()),
                ..Default::default()
            }),
            current_conversation: None,
            dialogue_state: None,
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: Some(0.69),
            second_memory_score: Some(0.52),
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 1,
        },
        EverydayEvalScenario {
            id: "reminder_uses_timezone",
            user_message: "Remind me tomorrow",
            profile: Some(UserProfile {
                timezone: Some("Europe/Berlin".into()),
                ..Default::default()
            }),
            current_conversation: None,
            dialogue_state: None,
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: None,
            second_memory_score: None,
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 0,
        },
        EverydayEvalScenario {
            id: "deliver_here_prefers_current_conversation",
            user_message: "Send it to our chat",
            profile: None,
            current_conversation: Some(CurrentConversationContext {
                source_adapter: "matrix".into(),
                conversation_ref: "!room:example".into(),
                reply_ref: "!room:example".into(),
                thread_ref: None,
                actor_id: "@victor:example".into(),
            }),
            dialogue_state: None,
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: None,
            second_memory_score: None,
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 0,
        },
        EverydayEvalScenario {
            id: "history_lookup_prefers_session_history",
            user_message: "What did we discuss last week?",
            profile: None,
            current_conversation: None,
            dialogue_state: None,
            top_session_score: Some(2.3),
            second_session_score: Some(1.1),
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: Some(0.58),
            second_memory_score: Some(0.54),
            recall_hits: 1,
            skill_hits: 0,
            entity_hits: 0,
        },
        EverydayEvalScenario {
            id: "repeat_work_prefers_recipe",
            user_message: "Do it like last time",
            profile: None,
            current_conversation: None,
            dialogue_state: None,
            top_session_score: Some(1.7),
            second_session_score: Some(0.9),
            top_recipe_score: Some(240),
            second_recipe_score: Some(150),
            top_memory_score: Some(0.61),
            second_memory_score: Some(0.59),
            recall_hits: 1,
            skill_hits: 1,
            entity_hits: 0,
        },
        EverydayEvalScenario {
            id: "second_one_uses_dialogue_state",
            user_message: "The second one",
            profile: None,
            current_conversation: None,
            dialogue_state: Some(DialogueState {
                comparison_set: vec![
                    crate::domain::dialogue_state::FocusEntity {
                        kind: "city".into(),
                        name: "Berlin".into(),
                        metadata: None,
                    },
                    crate::domain::dialogue_state::FocusEntity {
                        kind: "city".into(),
                        name: "Tbilisi".into(),
                        metadata: None,
                    },
                ],
                focus_entities: vec![],
                slots: vec![],
                last_tool_subjects: vec!["Berlin".into(), "Tbilisi".into()],
                updated_at: 1,
            }),
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: None,
            second_memory_score: None,
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 0,
        },
    ]
}

fn classify_clarification_shape(
    guidance: Option<&ClarificationGuidance>,
    _plan: &ResolutionPlan,
) -> ClarificationShape {
    match guidance {
        Some(guidance) if !guidance.candidate_set.is_empty() => ClarificationShape::CandidateSet,
        Some(guidance) if !guidance.use_defaults_for.is_empty() => ClarificationShape::DefaultsOnly,
        Some(guidance) if guidance.required => ClarificationShape::GenericRisk,
        Some(_) => ClarificationShape::None,
        None => ClarificationShape::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::application::services::turn_interpretation::InterpretationHintKind;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingDistanceMetric,
        EmbeddingProfile, Entity, HybridSearchResult, MemoryCategory, MemoryEntry, MemoryError,
        MemoryId, MemoryQuery, Reflection, SearchResult, SessionId, Skill, SkillUpdate,
        TemporalFact, Visibility,
    };
    use crate::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        WorkingMemoryPort,
    };
    use async_trait::async_trait;

    #[derive(Default)]
    struct StubMemory;

    #[async_trait]
    impl WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(
            &self,
            _: &AgentId,
        ) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
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
        async fn append_core_block(&self, _: &AgentId, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_recent(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
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
    impl SemanticMemoryPort for StubMemory {
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
        async fn get_current_facts(
            &self,
            _: &MemoryId,
        ) -> Result<Vec<TemporalFact>, MemoryError> {
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
    impl SkillMemoryPort for StubMemory {
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
        async fn get_skill(
            &self,
            _: &str,
            _: &AgentId,
        ) -> Result<Option<Skill>, MemoryError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl ReflectionPort for StubMemory {
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
    impl ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
            Ok(ConsolidationReport {
                episodes_processed: 0,
                entities_extracted: 0,
                facts_created: 0,
                facts_invalidated: 0,
                skills_generated: 0,
                entries_garbage_collected: 0,
            })
        }
        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
            Ok(0)
        }
        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl crate::ports::memory::UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(HybridSearchResult::default())
        }

        async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(embed_text(text))
        }

        async fn embed_query(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(embed_text(text))
        }

        async fn embed_document(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(embed_text(text))
        }

        fn embedding_profile(&self) -> EmbeddingProfile {
            EmbeddingProfile {
                profile_id: "eval_stub".into(),
                provider_family: "test".into(),
                model_id: "eval_stub".into(),
                distance_metric: EmbeddingDistanceMetric::Cosine,
                dimensions: 8,
                normalize_output: true,
                supports_multilingual: true,
                supports_code: false,
                query_prefix: None,
                document_prefix: None,
                recommended_chunk_chars: 512,
                recommended_top_k: 6,
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

        async fn get(
            &self,
            _: &str,
            _: &AgentId,
        ) -> Result<Option<MemoryEntry>, MemoryError> {
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
            "eval_stub"
        }

        async fn health_check(&self) -> bool {
            true
        }

        async fn promote_visibility(
            &self,
            _entry_id: &MemoryId,
            _visibility: &Visibility,
            _shared_with: &[AgentId],
            _agent_id: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn embed_text(text: &str) -> Vec<f32> {
        let normalized = text.to_lowercase();
        let mut vector = vec![0.0f32; 8];
        let features = [
            (0, &["discuss", "conversation", "past", "previous", "last week"][..]),
            (1, &["like last time", "repeat", "same as before", "successful way"][..]),
            (2, &["here", "this chat", "current conversation", "our chat"][..]),
            (3, &["second one", "that one", "this one"][..]),
            (4, &["language", "translate", "preferred language", "my language"][..]),
            (5, &["timezone", "remind", "tomorrow"][..]),
            (6, &["weather", "city", "default city"][..]),
            (7, &["default destination", "usual chat", "send it"][..]),
        ];
        for (index, keywords) in features {
            if keywords.iter().any(|keyword| normalized.contains(keyword)) {
                vector[index] = 1.0;
            }
        }
        vector
    }

    #[tokio::test]
    async fn default_city_scenario_prefers_profile_without_generic_clarify() {
        let memory = StubMemory;
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "weather_uses_default_city")
            .unwrap();

        let result = evaluate_scenario(&memory, &scenario).await;
        assert_eq!(result.selected_source, Some(ResolutionSource::UserProfile));
        assert_eq!(result.defaults_used, vec![DefaultKind::City]);
        assert_ne!(result.clarification_shape, ClarificationShape::GenericRisk);
    }

    #[tokio::test]
    async fn history_scenario_prefers_session_history() {
        let memory = StubMemory;
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "history_lookup_prefers_session_history")
            .unwrap();

        let result = evaluate_scenario(&memory, &scenario).await;
        assert!(result
            .interpretation
            .as_ref()
            .unwrap()
            .has_hint(InterpretationHintKind::HistoryLookup));
        assert_eq!(result.selected_source, Some(ResolutionSource::SessionHistory));
        assert!(result.used_session_history);
    }

    #[tokio::test]
    async fn repeat_work_scenario_prefers_recipe() {
        let memory = StubMemory;
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "repeat_work_prefers_recipe")
            .unwrap();

        let result = evaluate_scenario(&memory, &scenario).await;
        assert_eq!(result.selected_source, Some(ResolutionSource::RunRecipe));
        assert!(result.used_run_recipe);
    }

    #[tokio::test]
    async fn second_one_scenario_uses_candidate_set() {
        let memory = StubMemory;
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "second_one_uses_dialogue_state")
            .unwrap();

        let result = evaluate_scenario(&memory, &scenario).await;
        assert_eq!(result.selected_source, Some(ResolutionSource::DialogueState));
        assert_eq!(result.clarification_shape, ClarificationShape::CandidateSet);
        assert_eq!(
            result
                .clarification_guidance
                .as_ref()
                .unwrap()
                .candidate_set,
            vec!["Berlin", "Tbilisi"]
        );
    }

    #[tokio::test]
    async fn deliver_here_scenario_prefers_current_conversation() {
        let memory = StubMemory;
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "deliver_here_prefers_current_conversation")
            .unwrap();

        let result = evaluate_scenario(&memory, &scenario).await;
        assert_eq!(
            result.selected_source,
            Some(ResolutionSource::CurrentConversation)
        );
    }

    #[tokio::test]
    async fn translate_scenario_uses_language_default() {
        let memory = StubMemory;
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "translate_uses_preferred_language")
            .unwrap();

        let result = evaluate_scenario(&memory, &scenario).await;
        assert!(result.defaults_used.contains(&DefaultKind::Language));
    }

    #[tokio::test]
    async fn default_delivery_target_scenario_can_be_represented() {
        let scenario = EverydayEvalScenario {
            id: "default_delivery_target",
            user_message: "Send it to my usual chat",
            profile: Some(UserProfile {
                default_delivery_target: Some(ConversationDeliveryTarget::CurrentConversation),
                ..Default::default()
            }),
            current_conversation: None,
            dialogue_state: None,
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: None,
            second_memory_score: None,
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 0,
        };

        let result = evaluate_scenario(&StubMemory, &scenario).await;
        assert!(result.defaults_used.contains(&DefaultKind::DeliveryTarget));
    }
}
