use crate::agent::dispatcher::ToolDispatcher;
use synapse_domain::application::services::history_compaction;
use synapse_observability::ProviderContextStats;
use synapse_providers::{ChatMessage, ConversationMessage, ToolCall, ToolResultMessage};

const COMPACT_TOOL_RESULT_MAX_CHARS: usize = 320;
const PROVIDER_ASSISTANT_TEXT_MAX_CHARS: usize = 1_200;
const PROVIDER_TOOL_CALL_ARGS_MAX_CHARS: usize = 900;
const PROVIDER_TOOL_RESULT_MAX_CHARS: usize = 1_200;

#[derive(Debug, Clone)]
pub(crate) struct ProviderPromptSnapshot {
    pub(crate) messages: Vec<ChatMessage>,
    pub(crate) stats: ProviderContextStats,
}

pub(crate) fn total_message_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(|msg| msg.content.chars().count()).sum()
}

pub(crate) fn system_message_breakdown(history: &[ConversationMessage]) -> Vec<(String, usize)> {
    let mut breakdown = std::collections::BTreeMap::<String, usize>::new();
    for msg in history {
        let ConversationMessage::Chat(chat) = msg else {
            continue;
        };
        if chat.role != "system" {
            continue;
        }
        for (name, chars) in classify_system_message_sections(&chat.content) {
            *breakdown.entry(name.to_string()).or_default() += chars;
        }
    }
    breakdown.into_iter().collect()
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

    let latest_compaction_summary = recent_chat_context.iter().rev().find_map(|msg| match msg {
        ConversationMessage::Chat(chat)
            if chat.role == "assistant"
                && history_compaction::is_compaction_summary(&chat.content) =>
        {
            Some(ConversationMessage::Chat(chat.clone()))
        }
        _ => None,
    });

    let context_start = recent_chat_context.len().saturating_sub(recent_chat_limit);
    let mut prior_chat_context = recent_chat_context[context_start..].to_vec();
    if let Some(summary) = latest_compaction_summary {
        let already_present = prior_chat_context.iter().any(|message| {
            matches!(
                (message, &summary),
                (
                    ConversationMessage::Chat(left),
                    ConversationMessage::Chat(right)
                ) if left.role == right.role && left.content == right.content
            )
        });
        if !already_present {
            prior_chat_context.insert(0, summary);
        }
    }
    let prior_chat_messages = dispatcher.to_provider_messages(&prior_chat_context);
    let compacted_current_turn = compact_current_turn(current_turn);
    let bounded_current_turn = compacted_current_turn
        .iter()
        .map(sanitize_message_for_provider)
        .collect::<Vec<_>>();
    let current_turn_messages = dispatcher.to_provider_messages(&bounded_current_turn);

    let mut messages = Vec::with_capacity(
        system_messages.len() + prior_chat_messages.len() + current_turn_messages.len(),
    );
    messages.extend(system_messages.iter().cloned());
    messages.extend(prior_chat_messages.iter().cloned());
    messages.extend(current_turn_messages.iter().cloned());

    let system_breakdown = system_message_breakdown(history);
    let bootstrap_chars = lookup_section_chars(&system_breakdown, "bootstrap");
    let core_memory_chars = lookup_section_chars(&system_breakdown, "core_memory");
    let runtime_interpretation_chars =
        lookup_section_chars(&system_breakdown, "runtime_interpretation");
    let scoped_context_chars = lookup_section_chars(&system_breakdown, "scoped_context");
    let resolution_chars = lookup_section_chars(&system_breakdown, "resolution");
    let dynamic_system_chars =
        core_memory_chars + runtime_interpretation_chars + scoped_context_chars + resolution_chars;

    let stats = ProviderContextStats {
        system_messages: system_messages.len(),
        system_chars: total_message_chars(&system_messages),
        bootstrap_chars,
        core_memory_chars,
        runtime_interpretation_chars,
        scoped_context_chars,
        resolution_chars,
        dynamic_system_chars,
        stable_system_chars: bootstrap_chars,
        prior_chat_messages: prior_chat_messages.len(),
        prior_chat_chars: total_message_chars(&prior_chat_messages),
        current_turn_messages: current_turn_messages.len(),
        current_turn_chars: total_message_chars(&current_turn_messages),
        total_messages: messages.len(),
        total_chars: total_message_chars(&messages),
    };

    ProviderPromptSnapshot { messages, stats }
}

fn classify_system_message(content: &str) -> &'static str {
    if content.starts_with("[core-memory]\n") {
        "core_memory"
    } else if content.starts_with("[runtime-interpretation]") {
        "runtime_interpretation"
    } else if content.starts_with("[user-profile]") {
        "runtime_interpretation"
    } else if content.starts_with("[configured-runtime]") {
        "runtime_interpretation"
    } else if content.starts_with("[working-state]") {
        "runtime_interpretation"
    } else if content.starts_with("[current-conversation]") {
        "runtime_interpretation"
    } else if content.starts_with("[bounded-interpretation]") {
        "runtime_interpretation"
    } else if content.starts_with("[scoped-context]") {
        "scoped_context"
    } else if content.starts_with("[resolution-plan]") {
        "resolution_plan"
    } else if content.starts_with("[clarification-policy]") {
        "resolution"
    } else if content.starts_with("[execution-guidance]") {
        "resolution"
    } else {
        "bootstrap"
    }
}

fn classify_system_message_sections(content: &str) -> Vec<(&'static str, usize)> {
    let mut sections = Vec::new();
    let markers = [
        ("[core-memory]\n", "core_memory"),
        ("[runtime-interpretation]\n", "runtime_interpretation"),
        ("[scoped-context]\n", "scoped_context"),
        ("[resolution-plan]\n", "resolution"),
        ("[clarification-policy]\n", "resolution"),
        ("[execution-guidance]\n", "resolution"),
    ];

    let mut ranges = markers
        .iter()
        .filter_map(|(marker, name)| content.find(marker).map(|start| (start, *marker, *name)))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|(start, _, _)| *start);

    if ranges.is_empty() {
        return vec![(classify_system_message(content), content.chars().count())];
    }

    if let Some((first_start, _, _)) = ranges.first().copied() {
        if first_start > 0 {
            sections.push(("bootstrap", content[..first_start].chars().count()));
        }
    }

    for (idx, (start, marker, name)) in ranges.iter().enumerate() {
        let end = ranges
            .get(idx + 1)
            .map(|(next_start, _, _)| *next_start)
            .unwrap_or(content.len());
        let slice_start = *start;
        let slice = &content[slice_start..end];
        if !slice.is_empty() {
            let marker_chars = marker.chars().count();
            sections.push((*name, slice.chars().count().max(marker_chars)));
        }
    }

    sections
}

fn lookup_section_chars(breakdown: &[(String, usize)], section: &str) -> usize {
    breakdown
        .into_iter()
        .find_map(|(name, chars)| (name == section).then_some(*chars))
        .unwrap_or(0)
}

fn compact_current_turn(current_turn: &[ConversationMessage]) -> Vec<ConversationMessage> {
    let cycle_starts = current_turn
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| {
            matches!(msg, ConversationMessage::AssistantToolCalls { .. }).then_some(idx)
        })
        .collect::<Vec<_>>();

    let Some(raw_tail_start) = cycle_starts
        .len()
        .checked_sub(2)
        .and_then(|idx| cycle_starts.get(idx).copied())
    else {
        return current_turn.to_vec();
    };

    // Nothing to compact if the current turn only contains the preserved raw tail.
    if raw_tail_start <= 1 {
        return current_turn.to_vec();
    }

    let mut compacted = Vec::with_capacity(current_turn.len() - raw_tail_start + 2);
    compacted.push(current_turn[0].clone());

    if let Some(summary) = summarize_completed_turn_work(&current_turn[1..raw_tail_start]) {
        compacted.push(ConversationMessage::Chat(ChatMessage::system(summary)));
    }

    compacted.extend_from_slice(&current_turn[raw_tail_start..]);
    compacted
}

fn sanitize_message_for_provider(message: &ConversationMessage) -> ConversationMessage {
    match message {
        ConversationMessage::Chat(chat) if chat.role == "assistant" => {
            ConversationMessage::Chat(ChatMessage {
                role: chat.role.clone(),
                content: truncate_with_head_tail(&chat.content, PROVIDER_ASSISTANT_TEXT_MAX_CHARS),
            })
        }
        ConversationMessage::AssistantToolCalls {
            text,
            tool_calls,
            reasoning_content,
        } => ConversationMessage::AssistantToolCalls {
            text: text
                .as_ref()
                .map(|value| truncate_with_head_tail(value, PROVIDER_ASSISTANT_TEXT_MAX_CHARS)),
            tool_calls: tool_calls
                .iter()
                .map(|call| ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: truncate_with_head_tail(
                        &call.arguments,
                        PROVIDER_TOOL_CALL_ARGS_MAX_CHARS,
                    ),
                })
                .collect(),
            reasoning_content: reasoning_content
                .as_ref()
                .map(|value| truncate_with_head_tail(value, PROVIDER_ASSISTANT_TEXT_MAX_CHARS)),
        },
        ConversationMessage::ToolResults(results) => ConversationMessage::ToolResults(
            results
                .iter()
                .map(|result| ToolResultMessage {
                    tool_call_id: result.tool_call_id.clone(),
                    content: truncate_with_head_tail(
                        &result.content,
                        PROVIDER_TOOL_RESULT_MAX_CHARS,
                    ),
                })
                .collect(),
        ),
        _ => message.clone(),
    }
}

fn summarize_completed_turn_work(messages: &[ConversationMessage]) -> Option<String> {
    let mut lines = vec!["[completed-turn-work]".to_string()];
    let mut has_content = false;

    for message in messages {
        match message {
            ConversationMessage::Chat(chat) if chat.role == "assistant" => {
                let content = truncate_chars(chat.content.trim(), COMPACT_TOOL_RESULT_MAX_CHARS);
                if !content.is_empty() {
                    lines.push(format!("- assistant: {content}"));
                    has_content = true;
                }
            }
            ConversationMessage::AssistantToolCalls {
                tool_calls, text, ..
            } => {
                if let Some(note) = text.as_deref() {
                    let content = truncate_chars(note.trim(), COMPACT_TOOL_RESULT_MAX_CHARS);
                    if !content.is_empty() {
                        lines.push(format!("- assistant_note: {content}"));
                        has_content = true;
                    }
                }
                for call in tool_calls {
                    lines.push(format!(
                        "- tool_call {}: {}",
                        call.id,
                        summarize_tool_call(call)
                    ));
                    has_content = true;
                }
            }
            ConversationMessage::ToolResults(results) => {
                for result in results {
                    lines.push(format!(
                        "- tool_result {}: {}",
                        result.tool_call_id,
                        truncate_chars(result.content.trim(), COMPACT_TOOL_RESULT_MAX_CHARS)
                    ));
                    has_content = true;
                }
            }
            _ => {}
        }
    }

    has_content.then(|| format!("{}\n", lines.join("\n")))
}

fn summarize_tool_call(call: &ToolCall) -> String {
    let arguments = truncate_chars(call.arguments.trim(), COMPACT_TOOL_RESULT_MAX_CHARS);
    if arguments.is_empty() {
        call.name.clone()
    } else {
        format!("{} {}", call.name, arguments)
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let truncated = value
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| &value[..idx])
        .unwrap_or(value);
    format!("{truncated}...")
}

fn truncate_with_head_tail(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_string();
    }

    let head_chars = ((max_chars * 2) / 3).max(1);
    let tail_chars = max_chars.saturating_sub(head_chars + 1).max(1);

    let head = slice_first_chars(value, head_chars);
    let tail = slice_last_chars(value, tail_chars);
    format!("{head}…{tail}")
}

fn slice_first_chars(value: &str, count: usize) -> &str {
    value
        .char_indices()
        .nth(count)
        .map(|(idx, _)| &value[..idx])
        .unwrap_or(value)
}

fn slice_last_chars(value: &str, count: usize) -> &str {
    let total = value.chars().count();
    if count >= total {
        return value;
    }

    let skip = total - count;
    value
        .char_indices()
        .nth(skip)
        .map(|(idx, _)| &value[idx..])
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::dispatcher::NativeToolDispatcher;
    use synapse_domain::application::services::history_compaction::COMPACTION_SUMMARY_PREFIX;
    use synapse_providers::ToolResultMessage;

    fn tool_call(id: &str, name: &str) -> ConversationMessage {
        ConversationMessage::AssistantToolCalls {
            text: None,
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: "{\"path\":\"/tmp/demo\"}".to_string(),
            }],
            reasoning_content: None,
        }
    }

    fn tool_result(id: &str, content: &str) -> ConversationMessage {
        ConversationMessage::ToolResults(vec![ToolResultMessage {
            tool_call_id: id.to_string(),
            content: content.to_string(),
        }])
    }

    #[test]
    fn compacts_completed_tool_cycles_but_keeps_latest_cycle_raw() {
        let history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::user("send the report")),
            tool_call("call-1", "file_read"),
            tool_result("call-1", "first tool output"),
            tool_call("call-2", "message_send"),
            tool_result("call-2", "second tool output"),
            tool_call("call-3", "memory_store"),
            tool_result("call-3", "third tool output"),
        ];

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| format!("{}:{}", msg.role, msg.content))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("[completed-turn-work]"));
        assert!(rendered.contains("tool_call call-1: file_read"));
        assert!(rendered.contains("tool_result call-1: first tool output"));
        assert!(rendered.contains("call-2"));
        assert!(rendered.contains("second tool output"));
        assert!(rendered.contains("call-3"));
        assert!(rendered.contains("third tool output"));
        assert!(snapshot.stats.current_turn_messages < 5);
    }

    #[test]
    fn leaves_single_tool_cycle_uncompacted() {
        let history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::user("send the report")),
            tool_call("call-1", "message_send"),
            tool_result("call-1", "done"),
        ];

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("[completed-turn-work]"));
        assert!(rendered.contains("call-1"));
        assert!(rendered.contains("done"));
    }

    #[test]
    fn bounds_large_tool_results_in_provider_snapshot() {
        let long_output = "x".repeat(8_000);
        let history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::user("inspect output")),
            tool_call("call-1", "shell"),
            tool_result("call-1", &long_output),
        ];

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("tool_call_id"));
        assert!(rendered.contains("…"));
        assert!(snapshot.stats.current_turn_chars < 3_000);
    }

    #[test]
    fn preserves_full_audit_history_when_sanitizing_provider_snapshot() {
        let long_output = "tail-critical-error".repeat(200);
        let raw = tool_result("call-1", &long_output);
        let sanitized = sanitize_message_for_provider(&raw);

        match sanitized {
            ConversationMessage::ToolResults(results) => {
                assert_eq!(results.len(), 1);
                assert!(results[0].content.contains("…"));
                assert!(results[0].content.contains("tail-critical-error"));
            }
            _ => panic!("expected sanitized tool results"),
        }

        match raw {
            ConversationMessage::ToolResults(results) => {
                assert_eq!(results[0].content, long_output);
            }
            _ => panic!("expected raw tool results"),
        }
    }

    #[test]
    fn preserves_latest_compaction_summary_outside_recent_chat_window() {
        let mut history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::assistant(format!(
                "{COMPACTION_SUMMARY_PREFIX}- old decision\n- old preference"
            ))),
        ];

        for idx in 0..8 {
            history.push(ConversationMessage::Chat(ChatMessage::user(format!(
                "user {idx}"
            ))));
            history.push(ConversationMessage::Chat(ChatMessage::assistant(format!(
                "assistant {idx}"
            ))));
        }

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(COMPACTION_SUMMARY_PREFIX));
        assert!(rendered.contains("assistant 7"));
    }
}
