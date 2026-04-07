//! Contradiction detection across long-lived procedural memory.
//!
//! This surfaces cases where stored run-recipe clusters overlap strongly with
//! failure-pattern clusters. The goal is to expose and gate contradictory
//! procedural guidance before it quietly reinforces the wrong path.

use crate::application::services::failure_similarity_service;
use crate::application::services::procedural_cluster_service::ProceduralCluster;
use crate::application::services::run_recipe_cluster_service::RunRecipeCluster;
use crate::domain::run_recipe::RunRecipe;

#[derive(Debug, Clone, PartialEq)]
pub struct ProceduralContradiction {
    pub recipe_task_family: String,
    pub recipe_lineage_task_families: Vec<String>,
    pub recipe_cluster_size: usize,
    pub recipe_tool_pattern: Vec<String>,
    pub failure_representative_key: String,
    pub failure_cluster_size: usize,
    pub failed_tools: Vec<String>,
    pub overlap: f64,
}

pub fn find_recipe_failure_contradictions(
    recipe_clusters: &[RunRecipeCluster],
    failure_clusters: &[ProceduralCluster],
    min_overlap: f64,
) -> Vec<ProceduralContradiction> {
    let mut contradictions = recipe_clusters
        .iter()
        .flat_map(|recipe_cluster| {
            failure_clusters.iter().filter_map(|failure_cluster| {
                let failed_tools = failure_similarity_service::failure_summary_failed_tools(
                    &failure_cluster.representative.content,
                );
                let overlap = tool_pattern_overlap(
                    &recipe_cluster.representative.tool_pattern,
                    &failed_tools,
                );
                (overlap >= min_overlap && !failed_tools.is_empty()).then(|| {
                    ProceduralContradiction {
                        recipe_task_family: recipe_cluster.representative.task_family.clone(),
                        recipe_lineage_task_families: recipe_cluster
                            .representative
                            .lineage_task_families
                            .clone(),
                        recipe_cluster_size: recipe_cluster.member_count(),
                        recipe_tool_pattern: recipe_cluster.representative.tool_pattern.clone(),
                        failure_representative_key: failure_cluster.representative.key.clone(),
                        failure_cluster_size: failure_cluster.member_count(),
                        failed_tools,
                        overlap,
                    }
                })
            })
        })
        .collect::<Vec<_>>();

    contradictions.sort_by(|left, right| {
        right
            .overlap
            .partial_cmp(&left.overlap)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.failure_cluster_size.cmp(&left.failure_cluster_size))
            .then_with(|| left.recipe_task_family.cmp(&right.recipe_task_family))
    });
    contradictions
}

pub fn recipe_is_contradicted(
    recipe: &RunRecipe,
    failure_clusters: &[ProceduralCluster],
    min_overlap: f64,
) -> bool {
    failure_clusters.iter().any(|failure_cluster| {
        let failed_tools = failure_similarity_service::failure_summary_failed_tools(
            &failure_cluster.representative.content,
        );
        let overlap = tool_pattern_overlap(&recipe.tool_pattern, &failed_tools);
        overlap >= min_overlap && !failed_tools.is_empty()
    })
}

fn tool_pattern_overlap(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left
        .iter()
        .filter(|tool| right.iter().any(|other| other.eq_ignore_ascii_case(tool)))
        .count() as f64;
    let mut union = Vec::new();
    for tool in left.iter().chain(right.iter()) {
        if !union
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tool))
        {
            union.push(tool.clone());
        }
    }
    if union.is_empty() {
        0.0
    } else {
        shared / union.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::memory::{MemoryCategory, MemoryEntry};
    use crate::domain::run_recipe::RunRecipe;

    fn recipe_cluster(task_family: &str, tool_pattern: &[&str]) -> RunRecipeCluster {
        RunRecipeCluster {
            representative: RunRecipe {
                agent_id: "agent".into(),
                task_family: task_family.into(),
                lineage_task_families: vec![task_family.into()],
                sample_request: "sample".into(),
                summary: "summary".into(),
                tool_pattern: tool_pattern
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect(),
                success_count: 3,
                updated_at: 1,
            },
            member_task_families: vec![task_family.into()],
        }
    }

    fn failure_cluster(key: &str, summary: &str) -> ProceduralCluster {
        ProceduralCluster {
            representative: MemoryEntry {
                id: key.into(),
                key: key.into(),
                content: summary.into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            member_keys: vec![key.into()],
        }
    }

    #[test]
    fn detects_recipe_failure_contradictions() {
        let contradictions = find_recipe_failure_contradictions(
            &[recipe_cluster(
                "status_delivery",
                &["web_search", "message_send"],
            )],
            &[failure_cluster(
                "f1",
                "failed_tools=web_search -> message_send | outcomes=runtime_error",
            )],
            0.75,
        );

        assert_eq!(contradictions.len(), 1);
        assert_eq!(contradictions[0].recipe_task_family, "status_delivery");
        assert_eq!(
            contradictions[0].recipe_lineage_task_families,
            vec!["status_delivery"]
        );
        assert_eq!(contradictions[0].failure_representative_key, "f1");
    }

    #[test]
    fn ignores_unrelated_failure_clusters() {
        let contradictions = find_recipe_failure_contradictions(
            &[recipe_cluster(
                "status_delivery",
                &["web_search", "message_send"],
            )],
            &[failure_cluster(
                "f1",
                "failed_tools=shell | outcomes=runtime_error",
            )],
            0.75,
        );

        assert!(contradictions.is_empty());
    }

    #[test]
    fn detects_when_single_recipe_is_contradicted() {
        let recipe = RunRecipe {
            agent_id: "agent".into(),
            task_family: "status_delivery".into(),
            lineage_task_families: vec!["status_delivery".into()],
            sample_request: "sample".into(),
            summary: "summary".into(),
            tool_pattern: vec!["web_search".into(), "message_send".into()],
            success_count: 3,
            updated_at: 1,
        };

        assert!(recipe_is_contradicted(
            &recipe,
            &[failure_cluster(
                "f1",
                "failed_tools=web_search -> message_send | outcomes=runtime_error",
            )],
            0.75,
        ));
    }
}
