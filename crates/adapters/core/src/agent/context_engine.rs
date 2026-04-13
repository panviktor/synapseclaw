use crate::agent::dispatcher::ToolDispatcher;
use synapse_domain::application::services::history_compaction;
use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile;
use synapse_domain::application::services::provider_context_budget::{
    provider_context_prune_policy, ProviderContextBudgetInput,
};
use synapse_domain::ports::context_engine::{
    ContextEnginePort, ProviderPromptContextStats, ProviderPromptSnapshot,
    ProviderPromptSnapshotInput,
};
use synapse_providers::{ChatMessage, ConversationMessage, ToolCall, ToolResultMessage};

const COMPACT_TOOL_RESULT_MAX_CHARS: usize = 320;
const PROVIDER_ASSISTANT_TEXT_MAX_CHARS: usize = 1_200;
const PROVIDER_TOOL_CALL_ARGS_MAX_CHARS: usize = 900;
const PROVIDER_TOOL_RESULT_MAX_CHARS: usize = 1_200;
const PROVIDER_CONTEXT_RELEVANT_TAIL_MIN_MESSAGES: usize = 2;
const PROVIDER_CONTEXT_PROTECT_FIRST_CHAT_MESSAGES: usize = 2;
const PROVIDER_CONTEXT_QUERY_MIN_TERM_CHARS: usize = 4;

pub(crate) struct AdapterContextEngine<'a> {
    dispatcher: &'a dyn ToolDispatcher,
}

impl<'a> AdapterContextEngine<'a> {
    pub(crate) fn new(dispatcher: &'a dyn ToolDispatcher) -> Self {
        Self { dispatcher }
    }
}

impl ContextEnginePort for AdapterContextEngine<'_> {
    fn build_provider_prompt_snapshot(
        &self,
        input: ProviderPromptSnapshotInput<'_>,
    ) -> ProviderPromptSnapshot {
        build_provider_prompt_snapshot(
            self.dispatcher,
            input.history,
            input.recent_chat_limit,
            input.target_profile,
        )
    }
}

pub(crate) fn total_message_chars(messages: &[ChatMessage]) -> usize {
    messages.iter().map(|msg| msg.content.chars().count()).sum()
}

pub(crate) fn system_message_breakdown(history: &[ConversationMessage]) -> Vec<(String, usize)> {
    let system_messages = history
        .iter()
        .filter_map(|msg| match msg {
            ConversationMessage::Chat(chat) if chat.role == "system" => Some(chat.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    system_message_breakdown_from_chat_messages(&system_messages)
}

fn system_message_breakdown_from_chat_messages(messages: &[ChatMessage]) -> Vec<(String, usize)> {
    let mut breakdown = std::collections::BTreeMap::<String, usize>::new();
    for chat in messages {
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
    target_profile: Option<&ResolvedModelProfile>,
) -> ProviderPromptSnapshot {
    let latest_user_index = history.iter().rposition(|msg| {
        matches!(
            msg,
            ConversationMessage::Chat(chat) if chat.role == "user"
        )
    });

    let mut system_messages: Vec<ChatMessage> = history
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

    let current_user_query = latest_user_index.and_then(|index| match history.get(index) {
        Some(ConversationMessage::Chat(chat)) if chat.role == "user" => Some(chat.content.as_str()),
        _ => None,
    });

    let mut prior_chat_context =
        select_prior_chat_context(&recent_chat_context, recent_chat_limit, current_user_query);
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

    system_messages = prune_system_messages_for_pressure(
        system_messages,
        &prior_chat_messages,
        &current_turn_messages,
        target_profile,
    );

    let mut messages = Vec::with_capacity(
        system_messages.len() + prior_chat_messages.len() + current_turn_messages.len(),
    );
    messages.extend(system_messages.iter().cloned());
    messages.extend(prior_chat_messages.iter().cloned());
    messages.extend(current_turn_messages.iter().cloned());

    let stats = provider_context_stats_for_parts(
        &system_messages,
        &prior_chat_messages,
        &current_turn_messages,
    );

    ProviderPromptSnapshot { messages, stats }
}

fn select_prior_chat_context(
    recent_chat_context: &[ConversationMessage],
    recent_chat_limit: usize,
    current_user_query: Option<&str>,
) -> Vec<ConversationMessage> {
    if recent_chat_limit == 0 || recent_chat_context.is_empty() {
        return Vec::new();
    }
    if recent_chat_context.len() <= recent_chat_limit {
        return recent_chat_context.to_vec();
    }

    let mut selected = std::collections::BTreeSet::<usize>::new();
    let query_terms = current_user_query
        .map(|query| weighted_query_terms(query, recent_chat_context))
        .unwrap_or_default();
    let tail_min = PROVIDER_CONTEXT_RELEVANT_TAIL_MIN_MESSAGES.min(recent_chat_limit);
    let relevance_budget = recent_chat_limit.saturating_sub(tail_min);

    if !query_terms.is_empty() && relevance_budget > 0 {
        for candidate in relevant_prior_turn_pairs(recent_chat_context, &query_terms) {
            if !try_add_index_group(&mut selected, &candidate.indices, relevance_budget) {
                continue;
            }
            if selected.len() >= relevance_budget {
                break;
            }
        }
    }

    let head_budget = recent_chat_limit.saturating_sub(tail_min);
    for idx in 0..recent_chat_context
        .len()
        .min(PROVIDER_CONTEXT_PROTECT_FIRST_CHAT_MESSAGES)
    {
        if selected.len() >= head_budget {
            break;
        }
        selected.insert(idx);
    }

    let tail_start = recent_chat_context.len().saturating_sub(tail_min);
    for idx in tail_start..recent_chat_context.len() {
        if selected.len() >= recent_chat_limit {
            break;
        }
        selected.insert(idx);
    }

    if selected.len() < recent_chat_limit {
        for idx in (0..recent_chat_context.len()).rev() {
            if selected.len() >= recent_chat_limit {
                break;
            }
            selected.insert(idx);
        }
    }

    selected
        .into_iter()
        .filter_map(|idx| recent_chat_context.get(idx).cloned())
        .collect()
}

#[derive(Debug)]
struct RelevantPriorTurnPair {
    score: usize,
    first_index: usize,
    indices: Vec<usize>,
}

fn relevant_prior_turn_pairs(
    recent_chat_context: &[ConversationMessage],
    query_terms: &std::collections::BTreeMap<String, usize>,
) -> Vec<RelevantPriorTurnPair> {
    let mut candidates = Vec::new();
    let mut idx = 0usize;

    while idx < recent_chat_context.len() {
        let mut indices = vec![idx];
        if is_chat_role(&recent_chat_context[idx], "user")
            && idx + 1 < recent_chat_context.len()
            && is_chat_role(&recent_chat_context[idx + 1], "assistant")
        {
            indices.push(idx + 1);
        }

        let score = indices
            .iter()
            .filter_map(|index| {
                let message = recent_chat_context.get(*index)?;
                let role_weight = if is_chat_role(message, "user") { 2 } else { 1 };
                let content = chat_content(message)?;
                Some(query_overlap_score(content, query_terms) * role_weight)
            })
            .sum();
        if score > 0 {
            candidates.push(RelevantPriorTurnPair {
                score,
                first_index: idx,
                indices,
            });
        }

        idx += if is_chat_role(&recent_chat_context[idx], "user")
            && idx + 1 < recent_chat_context.len()
            && is_chat_role(&recent_chat_context[idx + 1], "assistant")
        {
            2
        } else {
            1
        };
    }

    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.first_index.cmp(&right.first_index))
    });
    candidates
}

fn try_add_index_group(
    selected: &mut std::collections::BTreeSet<usize>,
    indices: &[usize],
    limit: usize,
) -> bool {
    let missing = indices
        .iter()
        .filter(|index| !selected.contains(index))
        .count();
    if selected.len().saturating_add(missing) > limit {
        return false;
    }
    selected.extend(indices.iter().copied());
    true
}

fn is_chat_role(message: &ConversationMessage, role: &str) -> bool {
    matches!(message, ConversationMessage::Chat(chat) if chat.role == role)
}

fn chat_content(message: &ConversationMessage) -> Option<&str> {
    match message {
        ConversationMessage::Chat(chat) => Some(chat.content.as_str()),
        _ => None,
    }
}

fn query_overlap_score(
    content: &str,
    query_terms: &std::collections::BTreeMap<String, usize>,
) -> usize {
    let content_terms = lexical_term_set(content);
    query_terms
        .iter()
        .filter_map(|(term, weight)| content_terms.contains(term).then_some(*weight))
        .sum()
}

fn weighted_query_terms(
    query: &str,
    recent_chat_context: &[ConversationMessage],
) -> std::collections::BTreeMap<String, usize> {
    let query_terms = lexical_term_set(query);
    if query_terms.is_empty() {
        return std::collections::BTreeMap::new();
    }

    let frequencies = term_document_frequencies(recent_chat_context);
    let document_count = recent_chat_context
        .iter()
        .filter(|message| chat_content(message).is_some())
        .count()
        .max(1);

    query_terms
        .into_iter()
        .filter_map(|term| {
            let frequency = frequencies.get(&term).copied().unwrap_or(0);
            (frequency > 0).then(|| {
                (
                    term,
                    inverse_document_frequency_weight(document_count, frequency),
                )
            })
        })
        .collect()
}

fn term_document_frequencies(
    recent_chat_context: &[ConversationMessage],
) -> std::collections::BTreeMap<String, usize> {
    let mut frequencies = std::collections::BTreeMap::<String, usize>::new();
    for content in recent_chat_context.iter().filter_map(chat_content) {
        for term in lexical_term_set(content) {
            *frequencies.entry(term).or_default() += 1;
        }
    }
    frequencies
}

fn inverse_document_frequency_weight(document_count: usize, frequency: usize) -> usize {
    document_count
        .saturating_mul(2)
        .saturating_div(frequency.max(1))
        .max(1)
}

fn lexical_term_set(text: &str) -> std::collections::BTreeSet<String> {
    let mut terms = std::collections::BTreeSet::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            current.extend(ch.to_lowercase());
        } else if is_relevant_query_term(&current) {
            terms.insert(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if is_relevant_query_term(&current) {
        terms.insert(current);
    }

    terms
}

fn is_relevant_query_term(term: &str) -> bool {
    term.chars().count() >= PROVIDER_CONTEXT_QUERY_MIN_TERM_CHARS
}

fn prune_system_messages_for_pressure(
    system_messages: Vec<ChatMessage>,
    prior_chat_messages: &[ChatMessage],
    current_turn_messages: &[ChatMessage],
    target_profile: Option<&ResolvedModelProfile>,
) -> Vec<ChatMessage> {
    let stats = provider_context_stats_for_parts(
        &system_messages,
        prior_chat_messages,
        current_turn_messages,
    );
    let mut budget_input = provider_context_budget_input_from_stats(&stats);
    if let Some(profile) = target_profile {
        budget_input = budget_input.with_target_model_profile(profile);
    }
    let policy = provider_context_prune_policy(budget_input);
    if !policy.drop_scoped_context && policy.max_runtime_interpretation_chars.is_none() {
        return system_messages;
    }

    system_messages
        .into_iter()
        .filter_map(|mut message| {
            if policy.drop_scoped_context && message.content.starts_with("[scoped-context]\n") {
                return None;
            }
            if let Some(max_chars) = policy.max_runtime_interpretation_chars {
                if message.content.starts_with("[runtime-interpretation]\n") {
                    message.content =
                        compact_runtime_interpretation_for_pressure(&message.content, max_chars);
                }
            }
            Some(message)
        })
        .collect()
}

fn provider_context_stats_for_parts(
    system_messages: &[ChatMessage],
    prior_chat_messages: &[ChatMessage],
    current_turn_messages: &[ChatMessage],
) -> ProviderPromptContextStats {
    let system_breakdown = system_message_breakdown_from_chat_messages(system_messages);
    let bootstrap_chars = lookup_section_chars(&system_breakdown, "bootstrap");
    let core_memory_chars = lookup_section_chars(&system_breakdown, "core_memory");
    let runtime_interpretation_chars =
        lookup_section_chars(&system_breakdown, "runtime_interpretation");
    let scoped_context_chars = lookup_section_chars(&system_breakdown, "scoped_context");
    let resolution_chars = lookup_section_chars(&system_breakdown, "resolution");
    let dynamic_system_chars =
        core_memory_chars + runtime_interpretation_chars + scoped_context_chars + resolution_chars;
    let system_chars = total_message_chars(system_messages);
    let prior_chat_chars = total_message_chars(prior_chat_messages);
    let current_turn_chars = total_message_chars(current_turn_messages);

    ProviderPromptContextStats {
        system_messages: system_messages.len(),
        system_chars,
        bootstrap_chars,
        core_memory_chars,
        runtime_interpretation_chars,
        scoped_context_chars,
        resolution_chars,
        dynamic_system_chars,
        stable_system_chars: bootstrap_chars,
        prior_chat_messages: prior_chat_messages.len(),
        prior_chat_chars,
        current_turn_messages: current_turn_messages.len(),
        current_turn_chars,
        total_messages: system_messages.len()
            + prior_chat_messages.len()
            + current_turn_messages.len(),
        total_chars: system_chars
            .saturating_add(prior_chat_chars)
            .saturating_add(current_turn_chars),
    }
}

fn provider_context_budget_input_from_stats(
    stats: &ProviderPromptContextStats,
) -> ProviderContextBudgetInput {
    ProviderContextBudgetInput {
        total_chars: stats.total_chars,
        prior_chat_messages: stats.prior_chat_messages,
        current_turn_messages: stats.current_turn_messages,
        target_context_window_tokens: None,
        target_max_output_tokens: None,
        bootstrap_chars: stats.bootstrap_chars,
        core_memory_chars: stats.core_memory_chars,
        runtime_interpretation_chars: stats.runtime_interpretation_chars,
        scoped_context_chars: stats.scoped_context_chars,
        resolution_chars: stats.resolution_chars,
        prior_chat_chars: stats.prior_chat_chars,
        current_turn_chars: stats.current_turn_chars,
    }
}

fn compact_runtime_interpretation_for_pressure(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let marker = "[runtime-interpretation]\n";
    let Some(body) = content.strip_prefix(marker) else {
        return truncate_chars(content, max_chars);
    };

    let blocks = body
        .trim()
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>();
    let priority_markers = [
        "[user-profile]",
        "[configured-runtime]",
        "[working-state]",
        "[current-conversation]",
        "[bounded-interpretation]",
    ];
    let mut selected = Vec::new();
    for marker in priority_markers {
        if let Some(block) = blocks.iter().find(|block| block.starts_with(marker)) {
            if !selected.iter().any(|existing| existing == block) {
                selected.push(*block);
            }
        }
    }
    if selected.is_empty() {
        selected.extend(blocks.iter().take(1).copied());
    }

    let mut output = marker.to_string();
    for block in selected {
        let separator = if output.ends_with('\n') { "" } else { "\n\n" };
        let candidate = format!("{output}{separator}{block}");
        if candidate.chars().count() <= max_chars {
            output = candidate;
            continue;
        }
        let remaining = max_chars.saturating_sub(output.chars().count());
        if remaining > 16 {
            output.push_str(separator);
            output.push_str(&truncate_chars(block, remaining));
        }
        break;
    }

    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
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
            media_artifacts,
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
            media_artifacts: media_artifacts.clone(),
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
            media_artifacts: Vec::new(),
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

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, None);
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
        assert!(snapshot.stats.current_turn_messages < history.len() - 1);
    }

    #[test]
    fn leaves_single_tool_cycle_uncompacted() {
        let history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::user("send the report")),
            tool_call("call-1", "message_send"),
            tool_result("call-1", "done"),
        ];

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, None);
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

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, None);
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

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, None);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(COMPACTION_SUMMARY_PREFIX));
        assert!(rendered.contains("assistant 7"));
    }

    #[test]
    fn provider_snapshot_preserves_relevant_current_session_anchor_pairs() {
        let mut history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::user(
                "Early anchor: meaning needs both freedom and responsibility.",
            )),
            ConversationMessage::Chat(ChatMessage::assistant(
                "The early anchor is freedom plus responsibility.",
            )),
        ];

        for idx in 2..12 {
            history.push(ConversationMessage::Chat(ChatMessage::user(format!(
                "Philosophy turn {idx}: continue the ordinary reflection."
            ))));
            history.push(ConversationMessage::Chat(ChatMessage::assistant(format!(
                "Reflection filler {idx}: this comes from that thread, but it is generic."
            ))));
        }

        history.push(ConversationMessage::Chat(ChatMessage::user(
            "Late anchor: joy is not proof of truth, but it can be evidence of alignment.",
        )));
        history.push(ConversationMessage::Chat(ChatMessage::assistant(
            "The late anchor is joy as evidence of alignment.",
        )));
        history.push(ConversationMessage::Chat(ChatMessage::user(
            "Philosophy turn 20: continue with the clay metaphor.",
        )));
        history.push(ConversationMessage::Chat(ChatMessage::assistant(
            "The clay metaphor is only the newest tail turn.",
        )));
        history.push(ConversationMessage::Chat(ChatMessage::user(
            "Compare the early anchor and the late anchor from this long dialogue.",
        )));

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, None);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("freedom"));
        assert!(rendered.contains("responsibility"));
        assert!(rendered.contains("joy"));
        assert!(rendered.contains("alignment"));
        assert!(rendered.contains("clay metaphor"));
        assert!(rendered.contains("Compare the early anchor"));
        assert!(!rendered.contains("Reflection filler 8."));
        assert_eq!(snapshot.stats.prior_chat_messages, 6);
    }

    #[test]
    fn pressure_pruning_drops_scoped_context_and_compacts_runtime_interpretation() {
        let scoped = format!("[scoped-context]\n{}\n", "scoped rules\n".repeat(260));
        let runtime = format!(
            concat!(
                "[runtime-interpretation]\n",
                "[user-profile]\n",
                "- response_locale: ru\n\n",
                "[working-state]\n",
                "{}\n\n",
                "[bounded-interpretation]\n",
                "{}\n"
            ),
            "workspace=old\n".repeat(160),
            "candidate=stale\n".repeat(160)
        );
        let history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::system(runtime)),
            ConversationMessage::Chat(ChatMessage::system(scoped)),
            ConversationMessage::Chat(ChatMessage::user("reply briefly")),
        ];

        let snapshot = build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, None);
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("[scoped-context]"));
        assert!(rendered.contains("[runtime-interpretation]"));
        assert!(rendered.contains("[user-profile]"));
        assert!(snapshot.stats.scoped_context_chars == 0);
        assert!(snapshot.stats.runtime_interpretation_chars <= 430);
    }

    #[test]
    fn pressure_pruning_respects_large_window_profile() {
        let scoped = format!("[scoped-context]\n{}\n", "scoped rules\n".repeat(260));
        let runtime = format!(
            concat!(
                "[runtime-interpretation]\n",
                "[user-profile]\n",
                "- response_locale: ru\n\n",
                "[working-state]\n",
                "{}\n\n",
                "[bounded-interpretation]\n",
                "{}\n"
            ),
            "workspace=old\n".repeat(160),
            "candidate=stale\n".repeat(160)
        );
        let profile = ResolvedModelProfile {
            context_window_tokens: Some(262_144),
            max_output_tokens: Some(131_072),
            context_window_source:
                synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource::BundledCatalog,
            max_output_source:
                synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource::BundledCatalog,
            ..Default::default()
        };
        let history = vec![
            ConversationMessage::Chat(ChatMessage::system("bootstrap")),
            ConversationMessage::Chat(ChatMessage::system(runtime)),
            ConversationMessage::Chat(ChatMessage::system(scoped)),
            ConversationMessage::Chat(ChatMessage::user("reply briefly")),
        ];

        let snapshot =
            build_provider_prompt_snapshot(&NativeToolDispatcher, &history, 6, Some(&profile));
        let rendered = snapshot
            .messages
            .iter()
            .map(|msg| msg.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("[scoped-context]"));
        assert!(snapshot.stats.scoped_context_chars > 0);
        assert!(snapshot.stats.runtime_interpretation_chars > 430);
    }
}
