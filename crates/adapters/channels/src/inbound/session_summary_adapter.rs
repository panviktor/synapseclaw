//! Adapter: wraps any `SessionBackend` implementation as `SessionSummaryPort`.

use crate::session_backend::{ChannelSummary, SessionBackend};
use std::sync::Arc;
use synapse_domain::ports::session_summary::SessionSummaryPort;

pub struct SessionStoreAdapter {
    store: Arc<dyn SessionBackend>,
}

impl SessionStoreAdapter {
    pub fn new(store: Arc<dyn SessionBackend>) -> Self {
        Self { store }
    }
}

impl SessionSummaryPort for SessionStoreAdapter {
    fn load_summary(&self, key: &str) -> Option<String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.store.load_summary(key))
                .map(|s| s.summary)
        })
    }

    fn save_summary(&self, key: &str, summary: &str) {
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.store.save_summary(
                key,
                &ChannelSummary {
                    summary: summary.to_string(),
                    updated_at: chrono::Utc::now(),
                    message_count_at_summary: 0,
                },
            ))
        });
    }
}
