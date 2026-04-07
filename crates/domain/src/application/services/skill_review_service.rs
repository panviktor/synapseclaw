//! Deferred review and compaction for learned skills.
//!
//! This keeps the skill surface clean without invoking another model:
//! repeated successful candidates can become active, while weak, shadowed, or
//! duplicate learned skills get deprecated.

use crate::domain::memory::{MemoryId, Skill, SkillOrigin, SkillStatus};
use std::cmp::Ordering;

const ACTIVE_SUCCESS_THRESHOLD: u32 = 5;
const FAILURE_DOMINANT_THRESHOLD: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillReviewAction {
    PromoteToActive,
    Deprecate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillReviewDecision {
    pub skill_id: MemoryId,
    pub skill_name: String,
    pub action: SkillReviewAction,
    pub target_status: SkillStatus,
    pub reason: &'static str,
}

pub fn review_learned_skills(skills: &[Skill]) -> Vec<SkillReviewDecision> {
    skills
        .iter()
        .filter(|skill| skill.origin == SkillOrigin::Learned)
        .filter(|skill| skill.status != SkillStatus::Deprecated)
        .filter_map(|skill| review_learned_skill(skill, skills))
        .collect()
}

fn review_learned_skill(skill: &Skill, all_skills: &[Skill]) -> Option<SkillReviewDecision> {
    if is_shadowed_by_higher_priority_active_skill(skill, all_skills) {
        return Some(SkillReviewDecision {
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            action: SkillReviewAction::Deprecate,
            target_status: SkillStatus::Deprecated,
            reason: "shadowed_by_higher_priority_active_skill",
        });
    }

    if is_duplicate_of_preferred_learned_skill(skill, all_skills) {
        return Some(SkillReviewDecision {
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            action: SkillReviewAction::Deprecate,
            target_status: SkillStatus::Deprecated,
            reason: "duplicate_learned_skill",
        });
    }

    if skill.status == SkillStatus::Candidate
        && skill.success_count >= ACTIVE_SUCCESS_THRESHOLD
        && skill.success_count > skill.fail_count
    {
        return Some(SkillReviewDecision {
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            action: SkillReviewAction::PromoteToActive,
            target_status: SkillStatus::Active,
            reason: "repeated_successes",
        });
    }

    if skill.status == SkillStatus::Candidate
        && skill.fail_count >= FAILURE_DOMINANT_THRESHOLD
        && skill.fail_count > skill.success_count
    {
        return Some(SkillReviewDecision {
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            action: SkillReviewAction::Deprecate,
            target_status: SkillStatus::Deprecated,
            reason: "failure_dominant_candidate",
        });
    }

    None
}

fn is_shadowed_by_higher_priority_active_skill(skill: &Skill, all_skills: &[Skill]) -> bool {
    all_skills.iter().any(|other| {
        other.id != skill.id
            && other.status == SkillStatus::Active
            && skill_priority(other) > skill_priority(skill)
            && skills_overlap(skill, other)
    })
}

fn is_duplicate_of_preferred_learned_skill(skill: &Skill, all_skills: &[Skill]) -> bool {
    all_skills.iter().any(|other| {
        other.id != skill.id
            && other.origin == SkillOrigin::Learned
            && other.status != SkillStatus::Deprecated
            && skills_overlap(skill, other)
            && preferred_skill_cmp(other, skill) == Ordering::Greater
    })
}

fn skills_overlap(left: &Skill, right: &Skill) -> bool {
    left.name.eq_ignore_ascii_case(&right.name)
        || left
            .task_family
            .as_deref()
            .zip(right.task_family.as_deref())
            .is_some_and(|(left_family, right_family)| {
                left_family.eq_ignore_ascii_case(right_family)
            })
        || tool_pattern_overlap(&left.tool_pattern, &right.tool_pattern) >= 0.75
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

fn preferred_skill_cmp(left: &Skill, right: &Skill) -> Ordering {
    skill_status_rank(&left.status)
        .cmp(&skill_status_rank(&right.status))
        .then(left.success_count.cmp(&right.success_count))
        .then(right.fail_count.cmp(&left.fail_count))
        .then(left.updated_at.cmp(&right.updated_at))
        .then_with(|| right.id.cmp(&left.id))
}

fn skill_status_rank(status: &SkillStatus) -> u8 {
    match status {
        SkillStatus::Active => 2,
        SkillStatus::Candidate => 1,
        SkillStatus::Deprecated => 0,
    }
}

fn skill_priority(skill: &Skill) -> u8 {
    match skill.origin {
        SkillOrigin::Manual => 3,
        SkillOrigin::Imported => 2,
        SkillOrigin::Learned => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_skill(
        id: &str,
        name: &str,
        origin: SkillOrigin,
        status: SkillStatus,
        success_count: u32,
        fail_count: u32,
    ) -> Skill {
        Skill {
            id: id.into(),
            name: name.into(),
            description: "desc".into(),
            content: "content".into(),
            task_family: Some(name.into()),
            tool_pattern: vec!["web_search".into(), "message_send".into()],
            tags: vec![],
            success_count,
            fail_count,
            version: 1,
            origin,
            status,
            created_by: "agent".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn promotes_repeated_candidate_skill() {
        let decisions = review_learned_skills(&[sample_skill(
            "sk1",
            "search_delivery",
            SkillOrigin::Learned,
            SkillStatus::Candidate,
            5,
            1,
        )]);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].action, SkillReviewAction::PromoteToActive);
        assert_eq!(decisions[0].target_status, SkillStatus::Active);
    }

    #[test]
    fn deprecates_candidate_shadowed_by_manual_skill() {
        let decisions = review_learned_skills(&[
            sample_skill(
                "sk1",
                "search_delivery",
                SkillOrigin::Learned,
                SkillStatus::Candidate,
                4,
                0,
            ),
            sample_skill(
                "sk2",
                "search_delivery",
                SkillOrigin::Manual,
                SkillStatus::Active,
                1,
                0,
            ),
        ]);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].action, SkillReviewAction::Deprecate);
        assert_eq!(
            decisions[0].reason,
            "shadowed_by_higher_priority_active_skill"
        );
    }

    #[test]
    fn deprecates_failure_dominant_candidate() {
        let decisions = review_learned_skills(&[sample_skill(
            "sk1",
            "search_delivery",
            SkillOrigin::Learned,
            SkillStatus::Candidate,
            1,
            3,
        )]);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].action, SkillReviewAction::Deprecate);
        assert_eq!(decisions[0].target_status, SkillStatus::Deprecated);
        assert_eq!(decisions[0].reason, "failure_dominant_candidate");
    }

    #[test]
    fn deprecates_duplicate_weaker_learned_skill() {
        let older_candidate = sample_skill(
            "sk1",
            "search_delivery_candidate",
            SkillOrigin::Learned,
            SkillStatus::Candidate,
            3,
            0,
        );
        let mut stronger_active = sample_skill(
            "sk2",
            "search_delivery",
            SkillOrigin::Learned,
            SkillStatus::Active,
            7,
            1,
        );
        stronger_active.task_family = Some("search_delivery".into());

        let decisions = review_learned_skills(&[older_candidate, stronger_active]);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].action, SkillReviewAction::Deprecate);
        assert_eq!(decisions[0].reason, "duplicate_learned_skill");
    }
}
