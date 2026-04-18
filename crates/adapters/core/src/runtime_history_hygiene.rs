//! Provider-history hygiene helpers shared by runtime adapters.

use synapse_providers::ChatMessage;

use synapse_domain::application::services::history_compaction::{
    build_compaction_transcript, session_hygiene_dropped_messages,
    session_hygiene_dropped_messages_with_indices, HistoryCompressionPolicy,
};
use synapse_domain::application::services::memory_precompress_handoff::{
    execute_memory_precompress_handoff, is_precompress_preservation_message,
    precompress_preservation_message, MemoryPreCompressHandoffInput,
    MemoryPreCompressHandoffReason,
};
use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile;
use synapse_domain::application::services::provider_context_budget::{
    assess_provider_context_budget, provider_context_input_for_history, ProviderContextBudgetTier,
};
use synapse_domain::application::services::route_switch_preflight::{
    assess_route_switch_preflight_for_history, RouteSwitchPreflightResolution,
};
use synapse_domain::application::services::skill_trace_service::{
    parse_skill_activation_trace_entry, skill_activation_trace_memory_category,
};
use synapse_domain::config::schema::{
    ContextCompressionConfig, ContextCompressionRouteOverrideConfig,
};
use synapse_domain::ports::conversation_history::ConversationHistoryPort;
use synapse_domain::ports::history_compaction_cache::HistoryCompactionCachePort;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::route_selection::{ContextCacheStats, RouteSelection};
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;

pub(crate) fn normalize_cached_channel_turns(turns: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut normalized = Vec::with_capacity(turns.len());
    let mut expecting_user = true;

    for turn in turns {
        match (expecting_user, turn.role.as_str()) {
            (true, "user") => {
                normalized.push(turn);
                expecting_user = false;
            }
            (false, "assistant") => {
                normalized.push(turn);
                expecting_user = true;
            }
            // Interrupted runs can produce consecutive same-side messages.
            // Merge instead of dropping so provider history stays alternating.
            (false, "user") | (true, "assistant") => {
                if let Some(last_turn) = normalized.last_mut() {
                    if !turn.content.is_empty() {
                        if !last_turn.content.is_empty() {
                            last_turn.content.push_str("\n\n");
                        }
                        last_turn.content.push_str(&turn.content);
                    }
                }
            }
            _ => {}
        }
    }

    normalized
}

/// Proactively trim conversation turns to the given estimated character budget.
///
/// Drops oldest turns first, but always preserves the current/latest turn.
pub(crate) fn proactive_trim_turns(turns: &mut Vec<ChatMessage>, budget: usize) -> usize {
    let total_chars: usize = turns.iter().map(|turn| turn.content.chars().count()).sum();
    if total_chars <= budget || turns.len() <= 1 {
        return 0;
    }

    let mut excess = total_chars.saturating_sub(budget);
    let mut drop_count = 0;

    while excess > 0 && drop_count < turns.len().saturating_sub(1) {
        excess = excess.saturating_sub(turns[drop_count].content.chars().count());
        drop_count += 1;
    }

    if drop_count > 0 {
        turns.drain(..drop_count);
    }
    drop_count
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeCompactionSurface {
    Web,
    Channel,
}

impl RuntimeCompactionSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Channel => "channel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeManualCompactionOutcome {
    pub compacted: bool,
    pub before_messages: usize,
    pub after_messages: usize,
    pub keep_non_system_turns: usize,
    pub preservation_messages_added: usize,
    pub budget_tier: ProviderContextBudgetTier,
}

pub(crate) async fn compact_history_for_runtime_command(
    history: &dyn ConversationHistoryPort,
    conversation_key: &str,
    target_profile: &ResolvedModelProfile,
    memory: Option<&dyn UnifiedMemoryPort>,
    run_recipe_store: Option<&dyn RunRecipeStorePort>,
    agent_id: &str,
    surface: RuntimeCompactionSurface,
) -> RuntimeManualCompactionOutcome {
    let current_history = history.get_history(conversation_key);
    let before_messages = current_history.len();
    let budget = assess_provider_context_budget(
        provider_context_input_for_history(&current_history)
            .with_target_model_profile(target_profile),
    );
    let keep_non_system_turns = runtime_command_keep_non_system_turns(
        synapse_domain::application::services::history_compaction::SESSION_HYGIENE_KEEP_NON_SYSTEM_TURNS,
        budget.tier,
    );
    let preservation_message = precompress_session_hygiene_preservation_message(
        &current_history,
        keep_non_system_turns,
        memory,
        run_recipe_store,
        agent_id,
        MemoryPreCompressHandoffReason::ManualSessionCompaction,
    )
    .await;

    let compacted = history.compact_history(conversation_key, keep_non_system_turns);
    let preservation_messages_added = if compacted {
        preservation_message
            .as_ref()
            .map(|message| {
                append_missing_preservation_messages(
                    history,
                    conversation_key,
                    std::slice::from_ref(message),
                )
            })
            .unwrap_or(0)
    } else {
        0
    };
    let after_messages = history.get_history(conversation_key).len();
    tracing::info!(
        conversation_key,
        surface = surface.as_str(),
        before_messages,
        after_messages,
        keep_non_system_turns,
        preservation_messages_added,
        budget_tier = ?budget.tier,
        compacted,
        "Runtime command compact session complete"
    );
    RuntimeManualCompactionOutcome {
        compacted,
        before_messages,
        after_messages,
        keep_non_system_turns,
        preservation_messages_added,
        budget_tier: budget.tier,
    }
}

pub(crate) async fn precompress_session_hygiene_preservation_message(
    history: &[ChatMessage],
    keep_non_system_turns: usize,
    memory: Option<&dyn UnifiedMemoryPort>,
    run_recipe_store: Option<&dyn RunRecipeStorePort>,
    agent_id: &str,
    reason: MemoryPreCompressHandoffReason,
) -> Option<ChatMessage> {
    if history.is_empty() {
        return None;
    }
    let dropped = session_hygiene_dropped_messages_with_indices(history, keep_non_system_turns);
    if dropped.is_empty() {
        return None;
    }
    let dropped_messages = dropped
        .iter()
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>();
    let dropped_indices = dropped.iter().map(|entry| entry.index).collect::<Vec<_>>();
    let start_index = dropped_indices.first().copied().unwrap_or(0);
    let end_index = dropped_indices
        .last()
        .map(|index| index.saturating_add(1))
        .unwrap_or(start_index);
    let transcript = build_compaction_transcript(
        &dropped_messages,
        HistoryCompressionPolicy::default().max_source_chars,
    );
    let report = execute_memory_precompress_handoff(
        memory,
        MemoryPreCompressHandoffInput {
            agent_id,
            reason,
            start_index,
            end_index,
            transcript: &transcript,
            messages: &dropped_messages,
            message_indices: &dropped_indices,
            recent_tool_repairs: &[],
            run_recipe_store,
            observed_at_unix: chrono::Utc::now().timestamp(),
        },
    )
    .await;
    let mut preservation_hints = report.preservation_hints;
    preservation_hints.extend(active_skill_preservation_hints(memory, agent_id).await);
    dedupe_preservation_hints(&mut preservation_hints);
    precompress_preservation_message(&preservation_hints)
}

async fn active_skill_preservation_hints(
    memory: Option<&dyn UnifiedMemoryPort>,
    agent_id: &str,
) -> Vec<String> {
    let Some(memory) = memory else {
        return Vec::new();
    };
    let category = skill_activation_trace_memory_category();
    let Ok(entries) = memory.list(Some(&category), None, 16).await else {
        return Vec::new();
    };
    let mut hints = Vec::new();
    for trace in entries
        .iter()
        .filter_map(parse_skill_activation_trace_entry)
        .filter(|trace| !trace.loaded_skill_ids.is_empty() || !trace.selected_skill_ids.is_empty())
        .take(4)
    {
        let skill_ids = if trace.loaded_skill_ids.is_empty() {
            &trace.selected_skill_ids
        } else {
            &trace.loaded_skill_ids
        };
        let ids = skill_ids
            .iter()
            .filter(|id| !id.trim().is_empty())
            .take(4)
            .cloned()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let outcome = trace
            .outcome
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown");
        hints.push(format!(
            "active_skill_identity: agent={agent_id}; ids={}; outcome={outcome}; blocked_candidates={}",
            ids.join(","),
            trace.blocked_skill_ids.len()
        ));
    }
    hints
}

fn dedupe_preservation_hints(hints: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    hints.retain(|hint| seen.insert(hint.clone()));
}

fn runtime_command_keep_non_system_turns(
    base_keep: usize,
    budget_tier: ProviderContextBudgetTier,
) -> usize {
    match budget_tier {
        ProviderContextBudgetTier::Healthy => base_keep.saturating_mul(2).max(18),
        ProviderContextBudgetTier::Caution => base_keep.max(12),
        ProviderContextBudgetTier::OverBudget => base_keep.max(8),
    }
}

pub(crate) async fn resolve_route_switch_preflight_for_history_port(
    history: &dyn ConversationHistoryPort,
    conversation_key: &str,
    target_profile: &ResolvedModelProfile,
    keep_non_system_turns: usize,
    memory: Option<&dyn UnifiedMemoryPort>,
    run_recipe_store: Option<&dyn RunRecipeStorePort>,
    agent_id: &str,
) -> RouteSwitchPreflightResolution {
    let current_history = history.get_history(conversation_key);
    let mut resolution = RouteSwitchPreflightResolution::new(
        assess_route_switch_preflight_for_history(&current_history, target_profile),
    );
    let mut sticky_preservation_messages =
        precompress_preservation_messages_in_history(&current_history);

    while resolution.should_attempt_compaction() {
        let keep_non_system_turns = route_switch_keep_non_system_turns_for_pass(
            keep_non_system_turns,
            resolution.compaction_passes,
        );
        let preservation_message = precompress_route_switch_dropped_history(
            history,
            conversation_key,
            keep_non_system_turns,
            memory,
            run_recipe_store,
            agent_id,
        )
        .await;
        let had_preservation_message = preservation_message.is_some();
        if let Some(message) = preservation_message {
            push_unique_preservation_message(&mut sticky_preservation_messages, message);
        }
        if !history.compact_history(conversation_key, keep_non_system_turns) {
            let appended_sticky_preservation_messages = append_missing_preservation_messages(
                history,
                conversation_key,
                &sticky_preservation_messages,
            );
            tracing::info!(
                conversation_key,
                keep_non_system_turns,
                pass = resolution.compaction_passes,
                had_preservation_message,
                sticky_preservation_messages = sticky_preservation_messages.len(),
                appended_sticky_preservation_messages,
                "Route switch compaction made no history change"
            );
            resolution.record_compaction_attempt_without_change();
            continue;
        }
        let appended_sticky_preservation_messages = append_missing_preservation_messages(
            history,
            conversation_key,
            &sticky_preservation_messages,
        );
        let compacted_history = history.get_history(conversation_key);
        resolution.record_compaction_pass(assess_route_switch_preflight_for_history(
            &compacted_history,
            target_profile,
        ));
        tracing::info!(
            conversation_key,
            keep_non_system_turns,
            pass = resolution.compaction_passes,
            had_preservation_message,
            sticky_preservation_messages = sticky_preservation_messages.len(),
            appended_sticky_preservation_messages,
            status = ?resolution.preflight.status,
            estimated_context_tokens = resolution.preflight.estimated_context_tokens,
            "Route switch compaction pass complete"
        );
    }

    resolution
}

fn precompress_preservation_messages_in_history(history: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    for message in history {
        if is_precompress_preservation_message(message) {
            push_unique_preservation_message(&mut messages, message.clone());
        }
    }
    messages
}

fn push_unique_preservation_message(messages: &mut Vec<ChatMessage>, message: ChatMessage) {
    if messages
        .iter()
        .any(|existing| existing.content == message.content)
    {
        return;
    }
    messages.push(message);
}

fn append_missing_preservation_messages(
    history: &dyn ConversationHistoryPort,
    conversation_key: &str,
    messages: &[ChatMessage],
) -> usize {
    if messages.is_empty() {
        return 0;
    }
    let existing = history.get_history(conversation_key);
    let mut existing_contents = existing
        .iter()
        .filter(|message| is_precompress_preservation_message(message))
        .map(|message| message.content.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut appended = 0;
    for message in messages {
        if existing_contents.contains(message.content.as_str()) {
            continue;
        }
        history.append_turn(conversation_key, message.clone());
        existing_contents.insert(message.content.as_str());
        appended += 1;
    }
    appended
}

fn route_switch_keep_non_system_turns_for_pass(base_keep: usize, pass: usize) -> usize {
    let base_keep = base_keep.max(1);
    if pass == 0 {
        return base_keep;
    }
    let divisor = 2usize.saturating_pow(pass.min(usize::BITS as usize - 1) as u32);
    base_keep.saturating_div(divisor).max(2).min(base_keep)
}

async fn precompress_route_switch_dropped_history(
    history: &dyn ConversationHistoryPort,
    conversation_key: &str,
    keep_non_system_turns: usize,
    memory: Option<&dyn UnifiedMemoryPort>,
    run_recipe_store: Option<&dyn RunRecipeStorePort>,
    agent_id: &str,
) -> Option<ChatMessage> {
    let current_history = history.get_history(conversation_key);
    if current_history.is_empty() {
        return None;
    }
    let dropped_messages =
        session_hygiene_dropped_messages(&current_history, keep_non_system_turns);
    let preservation_message = precompress_session_hygiene_preservation_message(
        &current_history,
        keep_non_system_turns,
        memory,
        run_recipe_store,
        agent_id,
        MemoryPreCompressHandoffReason::ChannelSessionHygiene,
    )
    .await;
    tracing::info!(
        conversation_key,
        keep_non_system_turns,
        current_history_len = current_history.len(),
        dropped_len = dropped_messages.len(),
        hint_count = preservation_message.as_ref().map(|_| 1).unwrap_or(0),
        "Route switch pre-compress handoff inspected history"
    );
    preservation_message
}

pub(crate) async fn route_effective_context_cache_stats(
    compression: &ContextCompressionConfig,
    compression_overrides: &[ContextCompressionRouteOverrideConfig],
    history_compaction_cache: &dyn HistoryCompactionCachePort,
    route: &RouteSelection,
) -> ContextCacheStats {
    let compression =
        synapse_domain::application::services::history_compaction::resolve_context_compression_config_for_route(
            compression,
            compression_overrides,
            route.provider.as_str(),
            route.model.as_str(),
            route.lane,
            None,
        );
    if let Err(error) = history_compaction_cache.load(&compression).await {
        tracing::debug!(%error, "Failed to load visible history compaction cache");
    }
    history_compaction_cache.stats(&compression)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use synapse_domain::application::services::history_compaction::compact_provider_history_for_session_hygiene;
    use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfileSource;
    use synapse_domain::application::services::skill_governance_service::SkillActivationTrace;
    use synapse_domain::application::services::skill_trace_service::skill_activation_trace_to_memory_entry;
    use synapse_domain::domain::history_projection::format_projected_tool_call;
    use synapse_memory::EpisodicMemoryPort;

    #[derive(Default)]
    struct MockHistory {
        turns: Mutex<HashMap<String, Vec<ChatMessage>>>,
    }

    impl ConversationHistoryPort for MockHistory {
        fn has_history(&self, key: &str) -> bool {
            self.turns
                .lock()
                .unwrap()
                .get(key)
                .is_some_and(|turns| !turns.is_empty())
        }

        fn get_history(&self, key: &str) -> Vec<ChatMessage> {
            self.turns
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .unwrap_or_default()
        }

        fn append_turn(&self, key: &str, turn: ChatMessage) {
            self.turns
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_default()
                .push(turn);
        }

        fn clear_history(&self, key: &str) {
            self.turns.lock().unwrap().remove(key);
        }

        fn compact_history(&self, key: &str, keep_turns: usize) -> bool {
            self.turns
                .lock()
                .unwrap()
                .get_mut(key)
                .is_some_and(|history| {
                    compact_provider_history_for_session_hygiene(history, keep_turns)
                })
        }

        fn rollback_last_turn(&self, key: &str, expected_content: &str) -> bool {
            let mut turns = self.turns.lock().unwrap();
            let Some(history) = turns.get_mut(key) else {
                return false;
            };
            if history
                .last()
                .is_some_and(|message| message.content == expected_content)
            {
                history.pop();
                return true;
            }
            false
        }

        fn prepend_turn(&self, key: &str, turn: ChatMessage) {
            self.turns
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_default()
                .insert(0, turn);
        }
    }

    #[test]
    fn route_switch_keep_turns_tightens_by_pass() {
        assert_eq!(route_switch_keep_non_system_turns_for_pass(12, 0), 12);
        assert_eq!(route_switch_keep_non_system_turns_for_pass(12, 1), 6);
        assert_eq!(route_switch_keep_non_system_turns_for_pass(12, 2), 3);
        assert_eq!(route_switch_keep_non_system_turns_for_pass(12, 3), 2);
    }

    fn profile(context_window_tokens: usize) -> ResolvedModelProfile {
        ResolvedModelProfile {
            context_window_tokens: Some(context_window_tokens),
            max_output_tokens: Some(1_024),
            context_window_source: ResolvedModelProfileSource::ManualConfig,
            max_output_source: ResolvedModelProfileSource::ManualConfig,
            features_source: ResolvedModelProfileSource::ManualConfig,
            ..ResolvedModelProfile::default()
        }
    }

    fn non_system_count(history: &[ChatMessage]) -> usize {
        history
            .iter()
            .filter(|message| message.role != "system")
            .count()
    }

    #[tokio::test]
    async fn manual_compact_uses_soft_keep_when_context_is_healthy() {
        let history = MockHistory::default();
        let key = "manual-healthy";
        history.append_turn(key, ChatMessage::system("bootstrap"));
        for index in 0..30 {
            history.append_turn(key, ChatMessage::user(format!("turn {index}")));
        }

        let outcome = compact_history_for_runtime_command(
            &history,
            key,
            &profile(128_000),
            None,
            None,
            "agent",
            RuntimeCompactionSurface::Web,
        )
        .await;

        assert!(outcome.compacted);
        assert_eq!(outcome.keep_non_system_turns, 24);
        assert_eq!(non_system_count(&history.get_history(key)), 24);
    }

    #[tokio::test]
    async fn manual_compact_preserves_structural_handoff_without_duplicate_append() {
        let history = MockHistory::default();
        let key = "manual-handoff";
        for index in 0..2 {
            history.append_turn(key, ChatMessage::user(format!("old request {index}")));
        }
        history.append_turn(
            key,
            ChatMessage::assistant(format_projected_tool_call("a", "rg", "Matrix")),
        );
        history.append_turn(
            key,
            ChatMessage::assistant(format_projected_tool_call(
                "b",
                "systemctl",
                "show matrix-unit",
            )),
        );
        for index in 0..28 {
            history.append_turn(key, ChatMessage::user(format!("recent request {index}")));
        }

        let first = compact_history_for_runtime_command(
            &history,
            key,
            &profile(4_000),
            None,
            None,
            "agent",
            RuntimeCompactionSurface::Channel,
        )
        .await;
        let second = compact_history_for_runtime_command(
            &history,
            key,
            &profile(4_000),
            None,
            None,
            "agent",
            RuntimeCompactionSurface::Channel,
        )
        .await;
        let retained = history.get_history(key);
        let preservation_count = retained
            .iter()
            .filter(|message| is_precompress_preservation_message(message))
            .count();

        assert!(first.compacted);
        assert_eq!(first.preservation_messages_added, 1);
        assert_eq!(second.preservation_messages_added, 0);
        assert_eq!(preservation_count, 1);
    }

    #[tokio::test]
    async fn manual_compact_preserves_active_skill_identity_without_body() {
        let history = MockHistory::default();
        let key = "manual-skill-handoff";
        history.append_turn(key, ChatMessage::system("bootstrap"));
        for index in 0..30 {
            history.append_turn(key, ChatMessage::user(format!("turn {index}")));
        }

        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "agent".into(),
        )
        .await
        .unwrap();
        let trace = SkillActivationTrace {
            selected_skill_ids: vec!["matrix-upgrade".into()],
            loaded_skill_ids: vec!["matrix-upgrade".into()],
            blocked_skill_ids: Vec::new(),
            blocked_reasons: Vec::new(),
            budget_catalog_entries: 1,
            budget_preloaded_skills: 0,
            route_model: None,
            outcome: Some("loaded".into()),
        };
        let entry =
            skill_activation_trace_to_memory_entry("agent", &trace, chrono::Utc::now(), None)
                .unwrap();
        memory.store_episode(entry).await.unwrap();

        let outcome = compact_history_for_runtime_command(
            &history,
            key,
            &profile(4_000),
            Some(&memory),
            None,
            "agent",
            RuntimeCompactionSurface::Web,
        )
        .await;

        assert!(outcome.compacted);
        assert_eq!(outcome.preservation_messages_added, 1);
        let compacted = history.get_history(key);
        let handoff = compacted
            .iter()
            .find(|message| is_precompress_preservation_message(message))
            .expect("pre-compress handoff message");
        assert!(handoff.content.contains("active_skill_identity"));
        assert!(handoff.content.contains("matrix-upgrade"));
        assert!(!handoff.content.contains("Matrix release_status recipe"));
    }

    #[test]
    fn sticky_precompress_preservation_is_reappended_after_later_pass() {
        let history = MockHistory::default();
        let key = "session";
        let preservation = precompress_preservation_message(&[String::from(
            "stable_project_fact project=Atlas branch=release/hotfix-17 staging=https://staging.atlas.local",
        )])
        .expect("preservation message");

        history.append_turn(key, ChatMessage::user("old user"));
        history.append_turn(key, ChatMessage::assistant("old assistant"));
        let mut sticky = Vec::new();
        push_unique_preservation_message(&mut sticky, preservation.clone());

        assert_eq!(
            append_missing_preservation_messages(&history, key, &sticky),
            1
        );
        assert_eq!(
            append_missing_preservation_messages(&history, key, &sticky),
            0
        );
        assert!(history.compact_history(key, 1));
        assert_eq!(
            append_missing_preservation_messages(&history, key, &sticky),
            0
        );

        let retained = history.get_history(key);
        assert!(retained
            .iter()
            .any(|message| message.content == preservation.content));
    }
}
