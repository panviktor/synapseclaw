//! Adapter: wraps the existing `Mutex<HashMap<String, Vec<ChatMessage>>>` as ConversationHistoryPort.
//!
//! Also persists turns to the optional session backend so history
//! survives restarts.
//!
//! Since `providers::ChatMessage` is now a re-export of
//! `synapse_domain::domain::message::ChatMessage`, no conversions are needed.

use crate::session_backend::SessionBackend;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use synapse_domain::application::services::history_compaction::compact_provider_history_for_session_hygiene;
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

fn block_on_session_backend<T>(
    future: impl Future<Output = std::io::Result<T>> + Send,
) -> std::io::Result<T>
where
    T: Send,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            return tokio::task::block_in_place(|| handle.block_on(future));
        }

        return std::thread::scope(|scope| {
            scope
                .spawn(move || run_session_backend_future(future))
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("session backend worker panicked")))
        });
    }

    run_session_backend_future(future)
}

fn run_session_backend_future<T>(
    future: impl Future<Output = std::io::Result<T>>,
) -> std::io::Result<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(std::io::Error::other)?
        .block_on(future)
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
            let _ = block_on_session_backend(store.append(key, &turn));
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
        if let Some(ref store) = self.session_store {
            let _ = block_on_session_backend(store.delete(key));
        }
    }

    fn compact_history(&self, key: &str, keep_turns: usize) -> bool {
        let compacted_history = {
            let mut guard = self.map.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(history) = guard.get_mut(key) {
                if compact_provider_history_for_session_hygiene(history, keep_turns) {
                    Some(history.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(history) = compacted_history {
            if let Some(ref store) = self.session_store {
                let _ = block_on_session_backend(store.replace(key, &history));
            }
            return true;
        }
        false
    }

    fn rollback_last_turn(&self, key: &str, expected_content: &str) -> bool {
        // Rollback from session store
        if let Some(ref store) = self.session_store {
            let _ = block_on_session_backend(store.remove_last(key));
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[derive(Default)]
    struct RecordingSessionBackend {
        messages: Mutex<HashMap<String, Vec<ChatMessage>>>,
    }

    impl RecordingSessionBackend {
        fn snapshot(&self, session_key: &str) -> Vec<ChatMessage> {
            self.messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(session_key)
                .cloned()
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl SessionBackend for RecordingSessionBackend {
        async fn load(&self, session_key: &str) -> Vec<ChatMessage> {
            self.snapshot(session_key)
        }

        async fn append(&self, session_key: &str, message: &ChatMessage) -> std::io::Result<()> {
            self.messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(session_key.to_string())
                .or_default()
                .push(message.clone());
            Ok(())
        }

        async fn remove_last(&self, session_key: &str) -> std::io::Result<bool> {
            let mut guard = self.messages.lock().unwrap_or_else(|e| e.into_inner());
            let Some(messages) = guard.get_mut(session_key) else {
                return Ok(false);
            };
            Ok(messages.pop().is_some())
        }

        async fn replace(
            &self,
            session_key: &str,
            messages: &[ChatMessage],
        ) -> std::io::Result<()> {
            self.messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(session_key.to_string(), messages.to_vec());
            Ok(())
        }

        async fn list_sessions(&self) -> Vec<String> {
            self.messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .keys()
                .cloned()
                .collect()
        }

        async fn delete(&self, session_key: &str) -> std::io::Result<bool> {
            Ok(self
                .messages
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(session_key)
                .is_some())
        }
    }

    fn make_history(store: Arc<RecordingSessionBackend>) -> MutexMapConversationHistory {
        MutexMapConversationHistory::new(
            Arc::new(Mutex::new(HashMap::new())),
            Some(store as Arc<dyn SessionBackend>),
        )
    }

    #[test]
    fn append_turn_persists_without_tokio_runtime() {
        let store = Arc::new(RecordingSessionBackend::default());
        let history = make_history(Arc::clone(&store));

        history.append_turn("session", ChatMessage::user("hello"));

        let persisted = store.snapshot("session");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].role, "user");
        assert_eq!(persisted[0].content, "hello");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn append_turn_persists_from_current_thread_runtime() {
        let store = Arc::new(RecordingSessionBackend::default());
        let history = make_history(Arc::clone(&store));

        history.append_turn("session", ChatMessage::assistant("hi"));

        let persisted = store.snapshot("session");
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].role, "assistant");
        assert_eq!(persisted[0].content, "hi");
    }

    #[test]
    fn compact_history_rewrites_session_store_with_role_aligned_history() {
        let store = Arc::new(RecordingSessionBackend::default());
        let history = make_history(Arc::clone(&store));

        history.append_turn("session", ChatMessage::system("bootstrap"));
        for idx in 0..6 {
            history.append_turn("session", ChatMessage::user(format!("user {idx}")));
            history.append_turn(
                "session",
                ChatMessage::assistant(format!("assistant {idx}")),
            );
        }
        history.append_turn("session", ChatMessage::user("current"));

        assert!(history.compact_history("session", 8));

        let persisted = store.snapshot("session");
        let first_non_system = persisted
            .iter()
            .find(|message| message.role != "system")
            .expect("compacted history should keep non-system turns");
        assert_eq!(first_non_system.role, "user");
        assert_eq!(
            persisted.last().map(|message| message.content.as_str()),
            Some("current")
        );
    }
}
