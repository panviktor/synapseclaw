//! Learning evidence envelope built from typed runtime facts.
//!
//! This is the cheap bridge between Phase 4.8 typed runtime state and Phase 4.9
//! self-learning. It deliberately avoids extra model calls on the hot path.

use crate::domain::tool_fact::{ToolFactPayload, TypedToolFact};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningEvidenceFacet {
    Focus,
    Outcome,
    Delivery,
    Resource,
    Schedule,
    UserProfile,
    Search,
    Workspace,
    Knowledge,
    Project,
    Security,
    Routing,
    Notification,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LearningEvidenceEnvelope {
    pub typed_fact_count: usize,
    pub outcome_count: usize,
    pub failure_outcome_count: usize,
    pub projected_subject_count: usize,
    pub focus_entity_count: usize,
    pub profile_update_count: usize,
    pub external_procedural_fact_count: usize,
    pub internal_procedural_fact_count: usize,
    pub facets: Vec<LearningEvidenceFacet>,
}

impl LearningEvidenceEnvelope {
    pub fn has_actionable_evidence(&self) -> bool {
        self.typed_fact_count > 0
            || self.projected_subject_count > 0
            || self.focus_entity_count > 0
            || self.profile_update_count > 0
    }

    pub fn has_failure_outcomes(&self) -> bool {
        self.failure_outcome_count > 0
    }

    pub fn has_external_procedural_evidence(&self) -> bool {
        self.external_procedural_fact_count > 0
    }

    pub fn has_internal_only_procedural_evidence(&self) -> bool {
        self.internal_procedural_fact_count > 0 && self.external_procedural_fact_count == 0
    }
}

pub fn build_learning_evidence(tool_facts: &[TypedToolFact]) -> LearningEvidenceEnvelope {
    let mut envelope = LearningEvidenceEnvelope {
        typed_fact_count: tool_facts.len(),
        ..Default::default()
    };

    for fact in tool_facts {
        push_facet(&mut envelope.facets, facet_for_payload(&fact.payload));
        match classify_procedural_payload(&fact.payload) {
            ProceduralPayloadClass::External => envelope.external_procedural_fact_count += 1,
            ProceduralPayloadClass::Internal => envelope.internal_procedural_fact_count += 1,
            ProceduralPayloadClass::None => {}
        }
        if let ToolFactPayload::Outcome(outcome) = &fact.payload {
            envelope.outcome_count += 1;
            if outcome.status.is_failure() {
                envelope.failure_outcome_count += 1;
            }
        }
        envelope.projected_subject_count += fact.projected_subjects().len();
        envelope.focus_entity_count += fact.projected_focus_entities().len();
        if matches!(fact.payload, ToolFactPayload::UserProfile(_)) {
            envelope.profile_update_count += 1;
        }
    }

    envelope
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProceduralPayloadClass {
    External,
    Internal,
    None,
}

fn facet_for_payload(payload: &ToolFactPayload) -> LearningEvidenceFacet {
    match payload {
        ToolFactPayload::Focus(_) => LearningEvidenceFacet::Focus,
        ToolFactPayload::Outcome(_) => LearningEvidenceFacet::Outcome,
        ToolFactPayload::Delivery(_) => LearningEvidenceFacet::Delivery,
        ToolFactPayload::Resource(_) => LearningEvidenceFacet::Resource,
        ToolFactPayload::Schedule(_) => LearningEvidenceFacet::Schedule,
        ToolFactPayload::UserProfile(_) => LearningEvidenceFacet::UserProfile,
        ToolFactPayload::Search(_) => LearningEvidenceFacet::Search,
        ToolFactPayload::Workspace(_) => LearningEvidenceFacet::Workspace,
        ToolFactPayload::Knowledge(_) => LearningEvidenceFacet::Knowledge,
        ToolFactPayload::Project(_) => LearningEvidenceFacet::Project,
        ToolFactPayload::Security(_) => LearningEvidenceFacet::Security,
        ToolFactPayload::Routing(_) => LearningEvidenceFacet::Routing,
        ToolFactPayload::Notification(_) => LearningEvidenceFacet::Notification,
    }
}

fn classify_procedural_payload(payload: &ToolFactPayload) -> ProceduralPayloadClass {
    match payload {
        ToolFactPayload::Focus(_)
        | ToolFactPayload::Outcome(_)
        | ToolFactPayload::UserProfile(_) => ProceduralPayloadClass::None,
        ToolFactPayload::Search(search) => match search.domain {
            crate::domain::tool_fact::SearchDomain::Memory
            | crate::domain::tool_fact::SearchDomain::Session
            | crate::domain::tool_fact::SearchDomain::Precedent => ProceduralPayloadClass::Internal,
            crate::domain::tool_fact::SearchDomain::Web
            | crate::domain::tool_fact::SearchDomain::Workspace
            | crate::domain::tool_fact::SearchDomain::Knowledge => ProceduralPayloadClass::External,
        },
        ToolFactPayload::Routing(_) => ProceduralPayloadClass::Internal,
        ToolFactPayload::Delivery(_)
        | ToolFactPayload::Resource(_)
        | ToolFactPayload::Schedule(_)
        | ToolFactPayload::Workspace(_)
        | ToolFactPayload::Knowledge(_)
        | ToolFactPayload::Project(_)
        | ToolFactPayload::Security(_)
        | ToolFactPayload::Notification(_) => ProceduralPayloadClass::External,
    }
}

fn push_facet(facets: &mut Vec<LearningEvidenceFacet>, facet: LearningEvidenceFacet) {
    if !facets.contains(&facet) {
        facets.push(facet);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::dialogue_state::FocusEntity;
    use crate::domain::tool_fact::{
        DeliveryFact, DeliveryTargetKind, FocusFact, OutcomeStatus, ProfileOperation, SearchDomain,
        SearchFact, ToolFactPayload, TypedToolFact, UserProfileFact,
    };

    #[test]
    fn builds_evidence_from_typed_facts() {
        let evidence = build_learning_evidence(&[
            TypedToolFact {
                tool_id: "message_send".into(),
                payload: ToolFactPayload::Delivery(DeliveryFact {
                    target: DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                        channel: "telegram".into(),
                        recipient: "@synapseclaw".into(),
                        thread_ref: None,
                    }),
                    content_bytes: Some(32),
                }),
            },
            TypedToolFact {
                tool_id: "user_profile".into(),
                payload: ToolFactPayload::UserProfile(UserProfileFact {
                    key: "project_alias".into(),
                    operation: ProfileOperation::Set,
                    value: Some("Borealis".into()),
                }),
            },
            TypedToolFact::outcome("message_send", OutcomeStatus::ReportedFailure, Some(125)),
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
        ]);

        assert_eq!(evidence.typed_fact_count, 4);
        assert_eq!(evidence.outcome_count, 1);
        assert_eq!(evidence.failure_outcome_count, 1);
        assert_eq!(evidence.profile_update_count, 1);
        assert_eq!(evidence.external_procedural_fact_count, 1);
        assert_eq!(evidence.internal_procedural_fact_count, 0);
        assert!(evidence.focus_entity_count >= 1);
        assert!(evidence.projected_subject_count >= 2);
        assert!(evidence.facets.contains(&LearningEvidenceFacet::Delivery));
        assert!(evidence.facets.contains(&LearningEvidenceFacet::Outcome));
        assert!(evidence
            .facets
            .contains(&LearningEvidenceFacet::UserProfile));
        assert!(evidence.facets.contains(&LearningEvidenceFacet::Focus));
        assert!(evidence.has_actionable_evidence());
    }

    #[test]
    fn distinguishes_internal_and_external_procedural_evidence() {
        let evidence = build_learning_evidence(&[
            TypedToolFact {
                tool_id: "memory_recall".into(),
                payload: ToolFactPayload::Search(SearchFact {
                    domain: SearchDomain::Memory,
                    query: Some("reflective_memory_topic".into()),
                    result_count: Some(3),
                    primary_locator: Some("daily_123".into()),
                }),
            },
            TypedToolFact {
                tool_id: "web_search".into(),
                payload: ToolFactPayload::Search(SearchFact {
                    domain: SearchDomain::Web,
                    query: Some("status page".into()),
                    result_count: Some(2),
                    primary_locator: Some("https://status.example.com".into()),
                }),
            },
        ]);

        assert_eq!(evidence.internal_procedural_fact_count, 1);
        assert_eq!(evidence.external_procedural_fact_count, 1);
        assert!(evidence.has_external_procedural_evidence());
        assert!(!evidence.has_internal_only_procedural_evidence());
    }
}
