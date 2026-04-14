//! Port: route selection (lane/candidate-aware provider/model override per sender).
//!
//! Manages per-sender runtime route overrides for channel sessions.

use crate::application::services::runtime_assumptions::RuntimeAssumption;
use crate::application::services::runtime_calibration::RuntimeCalibrationRecord;
use crate::application::services::runtime_decision_trace::RuntimeDecisionTrace;
use crate::application::services::runtime_trace_janitor::{
    append_runtime_watchdog_alerts, run_runtime_trace_janitor, RuntimeHandoffArtifact,
    RuntimeTraceJanitorInput,
};
use crate::application::services::runtime_watchdog::{
    build_runtime_watchdog_digest, RuntimeWatchdogAlert, RuntimeWatchdogInput,
};
use crate::config::schema::{CapabilityLane, ContextCompressionConfig};
use crate::domain::tool_repair::ToolRepairTrace;
use crate::domain::turn_admission::{
    AdmissionRepairHint, CandidateAdmissionReason, TurnAdmissionSnapshot,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteAdmissionState {
    pub observed_at_unix: i64,
    pub snapshot: TurnAdmissionSnapshot,
    pub required_lane: Option<CapabilityLane>,
    pub reasons: Vec<CandidateAdmissionReason>,
    pub recommended_action: Option<AdmissionRepairHint>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextCacheStats {
    pub entries: usize,
    pub hits: u64,
    pub max_entries: usize,
    pub ttl_secs: u64,
    pub loaded: bool,
    /// Effective compression trigger ratio for this route, in basis points.
    pub threshold_basis_points: u32,
    /// Effective retained-tail ratio for this route, in basis points.
    pub target_basis_points: u32,
    pub protect_first_n: usize,
    pub protect_last_n: usize,
    /// Effective summary target ratio for this route, in basis points.
    pub summary_basis_points: u32,
    pub max_source_chars: usize,
    pub max_summary_chars: usize,
}

impl ContextCacheStats {
    pub fn from_compression_config(
        compression: &ContextCompressionConfig,
        entries: usize,
        hits: u64,
        loaded: bool,
    ) -> Self {
        Self {
            entries,
            hits,
            max_entries: compression.cache_max_entries.max(1),
            ttl_secs: compression.cache_ttl_secs,
            loaded,
            threshold_basis_points: ratio_basis_points(compression.threshold),
            target_basis_points: ratio_basis_points(compression.target_ratio),
            protect_first_n: compression.protect_first_n,
            protect_last_n: compression.protect_last_n,
            summary_basis_points: ratio_basis_points(compression.summary_ratio),
            max_source_chars: compression.max_source_chars,
            max_summary_chars: compression.max_summary_chars,
        }
    }
}

fn ratio_basis_points(value: f64) -> u32 {
    (value.clamp(0.0, 1.0) * 10_000.0).round() as u32
}

/// A sender's active routed model selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSelection {
    pub provider: String,
    pub model: String,
    /// Resolved capability lane when the route came from lane-based selection.
    pub lane: Option<CapabilityLane>,
    /// Candidate index within the selected lane when known.
    pub candidate_index: Option<usize>,
    /// Most recent structured admission decision for this route, when available.
    pub last_admission: Option<RouteAdmissionState>,
    /// Bounded recent structured route-admission history for explainability.
    pub recent_admissions: Vec<RouteAdmissionState>,
    /// Most recent structured tool self-repair trace observed on this route.
    pub last_tool_repair: Option<ToolRepairTrace>,
    /// Bounded recent structured tool self-repair traces for explainability.
    pub recent_tool_repairs: Vec<ToolRepairTrace>,
    /// Current context/compaction cache stats, when a runtime cache service is attached.
    pub context_cache: Option<ContextCacheStats>,
    /// Bounded session/runtime assumption ledger for this route.
    pub assumptions: Vec<RuntimeAssumption>,
    /// Bounded session/runtime calibration ledger for this route.
    pub calibrations: Vec<RuntimeCalibrationRecord>,
    /// Bounded session/runtime watchdog alerts for this route.
    pub watchdog_alerts: Vec<RuntimeWatchdogAlert>,
    /// Bounded short-lived handoff artifacts for this route.
    pub handoff_artifacts: Vec<RuntimeHandoffArtifact>,
    /// Bounded per-turn runtime decision traces for this route.
    pub runtime_decision_traces: Vec<RuntimeDecisionTrace>,
}

impl RouteSelection {
    pub fn clear_runtime_diagnostics(&mut self) {
        self.last_admission = None;
        self.recent_admissions.clear();
        self.last_tool_repair = None;
        self.recent_tool_repairs.clear();
        self.context_cache = None;
        self.assumptions.clear();
        self.calibrations.clear();
        self.watchdog_alerts.clear();
        self.handoff_artifacts.clear();
        self.runtime_decision_traces.clear();
    }

    pub fn clean_runtime_traces(&mut self, now_unix: i64) {
        let cleaned = run_runtime_trace_janitor(RuntimeTraceJanitorInput {
            tool_repairs: &self.recent_tool_repairs,
            assumptions: &self.assumptions,
            watchdog_alerts: &self.watchdog_alerts,
            calibration_records: &self.calibrations,
            handoff_artifacts: &self.handoff_artifacts,
            decision_traces: &self.runtime_decision_traces,
            now_unix,
        });
        self.recent_tool_repairs = cleaned.tool_repairs;
        self.last_tool_repair = self.recent_tool_repairs.last().cloned();
        self.assumptions = cleaned.assumptions;
        self.calibrations = cleaned.calibration_records;
        self.watchdog_alerts = cleaned.watchdog_alerts;
        self.handoff_artifacts = cleaned.handoff_artifacts;
        self.runtime_decision_traces = cleaned.decision_traces;
    }

    pub fn run_runtime_trace_maintenance(&mut self, now_unix: i64) {
        self.clean_runtime_traces(now_unix);

        let digest = build_runtime_watchdog_digest(RuntimeWatchdogInput {
            last_admission: self.last_admission.as_ref(),
            recent_admissions: &self.recent_admissions,
            last_tool_repair: self.last_tool_repair.as_ref(),
            recent_tool_repairs: &self.recent_tool_repairs,
            context_cache: self.context_cache.as_ref(),
            assumptions: &self.assumptions,
            subsystem_observations: &[],
            now_unix,
        });
        self.watchdog_alerts =
            append_runtime_watchdog_alerts(&self.watchdog_alerts, &digest.alerts, now_unix);
    }
}

/// Port for managing per-sender route overrides.
pub trait RouteSelectionPort: Send + Sync {
    /// Get the active route for a sender key, or the default.
    fn get_route(&self, sender_key: &str) -> RouteSelection;

    /// Set a route override for a sender key.
    fn set_route(&self, sender_key: &str, route: RouteSelection);

    /// Clear route override (revert to default).
    fn clear_route(&self, sender_key: &str);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_cache_stats_are_built_from_effective_compression_config() {
        let compression = ContextCompressionConfig {
            threshold: 0.375,
            target_ratio: 0.2,
            protect_first_n: 3,
            protect_last_n: 9,
            summary_ratio: 0.125,
            max_source_chars: 42_000,
            max_summary_chars: 7_000,
            cache_ttl_secs: 3_600,
            cache_max_entries: 64,
            ..Default::default()
        };

        let stats = ContextCacheStats::from_compression_config(&compression, 5, 8, true);

        assert_eq!(stats.entries, 5);
        assert_eq!(stats.hits, 8);
        assert_eq!(stats.max_entries, 64);
        assert_eq!(stats.ttl_secs, 3_600);
        assert!(stats.loaded);
        assert_eq!(stats.threshold_basis_points, 3_750);
        assert_eq!(stats.target_basis_points, 2_000);
        assert_eq!(stats.protect_first_n, 3);
        assert_eq!(stats.protect_last_n, 9);
        assert_eq!(stats.summary_basis_points, 1_250);
        assert_eq!(stats.max_source_chars, 42_000);
        assert_eq!(stats.max_summary_chars, 7_000);
    }
}
