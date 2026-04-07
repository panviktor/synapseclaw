//! Post-turn learning orchestrator — single source of truth for learning policy.
//!
//! Both web and channel paths call `execute_post_turn_learning()` instead of
//! implementing their own spawn/decide/mutate logic. This eliminates policy
//! divergence between transport adapters.

use crate::application::services::learning_events::LearningEvent;
use crate::application::services::learning_candidate_service::{
    self, LearningCandidate,
};
use crate::application::services::learning_evidence_service::{
    self, LearningEvidenceEnvelope,
};
use crate::application::services::learning_signals::{self, LearningSignal};
use crate::application::services::memory_mutation as mutation;
use crate::domain::memory::MemoryCategory;
use crate::domain::memory_mutation::{MutationCandidate, MutationSource, MutationThresholds};
use crate::domain::tool_fact::TypedToolFact;
use crate::ports::memory::UnifiedMemoryPort;

// ── Gate constants ───────────────────────────────────────────────

/// Minimum user message length (chars) for background consolidation.
const CONSOLIDATE_MIN_CHARS: usize = 20;

/// Minimum user message length (chars) for reflection.
const REFLECT_MIN_USER_CHARS: usize = 30;

/// Minimum response length (bytes) for reflection.
const REFLECT_MIN_RESPONSE_LEN: usize = 200;

// ── Input / Output ───────────────────────────────────────────────

/// Everything the orchestrator needs to decide and execute post-turn learning.
pub struct PostTurnInput {
    pub agent_id: String,
    pub user_message: String,
    pub assistant_response: String,
    pub tools_used: Vec<String>,
    pub tool_facts: Vec<TypedToolFact>,
    pub auto_save_enabled: bool,
    /// Optional SSE event sender for publishing reports to UI.
    /// Both web and channels should pass this if available.
    pub event_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
}

/// What the orchestrator did — returned to the transport adapter for logging/UI.
#[derive(Debug)]
pub struct PostTurnReport {
    /// Detected learning signal from user message.
    pub signal: LearningSignal,
    /// Learning event from explicit AUDN mutation (if any).
    pub explicit_mutation: Option<LearningEvent>,
    /// Whether background consolidation was started.
    pub consolidation_started: bool,
    /// Whether skill reflection was started.
    pub reflection_started: bool,
    /// Cheap typed evidence from this turn.
    pub learning_evidence: LearningEvidenceEnvelope,
    /// Structured low-cost candidates for downstream learning.
    pub learning_candidates: Vec<LearningCandidate>,
    /// Applied mutation events derived from low-cost learning candidates.
    pub candidate_mutations: Vec<LearningEvent>,
}

// ── Orchestrator ─────────────────────────────────────────────────

/// Execute all post-turn learning in one place.
///
/// This is the **single source of truth** for learning policy.
/// Web and channels are pure transport — they call this and log the report.
pub async fn execute_post_turn_learning(
    mem: &dyn UnifiedMemoryPort,
    input: PostTurnInput,
) -> PostTurnReport {
    // Load signal patterns from memory port — unified for all transports.
    let patterns = mem.list_signal_patterns().await.unwrap_or_default();
    let signal = learning_signals::classify_signal_with_patterns(&input.user_message, &patterns);
    let user_chars = input.user_message.chars().count();
    let learning_evidence = learning_evidence_service::build_learning_evidence(&input.tool_facts);
    let learning_candidates = learning_candidate_service::build_learning_candidates(
        &input.user_message,
        &input.assistant_response,
        &input.tools_used,
        &input.tool_facts,
        &learning_evidence,
    );
    let mutation_candidates = learning_candidate_service::build_mutation_candidates(&learning_candidates);

    let mut report = PostTurnReport {
        signal: signal.clone(),
        explicit_mutation: None,
        consolidation_started: false,
        reflection_started: false,
        learning_evidence: learning_evidence.clone(),
        learning_candidates: learning_candidates.clone(),
        candidate_mutations: Vec::new(),
    };

    // ── 1. Explicit hot-path: direct AUDN mutation ──
    if signal.is_explicit() {
        let candidate = MutationCandidate {
            category: MemoryCategory::Core,
            text: input.user_message.clone(),
            confidence: signal.confidence(),
            source: MutationSource::ExplicitUser,
        };
        let decision = mutation::evaluate_candidate(
            mem,
            candidate,
            &input.agent_id,
            &MutationThresholds::default(),
        )
        .await;
        match mutation::apply_decision_with_event(mem, &decision, &input.agent_id).await {
            Ok(event) => {
                tracing::debug!(
                    target: "post_turn",
                    kind = ?event.kind,
                    agent_id = %input.agent_id,
                    "Explicit learning event"
                );
                report.explicit_mutation = Some(event);
            }
            Err(e) => {
                tracing::warn!(
                    target: "post_turn",
                    error = %e,
                    "Explicit mutation failed"
                );
            }
        }
    }

    // ── 1b. Cheap typed candidate mutation path ──
    if !signal.is_explicit() && input.auto_save_enabled && !mutation_candidates.is_empty() {
        let decisions = mutation::evaluate_candidates(
            mem,
            mutation_candidates,
            &input.agent_id,
            &MutationThresholds::default(),
        )
        .await;
        for decision in decisions {
            match mutation::apply_decision_with_event(mem, &decision, &input.agent_id).await {
                Ok(event) => report.candidate_mutations.push(event),
                Err(e) => {
                    tracing::warn!(
                        target: "post_turn",
                        error = %e,
                        "Typed candidate mutation failed"
                    );
                }
            }
        }
    }

    // ── 2. Background consolidation (only for non-explicit turns) ──
    let should_consolidate = !signal.is_explicit()
        && input.auto_save_enabled
        && (user_chars >= CONSOLIDATE_MIN_CHARS || learning_evidence.has_actionable_evidence());

    if should_consolidate {
        if let Err(e) = mem
            .consolidate_turn(&input.user_message, &input.assistant_response)
            .await
        {
            tracing::warn!(target: "post_turn", error = %e, "Consolidation failed");
        }
        report.consolidation_started = true;
    }

    // ── 3. Skill reflection ──
    let resp_lower = input.assistant_response.to_lowercase();
    let has_errors = resp_lower.contains("error") || resp_lower.contains("failed");
    let should_reflect = input.assistant_response.len() > REFLECT_MIN_RESPONSE_LEN
        && user_chars >= REFLECT_MIN_USER_CHARS
        && (!input.tools_used.is_empty() || learning_evidence.has_actionable_evidence() || has_errors);

    if should_reflect {
        if let Err(e) = mem
            .reflect_on_turn(
                &input.user_message,
                &input.assistant_response,
                &input.tools_used,
            )
            .await
        {
            tracing::warn!(target: "post_turn", error = %e, "Reflection failed");
        }
        report.reflection_started = true;
    }

    tracing::debug!(
        target: "post_turn",
        signal = ?report.signal,
        explicit = report.explicit_mutation.is_some(),
        consolidation = report.consolidation_started,
        reflection = report.reflection_started,
        "Post-turn learning complete"
    );

    // Publish to SSE event stream (if available) — unified for web + channels.
    if let Some(ref tx) = input.event_tx {
        let _ = tx.send(serde_json::json!({
            "type": "post_turn_report",
            "agent_id": input.agent_id,
            "signal": report.signal.as_str(),
            "explicit_mutation": report.explicit_mutation.is_some(),
            "explicit_kind": report.explicit_mutation.as_ref().map(|event| format!("{:?}", event.kind)),
            "consolidation_started": report.consolidation_started,
            "reflection_started": report.reflection_started,
            "typed_fact_count": report.learning_evidence.typed_fact_count,
            "learning_facets": report.learning_evidence.facets,
            "learning_candidate_count": report.learning_candidates.len(),
            "learning_candidates": report.learning_candidates,
            "candidate_mutation_count": report.candidate_mutations.len(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
        }));
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidation_gate_constants() {
        assert_eq!(CONSOLIDATE_MIN_CHARS, 20);
        assert_eq!(REFLECT_MIN_USER_CHARS, 30);
        assert_eq!(REFLECT_MIN_RESPONSE_LEN, 200);
    }
}
