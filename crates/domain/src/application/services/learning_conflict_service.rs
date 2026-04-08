//! Conflict resolution for cheap learning assessments.
//!
//! This layer removes obviously contradictory candidates before they mutate
//! profile state or feed downstream learning surfaces.

use crate::application::services::learning_candidate_service::LearningCandidate;
use crate::application::services::learning_quality_service::LearningCandidateAssessment;
use crate::domain::tool_fact::{ProfileOperation, UserProfileField};

const CONFLICT_REASON: &str = "conflicting_profile_candidates";

pub fn resolve_learning_conflicts(
    assessments: &[LearningCandidateAssessment],
) -> Vec<LearningCandidateAssessment> {
    let mut resolved = assessments.to_vec();

    reject_conflicting_scalar_profile_candidates(&mut resolved);
    reject_conflicting_known_environment_candidates(&mut resolved);

    resolved
}

fn reject_conflicting_scalar_profile_candidates(assessments: &mut [LearningCandidateAssessment]) {
    let mut groups: Vec<(UserProfileField, Vec<usize>)> = Vec::new();
    for (index, assessment) in assessments.iter().enumerate() {
        let LearningCandidate::UserProfile(candidate) = &assessment.candidate else {
            continue;
        };
        if !assessment.accepted || matches!(candidate.field, UserProfileField::KnownEnvironments) {
            continue;
        }
        if let Some((_, indices)) = groups
            .iter_mut()
            .find(|(field, _)| field == &candidate.field)
        {
            indices.push(index);
        } else {
            groups.push((candidate.field.clone(), vec![index]));
        }
    }

    for (_, indices) in groups {
        if indices.len() <= 1 {
            continue;
        }
        let mut signatures = Vec::new();
        for peer_index in &indices {
            let LearningCandidate::UserProfile(candidate) = &assessments[*peer_index].candidate
            else {
                continue;
            };
            let signature = (
                candidate.operation.clone(),
                candidate
                    .value
                    .as_ref()
                    .map(|value| value.trim().to_ascii_lowercase()),
            );
            if !signatures.iter().any(|existing| existing == &signature) {
                signatures.push(signature);
            }
        }
        if signatures.len() > 1 {
            mark_conflicted(assessments, &indices);
        }
    }
}

fn reject_conflicting_known_environment_candidates(
    assessments: &mut [LearningCandidateAssessment],
) {
    let indices = assessments
        .iter()
        .enumerate()
        .filter_map(|(index, assessment)| match &assessment.candidate {
            LearningCandidate::UserProfile(candidate)
                if assessment.accepted
                    && matches!(candidate.field, UserProfileField::KnownEnvironments) =>
            {
                Some(index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    if indices.is_empty() {
        return;
    }

    let has_clear = indices.iter().any(|index| {
        matches!(
            &assessments[*index].candidate,
            LearningCandidate::UserProfile(candidate)
                if matches!(candidate.operation, ProfileOperation::Clear)
        )
    });
    let has_set = indices.iter().any(|index| {
        matches!(
            &assessments[*index].candidate,
            LearningCandidate::UserProfile(candidate)
                if matches!(candidate.operation, ProfileOperation::Set)
        )
    });

    if has_clear && has_set {
        mark_conflicted(assessments, &indices);
    }
}

fn mark_conflicted(assessments: &mut [LearningCandidateAssessment], indices: &[usize]) {
    for index in indices {
        if let Some(assessment) = assessments.get_mut(*index) {
            assessment.accepted = false;
            assessment.merge_with_existing = false;
            assessment.confidence = assessment.confidence.min(0.5);
            assessment.reason = CONFLICT_REASON;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::learning_candidate_service::UserProfileLearningCandidate;

    fn assessment(
        field: UserProfileField,
        operation: ProfileOperation,
        value: Option<&str>,
    ) -> LearningCandidateAssessment {
        LearningCandidateAssessment {
            candidate: LearningCandidate::UserProfile(UserProfileLearningCandidate {
                field,
                operation,
                value: value.map(str::to_string),
            }),
            confidence: 0.96,
            accepted: true,
            merge_with_existing: false,
            reason: "explicit_profile_fact",
        }
    }

    #[test]
    fn rejects_conflicting_scalar_profile_updates() {
        let resolved = resolve_learning_conflicts(&[
            assessment(
                UserProfileField::Timezone,
                ProfileOperation::Set,
                Some("Europe/Berlin"),
            ),
            assessment(
                UserProfileField::Timezone,
                ProfileOperation::Set,
                Some("Europe/Paris"),
            ),
        ]);

        assert!(resolved.iter().all(|assessment| !assessment.accepted));
        assert!(resolved
            .iter()
            .all(|assessment| assessment.reason == CONFLICT_REASON));
    }

    #[test]
    fn keeps_additive_known_environment_sets() {
        let resolved = resolve_learning_conflicts(&[
            assessment(
                UserProfileField::KnownEnvironments,
                ProfileOperation::Set,
                Some("prod"),
            ),
            assessment(
                UserProfileField::KnownEnvironments,
                ProfileOperation::Set,
                Some("staging"),
            ),
        ]);

        assert!(resolved.iter().all(|assessment| assessment.accepted));
    }

    #[test]
    fn rejects_known_environment_clear_plus_set() {
        let resolved = resolve_learning_conflicts(&[
            assessment(
                UserProfileField::KnownEnvironments,
                ProfileOperation::Clear,
                None,
            ),
            assessment(
                UserProfileField::KnownEnvironments,
                ProfileOperation::Set,
                Some("prod"),
            ),
        ]);

        assert!(resolved.iter().all(|assessment| !assessment.accepted));
        assert!(resolved
            .iter()
            .all(|assessment| assessment.reason == CONFLICT_REASON));
    }
}
