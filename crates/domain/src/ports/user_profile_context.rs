//! Port for accessing the current user-profile key during tool execution.
//!
//! This mirrors `ConversationContextPort`, but resolves the durable user-profile
//! identity rather than the current conversation target.

use std::collections::HashMap;

/// Thread-safe access to the current user-profile key.
pub trait UserProfileContextPort: Send + Sync {
    /// Get the current user-profile key (if any).
    fn get_current_key(&self) -> Option<String>;

    /// Set the current user-profile key (called before a turn starts).
    fn set_current_key(&self, key: Option<String>);
}

/// In-memory implementation with task-local scoping for concurrent turns.
pub struct InMemoryUserProfileContext {
    by_task: parking_lot::RwLock<HashMap<tokio::task::Id, String>>,
    fallback: parking_lot::RwLock<Option<String>>,
}

impl InMemoryUserProfileContext {
    pub fn new() -> Self {
        Self {
            by_task: parking_lot::RwLock::new(HashMap::new()),
            fallback: parking_lot::RwLock::new(None),
        }
    }
}

impl Default for InMemoryUserProfileContext {
    fn default() -> Self {
        Self::new()
    }
}

impl UserProfileContextPort for InMemoryUserProfileContext {
    fn get_current_key(&self) -> Option<String> {
        if let Some(task_id) = tokio::task::try_id() {
            if let Some(key) = self.by_task.read().get(&task_id) {
                return Some(key.clone());
            }
        }
        self.fallback.read().clone()
    }

    fn set_current_key(&self, key: Option<String>) {
        if let Some(task_id) = tokio::task::try_id() {
            let mut by_task = self.by_task.write();
            if let Some(key) = key {
                by_task.insert(task_id, key);
            } else {
                by_task.remove(&task_id);
            }
            return;
        }

        *self.fallback.write() = key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn isolates_profile_key_per_async_task() {
        let port = std::sync::Arc::new(InMemoryUserProfileContext::new());

        let left = {
            let port = std::sync::Arc::clone(&port);
            tokio::spawn(async move {
                port.set_current_key(Some("channel:matrix:alice".into()));
                tokio::task::yield_now().await;
                port.get_current_key()
            })
        };

        let right = {
            let port = std::sync::Arc::clone(&port);
            tokio::spawn(async move {
                port.set_current_key(Some("channel:telegram:bob".into()));
                tokio::task::yield_now().await;
                port.get_current_key()
            })
        };

        assert_eq!(left.await.unwrap().as_deref(), Some("channel:matrix:alice"));
        assert_eq!(
            right.await.unwrap().as_deref(),
            Some("channel:telegram:bob")
        );
    }

    #[test]
    fn fallback_key_works_outside_tokio() {
        let port = InMemoryUserProfileContext::new();
        port.set_current_key(Some("web:abc".into()));
        assert_eq!(port.get_current_key().as_deref(), Some("web:abc"));
        port.set_current_key(None);
        assert!(port.get_current_key().is_none());
    }
}
