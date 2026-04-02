//! Adapter: wraps the existing `Mutex<HashMap<String, Vec<ChatMessage>>>` as ConversationHistoryPort.
//!
//! Also persists turns to the optional session backend so history
//! survives restarts.
//!
//! Since `providers::ChatMessage` is now a re-export of
//! `synapse_domain::domain::message::ChatMessage`, no conversions are needed.

use crate::session_backend::SessionBackend;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use synapse_domain::ports::conversation_history::ConversationHistoryPort;
use synapse_providers::ChatMessage;

/// Max history turns per sender (same as old MAX_CHANNEL_HISTORY).
const MAX_HISTORY: usize = 50;

pub struct MutexMapConversationHistory {
    map: Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>,
    session_store: Option<Arc<dyn SessionBackend>>,
}

impl MutexMapConversationHistory {
    pub fn new(
        map: Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>,
        session_store: Option<Arc<dyn SessionBackend>>,
    ) -> Self {
        Self { map, session_store }
    }
}

impl ConversationHistoryPort for MutexMapConversationHistory {
    fn has_history(&self, key: &str) -> bool {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_some_and(|v| !v.is_empty())
    }

    fn get_history(&self, key: &str) -> Vec<ChatMessage> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    fn append_turn(&self, key: &str, turn: ChatMessage) {
        // Persist to session store (survives restart)
        if let Some(ref store) = self.session_store {
            let _ = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(store.append(key, &turn))
            });
        }

        // Append to in-memory map
        let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let history = guard.entry(key.to_string()).or_default();
        history.push(turn);
        if history.len() > MAX_HISTORY {
            history.remove(0);
        }
    }

    fn clear_history(&self, key: &str) {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        // Note: session store is NOT cleared here — only in-memory.
        // Session files persist for session list/history views.
    }

    fn compact_history(&self, key: &str, keep_turns: usize) -> bool {
        let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(history) = guard.get_mut(key) {
            if history.len() > keep_turns {
                let drain_count = history.len() - keep_turns;
                history.drain(..drain_count);
                return true;
            }
        }
        false
    }

    fn rollback_last_turn(&self, key: &str, expected_content: &str) -> bool {
        // Rollback from session store
        if let Some(ref store) = self.session_store {
            let _ = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(store.remove_last(key))
            });
        }

        let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(history) = guard.get_mut(key) {
            if let Some(last) = history.last() {
                if last.content == expected_content {
                    history.pop();
                    return true;
                }
            }
        }
        false
    }

    fn prepend_turn(&self, key: &str, turn: ChatMessage) {
        let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
        guard.entry(key.to_string()).or_default().insert(0, turn);
    }
}
