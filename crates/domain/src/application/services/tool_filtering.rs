//! Tool filtering — domain policy for which tools to expose per LLM turn.
//!
//! These are business rules that determine tool visibility based on
//! configuration groups, keywords, and capability allowlists.
//! They operate on domain types only (`ToolSpec`, `ToolFilterGroup`).

use crate::config::schema::{ToolFilterGroup, ToolFilterGroupMode};
use crate::ports::tool::{Tool, ToolSpec};
use std::collections::HashSet;
use std::fmt::Write;

// ── Glob matching (single `*` wildcard) ─────────────────────────────

pub fn glob_match(pattern: &str, name: &str) -> bool {
    match pattern.find('*') {
        None => pattern == name,
        Some(star) => {
            let prefix = &pattern[..star];
            let suffix = &pattern[star + 1..];
            name.starts_with(prefix)
                && name.ends_with(suffix)
                && name.len() >= prefix.len() + suffix.len()
        }
    }
}

// ── Filtering functions ─────────────────────────────────────────────

/// Returns the subset of `tool_specs` that should be sent to the LLM for this turn.
///
/// Rules:
/// - Built-in tools (names that do not start with `"mcp_"`) always pass through.
/// - When `groups` is empty, all tools pass through (backward compatible default).
/// - An MCP tool is included if at least one group matches it:
///   - `always` group: included unconditionally if any pattern matches the tool name.
///   - `dynamic` group: included if any pattern matches AND the user message contains
///     at least one keyword (case-insensitive substring).
pub fn filter_tool_specs_for_turn(
    tool_specs: Vec<ToolSpec>,
    groups: &[ToolFilterGroup],
    user_message: &str,
) -> Vec<ToolSpec> {
    if groups.is_empty() {
        return tool_specs;
    }

    let msg_lower = user_message.to_ascii_lowercase();

    tool_specs
        .into_iter()
        .filter(|spec| {
            // Built-in tools always pass through.
            if !spec.name.starts_with("mcp_") {
                return true;
            }
            // MCP tool: include if any active group matches.
            groups.iter().any(|group| {
                let pattern_matches = group.tools.iter().any(|pat| glob_match(pat, &spec.name));
                if !pattern_matches {
                    return false;
                }
                match group.mode {
                    ToolFilterGroupMode::Always => true,
                    ToolFilterGroupMode::Dynamic => group
                        .keywords
                        .iter()
                        .any(|kw| msg_lower.contains(&kw.to_ascii_lowercase())),
                }
            })
        })
        .collect()
}

/// Filters a tool spec list by an optional capability allowlist.
///
/// When `allowed` is `None`, all specs pass through unchanged.
/// When `allowed` is `Some(list)`, only specs whose name appears in the list
/// are retained. Unknown names in the allowlist are silently ignored.
pub fn filter_by_allowed_tools(specs: Vec<ToolSpec>, allowed: Option<&[String]>) -> Vec<ToolSpec> {
    match allowed {
        None => specs,
        Some(list) => specs
            .into_iter()
            .filter(|spec| list.iter().any(|name| name == &spec.name))
            .collect(),
    }
}

/// Computes the list of MCP tool names that should be excluded for a given turn
/// based on `tool_filter_groups` and the user message.
///
/// Returns an empty `Vec` when `groups` is empty (no filtering).
pub fn compute_excluded_mcp_tools(
    tools_registry: &[Box<dyn Tool>],
    groups: &[ToolFilterGroup],
    user_message: &str,
) -> Vec<String> {
    if groups.is_empty() {
        return Vec::new();
    }
    let filtered_specs = filter_tool_specs_for_turn(
        tools_registry.iter().map(|t| t.spec()).collect(),
        groups,
        user_message,
    );
    let included: HashSet<&str> = filtered_specs.iter().map(|s| s.name.as_str()).collect();
    tools_registry
        .iter()
        .filter(|t| t.name().starts_with("mcp_") && !included.contains(t.name()))
        .map(|t| t.name().to_string())
        .collect()
}

// ── Tool instruction builder ────────────────────────────────────────

/// Build the tool instruction block for the system prompt so the LLM knows
/// how to invoke tools. Takes a registry of `Box<dyn Tool>` and formats
/// each tool's metadata into a prompt-friendly text block.
pub fn build_tool_instructions(tools_registry: &[Box<dyn Tool>]) -> String {
    let mut instructions = String::new();
    instructions.push_str("\n## Tool Use Protocol\n\n");
    instructions.push_str("To use a tool, wrap a JSON object in <tool_call></tool_call> tags:\n\n");
    instructions.push_str("```\n<tool_call>\n{\"name\": \"tool_name\", \"arguments\": {\"param\": \"value\"}}\n</tool_call>\n```\n\n");
    instructions.push_str(
        "CRITICAL: Output actual <tool_call> tags\u{2014}never describe steps or give examples.\n\n",
    );
    instructions.push_str("Example: User says \"what's the date?\". You MUST respond with:\n<tool_call>\n{\"name\":\"shell\",\"arguments\":{\"command\":\"date\"}}\n</tool_call>\n\n");
    instructions.push_str("You may use multiple tool calls in a single response. ");
    instructions.push_str("After tool execution, results appear in <tool_result> tags. ");
    instructions
        .push_str("Continue reasoning with the results until you can give a final answer.\n\n");
    instructions.push_str("### Available Tools\n\n");

    for tool in tools_registry {
        let _ = writeln!(
            instructions,
            "**{}**: {}\nParameters: `{}`\n",
            tool.name(),
            tool.description(),
            tool.parameters_schema()
        );
    }

    instructions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{ToolFilterGroup, ToolFilterGroupMode};

    fn make_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: format!("{name} description"),
            parameters: serde_json::json!({"type": "object"}),
            runtime_role: None,
        }
    }

    #[test]
    fn empty_groups_passes_all() {
        let specs = vec![make_spec("shell"), make_spec("mcp_github")];
        let result = filter_tool_specs_for_turn(specs, &[], "hello");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn builtin_tools_always_pass() {
        let specs = vec![make_spec("shell"), make_spec("file_read")];
        let groups = vec![ToolFilterGroup {
            mode: ToolFilterGroupMode::Dynamic,
            tools: vec!["mcp_*".to_string()],
            keywords: vec!["deploy".to_string()],
        }];
        let result = filter_tool_specs_for_turn(specs, &groups, "hello");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn dynamic_group_filters_by_keyword() {
        let specs = vec![make_spec("shell"), make_spec("mcp_github")];
        let groups = vec![ToolFilterGroup {
            mode: ToolFilterGroupMode::Dynamic,
            tools: vec!["mcp_*".to_string()],
            keywords: vec!["deploy".to_string()],
        }];

        // Without keyword
        let result = filter_tool_specs_for_turn(specs.clone(), &groups, "hello");
        assert_eq!(result.len(), 1); // only shell

        // With keyword
        let result = filter_tool_specs_for_turn(specs, &groups, "please deploy");
        assert_eq!(result.len(), 2); // shell + mcp_github
    }

    #[test]
    fn always_group_includes_unconditionally() {
        let specs = vec![make_spec("mcp_slack")];
        let groups = vec![ToolFilterGroup {
            mode: ToolFilterGroupMode::Always,
            tools: vec!["mcp_slack".to_string()],
            keywords: vec![],
        }];
        let result = filter_tool_specs_for_turn(specs, &groups, "anything");
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn filter_by_allowlist() {
        let specs = vec![
            make_spec("shell"),
            make_spec("file_read"),
            make_spec("browser"),
        ];

        // No allowlist — all pass
        let result = filter_by_allowed_tools(specs.clone(), None);
        assert_eq!(result.len(), 3);

        // With allowlist
        let allowed = vec!["shell".to_string(), "browser".to_string()];
        let result = filter_by_allowed_tools(specs, Some(&allowed));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "shell");
        assert_eq!(result[1].name, "browser");
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("shell", "shell"));
        assert!(!glob_match("shell", "shell2"));
    }

    #[test]
    fn glob_match_wildcard() {
        assert!(glob_match("mcp_*", "mcp_github"));
        assert!(glob_match("mcp_*", "mcp_"));
        assert!(!glob_match("mcp_*", "shell"));
        assert!(glob_match("*_tool", "my_tool"));
        assert!(glob_match("mcp_git*", "mcp_github"));
    }
}
