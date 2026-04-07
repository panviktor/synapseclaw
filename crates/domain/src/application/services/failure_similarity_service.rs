//! Category-aware failure-pattern similarity and merge policy.
//!
//! Failure memories should not accumulate as raw duplicates when the same
//! operational failure repeats. This mirrors the precedent similarity path but
//! stays scoped to the `failure_pattern` category.

use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryQuery};
use crate::domain::memory_mutation::{MutationAction, MutationCandidate, MutationDecision};
use crate::ports::memory::UnifiedMemoryPort;

#[derive(Debug, Clone)]
pub struct FailureSimilarityThresholds {
    pub shortlist_limit: usize,
    pub noop_threshold: f64,
    pub update_threshold: f64,
}

impl Default for FailureSimilarityThresholds {
    fn default() -> Self {
        Self {
            shortlist_limit: 8,
            noop_threshold: 0.96,
            update_threshold: 0.84,
        }
    }
}

pub async fn evaluate_failure_candidate(
    mem: &dyn UnifiedMemoryPort,
    candidate: MutationCandidate,
    agent_id: &str,
    thresholds: &FailureSimilarityThresholds,
) -> MutationDecision {
    let existing =
        fetch_failure_shortlist(mem, &candidate.text, agent_id, thresholds.shortlist_limit)
            .await
            .unwrap_or_default();
    decide_failure_mutation(candidate, &existing, thresholds)
}

async fn fetch_failure_shortlist(
    mem: &dyn UnifiedMemoryPort,
    query_text: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, crate::domain::memory::MemoryError> {
    let query = MemoryQuery {
        text: query_text.to_string(),
        embedding: None,
        agent_id: agent_id.to_string(),
        categories: vec![MemoryCategory::Custom("failure_pattern".into())],
        include_shared: true,
        time_range: None,
        limit: limit.saturating_mul(2).max(limit),
    };
    let mut episodes = mem.hybrid_search(&query).await?.episodes;
    episodes.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(episodes
        .into_iter()
        .take(limit)
        .map(|result| {
            let mut entry = result.entry;
            entry.score = Some(result.score as f64);
            entry
        })
        .collect())
}

pub fn decide_failure_mutation(
    candidate: MutationCandidate,
    existing: &[MemoryEntry],
    thresholds: &FailureSimilarityThresholds,
) -> MutationDecision {
    let best = existing
        .iter()
        .filter(|entry| is_failure_category(&entry.category))
        .filter(|entry| entry.score.is_some())
        .max_by(|left, right| {
            left.score
                .unwrap_or(0.0)
                .partial_cmp(&right.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    let Some(best) = best else {
        return MutationDecision {
            action: MutationAction::Add,
            candidate,
            reason: "no similar failure patterns".into(),
            similarity: None,
        };
    };

    let best_score = best.score.unwrap_or(0.0);
    let (action, reason) = if best_score >= thresholds.noop_threshold {
        (
            MutationAction::Noop,
            format!(
                "near-duplicate failure pattern (score {best_score:.3}), existing: {}",
                truncate(&best.content, 80)
            ),
        )
    } else if best_score >= thresholds.update_threshold {
        (
            MutationAction::Update {
                target_id: best.key.clone(),
            },
            format!(
                "supersedes similar failure pattern (score {best_score:.3}), updating: {}",
                truncate(&best.content, 80)
            ),
        )
    } else {
        (
            MutationAction::Add,
            format!("distinct failure pattern (best score {best_score:.3})"),
        )
    };

    MutationDecision {
        action,
        candidate,
        reason,
        similarity: Some(best_score),
    }
}

fn is_failure_category(category: &MemoryCategory) -> bool {
    matches!(category, MemoryCategory::Custom(name) if name == "failure_pattern")
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let truncated = value.chars().take(max).collect::<String>();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::memory_mutation::MutationSource;

    fn candidate() -> MutationCandidate {
        MutationCandidate {
            category: MemoryCategory::Custom("failure_pattern".into()),
            text: "failed_tools=web_fetch | outcomes=runtime_error | subjects=status.example.com"
                .into(),
            confidence: 0.74,
            source: MutationSource::Reflection,
        }
    }

    #[test]
    fn no_matching_failure_pattern_adds_new_entry() {
        let decision =
            decide_failure_mutation(candidate(), &[], &FailureSimilarityThresholds::default());

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("no similar failure patterns"));
    }

    #[test]
    fn near_duplicate_failure_pattern_becomes_noop() {
        let decision = decide_failure_mutation(
            candidate(),
            &[MemoryEntry {
                id: "1".into(),
                key: "custom_failure_1".into(),
                content:
                    "failed_tools=web_fetch | outcomes=runtime_error | subjects=status.example.com"
                        .into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.98),
            }],
            &FailureSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Noop));
    }

    #[test]
    fn similar_failure_pattern_updates_existing_entry() {
        let decision = decide_failure_mutation(
            candidate(),
            &[MemoryEntry {
                id: "1".into(),
                key: "custom_failure_1".into(),
                content: "failed_tools=web_fetch | outcomes=runtime_error".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.87),
            }],
            &FailureSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Update { .. }));
    }
}
