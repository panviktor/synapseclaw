//! Strengthen accepted learning assessments with repeated evidence and conflict checks.
//!
//! This layer runs after cheap candidate assessment. It should not invent new
//! candidates; it only upgrades confidence/reasons when we already have stable
//! supporting state such as repeated recipes or current user-profile values.

use crate::application::services::learning_candidate_service::LearningCandidate;
use crate::application::services::learning_quality_service::LearningCandidateAssessment;
use crate::domain::run_recipe::RunRecipe;
use crate::domain::tool_fact::{ProfileOperation, UserProfileField};
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

    if let UserProfileField::KnownEnvironments = candidate.field {
        return strengthen_known_environments_assessment(assessment, candidate, profile);
    }

    let current_value = current_profile_value(profile, &candidate.field);
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

fn strengthen_known_environments_assessment(
    mut assessment: LearningCandidateAssessment,
    candidate: &crate::application::services::learning_candidate_service::UserProfileLearningCandidate,
    profile: &UserProfile,
) -> LearningCandidateAssessment {
    match (&candidate.operation, candidate.value.as_deref()) {
        (ProfileOperation::Set, Some(next)) => {
            if profile
                .known_environments
                .iter()
                .any(|existing| value_matches(existing, next))
            {
                assessment.confidence = assessment.confidence.max(0.99);
                assessment.accepted = true;
                assessment.reason = "reinforced_profile_fact";
            }
        }
        (ProfileOperation::Clear, _) if !profile.known_environments.is_empty() => {
            assessment.confidence = assessment.confidence.max(0.98);
            assessment.accepted = true;
            assessment.reason = "explicit_profile_correction";
        }
        _ => {}
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

fn current_profile_value(profile: &UserProfile, field: &UserProfileField) -> Option<String> {
    match field {
        UserProfileField::PreferredLanguage => profile.preferred_language.clone(),
        UserProfileField::Timezone => profile.timezone.clone(),
        UserProfileField::DefaultCity => profile.default_city.clone(),
        UserProfileField::CommunicationStyle => profile.communication_style.clone(),
        UserProfileField::KnownEnvironments => None,
        UserProfileField::DefaultDeliveryTarget => None,
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

    #[test]
    fn strengthens_matching_profile_fact_as_reinforcement() {
        let strengthened = strengthen_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::UserProfile(UserProfileLearningCandidate {
                    field: UserProfileField::Timezone,
                    operation: ProfileOperation::Set,
                    value: Some("Europe/Berlin".into()),
                }),
                confidence: 0.96,
                accepted: true,
                merge_with_existing: false,
                reason: "explicit_profile_fact",
            }],
            Some(&UserProfile {
                timezone: Some("Europe/Berlin".into()),
                ..Default::default()
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
                    field: UserProfileField::Timezone,
                    operation: ProfileOperation::Set,
                    value: Some("Europe/Paris".into()),
                }),
                confidence: 0.96,
                accepted: true,
                merge_with_existing: false,
                reason: "explicit_profile_fact",
            }],
            Some(&UserProfile {
                timezone: Some("Europe/Berlin".into()),
                ..Default::default()
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
    fn additive_known_environment_is_not_treated_as_correction() {
        let strengthened = strengthen_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::UserProfile(UserProfileLearningCandidate {
                    field: UserProfileField::KnownEnvironments,
                    operation: ProfileOperation::Set,
                    value: Some("staging".into()),
                }),
                confidence: 0.96,
                accepted: true,
                merge_with_existing: false,
                reason: "explicit_profile_fact",
            }],
            Some(&UserProfile {
                known_environments: vec!["prod".into()],
                ..Default::default()
            }),
            &[],
        );

        assert_eq!(strengthened[0].reason, "explicit_profile_fact");
    }
}
