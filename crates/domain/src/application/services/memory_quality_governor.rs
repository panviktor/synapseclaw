use crate::application::services::learning_candidate_service::LearningCandidate;
use crate::application::services::learning_evidence_service::LearningEvidenceEnvelope;
use crate::application::services::learning_quality_service::LearningCandidateAssessment;
use crate::application::services::turn_markup::leading_media_control_marker;
use crate::domain::memory::{MemoryCategory, ReflectionOutcome};
use crate::domain::memory_mutation::{MutationCandidate, MutationSource, MutationWriteClass};
use crate::domain::util::{is_low_information_repetition, should_skip_autosave_content};
use std::collections::HashMap;

pub const AUTOSAVE_MIN_CONTENT_CHARS: usize = 20;
pub const CONSOLIDATION_MIN_USER_CHARS: usize = 20;
pub const REFLECTION_MIN_USER_CHARS: usize = 30;
pub const REFLECTION_MIN_RESPONSE_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityStorageVerdict {
    Accept,
    Reject(EntityRejectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityRejectReason {
    MissingName,
    MissingType,
    UnanchoredGenericConcept,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationshipStorageVerdict {
    Accept,
    Reject(RelationshipRejectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationshipRejectReason {
    MissingSubject,
    MissingObject,
    MissingPredicate,
    SelfReference,
    PredicateTooLong,
    AbstractConceptPair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAssessmentRejectReason {
    InternalOnlyProceduralEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionStartVerdict {
    Start,
    Skip(ReflectionSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionSkipReason {
    InternalOnlyProceduralEvidence,
    NoMeaningfulEvidence,
    UserMessageTooShort,
    AssistantResponseTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidationStartVerdict {
    Start,
    Skip(ConsolidationSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidationSkipReason {
    InternalOnlyProceduralEvidence,
    NoMeaningfulEvidence,
    UserMessageTooShort,
    LowInformationRepetition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutosaveWriteVerdict {
    Write,
    Skip(AutosaveSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutosaveSkipReason {
    TooShort,
    SyntheticNoise,
    StructuredControlTurn,
    LowInformationRepetition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundLearningInputVerdict {
    Allow,
    Skip(AutosaveSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMutationVerdict {
    Accept(MutationWriteClass),
    Reject(MemoryMutationRejectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryMutationRejectReason {
    LowInformationRepetition,
    EphemeralRepairTrace,
    GenericDialogueNotDurable,
}

pub fn assess_extracted_entity(name: &str, entity_type: &str) -> EntityStorageVerdict {
    assess_extracted_entity_with_context(name, entity_type, false)
}

pub fn assess_extracted_entity_with_context(
    name: &str,
    entity_type: &str,
    is_accepted_relationship_endpoint: bool,
) -> EntityStorageVerdict {
    if name.trim().is_empty() {
        return EntityStorageVerdict::Reject(EntityRejectReason::MissingName);
    }
    if entity_type.trim().is_empty() {
        return EntityStorageVerdict::Reject(EntityRejectReason::MissingType);
    }
    if !is_accepted_relationship_endpoint && entity_type.trim().eq_ignore_ascii_case("concept") {
        return EntityStorageVerdict::Reject(EntityRejectReason::UnanchoredGenericConcept);
    }

    EntityStorageVerdict::Accept
}

pub fn assess_extracted_relationship(
    subject: &str,
    predicate: &str,
    object: &str,
    _confidence: f32,
    entity_types: &HashMap<String, String>,
) -> RelationshipStorageVerdict {
    if subject.trim().is_empty() {
        return RelationshipStorageVerdict::Reject(RelationshipRejectReason::MissingSubject);
    }
    if object.trim().is_empty() {
        return RelationshipStorageVerdict::Reject(RelationshipRejectReason::MissingObject);
    }
    if predicate.trim().is_empty() {
        return RelationshipStorageVerdict::Reject(RelationshipRejectReason::MissingPredicate);
    }
    if subject.trim().eq_ignore_ascii_case(object.trim()) {
        return RelationshipStorageVerdict::Reject(RelationshipRejectReason::SelfReference);
    }
    if predicate.chars().count() > 64 {
        return RelationshipStorageVerdict::Reject(RelationshipRejectReason::PredicateTooLong);
    }

    let subject_type = entity_types.get(&subject.trim().to_lowercase());
    let object_type = entity_types.get(&object.trim().to_lowercase());
    if matches!(subject_type, Some(kind) if kind == "concept")
        && matches!(object_type, Some(kind) if kind == "concept")
    {
        return RelationshipStorageVerdict::Reject(RelationshipRejectReason::AbstractConceptPair);
    }

    RelationshipStorageVerdict::Accept
}

pub fn govern_learning_assessments(
    assessments: &[LearningCandidateAssessment],
    evidence: &LearningEvidenceEnvelope,
) -> Vec<LearningCandidateAssessment> {
    assessments
        .iter()
        .cloned()
        .map(|mut assessment| {
            if !assessment.accepted {
                return assessment;
            }
            if let Some(reason) = reject_learning_assessment(&assessment.candidate, evidence) {
                assessment.accepted = false;
                assessment.merge_with_existing = false;
                assessment.reason = learning_assessment_reject_reason_name(reason);
            }
            assessment
        })
        .collect()
}

pub fn assess_reflection_start(
    evidence: &LearningEvidenceEnvelope,
    user_chars: usize,
    assistant_response_len: usize,
    min_user_chars: usize,
    min_response_len: usize,
) -> ReflectionStartVerdict {
    if user_chars < min_user_chars {
        return ReflectionStartVerdict::Skip(ReflectionSkipReason::UserMessageTooShort);
    }
    if evidence.has_failure_outcomes() {
        return ReflectionStartVerdict::Start;
    }
    if assistant_response_len < min_response_len {
        return ReflectionStartVerdict::Skip(ReflectionSkipReason::AssistantResponseTooShort);
    }
    if evidence.has_external_procedural_evidence() {
        return ReflectionStartVerdict::Start;
    }
    if evidence.has_internal_only_procedural_evidence() {
        return ReflectionStartVerdict::Skip(ReflectionSkipReason::InternalOnlyProceduralEvidence);
    }
    ReflectionStartVerdict::Skip(ReflectionSkipReason::NoMeaningfulEvidence)
}

pub fn assess_consolidation_start(
    evidence: &LearningEvidenceEnvelope,
    user_message: &str,
    min_user_chars: usize,
) -> ConsolidationStartVerdict {
    let user_chars = user_message.chars().count();
    if user_chars < min_user_chars {
        return ConsolidationStartVerdict::Skip(ConsolidationSkipReason::UserMessageTooShort);
    }
    if evidence.has_internal_only_procedural_evidence() {
        return ConsolidationStartVerdict::Skip(
            ConsolidationSkipReason::InternalOnlyProceduralEvidence,
        );
    }
    if evidence.has_failure_outcomes()
        || evidence.has_external_procedural_evidence()
        || evidence.profile_update_count > 0
        || evidence.focus_entity_count > 0
        || evidence.projected_subject_count > 0
    {
        return ConsolidationStartVerdict::Start;
    }
    if is_low_information_repetition(user_message) {
        return ConsolidationStartVerdict::Skip(ConsolidationSkipReason::LowInformationRepetition);
    }
    if user_chars > 0 {
        return ConsolidationStartVerdict::Start;
    }
    ConsolidationStartVerdict::Skip(ConsolidationSkipReason::NoMeaningfulEvidence)
}

pub fn assess_autosave_write(content: &str, min_chars: usize) -> AutosaveWriteVerdict {
    let trimmed = content.trim();
    if let BackgroundLearningInputVerdict::Skip(reason) = assess_background_learning_input(trimmed)
    {
        return AutosaveWriteVerdict::Skip(reason);
    }
    if trimmed.chars().count() < min_chars {
        return AutosaveWriteVerdict::Skip(AutosaveSkipReason::TooShort);
    }
    AutosaveWriteVerdict::Write
}

pub fn assess_background_learning_input(content: &str) -> BackgroundLearningInputVerdict {
    let trimmed = content.trim();
    if should_skip_autosave_content(trimmed) {
        return BackgroundLearningInputVerdict::Skip(AutosaveSkipReason::SyntheticNoise);
    }
    if trimmed.starts_with('/') || leading_media_control_marker(trimmed) {
        return BackgroundLearningInputVerdict::Skip(AutosaveSkipReason::StructuredControlTurn);
    }
    if is_low_information_repetition(trimmed) {
        return BackgroundLearningInputVerdict::Skip(AutosaveSkipReason::LowInformationRepetition);
    }
    BackgroundLearningInputVerdict::Allow
}

pub fn assess_memory_mutation_candidate(candidate: &MutationCandidate) -> MemoryMutationVerdict {
    let write_class = candidate
        .write_class
        .unwrap_or_else(|| infer_memory_write_class(candidate));

    if matches!(write_class, MutationWriteClass::EphemeralRepairTrace) {
        return MemoryMutationVerdict::Reject(MemoryMutationRejectReason::EphemeralRepairTrace);
    }

    if is_low_information_repetition(&candidate.text) {
        return MemoryMutationVerdict::Reject(MemoryMutationRejectReason::LowInformationRepetition);
    }

    if matches!(candidate.source, MutationSource::Consolidation) {
        if matches!(write_class, MutationWriteClass::GenericDialogue) {
            return MemoryMutationVerdict::Reject(
                MemoryMutationRejectReason::GenericDialogueNotDurable,
            );
        }
    }

    MemoryMutationVerdict::Accept(write_class)
}

pub fn memory_mutation_reject_reason_name(reason: MemoryMutationRejectReason) -> &'static str {
    match reason {
        MemoryMutationRejectReason::LowInformationRepetition => "low_information_repetition",
        MemoryMutationRejectReason::EphemeralRepairTrace => "ephemeral_repair_trace_not_durable",
        MemoryMutationRejectReason::GenericDialogueNotDurable => "generic_dialogue_not_durable",
    }
}

pub fn retrieval_noise_score_delta(category: &MemoryCategory, lexical_anchor_bonus: f64) -> f64 {
    if lexical_anchor_bonus > 0.0 {
        return 0.0;
    }

    match category {
        MemoryCategory::Daily => -0.10,
        MemoryCategory::Custom(name) if name == "precedent" => -0.16,
        _ => 0.0,
    }
}

pub fn retrieval_content_noise_score_delta(content: &str) -> f64 {
    if is_low_information_repetition(content) {
        -0.24
    } else {
        0.0
    }
}

pub fn derive_reflection_outcome(
    evidence: &LearningEvidenceEnvelope,
    tools_used: &[String],
) -> ReflectionOutcome {
    if evidence.has_failure_outcomes() {
        return ReflectionOutcome::Failure;
    }
    if evidence.has_external_procedural_evidence() {
        return ReflectionOutcome::Success;
    }
    if evidence.has_actionable_evidence() || !tools_used.is_empty() {
        return ReflectionOutcome::Partial;
    }
    ReflectionOutcome::Success
}

fn reject_learning_assessment(
    candidate: &LearningCandidate,
    evidence: &LearningEvidenceEnvelope,
) -> Option<LearningAssessmentRejectReason> {
    match candidate {
        LearningCandidate::Precedent(_) | LearningCandidate::RunRecipe(_)
            if !evidence.has_external_procedural_evidence()
                && evidence.has_internal_only_procedural_evidence() =>
        {
            Some(LearningAssessmentRejectReason::InternalOnlyProceduralEvidence)
        }
        _ => None,
    }
}

fn infer_memory_write_class(candidate: &MutationCandidate) -> MutationWriteClass {
    match (&candidate.category, &candidate.source) {
        (MemoryCategory::Custom(name), _) if name == "failure_pattern" => {
            MutationWriteClass::FailurePattern
        }
        (MemoryCategory::Custom(name), _) if name == "precedent" => MutationWriteClass::Recipe,
        (_, MutationSource::Reflection) => MutationWriteClass::FailurePattern,
        (_, MutationSource::ToolOutput) => MutationWriteClass::FactAnchor,
        (_, MutationSource::ExplicitUser) => MutationWriteClass::FactAnchor,
        (MemoryCategory::Core, MutationSource::Consolidation) => {
            MutationWriteClass::GenericDialogue
        }
        _ => MutationWriteClass::FactAnchor,
    }
}

fn learning_assessment_reject_reason_name(reason: LearningAssessmentRejectReason) -> &'static str {
    match reason {
        LearningAssessmentRejectReason::InternalOnlyProceduralEvidence => {
            "internal_only_procedural_turn"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::learning_candidate_service::{
        LearningCandidate, PrecedentLearningCandidate,
    };
    use crate::application::services::learning_evidence_service::LearningEvidenceFacet;
    use crate::application::services::learning_quality_service::LearningCandidateAssessment;
    use crate::domain::memory_mutation::{MutationCandidate, MutationSource, MutationWriteClass};

    #[test]
    fn rejects_empty_entity_name() {
        assert_eq!(
            assess_extracted_entity(" ", "person"),
            EntityStorageVerdict::Reject(EntityRejectReason::MissingName)
        );
    }

    #[test]
    fn rejects_concept_pair_relationships() {
        let entity_types = HashMap::from([
            ("abstract_topic_a".to_string(), "concept".to_string()),
            ("abstract_topic_b".to_string(), "concept".to_string()),
        ]);

        assert_eq!(
            assess_extracted_relationship(
                "abstract_topic_a",
                "relates_to",
                "abstract_topic_b",
                0.9,
                &entity_types
            ),
            RelationshipStorageVerdict::Reject(RelationshipRejectReason::AbstractConceptPair)
        );
    }

    #[test]
    fn rejects_standalone_generic_concept_entities_without_graph_anchor() {
        assert_eq!(
            assess_extracted_entity_with_context("Meaning", "concept", false),
            EntityStorageVerdict::Reject(EntityRejectReason::UnanchoredGenericConcept)
        );
        assert_eq!(
            assess_extracted_entity_with_context("Meaning", "concept", true),
            EntityStorageVerdict::Accept
        );
    }

    #[test]
    fn rejects_concept_pair_relationships_even_at_high_confidence() {
        let entity_types = HashMap::from([
            ("meaning".to_string(), "concept".to_string()),
            ("responsibility".to_string(), "concept".to_string()),
        ]);

        assert_eq!(
            assess_extracted_relationship(
                "Meaning",
                "relates_to",
                "Responsibility",
                0.99,
                &entity_types
            ),
            RelationshipStorageVerdict::Reject(RelationshipRejectReason::AbstractConceptPair)
        );
    }

    #[test]
    fn rejects_generic_children_parent_relationship_when_marked_as_concepts() {
        let entity_types = HashMap::from([
            ("children".to_string(), "concept".to_string()),
            ("parents".to_string(), "concept".to_string()),
        ]);

        assert_eq!(
            assess_extracted_relationship("Children", "learn_from", "Parents", 0.99, &entity_types),
            RelationshipStorageVerdict::Reject(RelationshipRejectReason::AbstractConceptPair)
        );
    }

    #[test]
    fn accepts_concrete_relationship() {
        let entity_types = HashMap::from([
            ("victor".to_string(), "person".to_string()),
            ("rust".to_string(), "concept".to_string()),
        ]);

        assert_eq!(
            assess_extracted_relationship("Victor", "prefers", "Rust", 0.8, &entity_types),
            RelationshipStorageVerdict::Accept
        );
    }

    #[test]
    fn rejects_internal_only_procedural_learning_assessment() {
        let assessments = govern_learning_assessments(
            &[LearningCandidateAssessment {
                candidate: LearningCandidate::Precedent(PrecedentLearningCandidate {
                    summary: "tools=memory_recall".into(),
                    tool_pattern: vec!["memory_recall".into()],
                    subjects: vec!["reflective_memory_topic".into()],
                }),
                confidence: 0.74,
                accepted: true,
                merge_with_existing: false,
                reason: "procedural_precedent",
            }],
            &LearningEvidenceEnvelope {
                typed_fact_count: 1,
                internal_procedural_fact_count: 1,
                external_procedural_fact_count: 0,
                facets: vec![LearningEvidenceFacet::Search],
                ..Default::default()
            },
        );

        assert!(!assessments[0].accepted);
        assert_eq!(assessments[0].reason, "internal_only_procedural_turn");
    }

    #[test]
    fn mutation_governor_rejects_ephemeral_repair_traces_as_durable_memory() {
        let verdict = assess_memory_mutation_candidate(&MutationCandidate {
            category: MemoryCategory::Core,
            text: "tool failure: missing delivery target".into(),
            confidence: 0.8,
            source: MutationSource::Reflection,
            write_class: Some(MutationWriteClass::EphemeralRepairTrace),
        });

        assert_eq!(
            verdict,
            MemoryMutationVerdict::Reject(MemoryMutationRejectReason::EphemeralRepairTrace)
        );
    }

    #[test]
    fn mutation_governor_rejects_generic_dialogue_consolidation_updates() {
        let verdict = assess_memory_mutation_candidate(&MutationCandidate {
            category: MemoryCategory::Core,
            text: "Meaning is shaped by responsibility, relationships, and attention to time."
                .into(),
            confidence: 0.74,
            source: MutationSource::Consolidation,
            write_class: Some(MutationWriteClass::GenericDialogue),
        });

        assert_eq!(
            verdict,
            MemoryMutationVerdict::Reject(MemoryMutationRejectReason::GenericDialogueNotDurable)
        );
    }

    #[test]
    fn mutation_governor_rejects_unanchored_legacy_abstract_updates() {
        let verdict = assess_memory_mutation_candidate(&MutationCandidate {
            category: MemoryCategory::Core,
            text: "Meaning is shaped by responsibility, relationships, and attention to time."
                .into(),
            confidence: 0.74,
            source: MutationSource::Consolidation,
            write_class: None,
        });

        assert_eq!(
            verdict,
            MemoryMutationVerdict::Reject(MemoryMutationRejectReason::GenericDialogueNotDurable)
        );
    }

    #[test]
    fn mutation_governor_accepts_specific_task_state_updates() {
        let verdict = assess_memory_mutation_candidate(&MutationCandidate {
            category: MemoryCategory::Core,
            text: "Atlas deployment is blocked on SSO login verification.".into(),
            confidence: 0.82,
            source: MutationSource::Consolidation,
            write_class: Some(MutationWriteClass::TaskState),
        });

        assert_eq!(
            verdict,
            MemoryMutationVerdict::Accept(MutationWriteClass::TaskState)
        );
    }

    #[test]
    fn reflection_skips_internal_only_procedural_turns() {
        assert_eq!(
            assess_reflection_start(
                &LearningEvidenceEnvelope {
                    typed_fact_count: 1,
                    internal_procedural_fact_count: 1,
                    external_procedural_fact_count: 0,
                    facets: vec![LearningEvidenceFacet::Search],
                    ..Default::default()
                },
                64,
                300,
                30,
                200,
            ),
            ReflectionStartVerdict::Skip(ReflectionSkipReason::InternalOnlyProceduralEvidence)
        );
    }

    #[test]
    fn reflection_starts_for_external_procedural_turns() {
        assert_eq!(
            assess_reflection_start(
                &LearningEvidenceEnvelope {
                    typed_fact_count: 1,
                    internal_procedural_fact_count: 0,
                    external_procedural_fact_count: 1,
                    facets: vec![LearningEvidenceFacet::Delivery],
                    ..Default::default()
                },
                64,
                300,
                30,
                200,
            ),
            ReflectionStartVerdict::Start
        );
    }

    #[test]
    fn reflection_skips_short_user_turns_before_other_logic() {
        assert_eq!(
            assess_reflection_start(&LearningEvidenceEnvelope::default(), 10, 500, 30, 200),
            ReflectionStartVerdict::Skip(ReflectionSkipReason::UserMessageTooShort)
        );
    }

    #[test]
    fn reflection_skips_short_response_without_failure() {
        assert_eq!(
            assess_reflection_start(&LearningEvidenceEnvelope::default(), 64, 20, 30, 200),
            ReflectionStartVerdict::Skip(ReflectionSkipReason::AssistantResponseTooShort)
        );
    }

    #[test]
    fn consolidation_skips_internal_only_procedural_turns() {
        assert_eq!(
            assess_consolidation_start(
                &LearningEvidenceEnvelope {
                    typed_fact_count: 1,
                    internal_procedural_fact_count: 1,
                    external_procedural_fact_count: 0,
                    facets: vec![LearningEvidenceFacet::Search],
                    ..Default::default()
                },
                "Tell me about the reflective memory topic after we queried memory.",
                20,
            ),
            ConsolidationStartVerdict::Skip(
                ConsolidationSkipReason::InternalOnlyProceduralEvidence
            )
        );
    }

    #[test]
    fn consolidation_starts_for_long_semantic_turn_without_tool_facts() {
        assert_eq!(
            assess_consolidation_start(
                &LearningEvidenceEnvelope::default(),
                "Мне кажется, смысл жизни связан с отношениями, трудом и тем, как мы держим слово.",
                20,
            ),
            ConsolidationStartVerdict::Start
        );
    }

    #[test]
    fn consolidation_skips_short_turns() {
        assert_eq!(
            assess_consolidation_start(&LearningEvidenceEnvelope::default(), "short", 20),
            ConsolidationStartVerdict::Skip(ConsolidationSkipReason::UserMessageTooShort)
        );
    }

    #[test]
    fn consolidation_skips_low_information_repetition_without_other_evidence() {
        assert_eq!(
            assess_consolidation_start(
                &LearningEvidenceEnvelope::default(),
                "echo echo echo echo echo echo echo echo echo echo echo echo echo",
                20,
            ),
            ConsolidationStartVerdict::Skip(ConsolidationSkipReason::LowInformationRepetition)
        );
    }

    #[test]
    fn derives_failure_outcome_from_typed_failure_evidence() {
        assert_eq!(
            derive_reflection_outcome(
                &LearningEvidenceEnvelope {
                    failure_outcome_count: 1,
                    ..Default::default()
                },
                &["web_fetch".into()]
            ),
            ReflectionOutcome::Failure
        );
    }

    #[test]
    fn autosave_rejects_structured_control_turns() {
        assert_eq!(
            assess_autosave_write("/model cheap", 20),
            AutosaveWriteVerdict::Skip(AutosaveSkipReason::StructuredControlTurn)
        );
        assert_eq!(
            assess_autosave_write("[GENERATE:IMAGE] album cover", 20),
            AutosaveWriteVerdict::Skip(AutosaveSkipReason::StructuredControlTurn)
        );
    }

    #[test]
    fn autosave_accepts_long_semantic_turns() {
        assert_eq!(
            assess_autosave_write(
                "Мне кажется, смысл жизни связан не с целью, а с тем, как мы проживаем время.",
                20
            ),
            AutosaveWriteVerdict::Write
        );
    }

    #[test]
    fn autosave_rejects_low_information_repetition() {
        assert_eq!(
            assess_autosave_write(
                "echo echo echo echo echo echo echo echo echo echo echo echo echo",
                20,
            ),
            AutosaveWriteVerdict::Skip(AutosaveSkipReason::LowInformationRepetition)
        );
    }

    #[test]
    fn background_learning_rejects_low_information_repetition_without_length_gate() {
        assert_eq!(
            assess_background_learning_input(
                "echo echo echo echo echo echo echo echo echo echo echo echo echo"
            ),
            BackgroundLearningInputVerdict::Skip(AutosaveSkipReason::LowInformationRepetition)
        );
    }

    #[test]
    fn retrieval_noise_penalty_applies_only_without_lexical_anchor() {
        assert_eq!(
            retrieval_noise_score_delta(&MemoryCategory::Custom("precedent".into()), 0.0),
            -0.16
        );
        assert_eq!(
            retrieval_noise_score_delta(&MemoryCategory::Daily, 0.0),
            -0.10
        );
        assert_eq!(
            retrieval_noise_score_delta(&MemoryCategory::Daily, 0.01),
            0.0
        );
        assert_eq!(retrieval_noise_score_delta(&MemoryCategory::Core, 0.0), 0.0);
    }

    #[test]
    fn retrieval_content_noise_penalizes_low_information_loops() {
        assert_eq!(
            retrieval_content_noise_score_delta(
                "meaning comes from choice and purpose comes from choice because meaning grows from choice and purpose grows from choice because meaning comes from choice",
            ),
            -0.24
        );
        assert_eq!(
            retrieval_content_noise_score_delta(
                "The early anchor was about responsibility, while the later point shifted toward relationships and work."
            ),
            0.0
        );
    }
}
