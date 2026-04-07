//! Deferred review for candidate learned skills.
//!
//! This keeps the skill surface clean without invoking another model:
//! repeated successful candidates can become active, while weak or shadowed
//! candidates get deprecated.

use crate::domain::memory::{MemoryId, Skill, SkillOrigin, SkillStatus};

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

pub fn review_candidate_skills(skills: &[Skill]) -> Vec<SkillReviewDecision> {
    skills
        .iter()
        .filter(|skill| skill.status == SkillStatus::Candidate)
        .filter_map(|skill| review_candidate_skill(skill, skills))
        .collect()
}

fn review_candidate_skill(skill: &Skill, all_skills: &[Skill]) -> Option<SkillReviewDecision> {
    if skill.origin != SkillOrigin::Learned {
        return None;
    }

    if is_shadowed_by_higher_priority_active_skill(skill, all_skills) {
        return Some(SkillReviewDecision {
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            action: SkillReviewAction::Deprecate,
            target_status: SkillStatus::Deprecated,
            reason: "shadowed_by_higher_priority_active_skill",
        });
    }

    if skill.success_count >= ACTIVE_SUCCESS_THRESHOLD && skill.success_count > skill.fail_count {
        return Some(SkillReviewDecision {
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            action: SkillReviewAction::PromoteToActive,
            target_status: SkillStatus::Active,
            reason: "repeated_successes",
        });
    }

    if skill.fail_count >= FAILURE_DOMINANT_THRESHOLD && skill.fail_count > skill.success_count {
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
            && other.name.eq_ignore_ascii_case(&skill.name)
            && skill_priority(other) > skill_priority(skill)
    })
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
        let decisions = review_candidate_skills(&[sample_skill(
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
        let decisions = review_candidate_skills(&[
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
        let decisions = review_candidate_skills(&[sample_skill(
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
}
