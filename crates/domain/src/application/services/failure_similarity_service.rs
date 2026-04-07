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
    pub ambiguity_margin: f64,
}

impl Default for FailureSimilarityThresholds {
    fn default() -> Self {
        Self {
            shortlist_limit: 8,
            noop_threshold: 0.96,
            update_threshold: 0.84,
            ambiguity_margin: 0.05,
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
    let candidate_shape = parse_failure_summary(&candidate.text);
    let best_shape = parse_failure_summary(&best.content);
    let ambiguous_cluster = best_score < thresholds.noop_threshold
        && has_ambiguous_failure_cluster(best, existing, thresholds);
    let (action, reason) = if best_score >= thresholds.noop_threshold {
        (
            MutationAction::Noop,
            format!(
                "near-duplicate failure pattern (score {best_score:.3}), existing: {}",
                truncate(&best.content, 80)
            ),
        )
    } else if ambiguous_cluster {
        (
            MutationAction::Add,
            format!("ambiguous failure cluster near score {best_score:.3}"),
        )
    } else if best_score >= thresholds.update_threshold
        && is_distinct_failure_shape(&candidate_shape, &best_shape)
    {
        (
            MutationAction::Add,
            format!("similar failure text but distinct failed tools (score {best_score:.3})"),
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

fn has_ambiguous_failure_cluster(
    best: &MemoryEntry,
    existing: &[MemoryEntry],
    thresholds: &FailureSimilarityThresholds,
) -> bool {
    let best_score = best.score.unwrap_or(0.0);
    existing
        .iter()
        .filter(|entry| entry.key != best.key)
        .filter(|entry| is_failure_category(&entry.category))
        .filter_map(|entry| entry.score.map(|score| (entry, score)))
        .any(|(entry, score)| {
            score >= thresholds.update_threshold
                && (best_score - score).abs() <= thresholds.ambiguity_margin
                && failure_contents_have_distinct_shape(&best.content, &entry.content)
        })
}

pub fn merge_failure_text(existing_content: &str, candidate_text: &str) -> String {
    let existing = parse_failure_summary(existing_content);
    let candidate = parse_failure_summary(candidate_text);
    let merged = FailureSummary {
        failed_tools: union_preserving_order(&existing.failed_tools, &candidate.failed_tools),
        outcomes: union_preserving_order(&existing.outcomes, &candidate.outcomes),
        subjects: union_preserving_order(&existing.subjects, &candidate.subjects),
    };
    format_failure_summary(&merged)
}

pub fn failure_contents_have_distinct_shape(left: &str, right: &str) -> bool {
    is_distinct_failure_shape(&parse_failure_summary(left), &parse_failure_summary(right))
}

pub fn failure_summary_failed_tools(value: &str) -> Vec<String> {
    parse_failure_summary(value).failed_tools
}

fn is_failure_category(category: &MemoryCategory) -> bool {
    matches!(category, MemoryCategory::Custom(name) if name == "failure_pattern")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FailureSummary {
    failed_tools: Vec<String>,
    outcomes: Vec<String>,
    subjects: Vec<String>,
}

fn parse_failure_summary(value: &str) -> FailureSummary {
    let mut summary = FailureSummary::default();
    for part in value.split(" | ").map(str::trim) {
        if let Some(raw) = part.strip_prefix("failed_tools=") {
            summary.failed_tools = raw
                .split("->")
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
        } else if let Some(raw) = part.strip_prefix("outcomes=") {
            summary.outcomes = raw
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
        } else if let Some(raw) = part.strip_prefix("subjects=") {
            summary.subjects = raw
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
        }
    }
    summary
}

fn format_failure_summary(summary: &FailureSummary) -> String {
    let mut parts = Vec::new();
    if !summary.failed_tools.is_empty() {
        parts.push(format!(
            "failed_tools={}",
            summary.failed_tools.join(" -> ")
        ));
    }
    if !summary.outcomes.is_empty() {
        parts.push(format!("outcomes={}", summary.outcomes.join(",")));
    }
    if !summary.subjects.is_empty() {
        parts.push(format!("subjects={}", summary.subjects.join(", ")));
    }
    parts.join(" | ")
}

fn is_distinct_failure_shape(candidate: &FailureSummary, existing: &FailureSummary) -> bool {
    !candidate.failed_tools.is_empty()
        && !existing.failed_tools.is_empty()
        && jaccard_similarity(&candidate.failed_tools, &existing.failed_tools) < 0.5
}

fn union_preserving_order(left: &[String], right: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    for value in left.iter().chain(right.iter()) {
        if !values
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(value))
        {
            values.push(value.clone());
        }
    }
    values
}

fn jaccard_similarity(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left
        .iter()
        .filter(|item| right.iter().any(|other| other.eq_ignore_ascii_case(item)))
        .count() as f64;
    let mut union = Vec::new();
    for item in left.iter().chain(right.iter()) {
        if !union
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(item))
        {
            union.push(item.clone());
        }
    }
    if union.is_empty() {
        0.0
    } else {
        shared / union.len() as f64
    }
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

    #[test]
    fn divergent_failed_tools_add_new_failure_pattern() {
        let decision = decide_failure_mutation(
            candidate(),
            &[MemoryEntry {
                id: "1".into(),
                key: "custom_failure_1".into(),
                content: "failed_tools=shell -> file_edit | outcomes=runtime_error".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.87),
            }],
            &FailureSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("distinct failed tools"));
    }

    #[test]
    fn ambiguous_failure_clusters_prefer_safe_add() {
        let decision = decide_failure_mutation(
            candidate(),
            &[
                MemoryEntry {
                    id: "1".into(),
                    key: "custom_failure_1".into(),
                    content: "failed_tools=web_fetch | outcomes=runtime_error".into(),
                    category: MemoryCategory::Custom("failure_pattern".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: Some(0.88),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "custom_failure_2".into(),
                    content: "failed_tools=shell | outcomes=runtime_error".into(),
                    category: MemoryCategory::Custom("failure_pattern".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: Some(0.85),
                },
            ],
            &FailureSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("ambiguous failure cluster"));
    }

    #[test]
    fn merge_failure_text_unions_outcomes_and_subjects() {
        let merged = merge_failure_text(
            "failed_tools=web_fetch | outcomes=runtime_error | subjects=status.example.com",
            "failed_tools=web_fetch | outcomes=timeout,network_error | subjects=status2.example.com",
        );

        assert!(merged.contains("failed_tools=web_fetch"));
        assert!(merged.contains("outcomes=runtime_error,timeout,network_error"));
        assert!(merged.contains("subjects=status.example.com, status2.example.com"));
    }
}
