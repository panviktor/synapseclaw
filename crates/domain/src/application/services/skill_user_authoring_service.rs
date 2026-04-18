//! User-authored skill normalization.
//!
//! This service turns plain operator-authored markdown into the same memory
//! `Skill` shape used by generated skills. It does not ask users to write the
//! internal governance envelope by hand.

use crate::domain::memory::{Skill, SkillOrigin, SkillStatus};
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAuthoredSkillPolicy {
    pub max_name_chars: usize,
    pub max_description_chars: usize,
    pub max_body_chars: usize,
    pub max_tags: usize,
    pub max_tag_chars: usize,
    pub max_tools: usize,
    pub max_tool_chars: usize,
}

impl Default for UserAuthoredSkillPolicy {
    fn default() -> Self {
        Self {
            max_name_chars: 96,
            max_description_chars: 280,
            max_body_chars: 16_000,
            max_tags: 12,
            max_tag_chars: 48,
            max_tools: 24,
            max_tool_chars: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAuthoredSkillInput {
    pub name: String,
    pub description: Option<String>,
    pub body: String,
    pub task_family: Option<String>,
    pub tool_pattern: Vec<String>,
    pub tags: Vec<String>,
    pub status: SkillStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserAuthoredSkillFindingKind {
    MissingName,
    MissingBody,
    NameTooLong,
    DescriptionTooLong,
    BodyTooLong,
    TooManyTags,
    TagTooLong,
    TooManyTools,
    ToolNameTooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserAuthoredSkillFinding {
    pub kind: UserAuthoredSkillFindingKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserAuthoredSkillValidationReport {
    pub accepted: bool,
    pub findings: Vec<UserAuthoredSkillFinding>,
}

impl UserAuthoredSkillValidationReport {
    pub fn is_clean(&self) -> bool {
        self.accepted
    }

    pub fn summary(&self) -> String {
        self.findings
            .iter()
            .map(|finding| finding.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, Clone)]
pub struct UserAuthoredSkillBuild {
    pub skill: Skill,
    pub validation: UserAuthoredSkillValidationReport,
}

pub fn build_user_authored_skill(
    agent_id: &str,
    input: UserAuthoredSkillInput,
    policy: &UserAuthoredSkillPolicy,
    now: DateTime<Utc>,
) -> Result<UserAuthoredSkillBuild, UserAuthoredSkillValidationReport> {
    let name = input.name.trim().to_string();
    let body = normalize_body(&input.body);
    let description = normalized_description(input.description.as_deref(), &body, policy);
    let task_family = input.task_family.as_deref().and_then(normalize_optional);
    let tool_pattern = normalize_deduped_list(input.tool_pattern);
    let tags = normalize_tags(input.tags);
    let mut validation =
        validate_user_authored_skill(&name, &description, &body, &tool_pattern, &tags, policy);
    if !validation.findings.is_empty() {
        validation.accepted = false;
        return Err(validation);
    }

    let lineage_task_families = task_family.iter().cloned().collect::<Vec<_>>();
    let skill = Skill {
        id: String::new(),
        name,
        description,
        content: body,
        task_family,
        tool_pattern,
        lineage_task_families,
        tags,
        success_count: 0,
        fail_count: 0,
        version: 1,
        origin: SkillOrigin::Manual,
        status: input.status,
        created_by: agent_id.to_string(),
        created_at: now,
        updated_at: now,
    };

    Ok(UserAuthoredSkillBuild { skill, validation })
}

fn validate_user_authored_skill(
    name: &str,
    description: &str,
    body: &str,
    tool_pattern: &[String],
    tags: &[String],
    policy: &UserAuthoredSkillPolicy,
) -> UserAuthoredSkillValidationReport {
    let mut findings = Vec::new();
    if name.is_empty() {
        findings.push(finding(
            UserAuthoredSkillFindingKind::MissingName,
            "skill name must not be empty",
        ));
    }
    if body.trim().is_empty() {
        findings.push(finding(
            UserAuthoredSkillFindingKind::MissingBody,
            "skill body must not be empty",
        ));
    }
    if name.chars().count() > policy.max_name_chars {
        findings.push(finding(
            UserAuthoredSkillFindingKind::NameTooLong,
            format!("skill name exceeds {} characters", policy.max_name_chars),
        ));
    }
    if description.chars().count() > policy.max_description_chars {
        findings.push(finding(
            UserAuthoredSkillFindingKind::DescriptionTooLong,
            format!(
                "skill description exceeds {} characters",
                policy.max_description_chars
            ),
        ));
    }
    if body.chars().count() > policy.max_body_chars {
        findings.push(finding(
            UserAuthoredSkillFindingKind::BodyTooLong,
            format!("skill body exceeds {} characters", policy.max_body_chars),
        ));
    }
    if tags.len() > policy.max_tags {
        findings.push(finding(
            UserAuthoredSkillFindingKind::TooManyTags,
            format!("skill has more than {} tags", policy.max_tags),
        ));
    }
    for tag in tags {
        if tag.chars().count() > policy.max_tag_chars {
            findings.push(finding(
                UserAuthoredSkillFindingKind::TagTooLong,
                format!(
                    "skill tag `{tag}` exceeds {} characters",
                    policy.max_tag_chars
                ),
            ));
        }
    }
    if tool_pattern.len() > policy.max_tools {
        findings.push(finding(
            UserAuthoredSkillFindingKind::TooManyTools,
            format!("skill has more than {} tool hints", policy.max_tools),
        ));
    }
    for tool in tool_pattern {
        if tool.chars().count() > policy.max_tool_chars {
            findings.push(finding(
                UserAuthoredSkillFindingKind::ToolNameTooLong,
                format!(
                    "skill tool hint `{tool}` exceeds {} characters",
                    policy.max_tool_chars
                ),
            ));
        }
    }

    UserAuthoredSkillValidationReport {
        accepted: findings.is_empty(),
        findings,
    }
}

fn finding(
    kind: UserAuthoredSkillFindingKind,
    detail: impl Into<String>,
) -> UserAuthoredSkillFinding {
    UserAuthoredSkillFinding {
        kind,
        detail: detail.into(),
    }
}

fn normalized_description(
    explicit: Option<&str>,
    body: &str,
    _policy: &UserAuthoredSkillPolicy,
) -> String {
    let raw = explicit
        .and_then(normalize_optional)
        .unwrap_or_else(|| extract_description(body));
    raw.trim().to_string()
}

fn extract_description(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("User-authored skill.")
        .to_string()
}

fn normalize_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

fn normalize_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_tags(values: Vec<String>) -> Vec<String> {
    let mut tags = normalize_deduped_list(values);
    if !tags.iter().any(|tag| tag.to_lowercase() == "user-authored") {
        tags.insert(0, "user-authored".to_string());
    }
    tags
}

fn normalize_deduped_list(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if !out
            .iter()
            .any(|current| current.to_lowercase() == value.to_lowercase())
        {
            out.push(value.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> UserAuthoredSkillInput {
        UserAuthoredSkillInput {
            name: "Matrix release check".into(),
            description: None,
            body: "# Matrix release check\n\nFind local repo and compare tags.".into(),
            task_family: Some("release-audit".into()),
            tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
            tags: vec!["matrix".into(), "Matrix".into()],
            status: SkillStatus::Active,
        }
    }

    #[test]
    fn builds_manual_memory_skill_from_plain_markdown() {
        let built = build_user_authored_skill(
            "agent-a",
            input(),
            &UserAuthoredSkillPolicy::default(),
            Utc::now(),
        )
        .expect("valid skill");

        assert_eq!(built.skill.origin, SkillOrigin::Manual);
        assert_eq!(built.skill.status, SkillStatus::Active);
        assert_eq!(built.skill.name, "Matrix release check");
        assert_eq!(built.skill.description, "Find local repo and compare tags.");
        assert_eq!(
            built.skill.tool_pattern,
            vec!["repo_discovery".to_string(), "git_operations".to_string()]
        );
        assert_eq!(
            built.skill.tags,
            vec!["user-authored".to_string(), "matrix".to_string()]
        );
        assert!(built.skill.content.ends_with('\n'));
    }

    #[test]
    fn rejects_empty_name_and_body() {
        let mut input = input();
        input.name.clear();
        input.body = " \n ".into();
        let report = build_user_authored_skill(
            "agent-a",
            input,
            &UserAuthoredSkillPolicy::default(),
            Utc::now(),
        )
        .expect_err("invalid skill");

        assert!(!report.accepted);
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.kind == UserAuthoredSkillFindingKind::MissingName));
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.kind == UserAuthoredSkillFindingKind::MissingBody));
    }
}
