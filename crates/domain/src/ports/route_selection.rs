//! Port: route selection (lane/candidate-aware provider/model override per sender).
//!
//! Manages per-sender runtime route overrides for channel sessions.

use crate::config::schema::CapabilityLane;
use crate::domain::tool_repair::ToolRepairTrace;
use crate::domain::turn_admission::{
    AdmissionRepairHint, CandidateAdmissionReason, TurnAdmissionSnapshot,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteAdmissionState {
    pub observed_at_unix: i64,
    pub snapshot: TurnAdmissionSnapshot,
    pub reasons: Vec<CandidateAdmissionReason>,
    pub recommended_action: Option<AdmissionRepairHint>,
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
