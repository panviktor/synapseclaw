//! Port for accessing the current conversation context during tool execution.
//!
//! Set by the inbound message handler before the agent turn starts;
//! read by tools that need "here" (message_send, cron with current_conversation).
//! Cleared after the turn completes.

use crate::domain::conversation_target::CurrentConversationContext;

/// Thread-safe access to the current conversation context.
///
/// Implementations should use `Arc<RwLock<Option<...>>>` or similar.
/// The context is scoped to a single agent turn — set before, cleared after.
pub trait ConversationContextPort: Send + Sync {
    /// Get the current conversation context (if any).
    fn get_current(&self) -> Option<CurrentConversationContext>;

    /// Set the current conversation context (called at turn start).
    fn set_current(&self, ctx: Option<CurrentConversationContext>);
}

/// Simple in-memory implementation using parking_lot RwLock.
pub struct InMemoryConversationContext {
    inner: parking_lot::RwLock<Option<CurrentConversationContext>>,
}

impl InMemoryConversationContext {
    pub fn new() -> Self {
        Self {
            inner: parking_lot::RwLock::new(None),
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
        self.inner.read().clone()
    }

    fn set_current(&self, ctx: Option<CurrentConversationContext>) {
        *self.inner.write() = ctx;
    }
}
