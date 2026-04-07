//! Cheap clustering for stored run recipes.
//!
//! This provides an inspectable view over recipe families that are effectively
//! the same procedural pattern, even if their `task_family` labels diverge.

use crate::domain::run_recipe::RunRecipe;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct RunRecipeCluster {
    pub representative: RunRecipe,
    pub member_task_families: Vec<String>,
}

impl RunRecipeCluster {
    pub fn member_count(&self) -> usize {
        self.member_task_families.len()
    }
}

pub fn plan_recipe_clusters(
    recipes: &[RunRecipe],
    similarity_threshold: f32,
) -> Vec<RunRecipeCluster> {
    let mut ordered = recipes.to_vec();
    ordered.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.task_family.cmp(&right.task_family))
    });

    let mut assigned = HashSet::new();
    let mut clusters = Vec::new();

    for recipe in &ordered {
        if assigned.contains(&recipe.task_family) {
            continue;
        }

        let mut member_task_families = Vec::new();
        let mut queue = VecDeque::from([recipe.task_family.clone()]);
        let mut queued = HashSet::from([recipe.task_family.clone()]);

        while let Some(task_family) = queue.pop_front() {
            if assigned.contains(&task_family) {
                continue;
            }
            assigned.insert(task_family.clone());
            member_task_families.push(task_family.clone());

            let Some(current_recipe) = recipes
                .iter()
                .find(|candidate| candidate.task_family == task_family)
            else {
                continue;
            };

            for similar in recipes {
                if similar.task_family == current_recipe.task_family
                    || assigned.contains(&similar.task_family)
                {
                    continue;
                }
                if cross_family_recipe_similarity(current_recipe, similar) < similarity_threshold {
                    continue;
                }
                if queued.insert(similar.task_family.clone()) {
                    queue.push_back(similar.task_family.clone());
                }
            }
        }

        member_task_families.sort();
        clusters.push(RunRecipeCluster {
            representative: recipe.clone(),
            member_task_families,
        });
    }

    clusters
}

pub fn cross_family_recipe_similarity(left: &RunRecipe, right: &RunRecipe) -> f32 {
    let tool_similarity = tool_pattern_similarity(&left.tool_pattern, &right.tool_pattern);
    let summary_similarity = text_token_overlap(left.summary.as_str(), right.summary.as_str()).max(
        text_token_overlap(left.sample_request.as_str(), right.sample_request.as_str()),
    );
    (tool_similarity * 0.8) + (summary_similarity * 0.2)
}

fn tool_pattern_similarity(left: &[String], right: &[String]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left
        .iter()
        .filter(|tool| right.iter().any(|other| other.eq_ignore_ascii_case(tool)))
        .count() as f32;
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
        shared / union.len() as f32
    }
}

fn text_token_overlap(left: &str, right: &str) -> f32 {
    let left_tokens = tokenize(left);
    let right_tokens = tokenize(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let shared = left_tokens
        .iter()
        .filter(|token| right_tokens.contains(*token))
        .count() as f32;
    let union = left_tokens.union(&right_tokens).count() as f32;
    if union <= f32::EPSILON {
        0.0
    } else {
        shared / union
    }
}

fn tokenize(value: &str) -> HashSet<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(
        task_family: &str,
        tool_pattern: &[&str],
        summary: &str,
        updated_at: u64,
    ) -> RunRecipe {
        RunRecipe {
            agent_id: "agent".into(),
            task_family: task_family.into(),
            lineage_task_families: vec![task_family.into()],
            sample_request: summary.into(),
            summary: summary.into(),
            tool_pattern: tool_pattern
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            success_count: 1,
            updated_at,
        }
    }

    #[test]
    fn clusters_cross_family_recipe_duplicates() {
        let recipes = vec![
            recipe(
                "delivery_search",
                &["web_search", "message_send"],
                "search and send status page",
                10,
            ),
            recipe(
                "status_page_delivery",
                &["web_search", "message_send"],
                "send status page after search",
                9,
            ),
        ];

        let clusters = plan_recipe_clusters(&recipes, 0.9);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].member_count(), 2);
    }

    #[test]
    fn keeps_distinct_recipe_shapes_apart() {
        let recipes = vec![
            recipe(
                "delivery_search",
                &["web_search", "message_send"],
                "search and send",
                10,
            ),
            recipe(
                "backup_delivery",
                &["shell", "message_send"],
                "run backup and send",
                9,
            ),
        ];

        let clusters = plan_recipe_clusters(&recipes, 0.9);
        assert_eq!(clusters.len(), 2);
    }
}
