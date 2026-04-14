//! Runtime decision trace — compact per-turn diagnostics.
//!
//! This service records decisions that already happened in admission, context
//! budgeting, tool repair, and post-turn learning. It must not introduce new
//! routing or memory policy.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::application::services::model_lane_resolution::{
    resolved_model_profile_confidence_name, resolved_model_profile_freshness_name,
    resolved_model_profile_source_name, ResolvedModelProfile,
};
use crate::application::services::provider_context_budget::{
    assess_provider_context_budget, provider_context_artifact_name,
    provider_context_budget_tier_name, provider_context_condensation_mode_name,
    provider_context_turn_shape_name, ProviderContextBudgetInput,
};
use crate::application::services::turn_admission::CandidateAdmissionDecision;
use crate::config::schema::{CapabilityLane, ModelFeature};
use crate::domain::memory::MemoryCategory;
use crate::domain::memory_mutation::{MutationAction, MutationDecision, MutationWriteClass};
use crate::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, ToolRepairAction, ToolRepairTrace,
};
use crate::domain::turn_admission::{
    admission_repair_hint_label, candidate_admission_reason_label, context_pressure_state_name,
    turn_admission_action_name, turn_intent_name,
};
use crate::ports::route_selection::ContextCacheStats;

pub const MAX_RUNTIME_DECISION_TRACE_HISTORY: usize = 8;
pub const MAX_RUNTIME_DECISION_TRACE_ITEMS: usize = 8;
const MAX_TRACE_TEXT_CHARS: usize = 160;
static RUNTIME_DECISION_TRACE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeDecisionTrace {
    pub trace_id: String,
    pub observed_at_unix: i64,
    pub route: RuntimeTraceRouteDecision,
    pub model_profile: RuntimeTraceModelProfileSnapshot,
    pub context: RuntimeTraceContextSnapshot,
    pub tools: Vec<RuntimeTraceToolDecision>,
    pub memory: Vec<RuntimeTraceMemoryDecision>,
    pub auxiliary: Vec<RuntimeTraceAuxiliaryDecision>,
    pub notes: Vec<RuntimeTraceNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceRouteRef {
    pub provider: String,
    pub model: String,
    pub lane: Option<String>,
    pub candidate_index: Option<usize>,
}

impl RuntimeTraceRouteRef {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        lane: Option<CapabilityLane>,
        candidate_index: Option<usize>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            lane: lane.map(|lane| lane.as_str().to_string()),
            candidate_index,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceRouteDecision {
    pub before: RuntimeTraceRouteRef,
    pub after: RuntimeTraceRouteRef,
    pub reroute_applied: bool,
    pub intent: String,
    pub pressure_state: String,
    pub action: String,
    pub reasons: Vec<String>,
    pub recommended_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceModelProfileSnapshot {
    pub context_window_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub features: Vec<String>,
    pub context_window_source: String,
    pub context_window_freshness: String,
    pub context_window_confidence: String,
    pub max_output_source: String,
    pub max_output_freshness: String,
    pub max_output_confidence: String,
    pub features_source: String,
    pub features_freshness: String,
    pub features_confidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceContextSnapshot {
    pub total_chars: usize,
    pub estimated_total_tokens: usize,
    pub target_total_tokens: usize,
    pub ceiling_total_tokens: usize,
    pub protected_chars: usize,
    pub removable_chars: usize,
    pub chars_over_target: usize,
    pub chars_over_ceiling: usize,
    pub tokens_headroom_to_target: usize,
    pub tokens_headroom_to_ceiling: usize,
    pub turn_shape: String,
    pub budget_tier: String,
    pub requires_compaction: bool,
    pub condensation_mode: Option<String>,
    pub condensation_target: Option<String>,
    pub condensation_minimum_reclaim_chars: Option<usize>,
    pub condensation_prefers_cached_artifact: bool,
    pub cache: Option<RuntimeTraceContextCacheSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceContextCacheSnapshot {
    pub entries: usize,
    pub hits: u64,
    pub max_entries: usize,
    pub ttl_secs: u64,
    pub loaded: bool,
    pub threshold_basis_points: u32,
    pub target_basis_points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceToolDecision {
    pub observed_at_unix: i64,
    pub tool_name: String,
    pub failure_kind: String,
    pub suggested_action: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceMemoryDecision {
    pub observed_at_unix: i64,
    pub source: String,
    pub category: String,
    pub write_class: Option<String>,
    pub action: String,
    pub applied: bool,
    pub entry_id_present: bool,
    pub reason: String,
    pub similarity_basis_points: Option<u32>,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceAuxiliaryDecision {
    pub observed_at_unix: i64,
    pub kind: String,
    pub action: String,
    pub count: usize,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceNote {
    pub observed_at_unix: i64,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeDecisionTraceUpdate {
    pub tools: Vec<RuntimeTraceToolDecision>,
    pub memory: Vec<RuntimeTraceMemoryDecision>,
    pub auxiliary: Vec<RuntimeTraceAuxiliaryDecision>,
    pub notes: Vec<RuntimeTraceNote>,
}

pub struct RuntimeDecisionTraceInput<'a> {
    pub trace_id: String,
    pub observed_at_unix: i64,
    pub route_before: RuntimeTraceRouteRef,
    pub route_after: RuntimeTraceRouteRef,
    pub admission: &'a CandidateAdmissionDecision,
    pub model_profile: &'a ResolvedModelProfile,
    pub provider_context: ProviderContextBudgetInput,
    pub context_cache: Option<ContextCacheStats>,
}

pub fn build_runtime_decision_trace(input: RuntimeDecisionTraceInput<'_>) -> RuntimeDecisionTrace {
    let budget = assess_provider_context_budget(input.provider_context);
    let condensation_plan = input.admission.condensation_plan;
    RuntimeDecisionTrace {
        trace_id: input.trace_id,
        observed_at_unix: input.observed_at_unix,
        route: RuntimeTraceRouteDecision {
            reroute_applied: input.route_before != input.route_after,
            before: input.route_before,
            after: input.route_after,
            intent: turn_intent_name(input.admission.snapshot.intent).to_string(),
            pressure_state: context_pressure_state_name(input.admission.snapshot.pressure_state)
                .to_string(),
            action: turn_admission_action_name(input.admission.snapshot.action).to_string(),
            reasons: input
                .admission
                .reasons
                .iter()
                .map(candidate_admission_reason_label)
                .collect(),
            recommended_action: input
                .admission
                .recommended_action
                .map(admission_repair_hint_label),
        },
        model_profile: RuntimeTraceModelProfileSnapshot {
            context_window_tokens: input.model_profile.context_window_tokens,
            max_output_tokens: input.model_profile.max_output_tokens,
            features: input
                .model_profile
                .features
                .iter()
                .map(|feature| model_feature_name(feature).to_string())
                .collect(),
            context_window_source: resolved_model_profile_source_name(
                input.model_profile.context_window_source,
            )
            .to_string(),
            context_window_freshness: resolved_model_profile_freshness_name(
                input.model_profile.context_window_freshness(),
            )
            .to_string(),
            context_window_confidence: resolved_model_profile_confidence_name(
                input.model_profile.context_window_confidence(),
            )
            .to_string(),
            max_output_source: resolved_model_profile_source_name(
                input.model_profile.max_output_source,
            )
            .to_string(),
            max_output_freshness: resolved_model_profile_freshness_name(
                input.model_profile.max_output_freshness(),
            )
            .to_string(),
            max_output_confidence: resolved_model_profile_confidence_name(
                input.model_profile.max_output_confidence(),
            )
            .to_string(),
            features_source: resolved_model_profile_source_name(
                input.model_profile.features_source,
            )
            .to_string(),
            features_freshness: resolved_model_profile_freshness_name(
                input.model_profile.features_freshness(),
            )
            .to_string(),
            features_confidence: resolved_model_profile_confidence_name(
                input.model_profile.features_confidence(),
            )
            .to_string(),
        },
        context: RuntimeTraceContextSnapshot {
            total_chars: input.provider_context.total_chars,
            estimated_total_tokens: budget.snapshot.estimated_total_tokens,
            target_total_tokens: budget.snapshot.target_total_tokens,
            ceiling_total_tokens: budget.snapshot.ceiling_total_tokens,
            protected_chars: budget.snapshot.protected_chars,
            removable_chars: budget.snapshot.removable_chars,
            chars_over_target: budget.snapshot.chars_over_target,
            chars_over_ceiling: budget.snapshot.chars_over_ceiling,
            tokens_headroom_to_target: budget.snapshot.tokens_headroom_to_target,
            tokens_headroom_to_ceiling: budget.snapshot.tokens_headroom_to_ceiling,
            turn_shape: provider_context_turn_shape_name(budget.turn_shape).to_string(),
            budget_tier: provider_context_budget_tier_name(budget.tier).to_string(),
            requires_compaction: input.admission.requires_compaction,
            condensation_mode: condensation_plan
                .map(|plan| provider_context_condensation_mode_name(plan.mode).to_string()),
            condensation_target: condensation_plan
                .and_then(|plan| plan.target_artifact)
                .map(|artifact| provider_context_artifact_name(artifact).to_string()),
            condensation_minimum_reclaim_chars: condensation_plan
                .map(|plan| plan.minimum_reclaim_chars),
            condensation_prefers_cached_artifact: condensation_plan
                .is_some_and(|plan| plan.prefer_cached_artifact),
            cache: input
                .context_cache
                .map(|cache| RuntimeTraceContextCacheSnapshot {
                    entries: cache.entries,
                    hits: cache.hits,
                    max_entries: cache.max_entries,
                    ttl_secs: cache.ttl_secs,
                    loaded: cache.loaded,
                    threshold_basis_points: cache.threshold_basis_points,
                    target_basis_points: cache.target_basis_points,
                }),
        },
        tools: Vec::new(),
        memory: Vec::new(),
        auxiliary: Vec::new(),
        notes: Vec::new(),
    }
}

pub fn build_runtime_decision_trace_id(observed_at_unix: i64, discriminator: &str) -> String {
    let mut hasher = DefaultHasher::new();
    discriminator.hash(&mut hasher);
    let sequence = RUNTIME_DECISION_TRACE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("rdt-{observed_at_unix}-{:016x}-{sequence}", hasher.finish())
}

pub fn append_runtime_decision_trace(
    history: &[RuntimeDecisionTrace],
    trace: RuntimeDecisionTrace,
    now_unix: i64,
    ttl_secs: i64,
) -> Vec<RuntimeDecisionTrace> {
    let mut bounded = clean_runtime_decision_traces(history, now_unix, ttl_secs);
    if let Some(existing) = bounded
        .iter_mut()
        .find(|existing| existing.trace_id == trace.trace_id)
    {
        *existing = trace;
    } else {
        bounded.push(trace);
    }
    cap_runtime_decision_traces(&mut bounded);
    bounded
}

pub fn clean_runtime_decision_traces(
    history: &[RuntimeDecisionTrace],
    now_unix: i64,
    ttl_secs: i64,
) -> Vec<RuntimeDecisionTrace> {
    let cutoff = now_unix.saturating_sub(ttl_secs);
    let mut traces = history
        .iter()
        .filter(|trace| trace.observed_at_unix >= cutoff)
        .cloned()
        .collect::<Vec<_>>();
    traces.sort_by(|left, right| left.observed_at_unix.cmp(&right.observed_at_unix));
    cap_runtime_decision_traces(&mut traces);
    traces
}

pub fn merge_runtime_decision_trace_update(
    history: &[RuntimeDecisionTrace],
    trace_id: &str,
    update: RuntimeDecisionTraceUpdate,
    now_unix: i64,
    ttl_secs: i64,
) -> Vec<RuntimeDecisionTrace> {
    let mut traces = clean_runtime_decision_traces(history, now_unix, ttl_secs);
    let Some(trace) = traces.iter_mut().find(|trace| trace.trace_id == trace_id) else {
        return traces;
    };
    append_bounded(&mut trace.tools, update.tools);
    append_bounded(&mut trace.memory, update.memory);
    append_bounded(&mut trace.auxiliary, update.auxiliary);
    append_bounded(&mut trace.notes, update.notes);
    traces
}

pub fn runtime_tool_decisions_from_repairs(
    repairs: &[ToolRepairTrace],
) -> Vec<RuntimeTraceToolDecision> {
    repairs
        .iter()
        .map(|repair| RuntimeTraceToolDecision {
            observed_at_unix: repair.observed_at_unix,
            tool_name: redact_trace_text(&repair.tool_name),
            failure_kind: tool_failure_kind_name(repair.failure_kind).to_string(),
            suggested_action: format_tool_repair_action(repair),
            detail: repair.detail.as_deref().map(redact_trace_text),
        })
        .collect()
}

pub fn runtime_memory_decision_from_mutation(
    decision: &MutationDecision,
    observed_at_unix: i64,
    source_override: Option<&str>,
    applied: bool,
    entry_id: Option<&str>,
    failure: Option<&str>,
) -> RuntimeTraceMemoryDecision {
    RuntimeTraceMemoryDecision {
        observed_at_unix,
        source: source_override
            .map(redact_trace_text)
            .unwrap_or_else(|| mutation_source_name(&decision.candidate.source).to_string()),
        category: redact_trace_text(&decision.candidate.category.to_string()),
        write_class: decision
            .candidate
            .write_class
            .map(mutation_write_class_name),
        action: mutation_action_name(&decision.action).to_string(),
        applied,
        entry_id_present: entry_id.is_some(),
        reason: sanitize_memory_reason(&decision.reason),
        similarity_basis_points: decision.similarity.map(similarity_basis_points),
        failure: failure.map(redact_trace_text),
    }
}

pub fn runtime_memory_decision_from_autosave(
    observed_at_unix: i64,
    category: &MemoryCategory,
    applied: bool,
    failure: Option<&str>,
) -> RuntimeTraceMemoryDecision {
    RuntimeTraceMemoryDecision {
        observed_at_unix,
        source: "autosave".to_string(),
        category: redact_trace_text(&category.to_string()),
        write_class: Some("generic_dialogue".to_string()),
        action: "store".to_string(),
        applied,
        entry_id_present: false,
        reason: "autosave_write_governor_accepted".to_string(),
        similarity_basis_points: None,
        failure: failure.map(redact_trace_text),
    }
}

pub fn runtime_auxiliary_decision(
    observed_at_unix: i64,
    kind: impl Into<String>,
    action: impl Into<String>,
    count: usize,
    reason: Option<&str>,
) -> RuntimeTraceAuxiliaryDecision {
    RuntimeTraceAuxiliaryDecision {
        observed_at_unix,
        kind: redact_trace_text(&kind.into()),
        action: redact_trace_text(&action.into()),
        count,
        reason: reason.map(redact_trace_text),
    }
}

fn cap_runtime_decision_traces(traces: &mut Vec<RuntimeDecisionTrace>) {
    if traces.len() > MAX_RUNTIME_DECISION_TRACE_HISTORY {
        let overflow = traces.len() - MAX_RUNTIME_DECISION_TRACE_HISTORY;
        traces.drain(0..overflow);
    }
}

fn append_bounded<T>(target: &mut Vec<T>, next: Vec<T>) {
    if next.is_empty() {
        return;
    }
    target.extend(next);
    if target.len() > MAX_RUNTIME_DECISION_TRACE_ITEMS {
        let overflow = target.len() - MAX_RUNTIME_DECISION_TRACE_ITEMS;
        target.drain(0..overflow);
    }
}

fn format_tool_repair_action(trace: &ToolRepairTrace) -> String {
    match trace.suggested_action {
        ToolRepairAction::SwitchRouteLane(lane) => format!(
            "{}:{}",
            tool_repair_action_name(trace.suggested_action),
            lane.as_str()
        ),
        _ => tool_repair_action_name(trace.suggested_action).to_string(),
    }
}

fn mutation_source_name(source: &crate::domain::memory_mutation::MutationSource) -> &'static str {
    match source {
        crate::domain::memory_mutation::MutationSource::Consolidation => "consolidation",
        crate::domain::memory_mutation::MutationSource::ExplicitUser => "explicit_user",
        crate::domain::memory_mutation::MutationSource::ToolOutput => "tool_output",
        crate::domain::memory_mutation::MutationSource::Reflection => "reflection",
    }
}

fn mutation_write_class_name(write_class: MutationWriteClass) -> String {
    match write_class {
        MutationWriteClass::Preference => "preference",
        MutationWriteClass::TaskState => "task_state",
        MutationWriteClass::FactAnchor => "fact_anchor",
        MutationWriteClass::Recipe => "recipe",
        MutationWriteClass::FailurePattern => "failure_pattern",
        MutationWriteClass::EphemeralRepairTrace => "ephemeral_repair_trace",
        MutationWriteClass::GenericDialogue => "generic_dialogue",
    }
    .to_string()
}

fn mutation_action_name(action: &MutationAction) -> &'static str {
    match action {
        MutationAction::Add => "add",
        MutationAction::Update { .. } => "update",
        MutationAction::Delete { .. } => "delete",
        MutationAction::Noop => "noop",
    }
}

fn similarity_basis_points(value: f64) -> u32 {
    (value.clamp(0.0, 1.0) * 10_000.0).round() as u32
}

fn redact_trace_text(value: &str) -> String {
    let mut text = value.trim().replace('\n', " ");
    if text.chars().count() > MAX_TRACE_TEXT_CHARS {
        text = text
            .chars()
            .take(MAX_TRACE_TEXT_CHARS.saturating_sub(3))
            .collect::<String>();
        text.push_str("...");
    }
    text
}

fn sanitize_memory_reason(value: &str) -> String {
    let reason = redact_trace_text(value);
    for marker in [", existing:", ", replacing:", ", updating:"] {
        if let Some((prefix, _)) = reason.split_once(marker) {
            return prefix.to_string();
        }
    }
    reason
}

fn model_feature_name(feature: &ModelFeature) -> &'static str {
    match feature {
        ModelFeature::ToolCalling => "tool_calling",
        ModelFeature::Vision => "vision",
        ModelFeature::Embedding => "embedding",
        ModelFeature::MultimodalUnderstanding => "multimodal_understanding",
        ModelFeature::ImageGeneration => "image_generation",
        ModelFeature::AudioGeneration => "audio_generation",
        ModelFeature::VideoGeneration => "video_generation",
        ModelFeature::MusicGeneration => "music_generation",
        ModelFeature::ServerContinuation => "server_continuation",
        ModelFeature::PromptCaching => "prompt_caching",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::model_lane_resolution::{
        ResolvedModelProfile, ResolvedModelProfileSource,
    };
    use crate::application::services::provider_context_budget::ProviderContextBudgetInput;
    use crate::application::services::turn_admission::CandidateAdmissionDecision;
    use crate::config::schema::{CapabilityLane, ModelFeature};
    use crate::domain::memory_mutation::{
        MutationAction, MutationCandidate, MutationDecision, MutationSource, MutationThresholds,
        MutationWriteClass,
    };
    use crate::domain::turn_admission::{
        AdmissionRepairHint, CandidateAdmissionReason, ContextPressureState, TurnAdmissionAction,
        TurnAdmissionSnapshot, TurnIntentCategory,
    };

    fn admission(action: TurnAdmissionAction) -> CandidateAdmissionDecision {
        CandidateAdmissionDecision {
            snapshot: TurnAdmissionSnapshot {
                intent: TurnIntentCategory::ToolHeavy,
                pressure_state: ContextPressureState::Critical,
                action,
            },
            required_lane: Some(CapabilityLane::Reasoning),
            route_override: None,
            reasons: vec![CandidateAdmissionReason::ProviderContextCritical],
            recommended_action: Some(AdmissionRepairHint::CompactSession),
            condensation_plan: None,
            requires_compaction: true,
        }
    }

    fn profile() -> ResolvedModelProfile {
        ResolvedModelProfile {
            context_window_tokens: Some(1024),
            max_output_tokens: Some(128),
            features: vec![ModelFeature::ToolCalling],
            context_window_source: ResolvedModelProfileSource::ManualConfig,
            max_output_source: ResolvedModelProfileSource::ManualConfig,
            features_source: ResolvedModelProfileSource::ManualConfig,
            observed_at_unix: None,
        }
    }

    #[test]
    fn builds_compact_route_profile_and_context_trace() {
        let trace = build_runtime_decision_trace(RuntimeDecisionTraceInput {
            trace_id: "trace-1".into(),
            observed_at_unix: 100,
            route_before: RuntimeTraceRouteRef::new("openai", "small", None, None),
            route_after: RuntimeTraceRouteRef::new(
                "openai",
                "reasoner",
                Some(CapabilityLane::Reasoning),
                Some(0),
            ),
            admission: &admission(TurnAdmissionAction::Reroute),
            model_profile: &profile(),
            provider_context: ProviderContextBudgetInput {
                total_chars: 10_000,
                target_context_window_tokens: Some(1024),
                target_max_output_tokens: Some(128),
                ..Default::default()
            },
            context_cache: None,
        });

        assert!(trace.route.reroute_applied);
        assert_eq!(trace.route.action, "reroute");
        assert_eq!(trace.model_profile.context_window_source, "manual_config");
        assert!(trace.context.requires_compaction);
        assert_eq!(trace.context.budget_tier, "over_budget");
    }

    #[test]
    fn bounds_and_expires_trace_history() {
        let mut history = Vec::new();
        for idx in 0..(MAX_RUNTIME_DECISION_TRACE_HISTORY + 2) {
            let trace = build_runtime_decision_trace(RuntimeDecisionTraceInput {
                trace_id: format!("trace-{idx}"),
                observed_at_unix: idx as i64,
                route_before: RuntimeTraceRouteRef::new("p", "m", None, None),
                route_after: RuntimeTraceRouteRef::new("p", "m", None, None),
                admission: &admission(TurnAdmissionAction::Proceed),
                model_profile: &profile(),
                provider_context: ProviderContextBudgetInput::default(),
                context_cache: None,
            });
            history = append_runtime_decision_trace(&history, trace, idx as i64, 10_000);
        }

        assert_eq!(history.len(), MAX_RUNTIME_DECISION_TRACE_HISTORY);
        assert_eq!(history[0].trace_id, "trace-2");

        let cleaned = clean_runtime_decision_traces(&history, 20_000, 1);
        assert!(cleaned.is_empty());
    }

    #[test]
    fn memory_trace_redacts_candidate_text_and_keeps_decision_shape() {
        let decision = MutationDecision {
            action: MutationAction::Noop,
            candidate: MutationCandidate {
                category: MemoryCategory::Core,
                text: "secret text that must not be copied".into(),
                confidence: 0.9,
                source: MutationSource::ExplicitUser,
                write_class: Some(MutationWriteClass::FactAnchor),
            },
            reason: "near-duplicate (score 0.950), existing: secret text that must not be copied"
                .into(),
            similarity: Some(MutationThresholds::default().noop_threshold),
        };

        let trace = runtime_memory_decision_from_mutation(&decision, 42, None, false, None, None);

        assert_eq!(trace.source, "explicit_user");
        assert_eq!(trace.action, "noop");
        assert_eq!(trace.write_class.as_deref(), Some("fact_anchor"));
        assert_eq!(trace.similarity_basis_points, Some(9_500));
        assert!(!trace.reason.contains("secret text"));
        assert_eq!(trace.reason, "near-duplicate (score 0.950)");
    }
}
