//! Promote repeated recipes into learned skill candidates or active skills.
//!
//! This is the deterministic bridge between repeated procedural evidence and
//! the skill surface. It deliberately uses recipe success counts as the first
//! repetition signal instead of introducing another model call.

use crate::domain::memory::{Skill, SkillOrigin, SkillStatus, SkillUpdate};
use crate::domain::run_recipe::RunRecipe;
use chrono::Utc;
use serde::Serialize;

const SKILL_CANDIDATE_SUCCESS_THRESHOLD: u32 = 3;
const SKILL_ACTIVE_SUCCESS_THRESHOLD: u32 = 5;

#[derive(Debug, Clone, Serialize)]
pub struct SkillPromotionAssessment {
    pub skill_name: String,
    pub accepted: bool,
    pub reason: &'static str,
    pub target_status: SkillStatus,
}

pub fn assess_recipe_for_skill_promotion(
    recipe: &RunRecipe,
    existing: Option<&Skill>,
    all_skills: &[Skill],
) -> SkillPromotionAssessment {
    let skill_name = build_skill_name(recipe);

    if recipe.success_count < SKILL_CANDIDATE_SUCCESS_THRESHOLD {
        return SkillPromotionAssessment {
            skill_name,
            accepted: false,
            reason: "insufficient_repetition",
            target_status: SkillStatus::Candidate,
        };
    }

    if recipe.tool_pattern.len() < 2 || recipe.summary.trim().is_empty() {
        return SkillPromotionAssessment {
            skill_name,
            accepted: false,
            reason: "weak_recipe_shape",
            target_status: SkillStatus::Candidate,
        };
    }

    if let Some(shadowing_skill) =
        find_shadowing_higher_origin_skill(recipe, &skill_name, all_skills)
    {
        return SkillPromotionAssessment {
            skill_name,
            accepted: false,
            reason: "shadowed_by_higher_origin_skill",
            target_status: shadowing_skill.status.clone(),
        };
    }

    if let Some(existing_skill) = existing {
        if matches!(
            existing_skill.origin,
            SkillOrigin::Manual | SkillOrigin::Imported
        ) {
            return SkillPromotionAssessment {
                skill_name,
                accepted: false,
                reason: "manual_or_imported_skill_exists",
                target_status: existing_skill.status.clone(),
            };
        }
    }

    let repeated_target_status = if recipe.success_count >= SKILL_ACTIVE_SUCCESS_THRESHOLD {
        SkillStatus::Active
    } else {
        SkillStatus::Candidate
    };
    let target_status = existing
        .map(|skill| max_skill_status(&skill.status, &repeated_target_status))
        .unwrap_or(repeated_target_status);

    let reason = match existing {
        Some(existing_skill)
            if existing_skill.origin == SkillOrigin::Learned
                && existing_skill.status != target_status =>
        {
            "promote_learned_skill"
        }
        Some(existing_skill) if existing_skill.origin == SkillOrigin::Learned => {
            "refresh_learned_skill"
        }
        Some(_) => "shadowed_by_higher_origin_skill",
        None if target_status == SkillStatus::Active => "create_active_skill",
        None => "create_candidate_skill",
    };

    SkillPromotionAssessment {
        skill_name,
        accepted: true,
        reason,
        target_status,
    }
}

pub fn build_skill_name(recipe: &RunRecipe) -> String {
    recipe.task_family.trim().to_string()
}

pub fn build_new_skill(
    agent_id: &str,
    recipe: &RunRecipe,
    assessment: &SkillPromotionAssessment,
) -> Skill {
    let now = Utc::now();
    Skill {
        id: String::new(),
        name: assessment.skill_name.clone(),
        description: build_skill_description(recipe),
        content: build_skill_content(recipe),
        task_family: Some(recipe.task_family.clone()),
        tool_pattern: recipe.tool_pattern.clone(),
        tags: vec!["recipe-promotion".into(), recipe.task_family.clone()],
        success_count: recipe.success_count,
        fail_count: 0,
        version: 1,
        origin: SkillOrigin::Learned,
        status: assessment.target_status.clone(),
        created_by: agent_id.to_string(),
        created_at: now,
        updated_at: now,
    }
}

pub fn build_skill_update(
    recipe: &RunRecipe,
    assessment: &SkillPromotionAssessment,
) -> SkillUpdate {
    SkillUpdate {
        increment_success: true,
        increment_fail: false,
        new_description: Some(build_skill_description(recipe)),
        new_content: Some(build_skill_content(recipe)),
        new_task_family: Some(Some(recipe.task_family.clone())),
        new_tool_pattern: Some(recipe.tool_pattern.clone()),
        new_status: Some(assessment.target_status.clone()),
    }
}

fn build_skill_description(recipe: &RunRecipe) -> String {
    format!(
        "Promoted from repeated '{}' recipe executions.",
        recipe.task_family
    )
}

fn build_skill_content(recipe: &RunRecipe) -> String {
    let mut lines = vec![
        "## When to Apply".to_string(),
        format!("- task_family: {}", recipe.task_family),
        format!("- example_request: {}", recipe.sample_request),
        "## Recommended Flow".to_string(),
    ];
    for (index, tool) in recipe.tool_pattern.iter().enumerate() {
        lines.push(format!("{}. {}", index + 1, tool));
    }
    lines.push("## Summary".to_string());
    lines.push(recipe.summary.trim().to_string());
    format!("{}\n", lines.join("\n"))
}

fn max_skill_status(left: &SkillStatus, right: &SkillStatus) -> SkillStatus {
    match (status_rank(left), status_rank(right)) {
        (left_rank, right_rank) if left_rank >= right_rank => left.clone(),
        _ => right.clone(),
    }
}

fn find_shadowing_higher_origin_skill<'a>(
    recipe: &RunRecipe,
    skill_name: &str,
    all_skills: &'a [Skill],
) -> Option<&'a Skill> {
    all_skills.iter().find(|skill| {
        skill.status == SkillStatus::Active
            && matches!(skill.origin, SkillOrigin::Manual | SkillOrigin::Imported)
            && !skill.name.eq_ignore_ascii_case(skill_name)
            && (skill
                .task_family
                .as_deref()
                .is_some_and(|task_family| task_family.eq_ignore_ascii_case(&recipe.task_family))
                || tool_pattern_overlap(&skill.tool_pattern, &recipe.tool_pattern) >= 0.75)
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

fn status_rank(status: &SkillStatus) -> u8 {
    match status {
        SkillStatus::Active => 2,
        SkillStatus::Candidate => 1,
        SkillStatus::Deprecated => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_recipe(success_count: u32) -> RunRecipe {
        RunRecipe {
            agent_id: "agent".into(),
            task_family: "search_delivery".into(),
            sample_request: "find the status page and send it".into(),
            summary: "Use web search, confirm the page, then deliver it.".into(),
            tool_pattern: vec!["web_search".into(), "message_send".into()],
            success_count,
            updated_at: 1,
        }
    }

    #[test]
    fn rejects_recipe_without_enough_repetition() {
        let assessment = assess_recipe_for_skill_promotion(&sample_recipe(2), None, &[]);
        assert!(!assessment.accepted);
        assert_eq!(assessment.reason, "insufficient_repetition");
    }

    #[test]
    fn creates_candidate_skill_from_repeated_recipe() {
        let assessment = assess_recipe_for_skill_promotion(&sample_recipe(3), None, &[]);
        assert!(assessment.accepted);
        assert_eq!(assessment.target_status, SkillStatus::Candidate);
        assert_eq!(assessment.reason, "create_candidate_skill");
    }

    #[test]
    fn promotes_repeated_recipe_to_active_skill() {
        let assessment = assess_recipe_for_skill_promotion(&sample_recipe(5), None, &[]);
        assert!(assessment.accepted);
        assert_eq!(assessment.target_status, SkillStatus::Active);
        assert_eq!(assessment.reason, "create_active_skill");
    }

    #[test]
    fn respects_manual_skill_boundary() {
        let existing = Skill {
            id: "sk1".into(),
            name: "search_delivery".into(),
            description: "Manual skill".into(),
            content: "Use manual process.".into(),
            task_family: Some("search_delivery".into()),
            tool_pattern: vec!["shell".into(), "message_send".into()],
            tags: vec![],
            success_count: 1,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Manual,
            status: SkillStatus::Active,
            created_by: "agent".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let assessment = assess_recipe_for_skill_promotion(&sample_recipe(6), Some(&existing), &[]);
        assert!(!assessment.accepted);
        assert_eq!(assessment.reason, "manual_or_imported_skill_exists");
    }

    #[test]
    fn rejects_recipe_shadowed_by_active_manual_skill() {
        let manual_skill = Skill {
            id: "sk2".into(),
            name: "manual_status_delivery".into(),
            description: "Manual skill".into(),
            content: "Use a fixed manual process.".into(),
            task_family: Some("search_delivery".into()),
            tool_pattern: vec!["web_search".into(), "message_send".into()],
            tags: vec![],
            success_count: 1,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Manual,
            status: SkillStatus::Active,
            created_by: "agent".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let assessment =
            assess_recipe_for_skill_promotion(&sample_recipe(6), None, &[manual_skill]);
        assert!(!assessment.accepted);
        assert_eq!(assessment.reason, "shadowed_by_higher_origin_skill");
    }
}
