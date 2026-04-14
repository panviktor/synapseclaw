//! History compaction — domain policy for managing conversation history size.
//!
//! Pure business logic for trimming and compacting conversation history.
//! The summarization step requires calling an LLM provider, so callers
//! pass a callback for that operation.

use crate::domain::history_projection::is_projected_tool_call;
use crate::domain::message::ChatMessage;
use crate::domain::util::truncate_with_ellipsis;
use crate::ports::provider::{ConversationMessage, ToolResultMessage};
use std::fmt::Write;

/// Default trigger for auto-compaction when non-system message count exceeds this threshold.
pub const DEFAULT_MAX_HISTORY_MESSAGES: usize = 50;

/// Keep this many most-recent non-system messages after compaction.
const COMPACTION_KEEP_RECENT_MESSAGES: usize = 20;

pub const SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS: usize = 12;

/// Safety cap for compaction source transcript passed to the summarizer.
const COMPACTION_MAX_SOURCE_CHARS: usize = 12_000;

/// Max characters retained in stored compaction summary.
const COMPACTION_MAX_SUMMARY_CHARS: usize = 2_000;

const COMPACTION_DEFAULT_THRESHOLD_RATIO: f64 = 0.50;
const COMPACTION_DEFAULT_TARGET_RATIO: f64 = 0.20;
const COMPACTION_DEFAULT_PROTECT_FIRST_N: usize = 3;
const COMPACTION_DEFAULT_SUMMARY_RATIO: f64 = 0.20;
const COMPACTION_MIN_TARGET_RATIO: f64 = 0.10;
const COMPACTION_MAX_TARGET_RATIO: f64 = 0.80;
const COMPACTION_MIN_THRESHOLD_RATIO: f64 = 0.05;
const COMPACTION_MAX_THRESHOLD_RATIO: f64 = 0.95;
const COMPACTION_TOOL_RESULT_PLACEHOLDER_HEAD_CHARS: usize = 180;
const COMPACTION_TOOL_RESULT_PLACEHOLDER_TAIL_CHARS: usize = 180;
const COMPACTION_TOOL_RESULT_PLACEHOLDER_TRIGGER_CHARS: usize = 900;
const MISSING_TOOL_RESULT_STUB: &str =
    "[tool-result-compacted]\nResult omitted by history compaction; use the compaction summary and subsequent conversation state.";

/// Prefix used for compacted conversation summaries stored back into provider history.
pub const COMPACTION_SUMMARY_PREFIX: &str = "[Compaction summary]\n";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistoryCompressionPolicy {
    pub enabled: bool,
    pub threshold_ratio: f64,
    pub target_ratio: f64,
    pub protect_last_n: usize,
    pub protect_first_n: usize,
    pub summary_ratio: f64,
    pub min_summary_tokens: usize,
    pub max_summary_tokens: usize,
    pub max_source_chars: usize,
    pub max_summary_chars: usize,
}

impl Default for HistoryCompressionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_ratio: COMPACTION_DEFAULT_THRESHOLD_RATIO,
            target_ratio: COMPACTION_DEFAULT_TARGET_RATIO,
            protect_last_n: COMPACTION_KEEP_RECENT_MESSAGES,
            protect_first_n: COMPACTION_DEFAULT_PROTECT_FIRST_N,
            summary_ratio: COMPACTION_DEFAULT_SUMMARY_RATIO,
            min_summary_tokens: 2_000,
            max_summary_tokens: 12_000,
            max_source_chars: COMPACTION_MAX_SOURCE_CHARS,
            max_summary_chars: COMPACTION_MAX_SUMMARY_CHARS,
        }
    }
}

impl From<&crate::config::schema::ContextCompressionConfig> for HistoryCompressionPolicy {
    fn from(config: &crate::config::schema::ContextCompressionConfig) -> Self {
        let mut policy = Self {
            enabled: config.enabled,
            threshold_ratio: clamp_ratio(
                config.threshold,
                COMPACTION_MIN_THRESHOLD_RATIO,
                COMPACTION_MAX_THRESHOLD_RATIO,
                COMPACTION_DEFAULT_THRESHOLD_RATIO,
            ),
            target_ratio: clamp_ratio(
                config.target_ratio,
                COMPACTION_MIN_TARGET_RATIO,
                COMPACTION_MAX_TARGET_RATIO,
                COMPACTION_DEFAULT_TARGET_RATIO,
            ),
            protect_last_n: config.protect_last_n.max(1),
            protect_first_n: config.protect_first_n,
            summary_ratio: clamp_ratio(config.summary_ratio, 0.05, 0.80, 0.20),
            min_summary_tokens: config.min_summary_tokens.max(1),
            max_summary_tokens: config.max_summary_tokens.max(1),
            max_source_chars: config.max_source_chars.max(1),
            max_summary_chars: config.max_summary_chars.max(1),
        };
        if policy.max_summary_tokens < policy.min_summary_tokens {
            policy.max_summary_tokens = policy.min_summary_tokens;
        }
        policy
    }
}

pub fn resolve_context_compression_config_for_route(
    base: &crate::config::schema::ContextCompressionConfig,
    overrides: &[crate::config::schema::ContextCompressionRouteOverrideConfig],
    provider: &str,
    model: &str,
    lane: Option<crate::config::schema::CapabilityLane>,
    hint: Option<&str>,
) -> crate::config::schema::ContextCompressionConfig {
    overrides.iter().fold(base.clone(), |policy, candidate| {
        if compression_override_matches(candidate, provider, model, lane, hint) {
            policy.apply_override(candidate)
        } else {
            policy
        }
    })
}

pub fn compact_provider_history_for_session_hygiene(
    history: &mut Vec<ChatMessage>,
    keep_non_system_turns: usize,
) -> bool {
    let non_system_count = history.iter().filter(|msg| msg.role != "system").count();
    if non_system_count <= keep_non_system_turns {
        return false;
    }

    let mut remaining_non_system = keep_non_system_turns;
    let mut compacted = Vec::with_capacity(history.len());
    for message in history.drain(..).rev() {
        if message.role == "system" {
            compacted.push(message);
        } else if remaining_non_system > 0 {
            remaining_non_system -= 1;
            compacted.push(message);
        }
    }
    compacted.reverse();
    drop_leading_non_user_provider_messages(&mut compacted);
    *history = compacted;
    true
}

#[derive(Debug, Clone)]
pub struct SessionHygieneDroppedMessage {
    pub index: usize,
    pub message: ChatMessage,
}

pub fn session_hygiene_dropped_messages(
    history: &[ChatMessage],
    keep_non_system_turns: usize,
) -> Vec<ChatMessage> {
    session_hygiene_dropped_messages_with_indices(history, keep_non_system_turns)
        .into_iter()
        .map(|entry| entry.message)
        .collect()
}

pub fn session_hygiene_dropped_messages_with_indices(
    history: &[ChatMessage],
    keep_non_system_turns: usize,
) -> Vec<SessionHygieneDroppedMessage> {
    let non_system_count = history.iter().filter(|msg| msg.role != "system").count();
    if non_system_count <= keep_non_system_turns {
        return Vec::new();
    }

    let mut remaining_non_system = keep_non_system_turns;
    let mut retained = Vec::with_capacity(history.len());
    let mut dropped = Vec::new();
    for (index, message) in history.iter().enumerate().rev() {
        let entry = SessionHygieneDroppedMessage {
            index,
            message: message.clone(),
        };
        if message.role == "system" {
            retained.push(entry);
        } else if remaining_non_system > 0 {
            remaining_non_system -= 1;
            retained.push(entry);
        } else {
            dropped.push(entry);
        }
    }
    retained.reverse();
    dropped.reverse();

    loop {
        let Some(first_non_system) = retained
            .iter()
            .position(|entry| entry.message.role != "system")
        else {
            break;
        };
        if retained[first_non_system].message.role == "user" {
            break;
        }
        dropped.push(retained.remove(first_non_system));
    }
    dropped.sort_by_key(|entry| entry.index);
    dropped
}

fn drop_leading_non_user_provider_messages(history: &mut Vec<ChatMessage>) {
    loop {
        let Some(first_non_system) = history.iter().position(|message| message.role != "system")
        else {
            return;
        };
        if history[first_non_system].role == "user" {
            return;
        }
        history.remove(first_non_system);
    }
}

fn compression_override_matches(
    candidate: &crate::config::schema::ContextCompressionRouteOverrideConfig,
    provider: &str,
    model: &str,
    lane: Option<crate::config::schema::CapabilityLane>,
    hint: Option<&str>,
) -> bool {
    let has_selector = candidate.hint.is_some()
        || candidate.provider.is_some()
        || candidate.model.is_some()
        || candidate.lane.is_some();
    if !has_selector {
        return true;
    }

    candidate.hint.as_deref().map_or(true, |expected| {
        hint.is_some_and(|actual| expected.eq_ignore_ascii_case(actual))
    }) && candidate
        .provider
        .as_deref()
        .map_or(true, |expected| expected.eq_ignore_ascii_case(provider))
        && candidate
            .model
            .as_deref()
            .map_or(true, |expected| expected == model)
        && candidate
            .lane
            .map_or(true, |expected| lane == Some(expected))
}

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

fn clamp_ratio(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

pub fn history_compression_threshold_tokens(
    context_budget_tokens: usize,
    policy: &HistoryCompressionPolicy,
) -> usize {
    scaled_tokens(context_budget_tokens, policy.threshold_ratio).max(1)
}

pub fn history_compression_tail_budget_tokens(
    threshold_tokens: usize,
    policy: &HistoryCompressionPolicy,
) -> usize {
    scaled_tokens(threshold_tokens, policy.target_ratio).max(1)
}

pub fn history_compression_summary_budget_tokens(
    content_tokens: usize,
    context_window_tokens: Option<usize>,
    policy: &HistoryCompressionPolicy,
) -> usize {
    let context_limit = context_window_tokens
        .map(|tokens| scaled_tokens(tokens, 0.05).max(1))
        .unwrap_or(policy.max_summary_tokens);
    let maximum = policy.max_summary_tokens.min(context_limit).max(1);
    scaled_tokens(content_tokens, policy.summary_ratio)
        .max(policy.min_summary_tokens)
        .min(maximum)
}

fn scaled_tokens(tokens: usize, ratio: f64) -> usize {
    if !ratio.is_finite() || ratio <= 0.0 || tokens == 0 {
        return 0;
    }
    ((tokens as f64) * ratio).round() as usize
}

pub fn build_compaction_transcript(messages: &[ChatMessage], max_source_chars: usize) -> String {
    let mut transcript = String::new();
    for msg in messages {
        let role = msg.role.to_uppercase();
        let content = compact_tool_result_for_summary(msg);
        let _ = writeln!(transcript, "{role}: {}", content.trim());
    }

    if transcript.chars().count() > max_source_chars {
        truncate_with_ellipsis(&transcript, max_source_chars)
    } else {
        transcript
    }
}

fn compact_tool_result_for_summary(message: &ChatMessage) -> String {
    if message.role != "tool" {
        return message.content.clone();
    }
    let char_count = message.content.chars().count();
    if char_count <= COMPACTION_TOOL_RESULT_PLACEHOLDER_TRIGGER_CHARS {
        return message.content.clone();
    }

    let head = first_chars(
        &message.content,
        COMPACTION_TOOL_RESULT_PLACEHOLDER_HEAD_CHARS,
    );
    let tail = last_chars(
        &message.content,
        COMPACTION_TOOL_RESULT_PLACEHOLDER_TAIL_CHARS,
    );
    format!("[tool-result-pruned chars={char_count}]\nhead:\n{head}\n\ntail:\n{tail}")
}

fn first_chars(value: &str, count: usize) -> &str {
    value
        .char_indices()
        .nth(count)
        .map(|(idx, _)| &value[..idx])
        .unwrap_or(value)
}

fn last_chars(value: &str, count: usize) -> &str {
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
    prepare_compaction_with_policy(
        history,
        max_history,
        max_context_tokens,
        &HistoryCompressionPolicy::default(),
    )
}

pub fn prepare_compaction_with_policy(
    history: &[ChatMessage],
    max_history: usize,
    max_context_tokens: usize,
    policy: &HistoryCompressionPolicy,
) -> Option<(usize, usize, String)> {
    prepare_compaction_with_policy_and_observed_tokens(
        history,
        max_history,
        max_context_tokens,
        policy,
        None,
    )
}

pub fn prepare_compaction_with_policy_and_observed_tokens(
    history: &[ChatMessage],
    max_history: usize,
    max_context_tokens: usize,
    policy: &HistoryCompressionPolicy,
    observed_provider_input_tokens: Option<usize>,
) -> Option<(usize, usize, String)> {
    if !policy.enabled {
        return None;
    }

    let has_system = history.first().is_some_and(|m| m.role == "system");
    let non_system_count = if has_system {
        history.len().saturating_sub(1)
    } else {
        history.len()
    };

    let estimated_tokens = estimate_history_tokens(history);
    let pressure_tokens = observed_provider_input_tokens
        .filter(|tokens| *tokens > 0)
        .map_or(estimated_tokens, |tokens| estimated_tokens.max(tokens));

    // Trigger compaction when either token budget OR message count is exceeded.
    if pressure_tokens <= max_context_tokens && non_system_count <= max_history {
        return None;
    }

    let start = advance_boundary_past_tool_group(
        history,
        protected_head_end(history, has_system, policy.protect_first_n),
    );
    let token_tail_start = protected_tail_start(history, start, max_context_tokens, policy);
    let message_tail_start = if non_system_count > max_history {
        protected_tail_start_for_message_budget(history, start, has_system, max_history)
    } else {
        start
    };
    let tail_start = token_tail_start.max(message_tail_start);
    if tail_start <= start {
        return None;
    }

    let mut compact_end = tail_start;

    // Snap compact_end to a user-turn boundary so we don't split mid-conversation.
    while compact_end > start && history.get(compact_end).is_some_and(|m| m.role != "user") {
        compact_end -= 1;
    }
    compact_end = rewind_boundary_before_tool_group(history, compact_end, start);
    if compact_end <= start {
        return None;
    }

    let to_compact: Vec<ChatMessage> = history[start..compact_end].to_vec();
    let transcript = build_compaction_transcript(&to_compact, policy.max_source_chars);

    Some((start, compact_end, transcript))
}

fn protected_head_end(
    history: &[ChatMessage],
    has_system: bool,
    configured_protect_first_n: usize,
) -> usize {
    let system_floor = if has_system { 1 } else { 0 };
    configured_protect_first_n
        .max(system_floor)
        .min(history.len())
}

fn protected_tail_start(
    history: &[ChatMessage],
    start: usize,
    threshold_tokens: usize,
    policy: &HistoryCompressionPolicy,
) -> usize {
    if history.len() <= start {
        return start;
    }

    let min_tail_count = policy
        .protect_last_n
        .min(history.len().saturating_sub(start));
    let tail_budget_tokens = history_compression_tail_budget_tokens(threshold_tokens, policy);
    let mut tail_start = history.len();
    let mut tail_tokens = 0usize;

    while tail_start > start {
        let next_index = tail_start - 1;
        let next_count = history.len() - next_index;
        let message_tokens = estimate_history_tokens(&history[next_index..tail_start]);
        if next_count > min_tail_count
            && tail_tokens.saturating_add(message_tokens) > tail_budget_tokens
        {
            break;
        }
        tail_tokens = tail_tokens.saturating_add(message_tokens);
        tail_start = next_index;
    }

    tail_start
}

fn protected_tail_start_for_message_budget(
    history: &[ChatMessage],
    start: usize,
    has_system: bool,
    max_history: usize,
) -> usize {
    let protected_head_non_system = start.saturating_sub(usize::from(has_system));
    let summary_message_budget = 1usize;
    let tail_message_budget = max_history
        .saturating_sub(protected_head_non_system)
        .saturating_sub(summary_message_budget);
    history.len().saturating_sub(tail_message_budget).max(start)
}

fn advance_boundary_past_tool_group(history: &[ChatMessage], mut boundary: usize) -> usize {
    while boundary < history.len() && boundary_splits_tool_group(history, boundary) {
        boundary += 1;
    }
    boundary
}

fn rewind_boundary_before_tool_group(
    history: &[ChatMessage],
    mut boundary: usize,
    floor: usize,
) -> usize {
    while boundary > floor && boundary_splits_tool_group(history, boundary) {
        boundary -= 1;
    }
    boundary
}

fn boundary_splits_tool_group(history: &[ChatMessage], boundary: usize) -> bool {
    if boundary == 0 || boundary >= history.len() {
        return false;
    }

    let left = &history[boundary - 1];
    let right = &history[boundary];
    is_tool_result_message(right) || is_tool_call_message(left)
}

fn is_tool_call_message(message: &ChatMessage) -> bool {
    message.role == "assistant"
        && (message.content.contains("<tool_call") || is_projected_tool_call(&message.content))
}

fn is_tool_result_message(message: &ChatMessage) -> bool {
    message.role == "tool" || message.content.contains("<tool_result")
}

/// The system prompt for the summarizer LLM call.
pub const COMPACTION_SUMMARIZER_SYSTEM: &str = "You are a conversation compaction engine. Summarize older chat history into concise context for future turns. Preserve: user preferences, commitments, decisions, unresolved tasks, key facts. Omit: filler, repeated chit-chat, verbose tool logs. Output plain text bullet points only.";

/// Build the user-facing prompt for the summarizer.
pub fn compaction_summarizer_prompt(transcript: &str) -> String {
    compaction_summarizer_prompt_with_policy(
        transcript,
        None,
        &HistoryCompressionPolicy::default(),
        None,
    )
}

pub fn compaction_summarizer_prompt_with_policy(
    transcript: &str,
    previous_summary: Option<&str>,
    policy: &HistoryCompressionPolicy,
    context_window_tokens: Option<usize>,
) -> String {
    compaction_summarizer_prompt_with_policy_and_hints(
        transcript,
        previous_summary,
        policy,
        context_window_tokens,
        &[],
    )
}

pub fn compaction_summarizer_prompt_with_policy_and_hints(
    transcript: &str,
    previous_summary: Option<&str>,
    policy: &HistoryCompressionPolicy,
    context_window_tokens: Option<usize>,
    preservation_hints: &[String],
) -> String {
    let content_tokens = transcript.chars().count().div_ceil(4);
    let summary_budget =
        history_compression_summary_budget_tokens(content_tokens, context_window_tokens, policy);
    let previous_summary = previous_summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .unwrap_or("(none)");
    let preservation_hints = crate::application::services::memory_precompress_handoff::format_precompress_preservation_hints(
        preservation_hints,
    );
    let preservation_hints = if preservation_hints.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n{preservation_hints}")
    };
    format!(
        "Update the compacted conversation context for future turns.\n\
         Target summary budget: about {summary_budget} tokens.\n\n\
         Preserve these sections when relevant:\n\
         ## Goal\n\
         ## Constraints & Preferences\n\
         ## Progress\n\
         ### Done\n\
         ### In Progress\n\
         ### Blocked\n\
         ## Key Decisions\n\
         ## Relevant Files\n\
         ## Next Steps\n\
         ## Critical Context\n\n\
         Previous compacted context:\n{previous_summary}{preservation_hints}\n\n\
         New history to merge:\n{transcript}"
    )
}

/// Apply a compaction summary to the history, replacing the compacted range.
///
/// `summary_raw` is the raw summarizer output. Empty summaries are rejected so
/// callers cannot silently replay raw transcript as compacted context.
pub fn apply_compaction(
    history: &mut Vec<ChatMessage>,
    start: usize,
    compact_end: usize,
    summary_raw: &str,
) -> bool {
    apply_compaction_with_policy(
        history,
        start,
        compact_end,
        summary_raw,
        &HistoryCompressionPolicy::default(),
    )
}

pub fn apply_compaction_with_policy(
    history: &mut Vec<ChatMessage>,
    start: usize,
    compact_end: usize,
    summary_raw: &str,
    policy: &HistoryCompressionPolicy,
) -> bool {
    let summary_raw = summary_raw.trim();
    if summary_raw.is_empty() {
        return false;
    }
    let summary = truncate_with_ellipsis(summary_raw, policy.max_summary_chars);
    apply_compaction_summary(history, start, compact_end, &summary);
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolProtocolSanitization {
    pub removed_orphan_results: usize,
    pub inserted_stub_results: usize,
}

/// Repair provider-facing tool-call history after compaction.
///
/// Compaction boundaries should avoid splitting tool-call/result groups. This
/// pass is a final structural safety net for native-tool providers that reject
/// orphaned tool results or assistant tool calls without matching results.
pub fn sanitize_tool_protocol_after_compaction(
    history: &mut Vec<ConversationMessage>,
) -> ToolProtocolSanitization {
    let mut sanitized = Vec::with_capacity(history.len());
    let mut pending_tool_call_ids: Vec<String> = Vec::new();
    let mut stats = ToolProtocolSanitization::default();

    for message in history.drain(..) {
        match message {
            ConversationMessage::AssistantToolCalls {
                text,
                tool_calls,
                reasoning_content,
                media_artifacts,
            } => {
                insert_missing_tool_result_stubs(
                    &mut sanitized,
                    &mut pending_tool_call_ids,
                    &mut stats,
                );
                pending_tool_call_ids = tool_calls.iter().map(|call| call.id.clone()).collect();
                sanitized.push(ConversationMessage::AssistantToolCalls {
                    text,
                    tool_calls,
                    reasoning_content,
                    media_artifacts,
                });
            }
            ConversationMessage::ToolResults(results) => {
                let mut kept = Vec::new();
                for result in results {
                    if let Some(pos) = pending_tool_call_ids
                        .iter()
                        .position(|id| id == &result.tool_call_id)
                    {
                        pending_tool_call_ids.remove(pos);
                        kept.push(result);
                    } else {
                        stats.removed_orphan_results += 1;
                    }
                }
                if !kept.is_empty() {
                    sanitized.push(ConversationMessage::ToolResults(kept));
                }
            }
            other => {
                insert_missing_tool_result_stubs(
                    &mut sanitized,
                    &mut pending_tool_call_ids,
                    &mut stats,
                );
                sanitized.push(other);
            }
        }
    }

    insert_missing_tool_result_stubs(&mut sanitized, &mut pending_tool_call_ids, &mut stats);
    *history = sanitized;
    stats
}

fn insert_missing_tool_result_stubs(
    history: &mut Vec<ConversationMessage>,
    pending_tool_call_ids: &mut Vec<String>,
    stats: &mut ToolProtocolSanitization,
) {
    if pending_tool_call_ids.is_empty() {
        return;
    }

    let results = pending_tool_call_ids
        .drain(..)
        .map(|tool_call_id| ToolResultMessage {
            tool_call_id,
            content: MISSING_TOOL_RESULT_STUB.to_string(),
        })
        .collect::<Vec<_>>();
    stats.inserted_stub_results += results.len();
    history.push(ConversationMessage::ToolResults(results));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        CapabilityLane, ContextCompressionConfig, ContextCompressionRouteOverrideConfig,
    };
    use crate::domain::history_projection::format_projected_tool_call;
    use crate::ports::provider::{ToolCall, ToolResultMessage};

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
    fn session_hygiene_reports_exact_dropped_messages() {
        let history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old user"),
            ChatMessage::assistant("old assistant"),
            ChatMessage::assistant("orphan assistant"),
            ChatMessage::user("recent user"),
            ChatMessage::assistant("recent assistant"),
        ];

        let dropped = session_hygiene_dropped_messages(&history, 3);
        assert_eq!(
            dropped
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["old user", "old assistant", "orphan assistant"]
        );
        let dropped_with_indices = session_hygiene_dropped_messages_with_indices(&history, 3);
        assert_eq!(
            dropped_with_indices
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        let mut compacted = history;
        assert!(compact_provider_history_for_session_hygiene(
            &mut compacted,
            3
        ));
        assert_eq!(
            compacted
                .iter()
                .filter(|message| message.role != "system")
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["recent user", "recent assistant"]
        );
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
        assert_eq!(start, 3);
        assert!(compact_end > start);
        assert!(transcript.contains("USER:"));
    }

    #[test]
    fn prepare_compaction_respects_disabled_policy() {
        let mut history = vec![ChatMessage::system("sys")];
        for i in 0..30 {
            history.push(ChatMessage::user(format!("msg {i}")));
        }

        let policy = HistoryCompressionPolicy {
            enabled: false,
            ..HistoryCompressionPolicy::default()
        };

        assert!(prepare_compaction_with_policy(&history, 4, 1, &policy).is_none());
    }

    #[test]
    fn prepare_compaction_uses_tail_budget_and_protected_head() {
        let mut history = vec![ChatMessage::system("sys")];
        history.push(ChatMessage::user("first request"));
        history.push(ChatMessage::assistant("first reply"));
        for i in 0..20 {
            history.push(ChatMessage::user(format!(
                "older msg {i} {}",
                "x".repeat(80)
            )));
            history.push(ChatMessage::assistant(format!(
                "older reply {i} {}",
                "y".repeat(80)
            )));
        }

        let policy = HistoryCompressionPolicy {
            protect_first_n: 3,
            protect_last_n: 4,
            target_ratio: 0.10,
            ..HistoryCompressionPolicy::default()
        };
        let (start, compact_end, transcript) =
            prepare_compaction_with_policy(&history, 6, 120, &policy).expect("compaction");

        assert_eq!(start, 3);
        assert!(compact_end <= history.len().saturating_sub(4));
        assert!(transcript.contains("older msg"));
    }

    #[test]
    fn prepare_compaction_does_not_split_tool_group_at_protected_head() {
        let mut history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("inspect file"),
            ChatMessage::assistant(format_projected_tool_call(
                "call-1",
                "file_read",
                "{\"path\":\"a.rs\"}",
            )),
            ChatMessage::tool("tool output"),
        ];
        for i in 0..12 {
            history.push(ChatMessage::user(format!("older msg {i}")));
            history.push(ChatMessage::assistant(format!("older reply {i}")));
        }

        let policy = HistoryCompressionPolicy {
            protect_first_n: 3,
            protect_last_n: 2,
            target_ratio: 0.10,
            ..HistoryCompressionPolicy::default()
        };
        let (start, compact_end, transcript) =
            prepare_compaction_with_policy(&history, 4, 40, &policy).expect("compaction");

        assert_eq!(start, 4);
        assert!(compact_end > start);
        assert!(!transcript.contains("tool output"));
        assert!(transcript.contains("older msg"));
    }

    #[test]
    fn provider_observed_tokens_can_trigger_compaction() {
        let mut history = vec![ChatMessage::system("sys")];
        history.push(ChatMessage::user("first request"));
        history.push(ChatMessage::assistant("first reply"));
        for i in 0..8 {
            history.push(ChatMessage::user(format!("older msg {i}")));
            history.push(ChatMessage::assistant(format!("older reply {i}")));
        }

        let policy = HistoryCompressionPolicy {
            protect_first_n: 1,
            protect_last_n: 2,
            ..HistoryCompressionPolicy::default()
        };
        assert!(prepare_compaction_with_policy(&history, 100, 100_000, &policy).is_none());
        assert!(prepare_compaction_with_policy_and_observed_tokens(
            &history,
            100,
            100,
            &policy,
            Some(1_000)
        )
        .is_some());
    }

    #[test]
    fn compaction_transcript_prunes_large_tool_results() {
        let history = vec![
            ChatMessage::user("inspect output"),
            ChatMessage::assistant(format_projected_tool_call("call-1", "shell", "{}")),
            ChatMessage::tool(format!("head{}tail", "x".repeat(2_000))),
            ChatMessage::user("next"),
        ];
        let policy = HistoryCompressionPolicy {
            protect_first_n: 0,
            protect_last_n: 1,
            max_source_chars: 4_000,
            ..HistoryCompressionPolicy::default()
        };
        let (_, _, transcript) =
            prepare_compaction_with_policy(&history, 2, 10, &policy).expect("compaction");

        assert!(transcript.contains("[tool-result-pruned chars="));
        assert!(transcript.contains("head:"));
        assert!(transcript.contains("tail:"));
        assert!(transcript.len() < 1_000);
    }

    #[test]
    fn tool_protocol_sanitizer_removes_orphans_and_inserts_missing_results() {
        let mut history = vec![
            ConversationMessage::ToolResults(vec![ToolResultMessage {
                tool_call_id: "orphan".into(),
                content: "orphaned".into(),
            }]),
            ConversationMessage::AssistantToolCalls {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "file_read".into(),
                    arguments: "{}".into(),
                }],
                reasoning_content: None,
                media_artifacts: Vec::new(),
            },
            ConversationMessage::Chat(ChatMessage::assistant("after compaction")),
        ];

        let stats = sanitize_tool_protocol_after_compaction(&mut history);

        assert_eq!(stats.removed_orphan_results, 1);
        assert_eq!(stats.inserted_stub_results, 1);
        assert_eq!(history.len(), 3);
        assert!(matches!(
            &history[0],
            ConversationMessage::AssistantToolCalls { .. }
        ));
        match &history[1] {
            ConversationMessage::ToolResults(results) => {
                assert_eq!(results[0].tool_call_id, "call-1");
                assert!(results[0].content.contains("tool-result-compacted"));
            }
            _ => panic!("expected inserted tool result stub"),
        }
    }

    #[test]
    fn summarizer_prompt_merges_previous_summary_and_uses_budget() {
        let policy = HistoryCompressionPolicy::default();
        let prompt = compaction_summarizer_prompt_with_policy(
            "USER: continue\nASSISTANT: done",
            Some("## Goal\nKeep the project stable"),
            &policy,
            Some(200_000),
        );

        assert!(prompt.contains("Previous compacted context"));
        assert!(prompt.contains("Keep the project stable"));
        assert!(prompt.contains("Target summary budget"));
        assert!(prompt.contains("## Critical Context"));
    }

    #[test]
    fn summarizer_prompt_adds_handoff_hints_only_when_present() {
        let policy = HistoryCompressionPolicy::default();
        let without_hints = compaction_summarizer_prompt_with_policy_and_hints(
            "USER: continue",
            None,
            &policy,
            None,
            &[],
        );
        assert!(!without_hints.contains("Authoritative compacted-context facts"));

        let with_hints = compaction_summarizer_prompt_with_policy_and_hints(
            "USER: continue",
            None,
            &policy,
            None,
            &["recipe: tool_sequence=rg -> systemctl".to_string()],
        );
        assert!(with_hints.contains("Authoritative compacted-context facts"));
        assert!(with_hints.contains("recipe: tool_sequence=rg -> systemctl"));
    }

    #[test]
    fn summary_budget_scales_from_content_and_window() {
        let policy = HistoryCompressionPolicy {
            summary_ratio: 0.50,
            min_summary_tokens: 10,
            max_summary_tokens: 10_000,
            ..HistoryCompressionPolicy::default()
        };

        assert_eq!(
            history_compression_summary_budget_tokens(1_000, Some(20_000), &policy),
            500
        );
        assert_eq!(
            history_compression_summary_budget_tokens(100_000, Some(20_000), &policy),
            1_000
        );
    }

    #[test]
    fn route_compression_overrides_compose_by_selector_order() {
        let base = ContextCompressionConfig::default();
        let overrides = vec![
            ContextCompressionRouteOverrideConfig {
                provider: Some("deepseek".into()),
                threshold: Some(0.35),
                protect_last_n: Some(10),
                ..Default::default()
            },
            ContextCompressionRouteOverrideConfig {
                provider: Some("deepseek".into()),
                lane: Some(CapabilityLane::CheapReasoning),
                threshold: Some(0.25),
                cache_ttl_secs: Some(3_600),
                ..Default::default()
            },
            ContextCompressionRouteOverrideConfig {
                hint: Some("grok-long".into()),
                target_ratio: Some(0.30),
                cache_max_entries: Some(1_024),
                ..Default::default()
            },
        ];

        let deepseek_reasoning = resolve_context_compression_config_for_route(
            &base,
            &overrides,
            "deepseek",
            "deepseek-chat",
            Some(CapabilityLane::Reasoning),
            None,
        );
        assert_eq!(deepseek_reasoning.threshold, 0.35);
        assert_eq!(deepseek_reasoning.protect_last_n, 10);
        assert_eq!(deepseek_reasoning.cache_ttl_secs, base.cache_ttl_secs);

        let deepseek_cheap = resolve_context_compression_config_for_route(
            &base,
            &overrides,
            "deepseek",
            "deepseek-chat",
            Some(CapabilityLane::CheapReasoning),
            None,
        );
        assert_eq!(deepseek_cheap.threshold, 0.25);
        assert_eq!(deepseek_cheap.protect_last_n, 10);
        assert_eq!(deepseek_cheap.cache_ttl_secs, 3_600);

        let grok_long = resolve_context_compression_config_for_route(
            &base,
            &overrides,
            "openrouter",
            "x-ai/grok-4.20",
            Some(CapabilityLane::Reasoning),
            Some("grok-long"),
        );
        assert_eq!(grok_long.threshold, base.threshold);
        assert_eq!(grok_long.target_ratio, 0.30);
        assert_eq!(grok_long.cache_max_entries, 1_024);
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
        assert!(apply_compaction(
            &mut history,
            1,
            5,
            "Summary of old messages"
        ));
        assert_eq!(history.len(), 3); // system + summary + recent
        assert!(history[1].content.contains("Compaction summary"));
        assert!(history[1].content.contains("Summary of old messages"));
        assert_eq!(history[2].content, "recent");
    }

    #[test]
    fn apply_compaction_rejects_empty_summary() {
        let mut history = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("old1"),
            ChatMessage::assistant("old2"),
            ChatMessage::user("recent"),
        ];
        let original = history.clone();
        assert!(!apply_compaction(&mut history, 1, 3, "   "));
        assert_eq!(history, original);
    }
}
