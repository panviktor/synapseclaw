//! Port: provider context engine.
//!
//! The domain owns the context-engine contract and normalized snapshot shape.
//! Adapters own concrete prompt-message conversion and provider-specific
//! assembly details.

use crate::application::services::model_lane_resolution::ResolvedModelProfile;
use crate::domain::message::ChatMessage;
use crate::ports::provider::ConversationMessage;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderPromptContextStats {
    pub system_messages: usize,
    pub system_chars: usize,
    pub bootstrap_chars: usize,
    pub core_memory_chars: usize,
    pub runtime_interpretation_chars: usize,
    pub scoped_context_chars: usize,
    pub resolution_chars: usize,
    pub dynamic_system_chars: usize,
    pub stable_system_chars: usize,
    pub prior_chat_messages: usize,
    pub prior_chat_chars: usize,
    pub current_turn_messages: usize,
    pub current_turn_chars: usize,
    pub total_messages: usize,
    pub total_chars: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderPromptSnapshot {
    pub messages: Vec<ChatMessage>,
    pub stats: ProviderPromptContextStats,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderPromptSnapshotInput<'a> {
    pub history: &'a [ConversationMessage],
    pub recent_chat_limit: usize,
    pub target_profile: Option<&'a ResolvedModelProfile>,
}

pub trait ContextEnginePort: Send + Sync {
    fn build_provider_prompt_snapshot(
        &self,
        input: ProviderPromptSnapshotInput<'_>,
    ) -> ProviderPromptSnapshot;
}
