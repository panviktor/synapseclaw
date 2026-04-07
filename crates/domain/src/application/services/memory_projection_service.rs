//! Human-readable memory projections.
//!
//! These are cheap, regenerable views over structured state. They are meant
//! for operators and UI inspection, not as the canonical source of truth.

use crate::domain::conversation::{ConversationEvent, ConversationSession, EventType};
use crate::domain::dialogue_state::DialogueState;
use crate::domain::memory::{CoreMemoryBlock, MemoryEntry};
use crate::domain::run_recipe::RunRecipe;

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
}
