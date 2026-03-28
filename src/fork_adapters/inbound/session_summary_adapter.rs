//! Adapter: wraps existing `SessionStore` (via SessionBackend trait) as SessionSummaryPort.

use crate::fork_adapters::channels::session_backend::{ChannelSummary, SessionBackend};
use fork_core::ports::session_summary::SessionSummaryPort;
use std::sync::Arc;

pub struct SessionStoreAdapter {
    store: Arc<crate::fork_adapters::channels::session_store::SessionStore>,
}

impl SessionStoreAdapter {
    pub fn new(store: Arc<crate::fork_adapters::channels::session_store::SessionStore>) -> Self {
        Self { store }
    }
}

impl SessionSummaryPort for SessionStoreAdapter {
    fn load_summary(&self, key: &str) -> Option<String> {
        self.store.load_summary(key).map(|s| s.summary)
    }

    fn save_summary(&self, key: &str, summary: &str) {
        let _ = self.store.save_summary(
            key,
            &ChannelSummary {
                summary: summary.to_string(),
                updated_at: chrono::Utc::now(),
                message_count_at_summary: 0,
            },
        );
    }
}
