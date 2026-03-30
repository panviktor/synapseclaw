//! Port: session summary — load/save conversation summaries.
//!
//! Used for thread context seeding and context window overflow recovery.

/// Port for loading and saving session summaries.
pub trait SessionSummaryPort: Send + Sync {
    /// Load summary for a conversation key.
    fn load_summary(&self, key: &str) -> Option<String>;

    /// Save summary for a conversation key.
    fn save_summary(&self, key: &str, summary: &str);
}
