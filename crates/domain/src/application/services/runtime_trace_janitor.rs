//! Bounded cleanup for short-lived runtime self-diagnosis traces.

use crate::application::services::runtime_assumptions::{
    bound_runtime_assumptions, runtime_assumption_kind_name, RuntimeAssumption,
    RuntimeAssumptionFreshness,
};
use crate::application::services::runtime_calibration::{
    clean_runtime_calibration_records, runtime_calibration_comparison_name,
    RuntimeCalibrationComparison, RuntimeCalibrationRecord,
};
use crate::application::services::runtime_watchdog::{
    runtime_watchdog_reason_name, runtime_watchdog_subsystem_name, RuntimeWatchdogAction,
    RuntimeWatchdogAlert, RuntimeWatchdogReason, RuntimeWatchdogSeverity, RuntimeWatchdogSubsystem,
};
use crate::application::services::session_handoff::{
    bound_session_handoff_packet, session_handoff_reason_name, SessionHandoffPacket,
};
use crate::application::services::tool_repair::{
    MAX_TOOL_REPAIR_HISTORY, TOOL_REPAIR_TRACE_TTL_SECS,
};
use crate::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, ToolRepairTrace,
};
use std::collections::BTreeMap;

pub const RUNTIME_TRACE_JANITOR_TTL_SECS: i64 = TOOL_REPAIR_TRACE_TTL_SECS;
const MAX_WATCHDOG_ALERT_HISTORY: usize = 12;
const MAX_HANDOFF_ARTIFACTS: usize = 4;
const MAX_PROMOTION_CANDIDATES: usize = 8;
const MAX_SIGNATURE_CHARS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTraceSource {
    ToolRepair,
    RuntimeAssumption,
    WatchdogAlert,
    RuntimeCalibration,
    SessionHandoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTracePromotionGate {
    ToolFailurePattern,
    AssumptionReview,
    WatchdogReview,
    CalibrationReview,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTracePromotionCandidate {
    pub source: RuntimeTraceSource,
    pub gate: RuntimeTracePromotionGate,
    pub signature: String,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeHandoffArtifact {
    pub observed_at_unix: i64,
    pub packet: SessionHandoffPacket,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeTraceJanitorInput<'a> {
    pub tool_repairs: &'a [ToolRepairTrace],
    pub assumptions: &'a [RuntimeAssumption],
    pub watchdog_alerts: &'a [RuntimeWatchdogAlert],
    pub calibration_records: &'a [RuntimeCalibrationRecord],
    pub handoff_artifacts: &'a [RuntimeHandoffArtifact],
    pub now_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceJanitorOutput {
    pub tool_repairs: Vec<ToolRepairTrace>,
    pub assumptions: Vec<RuntimeAssumption>,
    pub watchdog_alerts: Vec<RuntimeWatchdogAlert>,
    pub calibration_records: Vec<RuntimeCalibrationRecord>,
    pub handoff_artifacts: Vec<RuntimeHandoffArtifact>,
    pub report: RuntimeTraceJanitorReport,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeTraceJanitorReport {
    pub removed_tool_repairs: usize,
    pub removed_assumptions: usize,
    pub removed_watchdog_alerts: usize,
    pub removed_calibration_records: usize,
    pub removed_handoff_artifacts: usize,
    pub promotion_candidates: Vec<RuntimeTracePromotionCandidate>,
}

impl RuntimeTraceJanitorReport {
    pub fn removed_total(&self) -> usize {
        self.removed_tool_repairs
            + self.removed_assumptions
            + self.removed_watchdog_alerts
            + self.removed_calibration_records
            + self.removed_handoff_artifacts
    }
}

pub fn run_runtime_trace_janitor(input: RuntimeTraceJanitorInput<'_>) -> RuntimeTraceJanitorOutput {
    let tool_repairs = clean_tool_repairs(input.tool_repairs, input.now_unix);
    let assumptions = bound_runtime_assumptions(input.assumptions.to_vec());
    let watchdog_alerts = clean_watchdog_alerts(input.watchdog_alerts, input.now_unix);
    let calibration_records =
        clean_runtime_calibration_records(input.calibration_records, input.now_unix);
    let handoff_artifacts = clean_handoff_artifacts(input.handoff_artifacts, input.now_unix);
    let promotion_candidates = collect_promotion_candidates(
        &tool_repairs,
        &assumptions,
        &watchdog_alerts,
        &calibration_records,
        input.now_unix,
    );

    RuntimeTraceJanitorOutput {
        report: RuntimeTraceJanitorReport {
            removed_tool_repairs: input.tool_repairs.len().saturating_sub(tool_repairs.len()),
            removed_assumptions: input.assumptions.len().saturating_sub(assumptions.len()),
            removed_watchdog_alerts: input
                .watchdog_alerts
                .len()
                .saturating_sub(watchdog_alerts.len()),
            removed_calibration_records: input
                .calibration_records
                .len()
                .saturating_sub(calibration_records.len()),
            removed_handoff_artifacts: input
                .handoff_artifacts
                .len()
                .saturating_sub(handoff_artifacts.len()),
            promotion_candidates,
        },
        tool_repairs,
        assumptions,
        watchdog_alerts,
        calibration_records,
        handoff_artifacts,
    }
}

fn clean_tool_repairs(history: &[ToolRepairTrace], now_unix: i64) -> Vec<ToolRepairTrace> {
    let cutoff = now_unix.saturating_sub(RUNTIME_TRACE_JANITOR_TTL_SECS);
    let mut by_signature = BTreeMap::<String, ToolRepairTrace>::new();
    for trace in history
        .iter()
        .filter(|trace| trace.observed_at_unix >= cutoff)
    {
        let signature = tool_repair_signature(trace);
        match by_signature.get_mut(&signature) {
            Some(existing) if existing.observed_at_unix < trace.observed_at_unix => {
                *existing = trace.clone();
            }
            None => {
                by_signature.insert(signature, trace.clone());
            }
            Some(_) => {}
        }
    }

    let mut records = by_signature.into_values().collect::<Vec<_>>();
    records.sort_by(|left, right| left.observed_at_unix.cmp(&right.observed_at_unix));
    if records.len() > MAX_TOOL_REPAIR_HISTORY {
        let overflow = records.len() - MAX_TOOL_REPAIR_HISTORY;
        records.drain(0..overflow);
    }
    records
}

fn clean_watchdog_alerts(
    history: &[RuntimeWatchdogAlert],
    now_unix: i64,
) -> Vec<RuntimeWatchdogAlert> {
    let cutoff = now_unix.saturating_sub(RUNTIME_TRACE_JANITOR_TTL_SECS);
    let mut by_signature = BTreeMap::<
        (
            RuntimeWatchdogSubsystem,
            RuntimeWatchdogReason,
            RuntimeWatchdogAction,
        ),
        RuntimeWatchdogAlert,
    >::new();
    for alert in history
        .iter()
        .filter(|alert| alert.observed_at_unix >= cutoff)
    {
        let signature = (alert.subsystem, alert.reason, alert.recommended_action);
        match by_signature.get_mut(&signature) {
            Some(existing) => {
                existing.severity = existing.severity.max(alert.severity);
                existing.observed_at_unix = existing.observed_at_unix.max(alert.observed_at_unix);
            }
            None => {
                by_signature.insert(signature, alert.clone());
            }
        }
    }

    let mut records = by_signature.into_values().collect::<Vec<_>>();
    records.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| right.observed_at_unix.cmp(&left.observed_at_unix))
            .then_with(|| left.subsystem.cmp(&right.subsystem))
            .then_with(|| left.reason.cmp(&right.reason))
    });
    records.truncate(MAX_WATCHDOG_ALERT_HISTORY);
    records
}

fn clean_handoff_artifacts(
    history: &[RuntimeHandoffArtifact],
    now_unix: i64,
) -> Vec<RuntimeHandoffArtifact> {
    let cutoff = now_unix.saturating_sub(RUNTIME_TRACE_JANITOR_TTL_SECS);
    let mut by_signature = BTreeMap::<String, RuntimeHandoffArtifact>::new();
    for artifact in history
        .iter()
        .filter(|artifact| artifact.observed_at_unix >= cutoff)
    {
        let mut artifact = artifact.clone();
        artifact.packet = bound_session_handoff_packet(artifact.packet);
        let signature = handoff_artifact_signature(&artifact);
        match by_signature.get_mut(&signature) {
            Some(existing) if existing.observed_at_unix < artifact.observed_at_unix => {
                *existing = artifact;
            }
            None => {
                by_signature.insert(signature, artifact);
            }
            Some(_) => {}
        }
    }

    let mut records = by_signature.into_values().collect::<Vec<_>>();
    records.sort_by(|left, right| right.observed_at_unix.cmp(&left.observed_at_unix));
    records.truncate(MAX_HANDOFF_ARTIFACTS);
    records
}

fn collect_promotion_candidates(
    tool_repairs: &[ToolRepairTrace],
    assumptions: &[RuntimeAssumption],
    watchdog_alerts: &[RuntimeWatchdogAlert],
    calibration_records: &[RuntimeCalibrationRecord],
    now_unix: i64,
) -> Vec<RuntimeTracePromotionCandidate> {
    let mut candidates = Vec::new();
    collect_tool_repair_promotion_candidates(&mut candidates, tool_repairs);
    collect_assumption_promotion_candidates(&mut candidates, assumptions, now_unix);
    collect_watchdog_promotion_candidates(&mut candidates, watchdog_alerts);
    collect_calibration_promotion_candidates(&mut candidates, calibration_records);
    candidates.sort_by(|left, right| {
        right
            .observed_at_unix
            .cmp(&left.observed_at_unix)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.signature.cmp(&right.signature))
    });
    candidates.truncate(MAX_PROMOTION_CANDIDATES);
    candidates
}

fn collect_tool_repair_promotion_candidates(
    candidates: &mut Vec<RuntimeTracePromotionCandidate>,
    tool_repairs: &[ToolRepairTrace],
) {
    let mut classes = BTreeMap::<&'static str, (usize, i64)>::new();
    for repair in tool_repairs {
        let class = tool_failure_kind_name(repair.failure_kind);
        let entry = classes.entry(class).or_insert((0, repair.observed_at_unix));
        entry.0 += 1;
        entry.1 = entry.1.max(repair.observed_at_unix);
    }

    for (class, (count, observed_at_unix)) in classes {
        if count >= 2 {
            push_promotion_candidate(
                candidates,
                RuntimeTracePromotionCandidate {
                    source: RuntimeTraceSource::ToolRepair,
                    gate: RuntimeTracePromotionGate::ToolFailurePattern,
                    signature: bounded_signature(class),
                    observed_at_unix,
                },
            );
        }
    }
}

fn collect_assumption_promotion_candidates(
    candidates: &mut Vec<RuntimeTracePromotionCandidate>,
    assumptions: &[RuntimeAssumption],
    now_unix: i64,
) {
    for assumption in assumptions
        .iter()
        .filter(|assumption| assumption.freshness == RuntimeAssumptionFreshness::Challenged)
    {
        push_promotion_candidate(
            candidates,
            RuntimeTracePromotionCandidate {
                source: RuntimeTraceSource::RuntimeAssumption,
                gate: RuntimeTracePromotionGate::AssumptionReview,
                signature: bounded_signature(&format!(
                    "{}:{}",
                    runtime_assumption_kind_name(assumption.kind),
                    assumption.value
                )),
                observed_at_unix: now_unix,
            },
        );
    }
}

fn collect_watchdog_promotion_candidates(
    candidates: &mut Vec<RuntimeTracePromotionCandidate>,
    watchdog_alerts: &[RuntimeWatchdogAlert],
) {
    for alert in watchdog_alerts
        .iter()
        .filter(|alert| alert.severity >= RuntimeWatchdogSeverity::Critical)
    {
        push_promotion_candidate(
            candidates,
            RuntimeTracePromotionCandidate {
                source: RuntimeTraceSource::WatchdogAlert,
                gate: RuntimeTracePromotionGate::WatchdogReview,
                signature: bounded_signature(&format!(
                    "{}:{}",
                    runtime_watchdog_subsystem_name(alert.subsystem),
                    runtime_watchdog_reason_name(alert.reason)
                )),
                observed_at_unix: alert.observed_at_unix,
            },
        );
    }
}

fn collect_calibration_promotion_candidates(
    candidates: &mut Vec<RuntimeTracePromotionCandidate>,
    calibration_records: &[RuntimeCalibrationRecord],
) {
    for record in calibration_records
        .iter()
        .filter(|record| record.comparison == RuntimeCalibrationComparison::OverconfidentFailure)
    {
        push_promotion_candidate(
            candidates,
            RuntimeTracePromotionCandidate {
                source: RuntimeTraceSource::RuntimeCalibration,
                gate: RuntimeTracePromotionGate::CalibrationReview,
                signature: bounded_signature(&format!(
                    "{}:{}",
                    runtime_calibration_comparison_name(record.comparison),
                    record.decision_signature
                )),
                observed_at_unix: record.observed_at_unix,
            },
        );
    }
}

fn push_promotion_candidate(
    candidates: &mut Vec<RuntimeTracePromotionCandidate>,
    candidate: RuntimeTracePromotionCandidate,
) {
    if let Some(existing) = candidates.iter_mut().find(|existing| {
        existing.source == candidate.source
            && existing.gate == candidate.gate
            && existing.signature == candidate.signature
    }) {
        existing.observed_at_unix = existing.observed_at_unix.max(candidate.observed_at_unix);
    } else {
        candidates.push(candidate);
    }
}

fn tool_repair_signature(trace: &ToolRepairTrace) -> String {
    bounded_signature(&format!(
        "tool={},failure={},action={},detail={}",
        trace.tool_name,
        tool_failure_kind_name(trace.failure_kind),
        tool_repair_action_name(trace.suggested_action),
        trace.detail.as_deref().unwrap_or("")
    ))
}

fn handoff_artifact_signature(artifact: &RuntimeHandoffArtifact) -> String {
    bounded_signature(&format!(
        "reason={},action={},task={}",
        session_handoff_reason_name(artifact.packet.reason),
        artifact.packet.recommended_action.as_deref().unwrap_or(""),
        artifact.packet.active_task.as_deref().unwrap_or("")
    ))
}

fn bounded_signature(value: &str) -> String {
    value.trim().chars().take(MAX_SIGNATURE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::runtime_assumptions::{
        RuntimeAssumptionInvalidation, RuntimeAssumptionKind, RuntimeAssumptionReplacementPath,
        RuntimeAssumptionSource,
    };
    use crate::application::services::runtime_calibration::{
        build_runtime_calibration_record, RuntimeCalibrationDecisionKind,
        RuntimeCalibrationObservation, RuntimeCalibrationOutcome,
    };
    use crate::application::services::runtime_watchdog::RuntimeWatchdogAction;
    use crate::application::services::session_handoff::SessionHandoffReason;
    use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction};

    fn trace_at(
        observed_at_unix: i64,
        tool_name: &str,
        failure_kind: ToolFailureKind,
        detail: Option<&str>,
    ) -> ToolRepairTrace {
        ToolRepairTrace {
            observed_at_unix,
            tool_name: tool_name.into(),
            failure_kind,
            suggested_action: ToolRepairAction::InspectRuntimeFailure,
            detail: detail.map(str::to_string),
        }
    }

    fn assumption(freshness: RuntimeAssumptionFreshness, value: &str) -> RuntimeAssumption {
        RuntimeAssumption {
            kind: RuntimeAssumptionKind::RouteCapability,
            source: RuntimeAssumptionSource::RouteAdmission,
            freshness,
            confidence_basis_points: 3_500,
            value: value.into(),
            invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
            replacement_path: RuntimeAssumptionReplacementPath::SwitchRoute,
        }
    }

    #[test]
    fn janitor_expires_dedupes_and_bounds_tool_repairs() {
        let now = 10_000;
        let output = run_runtime_trace_janitor(RuntimeTraceJanitorInput {
            tool_repairs: &[
                trace_at(
                    now - RUNTIME_TRACE_JANITOR_TTL_SECS - 1,
                    "shell",
                    ToolFailureKind::RuntimeError,
                    Some("old"),
                ),
                trace_at(
                    now - 20,
                    "shell",
                    ToolFailureKind::RuntimeError,
                    Some("same"),
                ),
                trace_at(
                    now - 10,
                    "shell",
                    ToolFailureKind::RuntimeError,
                    Some("same"),
                ),
                trace_at(
                    now - 5,
                    "web_fetch",
                    ToolFailureKind::RuntimeError,
                    Some("distinct"),
                ),
            ],
            now_unix: now,
            ..Default::default()
        });

        assert_eq!(output.tool_repairs.len(), 2);
        assert!(output
            .tool_repairs
            .iter()
            .any(|trace| trace.detail.as_deref() == Some("same")
                && trace.observed_at_unix == now - 10));
        assert_eq!(output.report.removed_tool_repairs, 2);
        assert!(output.report.promotion_candidates.iter().any(|candidate| {
            candidate.source == RuntimeTraceSource::ToolRepair
                && candidate.gate == RuntimeTracePromotionGate::ToolFailurePattern
        }));
    }

    #[test]
    fn janitor_keeps_promotions_behind_explicit_gates() {
        let now = 100;
        let calibration = build_runtime_calibration_record(RuntimeCalibrationObservation {
            decision_kind: RuntimeCalibrationDecisionKind::RouteChoice,
            decision_signature: "route:primary".into(),
            suppression_key: None,
            confidence_basis_points: 9_000,
            outcome: RuntimeCalibrationOutcome::Failed,
            observed_at_unix: now - 1,
        })
        .unwrap();

        let output = run_runtime_trace_janitor(RuntimeTraceJanitorInput {
            assumptions: &[assumption(
                RuntimeAssumptionFreshness::Challenged,
                "route_failed",
            )],
            calibration_records: &[calibration],
            now_unix: now,
            ..Default::default()
        });

        assert!(output.report.promotion_candidates.iter().any(|candidate| {
            candidate.source == RuntimeTraceSource::RuntimeAssumption
                && candidate.gate == RuntimeTracePromotionGate::AssumptionReview
        }));
        assert!(output.report.promotion_candidates.iter().any(|candidate| {
            candidate.source == RuntimeTraceSource::RuntimeCalibration
                && candidate.gate == RuntimeTracePromotionGate::CalibrationReview
        }));
    }

    #[test]
    fn janitor_cleans_watchdog_and_handoff_artifacts() {
        let now = 500;
        let old_alert = RuntimeWatchdogAlert {
            subsystem: RuntimeWatchdogSubsystem::ContextBudget,
            severity: RuntimeWatchdogSeverity::Critical,
            reason: RuntimeWatchdogReason::ContextOverflow,
            recommended_action: RuntimeWatchdogAction::CompactContext,
            observed_at_unix: now - RUNTIME_TRACE_JANITOR_TTL_SECS - 1,
        };
        let fresh_alert = RuntimeWatchdogAlert {
            subsystem: RuntimeWatchdogSubsystem::ContextBudget,
            severity: RuntimeWatchdogSeverity::Critical,
            reason: RuntimeWatchdogReason::ContextOverflow,
            recommended_action: RuntimeWatchdogAction::CompactContext,
            observed_at_unix: now - 1,
        };
        let older_duplicate = RuntimeWatchdogAlert {
            observed_at_unix: now - 10,
            severity: RuntimeWatchdogSeverity::Caution,
            ..fresh_alert.clone()
        };
        let artifact = RuntimeHandoffArtifact {
            observed_at_unix: now - 1,
            packet: SessionHandoffPacket {
                reason: SessionHandoffReason::RouteSwitch,
                recommended_action: None,
                active_task: Some("switch route".into()),
                current_defaults: vec!["a".repeat(400)],
                anchors: Vec::new(),
                unresolved_questions: Vec::new(),
                assumptions: Vec::new(),
            },
        };

        let output = run_runtime_trace_janitor(RuntimeTraceJanitorInput {
            watchdog_alerts: &[old_alert, older_duplicate, fresh_alert],
            handoff_artifacts: &[artifact],
            now_unix: now,
            ..Default::default()
        });

        assert_eq!(output.watchdog_alerts.len(), 1);
        assert_eq!(
            output.watchdog_alerts[0].severity,
            RuntimeWatchdogSeverity::Critical
        );
        assert_eq!(output.report.removed_watchdog_alerts, 2);
        assert_eq!(output.handoff_artifacts.len(), 1);
        assert!(output.handoff_artifacts[0].packet.current_defaults[0].len() <= 183);
        assert!(output.report.promotion_candidates.iter().any(|candidate| {
            candidate.source == RuntimeTraceSource::WatchdogAlert
                && candidate.gate == RuntimeTracePromotionGate::WatchdogReview
        }));
    }
}
