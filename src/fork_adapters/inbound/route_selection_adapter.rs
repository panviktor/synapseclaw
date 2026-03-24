//! Adapter: wraps existing `Mutex<HashMap<String, ChannelRouteSelection>>` as RouteSelectionPort.

use crate::fork_core::ports::route_selection::{RouteSelection, RouteSelectionPort};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
