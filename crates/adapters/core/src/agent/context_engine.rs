use crate::agent::dispatcher::ToolDispatcher;
use synapse_observability::ProviderContextStats;
use synapse_providers::{ChatMessage, ConversationMessage};

#[derive(Debug, Clone)]
pub(crate) struct ProviderPromptSnapshot {
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) stats: ProviderContextStats,
}

pub(crate) fn total_message_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(|msg| msg.content.chars().count()).sum()
}

pub(crate) fn build_provider_prompt_snapshot(
    dispatcher: &dyn ToolDispatcher,
    history: &[ConversationMessage],
    recent_chat_limit: usize,
) -> ProviderPromptSnapshot {
    let latest_user_index = history.iter().rposition(|msg| {
        matches!(
            msg,
            ConversationMessage::Chat(chat) if chat.role == "user"
        )
    });

    let system_messages: Vec<ChatMessage> = history
        .iter()
        .filter_map(|msg| match msg {
            ConversationMessage::Chat(chat) if chat.role == "system" => Some(chat.clone()),
            _ => None,
        })
        .collect();

    let (prefix, current_turn) = match latest_user_index {
        Some(index) => history.split_at(index),
        None => (history, &[][..]),
    };

    let recent_chat_context = prefix
        .iter()
        .filter_map(|msg| match msg {
            ConversationMessage::Chat(chat) if chat.role == "user" || chat.role == "assistant" => {
                Some(ConversationMessage::Chat(chat.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    let context_start = recent_chat_context.len().saturating_sub(recent_chat_limit);
    let prior_chat_messages =
        dispatcher.to_provider_messages(&recent_chat_context[context_start..]);
    let current_turn_messages = dispatcher.to_provider_messages(current_turn);

    let mut messages = Vec::with_capacity(
        system_messages.len() + prior_chat_messages.len() + current_turn_messages.len(),
    );
    messages.extend(system_messages.iter().cloned());
    messages.extend(prior_chat_messages.iter().cloned());
    messages.extend(current_turn_messages.iter().cloned());

    let stats = ProviderContextStats {
        system_messages: system_messages.len(),
        system_chars: total_message_chars(&system_messages),
        prior_chat_messages: prior_chat_messages.len(),
        prior_chat_chars: total_message_chars(&prior_chat_messages),
        current_turn_messages: current_turn_messages.len(),
        current_turn_chars: total_message_chars(&current_turn_messages),
        total_messages: messages.len(),
        total_chars: total_message_chars(&messages),
    };

    ProviderPromptSnapshot { messages, stats }
}
