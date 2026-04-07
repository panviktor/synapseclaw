//! Deterministic self-learning eval harness.
//!
//! This keeps the early Phase 4.9 learning path measurable without relying on
//! the chat model. It exercises typed evidence, candidate formation, mutation
//! candidates, and safe profile patching.

use crate::application::services::learning_candidate_service::{self, LearningCandidate};
use crate::application::services::learning_evidence_service;
use crate::application::services::user_profile_service;
use crate::domain::dialogue_state::FocusEntity;
use crate::domain::tool_fact::{
    DeliveryFact, DeliveryTargetKind, FocusFact, ProfileOperation, SearchDomain, SearchFact,
    ToolFactPayload, TypedToolFact, UserProfileFact, UserProfileField,
};
use crate::domain::user_profile::UserProfile;

#[derive(Debug, Clone)]
pub struct SelfLearningEvalScenario {
    pub id: &'static str,
    pub user_message: &'static str,
    pub assistant_response: &'static str,
    pub current_profile: Option<UserProfile>,
    pub tool_facts: Vec<TypedToolFact>,
}

#[derive(Debug, Clone)]
pub struct SelfLearningEvalResult {
    pub scenario_id: &'static str,
    pub typed_fact_count: usize,
    pub candidate_kinds: Vec<&'static str>,
    pub user_profile_candidate_count: usize,
    pub precedent_candidate_count: usize,
    pub run_recipe_candidate_count: usize,
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
    let mutation_candidates = learning_candidate_service::build_mutation_candidates(&candidates);
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
        user_profile_candidate_count: count_candidate_kind(&candidates, candidate_is_user_profile),
        precedent_candidate_count: count_candidate_kind(&candidates, candidate_is_precedent),
        run_recipe_candidate_count: count_candidate_kind(&candidates, candidate_is_run_recipe),
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
            tool_facts: vec![TypedToolFact {
                tool_id: "user_profile".into(),
                payload: ToolFactPayload::UserProfile(UserProfileFact {
                    field: UserProfileField::KnownEnvironments,
                    operation: ProfileOperation::Set,
                    value: Some("staging".into()),
                }),
            }],
        },
    ]
}

fn candidate_kind_names(candidates: &[LearningCandidate]) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for candidate in candidates {
        let kind = match candidate {
            LearningCandidate::UserProfile(_) => "user_profile",
            LearningCandidate::Precedent(_) => "precedent",
            LearningCandidate::RunRecipe(_) => "run_recipe",
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
    candidates.iter().filter(|candidate| predicate(candidate)).count()
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
        assert_eq!(result.mutation_candidate_count, 1);
        assert!(result.candidate_kinds.contains(&"precedent"));
        assert!(result.candidate_kinds.contains(&"run_recipe"));
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
}
