//! Provider-history hygiene helpers shared by runtime adapters.

use synapse_providers::ChatMessage;

use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile;
use synapse_domain::application::services::route_switch_preflight::{
    assess_route_switch_preflight_for_history, RouteSwitchPreflightResolution,
};
use synapse_domain::config::schema::{
    ContextCompressionConfig, ContextCompressionRouteOverrideConfig,
};
use synapse_domain::ports::conversation_history::ConversationHistoryPort;
use synapse_domain::ports::history_compaction_cache::HistoryCompactionCachePort;
use synapse_domain::ports::route_selection::{ContextCacheStats, RouteSelection};

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

pub(crate) fn resolve_route_switch_preflight_for_history_port(
    history: &dyn ConversationHistoryPort,
    conversation_key: &str,
    target_profile: &ResolvedModelProfile,
    keep_non_system_turns: usize,
) -> RouteSwitchPreflightResolution {
    let current_history = history.get_history(conversation_key);
    let mut resolution = RouteSwitchPreflightResolution::new(
        assess_route_switch_preflight_for_history(&current_history, target_profile),
    );

    while resolution.should_attempt_compaction() {
        if !history.compact_history(conversation_key, keep_non_system_turns) {
            break;
        }
        let compacted_history = history.get_history(conversation_key);
        resolution.record_compaction_pass(assess_route_switch_preflight_for_history(
            &compacted_history,
            target_profile,
        ));
    }

    resolution
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
