//! Strengthen accepted learning assessments with repeated evidence and conflict checks.
//!
//! This layer runs after cheap candidate assessment. It should not invent new
//! candidates; it only upgrades confidence/reasons when we already have stable
//! supporting state such as repeated recipes or current user-profile values.

use crate::application::services::learning_candidate_service::LearningCandidate;
use crate::application::services::learning_quality_service::LearningCandidateAssessment;
use crate::domain::run_recipe::RunRecipe;
use crate::domain::tool_fact::ProfileOperation;
use crate::domain::user_profile::UserProfile;

pub fn strengthen_learning_assessments(
    assessments: &[LearningCandidateAssessment],
    current_profile: Option<&UserProfile>,
    existing_recipes: &[RunRecipe],
) -> Vec<LearningCandidateAssessment> {
    assessments
        .iter()
        .cloned()
        .map(|assessment| {
            let candidate = assessment.candidate.clone();
            match candidate {
                LearningCandidate::UserProfile(candidate) => {
                    strengthen_profile_assessment(assessment, &candidate, current_profile)
                }
                LearningCandidate::RunRecipe(candidate) => strengthen_recipe_assessment(
                    assessment,
                    candidate.task_family_hint.as_str(),
                    existing_recipes,
                ),
                _ => assessment,
            }
        })
        .collect()
}

fn strengthen_profile_assessment(
    mut assessment: LearningCandidateAssessment,
    candidate: &crate::application::services::learning_candidate_service::UserProfileLearningCandidate,
    current_profile: Option<&UserProfile>,
) -> LearningCandidateAssessment {
    let Some(profile) = current_profile else {
        return assessment;
    };

    let current_value = profile.get_text(&candidate.key);
    let is_reinforcement = match (
        &candidate.operation,
        current_value.as_deref(),
        candidate.value.as_deref(),
    ) {
        (ProfileOperation::Set, Some(current), Some(next)) => value_matches(current, next),
        (ProfileOperation::Clear, None, _) => true,
        _ => false,
    };
    if is_reinforcement {
        assessment.confidence = assessment.confidence.max(0.99);
        assessment.accepted = true;
        assessment.reason = "reinforced_profile_fact";
        return assessment;
    }

    let is_correction = match (
        &candidate.operation,
        current_value.as_deref(),
        candidate.value.as_deref(),
    ) {
        (ProfileOperation::Set, Some(current), Some(next)) => !value_matches(current, next),
        (ProfileOperation::Clear, Some(_), _) => true,
        _ => false,
    };
    if is_correction {
        assessment.confidence = assessment.confidence.max(0.98);
        assessment.accepted = true;
        assessment.reason = "explicit_profile_correction";
    }

    assessment
}

fn strengthen_recipe_assessment(
    mut assessment: LearningCandidateAssessment,
    task_family_hint: &str,
    existing_recipes: &[RunRecipe],
) -> LearningCandidateAssessment {
    if !assessment.accepted {
        return assessment;
    }

    let Some(existing) = existing_recipes
        .iter()
        .filter(|recipe| recipe.task_family == task_family_hint)
        .max_by_key(|recipe| recipe.success_count)
    else {
        return assessment;
    };

    if existing.success_count >= 2 {
        assessment.confidence =
            (assessment.confidence + recipe_repetition_bonus(existing.success_count)).min(0.96);
        assessment.reason = "repeated_recipe_pattern";
    }

    assessment
}

fn recipe_repetition_bonus(success_count: u32) -> f32 {
    match success_count {
        0..=1 => 0.0,
        2 => 0.04,
        3 => 0.06,
        _ => 0.08,
    }
}

fn value_matches(current: &str, next: &str) -> bool {
    current.trim().eq_ignore_ascii_case(next.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::learning_candidate_service::{
        RunRecipeLearningCandidate, UserProfileLearningCandidate,
    };
    use serde_json::json;

    #[test]
    fn strengthens_matching_profile_fact_as_reinforcement() {
        let strengthened = strengthen_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::UserProfile(UserProfileLearningCandidate {
                    key: "project_alias".into(),
                    operation: ProfileOperation::Set,
                    value: Some("Borealis".into()),
                }),
                confidence: 0.96,
                accepted: true,
                merge_with_existing: false,
                reason: "explicit_profile_fact",
            }],
            Some(&UserProfile {
                facts: [("project_alias".into(), json!("Borealis"))].into(),
            }),
            &[],
        );

        assert_eq!(strengthened[0].reason, "reinforced_profile_fact");
        assert!(strengthened[0].confidence >= 0.99);
    }

    #[test]
    fn strengthens_conflicting_profile_fact_as_correction() {
        let strengthened = strengthen_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::UserProfile(UserProfileLearningCandidate {
                    key: "project_alias".into(),
                    operation: ProfileOperation::Set,
                    value: Some("Atlas".into()),
                }),
                confidence: 0.96,
                accepted: true,
                merge_with_existing: false,
                reason: "explicit_profile_fact",
            }],
            Some(&UserProfile {
                facts: [("project_alias".into(), json!("Borealis"))].into(),
            }),
            &[],
        );

        assert_eq!(strengthened[0].reason, "explicit_profile_correction");
        assert!(strengthened[0].confidence >= 0.98);
    }

    #[test]
    fn strengthens_recipe_assessment_when_repeated_recipe_exists() {
        let strengthened = strengthen_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::RunRecipe(RunRecipeLearningCandidate {
                    task_family_hint: "search_delivery".into(),
                    sample_request: "find and send".into(),
                    summary: "pattern=web_search -> message_send".into(),
                    tool_pattern: vec!["web_search".into(), "message_send".into()],
                }),
                confidence: 0.82,
                accepted: true,
                merge_with_existing: true,
                reason: "merge_existing_recipe",
            }],
            None,
            &[RunRecipe {
                agent_id: "agent".into(),
                task_family: "search_delivery".into(),
                lineage_task_families: vec!["search_delivery".into()],
                sample_request: "find and send".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                success_count: 3,
                updated_at: 1,
            }],
        );

        assert_eq!(strengthened[0].reason, "repeated_recipe_pattern");
        assert!(strengthened[0].confidence > 0.82);
    }

    #[test]
    fn new_profile_fact_is_not_treated_as_correction() {
        let strengthened = strengthen_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::UserProfile(UserProfileLearningCandidate {
                    key: "release_tracks".into(),
                    operation: ProfileOperation::Set,
                    value: Some("staging".into()),
                }),
                confidence: 0.96,
                accepted: true,
                merge_with_existing: false,
                reason: "explicit_profile_fact",
            }],
            Some(&UserProfile {
                facts: [("release_tracks".into(), json!("prod"))].into(),
            }),
            &[],
        );

        assert_eq!(strengthened[0].reason, "explicit_profile_correction");
    }
}
