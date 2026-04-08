//! Port for accessing the current conversation context during tool execution.
//!
//! Set by the inbound message handler before the agent turn starts;
//! read by tools that need "here" (message_send, cron with current_conversation).
//! Cleared after the turn completes.

use crate::domain::conversation_target::CurrentConversationContext;
use std::collections::HashMap;

/// Thread-safe access to the current conversation context.
///
/// The context is scoped to a single agent turn — set before, cleared after.
/// Implementations should isolate concurrent turns from each other.
pub trait ConversationContextPort: Send + Sync {
    /// Get the current conversation context (if any).
    fn get_current(&self) -> Option<CurrentConversationContext>;

    /// Set the current conversation context (called at turn start).
    fn set_current(&self, ctx: Option<CurrentConversationContext>);
}

/// In-memory implementation with task-local scoping for concurrent turns.
pub struct InMemoryConversationContext {
    by_task: parking_lot::RwLock<HashMap<tokio::task::Id, CurrentConversationContext>>,
    fallback: parking_lot::RwLock<Option<CurrentConversationContext>>,
}

impl InMemoryConversationContext {
    pub fn new() -> Self {
        Self {
            by_task: parking_lot::RwLock::new(HashMap::new()),
            fallback: parking_lot::RwLock::new(None),
        }
    }
}

impl Default for InMemoryConversationContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationContextPort for InMemoryConversationContext {
    fn get_current(&self) -> Option<CurrentConversationContext> {
        if let Some(task_id) = tokio::task::try_id() {
            if let Some(ctx) = self.by_task.read().get(&task_id) {
                return Some(ctx.clone());
            }
        }
        self.fallback.read().clone()
    }

    fn set_current(&self, ctx: Option<CurrentConversationContext>) {
        if let Some(task_id) = tokio::task::try_id() {
            let mut by_task = self.by_task.write();
            if let Some(ctx) = ctx {
                by_task.insert(task_id, ctx);
            } else {
                by_task.remove(&task_id);
            }
            return;
        }

        *self.fallback.write() = ctx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(label: &str) -> CurrentConversationContext {
        CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_ref: label.into(),
            reply_ref: format!("!{label}:example.com"),
            thread_ref: Some(format!("${label}")),
            actor_id: "user".into(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn isolates_context_per_async_task() {
        let port = std::sync::Arc::new(InMemoryConversationContext::new());

        let left = {
            let port = std::sync::Arc::clone(&port);
            tokio::spawn(async move {
                port.set_current(Some(make_ctx("alpha")));
                tokio::task::yield_now().await;
                port.get_current().map(|ctx| ctx.conversation_ref)
            })
        };

        let right = {
            let port = std::sync::Arc::clone(&port);
            tokio::spawn(async move {
                port.set_current(Some(make_ctx("beta")));
                tokio::task::yield_now().await;
                port.get_current().map(|ctx| ctx.conversation_ref)
            })
        };

        assert_eq!(left.await.unwrap().as_deref(), Some("alpha"));
        assert_eq!(right.await.unwrap().as_deref(), Some("beta"));
    }

    #[test]
    fn fallback_context_works_outside_tokio() {
        let port = InMemoryConversationContext::new();
        port.set_current(Some(make_ctx("sync")));
        assert_eq!(
            port.get_current().map(|ctx| ctx.conversation_ref),
            Some("sync".to_string())
        );
        port.set_current(None);
        assert!(port.get_current().is_none());
    }
}
