//! Human-readable memory projections.
//!
//! These are cheap, regenerable views over structured state. They are meant
//! for operators and UI inspection, not as the canonical source of truth.

use crate::application::services::learning_maintenance_service::LearningMaintenancePlan;
use crate::application::services::procedural_cluster_service::ProceduralCluster;
use crate::application::services::procedural_contradiction_service::ProceduralContradiction;
use crate::application::services::run_recipe_cluster_service::RunRecipeCluster;
use crate::application::services::skill_review_service::SkillReviewDecision;
use crate::domain::conversation::{ConversationEvent, ConversationSession, EventType};
use crate::domain::dialogue_state::DialogueState;
use crate::domain::memory::{CoreMemoryBlock, MemoryEntry, Skill};
use crate::domain::run_recipe::RunRecipe;

#[derive(Debug, Clone)]
pub struct LearningDigestProjectionInput {
    pub has_current_profile: bool,
    pub effective_skill_names: Vec<String>,
    pub candidate_skill_count: usize,
    pub shadowed_skill_count: usize,
    pub run_recipe_families: Vec<String>,
    pub run_recipe_cluster_count: usize,
    pub procedural_contradiction_count: usize,
    pub precedent_count: usize,
    pub precedent_cluster_count: usize,
    pub failure_pattern_count: usize,
    pub failure_pattern_cluster_count: usize,
}

pub fn format_core_blocks_projection(blocks: &[CoreMemoryBlock]) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }

    let mut lines = vec!["[core-memory]".to_string()];
    for block in blocks {
        if block.content.trim().is_empty() {
            continue;
        }
        lines.push(format!(
            "- {} ({} chars)",
            block.label,
            block.content.chars().count()
        ));
        lines.push(indent_multiline(block.content.trim(), 2));
    }

    Some(format!("{}\n", lines.join("\n")))
}

pub fn format_dialogue_state_projection(state: &DialogueState) -> Option<String> {
    if state.focus_entities.is_empty()
        && state.comparison_set.is_empty()
        && state.last_tool_subjects.is_empty()
        && state.recent_delivery_target.is_none()
        && state.recent_schedule_job.is_none()
        && state.recent_resource.is_none()
        && state.recent_search.is_none()
        && state.recent_workspace.is_none()
    {
        return None;
    }

    let mut lines = vec!["[working-state]".to_string()];
    if !state.focus_entities.is_empty() {
        lines.push(format!(
            "- focus_entities: {}",
            state
                .focus_entities
                .iter()
                .map(|entity| format!("{}={}", entity.kind, entity.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !state.comparison_set.is_empty() {
        lines.push(format!(
            "- comparison_set: {}",
            state
                .comparison_set
                .iter()
                .map(|entity| format!("{}={}", entity.kind, entity.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !state.last_tool_subjects.is_empty() {
        lines.push(format!(
            "- last_tool_subjects: {}",
            state.last_tool_subjects.join(", ")
        ));
    }
    if let Some(target) = &state.recent_delivery_target {
        lines.push(format!("- recent_delivery_target: {target:?}"));
    }
    if let Some(job) = &state.recent_schedule_job {
        lines.push(format!("- recent_schedule_job: {}", job.job_id));
    }
    if let Some(resource) = &state.recent_resource {
        lines.push(format!("- recent_resource: {}", resource.locator));
    }
    if let Some(search) = &state.recent_search {
        if let Some(query) = &search.query {
            lines.push(format!("- recent_search_query: {query}"));
        }
        if let Some(locator) = &search.primary_locator {
            lines.push(format!("- recent_search_result: {locator}"));
        }
    }
    if let Some(workspace) = &state.recent_workspace {
        if let Some(name) = &workspace.name {
            lines.push(format!("- recent_workspace: {name}"));
        }
    }

    Some(format!("{}\n", lines.join("\n")))
}

pub fn format_session_projection(
    session: &ConversationSession,
    recap_events: &[ConversationEvent],
) -> String {
    let mut lines = vec![
        "[session]".to_string(),
        format!("- key: {}", session.key),
        format!("- kind: {}", session.kind),
    ];

    if let Some(label) = session.label.as_deref() {
        lines.push(format!("- label: {label}"));
    }
    if let Some(summary) = session.summary.as_deref() {
        lines.push("- summary:".to_string());
        lines.push(indent_multiline(summary.trim(), 2));
    }
    if let Some(goal) = session.current_goal.as_deref() {
        lines.push(format!("- current_goal: {goal}"));
    }
    lines.push(format!("- message_count: {}", session.message_count));

    if let Some(recap) = format_recap_events(recap_events) {
        lines.push("- recent_recap:".to_string());
        lines.push(indent_multiline(&recap, 2));
    }

    format!("{}\n", lines.join("\n"))
}

pub fn format_run_recipe_projection(recipe: &RunRecipe) -> String {
    let mut lines = vec![
        "[run-recipe]".to_string(),
        format!("- task_family: {}", recipe.task_family),
        format!("- success_count: {}", recipe.success_count),
        format!("- sample_request: {}", recipe.sample_request),
    ];
    if !recipe.tool_pattern.is_empty() {
        lines.push(format!(
            "- tool_pattern: {}",
            recipe.tool_pattern.join(" -> ")
        ));
    }
    if !recipe.summary.trim().is_empty() {
        lines.push("- summary:".to_string());
        lines.push(indent_multiline(recipe.summary.trim(), 2));
    }

    format!("{}\n", lines.join("\n"))
}

pub fn format_skill_projection(skill: &Skill) -> String {
    let mut lines = vec![
        "[skill]".to_string(),
        format!("- name: {}", skill.name),
        format!("- origin: {}", skill.origin),
        format!("- status: {}", skill.status),
        format!("- success_count: {}", skill.success_count),
        format!("- fail_count: {}", skill.fail_count),
        format!("- version: {}", skill.version),
    ];
    if let Some(task_family) = &skill.task_family {
        lines.push(format!("- task_family: {task_family}"));
    }
    if !skill.tool_pattern.is_empty() {
        lines.push(format!(
            "- tool_pattern: {}",
            skill.tool_pattern.join(" -> ")
        ));
    }
    if !skill.description.trim().is_empty() {
        lines.push("- description:".to_string());
        lines.push(indent_multiline(skill.description.trim(), 2));
    }
    if !skill.content.trim().is_empty() {
        lines.push("- content:".to_string());
        lines.push(indent_multiline(skill.content.trim(), 2));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn format_skill_conflict_policy_projection() -> String {
    [
        "[skill-conflict-policy]".to_string(),
        "- precedence: security/policy boundaries".to_string(),
        "- precedence: explicit current-turn user instruction".to_string(),
        "- precedence: manual skill".to_string(),
        "- precedence: imported skill".to_string(),
        "- precedence: hard user-profile defaults".to_string(),
        "- precedence: learned skill".to_string(),
        "- precedence: recipe".to_string(),
        "- precedence: precedent".to_string(),
        "- precedence: generic episodic or semantic retrieval".to_string(),
        "- note: lower-precedence skills remain inspectable but can be shadowed".to_string(),
    ]
    .join("\n")
        + "\n"
}

pub fn format_skill_review_projection(decisions: &[SkillReviewDecision]) -> Option<String> {
    if decisions.is_empty() {
        return None;
    }

    let mut lines = vec!["[skill-review]".to_string()];
    for decision in decisions {
        lines.push(format!(
            "- {} -> {:?} ({})",
            decision.skill_name, decision.action, decision.reason
        ));
    }

    Some(format!("{}\n", lines.join("\n")))
}

pub fn format_memory_entry_projection(section: &str, entry: &MemoryEntry) -> String {
    let mut lines = vec![
        format!("[{section}]"),
        format!("- key: {}", entry.key),
        format!("- category: {}", entry.category),
    ];
    if let Some(score) = entry.score {
        lines.push(format!("- score: {:.3}", score));
    }
    if let Some(session_id) = &entry.session_id {
        lines.push(format!("- session_id: {session_id}"));
    }
    if !entry.content.trim().is_empty() {
        lines.push("- content:".to_string());
        lines.push(indent_multiline(entry.content.trim(), 2));
    }

    format!("{}\n", lines.join("\n"))
}

pub fn format_learning_digest_projection(input: &LearningDigestProjectionInput) -> Option<String> {
    if !input.has_current_profile
        && input.effective_skill_names.is_empty()
        && input.candidate_skill_count == 0
        && input.shadowed_skill_count == 0
        && input.run_recipe_families.is_empty()
        && input.run_recipe_cluster_count == 0
        && input.procedural_contradiction_count == 0
        && input.precedent_count == 0
        && input.precedent_cluster_count == 0
        && input.failure_pattern_count == 0
        && input.failure_pattern_cluster_count == 0
    {
        return None;
    }

    let mut lines = vec!["[learning-digest]".to_string()];
    lines.push(format!(
        "- current_user_profile: {}",
        if input.has_current_profile {
            "present"
        } else {
            "absent"
        }
    ));
    if !input.effective_skill_names.is_empty() {
        lines.push(format!(
            "- effective_skills: {}",
            input.effective_skill_names.join(", ")
        ));
    }
    lines.push(format!(
        "- candidate_skill_count: {}",
        input.candidate_skill_count
    ));
    lines.push(format!(
        "- shadowed_skill_count: {}",
        input.shadowed_skill_count
    ));
    if !input.run_recipe_families.is_empty() {
        lines.push(format!(
            "- run_recipe_families: {}",
            input.run_recipe_families.join(", ")
        ));
    }
    lines.push(format!(
        "- run_recipe_cluster_count: {}",
        input.run_recipe_cluster_count
    ));
    lines.push(format!(
        "- procedural_contradiction_count: {}",
        input.procedural_contradiction_count
    ));
    lines.push(format!("- precedent_count: {}", input.precedent_count));
    lines.push(format!(
        "- precedent_cluster_count: {}",
        input.precedent_cluster_count
    ));
    lines.push(format!(
        "- failure_pattern_count: {}",
        input.failure_pattern_count
    ));
    lines.push(format!(
        "- failure_pattern_cluster_count: {}",
        input.failure_pattern_cluster_count
    ));

    Some(format!("{}\n", lines.join("\n")))
}

pub fn format_procedural_cluster_projection(section: &str, cluster: &ProceduralCluster) -> String {
    let mut lines = vec![
        format!("[{section}]"),
        format!("- representative_key: {}", cluster.representative.key),
        format!("- member_count: {}", cluster.member_count()),
        format!("- member_keys: {}", cluster.member_keys.join(", ")),
    ];
    if !cluster.representative.content.trim().is_empty() {
        lines.push("- representative_content:".to_string());
        lines.push(indent_multiline(cluster.representative.content.trim(), 2));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn format_run_recipe_cluster_projection(cluster: &RunRecipeCluster) -> String {
    let mut lines = vec![
        "[run-recipe-cluster]".to_string(),
        format!(
            "- representative_task_family: {}",
            cluster.representative.task_family
        ),
        format!("- member_count: {}", cluster.member_count()),
        format!(
            "- member_task_families: {}",
            cluster.member_task_families.join(", ")
        ),
    ];
    if !cluster.representative.tool_pattern.is_empty() {
        lines.push(format!(
            "- representative_tool_pattern: {}",
            cluster.representative.tool_pattern.join(" -> ")
        ));
    }
    if !cluster.representative.summary.trim().is_empty() {
        lines.push("- representative_summary:".to_string());
        lines.push(indent_multiline(cluster.representative.summary.trim(), 2));
    }
    format!("{}\n", lines.join("\n"))
}

pub fn format_procedural_contradiction_projection(
    contradictions: &[ProceduralContradiction],
) -> Option<String> {
    if contradictions.is_empty() {
        return None;
    }

    let mut lines = vec!["[procedural-contradictions]".to_string()];
    for contradiction in contradictions {
        lines.push(format!(
            "- recipe={} vs failure={} (overlap {:.2}, recipe_cluster_size={}, failure_cluster_size={})",
            contradiction.recipe_task_family,
            contradiction.failure_representative_key,
            contradiction.overlap,
            contradiction.recipe_cluster_size,
            contradiction.failure_cluster_size
        ));
    }

    Some(format!("{}\n", lines.join("\n")))
}

pub fn format_learning_maintenance_projection(plan: &LearningMaintenancePlan) -> Option<String> {
    if !plan.has_any_advisory_action() {
        return None;
    }

    let mut lines = vec!["[learning-maintenance]".to_string()];
    lines.push(format!(
        "- run_importance_decay: {}",
        plan.run_importance_decay
    ));
    lines.push(format!("- run_gc: {}", plan.run_gc));
    lines.push(format!(
        "- run_run_recipe_review: {}",
        plan.run_run_recipe_review
    ));
    lines.push(format!(
        "- run_precedent_compaction: {}",
        plan.run_precedent_compaction
    ));
    lines.push(format!(
        "- run_failure_pattern_compaction: {}",
        plan.run_failure_pattern_compaction
    ));
    lines.push(format!("- run_skill_review: {}", plan.run_skill_review));
    lines.push(format!(
        "- run_prompt_optimization: {}",
        plan.run_prompt_optimization
    ));
    if !plan.reasons.is_empty() {
        lines.push(format!(
            "- reasons: {}",
            plan.reasons
                .iter()
                .map(|reason| format!("{reason:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    Some(format!("{}\n", lines.join("\n")))
}

fn format_recap_events(events: &[ConversationEvent]) -> Option<String> {
    let lines = events
        .iter()
        .filter_map(|event| {
            let label = match event.event_type {
                EventType::User => "user",
                EventType::Assistant => "assistant",
                EventType::System => "system",
                EventType::Error => "error",
                EventType::Interrupted => "interrupted",
                EventType::ToolCall | EventType::ToolResult => return None,
            };
            let content = truncate_line(event.content.trim(), 180);
            (!content.is_empty()).then(|| format!("{label}: {content}"))
        })
        .collect::<Vec<_>>();

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn indent_multiline(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_line(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let truncated = trimmed.chars().take(max_chars).collect::<String>();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::{ConversationKind, EventType};
    use crate::domain::dialogue_state::FocusEntity;
    use chrono::Utc;

    #[test]
    fn formats_dialogue_state_projection() {
        let projection = format_dialogue_state_projection(&DialogueState {
            focus_entities: vec![FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            comparison_set: vec![FocusEntity {
                kind: "city".into(),
                name: "Tbilisi".into(),
                metadata: None,
            }],
            reference_anchors: vec![],
            last_tool_subjects: vec!["Berlin".into()],
            recent_delivery_target: None,
            recent_schedule_job: None,
            recent_resource: None,
            recent_search: None,
            recent_workspace: None,
            updated_at: 1,
        })
        .unwrap();

        assert!(projection.contains("[working-state]"));
        assert!(projection.contains("focus_entities: city=Berlin"));
        assert!(projection.contains("last_tool_subjects: Berlin"));
    }

    #[test]
    fn formats_session_projection_without_tool_noise() {
        let projection = format_session_projection(
            &ConversationSession {
                key: "web:test".into(),
                kind: ConversationKind::Web,
                label: Some("Weather".into()),
                summary: Some("Compared Berlin and Tbilisi.".into()),
                current_goal: None,
                created_at: 1,
                last_active: 2,
                message_count: 4,
                input_tokens: 0,
                output_tokens: 0,
            },
            &[
                ConversationEvent {
                    event_type: EventType::User,
                    actor: "user".into(),
                    content: "What's the weather in Berlin?".into(),
                    tool_name: None,
                    run_id: None,
                    input_tokens: None,
                    output_tokens: None,
                    timestamp: 1,
                },
                ConversationEvent {
                    event_type: EventType::ToolCall,
                    actor: "assistant".into(),
                    content: "tool call".into(),
                    tool_name: Some("weather".into()),
                    run_id: None,
                    input_tokens: None,
                    output_tokens: None,
                    timestamp: 2,
                },
                ConversationEvent {
                    event_type: EventType::Assistant,
                    actor: "assistant".into(),
                    content: "Berlin is 12C.".into(),
                    tool_name: None,
                    run_id: None,
                    input_tokens: None,
                    output_tokens: None,
                    timestamp: 3,
                },
            ],
        );

        assert!(projection.contains("[session]"));
        assert!(projection.contains("Compared Berlin and Tbilisi."));
        assert!(projection.contains("user: What's the weather in Berlin?"));
        assert!(!projection.contains("tool call"));
    }

    #[test]
    fn formats_run_recipe_projection() {
        let projection = format_run_recipe_projection(&RunRecipe {
            agent_id: "agent".into(),
            task_family: "deploy".into(),
            sample_request: "Deploy latest build".into(),
            summary: "Check staging, then deploy.".into(),
            tool_pattern: vec!["session_search".into(), "shell".into()],
            success_count: 3,
            updated_at: Utc::now().timestamp() as u64,
        });

        assert!(projection.contains("[run-recipe]"));
        assert!(projection.contains("tool_pattern: session_search -> shell"));
        assert!(projection.contains("Check staging, then deploy."));
    }

    #[test]
    fn formats_memory_entry_projection() {
        let projection = format_memory_entry_projection(
            "precedent",
            &MemoryEntry {
                id: "1".into(),
                key: "custom_abc".into(),
                content: "tools=web_search -> message_send | subjects=status.example.com".into(),
                category: crate::domain::memory::MemoryCategory::Custom("precedent".into()),
                timestamp: Utc::now().to_rfc3339(),
                session_id: None,
                score: Some(0.87),
            },
        );

        assert!(projection.contains("[precedent]"));
        assert!(projection.contains("category: precedent"));
        assert!(projection.contains("score: 0.870"));
        assert!(projection.contains("tools=web_search -> message_send"));
    }

    #[test]
    fn formats_skill_projection_with_origin_and_status() {
        let projection = format_skill_projection(&Skill {
            id: "sk1".into(),
            name: "deploy".into(),
            description: "Preferred deploy procedure".into(),
            content: "1. Build\n2. Test\n3. Deploy".into(),
            task_family: Some("deploy".into()),
            tool_pattern: vec!["shell".into(), "message_send".into()],
            tags: vec![],
            success_count: 4,
            fail_count: 1,
            version: 2,
            origin: crate::domain::memory::SkillOrigin::Manual,
            status: crate::domain::memory::SkillStatus::Active,
            created_by: "agent".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        });

        assert!(projection.contains("[skill]"));
        assert!(projection.contains("origin: manual"));
        assert!(projection.contains("status: active"));
        assert!(projection.contains("Preferred deploy procedure"));
    }

    #[test]
    fn formats_skill_conflict_policy_projection() {
        let projection = format_skill_conflict_policy_projection();

        assert!(projection.contains("[skill-conflict-policy]"));
        assert!(projection.contains("manual skill"));
        assert!(projection.contains("learned skill"));
    }

    #[test]
    fn formats_learning_digest_projection() {
        let projection = format_learning_digest_projection(&LearningDigestProjectionInput {
            has_current_profile: true,
            effective_skill_names: vec!["manual-deploy".into(), "search_delivery".into()],
            candidate_skill_count: 1,
            shadowed_skill_count: 1,
            run_recipe_families: vec!["search_delivery".into()],
            run_recipe_cluster_count: 1,
            procedural_contradiction_count: 1,
            precedent_count: 2,
            precedent_cluster_count: 1,
            failure_pattern_count: 1,
            failure_pattern_cluster_count: 1,
        })
        .unwrap();

        assert!(projection.contains("[learning-digest]"));
        assert!(projection.contains("effective_skills: manual-deploy, search_delivery"));
        assert!(projection.contains("candidate_skill_count: 1"));
        assert!(projection.contains("run_recipe_cluster_count: 1"));
        assert!(projection.contains("procedural_contradiction_count: 1"));
        assert!(projection.contains("precedent_cluster_count: 1"));
        assert!(projection.contains("failure_pattern_count: 1"));
        assert!(projection.contains("failure_pattern_cluster_count: 1"));
    }

    #[test]
    fn formats_learning_maintenance_projection() {
        let projection = format_learning_maintenance_projection(
            &crate::application::services::learning_maintenance_service::LearningMaintenancePlan {
                run_importance_decay: true,
                run_gc: true,
                run_run_recipe_review: true,
                run_precedent_compaction: true,
                run_failure_pattern_compaction: false,
                run_skill_review: true,
                run_prompt_optimization: false,
                reasons: vec![
                    crate::application::services::learning_maintenance_service::LearningMaintenanceReason::RecentLearningActivity,
                    crate::application::services::learning_maintenance_service::LearningMaintenanceReason::RunRecipeDuplicateBacklog,
                    crate::application::services::learning_maintenance_service::LearningMaintenanceReason::PrecedentDuplicateBacklog,
                    crate::application::services::learning_maintenance_service::LearningMaintenanceReason::ProceduralContradictionBacklog,
                    crate::application::services::learning_maintenance_service::LearningMaintenanceReason::CandidateSkillBacklog,
                ],
            },
        )
        .unwrap();

        assert!(projection.contains("[learning-maintenance]"));
        assert!(projection.contains("run_run_recipe_review: true"));
        assert!(projection.contains("run_precedent_compaction: true"));
        assert!(projection.contains("run_skill_review: true"));
    }

    #[test]
    fn formats_procedural_contradiction_projection() {
        let projection = format_procedural_contradiction_projection(&[ProceduralContradiction {
            recipe_task_family: "search_delivery".into(),
            recipe_cluster_size: 2,
            recipe_tool_pattern: vec!["web_search".into(), "message_send".into()],
            failure_representative_key: "f1".into(),
            failure_cluster_size: 1,
            failed_tools: vec!["web_search".into(), "message_send".into()],
            overlap: 1.0,
        }])
        .unwrap();

        assert!(projection.contains("[procedural-contradictions]"));
        assert!(projection.contains("search_delivery"));
        assert!(projection.contains("failure=f1"));
    }

    #[test]
    fn formats_skill_review_projection() {
        let projection = format_skill_review_projection(&[SkillReviewDecision {
            skill_id: "sk1".into(),
            skill_name: "search_delivery".into(),
            action:
                crate::application::services::skill_review_service::SkillReviewAction::Deprecate,
            target_status: crate::domain::memory::SkillStatus::Deprecated,
            reason: "unsupported_by_recipe_clusters",
        }])
        .unwrap();

        assert!(projection.contains("[skill-review]"));
        assert!(projection.contains("search_delivery"));
        assert!(projection.contains("unsupported_by_recipe_clusters"));
    }
}
