//! Human-review summaries for procedural memory clusters.
//!
//! These are cheap, deterministic action hints for operator surfaces. They do
//! not change memory by themselves; they explain whether a cluster looks
//! stable, compactable, or blocked by contradictory evidence.

use crate::application::services::precedent_similarity_service;
use crate::application::services::procedural_cluster_service::ProceduralCluster;
use crate::application::services::procedural_contradiction_service::ProceduralContradiction;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProceduralClusterReviewAction {
    Stable,
    CompactCandidate,
    PreserveBranch,
    BlocksProceduralPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProceduralClusterReview {
    pub kind: &'static str,
    pub representative_key: String,
    pub member_count: usize,
    pub action: ProceduralClusterReviewAction,
    pub reason: &'static str,
}

pub fn representative_keys_for_action(
    reviews: &[ProceduralClusterReview],
    kind: &'static str,
    action: ProceduralClusterReviewAction,
) -> Vec<String> {
    reviews
        .iter()
        .filter(|review| review.kind == kind && review.action == action)
        .map(|review| review.representative_key.clone())
        .collect()
}

pub fn review_precedent_clusters(
    clusters: &[ProceduralCluster],
    failure_clusters: &[ProceduralCluster],
) -> Vec<ProceduralClusterReview> {
    let mut reviews = clusters
        .iter()
        .map(|cluster| {
            let contradicted = precedent_similarity_service::precedent_is_contradicted_by_failures(
                &cluster.representative.content,
                failure_clusters,
                0.75,
            );
            let (action, reason) = if contradicted {
                (
                    ProceduralClusterReviewAction::PreserveBranch,
                    "failure_contradicted_branch",
                )
            } else if cluster.member_count() > 1 {
                (
                    ProceduralClusterReviewAction::CompactCandidate,
                    "duplicate_precedent_cluster",
                )
            } else {
                (
                    ProceduralClusterReviewAction::Stable,
                    "stable_precedent_cluster",
                )
            };
            ProceduralClusterReview {
                kind: "precedent",
                representative_key: cluster.representative.key.clone(),
                member_count: cluster.member_count(),
                action,
                reason,
            }
        })
        .collect::<Vec<_>>();
    sort_reviews(&mut reviews);
    reviews
}

pub fn review_failure_pattern_clusters(
    clusters: &[ProceduralCluster],
    contradictions: &[ProceduralContradiction],
) -> Vec<ProceduralClusterReview> {
    let mut reviews = clusters
        .iter()
        .map(|cluster| {
            let blocks_paths = contradictions.iter().any(|contradiction| {
                contradiction.failure_representative_key == cluster.representative.key
            });
            let (action, reason) = if blocks_paths {
                (
                    ProceduralClusterReviewAction::BlocksProceduralPaths,
                    "failure_cluster_blocks_procedural_paths",
                )
            } else if cluster.member_count() > 1 {
                (
                    ProceduralClusterReviewAction::CompactCandidate,
                    "duplicate_failure_cluster",
                )
            } else {
                (
                    ProceduralClusterReviewAction::Stable,
                    "stable_failure_cluster",
                )
            };
            ProceduralClusterReview {
                kind: "failure_pattern",
                representative_key: cluster.representative.key.clone(),
                member_count: cluster.member_count(),
                action,
                reason,
            }
        })
        .collect::<Vec<_>>();
    sort_reviews(&mut reviews);
    reviews
}

fn sort_reviews(reviews: &mut [ProceduralClusterReview]) {
    reviews.sort_by(|left, right| {
        right
            .member_count
            .cmp(&left.member_count)
            .then_with(|| left.representative_key.cmp(&right.representative_key))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::procedural_cluster_service::ProceduralCluster;
    use crate::application::services::procedural_contradiction_service::ProceduralContradiction;
    use crate::domain::memory::{MemoryCategory, MemoryEntry};

    fn cluster(key: &str, content: &str, members: &[&str]) -> ProceduralCluster {
        ProceduralCluster {
            representative: MemoryEntry {
                id: key.into(),
                key: key.into(),
                content: content.into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            member_keys: members.iter().map(|value| (*value).to_string()).collect(),
        }
    }

    #[test]
    fn marks_contradicted_precedent_cluster_as_branch_to_preserve() {
        let reviews = review_precedent_clusters(
            &[cluster(
                "p1",
                "tools=web_search -> message_send | subjects=status.example.com",
                &["p1"],
            )],
            &[cluster(
                "f1",
                "failed_tools=web_search -> message_send | outcomes=runtime_error",
                &["f1"],
            )],
        );

        assert_eq!(
            reviews[0].action,
            ProceduralClusterReviewAction::PreserveBranch
        );
        assert_eq!(reviews[0].reason, "failure_contradicted_branch");
    }

    #[test]
    fn marks_failure_cluster_with_contradictions_as_blocking() {
        let reviews = review_failure_pattern_clusters(
            &[cluster(
                "f1",
                "failed_tools=web_search -> message_send | outcomes=runtime_error",
                &["f1"],
            )],
            &[ProceduralContradiction {
                recipe_task_family: "status_delivery".into(),
                recipe_cluster_size: 1,
                recipe_tool_pattern: vec!["web_search".into(), "message_send".into()],
                failure_representative_key: "f1".into(),
                failure_cluster_size: 1,
                failed_tools: vec!["web_search".into(), "message_send".into()],
                overlap: 1.0,
            }],
        );

        assert_eq!(
            reviews[0].action,
            ProceduralClusterReviewAction::BlocksProceduralPaths
        );
        assert_eq!(reviews[0].reason, "failure_cluster_blocks_procedural_paths");
    }

    #[test]
    fn extracts_representative_keys_for_matching_action() {
        let reviews = vec![
            ProceduralClusterReview {
                kind: "precedent",
                representative_key: "p1".into(),
                member_count: 2,
                action: ProceduralClusterReviewAction::CompactCandidate,
                reason: "duplicate_precedent_cluster",
            },
            ProceduralClusterReview {
                kind: "precedent",
                representative_key: "p2".into(),
                member_count: 1,
                action: ProceduralClusterReviewAction::PreserveBranch,
                reason: "failure_contradicted_branch",
            },
            ProceduralClusterReview {
                kind: "failure_pattern",
                representative_key: "f1".into(),
                member_count: 2,
                action: ProceduralClusterReviewAction::CompactCandidate,
                reason: "duplicate_failure_cluster",
            },
        ];

        assert_eq!(
            representative_keys_for_action(
                &reviews,
                "precedent",
                ProceduralClusterReviewAction::CompactCandidate,
            ),
            vec!["p1".to_string()]
        );
    }
}
