//! Cheap candidate formation from typed runtime facts.
//!
//! This is the first concrete Phase 4.9 layer after typed learning evidence:
//! it turns a turn's facts into explicit learning candidates without invoking
//! an additional model on the hot path.

use crate::application::services::learning_evidence_service::{
    LearningEvidenceEnvelope, LearningEvidenceFacet,
};
use crate::application::services::learning_quality_service::LearningCandidateAssessment;
use crate::application::services::user_profile_service::{ProfileFieldPatch, UserProfilePatch};
use crate::domain::memory::MemoryCategory;
use crate::domain::memory_mutation::{MutationCandidate, MutationSource};
use crate::domain::tool_fact::{
    OutcomeStatus, ProfileOperation, ToolFactPayload, TypedToolFact, UserProfileField,
};
use crate::domain::user_profile::UserProfile;

fn precedent_memory_category() -> MemoryCategory {
    MemoryCategory::Custom("precedent".into())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearningCandidate {
    UserProfile(UserProfileLearningCandidate),
    Precedent(PrecedentLearningCandidate),
    RunRecipe(RunRecipeLearningCandidate),
    FailurePattern(FailureLearningCandidate),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UserProfileLearningCandidate {
    pub field: UserProfileField,
    pub operation: ProfileOperation,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrecedentLearningCandidate {
    pub summary: String,
    pub tool_pattern: Vec<String>,
    pub subjects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunRecipeLearningCandidate {
    pub task_family_hint: String,
    pub sample_request: String,
    pub summary: String,
    pub tool_pattern: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FailureLearningCandidate {
    pub summary: String,
    pub failed_tools: Vec<String>,
    pub outcome_statuses: Vec<OutcomeStatus>,
    pub tool_pattern: Vec<String>,
    pub subjects: Vec<String>,
}

pub fn build_learning_candidates(
    user_message: &str,
    assistant_response: &str,
    tool_facts: &[TypedToolFact],
    evidence: &LearningEvidenceEnvelope,
) -> Vec<LearningCandidate> {
    let mut candidates = Vec::new();

    for fact in tool_facts {
        if let ToolFactPayload::UserProfile(profile) = &fact.payload {
            candidates.push(LearningCandidate::UserProfile(
                UserProfileLearningCandidate {
                    field: profile.field.clone(),
                    operation: profile.operation.clone(),
                    value: profile.value.clone(),
                },
            ));
        }
    }

    let tool_pattern = collect_procedural_tool_pattern(tool_facts);
    let subjects = collect_subjects(tool_facts, 4);
    let has_procedural_signal = evidence.facets.iter().any(is_procedural_learning_facet);
    let failed_tools = collect_failed_tools(tool_facts);
    let outcome_statuses = collect_failure_outcome_statuses(tool_facts);

    if evidence.has_failure_outcomes() && !failed_tools.is_empty() {
        candidates.push(LearningCandidate::FailurePattern(
            FailureLearningCandidate {
                summary: format_failure_summary(&failed_tools, &outcome_statuses, &subjects),
                failed_tools: failed_tools.clone(),
                outcome_statuses,
                tool_pattern: if tool_pattern.is_empty() {
                    failed_tools.clone()
                } else {
                    tool_pattern.clone()
                },
                subjects: subjects.clone(),
            },
        ));
    }

    if !evidence.has_failure_outcomes()
        && !tool_pattern.is_empty()
        && evidence.has_actionable_evidence()
        && has_procedural_signal
    {
        let summary = format_precedent_summary(evidence, &tool_pattern, &subjects);
        candidates.push(LearningCandidate::Precedent(PrecedentLearningCandidate {
            summary: summary.clone(),
            tool_pattern: tool_pattern.clone(),
            subjects: subjects.clone(),
        }));

        if tool_pattern.len() >= 2 || evidence.facets.len() >= 2 {
            candidates.push(LearningCandidate::RunRecipe(RunRecipeLearningCandidate {
                task_family_hint: derive_task_family_hint(evidence),
                sample_request: user_message.trim().to_string(),
                summary: format_recipe_summary(assistant_response, &tool_pattern, &subjects),
                tool_pattern,
            }));
        }
    }

    candidates
}

pub fn build_mutation_candidates(candidates: &[LearningCandidate]) -> Vec<MutationCandidate> {
    let mut mutations = Vec::new();
    for candidate in candidates {
        if let LearningCandidate::Precedent(precedent) = candidate {
            mutations.push(MutationCandidate {
                category: precedent_memory_category(),
                text: precedent.summary.clone(),
                confidence: 0.72,
                source: MutationSource::ToolOutput,
            });
        } else if let LearningCandidate::FailurePattern(failure) = candidate {
            mutations.push(MutationCandidate {
                category: MemoryCategory::Reflection,
                text: format!("failure_pattern: {}", failure.summary),
                confidence: 0.74,
                source: MutationSource::Reflection,
            });
        }
    }
    mutations
}

pub fn build_mutation_candidates_from_assessments(
    assessments: &[LearningCandidateAssessment],
) -> Vec<MutationCandidate> {
    assessments
        .iter()
        .filter_map(build_mutation_candidate_from_assessment)
        .collect()
}

pub fn build_mutation_candidate_from_assessment(
    assessment: &LearningCandidateAssessment,
) -> Option<MutationCandidate> {
    if !assessment.accepted {
        return None;
    }
    match &assessment.candidate {
        LearningCandidate::Precedent(precedent) => Some(MutationCandidate {
            category: precedent_memory_category(),
            text: precedent.summary.clone(),
            confidence: assessment.confidence,
            source: MutationSource::ToolOutput,
        }),
        LearningCandidate::FailurePattern(failure) => Some(MutationCandidate {
            category: MemoryCategory::Reflection,
            text: format!("failure_pattern: {}", failure.summary),
            confidence: assessment.confidence,
            source: MutationSource::Reflection,
        }),
        _ => None,
    }
}

pub fn build_user_profile_patch(
    candidates: &[LearningCandidate],
    current: Option<&UserProfile>,
) -> UserProfilePatch {
    let mut patch = UserProfilePatch::default();
    let mut known_environments = current
        .map(|profile| profile.known_environments.clone())
        .unwrap_or_default();
    let mut known_environments_op = None;

    for candidate in candidates {
        let LearningCandidate::UserProfile(profile_candidate) = candidate else {
            continue;
        };
        match (&profile_candidate.field, &profile_candidate.operation) {
            (UserProfileField::PreferredLanguage, ProfileOperation::Set) => {
                if let Some(value) = profile_candidate.value.as_ref() {
                    patch.preferred_language = ProfileFieldPatch::Set(value.clone());
                }
            }
            (UserProfileField::PreferredLanguage, ProfileOperation::Clear) => {
                patch.preferred_language = ProfileFieldPatch::Clear;
            }
            (UserProfileField::Timezone, ProfileOperation::Set) => {
                if let Some(value) = profile_candidate.value.as_ref() {
                    patch.timezone = ProfileFieldPatch::Set(value.clone());
                }
            }
            (UserProfileField::Timezone, ProfileOperation::Clear) => {
                patch.timezone = ProfileFieldPatch::Clear;
            }
            (UserProfileField::DefaultCity, ProfileOperation::Set) => {
                if let Some(value) = profile_candidate.value.as_ref() {
                    patch.default_city = ProfileFieldPatch::Set(value.clone());
                }
            }
            (UserProfileField::DefaultCity, ProfileOperation::Clear) => {
                patch.default_city = ProfileFieldPatch::Clear;
            }
            (UserProfileField::CommunicationStyle, ProfileOperation::Set) => {
                if let Some(value) = profile_candidate.value.as_ref() {
                    patch.communication_style = ProfileFieldPatch::Set(value.clone());
                }
            }
            (UserProfileField::CommunicationStyle, ProfileOperation::Clear) => {
                patch.communication_style = ProfileFieldPatch::Clear;
            }
            (UserProfileField::KnownEnvironments, ProfileOperation::Set) => {
                if let Some(value) = profile_candidate.value.as_ref() {
                    known_environments_op = Some(ProfileOperation::Set);
                    if !known_environments
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(value))
                    {
                        known_environments.push(value.clone());
                    }
                }
            }
            (UserProfileField::KnownEnvironments, ProfileOperation::Clear) => {
                known_environments_op = Some(ProfileOperation::Clear);
                known_environments.clear();
            }
            (UserProfileField::DefaultDeliveryTarget, _) => {
                // Keep structured delivery targets tool-driven for now. The
                // learning bridge only auto-applies string-safe fields.
            }
        }
    }

    match known_environments_op {
        Some(ProfileOperation::Set) => {
            patch.known_environments = ProfileFieldPatch::Set(known_environments);
        }
        Some(ProfileOperation::Clear) => {
            patch.known_environments = ProfileFieldPatch::Clear;
        }
        None => {}
    }

    patch
}

fn collect_subjects(tool_facts: &[TypedToolFact], limit: usize) -> Vec<String> {
    let mut subjects = Vec::new();
    for fact in tool_facts {
        for subject in fact.projected_subjects() {
            if subject.trim().is_empty() {
                continue;
            }
            if !subjects.iter().any(|existing| existing == &subject) {
                subjects.push(subject);
            }
            if subjects.len() >= limit {
                return subjects;
            }
        }
    }
    subjects
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !value.trim().is_empty() && !unique.iter().any(|existing| existing == &value) {
            unique.push(value);
        }
    }
    unique
}

fn collect_procedural_tool_pattern(tool_facts: &[TypedToolFact]) -> Vec<String> {
    unique_strings(
        tool_facts
            .iter()
            .filter(|fact| is_procedural_payload(&fact.payload))
            .map(|fact| fact.tool_id.clone())
            .collect(),
    )
}

fn collect_failed_tools(tool_facts: &[TypedToolFact]) -> Vec<String> {
    unique_strings(
        tool_facts
            .iter()
            .filter_map(|fact| match &fact.payload {
                ToolFactPayload::Outcome(outcome) if outcome.status.is_failure() => {
                    Some(fact.tool_id.clone())
                }
                _ => None,
            })
            .collect(),
    )
}

fn collect_failure_outcome_statuses(tool_facts: &[TypedToolFact]) -> Vec<OutcomeStatus> {
    let mut statuses = Vec::new();
    for fact in tool_facts {
        let ToolFactPayload::Outcome(outcome) = &fact.payload else {
            continue;
        };
        if !outcome.status.is_failure() || statuses.contains(&outcome.status) {
            continue;
        }
        statuses.push(outcome.status.clone());
    }
    statuses
}

fn format_precedent_summary(
    evidence: &LearningEvidenceEnvelope,
    tool_pattern: &[String],
    subjects: &[String],
) -> String {
    let mut parts = vec![format!("tools={}", tool_pattern.join(" -> "))];
    if !subjects.is_empty() {
        parts.push(format!("subjects={}", subjects.join(", ")));
    }
    if !evidence.facets.is_empty() {
        parts.push(format!(
            "facets={}",
            evidence
                .facets
                .iter()
                .map(facet_name)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    parts.join(" | ")
}

fn format_recipe_summary(
    assistant_response: &str,
    tool_pattern: &[String],
    subjects: &[String],
) -> String {
    let response_excerpt = assistant_response
        .split_whitespace()
        .take(24)
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts = vec![format!("pattern={}", tool_pattern.join(" -> "))];
    if !subjects.is_empty() {
        parts.push(format!("subjects={}", subjects.join(", ")));
    }
    if !response_excerpt.is_empty() {
        parts.push(format!("response={response_excerpt}"));
    }
    parts.join(" | ")
}

fn format_failure_summary(
    failed_tools: &[String],
    outcome_statuses: &[OutcomeStatus],
    subjects: &[String],
) -> String {
    let mut parts = vec![format!("failed_tools={}", failed_tools.join(" -> "))];
    if !outcome_statuses.is_empty() {
        parts.push(format!(
            "outcomes={}",
            outcome_statuses
                .iter()
                .map(outcome_status_name)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if !subjects.is_empty() {
        parts.push(format!("subjects={}", subjects.join(", ")));
    }
    parts.join(" | ")
}

fn derive_task_family_hint(evidence: &LearningEvidenceEnvelope) -> String {
    if evidence.facets.is_empty() {
        return "generic".into();
    }
    evidence
        .facets
        .iter()
        .map(facet_name)
        .collect::<Vec<_>>()
        .join("_")
}

fn facet_name(facet: &LearningEvidenceFacet) -> &'static str {
    match facet {
        LearningEvidenceFacet::Focus => "focus",
        LearningEvidenceFacet::Outcome => "outcome",
        LearningEvidenceFacet::Delivery => "delivery",
        LearningEvidenceFacet::Resource => "resource",
        LearningEvidenceFacet::Schedule => "schedule",
        LearningEvidenceFacet::UserProfile => "user_profile",
        LearningEvidenceFacet::Search => "search",
        LearningEvidenceFacet::Workspace => "workspace",
        LearningEvidenceFacet::Knowledge => "knowledge",
        LearningEvidenceFacet::Project => "project",
        LearningEvidenceFacet::Security => "security",
        LearningEvidenceFacet::Routing => "routing",
        LearningEvidenceFacet::Notification => "notification",
    }
}

fn outcome_status_name(status: &OutcomeStatus) -> &'static str {
    match status {
        OutcomeStatus::Succeeded => "succeeded",
        OutcomeStatus::ReportedFailure => "reported_failure",
        OutcomeStatus::RuntimeError => "runtime_error",
        OutcomeStatus::Blocked => "blocked",
        OutcomeStatus::UnknownTool => "unknown_tool",
    }
}

fn is_procedural_learning_facet(facet: &LearningEvidenceFacet) -> bool {
    matches!(
        facet,
        LearningEvidenceFacet::Delivery
            | LearningEvidenceFacet::Resource
            | LearningEvidenceFacet::Schedule
            | LearningEvidenceFacet::Search
            | LearningEvidenceFacet::Workspace
            | LearningEvidenceFacet::Knowledge
            | LearningEvidenceFacet::Project
            | LearningEvidenceFacet::Security
            | LearningEvidenceFacet::Routing
            | LearningEvidenceFacet::Notification
    )
}

fn is_procedural_payload(payload: &ToolFactPayload) -> bool {
    matches!(
        payload,
        ToolFactPayload::Delivery(_)
            | ToolFactPayload::Resource(_)
            | ToolFactPayload::Schedule(_)
            | ToolFactPayload::Search(_)
            | ToolFactPayload::Workspace(_)
            | ToolFactPayload::Knowledge(_)
            | ToolFactPayload::Project(_)
            | ToolFactPayload::Security(_)
            | ToolFactPayload::Routing(_)
            | ToolFactPayload::Notification(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::learning_evidence_service::build_learning_evidence;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::dialogue_state::FocusEntity;
    use crate::domain::tool_fact::{
        DeliveryFact, DeliveryTargetKind, FocusFact, OutcomeStatus, ProfileOperation,
        ToolFactPayload, TypedToolFact, UserProfileFact, UserProfileField,
    };
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn builds_profile_and_recipe_candidates_from_typed_turn_data() {
        let tool_facts = vec![
            TypedToolFact {
                tool_id: "user_profile".into(),
                payload: ToolFactPayload::UserProfile(UserProfileFact {
                    field: UserProfileField::Timezone,
                    operation: ProfileOperation::Set,
                    value: Some("Europe/Berlin".into()),
                }),
            },
            TypedToolFact {
                tool_id: "message_send".into(),
                payload: ToolFactPayload::Delivery(DeliveryFact {
                    target: DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                        channel: "telegram".into(),
                        recipient: "@synapseclaw".into(),
                        thread_ref: None,
                    }),
                    content_bytes: Some(16),
                }),
            },
            TypedToolFact {
                tool_id: "focus".into(),
                payload: ToolFactPayload::Focus(FocusFact {
                    entities: vec![FocusEntity {
                        kind: "city".into(),
                        name: "Berlin".into(),
                        metadata: None,
                    }],
                    subjects: vec!["Berlin".into()],
                }),
            },
        ];
        let evidence = build_learning_evidence(&tool_facts);
        let candidates = build_learning_candidates(
            "send it there and remember my timezone",
            "Sent successfully.",
            &tool_facts,
            &evidence,
        );

        assert!(candidates.iter().any(|candidate| matches!(
            candidate,
            LearningCandidate::UserProfile(UserProfileLearningCandidate {
                field: UserProfileField::Timezone,
                operation: ProfileOperation::Set,
                value,
            }) if value.as_deref() == Some("Europe/Berlin")
        )));
        assert!(candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::Precedent(_))));
        assert!(candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::RunRecipe(_))));
        assert!(candidates.iter().any(|candidate| matches!(
            candidate,
            LearningCandidate::Precedent(PrecedentLearningCandidate { tool_pattern, .. })
                if tool_pattern == &vec!["message_send".to_string()]
        )));
    }

    #[test]
    fn failure_turn_forms_failure_candidate_without_recipe() {
        let tool_facts = vec![
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
            TypedToolFact::outcome("web_fetch", OutcomeStatus::RuntimeError, Some(220)),
        ];
        let evidence = build_learning_evidence(&tool_facts);
        let candidates = build_learning_candidates(
            "Fetch the status page",
            "I could not finish that.",
            &tool_facts,
            &evidence,
        );

        assert!(candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::FailurePattern(_))));
        assert!(!candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::RunRecipe(_))));
        assert!(!candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::Precedent(_))));
    }

    #[test]
    fn converts_precedent_candidates_to_conversation_mutations() {
        let candidates = vec![
            LearningCandidate::Precedent(PrecedentLearningCandidate {
                summary: "tools=web_search -> web_fetch | subjects=Berlin".into(),
                tool_pattern: vec!["web_search".into(), "web_fetch".into()],
                subjects: vec!["Berlin".into()],
            }),
            LearningCandidate::UserProfile(UserProfileLearningCandidate {
                field: UserProfileField::Timezone,
                operation: ProfileOperation::Set,
                value: Some("Europe/Berlin".into()),
            }),
        ];

        let mutations = build_mutation_candidates(&candidates);
        assert_eq!(mutations.len(), 1);
        assert_eq!(
            mutations[0].category,
            MemoryCategory::Custom("precedent".into())
        );
        assert_eq!(mutations[0].source, MutationSource::ToolOutput);
        assert!(mutations[0].text.contains("tools=web_search -> web_fetch"));
    }

    #[test]
    fn builds_mutation_candidates_only_from_accepted_assessments() {
        let assessments = vec![
            LearningCandidateAssessment {
                candidate: LearningCandidate::Precedent(PrecedentLearningCandidate {
                    summary: "tools=web_search".into(),
                    tool_pattern: vec!["web_search".into()],
                    subjects: vec!["Berlin".into()],
                }),
                confidence: 0.81,
                accepted: true,
                reason: "procedural_precedent",
            },
            LearningCandidateAssessment {
                candidate: LearningCandidate::Precedent(PrecedentLearningCandidate {
                    summary: "tools=shell".into(),
                    tool_pattern: vec!["shell".into()],
                    subjects: vec![],
                }),
                confidence: 0.41,
                accepted: false,
                reason: "weak_precedent_signal",
            },
            LearningCandidateAssessment {
                candidate: LearningCandidate::FailurePattern(FailureLearningCandidate {
                    summary: "failed_tools=web_fetch | outcomes=runtime_error".into(),
                    failed_tools: vec!["web_fetch".into()],
                    outcome_statuses: vec![OutcomeStatus::RuntimeError],
                    tool_pattern: vec!["web_fetch".into()],
                    subjects: vec!["status.example.com".into()],
                }),
                confidence: 0.77,
                accepted: true,
                reason: "typed_failure_pattern",
            },
        ];

        let mutations = build_mutation_candidates_from_assessments(&assessments);
        assert_eq!(mutations.len(), 2);
        assert!((mutations[0].confidence - 0.81).abs() < f32::EPSILON);
        assert_eq!(
            mutations[0].category,
            MemoryCategory::Custom("precedent".into())
        );
        assert_eq!(mutations[1].category, MemoryCategory::Reflection);
        assert_eq!(mutations[1].source, MutationSource::Reflection);
    }

    #[test]
    fn builds_safe_user_profile_patch_from_candidates() {
        let candidates = vec![
            LearningCandidate::UserProfile(UserProfileLearningCandidate {
                field: UserProfileField::Timezone,
                operation: ProfileOperation::Set,
                value: Some("Europe/Berlin".into()),
            }),
            LearningCandidate::UserProfile(UserProfileLearningCandidate {
                field: UserProfileField::DefaultCity,
                operation: ProfileOperation::Set,
                value: Some("Berlin".into()),
            }),
        ];

        let patch = build_user_profile_patch(&candidates, None);
        assert!(matches!(
            patch.timezone,
            ProfileFieldPatch::Set(ref value) if value == "Europe/Berlin"
        ));
        assert!(matches!(
            patch.default_city,
            ProfileFieldPatch::Set(ref value) if value == "Berlin"
        ));
    }

    #[test]
    fn profile_only_turn_does_not_create_procedural_candidates() {
        let tool_facts = vec![TypedToolFact {
            tool_id: "user_profile".into(),
            payload: ToolFactPayload::UserProfile(UserProfileFact {
                field: UserProfileField::PreferredLanguage,
                operation: ProfileOperation::Set,
                value: Some("ru".into()),
            }),
        }];
        let evidence = build_learning_evidence(&tool_facts);
        let candidates = build_learning_candidates(
            "remember my language",
            "Saved your language preference.",
            &tool_facts,
            &evidence,
        );

        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| matches!(candidate, LearningCandidate::UserProfile(_)))
                .count(),
            1
        );
        assert!(!candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::Precedent(_))));
        assert!(!candidates
            .iter()
            .any(|candidate| matches!(candidate, LearningCandidate::RunRecipe(_))));
    }

    #[test]
    fn profile_patch_merges_known_environments_with_existing_profile() {
        let candidates = vec![LearningCandidate::UserProfile(
            UserProfileLearningCandidate {
                field: UserProfileField::KnownEnvironments,
                operation: ProfileOperation::Set,
                value: Some("staging".into()),
            },
        )];

        let patch = build_user_profile_patch(
            &candidates,
            Some(&UserProfile {
                known_environments: vec!["prod".into()],
                ..Default::default()
            }),
        );

        assert!(matches!(
            patch.known_environments,
            ProfileFieldPatch::Set(ref values)
                if values == &vec!["prod".to_string(), "staging".to_string()]
        ));
    }
}
