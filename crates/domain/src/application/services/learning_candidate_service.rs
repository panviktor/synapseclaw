//! Cheap candidate formation from typed runtime facts.
//!
//! This is the first concrete Phase 4.9 layer after typed learning evidence:
//! it turns a turn's facts into explicit learning candidates without invoking
//! an additional model on the hot path.

use crate::application::services::learning_evidence_service::{
    LearningEvidenceEnvelope, LearningEvidenceFacet,
};
use crate::domain::tool_fact::{ProfileOperation, ToolFactPayload, TypedToolFact, UserProfileField};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearningCandidate {
    UserProfile(UserProfileLearningCandidate),
    Precedent(PrecedentLearningCandidate),
    RunRecipe(RunRecipeLearningCandidate),
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

pub fn build_learning_candidates(
    user_message: &str,
    assistant_response: &str,
    tools_used: &[String],
    tool_facts: &[TypedToolFact],
    evidence: &LearningEvidenceEnvelope,
) -> Vec<LearningCandidate> {
    let mut candidates = Vec::new();

    for fact in tool_facts {
        if let ToolFactPayload::UserProfile(profile) = &fact.payload {
            candidates.push(LearningCandidate::UserProfile(UserProfileLearningCandidate {
                field: profile.field.clone(),
                operation: profile.operation.clone(),
                value: profile.value.clone(),
            }));
        }
    }

    let tool_pattern = unique_strings(tools_used.to_vec());
    let subjects = collect_subjects(tool_facts, 4);

    if !tool_pattern.is_empty() && evidence.has_actionable_evidence() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::learning_evidence_service::build_learning_evidence;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::dialogue_state::FocusEntity;
    use crate::domain::tool_fact::{
        DeliveryFact, DeliveryTargetKind, FocusFact, ProfileOperation, ToolFactPayload,
        TypedToolFact, UserProfileFact, UserProfileField,
    };

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
        let tools_used = vec!["user_profile".into(), "message_send".into()];

        let candidates = build_learning_candidates(
            "send it there and remember my timezone",
            "Sent successfully.",
            &tools_used,
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
    }
}
