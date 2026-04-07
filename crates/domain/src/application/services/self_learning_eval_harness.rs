//! Deterministic self-learning eval harness.
//!
//! This keeps the early Phase 4.9 learning path measurable without relying on
//! the chat model. It exercises typed evidence, candidate formation, mutation
//! candidates, and safe profile patching.

use crate::application::services::learning_candidate_service::{self, LearningCandidate};
use crate::application::services::learning_conflict_service;
use crate::application::services::learning_evidence_service;
use crate::application::services::learning_maintenance_service;
use crate::application::services::learning_quality_service;
use crate::application::services::learning_strength_service;
use crate::application::services::precedent_similarity_service;
use crate::application::services::procedural_cluster_review_service;
use crate::application::services::procedural_cluster_service::ProceduralCluster;
use crate::application::services::procedural_contradiction_service;
use crate::application::services::recipe_evolution_service;
use crate::application::services::run_recipe_cluster_service;
use crate::application::services::run_recipe_review_service;
use crate::application::services::skill_feedback_service;
use crate::application::services::skill_promotion_service;
use crate::application::services::skill_review_service;
use crate::application::services::user_profile_service;
use crate::domain::dialogue_state::FocusEntity;
use crate::domain::memory::{MemoryCategory, MemoryEntry, Skill};
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
    pub existing_precedents: Vec<MemoryEntry>,
    pub existing_failure_patterns: Vec<MemoryEntry>,
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
    pub skill_promotion_assessments: Vec<skill_promotion_service::SkillPromotionAssessment>,
    pub skill_promotion_items: Vec<String>,
    pub accepted_skill_promotion_count: usize,
    pub skill_review_reasons: Vec<&'static str>,
    pub skill_review_decisions: Vec<skill_review_service::SkillReviewDecision>,
    pub skill_review_items: Vec<String>,
    pub accepted_skill_review_count: usize,
    pub skill_feedback_reasons: Vec<&'static str>,
    pub skill_feedback_items: Vec<skill_feedback_service::SkillFailureFeedback>,
    pub accepted_skill_feedback_count: usize,
    pub run_recipe_review_decisions: Vec<run_recipe_review_service::RunRecipeReviewDecision>,
    pub run_recipe_review_items: Vec<String>,
    pub procedural_contradictions: Vec<procedural_contradiction_service::ProceduralContradiction>,
    pub precedent_mutation_decisions: Vec<SelfLearningEvalMutationItem>,
    pub precedent_mutation_actions: Vec<&'static str>,
    pub precedent_mutation_reasons: Vec<String>,
    pub precedent_cluster_reviews: Vec<procedural_cluster_review_service::ProceduralClusterReview>,
    pub precedent_cluster_review_items: Vec<String>,
    pub failure_cluster_reviews: Vec<procedural_cluster_review_service::ProceduralClusterReview>,
    pub failure_cluster_review_items: Vec<String>,
    pub maintenance_reasons: Vec<&'static str>,
    pub maintenance_runs_precedent_compaction: bool,
    pub maintenance_runs_failure_pattern_compaction: bool,
    pub maintenance_runs_run_recipe_review: bool,
    pub maintenance_runs_skill_review: bool,
    pub mutation_candidate_count: usize,
    pub profile_patch_is_noop: bool,
    pub profile_projection: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SelfLearningEvalMutationItem {
    pub action: &'static str,
    pub source: &'static str,
    pub target_id: Option<String>,
    pub reason: String,
    pub similarity: Option<f64>,
    pub text: String,
}

pub fn evaluate_scenario(scenario: &SelfLearningEvalScenario) -> SelfLearningEvalResult {
    let evidence = learning_evidence_service::build_learning_evidence(&scenario.tool_facts);
    let candidates = learning_candidate_service::build_learning_candidates(
        scenario.user_message,
        scenario.assistant_response,
        &scenario.tool_facts,
        &evidence,
    );
    let assessments = learning_conflict_service::resolve_learning_conflicts(
        &learning_strength_service::strengthen_learning_assessments(
            &learning_quality_service::assess_learning_candidates(
                &candidates,
                &evidence,
                &scenario.existing_recipes,
            ),
            scenario.current_profile.as_ref(),
            &scenario.existing_recipes,
        ),
    );
    let mutation_candidates =
        learning_candidate_service::build_mutation_candidates_from_assessments(&assessments);
    let skill_promotion_assessments = build_skill_promotion_assessments(scenario, &assessments);
    let resulting_recipes = build_resulting_recipes(scenario, &assessments);
    let resulting_failure_clusters = build_resulting_failure_clusters(scenario, &assessments);
    let precedent_mutation_decisions =
        build_precedent_mutation_decisions(scenario, &assessments, &resulting_failure_clusters);
    let skill_review_decisions = skill_review_service::review_learned_skills_with_failures(
        &scenario.existing_skills,
        &resulting_recipes,
        &resulting_failure_clusters,
    );
    let skill_feedback = build_skill_feedback(scenario, &assessments);
    let resulting_precedent_clusters =
        build_resulting_precedent_clusters(scenario, &precedent_mutation_decisions);
    let resulting_recipe_clusters =
        run_recipe_cluster_service::plan_recipe_clusters(&resulting_recipes, 0.9);
    let run_recipe_review_decisions = run_recipe_review_service::review_run_recipes_with_failures(
        &resulting_recipes,
        &resulting_failure_clusters,
        &run_recipe_review_service::RunRecipeReviewThresholds::default(),
    );
    let procedural_contradictions =
        procedural_contradiction_service::find_recipe_failure_contradictions(
            &resulting_recipe_clusters,
            &resulting_failure_clusters,
            0.75,
        );
    let precedent_cluster_reviews = procedural_cluster_review_service::review_precedent_clusters(
        &resulting_precedent_clusters,
        &resulting_failure_clusters,
    );
    let failure_cluster_reviews =
        procedural_cluster_review_service::review_failure_pattern_clusters(
            &resulting_failure_clusters,
            &procedural_contradictions,
        );
    let maintenance_plan = learning_maintenance_service::build_learning_maintenance_plan(
        &learning_maintenance_service::LearningMaintenanceSnapshot {
            recent_run_recipe_count: resulting_recipes.len(),
            run_recipe_cluster_count: resulting_recipe_clusters.len(),
            procedural_contradiction_count: procedural_contradictions.len(),
            recent_precedent_count: resulting_precedent_clusters.len(),
            precedent_cluster_count: resulting_precedent_clusters.len(),
            precedent_compact_candidate_count: precedent_cluster_reviews
                .iter()
                .filter(|review| {
                    review.kind == "precedent"
                        && review.action
                            == procedural_cluster_review_service::ProceduralClusterReviewAction::CompactCandidate
                })
                .count(),
            precedent_preserve_branch_count: precedent_cluster_reviews
                .iter()
                .filter(|review| {
                    review.kind == "precedent"
                        && review.action
                            == procedural_cluster_review_service::ProceduralClusterReviewAction::PreserveBranch
                })
                .count(),
            recent_reflection_count: 0,
            recent_failure_pattern_count: resulting_failure_clusters.len(),
            failure_pattern_cluster_count: resulting_failure_clusters.len(),
            failure_pattern_compact_candidate_count: failure_cluster_reviews
                .iter()
                .filter(|review| {
                    review.kind == "failure_pattern"
                        && review.action
                            == procedural_cluster_review_service::ProceduralClusterReviewAction::CompactCandidate
                })
                .count(),
            failure_pattern_blocking_count: failure_cluster_reviews
                .iter()
                .filter(|review| {
                    review.kind == "failure_pattern"
                        && review.action
                            == procedural_cluster_review_service::ProceduralClusterReviewAction::BlocksProceduralPaths
                })
                .count(),
            recent_skill_count: scenario.existing_skills.len(),
            candidate_skill_count: scenario
                .existing_skills
                .iter()
                .filter(|skill| skill.status == crate::domain::memory::SkillStatus::Candidate)
                .count(),
            skipped_cycles_since_maintenance: 0,
            prompt_optimization_due: false,
        },
        &learning_maintenance_service::LearningMaintenancePolicy::default(),
    );
    let patch = learning_candidate_service::build_user_profile_patch_from_assessments(
        &assessments,
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
        skill_promotion_assessments: skill_promotion_assessments.clone(),
        skill_promotion_items: skill_promotion_items(&skill_promotion_assessments),
        accepted_skill_promotion_count: skill_promotion_assessments
            .iter()
            .filter(|assessment| assessment.accepted)
            .count(),
        skill_review_reasons: skill_review_reason_names(&skill_review_decisions),
        skill_review_decisions: skill_review_decisions.clone(),
        skill_review_items: skill_review_items(&skill_review_decisions),
        accepted_skill_review_count: skill_review_decisions.len(),
        skill_feedback_reasons: skill_feedback_reason_names(&skill_feedback),
        skill_feedback_items: skill_feedback.clone(),
        accepted_skill_feedback_count: skill_feedback.len(),
        run_recipe_review_decisions: run_recipe_review_decisions.clone(),
        run_recipe_review_items: run_recipe_review_items(&run_recipe_review_decisions),
        procedural_contradictions: procedural_contradictions.clone(),
        precedent_mutation_decisions: precedent_mutation_items(&precedent_mutation_decisions),
        precedent_mutation_actions: precedent_mutation_action_names(&precedent_mutation_decisions),
        precedent_mutation_reasons: precedent_mutation_decisions
            .iter()
            .map(|decision| decision.reason.clone())
            .collect(),
        precedent_cluster_reviews: precedent_cluster_reviews.clone(),
        precedent_cluster_review_items: cluster_review_items(&precedent_cluster_reviews),
        failure_cluster_reviews: failure_cluster_reviews.clone(),
        failure_cluster_review_items: cluster_review_items(&failure_cluster_reviews),
        maintenance_reasons: maintenance_reason_names(&maintenance_plan),
        maintenance_runs_precedent_compaction: maintenance_plan.run_precedent_compaction,
        maintenance_runs_failure_pattern_compaction: maintenance_plan
            .run_failure_pattern_compaction,
        maintenance_runs_run_recipe_review: maintenance_plan.run_run_recipe_review,
        maintenance_runs_skill_review: maintenance_plan.run_skill_review,
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
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
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
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
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
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
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
            id: "conflicting_profile_updates_are_rejected",
            user_message: "Remember both of these timezones",
            assistant_response: "I captured conflicting timezone updates.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: Vec::new(),
            existing_skills: Vec::new(),
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "user_profile".into(),
                    payload: ToolFactPayload::UserProfile(UserProfileFact {
                        field: UserProfileField::Timezone,
                        operation: ProfileOperation::Set,
                        value: Some("Europe/Berlin".into()),
                    }),
                },
                TypedToolFact {
                    tool_id: "user_profile".into(),
                    payload: ToolFactPayload::UserProfile(UserProfileFact {
                        field: UserProfileField::Timezone,
                        operation: ProfileOperation::Set,
                        value: Some("Europe/Paris".into()),
                    }),
                },
            ],
        },
        SelfLearningEvalScenario {
            id: "similar_existing_recipe_merges",
            user_message: "Find the status page and send it again",
            assistant_response: "Fetched the page and sent it again.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                sample_request: "find the status page and send it".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                lineage_task_families: vec!["search_delivery".into()],
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
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "resource_delivery".into(),
                sample_request: "fetch a page and send it".into(),
                summary: "pattern=web_fetch -> browser_open".into(),
                tool_pattern: vec!["web_fetch".into(), "browser_open".into()],
                lineage_task_families: vec!["resource_delivery".into()],
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
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
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
            id: "failure_pattern_penalizes_overlapping_learned_skill",
            user_message: "Fetch the page and send it",
            assistant_response: "The fetch failed.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: Vec::new(),
            existing_skills: vec![Skill {
                id: "sk1".into(),
                name: "search_delivery".into(),
                description: "Promoted skill".into(),
                content: "content".into(),
                task_family: Some("search_delivery".into()),
                tool_pattern: vec!["web_fetch".into(), "message_send".into()],
                lineage_task_families: vec!["search_delivery".into()],
                tags: vec!["recipe-promotion".into()],
                success_count: 4,
                fail_count: 0,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Learned,
                status: crate::domain::memory::SkillStatus::Active,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
            tool_facts: vec![
                TypedToolFact {
                    tool_id: "message_send".into(),
                    payload: ToolFactPayload::Delivery(DeliveryFact {
                        target: DeliveryTargetKind::CurrentConversation,
                        content_bytes: Some(24),
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
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                sample_request: "find the status page and send it".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                lineage_task_families: vec!["search_delivery".into()],
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
            id: "manual_skill_shadows_recipe_promotion",
            user_message: "Find the status page and send it again",
            assistant_response: "Fetched the page and sent it again.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                sample_request: "find the status page and send it".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                lineage_task_families: vec!["search_delivery".into()],
                success_count: 3,
                updated_at: 1,
            }],
            existing_skills: vec![Skill {
                id: "sk-manual".into(),
                name: "manual_status_delivery".into(),
                description: "Manual skill".into(),
                content: "Use the manual runbook.".into(),
                task_family: Some("search_delivery".into()),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                lineage_task_families: vec!["search_delivery".into()],
                tags: vec!["manual".into()],
                success_count: 1,
                fail_count: 0,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Manual,
                status: crate::domain::memory::SkillStatus::Active,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
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
            id: "candidate_skill_without_recipe_support_is_deprecated",
            user_message: "No-op turn",
            assistant_response: "No procedural work happened.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "backup_delivery".into(),
                sample_request: "run backup and send it".into(),
                summary: "pattern=shell -> message_send".into(),
                tool_pattern: vec!["shell".into(), "message_send".into()],
                lineage_task_families: vec!["backup_delivery".into()],
                success_count: 4,
                updated_at: 1,
            }],
            existing_skills: vec![Skill {
                id: "sk-learned".into(),
                name: "search_delivery".into(),
                description: "Learned skill".into(),
                content: "content".into(),
                task_family: Some("search_delivery".into()),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                lineage_task_families: vec!["search_delivery".into()],
                tags: vec!["recipe-promotion".into()],
                success_count: 3,
                fail_count: 0,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Learned,
                status: crate::domain::memory::SkillStatus::Candidate,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
            tool_facts: Vec::new(),
        },
        SelfLearningEvalScenario {
            id: "candidate_skill_with_ambiguous_recipe_support_is_deprecated",
            user_message: "No-op turn",
            assistant_response: "No procedural work happened.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![
                RunRecipe {
                    agent_id: "agent".into(),
                    task_family: "search_delivery".into(),
                    sample_request: "find and send".into(),
                    summary: "pattern=web_search -> message_send".into(),
                    tool_pattern: vec!["web_search".into(), "message_send".into()],
                    lineage_task_families: vec!["search_delivery".into()],
                    success_count: 4,
                    updated_at: 1,
                },
                RunRecipe {
                    agent_id: "agent".into(),
                    task_family: "fetch_delivery".into(),
                    sample_request: "fetch and send".into(),
                    summary: "pattern=web_fetch -> message_send".into(),
                    tool_pattern: vec!["web_fetch".into(), "message_send".into()],
                    lineage_task_families: vec!["fetch_delivery".into()],
                    success_count: 4,
                    updated_at: 2,
                },
            ],
            existing_skills: vec![Skill {
                id: "sk-learned".into(),
                name: "search_fetch_delivery".into(),
                description: "Learned skill".into(),
                content: "content".into(),
                task_family: Some("search_fetch_delivery".into()),
                tool_pattern: vec![
                    "web_search".into(),
                    "web_fetch".into(),
                    "message_send".into(),
                ],
                lineage_task_families: vec!["search_fetch_delivery".into()],
                tags: vec!["recipe-promotion".into()],
                success_count: 3,
                fail_count: 0,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Learned,
                status: crate::domain::memory::SkillStatus::Candidate,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
            tool_facts: Vec::new(),
        },
        SelfLearningEvalScenario {
            id: "candidate_skill_contradicted_by_failure_pattern_is_deprecated",
            user_message: "Fetch the page",
            assistant_response: "The fetch failed.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: Vec::new(),
            existing_skills: vec![Skill {
                id: "sk-learned".into(),
                name: "fetch_page".into(),
                description: "Learned skill".into(),
                content: "content".into(),
                task_family: Some("fetch_page".into()),
                tool_pattern: vec!["web_fetch".into()],
                lineage_task_families: vec!["fetch_page".into()],
                tags: vec!["recipe-promotion".into()],
                success_count: 3,
                fail_count: 0,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Learned,
                status: crate::domain::memory::SkillStatus::Candidate,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
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
            id: "candidate_skill_with_exact_recipe_support_is_deprecated_by_failure_pattern",
            user_message: "Fetch the page",
            assistant_response: "The fetch failed.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "fetch_page".into(),
                sample_request: "fetch the page".into(),
                summary: "pattern=web_fetch".into(),
                tool_pattern: vec!["web_fetch".into()],
                lineage_task_families: vec!["fetch_page".into()],
                success_count: 4,
                updated_at: 1,
            }],
            existing_skills: vec![Skill {
                id: "sk-learned".into(),
                name: "fetch_page".into(),
                description: "Learned skill".into(),
                content: "content".into(),
                task_family: Some("fetch_page".into()),
                tool_pattern: vec!["web_fetch".into()],
                lineage_task_families: vec!["fetch_page".into()],
                tags: vec!["recipe-promotion".into()],
                success_count: 3,
                fail_count: 0,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Learned,
                status: crate::domain::memory::SkillStatus::Candidate,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
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
            id: "active_skill_with_exact_recipe_support_is_downgraded_by_failure_pattern",
            user_message: "Fetch the page",
            assistant_response: "The fetch failed.",
            current_profile: None,
            existing_precedents: Vec::new(),
            existing_failure_patterns: Vec::new(),
            existing_recipes: vec![RunRecipe {
                agent_id: "agent".into(),
                task_family: "fetch_page".into(),
                sample_request: "fetch the page".into(),
                summary: "pattern=web_fetch".into(),
                tool_pattern: vec!["web_fetch".into()],
                lineage_task_families: vec!["fetch_page".into()],
                success_count: 5,
                updated_at: 1,
            }],
            existing_skills: vec![Skill {
                id: "sk-learned".into(),
                name: "fetch_page".into(),
                description: "Learned skill".into(),
                content: "content".into(),
                task_family: Some("fetch_page".into()),
                tool_pattern: vec!["web_fetch".into()],
                lineage_task_families: vec!["fetch_page".into()],
                tags: vec!["recipe-promotion".into()],
                success_count: 6,
                fail_count: 1,
                version: 1,
                origin: crate::domain::memory::SkillOrigin::Learned,
                status: crate::domain::memory::SkillStatus::Active,
                created_by: "agent".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }],
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
            id: "contradicted_similar_precedent_keeps_new_branch",
            user_message: "Find the status page and send it",
            assistant_response: "Found it and sent it.",
            current_profile: None,
            existing_precedents: vec![MemoryEntry {
                id: "p1".into(),
                key: "precedent_1".into(),
                content: "tools=web_search -> message_send | subjects=status".into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: Some(0.88),
            }],
            existing_failure_patterns: vec![MemoryEntry {
                id: "f1".into(),
                key: "failure_1".into(),
                content: "failed_tools=web_search -> message_send | outcomes=runtime_error".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            }],
            existing_recipes: Vec::new(),
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

fn build_resulting_recipes(
    scenario: &SelfLearningEvalScenario,
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<RunRecipe> {
    let mut recipes = scenario.existing_recipes.clone();
    for assessment in assessments.iter().filter(|assessment| assessment.accepted) {
        let LearningCandidate::RunRecipe(candidate) = &assessment.candidate else {
            continue;
        };
        let recipe = if assessment.merge_with_existing {
            recipes
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

        if let Some(existing) = recipes
            .iter_mut()
            .find(|existing| existing.task_family == recipe.task_family)
        {
            *existing = recipe;
        } else {
            recipes.push(recipe);
        }
    }
    recipes
}

fn build_skill_promotion_assessments(
    scenario: &SelfLearningEvalScenario,
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<skill_promotion_service::SkillPromotionAssessment> {
    let failure_clusters = build_resulting_failure_clusters(scenario, assessments);
    assessments
        .iter()
        .filter(|assessment| assessment.accepted)
        .filter_map(|assessment| {
            let LearningCandidate::RunRecipe(candidate) = &assessment.candidate else {
                return None;
            };
            let recipe = if assessment.merge_with_existing {
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
            Some(
                skill_promotion_service::assess_recipe_for_skill_promotion_with_failures(
                    &recipe,
                    existing_skill,
                    &scenario.existing_skills,
                    &failure_clusters,
                ),
            )
        })
        .collect()
}

fn build_skill_feedback(
    scenario: &SelfLearningEvalScenario,
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<skill_feedback_service::SkillFailureFeedback> {
    assessments
        .iter()
        .filter(|assessment| assessment.accepted)
        .filter_map(|assessment| {
            let LearningCandidate::FailurePattern(failure) = &assessment.candidate else {
                return None;
            };
            Some(skill_feedback_service::assess_failure_feedback(
                failure,
                &scenario.existing_skills,
            ))
        })
        .flatten()
        .collect()
}

fn build_resulting_failure_clusters(
    scenario: &SelfLearningEvalScenario,
    assessments: &[learning_quality_service::LearningCandidateAssessment],
) -> Vec<ProceduralCluster> {
    let mut clusters = scenario
        .existing_failure_patterns
        .iter()
        .map(|entry| ProceduralCluster {
            representative: entry.clone(),
            member_keys: vec![entry.key.clone()],
        })
        .collect::<Vec<_>>();

    clusters.extend(
        assessments
            .iter()
            .enumerate()
            .filter(|(_, assessment)| assessment.accepted)
            .filter_map(|(index, assessment)| {
                let LearningCandidate::FailurePattern(failure) = &assessment.candidate else {
                    return None;
                };
                let key = format!("eval-failure-{index}");
                Some(ProceduralCluster {
                    representative: MemoryEntry {
                        id: key.clone(),
                        key: key.clone(),
                        content: failure.summary.clone(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-01-01T00:00:00Z".into(),
                        session_id: None,
                        score: None,
                    },
                    member_keys: vec![key],
                })
            }),
    );

    clusters
}

fn build_precedent_mutation_decisions(
    scenario: &SelfLearningEvalScenario,
    assessments: &[learning_quality_service::LearningCandidateAssessment],
    failure_clusters: &[ProceduralCluster],
) -> Vec<crate::domain::memory_mutation::MutationDecision> {
    assessments
        .iter()
        .filter(|assessment| assessment.accepted)
        .filter_map(learning_candidate_service::build_mutation_candidate_from_assessment)
        .filter(|candidate| {
            matches!(
                candidate.category,
                MemoryCategory::Custom(ref name) if name == "precedent"
            )
        })
        .map(|candidate| {
            precedent_similarity_service::decide_precedent_mutation_with_failures(
                candidate,
                &scenario.existing_precedents,
                &precedent_similarity_service::PrecedentSimilarityThresholds::default(),
                failure_clusters,
            )
        })
        .collect()
}

fn build_resulting_precedent_clusters(
    scenario: &SelfLearningEvalScenario,
    decisions: &[crate::domain::memory_mutation::MutationDecision],
) -> Vec<ProceduralCluster> {
    let mut clusters = scenario
        .existing_precedents
        .iter()
        .map(|entry| ProceduralCluster {
            representative: entry.clone(),
            member_keys: vec![entry.key.clone()],
        })
        .collect::<Vec<_>>();

    clusters.extend(
        decisions
            .iter()
            .enumerate()
            .filter(|(_, decision)| {
                matches!(
                    decision.action,
                    crate::domain::memory_mutation::MutationAction::Add
                )
            })
            .map(|(index, decision)| {
                let key = format!("eval-precedent-{index}");
                ProceduralCluster {
                    representative: MemoryEntry {
                        id: key.clone(),
                        key: key.clone(),
                        content: decision.candidate.text.clone(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-01-01T00:00:00Z".into(),
                        session_id: None,
                        score: None,
                    },
                    member_keys: vec![key],
                }
            }),
    );

    clusters
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

fn skill_promotion_items(
    assessments: &[skill_promotion_service::SkillPromotionAssessment],
) -> Vec<String> {
    assessments
        .iter()
        .map(|assessment| {
            format!(
                "{}:{}:[{}]",
                assessment.reason,
                assessment.skill_name,
                assessment.lineage_task_families.join(", ")
            )
        })
        .collect()
}

fn skill_feedback_reason_names(
    feedback: &[skill_feedback_service::SkillFailureFeedback],
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    for item in feedback {
        if !reasons.contains(&item.reason) {
            reasons.push(item.reason);
        }
    }
    reasons
}

fn skill_review_reason_names(
    decisions: &[skill_review_service::SkillReviewDecision],
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    for decision in decisions {
        if !reasons.contains(&decision.reason) {
            reasons.push(decision.reason);
        }
    }
    reasons
}

fn skill_review_items(decisions: &[skill_review_service::SkillReviewDecision]) -> Vec<String> {
    decisions
        .iter()
        .map(|decision| {
            format!(
                "{}:{}:[{}]",
                decision.reason,
                decision.skill_name,
                decision.lineage_task_families.join(", ")
            )
        })
        .collect()
}

fn run_recipe_review_items(
    decisions: &[run_recipe_review_service::RunRecipeReviewDecision],
) -> Vec<String> {
    decisions
        .iter()
        .map(|decision| {
            format!(
                "{}:{}:[{}]",
                decision.reason,
                decision.canonical_recipe.task_family,
                decision.canonical_recipe.lineage_task_families.join(", ")
            )
        })
        .collect()
}

fn precedent_mutation_action_names(
    decisions: &[crate::domain::memory_mutation::MutationDecision],
) -> Vec<&'static str> {
    let mut actions = Vec::new();
    for decision in decisions {
        let action = mutation_action_name(&decision.action);
        if !actions.contains(&action) {
            actions.push(action);
        }
    }
    actions
}

fn precedent_mutation_items(
    decisions: &[crate::domain::memory_mutation::MutationDecision],
) -> Vec<SelfLearningEvalMutationItem> {
    decisions
        .iter()
        .map(|decision| {
            let target_id = match &decision.action {
                crate::domain::memory_mutation::MutationAction::Update { target_id }
                | crate::domain::memory_mutation::MutationAction::Delete { target_id } => {
                    Some(target_id.clone())
                }
                crate::domain::memory_mutation::MutationAction::Add
                | crate::domain::memory_mutation::MutationAction::Noop => None,
            };
            SelfLearningEvalMutationItem {
                action: mutation_action_name(&decision.action),
                source: mutation_source_name(&decision.candidate.source),
                target_id,
                reason: decision.reason.clone(),
                similarity: decision.similarity,
                text: decision.candidate.text.clone(),
            }
        })
        .collect()
}

fn mutation_action_name(
    action: &crate::domain::memory_mutation::MutationAction,
) -> &'static str {
    match action {
        crate::domain::memory_mutation::MutationAction::Add => "add",
        crate::domain::memory_mutation::MutationAction::Update { .. } => "update",
        crate::domain::memory_mutation::MutationAction::Delete { .. } => "delete",
        crate::domain::memory_mutation::MutationAction::Noop => "noop",
    }
}

fn mutation_source_name(
    source: &crate::domain::memory_mutation::MutationSource,
) -> &'static str {
    match source {
        crate::domain::memory_mutation::MutationSource::Consolidation => "consolidation",
        crate::domain::memory_mutation::MutationSource::ExplicitUser => "explicit_user",
        crate::domain::memory_mutation::MutationSource::ToolOutput => "tool_output",
        crate::domain::memory_mutation::MutationSource::Reflection => "reflection",
    }
}

fn cluster_review_items(
    reviews: &[procedural_cluster_review_service::ProceduralClusterReview],
) -> Vec<String> {
    reviews
        .iter()
        .map(|review| {
            format!(
                "{}:{}",
                cluster_review_action_name(&review.action),
                review.representative_key
            )
        })
        .collect()
}

fn cluster_review_action_name(
    action: &procedural_cluster_review_service::ProceduralClusterReviewAction,
) -> &'static str {
    match action {
        procedural_cluster_review_service::ProceduralClusterReviewAction::Stable => "stable",
        procedural_cluster_review_service::ProceduralClusterReviewAction::CompactCandidate => {
            "compact_candidate"
        }
        procedural_cluster_review_service::ProceduralClusterReviewAction::PreserveBranch => {
            "preserve_branch"
        }
        procedural_cluster_review_service::ProceduralClusterReviewAction::BlocksProceduralPaths => {
            "blocks_procedural_paths"
        }
    }
}

fn maintenance_reason_names(
    plan: &learning_maintenance_service::LearningMaintenancePlan,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    for reason in &plan.reasons {
        let value = match reason {
            learning_maintenance_service::LearningMaintenanceReason::RecentLearningActivity => {
                "recent_learning_activity"
            }
            learning_maintenance_service::LearningMaintenanceReason::RunRecipeDuplicateBacklog => {
                "run_recipe_duplicate_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::PrecedentDuplicateBacklog => {
                "precedent_duplicate_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::PrecedentPreserveBranchBacklog => {
                "precedent_preserve_branch_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::FailurePatternDuplicateBacklog => {
                "failure_pattern_duplicate_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::FailureBlockingClusterBacklog => {
                "failure_blocking_cluster_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::ProceduralContradictionBacklog => {
                "procedural_contradiction_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::SkillBacklog => {
                "skill_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::CandidateSkillBacklog => {
                "candidate_skill_backlog"
            }
            learning_maintenance_service::LearningMaintenanceReason::PromptOptimizationDue => {
                "prompt_optimization_due"
            }
            learning_maintenance_service::LearningMaintenanceReason::ForcedMaintenanceInterval => {
                "forced_maintenance_interval"
            }
        };
        if !reasons.contains(&value) {
            reasons.push(value);
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
    fn conflicting_profile_updates_do_not_patch_profile() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "conflicting_profile_updates_are_rejected")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert!(result.profile_patch_is_noop);
        assert!(result
            .assessment_reasons
            .contains(&"conflicting_profile_candidates"));
        assert!(!result.accepted_candidate_kinds.contains(&"user_profile"));
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
        assert!(result
            .assessment_reasons
            .contains(&"repeated_recipe_pattern"));
        assert_eq!(result.accepted_skill_promotion_count, 1);
        assert!(result
            .skill_promotion_reasons
            .contains(&"create_candidate_skill"));
        assert!(result
            .skill_promotion_items
            .iter()
            .any(|item| item.contains("[search_delivery]")));
        assert!(result.skill_promotion_assessments.iter().any(|assessment| {
            assessment.reason == "create_candidate_skill"
                && assessment
                    .lineage_task_families
                    .iter()
                    .any(|family| family == "search_delivery")
        }));
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
        assert!(result.failure_cluster_review_items.iter().any(
            |item| item.starts_with("stable:") || item.starts_with("blocks_procedural_paths:")
        ));
        assert!(result.failure_cluster_reviews.iter().any(|review| {
            matches!(
                review.action,
                procedural_cluster_review_service::ProceduralClusterReviewAction::Stable
                    | procedural_cluster_review_service::ProceduralClusterReviewAction::BlocksProceduralPaths
            )
        }));
    }

    #[test]
    fn failure_turn_penalizes_overlapping_learned_skill() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "failure_pattern_penalizes_overlapping_learned_skill")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_failure_pattern_count, 1);
        assert_eq!(result.accepted_skill_feedback_count, 1);
        assert!(result
            .skill_feedback_reasons
            .contains(&"failed_tool_pattern_overlap"));
        assert!(result.skill_feedback_items.iter().any(|item| {
            item.reason == "failed_tool_pattern_overlap"
                && !item.skill_id.is_empty()
                && !item.skill_name.is_empty()
        }));
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

    #[test]
    fn manual_skill_blocks_shadowed_recipe_promotion() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "manual_skill_shadows_recipe_promotion")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_run_recipe_count, 1);
        assert_eq!(result.accepted_skill_promotion_count, 0);
        assert!(result
            .skill_promotion_reasons
            .contains(&"shadowed_by_higher_origin_skill"));
    }

    #[test]
    fn candidate_skill_without_recipe_support_is_deprecated() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "candidate_skill_without_recipe_support_is_deprecated")
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_skill_review_count, 1);
        assert!(result
            .skill_review_reasons
            .contains(&"unsupported_by_recipe_clusters"));
        assert!(result
            .skill_review_items
            .iter()
            .any(|item| item.contains("[search_delivery]")));
        assert!(result.skill_review_decisions.iter().any(|decision| {
            decision.reason == "unsupported_by_recipe_clusters"
                && decision
                    .lineage_task_families
                    .iter()
                    .any(|family| family == "search_delivery")
        }));
    }

    #[test]
    fn candidate_skill_with_ambiguous_recipe_support_is_deprecated() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| {
                scenario.id == "candidate_skill_with_ambiguous_recipe_support_is_deprecated"
            })
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_skill_review_count, 1);
        assert!(result
            .skill_review_reasons
            .contains(&"ambiguous_recipe_cluster_support"));
    }

    #[test]
    fn candidate_skill_contradicted_by_failure_pattern_is_deprecated() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| {
                scenario.id == "candidate_skill_contradicted_by_failure_pattern_is_deprecated"
            })
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_failure_pattern_count, 1);
        assert_eq!(result.accepted_skill_review_count, 1);
        assert!(result
            .skill_review_reasons
            .contains(&"contradicted_by_failure_clusters"));
    }

    #[test]
    fn candidate_skill_with_exact_recipe_support_is_deprecated_by_failure_pattern() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| {
                scenario.id
                    == "candidate_skill_with_exact_recipe_support_is_deprecated_by_failure_pattern"
            })
            .unwrap();

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_failure_pattern_count, 1);
        assert_eq!(result.accepted_skill_review_count, 1);
        assert!(result
            .skill_review_reasons
            .contains(&"supported_recipe_cluster_contradicted_by_failure_clusters"));
    }

    #[test]
    fn active_skill_with_exact_recipe_support_is_downgraded_by_failure_pattern() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| {
                scenario.id
                    == "active_skill_with_exact_recipe_support_is_downgraded_by_failure_pattern"
            })
            .expect("scenario present");

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.accepted_skill_review_count, 1);
        assert!(result
            .skill_review_reasons
            .contains(&"active_supported_recipe_cluster_contradicted_by_failure_clusters"));
        assert!(result
            .skill_review_items
            .iter()
            .any(|item| item.contains("[fetch_page]")));
        assert!(result.skill_review_decisions.iter().any(|decision| {
            decision.reason == "active_supported_recipe_cluster_contradicted_by_failure_clusters"
                && matches!(
                    decision.action,
                    skill_review_service::SkillReviewAction::DowngradeToCandidate
                )
                && decision
                    .lineage_task_families
                    .iter()
                    .any(|family| family == "fetch_page")
        }));
        assert!(result.procedural_contradictions.iter().any(|contradiction| {
            contradiction
                .recipe_lineage_task_families
                .iter()
                .any(|family| family == "fetch_page")
        }));
    }

    #[test]
    fn contradicted_similar_precedent_keeps_new_branch() {
        let scenario = default_golden_scenarios()
            .into_iter()
            .find(|scenario| scenario.id == "contradicted_similar_precedent_keeps_new_branch")
            .expect("scenario present");

        let result = evaluate_scenario(&scenario);
        assert_eq!(result.precedent_candidate_count, 1);
        assert_eq!(result.mutation_candidate_count, 1);
        assert!(result.precedent_mutation_actions.contains(&"add"));
        assert!(result.precedent_mutation_decisions.iter().any(|decision| {
            decision.action == "add"
                && decision.source == "tool_output"
                && decision.reason.contains("contradicted by failure clusters")
        }));
        assert!(result
            .precedent_mutation_reasons
            .iter()
            .any(|reason| reason.contains("contradicted by failure clusters")));
        assert!(result
            .precedent_cluster_review_items
            .iter()
            .any(|item| item.starts_with("preserve_branch:")));
        assert!(result.precedent_cluster_reviews.iter().any(|review| {
            matches!(
                review.action,
                procedural_cluster_review_service::ProceduralClusterReviewAction::PreserveBranch
            )
        }));
        assert!(result
            .maintenance_reasons
            .contains(&"precedent_preserve_branch_backlog"));
        assert!(result.maintenance_runs_run_recipe_review);
        assert!(result.maintenance_runs_skill_review);
    }
}
