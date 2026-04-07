//! Deterministic review and cleanup for stored run recipes.
//!
//! Recipe clusters are already inspectable via `run_recipe_cluster_service`.
//! This service turns those clusters into concrete cleanup decisions so the
//! store does not keep redundant cross-family recipes forever.

use crate::application::services::run_recipe_cluster_service::plan_recipe_clusters;
use crate::domain::run_recipe::RunRecipe;
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RunRecipeReviewThresholds {
    pub cluster_similarity_threshold: f32,
}

impl Default for RunRecipeReviewThresholds {
    fn default() -> Self {
        Self {
            cluster_similarity_threshold: 0.9,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecipeReviewDecision {
    pub canonical_recipe: RunRecipe,
    pub removed_task_families: Vec<String>,
    pub cluster_task_families: Vec<String>,
    pub reason: &'static str,
}

pub fn review_run_recipes(
    recipes: &[RunRecipe],
    thresholds: &RunRecipeReviewThresholds,
) -> Vec<RunRecipeReviewDecision> {
    let recipe_lookup = recipes
        .iter()
        .map(|recipe| (recipe.task_family.clone(), recipe))
        .collect::<HashMap<_, _>>();

    plan_recipe_clusters(recipes, thresholds.cluster_similarity_threshold)
        .into_iter()
        .filter(|cluster| cluster.member_count() >= 2)
        .filter_map(|cluster| {
            let members = cluster
                .member_task_families
                .iter()
                .filter_map(|task_family| recipe_lookup.get(task_family).copied())
                .collect::<Vec<_>>();
            build_review_decision(&members)
        })
        .collect()
}

fn build_review_decision(members: &[&RunRecipe]) -> Option<RunRecipeReviewDecision> {
    let canonical = select_canonical_recipe(members)?;
    let mut merged = canonical.clone();
    let mut removed_task_families = Vec::new();
    let mut cluster_task_families = members
        .iter()
        .map(|recipe| recipe.task_family.clone())
        .collect::<Vec<_>>();
    cluster_task_families.sort();

    for member in members {
        if member.task_family == canonical.task_family {
            continue;
        }
        merged = merge_recipe_pair(&merged, member);
        removed_task_families.push(member.task_family.clone());
    }

    removed_task_families.sort();
    if removed_task_families.is_empty() {
        return None;
    }

    Some(RunRecipeReviewDecision {
        canonical_recipe: merged,
        removed_task_families,
        cluster_task_families,
        reason: "redundant_recipe_cluster",
    })
}

fn select_canonical_recipe<'a>(members: &'a [&RunRecipe]) -> Option<&'a RunRecipe> {
    members
        .iter()
        .copied()
        .max_by(|left, right| compare_recipe_priority(left, right))
}

fn compare_recipe_priority(left: &RunRecipe, right: &RunRecipe) -> Ordering {
    left.success_count
        .cmp(&right.success_count)
        .then_with(|| left.updated_at.cmp(&right.updated_at))
        .then_with(|| text_richness(&left.summary).cmp(&text_richness(&right.summary)))
        .then_with(|| {
            text_richness(&left.sample_request).cmp(&text_richness(&right.sample_request))
        })
        .then_with(|| right.task_family.cmp(&left.task_family))
}

fn merge_recipe_pair(canonical: &RunRecipe, other: &RunRecipe) -> RunRecipe {
    RunRecipe {
        agent_id: canonical.agent_id.clone(),
        task_family: canonical.task_family.clone(),
        sample_request: prefer_richer_text(&canonical.sample_request, &other.sample_request),
        summary: prefer_richer_text(&canonical.summary, &other.summary),
        tool_pattern: merge_tool_patterns(&canonical.tool_pattern, &other.tool_pattern),
        success_count: canonical.success_count.saturating_add(other.success_count),
        updated_at: canonical.updated_at.max(other.updated_at),
    }
}

fn merge_tool_patterns(left: &[String], right: &[String]) -> Vec<String> {
    let mut merged = left.to_vec();
    for tool in right {
        if !merged
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(tool))
        {
            merged.push(tool.clone());
        }
    }
    merged
}

fn prefer_richer_text(left: &str, right: &str) -> String {
    if text_richness(right) > text_richness(left) {
        right.trim().to_string()
    } else {
        left.trim().to_string()
    }
}

fn text_richness(value: &str) -> usize {
    value.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(
        task_family: &str,
        tool_pattern: &[&str],
        summary: &str,
        success_count: u32,
        updated_at: u64,
    ) -> RunRecipe {
        RunRecipe {
            agent_id: "agent".into(),
            task_family: task_family.into(),
            sample_request: summary.into(),
            summary: summary.into(),
            tool_pattern: tool_pattern
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            success_count,
            updated_at,
        }
    }

    #[test]
    fn reviews_cross_family_duplicate_cluster_into_single_canonical_recipe() {
        let decisions = review_run_recipes(
            &[
                recipe(
                    "delivery_search",
                    &["web_search", "message_send"],
                    "search and send status page",
                    2,
                    10,
                ),
                recipe(
                    "status_delivery",
                    &["web_search", "message_send"],
                    "search and send the status page",
                    4,
                    12,
                ),
            ],
            &RunRecipeReviewThresholds::default(),
        );

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].canonical_recipe.task_family, "status_delivery");
        assert_eq!(decisions[0].canonical_recipe.success_count, 6);
        assert_eq!(
            decisions[0].removed_task_families,
            vec!["delivery_search".to_string()]
        );
    }

    #[test]
    fn ignores_singleton_recipe_clusters() {
        let decisions = review_run_recipes(
            &[recipe(
                "delivery_search",
                &["web_search", "message_send"],
                "search and send status page",
                2,
                10,
            )],
            &RunRecipeReviewThresholds::default(),
        );

        assert!(decisions.is_empty());
    }

    #[test]
    fn canonical_selection_prefers_higher_success_before_newness() {
        let decisions = review_run_recipes(
            &[
                recipe(
                    "stable_family",
                    &["web_search", "message_send"],
                    "search and send status page",
                    5,
                    8,
                ),
                recipe(
                    "newer_family",
                    &["web_search", "message_send"],
                    "search and send status page",
                    2,
                    20,
                ),
            ],
            &RunRecipeReviewThresholds::default(),
        );

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].canonical_recipe.task_family, "stable_family");
        assert_eq!(decisions[0].canonical_recipe.success_count, 7);
        assert_eq!(decisions[0].canonical_recipe.updated_at, 20);
    }
}
