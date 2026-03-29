//! Port: channel output — send messages, typing indicators, reactions, drafts.
//!
//! Abstracts the outbound channel operations so the application core
//! can orchestrate responses without depending on concrete channel adapters.

use anyhow::Result;
use async_trait::async_trait;

/// Port for sending output to a channel.
#[async_trait]
pub trait ChannelOutputPort: Send + Sync {
    /// Send a text message to the recipient.
    async fn send_message(
        &self,
        recipient: &str,
        text: &str,
        thread_ref: Option<&str>,
    ) -> Result<()>;

    /// Start typing indicator.
    async fn start_typing(&self, recipient: &str) -> Result<()>;

    /// Stop typing indicator.
    async fn stop_typing(&self, recipient: &str) -> Result<()>;

    /// Add reaction to a message.
    async fn add_reaction(&self, recipient: &str, message_id: &str, emoji: &str) -> Result<()>;

    /// Remove reaction from a message.
    async fn remove_reaction(&self, recipient: &str, message_id: &str, emoji: &str) -> Result<()>;

    /// Fetch a message's text content by ID (for thread seeding).
    async fn fetch_message_text(&self, message_id: &str) -> Result<Option<String>>;

    /// Whether this channel supports streaming draft updates.
    fn supports_streaming(&self) -> bool;

    /// Send an initial draft message. Returns draft ID if supported.
    async fn send_draft(
        &self,
        recipient: &str,
        text: &str,
        thread_ref: Option<&str>,
    ) -> Result<Option<String>> {
        let _ = (recipient, text, thread_ref);
        Ok(None)
    }

    /// Update an existing draft message.
    async fn update_draft(&self, recipient: &str, draft_id: &str, text: &str) -> Result<()> {
        let _ = (recipient, draft_id, text);
        Ok(())
    }

    /// Finalize a draft (replace with final text).
    async fn finalize_draft(&self, recipient: &str, draft_id: &str, text: &str) -> Result<()> {
        let _ = (recipient, draft_id, text);
        Ok(())
    }

    /// Cancel a draft.
    async fn cancel_draft(&self, recipient: &str, draft_id: &str) -> Result<()> {
        let _ = (recipient, draft_id);
        Ok(())
    }
}
