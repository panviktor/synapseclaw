//! Adapter: wraps existing `HookRunner` as HooksPort.

use async_trait::async_trait;
use fork_core::ports::hooks::{HookOutcome, HooksPort};
use std::sync::Arc;

pub struct HookRunnerAdapter {
    runner: Arc<crate::fork_adapters::hooks::HookRunner>,
}

impl HookRunnerAdapter {
    pub fn new(runner: Arc<crate::fork_adapters::hooks::HookRunner>) -> Self {
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
        let msg = crate::fork_adapters::channels::traits::ChannelMessage {
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
            crate::fork_adapters::hooks::HookResult::Continue(modified_msg) => {
                HookOutcome::Continue(modified_msg.content)
            }
            crate::fork_adapters::hooks::HookResult::Cancel(reason) => HookOutcome::Cancel(reason),
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
            crate::fork_adapters::hooks::HookResult::Continue((_ch, _rcpt, modified_content)) => {
                HookOutcome::Continue(modified_content)
            }
            crate::fork_adapters::hooks::HookResult::Cancel(reason) => HookOutcome::Cancel(reason),
        }
    }
}
