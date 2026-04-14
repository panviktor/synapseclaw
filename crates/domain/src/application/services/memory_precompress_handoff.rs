//! Pre-compress memory handoff.
//!
//! This service inspects the exact provider-history region that is about to be
//! compacted or dropped. It forms bounded durable-memory candidates and sends
//! them through the existing memory mutation pipeline. It does not add memory
//! blocks to the normal provider prompt.

use crate::application::services::failure_similarity_service::{
    evaluate_failure_candidate, FailureSimilarityThresholds,
};
use crate::application::services::learning_candidate_service::RunRecipeLearningCandidate;
use crate::application::services::memory_mutation;
use crate::application::services::memory_quality_governor::{
    assess_memory_mutation_candidate, MemoryMutationVerdict,
};
use crate::application::services::precedent_similarity_service::{
    evaluate_precedent_candidate, PrecedentSimilarityThresholds,
};
use crate::application::services::recipe_evolution_service::{
    build_new_recipe, merge_existing_recipe,
};
use crate::application::services::runtime_decision_trace::{
    runtime_memory_decision_from_mutation, RuntimeTraceMemoryDecision,
};
use crate::domain::history_projection::{
    parse_projected_fact_anchor, parse_projected_tool_call, ProjectedToolCall,
};
use crate::domain::memory::MemoryCategory;
use crate::domain::memory_mutation::{
    MutationAction, MutationCandidate, MutationDecision, MutationSource, MutationThresholds,
    MutationWriteClass,
};
use crate::domain::message::ChatMessage;
use crate::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, tool_repair_attempt_reason_name,
    tool_repair_outcome_name, ToolRepairTrace,
};
use crate::domain::util::truncate_with_ellipsis;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::run_recipe_store::RunRecipeStorePort;

const MAX_CANDIDATES: usize = 8;
const MAX_HINTS: usize = 5;
const MAX_HINT_CHARS: usize = 180;
const MAX_CANDIDATE_CHARS: usize = 360;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPreCompressHandoffReason {
    LiveAgentCompaction,
    ChannelSessionHygiene,
    ManualSessionCompaction,
}

impl MemoryPreCompressHandoffReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LiveAgentCompaction => "live_agent_compaction",
            Self::ChannelSessionHygiene => "channel_session_hygiene",
            Self::ManualSessionCompaction => "manual_session_compaction",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPreCompressCandidateKind {
    FactAnchor,
    TaskState,
    Recipe,
    FailurePattern,
    EphemeralRepairTrace,
    GenericDialogue,
}

impl MemoryPreCompressCandidateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FactAnchor => "fact_anchor",
            Self::TaskState => "task_state",
            Self::Recipe => "recipe",
            Self::FailurePattern => "failure_pattern",
            Self::EphemeralRepairTrace => "ephemeral_repair_trace",
            Self::GenericDialogue => "generic_dialogue",
        }
    }
}

#[derive(Clone)]
pub struct MemoryPreCompressHandoffInput<'a> {
    pub agent_id: &'a str,
    pub reason: MemoryPreCompressHandoffReason,
    pub start_index: usize,
    pub end_index: usize,
    pub transcript: &'a str,
    pub messages: &'a [ChatMessage],
    pub message_indices: &'a [usize],
    pub recent_tool_repairs: &'a [ToolRepairTrace],
    pub run_recipe_store: Option<&'a dyn RunRecipeStorePort>,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone)]
pub struct MemoryPreCompressCandidate {
    pub kind: MemoryPreCompressCandidateKind,
    pub mutation: MutationCandidate,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryPreCompressHandoffReport {
    pub candidates: Vec<MemoryPreCompressCandidate>,
    pub preservation_hints: Vec<String>,
    pub runtime_memory_decisions: Vec<RuntimeTraceMemoryDecision>,
    pub run_recipes_upserted: usize,
}

pub fn build_precompress_handoff_candidates(
    input: &MemoryPreCompressHandoffInput<'_>,
) -> Vec<MemoryPreCompressCandidate> {
    let mut candidates = Vec::new();

    if let Some(candidate) = stable_project_fact_candidate(input) {
        push_unique_candidate(&mut candidates, candidate);
    }
    if let Some(candidate) = procedure_candidate(input.messages, input.recent_tool_repairs) {
        push_unique_candidate(&mut candidates, candidate);
    }
    if let Some(candidate) = failure_candidate(input.messages, input.recent_tool_repairs) {
        push_unique_candidate(&mut candidates, candidate);
    }

    for trace in input.recent_tool_repairs.iter().take(2) {
        push_unique_candidate(&mut candidates, repair_trace_candidate(trace));
    }

    if candidates.is_empty() && !input.transcript.trim().is_empty() {
        push_unique_candidate(
            &mut candidates,
            generic_dialogue_candidate(input.transcript),
        );
    }

    candidates.truncate(MAX_CANDIDATES);
    candidates
}

pub async fn execute_memory_precompress_handoff(
    mem: Option<&dyn UnifiedMemoryPort>,
    input: MemoryPreCompressHandoffInput<'_>,
) -> MemoryPreCompressHandoffReport {
    let candidates = build_precompress_handoff_candidates(&input);
    let preservation_hints = build_preservation_hints(&candidates);
    let mut runtime_memory_decisions = Vec::new();
    let mut run_recipes_upserted = 0usize;

    for candidate in &candidates {
        let mutation_decision_for_recipe = if let Some(mem) = mem {
            let decision = match candidate.kind {
                MemoryPreCompressCandidateKind::Recipe => {
                    evaluate_precedent_candidate(
                        mem,
                        candidate.mutation.clone(),
                        input.agent_id,
                        &PrecedentSimilarityThresholds::default(),
                    )
                    .await
                }
                MemoryPreCompressCandidateKind::FailurePattern => {
                    evaluate_failure_candidate(
                        mem,
                        candidate.mutation.clone(),
                        input.agent_id,
                        &FailureSimilarityThresholds::default(),
                    )
                    .await
                }
                _ => {
                    memory_mutation::evaluate_candidate(
                        mem,
                        candidate.mutation.clone(),
                        input.agent_id,
                        &MutationThresholds::default(),
                    )
                    .await
                }
            };
            let (applied, entry_id, failure) =
                match memory_mutation::apply_decision(mem, &decision, input.agent_id).await {
                    Ok(entry_id) => (!decision.action.is_noop(), entry_id, None),
                    Err(error) => (false, None, Some(error.to_string())),
                };
            runtime_memory_decisions.push(runtime_memory_decision_from_mutation(
                &decision,
                input.observed_at_unix,
                Some("pre_compress_handoff"),
                applied,
                entry_id.as_deref(),
                failure.as_deref(),
            ));
            Some(decision)
        } else {
            let decision = MutationDecision {
                action: MutationAction::Noop,
                candidate: candidate.mutation.clone(),
                reason: "memory_backend_unavailable".into(),
                similarity: None,
            };
            runtime_memory_decisions.push(runtime_memory_decision_from_mutation(
                &decision,
                input.observed_at_unix,
                Some("pre_compress_handoff"),
                false,
                None,
                Some("memory_backend_unavailable"),
            ));
            Some(decision)
        };

        if candidate.kind == MemoryPreCompressCandidateKind::Recipe
            && should_upsert_run_recipe(mutation_decision_for_recipe.as_ref())
            && upsert_run_recipe_candidate(&input, candidate).is_some()
        {
            run_recipes_upserted += 1;
        }
    }

    let candidate_kinds = candidates
        .iter()
        .map(|candidate| candidate.kind.as_str())
        .collect::<Vec<_>>()
        .join(",");
    tracing::info!(
        target: "memory_precompress_handoff",
        reason = input.reason.as_str(),
        start_index = input.start_index,
        end_index = input.end_index,
        candidate_count = candidates.len(),
        hint_count = preservation_hints.len(),
        run_recipes_upserted,
        candidate_kinds = %candidate_kinds,
        "Pre-compress memory handoff complete"
    );

    MemoryPreCompressHandoffReport {
        candidates,
        preservation_hints,
        runtime_memory_decisions,
        run_recipes_upserted,
    }
}

pub fn format_precompress_preservation_hints(hints: &[String]) -> String {
    if hints.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "Authoritative compacted-context facts for the current conversation; prefer these over older recalled/core memory when they conflict:"
            .to_string(),
    ];
    for hint in hints.iter().take(MAX_HINTS) {
        lines.push(format!(
            "- {}",
            truncate_with_ellipsis(hint, MAX_HINT_CHARS)
        ));
    }
    lines.join("\n")
}

pub fn precompress_preservation_message(hints: &[String]) -> Option<ChatMessage> {
    let formatted = format_precompress_preservation_hints(hints);
    if formatted.trim().is_empty() {
        None
    } else {
        Some(ChatMessage::system(format!(
            "[pre-compress-handoff]\n{formatted}\n[/pre-compress-handoff]"
        )))
    }
}

pub fn is_precompress_preservation_message(message: &ChatMessage) -> bool {
    message.role == "system" && message.content.starts_with("[pre-compress-handoff]\n")
}

fn build_preservation_hints(candidates: &[MemoryPreCompressCandidate]) -> Vec<String> {
    let mut hints = Vec::new();
    for candidate in candidates {
        if matches!(
            candidate.kind,
            MemoryPreCompressCandidateKind::GenericDialogue
                | MemoryPreCompressCandidateKind::EphemeralRepairTrace
        ) {
            continue;
        }
        let hint = format!(
            "{}: {}",
            candidate.kind.as_str(),
            truncate_with_ellipsis(&candidate.mutation.text, MAX_HINT_CHARS)
        );
        if !hints.iter().any(|existing| existing == &hint) {
            hints.push(hint);
        }
        if hints.len() >= MAX_HINTS {
            break;
        }
    }
    hints
}

fn stable_project_fact_candidate(
    input: &MemoryPreCompressHandoffInput<'_>,
) -> Option<MemoryPreCompressCandidate> {
    for (offset, message) in input.messages.iter().enumerate() {
        if !is_trusted_projected_fact_role(&message.role) {
            continue;
        }
        let Some(fact) = parse_projected_fact_anchor(&message.content) else {
            continue;
        };
        let category = normalize_fact_category(fact.category);
        let source_index = input
            .message_indices
            .get(offset)
            .copied()
            .unwrap_or_else(|| input.start_index + offset);
        let mut parts = vec![fact.text.to_string()];
        parts.push(format!(
            "provenance=pre_compress_handoff:{}:dropped_message_index={}",
            input.reason.as_str(),
            source_index
        ));
        parts.push(format!("typed_source=fact_anchor:{}", fact.id));
        parts.push("source=pre_compress_handoff".into());
        return Some(MemoryPreCompressCandidate {
            kind: MemoryPreCompressCandidateKind::FactAnchor,
            mutation: MutationCandidate {
                category: MemoryCategory::Custom(category),
                text: truncate_with_ellipsis(&parts.join(" | "), MAX_CANDIDATE_CHARS),
                confidence: 0.82,
                source: MutationSource::PreCompressHandoff,
                write_class: Some(MutationWriteClass::FactAnchor),
            },
        });
    }
    None
}

fn normalize_fact_category(raw: &str) -> String {
    let normalized = raw
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let normalized = normalized.trim_matches('_').to_string();
    if normalized.is_empty() {
        "fact".into()
    } else {
        normalized
    }
}

fn procedure_candidate(
    messages: &[ChatMessage],
    recent_tool_repairs: &[ToolRepairTrace],
) -> Option<MemoryPreCompressCandidate> {
    let steps = tool_steps(messages);
    if steps.len() < 2 || sequence_overlaps_repair(&steps, recent_tool_repairs) {
        return None;
    }
    let mut parts = vec![format!(
        "tool_sequence={}",
        steps
            .iter()
            .map(format_tool_step)
            .collect::<Vec<_>>()
            .join(" -> ")
    )];
    parts.push("source=pre_compress_handoff".into());
    Some(MemoryPreCompressCandidate {
        kind: MemoryPreCompressCandidateKind::Recipe,
        mutation: MutationCandidate {
            category: MemoryCategory::Custom("precedent".into()),
            text: truncate_with_ellipsis(&parts.join(" | "), MAX_CANDIDATE_CHARS),
            confidence: 0.74,
            source: MutationSource::PreCompressHandoff,
            write_class: Some(MutationWriteClass::Recipe),
        },
    })
}

fn failure_candidate(
    messages: &[ChatMessage],
    recent_tool_repairs: &[ToolRepairTrace],
) -> Option<MemoryPreCompressCandidate> {
    if recent_tool_repairs.is_empty() {
        return None;
    }
    let mut parts = vec![format!(
        "tool_repairs={}",
        recent_tool_repairs
            .iter()
            .take(3)
            .map(format_tool_repair_step)
            .collect::<Vec<_>>()
            .join(" | ")
    )];
    let steps = tool_steps(messages);
    if !steps.is_empty() {
        parts.push(format!(
            "observed_tool_sequence={}",
            steps
                .iter()
                .map(|step| step.name.to_string())
                .collect::<Vec<_>>()
                .join(" -> ")
        ));
    }
    parts.push("source=pre_compress_handoff".into());
    Some(MemoryPreCompressCandidate {
        kind: MemoryPreCompressCandidateKind::FailurePattern,
        mutation: MutationCandidate {
            category: MemoryCategory::Custom("failure_pattern".into()),
            text: truncate_with_ellipsis(&parts.join(" | "), MAX_CANDIDATE_CHARS),
            confidence: 0.72,
            source: MutationSource::PreCompressHandoff,
            write_class: Some(MutationWriteClass::FailurePattern),
        },
    })
}

fn repair_trace_candidate(trace: &ToolRepairTrace) -> MemoryPreCompressCandidate {
    let detail = trace
        .detail
        .as_deref()
        .map(|value| format!(" detail={}", truncate_with_ellipsis(value, 80)))
        .unwrap_or_default();
    MemoryPreCompressCandidate {
        kind: MemoryPreCompressCandidateKind::EphemeralRepairTrace,
        mutation: MutationCandidate {
            category: MemoryCategory::Custom("repair_trace".into()),
            text: truncate_with_ellipsis(
                &format!(
                    "tool_repair={}:{}->{}{}",
                    trace.tool_name,
                    tool_failure_kind_name(trace.failure_kind),
                    tool_repair_action_name(trace.suggested_action),
                    detail
                ),
                MAX_CANDIDATE_CHARS,
            ),
            confidence: 0.7,
            source: MutationSource::PreCompressHandoff,
            write_class: Some(MutationWriteClass::EphemeralRepairTrace),
        },
    }
}

fn generic_dialogue_candidate(transcript: &str) -> MemoryPreCompressCandidate {
    MemoryPreCompressCandidate {
        kind: MemoryPreCompressCandidateKind::GenericDialogue,
        mutation: MutationCandidate {
            category: MemoryCategory::Core,
            text: truncate_with_ellipsis(transcript, MAX_CANDIDATE_CHARS),
            confidence: 0.2,
            source: MutationSource::PreCompressHandoff,
            write_class: Some(MutationWriteClass::GenericDialogue),
        },
    }
}

fn tool_steps(messages: &[ChatMessage]) -> Vec<ProjectedToolCall<'_>> {
    let mut steps = Vec::new();
    for message in messages {
        if message.role != "assistant" {
            continue;
        }
        if let Some(step) = parse_projected_tool_call(&message.content) {
            steps.push(step);
        }
    }
    steps.truncate(4);
    steps
}

fn is_trusted_projected_fact_role(role: &str) -> bool {
    matches!(role, "assistant" | "system")
}

fn sequence_overlaps_repair(
    steps: &[ProjectedToolCall<'_>],
    recent_tool_repairs: &[ToolRepairTrace],
) -> bool {
    steps.iter().any(|step| {
        recent_tool_repairs
            .iter()
            .any(|repair| repair.tool_name == step.name)
    })
}

fn format_tool_step(step: &ProjectedToolCall<'_>) -> String {
    if step.arguments.is_empty() {
        step.name.to_string()
    } else {
        format!(
            "{}({})",
            step.name,
            truncate_with_ellipsis(step.arguments, 96)
        )
    }
}

fn format_tool_repair_step(trace: &ToolRepairTrace) -> String {
    let mut parts = vec![format!(
        "{}:{}->{}",
        trace.tool_name,
        tool_failure_kind_name(trace.failure_kind),
        tool_repair_action_name(trace.suggested_action)
    )];
    parts.push(format!(
        "outcome={}",
        tool_repair_outcome_name(trace.repair_outcome)
    ));
    parts.push(format!(
        "attempt={}",
        tool_repair_attempt_reason_name(trace.attempt_reason)
    ));
    if trace.repeat_count > 0 {
        parts.push(format!("repeat={}", trace.repeat_count));
    }
    parts.join(",")
}

fn should_upsert_run_recipe(decision: Option<&MutationDecision>) -> bool {
    let Some(decision) = decision else {
        return false;
    };
    if !matches!(
        assess_memory_mutation_candidate(&decision.candidate),
        MemoryMutationVerdict::Accept(MutationWriteClass::Recipe)
    ) {
        return false;
    }
    !decision.action.is_noop()
        || decision.reason == "memory_backend_unavailable"
        || decision.reason.starts_with("recall failed")
}

fn upsert_run_recipe_candidate(
    input: &MemoryPreCompressHandoffInput<'_>,
    candidate: &MemoryPreCompressCandidate,
) -> Option<()> {
    let store = input.run_recipe_store?;
    let steps = tool_steps(input.messages);
    if steps.len() < 2 {
        return None;
    }
    let tool_pattern = unique_tool_pattern(&steps);
    if tool_pattern.len() < 2 {
        return None;
    }
    let task_family_hint = task_family_for_tool_pattern(&tool_pattern);
    let learning_candidate = RunRecipeLearningCandidate {
        task_family_hint: task_family_hint.clone(),
        sample_request: sample_request_from_messages(input.messages),
        summary: candidate.mutation.text.clone(),
        tool_pattern,
    };
    let updated_at = u64::try_from(input.observed_at_unix.max(0)).unwrap_or_default();
    let next = if let Some(existing) = store.get(input.agent_id, &task_family_hint) {
        if existing.sample_request == learning_candidate.sample_request
            && existing.summary == learning_candidate.summary
            && existing.tool_pattern == learning_candidate.tool_pattern
        {
            return None;
        }
        merge_existing_recipe(&existing, &learning_candidate, updated_at)
    } else {
        build_new_recipe(input.agent_id, &learning_candidate, updated_at)
    };
    if let Err(error) = store.upsert(next) {
        tracing::warn!(
            target: "memory_precompress_handoff",
            error = %error,
            task_family = %task_family_hint,
            "Pre-compress run recipe upsert failed"
        );
        return None;
    }
    Some(())
}

fn unique_tool_pattern(steps: &[ProjectedToolCall<'_>]) -> Vec<String> {
    let mut pattern = Vec::new();
    for step in steps {
        if !pattern.iter().any(|existing| existing == step.name) {
            pattern.push(step.name.to_string());
        }
    }
    pattern
}

fn task_family_for_tool_pattern(tool_pattern: &[String]) -> String {
    let mut family = tool_pattern
        .iter()
        .take(4)
        .map(|tool| normalize_task_family_segment(tool))
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if family.is_empty() {
        family = "precompress_recipe".into();
    }
    family
}

fn normalize_task_family_segment(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn sample_request_from_messages(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| truncate_with_ellipsis(message.content.trim(), 160))
        .unwrap_or_else(|| "pre-compress compacted tool sequence".into())
}

fn push_unique_candidate(
    candidates: &mut Vec<MemoryPreCompressCandidate>,
    candidate: MemoryPreCompressCandidate,
) {
    if candidates.iter().any(|existing| {
        existing.kind == candidate.kind && existing.mutation.text == candidate.mutation.text
    }) {
        return;
    }
    candidates.push(candidate);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::history_projection::{
        format_projected_fact_anchor, format_projected_tool_call,
    };
    use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction};
    use crate::ports::run_recipe_store::{InMemoryRunRecipeStore, RunRecipeStorePort};

    fn input<'a>(
        transcript: &'a str,
        messages: &'a [ChatMessage],
    ) -> MemoryPreCompressHandoffInput<'a> {
        MemoryPreCompressHandoffInput {
            agent_id: "agent",
            reason: MemoryPreCompressHandoffReason::LiveAgentCompaction,
            start_index: 1,
            end_index: 3,
            transcript,
            messages,
            message_indices: &[],
            recent_tool_repairs: &[],
            run_recipe_store: None,
            observed_at_unix: 100,
        }
    }

    #[test]
    fn projected_fact_anchor_from_dropped_context_is_promoted_with_provenance() {
        let messages = vec![ChatMessage::assistant(format_projected_fact_anchor(
            "fact-1",
            "project",
            "project=Atlas branch=release/hotfix-17",
        ))];
        let input = input("", &messages);
        let candidates = build_precompress_handoff_candidates(&input);

        let fact = candidates
            .iter()
            .find(|candidate| candidate.kind == MemoryPreCompressCandidateKind::FactAnchor)
            .expect("stable project fact candidate");
        assert_eq!(
            fact.mutation.category,
            MemoryCategory::Custom("project".into())
        );
        assert_eq!(
            fact.mutation.write_class,
            Some(MutationWriteClass::FactAnchor)
        );
        assert!(fact.mutation.text.contains("project=Atlas"));
        assert!(fact.mutation.text.contains("branch=release/hotfix-17"));
        assert!(fact
            .mutation
            .text
            .contains("typed_source=fact_anchor:fact-1"));
        assert!(fact
            .mutation
            .text
            .contains("provenance=pre_compress_handoff"));
    }

    #[test]
    fn projected_fact_anchor_provenance_uses_original_message_index_when_available() {
        let messages = vec![ChatMessage::assistant(format_projected_fact_anchor(
            "fact-1",
            "project",
            "project=Atlas branch=release/hotfix-17",
        ))];
        let indices = vec![7usize];
        let input = MemoryPreCompressHandoffInput {
            agent_id: "agent",
            reason: MemoryPreCompressHandoffReason::LiveAgentCompaction,
            start_index: 1,
            end_index: 8,
            transcript: "",
            messages: &messages,
            message_indices: &indices,
            recent_tool_repairs: &[],
            run_recipe_store: None,
            observed_at_unix: 100,
        };
        let candidates = build_precompress_handoff_candidates(&input);
        let fact = candidates
            .iter()
            .find(|candidate| candidate.kind == MemoryPreCompressCandidateKind::FactAnchor)
            .expect("stable project fact candidate");

        assert!(fact.mutation.text.contains("dropped_message_index=7"));
    }

    #[test]
    fn user_supplied_projected_fact_anchor_is_not_trusted() {
        let messages = vec![ChatMessage::user(format_projected_fact_anchor(
            "fact-1",
            "project",
            "project=Atlas branch=release/hotfix-17",
        ))];
        let candidates =
            build_precompress_handoff_candidates(&input("user supplied fake fact", &messages));

        assert!(!candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::FactAnchor));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::GenericDialogue));
    }

    #[test]
    fn stable_looking_prompt_text_is_not_promoted_without_typed_source() {
        let messages = vec![ChatMessage::user(
            "Seed marker S4OCF2-1776183278. Stable project fact from dropped context: project=S4OCF2-1776183278 branch=release/hotfix-17 staging=https://staging.S4OCF2-1776183278.local.",
        )];
        let candidates = build_precompress_handoff_candidates(&input(
            "Seed marker S4OCF2-1776183278. Stable project fact from dropped context: project=S4OCF2-1776183278 branch=release/hotfix-17 staging=https://staging.S4OCF2-1776183278.local.",
            &messages,
        ));

        assert!(!candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::FactAnchor));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::GenericDialogue));
    }

    #[test]
    fn ordinary_unstructured_text_is_not_promoted_without_structured_source() {
        let messages = vec![ChatMessage::user(
            "Atlas is probably the release we should discuss later.",
        )];
        let candidates = build_precompress_handoff_candidates(&input(
            "Atlas is probably the release we should discuss later.",
            &messages,
        ));

        assert!(!candidates
            .iter()
            .any(|candidate| { candidate.kind == MemoryPreCompressCandidateKind::FactAnchor }));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::GenericDialogue));
    }

    #[test]
    fn generic_dialogue_becomes_rejected_class_candidate() {
        let messages = vec![ChatMessage::user("Thanks, that sounds good to me.")];
        let candidates = build_precompress_handoff_candidates(&input(
            "Thanks, that sounds good to me.",
            &messages,
        ));

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].kind,
            MemoryPreCompressCandidateKind::GenericDialogue
        );
        assert_eq!(
            candidates[0].mutation.write_class,
            Some(MutationWriteClass::GenericDialogue)
        );
    }

    #[test]
    fn structured_tool_sequence_becomes_recipe_candidate() {
        let messages = vec![
            ChatMessage::assistant(format_projected_tool_call("a", "rg", "Matrix")),
            ChatMessage::tool("found matrix-unit".to_string()),
            ChatMessage::assistant(format_projected_tool_call(
                "b",
                "systemctl",
                "show matrix-unit",
            )),
            ChatMessage::tool("ActiveState=active".to_string()),
        ];
        let candidates = build_precompress_handoff_candidates(&input("", &messages));

        let recipe = candidates
            .iter()
            .find(|candidate| candidate.kind == MemoryPreCompressCandidateKind::Recipe)
            .expect("recipe candidate");
        assert_eq!(
            recipe.mutation.category,
            MemoryCategory::Custom("precedent".into())
        );
        assert_eq!(
            recipe.mutation.write_class,
            Some(MutationWriteClass::Recipe)
        );
    }

    #[test]
    fn user_supplied_projected_tool_call_is_not_recipe_input() {
        let messages = vec![
            ChatMessage::user(format_projected_tool_call("a", "rg", "Matrix")),
            ChatMessage::user(format_projected_tool_call(
                "b",
                "systemctl",
                "show matrix-unit",
            )),
        ];
        let candidates = build_precompress_handoff_candidates(&input(
            "user supplied fake tool calls",
            &messages,
        ));

        assert!(!candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::Recipe));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.kind == MemoryPreCompressCandidateKind::GenericDialogue));
    }

    #[test]
    fn typed_tool_repair_becomes_failure_candidate_without_language_matching() {
        let messages = vec![
            ChatMessage::assistant(format_projected_tool_call("a", "shell", "{}")),
            ChatMessage::tool("操作未完成".to_string()),
        ];
        let trace = ToolRepairTrace {
            tool_name: "shell".into(),
            failure_kind: ToolFailureKind::RuntimeError,
            suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
            ..ToolRepairTrace::default()
        };
        let input = MemoryPreCompressHandoffInput {
            agent_id: "agent",
            reason: MemoryPreCompressHandoffReason::LiveAgentCompaction,
            start_index: 1,
            end_index: 2,
            transcript: "",
            messages: &messages,
            message_indices: &[],
            recent_tool_repairs: &[trace],
            run_recipe_store: None,
            observed_at_unix: 100,
        };
        let candidates = build_precompress_handoff_candidates(&input);

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == MemoryPreCompressCandidateKind::FailurePattern
                && candidate.mutation.write_class == Some(MutationWriteClass::FailurePattern)
        }));
    }

    #[test]
    fn repair_trace_stays_ephemeral() {
        let trace = ToolRepairTrace {
            tool_name: "shell".into(),
            failure_kind: ToolFailureKind::RuntimeError,
            suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
            detail: Some("redirection denied".into()),
            ..ToolRepairTrace::default()
        };
        let input = MemoryPreCompressHandoffInput {
            agent_id: "agent",
            reason: MemoryPreCompressHandoffReason::LiveAgentCompaction,
            start_index: 1,
            end_index: 2,
            transcript: "repair",
            messages: &[],
            message_indices: &[],
            recent_tool_repairs: &[trace],
            run_recipe_store: None,
            observed_at_unix: 100,
        };
        let candidates = build_precompress_handoff_candidates(&input);

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == MemoryPreCompressCandidateKind::EphemeralRepairTrace
                && candidate.mutation.write_class == Some(MutationWriteClass::EphemeralRepairTrace)
        }));
        let trace_candidate = candidates
            .iter()
            .find(|candidate| {
                candidate.kind == MemoryPreCompressCandidateKind::EphemeralRepairTrace
            })
            .expect("repair trace candidate");
        assert_eq!(
            crate::application::services::memory_quality_governor::assess_memory_mutation_candidate(
                &trace_candidate.mutation,
            ),
            crate::application::services::memory_quality_governor::MemoryMutationVerdict::Reject(
                crate::application::services::memory_quality_governor::MemoryMutationRejectReason::EphemeralRepairTrace,
            )
        );
    }

    #[test]
    fn hints_exclude_generic_and_repair_candidates() {
        let messages = vec![
            ChatMessage::assistant(format_projected_fact_anchor(
                "fact-1",
                "project",
                "project=Atlas branch=release/hotfix-17",
            )),
            ChatMessage::assistant(format_projected_tool_call("a", "rg", "Matrix")),
            ChatMessage::assistant(format_projected_tool_call(
                "b",
                "systemctl",
                "show matrix-unit",
            )),
        ];
        let mut candidates = build_precompress_handoff_candidates(&input("", &messages));
        candidates.push(generic_dialogue_candidate("hello"));
        let hints = build_preservation_hints(&candidates);

        assert_eq!(hints.len(), 2);
        assert!(hints.iter().any(|hint| hint.contains("fact_anchor")));
        assert!(hints.iter().any(|hint| hint.contains("recipe")));
        assert!(!hints.iter().any(|hint| hint.contains("hello")));
    }

    #[test]
    fn preservation_message_carries_approved_handoff_facts() {
        let messages = vec![ChatMessage::assistant(format_projected_fact_anchor(
            "fact-1",
            "project",
            "project=Atlas branch=release/hotfix-17 staging=https://staging.atlas.local",
        ))];
        let candidates = build_precompress_handoff_candidates(&input("", &messages));
        let hints = build_preservation_hints(&candidates);
        let message = precompress_preservation_message(&hints).expect("handoff message");

        assert_eq!(message.role, "system");
        assert!(message.content.contains("[pre-compress-handoff]"));
        assert!(message.content.contains("fact_anchor"));
        assert!(message.content.contains("project=Atlas"));
        assert!(message.content.contains("[/pre-compress-handoff]"));
    }

    #[tokio::test]
    async fn structured_tool_sequence_upserts_run_recipe_once() {
        let messages = vec![
            ChatMessage::user("Find Matrix service status"),
            ChatMessage::assistant(format_projected_tool_call("a", "rg", "Matrix")),
            ChatMessage::tool("found matrix-unit".to_string()),
            ChatMessage::assistant(format_projected_tool_call(
                "b",
                "systemctl",
                "show matrix-unit",
            )),
            ChatMessage::tool("ActiveState=active".to_string()),
        ];
        let store = InMemoryRunRecipeStore::new();
        let input = MemoryPreCompressHandoffInput {
            agent_id: "agent",
            reason: MemoryPreCompressHandoffReason::LiveAgentCompaction,
            start_index: 0,
            end_index: messages.len(),
            transcript: "",
            messages: &messages,
            message_indices: &[],
            recent_tool_repairs: &[],
            run_recipe_store: Some(&store),
            observed_at_unix: 100,
        };

        let report = execute_memory_precompress_handoff(None, input.clone()).await;
        assert_eq!(report.run_recipes_upserted, 1);
        let recipes = store.list("agent");
        assert_eq!(recipes.len(), 1);
        assert_eq!(
            recipes[0].tool_pattern,
            vec!["rg".to_string(), "systemctl".to_string()]
        );

        let second = execute_memory_precompress_handoff(None, input).await;
        assert_eq!(second.run_recipes_upserted, 0);
        assert_eq!(store.list("agent").len(), 1);
    }

    #[test]
    fn failure_candidate_uses_typed_repair_shape_without_raw_detail() {
        let trace = ToolRepairTrace {
            tool_name: "shell".into(),
            failure_kind: ToolFailureKind::RuntimeError,
            suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
            detail: Some("secret raw stderr".into()),
            repeat_count: 2,
            ..ToolRepairTrace::default()
        };
        let input = MemoryPreCompressHandoffInput {
            agent_id: "agent",
            reason: MemoryPreCompressHandoffReason::LiveAgentCompaction,
            start_index: 0,
            end_index: 0,
            transcript: "",
            messages: &[],
            message_indices: &[],
            recent_tool_repairs: &[trace],
            run_recipe_store: None,
            observed_at_unix: 100,
        };
        let candidates = build_precompress_handoff_candidates(&input);
        let failure = candidates
            .iter()
            .find(|candidate| candidate.kind == MemoryPreCompressCandidateKind::FailurePattern)
            .expect("failure candidate");

        assert!(failure.mutation.text.contains("repeat=2"));
        assert!(!failure.mutation.text.contains("secret raw stderr"));
    }
}
