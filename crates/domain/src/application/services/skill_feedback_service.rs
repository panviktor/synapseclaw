//! Deterministic feedback from failure learning into learned skills.
//!
//! Repeated failure patterns should cool down overlapping learned skills. This
//! uses structured skill metadata (`task_family`, `tool_pattern`) instead of
//! parsing free-form skill content.

use crate::application::services::learning_candidate_service::FailureLearningCandidate;
use crate::domain::memory::{Skill, SkillOrigin, SkillStatus};

const TOOL_PATTERN_MATCH_THRESHOLD: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SkillFailureFeedback {
    pub skill_id: String,
    pub skill_name: String,
    pub reason: &'static str,
}

pub fn assess_failure_feedback(
    failure: &FailureLearningCandidate,
    skills: &[Skill],
) -> Vec<SkillFailureFeedback> {
    skills
        .iter()
        .filter(|skill| {
            skill.origin == SkillOrigin::Learned
                && matches!(skill.status, SkillStatus::Active | SkillStatus::Candidate)
        })
        .filter_map(|skill| {
            if !skill.tool_pattern.is_empty()
                && tool_pattern_similarity(&failure.tool_pattern, &skill.tool_pattern)
                    >= TOOL_PATTERN_MATCH_THRESHOLD
            {
                return Some(SkillFailureFeedback {
                    skill_id: skill.id.clone(),
                    skill_name: skill.name.clone(),
                    reason: "failed_tool_pattern_overlap",
                });
            }

            let task_family = skill.task_family.as_deref()?;
            if failure
                .subjects
                .iter()
                .any(|subject| subject.eq_ignore_ascii_case(task_family))
            {
                return Some(SkillFailureFeedback {
                    skill_id: skill.id.clone(),
                    skill_name: skill.name.clone(),
                    reason: "failed_task_family_subject",
                });
            }
            None
        })
        .collect()
}

fn tool_pattern_similarity(left: &[String], right: &[String]) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn skill(
        id: &str,
        name: &str,
        status: SkillStatus,
        tool_pattern: &[&str],
        task_family: Option<&str>,
    ) -> Skill {
        Skill {
            id: id.into(),
            name: name.into(),
            description: "desc".into(),
            content: "content".into(),
            task_family: task_family.map(str::to_string),
            lineage_task_families: task_family
                .map(|value| vec![value.to_string()])
                .unwrap_or_default(),
            tool_pattern: tool_pattern.iter().map(|item| item.to_string()).collect(),
            tags: vec![],
            success_count: 4,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Learned,
            status,
            created_by: "agent".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn failure(tool_pattern: &[&str], subjects: &[&str]) -> FailureLearningCandidate {
        FailureLearningCandidate {
            summary: "failed_tools=web_fetch".into(),
            failed_tools: vec!["web_fetch".into()],
            outcome_statuses: vec![],
            tool_pattern: tool_pattern.iter().map(|item| item.to_string()).collect(),
            subjects: subjects.iter().map(|item| item.to_string()).collect(),
        }
    }

    #[test]
    fn matches_learned_skill_by_tool_pattern_overlap() {
        let feedback = assess_failure_feedback(
            &failure(&["web_fetch", "message_send"], &[]),
            &[skill(
                "sk1",
                "search_delivery",
                SkillStatus::Active,
                &["web_fetch", "message_send"],
                Some("search_delivery"),
            )],
        );

        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0].reason, "failed_tool_pattern_overlap");
    }

    #[test]
    fn ignores_manual_or_deprecated_skills() {
        let mut manual = skill(
            "sk1",
            "search_delivery",
            SkillStatus::Active,
            &["web_fetch", "message_send"],
            Some("search_delivery"),
        );
        manual.origin = SkillOrigin::Manual;
        let deprecated = skill(
            "sk2",
            "search_delivery",
            SkillStatus::Deprecated,
            &["web_fetch", "message_send"],
            Some("search_delivery"),
        );

        let feedback = assess_failure_feedback(
            &failure(&["web_fetch", "message_send"], &[]),
            &[manual, deprecated],
        );

        assert!(feedback.is_empty());
    }
}
