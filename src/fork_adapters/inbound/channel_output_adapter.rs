//! Adapter: wraps `Arc<dyn Channel>` as ChannelOutputPort.

use crate::fork_adapters::channels::traits::{Channel, SendMessage};
use crate::fork_core::ports::channel_output::ChannelOutputPort;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

pub struct ChannelOutputAdapter {
    channel: Arc<dyn Channel>,
}

impl ChannelOutputAdapter {
    pub fn new(channel: Arc<dyn Channel>) -> Self {
        Self { channel }
    }
}

#[async_trait]
impl ChannelOutputPort for ChannelOutputAdapter {
    async fn send_message(
        &self,
        recipient: &str,
        text: &str,
        thread_ref: Option<&str>,
    ) -> Result<()> {
        let mut msg = SendMessage::new(text, recipient);
        if let Some(tr) = thread_ref {
            msg = msg.in_thread(Some(tr.to_string()));
        }
        self.channel.send(&msg).await
    }

    async fn start_typing(&self, recipient: &str) -> Result<()> {
        self.channel.start_typing(recipient).await
    }

    async fn stop_typing(&self, recipient: &str) -> Result<()> {
        self.channel.stop_typing(recipient).await
    }

    async fn add_reaction(&self, recipient: &str, message_id: &str, emoji: &str) -> Result<()> {
        self.channel
            .add_reaction(recipient, message_id, emoji)
            .await
    }

    async fn remove_reaction(&self, recipient: &str, message_id: &str, emoji: &str) -> Result<()> {
        self.channel
            .remove_reaction(recipient, message_id, emoji)
            .await
    }

    async fn fetch_message_text(&self, message_id: &str) -> Result<Option<String>> {
        self.channel.fetch_message(message_id).await
    }

    fn supports_streaming(&self) -> bool {
        self.channel.supports_draft_updates()
    }

    async fn send_draft(
        &self,
        recipient: &str,
        text: &str,
        thread_ref: Option<&str>,
    ) -> Result<Option<String>> {
        let mut msg = SendMessage::new(text, recipient);
        if let Some(tr) = thread_ref {
            msg = msg.in_thread(Some(tr.to_string()));
        }
        self.channel.send_draft(&msg).await
    }

    async fn update_draft(&self, recipient: &str, draft_id: &str, text: &str) -> Result<()> {
        self.channel.update_draft(recipient, draft_id, text).await
    }

    async fn finalize_draft(&self, recipient: &str, draft_id: &str, text: &str) -> Result<()> {
        self.channel.finalize_draft(recipient, draft_id, text).await
    }

    async fn cancel_draft(&self, recipient: &str, draft_id: &str) -> Result<()> {
        self.channel.cancel_draft(recipient, draft_id).await
    }
}
