//! Recipe evolution helpers.
//!
//! Phase 4.9 should not treat every accepted recipe candidate as a blind
//! overwrite. This module provides the first merge helper for high-confidence
//! same-family recipe updates.

use crate::application::services::learning_candidate_service::RunRecipeLearningCandidate;
use crate::domain::run_recipe::RunRecipe;

pub fn build_new_recipe(
    agent_id: &str,
    candidate: &RunRecipeLearningCandidate,
    updated_at: u64,
) -> RunRecipe {
    RunRecipe {
        agent_id: agent_id.to_string(),
        task_family: candidate.task_family_hint.clone(),
        sample_request: candidate.sample_request.clone(),
        summary: candidate.summary.clone(),
        tool_pattern: candidate.tool_pattern.clone(),
        success_count: 1,
        updated_at,
    }
}

pub fn merge_existing_recipe(
    existing: &RunRecipe,
    candidate: &RunRecipeLearningCandidate,
    updated_at: u64,
) -> RunRecipe {
    RunRecipe {
        agent_id: existing.agent_id.clone(),
        task_family: existing.task_family.clone(),
        sample_request: prefer_richer_text(&existing.sample_request, &candidate.sample_request),
        summary: prefer_richer_text(&existing.summary, &candidate.summary),
        tool_pattern: merge_tool_patterns(&existing.tool_pattern, &candidate.tool_pattern),
        success_count: existing.success_count.saturating_add(1),
        updated_at,
    }
}

fn merge_tool_patterns(existing: &[String], candidate: &[String]) -> Vec<String> {
    let mut merged = existing.to_vec();
    for tool in candidate {
        if !merged.iter().any(|existing_tool| existing_tool == tool) {
            merged.push(tool.clone());
        }
    }
    merged
}

fn prefer_richer_text(existing: &str, candidate: &str) -> String {
    let existing_score = text_richness(existing);
    let candidate_score = text_richness(candidate);
    if candidate_score > existing_score {
        candidate.trim().to_string()
    } else {
        existing.trim().to_string()
    }
}

fn text_richness(value: &str) -> usize {
    value.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_existing_recipe_without_dropping_pattern_or_count() {
        let existing = RunRecipe {
            agent_id: "agent".into(),
            task_family: "delivery_search".into(),
            sample_request: "send the status page".into(),
            summary: "pattern=web_search -> message_send".into(),
            tool_pattern: vec!["web_search".into(), "message_send".into()],
            success_count: 3,
            updated_at: 10,
        };
        let candidate = RunRecipeLearningCandidate {
            task_family_hint: "delivery_search".into(),
            sample_request: "search the status page and send it".into(),
            summary: "pattern=web_search -> web_fetch -> message_send".into(),
            tool_pattern: vec![
                "web_search".into(),
                "web_fetch".into(),
                "message_send".into(),
            ],
        };

        let merged = merge_existing_recipe(&existing, &candidate, 20);
        assert_eq!(merged.success_count, 4);
        assert_eq!(
            merged.tool_pattern,
            vec![
                "web_search".to_string(),
                "message_send".to_string(),
                "web_fetch".to_string()
            ]
        );
        assert_eq!(merged.updated_at, 20);
        assert!(merged.summary.contains("web_fetch"));
    }

    #[test]
    fn builds_new_recipe_from_candidate() {
        let candidate = RunRecipeLearningCandidate {
            task_family_hint: "resource_delivery".into(),
            sample_request: "send the latest backup report".into(),
            summary: "pattern=shell -> message_send".into(),
            tool_pattern: vec!["shell".into(), "message_send".into()],
        };

        let recipe = build_new_recipe("agent", &candidate, 42);
        assert_eq!(recipe.agent_id, "agent");
        assert_eq!(recipe.task_family, "resource_delivery");
        assert_eq!(recipe.success_count, 1);
        assert_eq!(recipe.updated_at, 42);
    }
}
