//! Category-aware precedent similarity and merge policy.
//!
//! This is the first learning path that stops relying on the generic mutation
//! engine alone. Accepted precedent candidates should compare primarily against
//! existing precedent memories, not against all episodic memories.

use crate::application::services::{
    failure_similarity_service, procedural_cluster_service::ProceduralCluster,
};
use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryQuery};
use crate::domain::memory_mutation::{MutationAction, MutationCandidate, MutationDecision};
use crate::ports::memory::UnifiedMemoryPort;

#[derive(Debug, Clone)]
pub struct PrecedentSimilarityThresholds {
    pub shortlist_limit: usize,
    pub noop_threshold: f64,
    pub update_threshold: f64,
    pub ambiguity_margin: f64,
}

impl Default for PrecedentSimilarityThresholds {
    fn default() -> Self {
        Self {
            shortlist_limit: 8,
            noop_threshold: 0.95,
            update_threshold: 0.82,
            ambiguity_margin: 0.05,
        }
    }
}

pub async fn evaluate_precedent_candidate(
    mem: &dyn UnifiedMemoryPort,
    candidate: MutationCandidate,
    agent_id: &str,
    thresholds: &PrecedentSimilarityThresholds,
) -> MutationDecision {
    evaluate_precedent_candidate_with_failures(mem, candidate, agent_id, thresholds, &[]).await
}

pub async fn evaluate_precedent_candidate_with_failures(
    mem: &dyn UnifiedMemoryPort,
    candidate: MutationCandidate,
    agent_id: &str,
    thresholds: &PrecedentSimilarityThresholds,
    failure_clusters: &[ProceduralCluster],
) -> MutationDecision {
    let existing =
        fetch_precedent_shortlist(mem, &candidate.text, agent_id, thresholds.shortlist_limit)
            .await
            .unwrap_or_default();
    decide_precedent_mutation_with_failures(candidate, &existing, thresholds, failure_clusters)
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
        categories: vec![MemoryCategory::Custom("precedent".into())],
        include_shared: false,
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

pub fn decide_precedent_mutation(
    candidate: MutationCandidate,
    existing: &[MemoryEntry],
    thresholds: &PrecedentSimilarityThresholds,
) -> MutationDecision {
    decide_precedent_mutation_with_failures(candidate, existing, thresholds, &[])
}

pub fn decide_precedent_mutation_with_failures(
    candidate: MutationCandidate,
    existing: &[MemoryEntry],
    thresholds: &PrecedentSimilarityThresholds,
    failure_clusters: &[ProceduralCluster],
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
    let candidate_shape = parse_precedent_summary(&candidate.text);
    let best_shape = parse_precedent_summary(&best.content);
    let ambiguous_cluster = best_score < thresholds.noop_threshold
        && has_ambiguous_precedent_cluster(best, existing, thresholds);
    let contradicted_branch =
        precedent_conflicts_with_failure_clusters(&best.content, failure_clusters, 0.75);
    let (action, reason) = if best_score >= thresholds.noop_threshold {
        (
            MutationAction::Noop,
            format!(
                "near-duplicate precedent (score {best_score:.3}), existing: {}",
                truncate(&best.content, 80)
            ),
        )
    } else if ambiguous_cluster {
        (
            MutationAction::Add,
            format!("ambiguous precedent cluster near score {best_score:.3}"),
        )
    } else if best_score >= thresholds.update_threshold && contradicted_branch {
        (
            MutationAction::Add,
            format!(
                "similar precedent but contradicted by failure clusters (score {best_score:.3})"
            ),
        )
    } else if best_score >= thresholds.update_threshold
        && is_distinct_precedent_shape(&candidate_shape, &best_shape)
    {
        (
            MutationAction::Add,
            format!("similar precedent text but distinct procedure shape (score {best_score:.3})"),
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

fn has_ambiguous_precedent_cluster(
    best: &MemoryEntry,
    existing: &[MemoryEntry],
    thresholds: &PrecedentSimilarityThresholds,
) -> bool {
    let best_score = best.score.unwrap_or(0.0);
    existing
        .iter()
        .filter(|entry| entry.key != best.key)
        .filter(|entry| is_precedent_category(&entry.category))
        .filter_map(|entry| entry.score.map(|score| (entry, score)))
        .any(|(entry, score)| {
            score >= thresholds.update_threshold
                && (best_score - score).abs() <= thresholds.ambiguity_margin
                && precedent_contents_have_distinct_shape(&best.content, &entry.content)
        })
}

pub fn merge_precedent_text(existing_content: &str, candidate_text: &str) -> String {
    let existing = parse_precedent_summary(existing_content);
    let candidate = parse_precedent_summary(candidate_text);
    let merged = PrecedentSummary {
        tools: union_preserving_order(&existing.tools, &candidate.tools),
        subjects: union_preserving_order(&existing.subjects, &candidate.subjects),
        facets: union_preserving_order(&existing.facets, &candidate.facets),
    };
    format_precedent_summary(&merged)
}

pub fn precedent_contents_have_distinct_shape(left: &str, right: &str) -> bool {
    is_distinct_precedent_shape(
        &parse_precedent_summary(left),
        &parse_precedent_summary(right),
    )
}

pub fn precedent_summary_tools(value: &str) -> Vec<String> {
    parse_precedent_summary(value).tools
}

fn is_precedent_category(category: &MemoryCategory) -> bool {
    matches!(category, MemoryCategory::Custom(name) if name == "precedent")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PrecedentSummary {
    tools: Vec<String>,
    subjects: Vec<String>,
    facets: Vec<String>,
}

fn parse_precedent_summary(value: &str) -> PrecedentSummary {
    let mut summary = PrecedentSummary::default();
    for part in value.split(" | ").map(str::trim) {
        if let Some(raw) = part.strip_prefix("tools=") {
            summary.tools = raw
                .split("->")
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
        } else if let Some(raw) = part.strip_prefix("subjects=") {
            summary.subjects = raw
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
        } else if let Some(raw) = part.strip_prefix("facets=") {
            summary.facets = raw
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
        }
    }
    summary
}

fn format_precedent_summary(summary: &PrecedentSummary) -> String {
    let mut parts = Vec::new();
    if !summary.tools.is_empty() {
        parts.push(format!("tools={}", summary.tools.join(" -> ")));
    }
    if !summary.subjects.is_empty() {
        parts.push(format!("subjects={}", summary.subjects.join(", ")));
    }
    if !summary.facets.is_empty() {
        parts.push(format!("facets={}", summary.facets.join(",")));
    }
    parts.join(" | ")
}

fn is_distinct_precedent_shape(candidate: &PrecedentSummary, existing: &PrecedentSummary) -> bool {
    !candidate.tools.is_empty()
        && !existing.tools.is_empty()
        && jaccard_similarity(&candidate.tools, &existing.tools) < 0.5
}

fn precedent_conflicts_with_failure_clusters(
    precedent_content: &str,
    failure_clusters: &[ProceduralCluster],
    min_overlap: f64,
) -> bool {
    let tools = precedent_summary_tools(precedent_content);
    !tools.is_empty()
        && failure_clusters.iter().any(|cluster| {
            let failed_tools = failure_similarity_service::failure_summary_failed_tools(
                &cluster.representative.content,
            );
            !failed_tools.is_empty() && jaccard_similarity(&tools, &failed_tools) >= min_overlap
        })
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

    #[test]
    fn divergent_tool_patterns_add_new_precedent_instead_of_updating() {
        let decision = decide_precedent_mutation(
            candidate(),
            &[MemoryEntry {
                id: "1".into(),
                key: "custom_precedent_1".into(),
                content: "tools=shell -> file_edit | subjects=status.example.com".into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.86),
            }],
            &PrecedentSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("distinct procedure shape"));
    }

    #[test]
    fn ambiguous_precedent_clusters_prefer_safe_add() {
        let decision = decide_precedent_mutation(
            candidate(),
            &[
                MemoryEntry {
                    id: "1".into(),
                    key: "custom_precedent_1".into(),
                    content: "tools=web_search -> message_send | subjects=status".into(),
                    category: MemoryCategory::Custom("precedent".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: Some(0.88),
                },
                MemoryEntry {
                    id: "2".into(),
                    key: "custom_precedent_2".into(),
                    content: "tools=shell -> message_send | subjects=status".into(),
                    category: MemoryCategory::Custom("precedent".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: Some(0.86),
                },
            ],
            &PrecedentSimilarityThresholds::default(),
        );

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("ambiguous precedent cluster"));
    }

    #[test]
    fn contradicted_similar_precedent_adds_new_branch() {
        let decision = decide_precedent_mutation_with_failures(
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
            &[ProceduralCluster {
                representative: MemoryEntry {
                    id: "f1".into(),
                    key: "failure_1".into(),
                    content: "failed_tools=web_search -> message_send | outcomes=runtime_error"
                        .into(),
                    category: MemoryCategory::Custom("failure_pattern".into()),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    session_id: None,
                    score: None,
                },
                member_keys: vec!["failure_1".into()],
            }],
        );

        assert!(matches!(decision.action, MutationAction::Add));
        assert!(decision.reason.contains("contradicted by failure clusters"));
    }

    #[test]
    fn merge_precedent_text_unions_subjects_and_facets() {
        let merged = merge_precedent_text(
            "tools=web_search -> message_send | subjects=status.example.com | facets=search,delivery",
            "tools=web_search -> message_send | subjects=status2.example.com | facets=delivery,workspace",
        );

        assert!(merged.contains("tools=web_search -> message_send"));
        assert!(merged.contains("subjects=status.example.com, status2.example.com"));
        assert!(merged.contains("facets=search,delivery,workspace"));
    }
}
