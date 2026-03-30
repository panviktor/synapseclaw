//! Adapter: wraps existing `HookRunner` as HooksPort.

use async_trait::async_trait;
use std::sync::Arc;
use synapse_core::ports::hooks::{HookOutcome, HooksPort};

pub struct HookRunnerAdapter {
    runner: Arc<crate::hooks::HookRunner>,
}

impl HookRunnerAdapter {
    pub fn new(runner: Arc<crate::hooks::HookRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl HooksPort for HookRunnerAdapter {
    async fn on_message_received(
        &self,
        channel: &str,
        sender: &str,
        content: String,
    ) -> HookOutcome<String> {
        let msg = crate::channels::traits::ChannelMessage {
            id: uuid::Uuid::new_v4().to_string(),
            sender: sender.to_string(),
            reply_target: sender.to_string(),
            content,
            channel: channel.to_string(),
            #[allow(clippy::cast_sign_loss)]
            timestamp: chrono::Utc::now().timestamp() as u64,
            thread_ts: None,
        };

        match self.runner.run_on_message_received(msg).await {
            crate::hooks::HookResult::Continue(modified_msg) => {
                HookOutcome::Continue(modified_msg.content)
            }
            crate::hooks::HookResult::Cancel(reason) => HookOutcome::Cancel(reason),
        }
    }

    async fn on_message_sending(
        &self,
        channel: &str,
        recipient: &str,
        content: String,
    ) -> HookOutcome<String> {
        match self
            .runner
            .run_on_message_sending(channel.to_string(), recipient.to_string(), content)
            .await
        {
            crate::hooks::HookResult::Continue((_ch, _rcpt, modified_content)) => {
                HookOutcome::Continue(modified_content)
            }
            crate::hooks::HookResult::Cancel(reason) => HookOutcome::Cancel(reason),
        }
    }
}
