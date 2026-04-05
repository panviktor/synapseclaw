//! Turn interpretation — bounded typed interpretation for a single turn.
//!
//! This layer is intentionally narrow. It combines structured runtime facts
//! without turning into a phrase-engine.

use crate::domain::conversation_target::{ConversationDeliveryTarget, CurrentConversationContext};
use crate::domain::dialogue_state::{
    DialogueState, ReferenceAnchor, ReferenceAnchorSelector, ReferenceOrdinal,
};
use crate::domain::user_profile::UserProfile;
use crate::ports::memory::UnifiedMemoryPort;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnInterpretation {
    pub user_profile: Option<UserProfile>,
    pub current_conversation: Option<CurrentConversationSnapshot>,
    pub dialogue_state: Option<DialogueStateSnapshot>,
    pub reference_candidates: Vec<ReferenceCandidate>,
    pub defaults_requested: Vec<DefaultKind>,
    pub temporal_scope: Option<TemporalScope>,
    pub delivery_scope: Option<DeliveryScope>,
    pub clarification_candidates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentConversationSnapshot {
    pub adapter: String,
    pub has_thread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueStateSnapshot {
    pub focus_entities: Vec<(String, String)>,
    pub comparison_set: Vec<(String, String)>,
    pub slots: Vec<(String, String)>,
    pub reference_anchors: Vec<ReferenceAnchor>,
    pub last_tool_subjects: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceSource {
    DialogueState,
    UserProfile,
    CurrentConversation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceCandidateKind {
    Entity {
        entity_kind: String,
    },
    Slot {
        slot_name: String,
    },
    Anchor {
        selector: ReferenceAnchorSelector,
        entity_kind: Option<String>,
    },
    Profile(DefaultKind),
    DeliveryTarget,
    RecentSubject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceCandidate {
    pub kind: ReferenceCandidateKind,
    pub value: String,
    pub source: ReferenceSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultKind {
    Language,
    Timezone,
    City,
    DeliveryTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalScope {
    Historical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryScope {
    CurrentConversation,
    DefaultTarget,
}

pub async fn build_turn_interpretation(
    _memory: Option<&dyn UnifiedMemoryPort>,
    _user_message: &str,
    profile: Option<UserProfile>,
    current_conversation: Option<&CurrentConversationContext>,
    dialogue_state: Option<&DialogueState>,
) -> Option<TurnInterpretation> {
    let user_profile = profile.filter(|profile| !profile.is_empty());
    let current_conversation = current_conversation.map(|ctx| CurrentConversationSnapshot {
        adapter: ctx.source_adapter.clone(),
        has_thread: ctx.thread_ref.is_some(),
    });
    let dialogue_state = dialogue_state.and_then(snapshot_dialogue_state);

    let reference_candidates = collect_reference_candidates(
        user_profile.as_ref(),
        current_conversation.as_ref(),
        dialogue_state.as_ref(),
    );
    let clarification_candidates =
        collect_clarification_candidates(dialogue_state.as_ref(), user_profile.as_ref());

    let interpretation = TurnInterpretation {
        user_profile,
        current_conversation,
        dialogue_state,
        reference_candidates,
        defaults_requested: Vec::new(),
        temporal_scope: None,
        delivery_scope: None,
        clarification_candidates,
    };

    if interpretation.user_profile.is_none()
        && interpretation.current_conversation.is_none()
        && interpretation.dialogue_state.is_none()
        && interpretation.reference_candidates.is_empty()
        && interpretation.defaults_requested.is_empty()
        && interpretation.temporal_scope.is_none()
        && interpretation.delivery_scope.is_none()
        && interpretation.clarification_candidates.is_empty()
    {
        None
    } else {
        Some(interpretation)
    }
}

pub fn format_turn_interpretation(interpretation: &TurnInterpretation) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(profile) = &interpretation.user_profile {
        lines.push("[user-profile]".to_string());
        if let Some(language) = &profile.preferred_language {
            lines.push(format!("- preferred_language: {language}"));
        }
        if let Some(timezone) = &profile.timezone {
            lines.push(format!("- timezone: {timezone}"));
        }
        if let Some(city) = &profile.default_city {
            lines.push(format!("- default_city: {city}"));
        }
        if let Some(style) = &profile.communication_style {
            lines.push(format!("- communication_style: {style}"));
        }
        if !profile.known_environments.is_empty() {
            lines.push(format!(
                "- known_environments: {}",
                profile.known_environments.join(", ")
            ));
        }
        if let Some(target) = &profile.default_delivery_target {
            lines.push(format!(
                "- default_delivery_target: {}",
                format_delivery_target(target)
            ));
        }
    }

    if let Some(conversation) = &interpretation.current_conversation {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[current-conversation]".to_string());
        lines.push(format!("- adapter: {}", conversation.adapter));
        lines.push("- reply_here_available: true".to_string());
        lines.push(format!("- threaded_reply: {}", conversation.has_thread));
    }

    if let Some(state) = &interpretation.dialogue_state {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[working-state]".to_string());
        if !state.focus_entities.is_empty() {
            lines.push(format!(
                "- focus_entities: {}",
                format_pairs(&state.focus_entities)
            ));
        }
        if !state.comparison_set.is_empty() {
            lines.push(format!(
                "- comparison_set: {}",
                format_pairs(&state.comparison_set)
            ));
        }
        if !state.slots.is_empty() {
            lines.push(format!("- slots: {}", format_pairs(&state.slots)));
        }
        if !state.reference_anchors.is_empty() {
            lines.push(format!(
                "- reference_anchors: {}",
                state
                    .reference_anchors
                    .iter()
                    .map(format_reference_anchor)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !state.last_tool_subjects.is_empty() {
            lines.push(format!(
                "- last_tool_subjects: {}",
                state.last_tool_subjects.join(", ")
            ));
        }
    }

    if interpretation.temporal_scope.is_some()
        || interpretation.delivery_scope.is_some()
        || !interpretation.defaults_requested.is_empty()
        || !interpretation.reference_candidates.is_empty()
        || !interpretation.clarification_candidates.is_empty()
    {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[bounded-interpretation]".to_string());
        if let Some(scope) = interpretation.temporal_scope {
            lines.push(format!("- temporal_scope: {}", temporal_scope_name(scope)));
        }
        if let Some(scope) = interpretation.delivery_scope {
            lines.push(format!("- delivery_scope: {}", delivery_scope_name(scope)));
        }
        if !interpretation.defaults_requested.is_empty() {
            lines.push(format!(
                "- defaults_requested: {}",
                interpretation
                    .defaults_requested
                    .iter()
                    .map(|kind| default_kind_name(*kind))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !interpretation.reference_candidates.is_empty() {
            lines.push(format!(
                "- reference_candidates: {}",
                interpretation
                    .reference_candidates
                    .iter()
                    .map(format_reference_candidate)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        if !interpretation.clarification_candidates.is_empty() {
            lines.push(format!(
                "- clarification_candidates: {}",
                interpretation.clarification_candidates.join(" | ")
            ));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("[runtime-interpretation]\n{}\n", lines.join("\n")))
    }
}

fn snapshot_dialogue_state(state: &DialogueState) -> Option<DialogueStateSnapshot> {
    let focus_entities = state
        .focus_entities
        .iter()
        .map(|entity| (entity.kind.clone(), entity.name.clone()))
        .collect::<Vec<_>>();
    let comparison_set = state
        .comparison_set
        .iter()
        .map(|entity| (entity.kind.clone(), entity.name.clone()))
        .collect::<Vec<_>>();
    let slots = state
        .slots
        .iter()
        .map(|slot| (slot.name.clone(), slot.value.clone()))
        .collect::<Vec<_>>();
    let reference_anchors = state.reference_anchors.clone();
    let last_tool_subjects = state.last_tool_subjects.clone();

    if focus_entities.is_empty()
        && comparison_set.is_empty()
        && slots.is_empty()
        && reference_anchors.is_empty()
        && last_tool_subjects.is_empty()
    {
        None
    } else {
        Some(DialogueStateSnapshot {
            focus_entities,
            comparison_set,
            slots,
            reference_anchors,
            last_tool_subjects,
        })
    }
}

fn collect_reference_candidates(
    profile: Option<&UserProfile>,
    current_conversation: Option<&CurrentConversationSnapshot>,
    dialogue_state: Option<&DialogueStateSnapshot>,
) -> Vec<ReferenceCandidate> {
    let mut candidates = Vec::new();

    if let Some(state) = dialogue_state {
        for (entity_kind, value) in state
            .focus_entities
            .iter()
            .chain(state.comparison_set.iter())
        {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Entity {
                    entity_kind: entity_kind.clone(),
                },
                value,
                ReferenceSource::DialogueState,
            );
        }
        for (slot_name, value) in &state.slots {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Slot {
                    slot_name: slot_name.clone(),
                },
                value,
                ReferenceSource::DialogueState,
            );
        }
        for anchor in &state.reference_anchors {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Anchor {
                    selector: anchor.selector.clone(),
                    entity_kind: anchor.entity_kind.clone(),
                },
                &anchor.value,
                ReferenceSource::DialogueState,
            );
        }
        for subject in &state.last_tool_subjects {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::RecentSubject,
                subject,
                ReferenceSource::DialogueState,
            );
        }
    }

    if let Some(profile) = profile {
        if let Some(city) = profile.default_city.as_deref() {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Profile(DefaultKind::City),
                city,
                ReferenceSource::UserProfile,
            );
        }
        if let Some(language) = profile.preferred_language.as_deref() {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Profile(DefaultKind::Language),
                language,
                ReferenceSource::UserProfile,
            );
        }
        if let Some(timezone) = profile.timezone.as_deref() {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Profile(DefaultKind::Timezone),
                timezone,
                ReferenceSource::UserProfile,
            );
        }
        for environment in &profile.known_environments {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Entity {
                    entity_kind: "environment".into(),
                },
                environment,
                ReferenceSource::UserProfile,
            );
        }
        if profile.default_delivery_target.is_some() {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Profile(DefaultKind::DeliveryTarget),
                "default_delivery_target",
                ReferenceSource::UserProfile,
            );
        }
    }

    if current_conversation.is_some() {
        push_reference_candidate(
            &mut candidates,
            ReferenceCandidateKind::DeliveryTarget,
            "current_conversation",
            ReferenceSource::CurrentConversation,
        );
    }

    candidates
}

fn collect_clarification_candidates(
    dialogue_state: Option<&DialogueStateSnapshot>,
    profile: Option<&UserProfile>,
) -> Vec<String> {
    let mut values = Vec::new();

    if let Some(state) = dialogue_state {
        let source = if !state.comparison_set.is_empty() {
            &state.comparison_set
        } else {
            &state.focus_entities
        };
        for (_, value) in source {
            push_unique_string(&mut values, value);
        }
    }

    if values.is_empty() {
        if let Some(profile) = profile {
            if (2..=6).contains(&profile.known_environments.len()) {
                for environment in &profile.known_environments {
                    push_unique_string(&mut values, environment);
                }
            }
        }
    }

    values
}

fn push_reference_candidate(
    values: &mut Vec<ReferenceCandidate>,
    kind: ReferenceCandidateKind,
    value: &str,
    source: ReferenceSource,
) {
    if value.trim().is_empty() {
        return;
    }
    if !values.iter().any(|candidate| {
        candidate.kind == kind && candidate.value == value && candidate.source == source
    }) {
        values.push(ReferenceCandidate {
            kind,
            value: value.to_string(),
            source,
        });
    }
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !value.trim().is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn format_pairs(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(left, right)| format!("{left}={right}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_reference_candidate(candidate: &ReferenceCandidate) -> String {
    format!(
        "{}:{}={}",
        reference_source_name(candidate.source),
        reference_candidate_kind_name(&candidate.kind),
        candidate.value
    )
}

fn format_reference_anchor(anchor: &ReferenceAnchor) -> String {
    let selector = match &anchor.selector {
        ReferenceAnchorSelector::Current => "current".to_string(),
        ReferenceAnchorSelector::Latest => "latest".to_string(),
        ReferenceAnchorSelector::Ordinal(ordinal) => ordinal_name(ordinal).to_string(),
    };
    match anchor.entity_kind.as_deref() {
        Some(entity_kind) => format!("{selector}<{entity_kind}>={}", anchor.value),
        None => format!("{selector}={}", anchor.value),
    }
}

fn format_delivery_target(target: &ConversationDeliveryTarget) -> String {
    match target {
        ConversationDeliveryTarget::CurrentConversation => "current_conversation".into(),
        ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } => {
            if thread_ref.is_some() {
                format!("explicit:{channel}:{recipient}#thread")
            } else {
                format!("explicit:{channel}:{recipient}")
            }
        }
    }
}

fn reference_source_name(source: ReferenceSource) -> &'static str {
    match source {
        ReferenceSource::DialogueState => "dialogue_state",
        ReferenceSource::UserProfile => "user_profile",
        ReferenceSource::CurrentConversation => "current_conversation",
    }
}

fn default_kind_name(kind: DefaultKind) -> &'static str {
    match kind {
        DefaultKind::Language => "language",
        DefaultKind::Timezone => "timezone",
        DefaultKind::City => "city",
        DefaultKind::DeliveryTarget => "delivery_target",
    }
}

fn reference_candidate_kind_name(kind: &ReferenceCandidateKind) -> String {
    match kind {
        ReferenceCandidateKind::Entity { entity_kind } => format!("entity<{entity_kind}>"),
        ReferenceCandidateKind::Slot { slot_name } => format!("slot<{slot_name}>"),
        ReferenceCandidateKind::Anchor {
            selector,
            entity_kind,
        } => match entity_kind.as_deref() {
            Some(entity_kind) => format!("anchor<{}:{}>", selector_name(selector), entity_kind),
            None => format!("anchor<{}>", selector_name(selector)),
        },
        ReferenceCandidateKind::Profile(default_kind) => {
            format!("profile<{}>", default_kind_name(*default_kind))
        }
        ReferenceCandidateKind::DeliveryTarget => "delivery_target".into(),
        ReferenceCandidateKind::RecentSubject => "recent_subject".into(),
    }
}

fn selector_name(selector: &ReferenceAnchorSelector) -> &'static str {
    match selector {
        ReferenceAnchorSelector::Current => "current",
        ReferenceAnchorSelector::Latest => "latest",
        ReferenceAnchorSelector::Ordinal(ordinal) => ordinal_name(ordinal),
    }
}

fn ordinal_name(ordinal: &ReferenceOrdinal) -> &'static str {
    match ordinal {
        ReferenceOrdinal::First => "first",
        ReferenceOrdinal::Second => "second",
        ReferenceOrdinal::Third => "third",
        ReferenceOrdinal::Fourth => "fourth",
    }
}

fn temporal_scope_name(scope: TemporalScope) -> &'static str {
    match scope {
        TemporalScope::Historical => "historical",
    }
}

fn delivery_scope_name(scope: DeliveryScope) -> &'static str {
    match scope {
        DeliveryScope::CurrentConversation => "current_conversation",
        DeliveryScope::DefaultTarget => "default_target",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dialogue_state::FocusEntity;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingProfile, Entity,
        HybridSearchResult, MemoryCategory, MemoryEntry, MemoryError, MemoryId, MemoryQuery,
        Reflection, SearchResult, SessionId, Skill, SkillUpdate, TemporalFact, Visibility,
    };
    use crate::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        WorkingMemoryPort,
    };
    use async_trait::async_trait;

    #[derive(Default)]
    struct StubMemory;

    #[async_trait]
    impl WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
            Ok(vec![])
        }
        async fn update_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: String,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn append_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SemanticMemoryPort for StubMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
            Ok(None)
        }
        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
            Ok(vec![])
        }
        async fn traverse(
            &self,
            _: &MemoryId,
            _: usize,
        ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
            Ok(vec![])
        }
        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SkillMemoryPort for StubMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(vec![])
        }
        async fn update_skill(
            &self,
            _: &MemoryId,
            _: SkillUpdate,
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl ReflectionPort for StubMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_relevant_reflections(
            &self,
            _: &MemoryQuery,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
        async fn get_failure_patterns(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
            Ok(ConsolidationReport::default())
        }
        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
            Ok(0)
        }
        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(HybridSearchResult::default())
        }
        async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(test_embedding(text))
        }
        async fn store(
            &self,
            _: &str,
            _: &str,
            _: &MemoryCategory,
            _: Option<&str>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn recall(
            &self,
            _: &str,
            _: usize,
            _: Option<&str>,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> {
            Ok(false)
        }
        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> {
            Ok(None)
        }
        async fn list(
            &self,
            _: Option<&MemoryCategory>,
            _: Option<&str>,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        fn should_skip_autosave(&self, _: &str) -> bool {
            false
        }
        async fn count(&self) -> Result<usize, MemoryError> {
            Ok(0)
        }
        fn name(&self) -> &str {
            "stub"
        }
        async fn health_check(&self) -> bool {
            true
        }
        async fn promote_visibility(
            &self,
            _: &MemoryId,
            _: &Visibility,
            _: &[AgentId],
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        fn embedding_profile(&self) -> EmbeddingProfile {
            EmbeddingProfile {
                profile_id: "test:multilingual:8".into(),
                provider_family: "test".into(),
                model_id: "multilingual".into(),
                dimensions: 8,
                supports_multilingual: true,
                ..EmbeddingProfile::default()
            }
        }
    }

    fn test_embedding(_text: &str) -> Vec<f32> {
        vec![0.0; 8]
    }

    #[tokio::test]
    async fn returns_none_for_empty_inputs() {
        assert!(build_turn_interpretation(None, "", None, None, None)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn builds_profile_and_followup_candidates_without_phrase_router() {
        let memory = StubMemory;
        let profile = UserProfile {
            preferred_language: Some("ru".into()),
            default_city: Some("Berlin".into()),
            ..Default::default()
        };
        let state = DialogueState {
            comparison_set: vec![
                crate::domain::dialogue_state::FocusEntity {
                    kind: "city".into(),
                    name: "Berlin".into(),
                    metadata: None,
                },
                crate::domain::dialogue_state::FocusEntity {
                    kind: "city".into(),
                    name: "Tbilisi".into(),
                    metadata: None,
                },
            ],
            ..Default::default()
        };

        let interpretation = build_turn_interpretation(
            Some(&memory),
            "translate it into my language and what about the second one",
            Some(profile),
            None,
            Some(&state),
        )
        .await
        .unwrap();

        assert!(interpretation.defaults_requested.is_empty());
        assert!(!interpretation.reference_candidates.is_empty());
        assert_eq!(
            interpretation.clarification_candidates,
            vec!["Berlin", "Tbilisi"]
        );
    }

    #[tokio::test]
    async fn formats_profile_and_structured_interpretation() {
        let memory = StubMemory;
        let profile = UserProfile {
            preferred_language: Some("ru".into()),
            timezone: Some("Europe/Berlin".into()),
            ..Default::default()
        };
        let state = DialogueState {
            focus_entities: vec![crate::domain::dialogue_state::FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            slots: vec![crate::domain::dialogue_state::DialogueSlot::observed(
                "timezone",
                "Europe/Berlin",
            )],
            last_tool_subjects: vec!["weather_lookup".into()],
            ..Default::default()
        };
        let current = CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_ref: "matrix_room".into(),
            reply_ref: "!room:example.com".into(),
            thread_ref: Some("$thread".into()),
            actor_id: "alice".into(),
        };

        let interpretation = build_turn_interpretation(
            Some(&memory),
            "send it here and translate it into my language",
            Some(profile),
            Some(&current),
            Some(&state),
        )
        .await
        .unwrap();
        let block = format_turn_interpretation(&interpretation).unwrap();

        assert!(block.contains("[runtime-interpretation]"));
        assert!(block.contains("preferred_language: ru"));
        assert!(block.contains("adapter: matrix"));
        assert!(block.contains("focus_entities: city=Berlin"));
        assert!(block.contains("slots: timezone=Europe/Berlin"));
        assert!(block.contains("last_tool_subjects: weather_lookup"));
    }

    #[tokio::test]
    async fn dialogue_state_slots_surface_reference_candidates() {
        let interpretation = build_turn_interpretation(
            None,
            "the second one",
            None,
            None,
            Some(&DialogueState {
                comparison_set: vec![
                    FocusEntity {
                        kind: "city".into(),
                        name: "Berlin".into(),
                        metadata: None,
                    },
                    FocusEntity {
                        kind: "city".into(),
                        name: "Tbilisi".into(),
                        metadata: None,
                    },
                ],
                reference_anchors: vec![
                    crate::domain::dialogue_state::ReferenceAnchor {
                        selector: crate::domain::dialogue_state::ReferenceAnchorSelector::Ordinal(
                            crate::domain::dialogue_state::ReferenceOrdinal::First,
                        ),
                        entity_kind: Some("city".into()),
                        value: "Berlin".into(),
                    },
                    crate::domain::dialogue_state::ReferenceAnchor {
                        selector: crate::domain::dialogue_state::ReferenceAnchorSelector::Ordinal(
                            crate::domain::dialogue_state::ReferenceOrdinal::Second,
                        ),
                        entity_kind: Some("city".into()),
                        value: "Tbilisi".into(),
                    },
                ],
                last_tool_subjects: vec!["Berlin".into(), "Tbilisi".into()],
                ..Default::default()
            }),
        )
        .await
        .unwrap();

        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::Anchor {
                    selector: ReferenceAnchorSelector::Ordinal(ReferenceOrdinal::Second),
                    entity_kind,
                } if entity_kind.as_deref() == Some("city")
            ) && candidate.value == "Tbilisi"
        }));
    }
}
