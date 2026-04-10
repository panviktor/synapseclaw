//! Progressive scoped instruction resolution.
//!
//! This service decides *when* scoped project instructions are relevant and
//! surfaces path hints for adapter-side discovery. It intentionally stays
//! structural: paths and recent typed resource/search context, not phrase rules.

use crate::application::services::turn_interpretation::TurnInterpretation;
use crate::application::services::turn_model_routing::infer_turn_capability_requirement;
use crate::domain::tool_fact::{ResourceKind, SearchDomain};
use crate::ports::scoped_instruction_context::ScopedInstructionSnippet;
use std::collections::BTreeSet;

const DEFAULT_SCOPED_MAX_FILES: usize = 2;
const DEFAULT_SCOPED_MAX_TOTAL_CHARS: usize = 1_800;
const INFERRED_SCOPED_MAX_FILES: usize = 1;
const INFERRED_SCOPED_MAX_TOTAL_CHARS: usize = 900;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedInstructionHintSource {
    UserMessagePath,
    RecentResource,
    RecentSearch,
    RecentWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedInstructionHint {
    pub path: String,
    pub source: ScopedInstructionHintSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedInstructionPlan {
    pub hints: Vec<ScopedInstructionHint>,
    pub max_files: usize,
    pub max_total_chars: usize,
}

pub fn build_scoped_instruction_plan(
    user_message: &str,
    interpretation: Option<&TurnInterpretation>,
) -> Option<ScopedInstructionPlan> {
    let mut hints = Vec::new();
    hints.extend(
        extract_user_message_path_hints(user_message)
            .into_iter()
            .map(|path| ScopedInstructionHint {
                path,
                source: ScopedInstructionHintSource::UserMessagePath,
            }),
    );
    let has_explicit_user_path = !hints.is_empty();

    if infer_turn_capability_requirement(user_message).is_some() && !has_explicit_user_path {
        return None;
    }

    if let Some(interpretation) = interpretation {
        if let Some(state) = interpretation.dialogue_state.as_ref() {
            if let Some(resource) = state.recent_resource.as_ref() {
                if matches!(
                    resource.kind,
                    ResourceKind::File
                        | ResourceKind::Directory
                        | ResourceKind::ConfigFile
                        | ResourceKind::GitRepository
                ) && looks_like_scope_path(&resource.locator)
                {
                    hints.push(ScopedInstructionHint {
                        path: resource.locator.clone(),
                        source: ScopedInstructionHintSource::RecentResource,
                    });
                }
            }

            if let Some(search) = state.recent_search.as_ref() {
                if matches!(search.domain, SearchDomain::Workspace) {
                    if let Some(locator) = search.primary_locator.as_ref() {
                        if looks_like_scope_path(locator) {
                            hints.push(ScopedInstructionHint {
                                path: locator.clone(),
                                source: ScopedInstructionHintSource::RecentSearch,
                            });
                        }
                    }
                }
            }

            if let Some(workspace) = state.recent_workspace.as_ref() {
                if let Some(name) = workspace.name.as_ref() {
                    if looks_like_scope_path(name) {
                        hints.push(ScopedInstructionHint {
                            path: name.clone(),
                            source: ScopedInstructionHintSource::RecentWorkspace,
                        });
                    }
                }
            }
        }
    }

    let mut seen = BTreeSet::new();
    hints.retain(|hint| seen.insert(hint.path.clone()));
    hints.truncate(4);

    if hints.is_empty() {
        None
    } else {
        let (max_files, max_total_chars) = if has_explicit_user_path {
            (DEFAULT_SCOPED_MAX_FILES, DEFAULT_SCOPED_MAX_TOTAL_CHARS)
        } else {
            (INFERRED_SCOPED_MAX_FILES, INFERRED_SCOPED_MAX_TOTAL_CHARS)
        };
        Some(ScopedInstructionPlan {
            hints,
            max_files,
            max_total_chars,
        })
    }
}

pub fn format_scoped_instruction_block(snippets: &[ScopedInstructionSnippet]) -> Option<String> {
    if snippets.is_empty() {
        return None;
    }

    let mut lines = vec!["[scoped-context]".to_string()];
    for snippet in snippets {
        lines.push(format!(
            "- scope: {} | source: {}{}",
            snippet.scope_root,
            snippet.source_file,
            if snippet.cache_hit {
                " | cache: hit"
            } else {
                ""
            }
        ));
        lines.push(format!("### {}", snippet.source_file));
        lines.push(snippet.content.trim().to_string());
        lines.push(String::new());
    }
    Some(lines.join("\n").trim_end().to_string() + "\n")
}

fn extract_user_message_path_hints(user_message: &str) -> Vec<String> {
    let mut hints = Vec::new();
    let mut seen = BTreeSet::new();

    for token in user_message.split_whitespace() {
        let cleaned = trim_path_token(token);
        if is_media_control_path_token(cleaned) {
            continue;
        }
        if looks_like_scope_path(cleaned) && seen.insert(cleaned.to_string()) {
            hints.push(cleaned.to_string());
        }
    }

    hints
}

fn trim_path_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':' | '!' | '?'
        )
    })
}

fn looks_like_scope_path(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.starts_with("matrix:")
    {
        return false;
    }

    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.contains('/')
}

fn is_media_control_path_token(value: &str) -> bool {
    value.starts_with("IMAGE:")
        || value.starts_with("data:image/")
        || value.starts_with("[IMAGE:")
        || value.starts_with("[GENERATE:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::DialogueStateSnapshot;
    use crate::domain::dialogue_state::{ResourceReference, SearchReference, WorkspaceReference};
    use crate::domain::tool_fact::{
        ResourceKind, ResourceOperation, SearchDomain, WorkspaceAction,
    };

    #[test]
    fn builds_plan_from_explicit_user_message_paths() {
        let plan =
            build_scoped_instruction_plan("Look at src/core/agent.rs and ./Cargo.toml", None)
                .expect("plan");

        assert_eq!(plan.max_files, DEFAULT_SCOPED_MAX_FILES);
        assert_eq!(plan.max_total_chars, DEFAULT_SCOPED_MAX_TOTAL_CHARS);
        let paths = plan
            .hints
            .into_iter()
            .map(|hint| hint.path)
            .collect::<Vec<_>>();
        assert!(paths.contains(&"src/core/agent.rs".to_string()));
        assert!(paths.contains(&"./Cargo.toml".to_string()));
    }

    #[test]
    fn builds_plan_from_recent_typed_workspace_context() {
        let interpretation = TurnInterpretation {
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: Vec::new(),
                comparison_set: Vec::new(),
                reference_anchors: Vec::new(),
                last_tool_subjects: Vec::new(),
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: Some(ResourceReference {
                    kind: ResourceKind::File,
                    operation: ResourceOperation::Read,
                    locator: "crates/domain/src/lib.rs".into(),
                    host: None,
                }),
                recent_search: Some(SearchReference {
                    domain: SearchDomain::Workspace,
                    query: Some("turn context".into()),
                    primary_locator: Some(
                        "crates/domain/src/application/services/turn_context.rs".into(),
                    ),
                    result_count: Some(1),
                }),
                recent_workspace: Some(WorkspaceReference {
                    action: WorkspaceAction::Info,
                    name: Some("docs/fork".into()),
                    item_count: Some(2),
                }),
            }),
            ..Default::default()
        };

        let plan = build_scoped_instruction_plan("continue", Some(&interpretation)).expect("plan");
        assert_eq!(plan.max_files, INFERRED_SCOPED_MAX_FILES);
        assert_eq!(plan.max_total_chars, INFERRED_SCOPED_MAX_TOTAL_CHARS);
        let paths = plan
            .hints
            .into_iter()
            .map(|hint| hint.path)
            .collect::<Vec<_>>();
        assert!(paths.contains(&"crates/domain/src/lib.rs".to_string()));
        assert!(
            paths.contains(&"crates/domain/src/application/services/turn_context.rs".to_string())
        );
        assert!(paths.contains(&"docs/fork".to_string()));
    }

    #[test]
    fn media_turn_suppresses_recent_inferred_scope_without_explicit_path() {
        let interpretation = TurnInterpretation {
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: Vec::new(),
                comparison_set: Vec::new(),
                reference_anchors: Vec::new(),
                last_tool_subjects: Vec::new(),
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: Some(ResourceReference {
                    kind: ResourceKind::File,
                    operation: ResourceOperation::Read,
                    locator: "docs/fork/ipc-phase4_10-plan.md".into(),
                    host: None,
                }),
                recent_search: None,
                recent_workspace: None,
            }),
            ..Default::default()
        };

        assert!(build_scoped_instruction_plan(
            "Describe the image [IMAGE:/tmp/smoke.png]",
            Some(&interpretation),
        )
        .is_none());
    }

    #[test]
    fn image_control_marker_is_not_treated_as_scope_path() {
        assert!(extract_user_message_path_hints("Describe [IMAGE:/tmp/smoke.png]").is_empty());
        assert!(
            extract_user_message_path_hints("Describe [IMAGE:data:image/png;base64,abc]")
                .is_empty()
        );
    }

    #[test]
    fn formats_scoped_context_block() {
        let block = format_scoped_instruction_block(&[ScopedInstructionSnippet {
            scope_root: "crates/domain".into(),
            source_file: "crates/domain/AGENTS.md".into(),
            content: "Prefer small patches.".into(),
            cache_hit: true,
        }])
        .expect("block");

        assert!(block.starts_with("[scoped-context]"));
        assert!(block.contains("cache: hit"));
        assert!(block.contains("Prefer small patches."));
    }
}
