//! Memory mutation service — AUDN-lite decision engine.
//!
//! Evaluates mutation candidates against existing memory and decides
//! whether to Add, Update, Delete, or Noop. This service is the single
//! point where long-term memory mutation policy lives.
//!
//! Design:
//! - Accepts candidates from consolidation, explicit user signals, or tools.
//! - Fetches a small shortlist of similar existing memories.
//! - Applies deterministic similarity thresholds (not per-fact LLM calls).
//! - Emits decisions that callers apply through memory ports.

use crate::domain::memory::{MemoryCategory, MemoryError};
use crate::domain::memory_mutation::{
    MutationAction, MutationCandidate, MutationDecision, MutationSource, MutationThresholds,
};
use crate::ports::memory::UnifiedMemoryPort;

/// Evaluate a single mutation candidate against existing memory.
///
/// Returns a `MutationDecision` with the recommended action.
/// The caller is responsible for applying the action via memory ports.
pub async fn evaluate_candidate(
    mem: &dyn UnifiedMemoryPort,
    candidate: MutationCandidate,
    _agent_id: &str,
    thresholds: &MutationThresholds,
) -> MutationDecision {
    // Fetch shortlist of similar existing memories
    let existing = match mem
        .recall(&candidate.text, 3, None)
        .await
    {
        Ok(entries) => entries,
        Err(_) => {
            // If recall fails, default to Add (safe — no data loss)
            tracing::warn!(target: "memory_mutation", "Recall failed during mutation eval, defaulting to Add");
            return MutationDecision {
                action: MutationAction::Add,
                candidate,
                reason: "recall failed, defaulting to add".into(),
                similarity: None,
            };
        }
    };

    if existing.is_empty() {
        tracing::debug!(target: "memory_mutation", action = "add", reason = "no similar entries", "Mutation decided");
        return MutationDecision {
            action: MutationAction::Add,
            candidate,
            reason: "no similar existing memories".into(),
            similarity: None,
        };
    }

    // Find the closest match
    let best = existing
        .iter()
        .filter(|e| e.score.is_some())
        .max_by(|a, b| {
            a.score
                .unwrap_or(0.0)
                .partial_cmp(&b.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    let (best_entry, best_score) = match best {
        Some(e) => (e, e.score.unwrap_or(0.0)),
        None => {
            tracing::debug!(target: "memory_mutation", action = "add", reason = "no scored entries", "Mutation decided");
            return MutationDecision {
                action: MutationAction::Add,
                candidate,
                reason: "no scored existing memories".into(),
                similarity: None,
            };
        }
    };

    // Apply AUDN thresholds
    let (action, reason) = if best_score >= thresholds.noop_threshold {
        (MutationAction::Noop, format!(
            "near-duplicate (score {best_score:.3}), existing: {}",
            truncate(&best_entry.content, 80)
        ))
    } else if best_score >= thresholds.update_threshold
        && same_category(&candidate.category, &best_entry.category)
    {
        // Same category + high similarity = update/replace
        if is_contradictory(&candidate.text, &best_entry.content) {
            (
                MutationAction::Delete {
                    target_id: best_entry.id.clone(),
                },
                format!(
                    "contradictory (score {best_score:.3}), replacing: {}",
                    truncate(&best_entry.content, 80)
                ),
            )
        } else {
            (
                MutationAction::Update {
                    target_id: best_entry.id.clone(),
                },
                format!(
                    "supersedes (score {best_score:.3}), updating: {}",
                    truncate(&best_entry.content, 80)
                ),
            )
        }
    } else {
        (MutationAction::Add, format!(
            "sufficiently distinct (best score {best_score:.3})"
        ))
    };

    tracing::debug!(
        target: "memory_mutation",
        action = ?action,
        score = best_score,
        source = ?candidate.source,
        "Mutation decided"
    );

    MutationDecision {
        action,
        candidate,
        reason,
        similarity: Some(best_score),
    }
}

/// Evaluate multiple candidates from a single turn.
///
/// Returns decisions for all candidates. Noop decisions are included
/// so callers can log/trace them.
pub async fn evaluate_candidates(
    mem: &dyn UnifiedMemoryPort,
    candidates: Vec<MutationCandidate>,
    agent_id: &str,
    thresholds: &MutationThresholds,
) -> Vec<MutationDecision> {
    let mut decisions = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let decision = evaluate_candidate(mem, candidate, agent_id, thresholds).await;
        decisions.push(decision);
    }
    decisions
}

/// Apply a mutation decision through memory ports.
///
/// Returns `true` if a write was made, `false` for Noop.
pub async fn apply_decision(
    mem: &dyn UnifiedMemoryPort,
    decision: &MutationDecision,
    agent_id: &str,
) -> Result<bool, MemoryError> {
    match &decision.action {
        MutationAction::Add => {
            let key = format!(
                "{}_{}_{}",
                category_prefix(&decision.candidate.category),
                &uuid::Uuid::new_v4().to_string()[..8],
                source_tag(&decision.candidate.source)
            );
            mem.store(
                &key,
                &decision.candidate.text,
                &decision.candidate.category,
                None,
            )
            .await?;
            tracing::debug!(target: "memory_mutation", key, "Memory added");
            Ok(true)
        }
        MutationAction::Update { target_id } => {
            // Delete old, add new (atomic upsert not available via ports)
            let _ = mem.forget(target_id, &agent_id.to_string()).await;
            let key = format!(
                "{}_{}_{}",
                category_prefix(&decision.candidate.category),
                &uuid::Uuid::new_v4().to_string()[..8],
                source_tag(&decision.candidate.source)
            );
            mem.store(
                &key,
                &decision.candidate.text,
                &decision.candidate.category,
                None,
            )
            .await?;
            tracing::debug!(target: "memory_mutation", old_id = target_id, key, "Memory updated");
            Ok(true)
        }
        MutationAction::Delete { target_id } => {
            mem.forget(target_id, &agent_id.to_string()).await?;
            tracing::debug!(target: "memory_mutation", target_id, "Memory deleted");
            Ok(true)
        }
        MutationAction::Noop => {
            tracing::trace!(target: "memory_mutation", reason = %decision.reason, "Mutation skipped (noop)");
            Ok(false)
        }
    }
}

/// Apply a mutation decision and return a learning event.
///
/// Combines `apply_decision` with `LearningEvent` emission.
pub async fn apply_decision_with_event(
    mem: &dyn UnifiedMemoryPort,
    decision: &MutationDecision,
    agent_id: &str,
) -> Result<super::learning_events::LearningEvent, MemoryError> {
    let _wrote = apply_decision(mem, decision, agent_id).await?;
    let event = super::learning_events::LearningEvent::from_mutation(
        &decision.action,
        agent_id,
        match &decision.action {
            MutationAction::Update { target_id } | MutationAction::Delete { target_id } => {
                Some(target_id.as_str())
            }
            _ => None,
        },
        &decision.reason,
    );
    Ok(event)
}

// ── Helpers ──────────────────────────────────────────────────────

fn same_category(a: &MemoryCategory, b: &MemoryCategory) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Simple heuristic: if both texts share subject but differ in predicate/value,
/// they may be contradictory. This avoids an LLM call for common cases.
fn is_contradictory(new: &str, old: &str) -> bool {
    let new_lower = new.to_lowercase();
    let old_lower = old.to_lowercase();
    // Negation patterns
    let negation_pairs = [
        ("prefers", "does not prefer"),
        ("likes", "dislikes"),
        ("uses", "does not use"),
        ("wants", "does not want"),
        ("is", "is not"),
    ];
    for (pos, neg) in &negation_pairs {
        if (new_lower.contains(pos) && old_lower.contains(neg))
            || (new_lower.contains(neg) && old_lower.contains(pos))
        {
            return true;
        }
    }
    false
}

fn category_prefix(cat: &MemoryCategory) -> &'static str {
    match cat {
        MemoryCategory::Core => "core",
        MemoryCategory::Daily => "daily",
        MemoryCategory::Conversation => "conv",
        MemoryCategory::Entity => "entity",
        MemoryCategory::Skill => "skill",
        MemoryCategory::Reflection => "refl",
        MemoryCategory::Custom(_) => "custom",
    }
}

fn source_tag(source: &MutationSource) -> &'static str {
    match source {
        MutationSource::Consolidation => "cons",
        MutationSource::ExplicitUser => "user",
        MutationSource::ToolOutput => "tool",
        MutationSource::Reflection => "refl",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_category_matches() {
        assert!(same_category(&MemoryCategory::Core, &MemoryCategory::Core));
        assert!(!same_category(
            &MemoryCategory::Core,
            &MemoryCategory::Daily
        ));
    }

    #[test]
    fn contradiction_detection() {
        assert!(is_contradictory(
            "User prefers Python",
            "User does not prefer Python"
        ));
        assert!(is_contradictory("User likes Rust", "User dislikes Rust"));
        assert!(!is_contradictory("User likes Rust", "User likes Python"));
    }

    #[test]
    fn category_prefix_mapping() {
        assert_eq!(category_prefix(&MemoryCategory::Core), "core");
        assert_eq!(category_prefix(&MemoryCategory::Daily), "daily");
        assert_eq!(category_prefix(&MemoryCategory::Skill), "skill");
    }

    #[test]
    fn source_tag_mapping() {
        assert_eq!(source_tag(&MutationSource::Consolidation), "cons");
        assert_eq!(source_tag(&MutationSource::ExplicitUser), "user");
    }
}
