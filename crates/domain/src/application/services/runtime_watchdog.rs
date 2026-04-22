use crate::application::services::epistemic_state::{
    epistemic_entry_for_runtime_assumption, EpistemicState,
};
use crate::application::services::runtime_calibration::{
    RuntimeCalibrationComparison, RuntimeCalibrationDecisionKind, RuntimeCalibrationRecord,
};
use crate::application::services::runtime_decision_trace::RuntimeDecisionTrace;
use crate::application::services::runtime_assumptions::{
    RuntimeAssumption, RuntimeAssumptionKind, RuntimeAssumptionReplacementPath,
};
use crate::domain::memory::EmbeddingProfile;
use crate::domain::tool_repair::{ToolFailureKind, ToolRepairTrace};
use crate::domain::turn_admission::{CandidateAdmissionReason, ContextPressureState};
use crate::ports::route_selection::{ContextCacheStats, RouteAdmissionState};
use std::collections::BTreeSet;

const MAX_WATCHDOG_ALERTS: usize = 6;
const REPEATED_TOOL_FAILURE_THRESHOLD: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWatchdogSeverity {
    Info,
    Caution,
    Degraded,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWatchdogSubsystem {
    RouteCandidate,
    ToolExecution,
    ContextBudget,
    ModelProfile,
    MemoryBackend,
    EmbeddingBackend,
    ChannelDelivery,
    RuntimeAssumptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWatchdogReason {
    CapabilityMismatch,
    CapabilityMetadataWeak,
    ContextPressure,
    ContextOverflow,
    RisingPressureTrend,
    ContextCacheFull,
    RepeatedCompactionFailure,
    RepeatedToolFailure,
    ToolFailure,
    MemoryPollutionCandidate,
    StaleModelProfile,
    ChallengedAssumption,
    SubsystemDegraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeWatchdogAction {
    InspectRuntime,
    SwitchRoute,
    RefreshCapabilityMetadata,
    CompactContext,
    StartFreshHandoff,
    RepairToolRequest,
    CheckMemoryBackend,
    CheckEmbeddingBackend,
    CheckChannelDelivery,
    AskUserClarification,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeWatchdogAlert {
    pub subsystem: RuntimeWatchdogSubsystem,
    pub severity: RuntimeWatchdogSeverity,
    pub reason: RuntimeWatchdogReason,
    pub recommended_action: RuntimeWatchdogAction,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSubsystemObservation {
    pub subsystem: RuntimeWatchdogSubsystem,
    pub degraded: bool,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeSubsystemObservationInput<'a> {
    pub memory_backend_healthy: Option<bool>,
    pub embedding_profile: Option<&'a EmbeddingProfile>,
    pub channel_available: Option<bool>,
    pub now_unix: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeWatchdogInput<'a> {
    pub last_admission: Option<&'a RouteAdmissionState>,
    pub recent_admissions: &'a [RouteAdmissionState],
    pub last_tool_repair: Option<&'a ToolRepairTrace>,
    pub recent_tool_repairs: &'a [ToolRepairTrace],
    pub context_cache: Option<&'a ContextCacheStats>,
    pub assumptions: &'a [RuntimeAssumption],
    pub calibration_records: &'a [RuntimeCalibrationRecord],
    pub decision_traces: &'a [RuntimeDecisionTrace],
    pub subsystem_observations: &'a [RuntimeSubsystemObservation],
    pub now_unix: i64,
}

pub fn build_runtime_subsystem_observations(
    input: RuntimeSubsystemObservationInput<'_>,
) -> Vec<RuntimeSubsystemObservation> {
    let mut observations = Vec::new();

    if let Some(healthy) = input.memory_backend_healthy {
        observations.push(RuntimeSubsystemObservation {
            subsystem: RuntimeWatchdogSubsystem::MemoryBackend,
            degraded: !healthy,
            observed_at_unix: input.now_unix,
        });
    }

    if let Some(profile) = input.embedding_profile {
        if profile != &EmbeddingProfile::default() {
            observations.push(RuntimeSubsystemObservation {
                subsystem: RuntimeWatchdogSubsystem::EmbeddingBackend,
                degraded: profile.dimensions == 0,
                observed_at_unix: input.now_unix,
            });
        }
    }

    if let Some(available) = input.channel_available {
        observations.push(RuntimeSubsystemObservation {
            subsystem: RuntimeWatchdogSubsystem::ChannelDelivery,
            degraded: !available,
            observed_at_unix: input.now_unix,
        });
    }

    observations
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeWatchdogDigest {
    pub generated_at_unix: i64,
    pub alerts: Vec<RuntimeWatchdogAlert>,
}

impl RuntimeWatchdogDigest {
    pub fn has_alerts(&self) -> bool {
        !self.alerts.is_empty()
    }

    pub fn degraded_subsystems(&self) -> Vec<RuntimeWatchdogSubsystem> {
        let mut subsystems = BTreeSet::new();
        for alert in &self.alerts {
            if alert.severity >= RuntimeWatchdogSeverity::Degraded {
                subsystems.insert(alert.subsystem);
            }
        }
        subsystems.into_iter().collect()
    }
}

pub fn build_runtime_watchdog_digest(input: RuntimeWatchdogInput<'_>) -> RuntimeWatchdogDigest {
    let mut alerts = Vec::new();

    if let Some(admission) = input.last_admission {
        push_admission_alerts(&mut alerts, admission);
    }
    for admission in input.recent_admissions.iter().rev().take(3) {
        push_admission_alerts(&mut alerts, admission);
    }

    if let Some(repair) = input.last_tool_repair {
        push_tool_repair_alert(&mut alerts, repair, false);
    }
    push_repeated_tool_failure_alerts(&mut alerts, input.recent_tool_repairs);

    if let Some(cache) = input.context_cache {
        push_context_cache_alert(&mut alerts, cache, input.now_unix);
    }

    push_trace_alerts(&mut alerts, input.decision_traces, input.now_unix);
    push_calibration_alerts(&mut alerts, input.calibration_records);

    for assumption in input.assumptions {
        push_assumption_alert(&mut alerts, assumption, input.now_unix);
    }

    for observation in input.subsystem_observations {
        if observation.degraded {
            push_alert(
                &mut alerts,
                RuntimeWatchdogAlert {
                    subsystem: observation.subsystem,
                    severity: RuntimeWatchdogSeverity::Degraded,
                    reason: RuntimeWatchdogReason::SubsystemDegraded,
                    recommended_action: action_for_degraded_subsystem(observation.subsystem),
                    observed_at_unix: observation.observed_at_unix,
                },
            );
        }
    }

    alerts.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.subsystem.cmp(&right.subsystem))
            .then_with(|| left.reason.cmp(&right.reason))
            .then_with(|| right.observed_at_unix.cmp(&left.observed_at_unix))
    });
    alerts.truncate(MAX_WATCHDOG_ALERTS);

    RuntimeWatchdogDigest {
        generated_at_unix: input.now_unix,
        alerts,
    }
}

pub fn runtime_watchdog_severity_name(severity: RuntimeWatchdogSeverity) -> &'static str {
    match severity {
        RuntimeWatchdogSeverity::Info => "info",
        RuntimeWatchdogSeverity::Caution => "caution",
        RuntimeWatchdogSeverity::Degraded => "degraded",
        RuntimeWatchdogSeverity::Critical => "critical",
    }
}

pub fn runtime_watchdog_subsystem_name(subsystem: RuntimeWatchdogSubsystem) -> &'static str {
    match subsystem {
        RuntimeWatchdogSubsystem::RouteCandidate => "route_candidate",
        RuntimeWatchdogSubsystem::ToolExecution => "tool_execution",
        RuntimeWatchdogSubsystem::ContextBudget => "context_budget",
        RuntimeWatchdogSubsystem::ModelProfile => "model_profile",
        RuntimeWatchdogSubsystem::MemoryBackend => "memory_backend",
        RuntimeWatchdogSubsystem::EmbeddingBackend => "embedding_backend",
        RuntimeWatchdogSubsystem::ChannelDelivery => "channel_delivery",
        RuntimeWatchdogSubsystem::RuntimeAssumptions => "runtime_assumptions",
    }
}

pub fn runtime_watchdog_reason_name(reason: RuntimeWatchdogReason) -> &'static str {
    match reason {
        RuntimeWatchdogReason::CapabilityMismatch => "capability_mismatch",
        RuntimeWatchdogReason::CapabilityMetadataWeak => "capability_metadata_weak",
        RuntimeWatchdogReason::ContextPressure => "context_pressure",
        RuntimeWatchdogReason::ContextOverflow => "context_overflow",
        RuntimeWatchdogReason::RisingPressureTrend => "rising_pressure_trend",
        RuntimeWatchdogReason::ContextCacheFull => "context_cache_full",
        RuntimeWatchdogReason::RepeatedCompactionFailure => "repeated_compaction_failure",
        RuntimeWatchdogReason::RepeatedToolFailure => "repeated_tool_failure",
        RuntimeWatchdogReason::ToolFailure => "tool_failure",
        RuntimeWatchdogReason::MemoryPollutionCandidate => "memory_pollution_candidate",
        RuntimeWatchdogReason::StaleModelProfile => "stale_model_profile",
        RuntimeWatchdogReason::ChallengedAssumption => "challenged_assumption",
        RuntimeWatchdogReason::SubsystemDegraded => "subsystem_degraded",
    }
}

pub fn runtime_watchdog_action_name(action: RuntimeWatchdogAction) -> &'static str {
    match action {
        RuntimeWatchdogAction::InspectRuntime => "inspect_runtime",
        RuntimeWatchdogAction::SwitchRoute => "switch_route",
        RuntimeWatchdogAction::RefreshCapabilityMetadata => "refresh_capability_metadata",
        RuntimeWatchdogAction::CompactContext => "compact_context",
        RuntimeWatchdogAction::StartFreshHandoff => "start_fresh_handoff",
        RuntimeWatchdogAction::RepairToolRequest => "repair_tool_request",
        RuntimeWatchdogAction::CheckMemoryBackend => "check_memory_backend",
        RuntimeWatchdogAction::CheckEmbeddingBackend => "check_embedding_backend",
        RuntimeWatchdogAction::CheckChannelDelivery => "check_channel_delivery",
        RuntimeWatchdogAction::AskUserClarification => "ask_user_clarification",
    }
}

pub fn format_runtime_watchdog_context(digest: &RuntimeWatchdogDigest) -> Option<String> {
    if !digest.has_alerts() {
        return None;
    }

    let mut lines = vec!["[runtime-watchdog]".to_string()];
    for alert in &digest.alerts {
        lines.push(format!(
            "- severity={} subsystem={} reason={} action={} observed_at_unix={}",
            runtime_watchdog_severity_name(alert.severity),
            runtime_watchdog_subsystem_name(alert.subsystem),
            runtime_watchdog_reason_name(alert.reason),
            runtime_watchdog_action_name(alert.recommended_action),
            alert.observed_at_unix
        ));
    }
    Some(format!("{}\n", lines.join("\n")))
}

fn push_admission_alerts(alerts: &mut Vec<RuntimeWatchdogAlert>, admission: &RouteAdmissionState) {
    match admission.snapshot.pressure_state {
        ContextPressureState::Critical => push_alert(
            alerts,
            RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::ContextBudget,
                severity: RuntimeWatchdogSeverity::Degraded,
                reason: RuntimeWatchdogReason::ContextPressure,
                recommended_action: RuntimeWatchdogAction::CompactContext,
                observed_at_unix: admission.observed_at_unix,
            },
        ),
        ContextPressureState::OverflowRisk => push_alert(
            alerts,
            RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::ContextBudget,
                severity: RuntimeWatchdogSeverity::Critical,
                reason: RuntimeWatchdogReason::ContextOverflow,
                recommended_action: RuntimeWatchdogAction::StartFreshHandoff,
                observed_at_unix: admission.observed_at_unix,
            },
        ),
        ContextPressureState::Warning | ContextPressureState::Healthy => {}
    }

    for reason in &admission.reasons {
        push_admission_reason_alert(alerts, reason, admission.observed_at_unix);
    }
}

fn push_admission_reason_alert(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    reason: &CandidateAdmissionReason,
    observed_at_unix: i64,
) {
    let Some((subsystem, severity, reason, action)) = admission_reason_alert(reason) else {
        return;
    };
    push_alert(
        alerts,
        RuntimeWatchdogAlert {
            subsystem,
            severity,
            reason,
            recommended_action: action,
            observed_at_unix,
        },
    );
}

fn admission_reason_alert(
    reason: &CandidateAdmissionReason,
) -> Option<(
    RuntimeWatchdogSubsystem,
    RuntimeWatchdogSeverity,
    RuntimeWatchdogReason,
    RuntimeWatchdogAction,
)> {
    match reason {
        CandidateAdmissionReason::ProviderContextOverflowRisk
        | CandidateAdmissionReason::CandidateWindowExceeded => Some((
            RuntimeWatchdogSubsystem::ContextBudget,
            RuntimeWatchdogSeverity::Critical,
            RuntimeWatchdogReason::ContextOverflow,
            RuntimeWatchdogAction::StartFreshHandoff,
        )),
        CandidateAdmissionReason::ProviderContextCritical
        | CandidateAdmissionReason::CandidateWindowNearLimit => Some((
            RuntimeWatchdogSubsystem::ContextBudget,
            RuntimeWatchdogSeverity::Degraded,
            RuntimeWatchdogReason::ContextPressure,
            RuntimeWatchdogAction::CompactContext,
        )),
        CandidateAdmissionReason::CapabilityMetadataUnknown(_)
        | CandidateAdmissionReason::CapabilityMetadataStale(_)
        | CandidateAdmissionReason::CapabilityMetadataLowConfidence(_)
        | CandidateAdmissionReason::CandidateWindowMetadataUnknown => Some((
            RuntimeWatchdogSubsystem::ModelProfile,
            RuntimeWatchdogSeverity::Caution,
            RuntimeWatchdogReason::CapabilityMetadataWeak,
            RuntimeWatchdogAction::RefreshCapabilityMetadata,
        )),
        CandidateAdmissionReason::RequiresLane(_)
        | CandidateAdmissionReason::MissingFeature(_)
        | CandidateAdmissionReason::SpecializedLaneMismatch(_)
        | CandidateAdmissionReason::CalibrationSuppressedRoute => Some((
            RuntimeWatchdogSubsystem::RouteCandidate,
            RuntimeWatchdogSeverity::Degraded,
            RuntimeWatchdogReason::CapabilityMismatch,
            RuntimeWatchdogAction::SwitchRoute,
        )),
        CandidateAdmissionReason::ProviderContextWarning => Some((
            RuntimeWatchdogSubsystem::ContextBudget,
            RuntimeWatchdogSeverity::Caution,
            RuntimeWatchdogReason::ContextPressure,
            RuntimeWatchdogAction::CompactContext,
        )),
    }
}

fn push_repeated_tool_failure_alerts(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    repairs: &[ToolRepairTrace],
) {
    let mut kinds = Vec::<(ToolFailureKind, usize, i64)>::new();
    for repair in repairs {
        if let Some((_, count, observed_at)) = kinds
            .iter_mut()
            .find(|(kind, _, _)| *kind == repair.failure_kind)
        {
            *count += usize::try_from(repair.repeat_count.max(1)).unwrap_or(usize::MAX);
            *observed_at = (*observed_at).max(repair.observed_at_unix);
        } else {
            kinds.push((
                repair.failure_kind,
                usize::try_from(repair.repeat_count.max(1)).unwrap_or(usize::MAX),
                repair.observed_at_unix,
            ));
        }
    }

    for (kind, count, observed_at) in kinds {
        if count >= REPEATED_TOOL_FAILURE_THRESHOLD {
            push_tool_failure_alert(alerts, kind, observed_at, true);
        }
    }
}

fn push_tool_repair_alert(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    repair: &ToolRepairTrace,
    repeated: bool,
) {
    push_tool_failure_alert(
        alerts,
        repair.failure_kind,
        repair.observed_at_unix,
        repeated,
    );
}

fn push_tool_failure_alert(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    kind: ToolFailureKind,
    observed_at_unix: i64,
    repeated: bool,
) {
    let severity = if repeated || matches!(kind, ToolFailureKind::ContextLimitExceeded) {
        RuntimeWatchdogSeverity::Degraded
    } else {
        RuntimeWatchdogSeverity::Caution
    };
    let action = match kind {
        ToolFailureKind::ContextLimitExceeded => RuntimeWatchdogAction::CompactContext,
        ToolFailureKind::AuthFailure => RuntimeWatchdogAction::AskUserClarification,
        ToolFailureKind::CapabilityMismatch => RuntimeWatchdogAction::SwitchRoute,
        ToolFailureKind::UnknownTool
        | ToolFailureKind::PolicyBlocked
        | ToolFailureKind::DuplicateInvocation
        | ToolFailureKind::MissingResource
        | ToolFailureKind::Timeout
        | ToolFailureKind::SchemaMismatch
        | ToolFailureKind::RuntimeError
        | ToolFailureKind::ReportedFailure => RuntimeWatchdogAction::RepairToolRequest,
    };
    push_alert(
        alerts,
        RuntimeWatchdogAlert {
            subsystem: RuntimeWatchdogSubsystem::ToolExecution,
            severity,
            reason: if repeated {
                RuntimeWatchdogReason::RepeatedToolFailure
            } else {
                RuntimeWatchdogReason::ToolFailure
            },
            recommended_action: action,
            observed_at_unix,
        },
    );
}

fn push_context_cache_alert(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    cache: &ContextCacheStats,
    now_unix: i64,
) {
    if cache.loaded && cache.entries >= cache.max_entries {
        push_alert(
            alerts,
            RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::ContextBudget,
                severity: RuntimeWatchdogSeverity::Caution,
                reason: RuntimeWatchdogReason::ContextCacheFull,
                recommended_action: RuntimeWatchdogAction::CompactContext,
                observed_at_unix: now_unix,
            },
        );
    }
}

fn push_trace_alerts(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    traces: &[RuntimeDecisionTrace],
    now_unix: i64,
) {
    let recent = traces.iter().rev().take(3).collect::<Vec<_>>();
    if recent.len() >= 2
        && recent.iter().all(|trace| trace.context.requires_compaction)
        && recent
            .iter()
            .all(|trace| trace.context.chars_over_ceiling > 0 || trace.context.chars_over_target > 0)
    {
        push_alert(
            alerts,
            RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::ContextBudget,
                severity: RuntimeWatchdogSeverity::Critical,
                reason: RuntimeWatchdogReason::RepeatedCompactionFailure,
                recommended_action: RuntimeWatchdogAction::StartFreshHandoff,
                observed_at_unix: now_unix,
            },
        );
    }

    if recent.len() >= 3 {
        let mut pressure = recent
            .iter()
            .map(|trace| trace.context.estimated_total_tokens as i64)
            .collect::<Vec<_>>();
        pressure.reverse();
        if pressure.windows(2).all(|window| window[1] > window[0]) {
            push_alert(
                alerts,
                RuntimeWatchdogAlert {
                    subsystem: RuntimeWatchdogSubsystem::ContextBudget,
                    severity: RuntimeWatchdogSeverity::Caution,
                    reason: RuntimeWatchdogReason::RisingPressureTrend,
                    recommended_action: RuntimeWatchdogAction::CompactContext,
                    observed_at_unix: now_unix,
                },
            );
        }
    }

    if traces.iter().rev().take(3).any(|trace| {
        trace.memory.iter().any(|decision| {
            !decision.applied
                && (decision.reason.contains("generic")
                    || decision.reason.contains("concept")
                    || decision.reason.contains("dialogue"))
        })
    }) {
        push_alert(
            alerts,
            RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::MemoryBackend,
                severity: RuntimeWatchdogSeverity::Caution,
                reason: RuntimeWatchdogReason::MemoryPollutionCandidate,
                recommended_action: RuntimeWatchdogAction::InspectRuntime,
                observed_at_unix: now_unix,
            },
        );
    }
}

fn push_calibration_alerts(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    calibrations: &[RuntimeCalibrationRecord],
) {
    if calibrations.iter().rev().take(4).any(|record| {
        record.decision_kind == RuntimeCalibrationDecisionKind::RouteChoice
            && record.comparison == RuntimeCalibrationComparison::OverconfidentFailure
    }) {
        push_alert(
            alerts,
            RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::ModelProfile,
                severity: RuntimeWatchdogSeverity::Caution,
                reason: RuntimeWatchdogReason::StaleModelProfile,
                recommended_action: RuntimeWatchdogAction::RefreshCapabilityMetadata,
                observed_at_unix: calibrations
                    .iter()
                    .map(|record| record.observed_at_unix)
                    .max()
                    .unwrap_or_default(),
            },
        );
    }
}

fn push_assumption_alert(
    alerts: &mut Vec<RuntimeWatchdogAlert>,
    assumption: &RuntimeAssumption,
    now_unix: i64,
) {
    let epistemic = epistemic_entry_for_runtime_assumption(assumption);
    if epistemic.state != EpistemicState::NeedsVerification {
        return;
    }

    push_alert(
        alerts,
        RuntimeWatchdogAlert {
            subsystem: subsystem_for_assumption(assumption.kind),
            severity: RuntimeWatchdogSeverity::Caution,
            reason: RuntimeWatchdogReason::ChallengedAssumption,
            recommended_action: action_for_assumption_replacement(assumption.replacement_path),
            observed_at_unix: now_unix,
        },
    );
}

fn subsystem_for_assumption(kind: RuntimeAssumptionKind) -> RuntimeWatchdogSubsystem {
    match kind {
        RuntimeAssumptionKind::RouteCapability => RuntimeWatchdogSubsystem::RouteCandidate,
        RuntimeAssumptionKind::ContextWindow => RuntimeWatchdogSubsystem::ContextBudget,
        RuntimeAssumptionKind::DeliveryTarget | RuntimeAssumptionKind::CurrentConversation => {
            RuntimeWatchdogSubsystem::ChannelDelivery
        }
        RuntimeAssumptionKind::ActiveTask
        | RuntimeAssumptionKind::ProfileFact
        | RuntimeAssumptionKind::WorkspaceAnchor => RuntimeWatchdogSubsystem::RuntimeAssumptions,
    }
}

fn action_for_assumption_replacement(
    replacement: RuntimeAssumptionReplacementPath,
) -> RuntimeWatchdogAction {
    match replacement {
        RuntimeAssumptionReplacementPath::AskUserClarification => {
            RuntimeWatchdogAction::AskUserClarification
        }
        RuntimeAssumptionReplacementPath::CompactSession => RuntimeWatchdogAction::CompactContext,
        RuntimeAssumptionReplacementPath::RefreshCapabilityMetadata => {
            RuntimeWatchdogAction::RefreshCapabilityMetadata
        }
        RuntimeAssumptionReplacementPath::SwitchRoute => RuntimeWatchdogAction::SwitchRoute,
        RuntimeAssumptionReplacementPath::UpdateProfile
        | RuntimeAssumptionReplacementPath::UseCurrentConversation => {
            RuntimeWatchdogAction::InspectRuntime
        }
    }
}

fn action_for_degraded_subsystem(subsystem: RuntimeWatchdogSubsystem) -> RuntimeWatchdogAction {
    match subsystem {
        RuntimeWatchdogSubsystem::MemoryBackend => RuntimeWatchdogAction::CheckMemoryBackend,
        RuntimeWatchdogSubsystem::EmbeddingBackend => RuntimeWatchdogAction::CheckEmbeddingBackend,
        RuntimeWatchdogSubsystem::ChannelDelivery => RuntimeWatchdogAction::CheckChannelDelivery,
        RuntimeWatchdogSubsystem::RouteCandidate => RuntimeWatchdogAction::SwitchRoute,
        RuntimeWatchdogSubsystem::ContextBudget => RuntimeWatchdogAction::CompactContext,
        RuntimeWatchdogSubsystem::ModelProfile => RuntimeWatchdogAction::RefreshCapabilityMetadata,
        RuntimeWatchdogSubsystem::ToolExecution | RuntimeWatchdogSubsystem::RuntimeAssumptions => {
            RuntimeWatchdogAction::InspectRuntime
        }
    }
}

fn push_alert(alerts: &mut Vec<RuntimeWatchdogAlert>, alert: RuntimeWatchdogAlert) {
    if alerts.iter().any(|existing| {
        existing.subsystem == alert.subsystem
            && existing.reason == alert.reason
            && existing.recommended_action == alert.recommended_action
    }) {
        return;
    }
    alerts.push(alert);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::runtime_calibration::{
        RuntimeCalibrationComparison, RuntimeCalibrationDecisionKind, RuntimeCalibrationRecord,
    };
    use crate::application::services::runtime_decision_trace::{
        RuntimeDecisionTrace, RuntimeTraceAuxiliaryDecision, RuntimeTraceContextSnapshot,
        RuntimeTraceMemoryDecision, RuntimeTraceModelProfileSnapshot, RuntimeTraceNote,
        RuntimeTraceRouteDecision, RuntimeTraceRouteRef,
    };
    use crate::config::schema::{CapabilityLane, ContextCompressionConfig};
    use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction};
    use crate::domain::turn_admission::{
        AdmissionRepairHint, ContextPressureState, TurnAdmissionAction, TurnAdmissionSnapshot,
        TurnIntentCategory,
    };
    use crate::ports::route_selection::ContextCacheStats;

    #[test]
    fn context_overflow_admission_yields_critical_digest() {
        let admission = RouteAdmissionState {
            observed_at_unix: 100,
            snapshot: TurnAdmissionSnapshot {
                intent: TurnIntentCategory::Reply,
                pressure_state: ContextPressureState::OverflowRisk,
                action: TurnAdmissionAction::Compact,
            },
            required_lane: None,
            reasons: vec![CandidateAdmissionReason::ProviderContextOverflowRisk],
            recommended_action: Some(AdmissionRepairHint::StartFreshHandoff),
        };

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            last_admission: Some(&admission),
            now_unix: 200,
            ..Default::default()
        });

        assert!(digest.alerts.iter().any(|alert| {
            alert.subsystem == RuntimeWatchdogSubsystem::ContextBudget
                && alert.severity == RuntimeWatchdogSeverity::Critical
                && alert.recommended_action == RuntimeWatchdogAction::StartFreshHandoff
        }));
    }

    #[test]
    fn challenged_assumption_is_projected_as_watchdog_alert() {
        let assumption = RuntimeAssumption {
            kind: RuntimeAssumptionKind::ContextWindow,
            source: crate::application::services::runtime_assumptions::RuntimeAssumptionSource::RouteAdmission,
            freshness: crate::application::services::runtime_assumptions::RuntimeAssumptionFreshness::Challenged,
            confidence_basis_points: 3_500,
            value: "context_limit_exceeded".into(),
            invalidation: crate::application::services::runtime_assumptions::RuntimeAssumptionInvalidation::ContextOverflow,
            replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
        };

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            assumptions: &[assumption],
            now_unix: 300,
            ..Default::default()
        });

        assert_eq!(digest.alerts.len(), 1);
        assert_eq!(
            digest.alerts[0].reason,
            RuntimeWatchdogReason::ChallengedAssumption
        );
        assert_eq!(
            digest.alerts[0].recommended_action,
            RuntimeWatchdogAction::CompactContext
        );
    }

    #[test]
    fn repeated_tool_failures_are_deduped_and_bounded() {
        let repairs = vec![
            ToolRepairTrace {
                observed_at_unix: 100,
                tool_name: "message_send".into(),
                failure_kind: ToolFailureKind::ReportedFailure,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                detail: None,
                ..ToolRepairTrace::default()
            },
            ToolRepairTrace {
                observed_at_unix: 101,
                tool_name: "message_send".into(),
                failure_kind: ToolFailureKind::ReportedFailure,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                detail: None,
                ..ToolRepairTrace::default()
            },
        ];
        let cache = ContextCacheStats::from_compression_config(
            &ContextCompressionConfig {
                cache_max_entries: 1,
                ..Default::default()
            },
            1,
            0,
            true,
        );

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            recent_tool_repairs: &repairs,
            context_cache: Some(&cache),
            subsystem_observations: &[
                RuntimeSubsystemObservation {
                    subsystem: RuntimeWatchdogSubsystem::MemoryBackend,
                    degraded: true,
                    observed_at_unix: 110,
                },
                RuntimeSubsystemObservation {
                    subsystem: RuntimeWatchdogSubsystem::EmbeddingBackend,
                    degraded: true,
                    observed_at_unix: 111,
                },
                RuntimeSubsystemObservation {
                    subsystem: RuntimeWatchdogSubsystem::ChannelDelivery,
                    degraded: true,
                    observed_at_unix: 112,
                },
                RuntimeSubsystemObservation {
                    subsystem: RuntimeWatchdogSubsystem::RouteCandidate,
                    degraded: true,
                    observed_at_unix: 113,
                },
                RuntimeSubsystemObservation {
                    subsystem: RuntimeWatchdogSubsystem::ModelProfile,
                    degraded: true,
                    observed_at_unix: 114,
                },
                RuntimeSubsystemObservation {
                    subsystem: RuntimeWatchdogSubsystem::RuntimeAssumptions,
                    degraded: true,
                    observed_at_unix: 115,
                },
            ],
            now_unix: 200,
            ..Default::default()
        });

        assert!(digest.alerts.len() <= MAX_WATCHDOG_ALERTS);
        assert!(digest.alerts.iter().any(|alert| {
            alert.subsystem == RuntimeWatchdogSubsystem::ToolExecution
                && alert.reason == RuntimeWatchdogReason::RepeatedToolFailure
        }));
        assert!(digest
            .degraded_subsystems()
            .contains(&RuntimeWatchdogSubsystem::ToolExecution));
    }

    #[test]
    fn builds_live_subsystem_observations_without_flagging_disabled_embeddings() {
        let observations = build_runtime_subsystem_observations(RuntimeSubsystemObservationInput {
            memory_backend_healthy: Some(false),
            embedding_profile: Some(&crate::domain::memory::EmbeddingProfile::default()),
            channel_available: Some(false),
            now_unix: 42,
        });

        assert!(observations.iter().any(|observation| {
            observation.subsystem == RuntimeWatchdogSubsystem::MemoryBackend && observation.degraded
        }));
        assert!(observations.iter().any(|observation| {
            observation.subsystem == RuntimeWatchdogSubsystem::ChannelDelivery
                && observation.degraded
        }));
        assert!(
            !observations
                .iter()
                .any(|observation| observation.subsystem
                    == RuntimeWatchdogSubsystem::EmbeddingBackend)
        );
    }

    #[test]
    fn builds_embedding_observation_for_configured_zero_dimensional_profile() {
        let mut profile = crate::domain::memory::EmbeddingProfile::default();
        profile.profile_id = "custom:model:0".into();
        profile.provider_family = "custom".into();
        profile.model_id = "model".into();

        let observations = build_runtime_subsystem_observations(RuntimeSubsystemObservationInput {
            embedding_profile: Some(&profile),
            now_unix: 43,
            ..Default::default()
        });

        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0].subsystem,
            RuntimeWatchdogSubsystem::EmbeddingBackend
        );
        assert!(observations[0].degraded);
    }

    #[test]
    fn metadata_admission_recommends_profile_refresh() {
        let admission = RouteAdmissionState {
            observed_at_unix: 100,
            snapshot: TurnAdmissionSnapshot {
                intent: TurnIntentCategory::ImageGeneration,
                pressure_state: ContextPressureState::Healthy,
                action: TurnAdmissionAction::Reroute,
            },
            required_lane: Some(CapabilityLane::ImageGeneration),
            reasons: vec![CandidateAdmissionReason::CapabilityMetadataLowConfidence(
                CapabilityLane::ImageGeneration,
            )],
            recommended_action: Some(AdmissionRepairHint::RefreshCapabilityMetadata(
                CapabilityLane::ImageGeneration,
            )),
        };

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            last_admission: Some(&admission),
            now_unix: 200,
            ..Default::default()
        });

        assert_eq!(
            digest.alerts.first().map(|alert| alert.recommended_action),
            Some(RuntimeWatchdogAction::RefreshCapabilityMetadata)
        );
    }

    #[test]
    fn formats_bounded_runtime_watchdog_context_only_when_alerts_exist() {
        let empty = RuntimeWatchdogDigest {
            generated_at_unix: 200,
            alerts: Vec::new(),
        };
        assert!(format_runtime_watchdog_context(&empty).is_none());

        let digest = RuntimeWatchdogDigest {
            generated_at_unix: 200,
            alerts: vec![RuntimeWatchdogAlert {
                subsystem: RuntimeWatchdogSubsystem::ContextBudget,
                severity: RuntimeWatchdogSeverity::Critical,
                reason: RuntimeWatchdogReason::ContextOverflow,
                recommended_action: RuntimeWatchdogAction::StartFreshHandoff,
                observed_at_unix: 123,
            }],
        };

        let block = format_runtime_watchdog_context(&digest).unwrap();
        assert!(block.contains("[runtime-watchdog]"));
        assert!(block.contains(
            "severity=critical subsystem=context_budget reason=context_overflow action=start_fresh_handoff"
        ));
        assert!(block.ends_with('\n'));
    }

    #[test]
    fn repeated_compaction_failures_and_memory_pollution_become_alerts() {
        let traces = vec![
            trace_with_notes(
                100,
                true,
                vec![
                    RuntimeTraceNote {
                        observed_at_unix: 100,
                        kind: "compaction_failure".into(),
                        detail: "summary_failed".into(),
                    },
                    RuntimeTraceNote {
                        observed_at_unix: 100,
                        kind: "implicit_memory_recall".into(),
                        detail: "accepted=0 rejected=1".into(),
                    },
                ],
                vec![RuntimeTraceMemoryDecision {
                    observed_at_unix: 100,
                    source: "implicit_memory_recall".into(),
                    category: "local_infra".into(),
                    write_class: None,
                    action: "recall_reject".into(),
                    applied: false,
                    entry_id_present: true,
                    reason: "generic_dialogue_candidate key=matrix".into(),
                    similarity_basis_points: Some(8200),
                    failure: None,
                }],
            ),
            trace_with_notes(
                101,
                true,
                vec![RuntimeTraceNote {
                    observed_at_unix: 101,
                    kind: "compaction_failure".into(),
                    detail: "summary_failed".into(),
                }],
                vec![],
            ),
        ];

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            decision_traces: &traces,
            now_unix: 200,
            ..Default::default()
        });

        assert!(digest.alerts.iter().any(|alert| {
            alert.reason == RuntimeWatchdogReason::RepeatedCompactionFailure
        }));
        assert!(digest.alerts.iter().any(|alert| {
            alert.reason == RuntimeWatchdogReason::MemoryPollutionCandidate
        }));
    }

    #[test]
    fn stale_model_profile_from_failed_calibration_is_reported() {
        let records = vec![RuntimeCalibrationRecord {
            decision_kind: RuntimeCalibrationDecisionKind::RouteChoice,
            decision_signature: "provider=openrouter,model=gpt-5.4-mini".into(),
            suppression_key: None,
            confidence_basis_points: 9000,
            outcome: crate::application::services::runtime_calibration::RuntimeCalibrationOutcome::Failed,
            comparison: RuntimeCalibrationComparison::OverconfidentFailure,
            recommended_action: crate::application::services::runtime_calibration::RuntimeCalibrationAction::InspectOutcome,
            observed_at_unix: 100,
        }];

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            calibration_records: &records,
            now_unix: 200,
            ..Default::default()
        });

        assert!(digest.alerts.iter().any(|alert| {
            alert.reason == RuntimeWatchdogReason::StaleModelProfile
                && alert.recommended_action == RuntimeWatchdogAction::RefreshCapabilityMetadata
        }));
    }

    fn trace_with_notes(
        observed_at_unix: i64,
        requires_compaction: bool,
        notes: Vec<RuntimeTraceNote>,
        memory: Vec<RuntimeTraceMemoryDecision>,
    ) -> RuntimeDecisionTrace {
        RuntimeDecisionTrace {
            trace_id: format!("trace-{observed_at_unix}"),
            observed_at_unix,
            route: RuntimeTraceRouteDecision {
                before: RuntimeTraceRouteRef::new("openai", "gpt-5.4", None, None),
                after: RuntimeTraceRouteRef::new("openai", "gpt-5.4", None, None),
                reroute_applied: false,
                intent: "reply".into(),
                pressure_state: "healthy".into(),
                action: "proceed".into(),
                reasons: vec![],
                recommended_action: None,
            },
            model_profile: RuntimeTraceModelProfileSnapshot {
                context_window_tokens: Some(128000),
                max_output_tokens: Some(8192),
                features: vec![],
                context_window_source: "catalog".into(),
                context_window_freshness: "fresh".into(),
                context_window_confidence: "high".into(),
                max_output_source: "catalog".into(),
                max_output_freshness: "fresh".into(),
                max_output_confidence: "high".into(),
                features_source: "catalog".into(),
                features_freshness: "fresh".into(),
                features_confidence: "high".into(),
            },
            context: RuntimeTraceContextSnapshot {
                total_chars: 0,
                estimated_total_tokens: 1000,
                target_total_tokens: 4000,
                ceiling_total_tokens: 8000,
                protected_chars: 0,
                removable_chars: 0,
                chars_over_target: if requires_compaction { 1200 } else { 0 },
                chars_over_ceiling: 0,
                tokens_headroom_to_target: 3000,
                tokens_headroom_to_ceiling: 7000,
                turn_shape: "default".into(),
                budget_tier: "normal".into(),
                requires_compaction,
                condensation_mode: None,
                condensation_target: None,
                condensation_minimum_reclaim_chars: None,
                condensation_prefers_cached_artifact: false,
                cache: None,
            },
            tools: vec![],
            memory,
            auxiliary: vec![RuntimeTraceAuxiliaryDecision {
                observed_at_unix,
                kind: "test".into(),
                action: "observe".into(),
                count: 1,
                reason: None,
                lane: None,
                selected_provider: None,
                selected_model: None,
                selected_candidate_index: None,
                candidate_order: vec![],
            }],
            notes,
        }
    }
}
