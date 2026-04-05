//! Turn interpretation — bounded typed interpretation for a single turn.
//!
//! This layer is intentionally narrow. It combines structured runtime facts
//! with embedding-backed semantic hints, without turning into a phrase-engine.

use crate::domain::conversation_target::{ConversationDeliveryTarget, CurrentConversationContext};
use crate::domain::dialogue_state::DialogueState;
use crate::domain::memory::EmbeddingProfile;
use crate::domain::user_profile::UserProfile;
use crate::ports::memory::UnifiedMemoryPort;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
    pub semantic_hints: Vec<InterpretationHint>,
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
    pub last_tool_subjects: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceSource {
    DialogueState,
    UserProfile,
    CurrentConversation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceCandidate {
    pub kind: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterpretationHintKind {
    HistoryLookup,
    RepeatWork,
    DeliverHere,
    FollowupReference,
    UseDefaultLanguage,
    UseDefaultTimezone,
    UseDefaultCity,
    UseDefaultDeliveryTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpretationHint {
    pub kind: InterpretationHintKind,
    pub score_bps: u16,
}

#[derive(Clone, Copy)]
struct SemanticPrototype {
    kind: InterpretationHintKind,
    text: &'static str,
}

#[derive(Debug, Clone)]
struct PrototypeEmbedding {
    kind: InterpretationHintKind,
    embedding: Vec<f32>,
}

const HINT_THRESHOLD_BPS: u16 = 700;
const PROTOTYPES: &[SemanticPrototype] = &[
    SemanticPrototype {
        kind: InterpretationHintKind::HistoryLookup,
        text: "what did we discuss previously",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::HistoryLookup,
        text: "summarize our past conversation",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::RepeatWork,
        text: "do it like last time",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::RepeatWork,
        text: "repeat the previous successful way",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::DeliverHere,
        text: "send it to this chat",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::DeliverHere,
        text: "reply here in the current conversation",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::FollowupReference,
        text: "what about the second one",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::FollowupReference,
        text: "do the same for that one",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultLanguage,
        text: "translate it into my language",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultLanguage,
        text: "answer in my preferred language",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultTimezone,
        text: "schedule it in my timezone",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultTimezone,
        text: "remind me tomorrow using my timezone",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultCity,
        text: "what is the weather in my city",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultCity,
        text: "use my default city",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultDeliveryTarget,
        text: "send it to my default destination",
    },
    SemanticPrototype {
        kind: InterpretationHintKind::UseDefaultDeliveryTarget,
        text: "notify me in my usual chat",
    },
];

static PROTOTYPE_CACHE: OnceLock<Mutex<HashMap<String, Vec<PrototypeEmbedding>>>> = OnceLock::new();

pub async fn build_turn_interpretation(
    memory: Option<&dyn UnifiedMemoryPort>,
    user_message: &str,
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

    let semantic_hints = build_semantic_hints(
        memory,
        user_message,
        user_profile.as_ref(),
        current_conversation.as_ref(),
        dialogue_state.as_ref(),
    )
    .await;
    let defaults_requested = derive_defaults_requested(&semantic_hints, user_profile.as_ref());
    let temporal_scope = derive_temporal_scope(&semantic_hints);
    let delivery_scope = derive_delivery_scope(
        &semantic_hints,
        user_profile.as_ref(),
        current_conversation.as_ref(),
    );
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
        defaults_requested,
        temporal_scope,
        delivery_scope,
        clarification_candidates,
        semantic_hints,
    };

    if interpretation.user_profile.is_none()
        && interpretation.current_conversation.is_none()
        && interpretation.dialogue_state.is_none()
        && interpretation.reference_candidates.is_empty()
        && interpretation.defaults_requested.is_empty()
        && interpretation.temporal_scope.is_none()
        && interpretation.delivery_scope.is_none()
        && interpretation.clarification_candidates.is_empty()
        && interpretation.semantic_hints.is_empty()
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
        if !state.last_tool_subjects.is_empty() {
            lines.push(format!(
                "- last_tool_subjects: {}",
                state.last_tool_subjects.join(", ")
            ));
        }
    }

    if !interpretation.semantic_hints.is_empty()
        || interpretation.temporal_scope.is_some()
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
        if !interpretation.semantic_hints.is_empty() {
            lines.push(format!(
                "- semantic_hints: {}",
                interpretation
                    .semantic_hints
                    .iter()
                    .map(|hint| format!(
                        "{}({})",
                        interpretation_hint_name(hint.kind),
                        hint.score_bps
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("[runtime-interpretation]\n{}\n", lines.join("\n")))
    }
}

impl TurnInterpretation {
    pub fn has_hint(&self, kind: InterpretationHintKind) -> bool {
        self.semantic_hints.iter().any(|hint| hint.kind == kind)
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
    let last_tool_subjects = state.last_tool_subjects.clone();

    if focus_entities.is_empty()
        && comparison_set.is_empty()
        && slots.is_empty()
        && last_tool_subjects.is_empty()
    {
        None
    } else {
        Some(DialogueStateSnapshot {
            focus_entities,
            comparison_set,
            slots,
            last_tool_subjects,
        })
    }
}

async fn build_semantic_hints(
    memory: Option<&dyn UnifiedMemoryPort>,
    user_message: &str,
    profile: Option<&UserProfile>,
    current_conversation: Option<&CurrentConversationSnapshot>,
    dialogue_state: Option<&DialogueStateSnapshot>,
) -> Vec<InterpretationHint> {
    let Some(memory) = memory else {
        return Vec::new();
    };
    let trimmed = user_message.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let Some(query_embedding) = embed_query_or_none(memory, trimmed).await else {
        return Vec::new();
    };
    let prototypes = cached_prototype_embeddings(memory).await;
    if prototypes.is_empty() {
        return Vec::new();
    }

    let relevant_kinds =
        relevant_hint_kinds(profile, current_conversation.is_some(), dialogue_state.is_some());
    let mut best_scores = HashMap::<InterpretationHintKind, u16>::new();

    for kind in relevant_kinds {
        let mut best_bps = 0u16;
        for prototype in prototypes.iter().filter(|prototype| prototype.kind == kind) {
            let Some(score) = cosine_similarity(&query_embedding, &prototype.embedding) else {
                continue;
            };
            let score_bps = (score.clamp(0.0, 1.0) * 1000.0).round() as u16;
            best_bps = best_bps.max(score_bps);
        }
        if best_bps >= HINT_THRESHOLD_BPS {
            best_scores.insert(kind, best_bps);
        }
    }

    let mut hints = best_scores
        .into_iter()
        .map(|(kind, score_bps)| InterpretationHint { kind, score_bps })
        .collect::<Vec<_>>();
    hints.sort_by(|left, right| right.score_bps.cmp(&left.score_bps));
    hints
}

fn relevant_hint_kinds(
    profile: Option<&UserProfile>,
    has_current_conversation: bool,
    has_dialogue_state: bool,
) -> Vec<InterpretationHintKind> {
    let mut kinds = vec![
        InterpretationHintKind::HistoryLookup,
        InterpretationHintKind::RepeatWork,
    ];

    if has_current_conversation {
        kinds.push(InterpretationHintKind::DeliverHere);
    }
    if has_dialogue_state {
        kinds.push(InterpretationHintKind::FollowupReference);
    }
    if let Some(profile) = profile {
        if profile.preferred_language.is_some() {
            kinds.push(InterpretationHintKind::UseDefaultLanguage);
        }
        if profile.timezone.is_some() {
            kinds.push(InterpretationHintKind::UseDefaultTimezone);
        }
        if profile.default_city.is_some() {
            kinds.push(InterpretationHintKind::UseDefaultCity);
        }
        if profile.default_delivery_target.is_some() {
            kinds.push(InterpretationHintKind::UseDefaultDeliveryTarget);
        }
    }

    kinds
}

fn derive_defaults_requested(
    hints: &[InterpretationHint],
    profile: Option<&UserProfile>,
) -> Vec<DefaultKind> {
    let Some(profile) = profile else {
        return Vec::new();
    };

    let mut defaults = Vec::new();
    if profile.preferred_language.is_some()
        && hints
            .iter()
            .any(|hint| hint.kind == InterpretationHintKind::UseDefaultLanguage)
    {
        defaults.push(DefaultKind::Language);
    }
    if profile.timezone.is_some()
        && hints
            .iter()
            .any(|hint| hint.kind == InterpretationHintKind::UseDefaultTimezone)
    {
        defaults.push(DefaultKind::Timezone);
    }
    if profile.default_city.is_some()
        && hints
            .iter()
            .any(|hint| hint.kind == InterpretationHintKind::UseDefaultCity)
    {
        defaults.push(DefaultKind::City);
    }
    if profile.default_delivery_target.is_some()
        && hints
            .iter()
            .any(|hint| hint.kind == InterpretationHintKind::UseDefaultDeliveryTarget)
    {
        defaults.push(DefaultKind::DeliveryTarget);
    }
    defaults
}

fn derive_temporal_scope(hints: &[InterpretationHint]) -> Option<TemporalScope> {
    if hints.iter().any(|hint| {
        matches!(
            hint.kind,
            InterpretationHintKind::HistoryLookup | InterpretationHintKind::RepeatWork
        )
    }) {
        Some(TemporalScope::Historical)
    } else {
        None
    }
}

fn derive_delivery_scope(
    hints: &[InterpretationHint],
    profile: Option<&UserProfile>,
    current_conversation: Option<&CurrentConversationSnapshot>,
) -> Option<DeliveryScope> {
    if current_conversation.is_some()
        && hints
            .iter()
            .any(|hint| hint.kind == InterpretationHintKind::DeliverHere)
    {
        return Some(DeliveryScope::CurrentConversation);
    }
    if profile
        .and_then(|profile| profile.default_delivery_target.as_ref())
        .is_some()
        && hints
            .iter()
            .any(|hint| hint.kind == InterpretationHintKind::UseDefaultDeliveryTarget)
    {
        return Some(DeliveryScope::DefaultTarget);
    }
    None
}

fn collect_reference_candidates(
    profile: Option<&UserProfile>,
    current_conversation: Option<&CurrentConversationSnapshot>,
    dialogue_state: Option<&DialogueStateSnapshot>,
) -> Vec<ReferenceCandidate> {
    let mut candidates = Vec::new();

    if let Some(state) = dialogue_state {
        for (kind, value) in state
            .focus_entities
            .iter()
            .chain(state.comparison_set.iter())
            .chain(state.slots.iter())
        {
            push_reference_candidate(
                &mut candidates,
                kind,
                value,
                ReferenceSource::DialogueState,
            );
        }
        for subject in &state.last_tool_subjects {
            push_reference_candidate(
                &mut candidates,
                "recent_subject",
                subject,
                ReferenceSource::DialogueState,
            );
        }
    }

    if let Some(profile) = profile {
        if let Some(city) = profile.default_city.as_deref() {
            push_reference_candidate(&mut candidates, "city", city, ReferenceSource::UserProfile);
        }
        if let Some(language) = profile.preferred_language.as_deref() {
            push_reference_candidate(
                &mut candidates,
                "language",
                language,
                ReferenceSource::UserProfile,
            );
        }
        if let Some(timezone) = profile.timezone.as_deref() {
            push_reference_candidate(
                &mut candidates,
                "timezone",
                timezone,
                ReferenceSource::UserProfile,
            );
        }
        for environment in &profile.known_environments {
            push_reference_candidate(
                &mut candidates,
                "environment",
                environment,
                ReferenceSource::UserProfile,
            );
        }
        if profile.default_delivery_target.is_some() {
            push_reference_candidate(
                &mut candidates,
                "delivery_target",
                "default_delivery_target",
                ReferenceSource::UserProfile,
            );
        }
    }

    if current_conversation.is_some() {
        push_reference_candidate(
            &mut candidates,
            "delivery_target",
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

async fn cached_prototype_embeddings(memory: &dyn UnifiedMemoryPort) -> Vec<PrototypeEmbedding> {
    let EmbeddingProfile { profile_id, .. } = memory.embedding_profile();
    if profile_id == EmbeddingProfile::default().profile_id {
        return Vec::new();
    }

    let cache = PROTOTYPE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(existing) = cache
        .lock()
        .expect("prototype cache poisoned")
        .get(&profile_id)
        .cloned()
    {
        return existing;
    }

    let mut built = Vec::new();
    for prototype in PROTOTYPES {
        let Some(embedding) = embed_document_or_none(memory, prototype.text).await else {
            continue;
        };
        built.push(PrototypeEmbedding {
            kind: prototype.kind,
            embedding,
        });
    }

    cache.lock()
        .expect("prototype cache poisoned")
        .insert(profile_id, built.clone());
    built
}

async fn embed_query_or_none(memory: &dyn UnifiedMemoryPort, text: &str) -> Option<Vec<f32>> {
    match memory.embed_query(text).await {
        Ok(embedding) if !embedding.is_empty() => Some(embedding),
        _ => None,
    }
}

async fn embed_document_or_none(memory: &dyn UnifiedMemoryPort, text: &str) -> Option<Vec<f32>> {
    match memory.embed_document(text).await {
        Ok(embedding) if !embedding.is_empty() => Some(embedding),
        _ => None,
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return None;
    }

    let mut dot = 0.0f64;
    let mut left_mag = 0.0f64;
    let mut right_mag = 0.0f64;
    for (l, r) in left.iter().zip(right.iter()) {
        let l = *l as f64;
        let r = *r as f64;
        dot += l * r;
        left_mag += l * l;
        right_mag += r * r;
    }
    if left_mag == 0.0 || right_mag == 0.0 {
        return None;
    }
    Some(dot / (left_mag.sqrt() * right_mag.sqrt()))
}

fn push_reference_candidate(
    values: &mut Vec<ReferenceCandidate>,
    kind: &str,
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
            kind: kind.to_string(),
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
        candidate.kind,
        candidate.value
    )
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

fn interpretation_hint_name(kind: InterpretationHintKind) -> &'static str {
    match kind {
        InterpretationHintKind::HistoryLookup => "history_lookup",
        InterpretationHintKind::RepeatWork => "repeat_work",
        InterpretationHintKind::DeliverHere => "deliver_here",
        InterpretationHintKind::FollowupReference => "followup_reference",
        InterpretationHintKind::UseDefaultLanguage => "use_default_language",
        InterpretationHintKind::UseDefaultTimezone => "use_default_timezone",
        InterpretationHintKind::UseDefaultCity => "use_default_city",
        InterpretationHintKind::UseDefaultDeliveryTarget => "use_default_delivery_target",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        async fn get_core_blocks(
            &self,
            _: &AgentId,
        ) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
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
        async fn get_recent(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
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
        async fn get_current_facts(
            &self,
            _: &MemoryId,
        ) -> Result<Vec<TemporalFact>, MemoryError> {
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

    fn test_embedding(text: &str) -> Vec<f32> {
        let normalized = text.to_lowercase();
        let mut vector = vec![0.0f32; 8];
        let features = [
            (0, &["discuss", "conversation", "past", "previous", "last week"][..]),
            (1, &["like last time", "repeat", "same as before", "successful way"][..]),
            (2, &["here", "this chat", "current conversation", "reply"][..]),
            (3, &["second one", "that one", "this one"][..]),
            (4, &["language", "translate", "preferred language"][..]),
            (5, &["timezone", "remind", "tomorrow"][..]),
            (6, &["weather", "city", "temperature"][..]),
            (7, &["default destination", "usual chat", "notify me"][..]),
        ];

        for (idx, tokens) in features {
            if tokens.iter().any(|token| normalized.contains(token)) {
                vector[idx] = 1.0;
            }
        }
        vector
    }

    #[tokio::test]
    async fn returns_none_for_empty_inputs() {
        assert!(
            build_turn_interpretation(None, "", None, None, None)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn builds_semantic_hints_and_defaults() {
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

        assert!(interpretation.has_hint(InterpretationHintKind::UseDefaultLanguage));
        assert!(interpretation.has_hint(InterpretationHintKind::FollowupReference));
        assert_eq!(interpretation.defaults_requested, vec![DefaultKind::Language]);
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
            slots: vec![crate::domain::dialogue_state::DialogueSlot {
                name: "timezone".into(),
                value: "Europe/Berlin".into(),
            }],
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
        assert!(block.contains("delivery_scope: current_conversation"));
        assert!(block.contains("defaults_requested: language"));
        assert!(block.contains("semantic_hints:"));
    }
}
