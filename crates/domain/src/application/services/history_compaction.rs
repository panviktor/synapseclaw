//! History compaction — domain policy for managing conversation history size.
//!
//! Pure business logic for trimming and compacting conversation history.
//! The summarization step requires calling an LLM provider, so callers
//! pass a callback for that operation.

use crate::domain::message::ChatMessage;
use crate::domain::util::truncate_with_ellipsis;
use std::fmt::Write;

/// Default trigger for auto-compaction when non-system message count exceeds this threshold.
pub const DEFAULT_MAX_HISTORY_MESSAGES: usize = 50;

/// Keep this many most-recent non-system messages after compaction.
const COMPACTION_KEEP_RECENT_MESSAGES: usize = 20;

/// Safety cap for compaction source transcript passed to the summarizer.
const COMPACTION_MAX_SOURCE_CHARS: usize = 12_000;

/// Max characters retained in stored compaction summary.
const COMPACTION_MAX_SUMMARY_CHARS: usize = 2_000;

/// Prefix used for compacted conversation summaries stored back into provider history.
pub const COMPACTION_SUMMARY_PREFIX: &str = "[Compaction summary]\n";

/// Estimate token count for a message history using ~4 chars/token heuristic.
/// Includes a small overhead per message for role/framing tokens.
pub fn estimate_history_tokens(history: &[ChatMessage]) -> usize {
    history
        .iter()
        .map(|m| {
            // ~4 chars per token + ~4 framing tokens per message (role, delimiters)
            m.content.len().div_ceil(4) + 4
        })
        .sum()
}

/// Trim conversation history to prevent unbounded growth.
/// Preserves the system prompt (first message if role=system) and the most recent messages.
pub fn trim_history(history: &mut Vec<ChatMessage>, max_history: usize) {
    let has_system = history.first().is_some_and(|m| m.role == "system");
    let non_system_count = if has_system {
        history.len() - 1
    } else {
        history.len()
    };

    if non_system_count <= max_history {
        return;
    }

    let start = if has_system { 1 } else { 0 };
    let to_remove = non_system_count - max_history;
    history.drain(start..start + to_remove);

    // Safety: remove orphan tool_result messages left after trim.
    // OpenAI-compatible APIs reject tool_result without preceding tool_call.
    while history.len() > start {
        let is_orphan_tool = history[start].role == "tool"
            || (history[start].role == "assistant"
                && history[start].content.contains("<tool_result>"));
        if is_orphan_tool {
            history.remove(start);
        } else {
            break;
        }
    }
}

fn build_compaction_transcript(messages: &[ChatMessage]) -> String {
    let mut transcript = String::new();
    for msg in messages {
        let role = msg.role.to_uppercase();
        let _ = writeln!(transcript, "{role}: {}", msg.content.trim());
    }

    if transcript.chars().count() > COMPACTION_MAX_SOURCE_CHARS {
        truncate_with_ellipsis(&transcript, COMPACTION_MAX_SOURCE_CHARS)
    } else {
        transcript
    }
}

fn apply_compaction_summary(
    history: &mut Vec<ChatMessage>,
    start: usize,
    compact_end: usize,
    summary: &str,
) {
    let summary_msg =
        ChatMessage::assistant(format!("{COMPACTION_SUMMARY_PREFIX}{}", summary.trim()));
    history.splice(start..compact_end, std::iter::once(summary_msg));
}

/// Returns true when the message content is a stored compaction summary.
pub fn is_compaction_summary(content: &str) -> bool {
    content.starts_with(COMPACTION_SUMMARY_PREFIX)
}

/// Determine the compaction range and build the transcript to summarize.
///
/// Returns `None` if no compaction is needed.
/// Returns `Some((start, compact_end, transcript))` with the range to compact
/// and the formatted transcript for summarization.
pub fn prepare_compaction(
    history: &[ChatMessage],
    max_history: usize,
    max_context_tokens: usize,
) -> Option<(usize, usize, String)> {
    let has_system = history.first().is_some_and(|m| m.role == "system");
    let non_system_count = if has_system {
        history.len().saturating_sub(1)
    } else {
        history.len()
    };

    let estimated_tokens = estimate_history_tokens(history);

    // Trigger compaction when either token budget OR message count is exceeded.
    if estimated_tokens <= max_context_tokens && non_system_count <= max_history {
        return None;
    }

    let start = if has_system { 1 } else { 0 };
    let keep_recent = COMPACTION_KEEP_RECENT_MESSAGES.min(non_system_count);
    let compact_count = non_system_count.saturating_sub(keep_recent);
    if compact_count == 0 {
        return None;
    }

    let mut compact_end = start + compact_count;

    // Snap compact_end to a user-turn boundary so we don't split mid-conversation.
    while compact_end > start && history.get(compact_end).is_some_and(|m| m.role != "user") {
        compact_end -= 1;
    }
    if compact_end <= start {
        return None;
    }

    let to_compact: Vec<ChatMessage> = history[start..compact_end].to_vec();
    let transcript = build_compaction_transcript(&to_compact);

    Some((start, compact_end, transcript))
}

/// The system prompt for the summarizer LLM call.
pub const COMPACTION_SUMMARIZER_SYSTEM: &str = "You are a conversation compaction engine. Summarize older chat history into concise context for future turns. Preserve: user preferences, commitments, decisions, unresolved tasks, key facts. Omit: filler, repeated chit-chat, verbose tool logs. Output plain text bullet points only.";

/// Build the user-facing prompt for the summarizer.
pub fn compaction_summarizer_prompt(transcript: &str) -> String {
    format!(
        "Summarize the following conversation history for context preservation. Keep it short (max 12 bullet points).\n\n{}",
        transcript
    )
}

/// Apply a compaction summary to the history, replacing the compacted range.
///
/// `summary_raw` is the raw summarizer output (or fallback transcript).
pub fn apply_compaction(
    history: &mut Vec<ChatMessage>,
    start: usize,
    compact_end: usize,
    summary_raw: &str,
    fallback_transcript: &str,
) {
    let summary = if summary_raw.is_empty() {
        truncate_with_ellipsis(fallback_transcript, COMPACTION_MAX_SUMMARY_CHARS)
    } else {
        truncate_with_ellipsis(summary_raw, COMPACTION_MAX_SUMMARY_CHARS)
    };
    apply_compaction_summary(history, start, compact_end, &summary);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_history_preserves_system() {
        let mut history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("1"),
            ChatMessage::assistant("2"),
            ChatMessage::user("3"),
            ChatMessage::assistant("4"),
        ];
        trim_history(&mut history, 2);
        assert_eq!(history.len(), 3); // system + 2 recent
        assert_eq!(history[0].role, "system");
    }

    #[test]
    fn trim_history_no_system() {
        let mut history = vec![
            ChatMessage::user("1"),
            ChatMessage::assistant("2"),
            ChatMessage::user("3"),
        ];
        trim_history(&mut history, 2);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn trim_history_within_limit() {
        let mut history = vec![ChatMessage::user("1"), ChatMessage::assistant("2")];
        trim_history(&mut history, 5);
        assert_eq!(history.len(), 2); // unchanged
    }

    #[test]
    fn estimate_tokens_basic() {
        let history = vec![ChatMessage::user("hello world")]; // 11 chars
        let tokens = estimate_history_tokens(&history);
        assert_eq!(tokens, 11_usize.div_ceil(4) + 4); // 3 + 4 = 7
    }

    #[test]
    fn prepare_compaction_not_needed() {
        let history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
        ];
        assert!(prepare_compaction(&history, 50, 100_000).is_none());
    }

    #[test]
    fn prepare_compaction_by_message_count() {
        let mut history = vec![ChatMessage::system("sys")];
        for i in 0..30 {
            history.push(ChatMessage::user(format!("msg {i}")));
            history.push(ChatMessage::assistant(format!("reply {i}")));
        }
        // 60 non-system messages, max_history = 25
        let result = prepare_compaction(&history, 25, 100_000);
        assert!(result.is_some());
        let (start, compact_end, transcript) = result.unwrap();
        assert_eq!(start, 1);
        assert!(compact_end > start);
        assert!(transcript.contains("USER:"));
    }

    #[test]
    fn apply_compaction_replaces_range() {
        let mut history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old1"),
            ChatMessage::assistant("old2"),
            ChatMessage::user("old3"),
            ChatMessage::assistant("old4"),
            ChatMessage::user("recent"),
        ];
        apply_compaction(&mut history, 1, 5, "Summary of old messages", "fallback");
        assert_eq!(history.len(), 3); // system + summary + recent
        assert!(history[1].content.contains("Compaction summary"));
        assert!(history[1].content.contains("Summary of old messages"));
        assert_eq!(history[2].content, "recent");
    }
}
