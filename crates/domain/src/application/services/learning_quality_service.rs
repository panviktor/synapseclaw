//! Quality gating for typed learning candidates.
//!
//! Candidate formation should stay cheap and permissive. This layer decides
//! which candidates are strong enough to write immediately, which ones should
//! merge with existing procedural memory, and which ones should be deferred.

use crate::application::services::learning_candidate_service::LearningCandidate;
use crate::application::services::learning_evidence_service::LearningEvidenceEnvelope;
use crate::domain::run_recipe::RunRecipe;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LearningCandidateAssessment {
    pub candidate: LearningCandidate,
    pub confidence: f32,
    pub accepted: bool,
    pub reason: &'static str,
}

pub fn assess_learning_candidates(
    candidates: &[LearningCandidate],
    evidence: &LearningEvidenceEnvelope,
    existing_recipes: &[RunRecipe],
) -> Vec<LearningCandidateAssessment> {
    candidates
        .iter()
        .cloned()
        .map(|candidate| match &candidate {
            LearningCandidate::UserProfile(_) => LearningCandidateAssessment {
                candidate,
                confidence: 0.96,
                accepted: true,
                reason: "explicit_profile_fact",
            },
            LearningCandidate::Precedent(precedent) => {
                let subject_bonus = (!precedent.subjects.is_empty()) as u8 as f32 * 0.08;
                let pattern_bonus = (precedent.tool_pattern.len().min(3) as f32) * 0.08;
                let facet_bonus = (evidence.facets.len().min(3) as f32) * 0.04;
                let confidence = (0.56 + subject_bonus + pattern_bonus + facet_bonus).min(0.86);
                let accepted = !precedent.tool_pattern.is_empty()
                    && (!precedent.subjects.is_empty() || evidence.facets.len() >= 2)
                    && confidence >= 0.68;
                let reason = if accepted {
                    "procedural_precedent"
                } else {
                    "weak_precedent_signal"
                };
                LearningCandidateAssessment {
                    candidate,
                    confidence,
                    accepted,
                    reason,
                }
            }
            LearningCandidate::RunRecipe(recipe) => {
                let matching_existing = existing_recipes
                    .iter()
                    .filter(|existing| existing.task_family == recipe.task_family_hint)
                    .collect::<Vec<_>>();
                let best_existing = matching_existing
                    .iter()
                    .map(|existing| {
                        tool_pattern_similarity(&existing.tool_pattern, &recipe.tool_pattern)
                    })
                    .fold(0.0_f32, f32::max);
                let pattern_bonus = (recipe.tool_pattern.len().min(4) as f32) * 0.08;
                let facet_bonus = (evidence.facets.len().min(3) as f32) * 0.04;
                let merge_bonus = if best_existing >= 0.9 { 0.1 } else { 0.0 };
                let confidence = (0.52 + pattern_bonus + facet_bonus + merge_bonus).min(0.9);
                let diverged_existing = !matching_existing.is_empty() && best_existing < 0.45;
                let ambiguous_existing =
                    !matching_existing.is_empty() && (0.45..0.9).contains(&best_existing);
                let accepted = recipe.tool_pattern.len() >= 2
                    && !diverged_existing
                    && !ambiguous_existing
                    && confidence >= 0.7;
                let reason = if diverged_existing {
                    "diverged_existing_recipe"
                } else if ambiguous_existing {
                    "ambiguous_existing_recipe"
                } else if accepted && best_existing >= 0.9 {
                    "merge_existing_recipe"
                } else if accepted {
                    "strong_recipe_pattern"
                } else {
                    "weak_recipe_pattern"
                };
                LearningCandidateAssessment {
                    candidate,
                    confidence,
                    accepted,
                    reason,
                }
            }
            LearningCandidate::FailurePattern(failure) => {
                let failed_tool_bonus = (failure.failed_tools.len().min(2) as f32) * 0.1;
                let subject_bonus = (!failure.subjects.is_empty()) as u8 as f32 * 0.08;
                let pattern_bonus = (!failure.tool_pattern.is_empty()) as u8 as f32 * 0.06;
                let status_bonus = (!failure.outcome_statuses.is_empty()) as u8 as f32 * 0.06;
                let confidence =
                    (0.42 + failed_tool_bonus + subject_bonus + pattern_bonus + status_bonus)
                        .min(0.86);
                let accepted = !failure.failed_tools.is_empty() && confidence >= 0.66;
                let reason = if accepted {
                    "typed_failure_pattern"
                } else {
                    "weak_failure_signal"
                };
                LearningCandidateAssessment {
                    candidate,
                    confidence,
                    accepted,
                    reason,
                }
            }
        })
        .collect()
}

fn tool_pattern_similarity(left: &[String], right: &[String]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left
        .iter()
        .filter(|item| right.iter().any(|other| other == *item))
        .count() as f32;
    let union = left
        .iter()
        .chain(right.iter())
        .fold(Vec::<&String>::new(), |mut seen, item| {
            if !seen.contains(&item) {
                seen.push(item);
            }
            seen
        })
        .len() as f32;
    if union <= f32::EPSILON {
        0.0
    } else {
        shared / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::learning_candidate_service::{
        FailureLearningCandidate, PrecedentLearningCandidate, RunRecipeLearningCandidate,
        UserProfileLearningCandidate,
    };
    use crate::application::services::learning_evidence_service::LearningEvidenceFacet;
    use crate::domain::tool_fact::{OutcomeStatus, ProfileOperation, UserProfileField};

    #[test]
    fn accepts_profile_candidates_immediately() {
        let assessments = assess_learning_candidates(
            &[LearningCandidate::UserProfile(
                UserProfileLearningCandidate {
                    field: UserProfileField::Timezone,
                    operation: ProfileOperation::Set,
                    value: Some("Europe/Berlin".into()),
                },
            )],
            &LearningEvidenceEnvelope::default(),
            &[],
        );

        assert_eq!(assessments.len(), 1);
        assert!(assessments[0].accepted);
        assert_eq!(assessments[0].reason, "explicit_profile_fact");
    }

    #[test]
    fn rejects_diverged_recipe_candidate_against_existing_task_family() {
        let assessments = assess_learning_candidates(
            &[LearningCandidate::RunRecipe(RunRecipeLearningCandidate {
                task_family_hint: "delivery_search".into(),
                sample_request: "send status".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
            })],
            &LearningEvidenceEnvelope {
                facets: vec![
                    LearningEvidenceFacet::Search,
                    LearningEvidenceFacet::Delivery,
                ],
                ..Default::default()
            },
            &[RunRecipe {
                agent_id: "agent".into(),
                task_family: "delivery_search".into(),
                sample_request: "restart service".into(),
                summary: "pattern=shell -> file_edit".into(),
                tool_pattern: vec!["shell".into(), "file_edit".into()],
                success_count: 3,
                updated_at: 1,
            }],
        );

        assert_eq!(assessments.len(), 1);
        assert!(!assessments[0].accepted);
        assert_eq!(assessments[0].reason, "diverged_existing_recipe");
    }

    #[test]
    fn rejects_ambiguous_recipe_candidate_until_merge_logic_is_clear() {
        let assessments = assess_learning_candidates(
            &[LearningCandidate::RunRecipe(RunRecipeLearningCandidate {
                task_family_hint: "delivery_search".into(),
                sample_request: "send status".into(),
                summary: "pattern=web_search -> web_fetch -> message_send".into(),
                tool_pattern: vec![
                    "web_search".into(),
                    "web_fetch".into(),
                    "message_send".into(),
                ],
            })],
            &LearningEvidenceEnvelope {
                facets: vec![
                    LearningEvidenceFacet::Search,
                    LearningEvidenceFacet::Delivery,
                ],
                ..Default::default()
            },
            &[RunRecipe {
                agent_id: "agent".into(),
                task_family: "delivery_search".into(),
                sample_request: "search and send".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                success_count: 2,
                updated_at: 1,
            }],
        );

        assert_eq!(assessments.len(), 1);
        assert!(!assessments[0].accepted);
        assert_eq!(assessments[0].reason, "ambiguous_existing_recipe");
    }

    #[test]
    fn accepts_high_similarity_recipe_as_merge_candidate() {
        let assessments = assess_learning_candidates(
            &[LearningCandidate::RunRecipe(RunRecipeLearningCandidate {
                task_family_hint: "delivery_search".into(),
                sample_request: "send status".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
            })],
            &LearningEvidenceEnvelope {
                facets: vec![
                    LearningEvidenceFacet::Search,
                    LearningEvidenceFacet::Delivery,
                ],
                ..Default::default()
            },
            &[RunRecipe {
                agent_id: "agent".into(),
                task_family: "delivery_search".into(),
                sample_request: "search and send".into(),
                summary: "pattern=web_search -> message_send".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                success_count: 2,
                updated_at: 1,
            }],
        );

        assert_eq!(assessments.len(), 1);
        assert!(assessments[0].accepted);
        assert_eq!(assessments[0].reason, "merge_existing_recipe");
    }

    #[test]
    fn accepts_strong_precedent_with_subjects_and_pattern() {
        let assessments = assess_learning_candidates(
            &[LearningCandidate::Precedent(PrecedentLearningCandidate {
                summary: "tools=web_search -> message_send | subjects=status.example.com".into(),
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                subjects: vec!["status.example.com".into()],
            })],
            &LearningEvidenceEnvelope {
                facets: vec![
                    LearningEvidenceFacet::Search,
                    LearningEvidenceFacet::Delivery,
                ],
                ..Default::default()
            },
            &[],
        );

        assert_eq!(assessments.len(), 1);
        assert!(assessments[0].accepted);
        assert_eq!(assessments[0].reason, "procedural_precedent");
    }

    #[test]
    fn accepts_typed_failure_pattern_with_structured_context() {
        let assessments = assess_learning_candidates(
            &[LearningCandidate::FailurePattern(FailureLearningCandidate {
                summary: "failed_tools=web_fetch | outcomes=runtime_error | subjects=status.example.com".into(),
                failed_tools: vec!["web_fetch".into()],
                outcome_statuses: vec![OutcomeStatus::RuntimeError],
                tool_pattern: vec!["web_fetch".into()],
                subjects: vec!["status.example.com".into()],
            })],
            &LearningEvidenceEnvelope {
                facets: vec![LearningEvidenceFacet::Outcome, LearningEvidenceFacet::Resource],
                failure_outcome_count: 1,
                ..Default::default()
            },
            &[],
        );

        assert_eq!(assessments.len(), 1);
        assert!(assessments[0].accepted);
        assert_eq!(assessments[0].reason, "typed_failure_pattern");
    }
}
