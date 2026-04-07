//! Deterministic self-learning eval harness.
//!
//! This keeps the early Phase 4.9 learning path measurable without relying on
//! the chat model. It exercises typed evidence, candidate formation, mutation
//! candidates, and safe profile patching.

use crate::application::services::learning_candidate_service::{self, LearningCandidate};
use crate::application::services::learning_evidence_service;
use crate::application::services::learning_quality_service;
use crate::application::services::recipe_evolution_service;
use crate::application::services::skill_promotion_service;
use crate::application::services::user_profile_service;
use crate::domain::dialogue_state::FocusEntity;
use crate::domain::memory::Skill;
use crate::domain::run_recipe::RunRecipe;
use crate::domain::tool_fact::{
    DeliveryFact, DeliveryTargetKind, FocusFact, OutcomeStatus, ProfileOperation, ResourceFact,
    ResourceKind, ResourceMetadata, ResourceOperation, SearchDomain, SearchFact, ToolFactPayload,
    TypedToolFact, UserProfileFact, UserProfileField,
};
use crate::domain::user_profile::UserProfile;

#[derive(Debug, Clone)]
pub struct SelfLearningEvalScenario {
    pub id: &'static str,
    pub user_message: &'static str,
    pub assistant_response: &'static str,
    pub current_profile: Option<UserProfile>,
    pub existing_recipes: Vec<RunRecipe>,
    pub existing_skills: Vec<Skill>,
    pub tool_facts: Vec<TypedToolFact>,
}

#[derive(Debug, Clone)]
pub struct SelfLearningEvalResult {
    pub scenario_id: &'static str,
    pub typed_fact_count: usize,
    pub candidate_kinds: Vec<&'static str>,
    pub assessment_reasons: Vec<&'static str>,
    pub user_profile_candidate_count: usize,
    pub precedent_candidate_count: usize,
    pub run_recipe_candidate_count: usize,
    pub failure_pattern_candidate_count: usize,
    pub accepted_candidate_kinds: Vec<&'static str>,
    pub accepted_run_recipe_count: usize,
    pub accepted_failure_pattern_count: usize,
    pub skill_promotion_reasons: Vec<&'static str>,
    pub accepted_skill_promotion_count: usize,
    pub mutation_candidate_count: usize,
    pub profile_patch_is_noop: bool,
    pub profile_projection: Option<String>,
}

pub fn evaluate_scenario(scenario: &SelfLearningEvalScenario) -> SelfLearningEvalResult {
    let evidence = learning_evidence_service::build_learning_evidence(&scenario.tool_facts);
    let candidates = learning_candidate_service::build_learning_candidates(
        scenario.user_message,
        scenario.assistant_response,
        &scenario.tool_facts,
        &evidence,
    );
    let assessments = learning_quality_service::assess_learning_candidates(
        &candidates,
        &evidence,
        &scenario.existing_recipes,
    );
    let mutation_candidates =
        learning_candidate_service::build_mutation_candidates_from_assessments(&assessments);
    let skill_promotion_assessments = build_skill_promotion_assessments(scenario, &assessments);
    let patch = learning_candidate_service::build_user_profile_patch(
        &candidates,
        scenario.current_profile.as_ref(),
    );
    let projected_profile =
        user_profile_service::apply_patch(scenario.current_profile.clone(), &patch);

    SelfLearningEvalResult {
        scenario_id: scenario.id,
        typed_fact_count: evidence.typed_fact_count,
        candidate_kinds: candidate_kind_names(&candidates),
        assessment_reasons: assessment_reason_names(&assessments),
        user_profile_candidate_count: count_candidate_kind(&candidates, candidate_is_user_profile),
        precedent_candidate_count: count_candidate_kind(&candidates, candidate_is_precedent),
        run_recipe_candidate_count: count_candidate_kind(&candidates, candidate_is_run_recipe),
        failure_pattern_candidate_count: count_candidate_kind(
            &candidates,
            candidate_is_failure_pattern,
        ),
        accepted_candidate_kinds: accepted_candidate_kind_names(&assessments),
        accepted_run_recipe_count: assessments
            .iter()
            .filter(|assessment| {
                assessment.accepted
                    && matches!(assessment.candidate, LearningCandidate::RunRecipe(_))
            })
            .count(),
        accepted_failure_pattern_count: assessments
            .iter()
            .filter(|assessment| {
                assessment.accepted
                    && matches!(assessment.candidate, LearningCandidate::FailurePattern(_))
            })
            .count(),
        skill_promotion_reasons: skill_promotion_reason_names(&skill_promotion_assessments),
        accepted_skill_promotion_count: skill_promotion_assessments
            .iter()
            .filter(|assessment| assessment.accepted)
            .count(),
        mutation_candidate_count: mutation_candidates.len(),
        profile_patch_is_noop: patch.is_noop(),
        profile_projection: projected_profile
            .as_ref()
            .map(user_profile_service::format_profile_projection),
    }
}

pub fn default_golden_scenarios() -> Vec<SelfLearningEvalScenario> {
    vec![
        SelfLearningEvalScenario {
            id: "profile_only_update_stays_profile_only",
            user_message: "Remember my timezone",
            assistant_response: "Saved your timezone.",
            current_profile: None,
            existing_recipes: Vec::new(),
            existing_skills: Vec::new(),
            tool_facts: vec![TypedToolFact {
                tool_id: "user_profile".into(),
                payload: ToolFactPayload::UserProfile(UserProfileFact {
                    field: UserProfileField::Timezone,
                    operation: ProfileOperation::Set,
                    value: Some("Europe/Berlin".into()),
                }),
            }],
        },
        SelfLearningEvalScenario {
            id: "procedural_turn_forms_precedent_and_recipe",
            user_message: "Find the status page and send it to the chat",
            assistant_response: "Fetched the page and sent it.",
            current_profile: None,
            existing_recipes: Vec::new(),
            existing_skills: Vec::new(),
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "web_search".into(),
                    payload: ToolFactPayload::Search(SearchFact {
                        domain: SearchDomain::Web,
                        query: Some("status page".into()),
                        result_count: Some(3),
                        primary_locator: Some("https://status.example.com".into()),
                    }),
                },
                TypedToolFact {
                    tool_id: "web_fetch".into(),
                    payload: ToolFactPayload::Focus(FocusFact {
                        entities: vec![FocusEntity {
                            kind: "service".into(),
                            name: "status.example.com".into(),
                            metadata: None,
                        }],
                        subjects: vec!["status.example.com".into()],
                    }),
                },
                TypedToolFact {
                    tool_id: "message_send".into(),
                    payload: ToolFactPayload::Delivery(DeliveryFact {
                        target: DeliveryTargetKind::CurrentConversation,
                        content_bytes: Some(24),
                    }),
                },
            ],
        },
        SelfLearningEvalScenario {
            id: "known_environment_merges_with_existing_profile",
            user_message: "Remember staging too",
            assistant_response: "Saved staging.",
            current_profile: Some(UserProfile {
                known_environments: vec!["prod".into()],
                ..Default::default()
            }),
            existing_recipes: Vec::new(),
            existing_skills: Vec::new(),
            tool_facts: vec![TypedToolFact {
                tool_id: "user_profile".into(),
                payload: ToolFactPayload::UserProfile(UserProfileFact {
                    field: UserProfileField::KnownEnvironments,
                    operation: ProfileOperation::Set,
                    value: Some("staging".into()),
                }),
            }],
        },
        SelfLearningEvalScenario {
            id: "similar_existing_recipe_merges",
            user_message: "Find the status page and send it again",
            assistant_response: "Fetched the page and sent it again.",
            current_profile: None,
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                sample_request: "find the status page and send it".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                success_count: 2,
                updated_at: 1,
            }],
            existing_skills: Vec::new(),
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "web_search".into(),
                    payload: ToolFactPayload::Search(SearchFact {
                        domain: SearchDomain::Web,
                        query: Some("status page".into()),
                        result_count: Some(2),
                        primary_locator: Some("https://status.example.com".into()),
                    }),
                },
                TypedToolFact {
                    tool_id: "message_send".into(),
                    payload: ToolFactPayload::Delivery(DeliveryFact {
                        target: DeliveryTargetKind::CurrentConversation,
                        content_bytes: Some(24),
                    }),
                },
            ],
        },
        SelfLearningEvalScenario {
            id: "diverged_recipe_is_not_auto_accepted",
            user_message: "Send the latest backup report",
            assistant_response: "Created a shell backup and sent it.",
            current_profile: None,
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "resource_delivery".into(),
                sample_request: "fetch a page and send it".into(),
                summary: "pattern=web_fetch -> browser_open".into(),
                tool_pattern: vec!["web_fetch".into(), "browser_open".into()],
                success_count: 4,
                updated_at: 1,
            }],
            existing_skills: Vec::new(),
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "shell".into(),
                    payload: ToolFactPayload::Resource(ResourceFact {
                        kind: ResourceKind::BackupSnapshot,
                        operation: ResourceOperation::Snapshot,
                        locator: "nightly-backup".into(),
                        host: None,
                        metadata: ResourceMetadata::default(),
                    }),
                },
                TypedToolFact {
                    tool_id: "message_send".into(),
                    payload: ToolFactPayload::Delivery(DeliveryFact {
                        target: DeliveryTargetKind::CurrentConversation,
                        content_bytes: Some(32),
                    }),
                },
            ],
        },
        SelfLearningEvalScenario {
            id: "failed_turn_forms_failure_pattern_only",
            user_message: "Fetch the status page",
            assistant_response: "I could not complete that.",
            current_profile: None,
            existing_recipes: Vec::new(),
            existing_skills: Vec::new(),
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "web_fetch".into(),
                    payload: ToolFactPayload::Resource(ResourceFact {
                        kind: ResourceKind::WebResource,
                        operation: ResourceOperation::Fetch,
                        locator: "https://status.example.com".into(),
                        host: Some("status.example.com".into()),
                        metadata: ResourceMetadata::default(),
                    }),
                },
                TypedToolFact::outcome("web_fetch", OutcomeStatus::RuntimeError, Some(220)),
            ],
        },
        SelfLearningEvalScenario {
            id: "strong_repeated_recipe_promotes_skill_candidate",
            user_message: "Find the status page and send it again",
            assistant_response: "Fetched the page and sent it again.",
            current_profile: None,
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                sample_request: "find the status page and send it".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                success_count: 2,
                updated_at: 1,
            }],
            existing_skills: Vec::new(),
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "web_search".into(),
                    payload: ToolFactPayload::Search(SearchFact {
                        domain: SearchDomain::Web,
                        query: Some("status page".into()),
                        result_count: Some(2),
                        primary_locator: Some("https://status.example.com".into()),
                    }),
                },
                TypedToolFact {
                    tool_id: "message_send".into(),
                    payload: ToolFactPayload::Delivery(DeliveryFact {
                        target: DeliveryTargetKind::CurrentConversation,
                        content_bytes: Some(24),
                    }),
                },
            ],
        },
    ]
}

fn build_skill_promotion_assessments(
    scenario: &SelfLearningEvalScenario,
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<skill_promotion_service::SkillPromotionAssessment> {
    assessments
        .iter()
        .filter(|assessment| assessment.accepted)
        .filter_map(|assessment| {
            let LearningCandidate::RunRecipe(candidate) = &assessment.candidate else {
                return None;
            };
            let recipe = if assessment.reason == "merge_existing_recipe" {
                scenario
                    .existing_recipes
                    .iter()
                    .find(|existing| existing.task_family == candidate.task_family_hint)
                    .map(|existing| {
                        recipe_evolution_service::merge_existing_recipe(existing, candidate, 1)
                    })
                    .unwrap_or_else(|| {
                        recipe_evolution_service::build_new_recipe("agent", candidate, 1)
                    })
            } else {
                recipe_evolution_service::build_new_recipe("agent", candidate, 1)
            };
            let skill_name = skill_promotion_service::build_skill_name(&recipe);
            let existing_skill = scenario
                .existing_skills
                .iter()
                .find(|skill| skill.name == skill_name);
            Some(skill_promotion_service::assess_recipe_for_skill_promotion(
                &recipe,
                existing_skill,
            ))
        })
        .collect()
}

fn candidate_kind_names(candidates: &[LearningCandidate]) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for candidate in candidates {
        let kind = match candidate {
            LearningCandidate::UserProfile(_) => "user_profile",
            LearningCandidate::Precedent(_) => "precedent",
            LearningCandidate::RunRecipe(_) => "run_recipe",
            LearningCandidate::FailurePattern(_) => "failure_pattern",
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

fn count_candidate_kind(
    candidates: &[LearningCandidate],
    predicate: fn(&LearningCandidate) -> bool,
) -> usize {
    candidates
        .iter()
        .filter(|candidate| predicate(candidate))
        .count()
}

fn candidate_is_user_profile(candidate: &LearningCandidate) -> bool {
    matches!(candidate, LearningCandidate::UserProfile(_))
}

fn candidate_is_precedent(candidate: &LearningCandidate) -> bool {
    matches!(candidate, LearningCandidate::Precedent(_))
}

fn candidate_is_run_recipe(candidate: &LearningCandidate) -> bool {
    matches!(candidate, LearningCandidate::RunRecipe(_))
}

fn candidate_is_failure_pattern(candidate: &LearningCandidate) -> bool {
    matches!(candidate, LearningCandidate::FailurePattern(_))
}

fn accepted_candidate_kind_names(
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for assessment in assessments {
        if !assessment.accepted {
            continue;
        }
        let kind = match assessment.candidate {
            LearningCandidate::UserProfile(_) => "user_profile",
            LearningCandidate::Precedent(_) => "precedent",
            LearningCandidate::RunRecipe(_) => "run_recipe",
            LearningCandidate::FailurePattern(_) => "failure_pattern",
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

fn assessment_reason_names(
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    for assessment in assessments {
        if !reasons.contains(&assessment.reason) {
            reasons.push(assessment.reason);
        }
    }
    reasons
}

fn skill_promotion_reason_names(
    assessments: &[skill_promotion_service::SkillPromotionAssessment],
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    for assessment in assessments {
        if !reasons.contains(&assessment.reason) {
            reasons.push(assessment.reason);
        }
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_only_turn_stays_profile_only() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "profile_only_update_stays_profile_only")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.user_profile_candidate_count, 1);
        assert_eq!(result.precedent_candidate_count, 0);
        assert_eq!(result.run_recipe_candidate_count, 0);
        assert!(!result.profile_patch_is_noop);
        assert!(result
            .profile_projection
            .as_deref()
            .is_some_and(|projection| projection.contains("timezone: Europe/Berlin")));
    }

    #[test]
    fn procedural_turn_forms_precedent_and_recipe() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "procedural_turn_forms_precedent_and_recipe")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.precedent_candidate_count, 1);
        assert_eq!(result.run_recipe_candidate_count, 1);
        assert_eq!(result.accepted_run_recipe_count, 1);
        assert_eq!(result.mutation_candidate_count, 1);
        assert!(result.candidate_kinds.contains(&"precedent"));
        assert!(result.candidate_kinds.contains(&"run_recipe"));
        assert!(result.accepted_candidate_kinds.contains(&"run_recipe"));
    }

    #[test]
    fn profile_patch_merges_known_environments() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "known_environment_merges_with_existing_profile")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        let projection = result.profile_projection.unwrap();
        assert!(projection.contains("known_environments: prod, staging"));
    }

    #[test]
    fn similar_existing_recipe_is_marked_as_merge() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "similar_existing_recipe_merges")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.run_recipe_candidate_count, 1);
        assert_eq!(result.accepted_run_recipe_count, 1);
        assert!(result.accepted_candidate_kinds.contains(&"run_recipe"));
        assert!(result.assessment_reasons.contains(&"merge_existing_recipe"));
        assert_eq!(result.accepted_skill_promotion_count, 1);
        assert!(result
            .skill_promotion_reasons
            .contains(&"create_candidate_skill"));
    }

    #[test]
    fn diverged_recipe_is_not_auto_accepted() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "diverged_recipe_is_not_auto_accepted")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.run_recipe_candidate_count, 1);
        assert_eq!(result.accepted_run_recipe_count, 0);
        assert!(!result.accepted_candidate_kinds.contains(&"run_recipe"));
    }

    #[test]
    fn failed_turn_forms_failure_pattern_only() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "failed_turn_forms_failure_pattern_only")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.precedent_candidate_count, 0);
        assert_eq!(result.run_recipe_candidate_count, 0);
        assert_eq!(result.failure_pattern_candidate_count, 1);
        assert!(result.candidate_kinds.contains(&"failure_pattern"));
        assert!(result.accepted_candidate_kinds.contains(&"failure_pattern"));
        assert!(result.assessment_reasons.contains(&"typed_failure_pattern"));
        assert_eq!(result.accepted_failure_pattern_count, 1);
        assert_eq!(result.mutation_candidate_count, 1);
    }

    #[test]
    fn repeated_recipe_promotes_candidate_skill() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "strong_repeated_recipe_promotes_skill_candidate")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_run_recipe_count, 1);
        assert_eq!(result.accepted_skill_promotion_count, 1);
        assert!(result
            .skill_promotion_reasons
            .contains(&"create_candidate_skill"));
    }
}
