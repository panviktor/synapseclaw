//! Adapter: wraps the existing `Mutex<HashMap<String, Vec<ChatMessage>>>` as ConversationHistoryPort.
//!
//! Also persists turns to the optional SessionStore (JSONL) so history
//! survives restarts.
//!
//! The internal map and session store use the upstream `providers::ChatMessage`.
//! The port trait expects `fork_core::domain::message::ChatMessage`.
//! Conversions happen at the trait boundary using helpers from `fork_adapters`.

use crate::fork_adapters::channels::session_store::SessionStore;
use crate::fork_adapters::providers::ChatMessage;
use crate::fork_adapters::{from_core_message, to_core_message};
use crate::fork_core::domain::message::ChatMessage as CoreChatMessage;
use crate::fork_core::ports::conversation_history::ConversationHistoryPort;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Max history turns per sender (same as old MAX_CHANNEL_HISTORY).
const MAX_HISTORY: usize = 50;

pub struct MutexMapConversationHistory {
    map: Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>,
    session_store: Option<Arc<SessionStore>>,
}

impl MutexMapConversationHistory {
    pub fn new(
        map: Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>,
        session_store: Option<Arc<SessionStore>>,
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

    fn get_history(&self, key: &str) -> Vec<CoreChatMessage> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .map(|v| v.iter().map(to_core_message).collect())
            .unwrap_or_default()
    }

    fn append_turn(&self, key: &str, turn: CoreChatMessage) {
        let provider_msg = from_core_message(&turn);

        // Persist to JSONL session store (survives restart)
        if let Some(ref store) = self.session_store {
            let _ = store.append(key, &provider_msg);
        }

        // Append to in-memory map
        let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let history = guard.entry(key.to_string()).or_default();
        history.push(provider_msg);
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
            let _ = store.remove_last(key);
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

    fn prepend_turn(&self, key: &str, turn: CoreChatMessage) {
        let provider_msg = from_core_message(&turn);
        let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .entry(key.to_string())
            .or_default()
            .insert(0, provider_msg);
    }
}
