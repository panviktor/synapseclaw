//! Port: hooks — message lifecycle hooks.
//!
//! Abstracts the hook runner so the application core can execute
//! hooks without depending on the concrete hook implementation.

use anyhow::Result;
use async_trait::async_trait;

/// Result of a hook execution.
#[derive(Debug, Clone)]
pub enum HookOutcome<T> {
    /// Continue processing with (possibly modified) value.
    Continue(T),
    /// Cancel processing with a reason.
    Cancel(String),
}

/// Port for executing message lifecycle hooks.
#[async_trait]
pub trait HooksPort: Send + Sync {
    /// Run on_message_received hook.
    /// Returns modified content or cancellation.
    async fn on_message_received(
        &self,
        channel: &str,
        sender: &str,
        content: String,
    ) -> HookOutcome<String>;

    /// Run on_message_sending hook.
    /// Returns modified response or cancellation.
    async fn on_message_sending(
        &self,
        channel: &str,
        recipient: &str,
        content: String,
    ) -> HookOutcome<String>;
}

/// No-op implementation for when hooks are not configured.
pub struct NoOpHooks;

#[async_trait]
impl HooksPort for NoOpHooks {
    async fn on_message_received(
        &self,
        _channel: &str,
        _sender: &str,
        content: String,
    ) -> HookOutcome<String> {
        HookOutcome::Continue(content)
    }

    async fn on_message_sending(
        &self,
        _channel: &str,
        _recipient: &str,
        content: String,
    ) -> HookOutcome<String> {
        HookOutcome::Continue(content)
    }
}
