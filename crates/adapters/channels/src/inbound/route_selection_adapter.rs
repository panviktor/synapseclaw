//! Adapter: wraps existing `Mutex<HashMap<String, ChannelRouteSelection>>` as RouteSelectionPort.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use synapse_domain::ports::route_selection::{RouteSelection, RouteSelectionPort};

pub struct MutexMapRouteSelection {
    map: Arc<Mutex<HashMap<String, RouteSelection>>>,
    default_provider: String,
    default_model: String,
}

impl MutexMapRouteSelection {
    pub fn new(
        map: Arc<Mutex<HashMap<String, RouteSelection>>>,
        default_provider: String,
        default_model: String,
    ) -> Self {
        Self {
            map,
            default_provider,
            default_model,
        }
    }
}

impl RouteSelectionPort for MutexMapRouteSelection {
    fn get_route(&self, sender_key: &str) -> RouteSelection {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(sender_key)
            .cloned()
            .unwrap_or_else(|| RouteSelection {
                provider: self.default_provider.clone(),
                model: self.default_model.clone(),
                lane: None,
                candidate_index: None,
                last_admission: None,
                recent_admissions: Vec::new(),
                last_tool_repair: None,
                recent_tool_repairs: Vec::new(),
                context_cache: None,
                assumptions: Vec::new(),
                calibrations: Vec::new(),
                watchdog_alerts: Vec::new(),
                handoff_artifacts: Vec::new(),
                runtime_decision_traces: Vec::new(),
            })
    }

    fn set_route(&self, sender_key: &str, route: RouteSelection) {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sender_key.to_string(), route);
    }

    fn clear_route(&self, sender_key: &str) {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(sender_key);
    }
}
