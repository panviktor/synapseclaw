//! Port: route selection (provider/model override per sender).
//!
//! Manages per-sender provider and model overrides for channel sessions.

/// A sender's active provider + model route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSelection {
    pub provider: String,
    pub model: String,
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
