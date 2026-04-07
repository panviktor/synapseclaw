//! Category-aware precedent similarity and merge policy.
//!
//! This is the first learning path that stops relying on the generic mutation
//! engine alone. Accepted precedent candidates should compare primarily against
//! existing precedent memories, not against all episodic memories.

use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryQuery};
use crate::domain::memory_mutation::{MutationAction, MutationCandidate, MutationDecision};
use crate::ports::memory::UnifiedMemoryPort;

#[derive(Debug, Clone)]
pub struct PrecedentSimilarityThresholds {
    pub shortlist_limit: usize,
    pub noop_threshold: f64,
    pub update_threshold: f64,
}

impl Default for PrecedentSimilarityThresholds {
    fn default() -> Self {
        Self {
            shortlist_limit: 8,
            noop_threshold: 0.95,
            update_threshold: 0.82,
        }
    }
}

pub async fn evaluate_precedent_candidate(
    mem: &dyn UnifiedMemoryPort,
    candidate: MutationCandidate,
    agent_id: &str,
    thresholds: &PrecedentSimilarityThresholds,
) -> MutationDecision {
    let existing = fetch_precedent_shortlist(mem, &candidate.text, agent_id, thresholds.shortlist_limit)
        .await
        .unwrap_or_default();
    decide_precedent_mutation(candidate, &existing, thresholds)
}

async fn fetch_precedent_shortlist(
    mem: &dyn UnifiedMemoryPort,
    query_text: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, crate::domain::memory::MemoryError> {
    let query = MemoryQuery {
        text: query_text.to_string(),
        embedding: None,
        agent_id: agent_id.to_string(),
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
        .filter(|result| is_precedent_category(&result.entry.category))
        .take(limit)
        .map(|result| {
            let mut entry = result.entry;
            entry.score = Some(result.score as f64);
            entry
        })
        .collect())
}

pub fn decide_precedent_mutation(
    candidate: MutationCandidate,
    existing: &[MemoryEntry],
    thresholds: &PrecedentSimilarityThresholds,
) -> MutationDecision {
    let best = existing
        .iter()
        .filter(|entry| is_precedent_category(&entry.category))
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
            reason: "no similar precedents".into(),
            similarity: None,
        };
    };

    let best_score = best.score.unwrap_or(0.0);
    let (action, reason) = if best_score >= thresholds.noop_threshold {
        (
            MutationAction::Noop,
            format!(
                "near-duplicate precedent (score {best_score:.3}), existing: {}",
                truncate(&best.content, 80)
            ),
        )
    } else if best_score >= thresholds.update_threshold {
        (
            MutationAction::Update {
                target_id: best.key.clone(),
            },
            format!(
                "supersedes similar precedent (score {best_score:.3}), updating: {}",
                truncate(&best.content, 80)
            ),
        )
    } else {
        (
            MutationAction::Add,
            format!("distinct precedent (best score {best_score:.3})"),
        )
    };

    MutationDecision {
        action,
        candidate,
        reason,
        similarity: Some(best_score),
    }
}

fn is_precedent_category(category: &MemoryCategory) -> bool {
    matches!(category, MemoryCategory::Custom(name) if name == "precedent")
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
    use crate::domain::memory::MemoryCategory;
    use crate::domain::memory_mutation::MutationSource;

    fn candidate() -> MutationCandidate {
        MutationCandidate {
            category: MemoryCategory::Custom("precedent".into()),
            text: "tools=web_search -> message_send | subjects=status.example.com".into(),
            confidence: 0.81,
            source: MutationSource::ToolOutput,
        }
    }

    #[test]
    fn no_matching_precedent_adds_new_entry() {
        let decision =
            decide_precedent_mutation(candidate(), &[], &PrecedentSimilarityThresholds::default());

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("no similar precedents"));
    }

    #[test]
    fn near_duplicate_precedent_becomes_noop() {
        let decision = decide_precedent_mutation(
            candidate(),
            &[MemoryEntry {
                id: "1".into(),
                key: "custom_precedent_1".into(),
                content: "tools=web_search -> message_send | subjects=status.example.com".into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.97),
            }],
            &PrecedentSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Noop));
    }

    #[test]
    fn similar_precedent_updates_existing_entry() {
        let decision = decide_precedent_mutation(
            candidate(),
            &[MemoryEntry {
                id: "1".into(),
                key: "custom_precedent_1".into(),
                content: "tools=web_search -> message_send | subjects=status".into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.88),
            }],
            &PrecedentSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Update { .. }));
    }

    #[test]
    fn ignores_other_custom_categories_when_matching_precedents() {
        let decision = decide_precedent_mutation(
            candidate(),
            &[
                MemoryEntry {
                    id: "1".into(),
                    key: "custom_project_1".into(),
                    content: "project status".into(),
                    category: MemoryCategory::Custom("project".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: Some(0.99),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "custom_precedent_1".into(),
                    content: "tools=web_search -> message_send".into(),
                    category: MemoryCategory::Custom("precedent".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: Some(0.84),
                },
            ],
            &PrecedentSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Update { .. }));
    }

    #[test]
    fn precedent_category_filtering_keeps_only_precedent_entries() {
        let shortlisted = vec![
            MemoryEntry {
                id: "1".into(),
                key: "custom_project_1".into(),
                content: "project status".into(),
                category: MemoryCategory::Custom("project".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.99),
            },
            MemoryEntry {
                id: "2".into(),
                key: "custom_precedent_1".into(),
                content: "tools=web_search -> message_send".into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.84),
            },
        ];

        let decision = decide_precedent_mutation(
            candidate(),
            &shortlisted,
            &PrecedentSimilarityThresholds::default(),
        );
        assert!(matches!(decision.action, MutationAction::Update { .. }));
    }
}
