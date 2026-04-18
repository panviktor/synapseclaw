//! Repository discovery tool.
//!
//! Finds local Git repositories with bounded, typed traversal so higher-level
//! Git tools do not need shell `find` or overloaded operation modes.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::tool::{ToolArgumentPolicy, ToolContract, ToolRuntimeRole};

pub struct RepoDiscoveryTool {
    security: Arc<SecurityPolicy>,
    workspace_dir: PathBuf,
}

impl RepoDiscoveryTool {
    pub fn new(security: Arc<SecurityPolicy>, workspace_dir: PathBuf) -> Self {
        Self {
            security,
            workspace_dir,
        }
    }

    fn resolve_scan_root(&self, args: &serde_json::Value) -> anyhow::Result<PathBuf> {
        let raw_root_path = args
            .get("root_path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(".");
        if !self.security.is_path_allowed(raw_root_path) {
            anyhow::bail!("root_path is outside the allowed workspace roots");
        }
        let path = Path::new(raw_root_path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_dir.join(path)
        }
        .canonicalize()?;
        if !self.security.is_resolved_path_allowed(&resolved) {
            anyhow::bail!("root_path resolves outside the allowed workspace roots");
        }
        Ok(resolved)
    }

    fn bounded_usize_arg(
        args: &serde_json::Value,
        key: &str,
        default: usize,
        min: usize,
        max: usize,
    ) -> usize {
        args.get(key)
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(default)
            .clamp(min, max)
    }

    fn workspace_relative_path(&self, path: &Path) -> String {
        let workspace = self
            .workspace_dir
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_dir.clone());
        path.strip_prefix(&workspace)
            .ok()
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    relative.display().to_string()
                }
            })
            .unwrap_or_else(|| path.display().to_string())
    }

    fn repository_kind(path: &Path, include_bare: bool) -> Option<&'static str> {
        let git_meta = path.join(".git");
        if git_meta.is_dir() || git_meta.is_file() {
            return Some("worktree");
        }
        if include_bare
            && path.join("HEAD").is_file()
            && path.join("objects").is_dir()
            && path.join("refs").is_dir()
        {
            return Some("bare");
        }
        None
    }

    fn discover(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let root = self.resolve_scan_root(&args)?;
        let max_depth = Self::bounded_usize_arg(&args, "max_depth", 4, 0, 8);
        let limit = Self::bounded_usize_arg(&args, "limit", 20, 1, 100);
        let include_bare = args
            .get("include_bare")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let name_contains = args
            .get("name_contains")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);

        let mut queue = VecDeque::from([(root.clone(), 0usize)]);
        let mut repositories = Vec::new();
        let mut scanned_dirs = 0usize;
        let mut truncated = false;

        while let Some((dir, depth)) = queue.pop_front() {
            scanned_dirs += 1;
            if let Some(kind) = Self::repository_kind(&dir, include_bare) {
                let name = dir
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_else(|| dir.display().to_string());
                let filter_matches = name_contains
                    .as_ref()
                    .map_or(true, |filter| name.to_lowercase().contains(filter));
                if filter_matches {
                    repositories.push(json!({
                        "path": dir.display().to_string(),
                        "relative_path": self.workspace_relative_path(&dir),
                        "name": name,
                        "kind": kind,
                    }));
                    if repositories.len() >= limit {
                        truncated = true;
                        break;
                    }
                }
            }

            if depth >= max_depth {
                continue;
            }

            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => continue,
                };
                if !file_type.is_dir() || file_type.is_symlink() {
                    continue;
                }
                if entry.file_name() == ".git" {
                    continue;
                }
                let child = match entry.path().canonicalize() {
                    Ok(child) => child,
                    Err(_) => continue,
                };
                if !self.security.is_resolved_path_allowed(&child) {
                    continue;
                }
                queue.push_back((child, depth + 1));
            }
        }

        Ok(ToolResult {
            success: true,
            output: serde_json::to_string_pretty(&json!({
                "root_path": root.display().to_string(),
                "max_depth": max_depth,
                "limit": limit,
                "name_contains": name_contains,
                "include_bare": include_bare,
                "repositories": repositories,
                "truncated": truncated,
                "scanned_dirs": scanned_dirs,
            }))
            .unwrap_or_default(),
            error: None,
        })
    }
}

#[async_trait]
impl Tool for RepoDiscoveryTool {
    fn name(&self) -> &str {
        "repo_discovery"
    }

    fn description(&self) -> &str {
        "Find local Git repositories under an allowed workspace root. Returns bounded structured candidates for follow-up git_operations calls."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "root_path": {
                    "type": "string",
                    "description": "Optional scan root, relative to the configured workspace or inside an allowed root"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth to scan (default: 4, max: 8)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum repositories to return (default: 20, max: 100)"
                },
                "name_contains": {
                    "type": "string",
                    "description": "Optional repository directory name substring filter"
                },
                "include_bare": {
                    "type": "boolean",
                    "description": "Include bare repositories in results"
                }
            }
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::WorkspaceDiscovery)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::replayable(self.runtime_role()).with_arguments(vec![
            ToolArgumentPolicy::replayable("root_path"),
            ToolArgumentPolicy::replayable("max_depth"),
            ToolArgumentPolicy::replayable("limit"),
            ToolArgumentPolicy::replayable("name_contains"),
            ToolArgumentPolicy::replayable("include_bare"),
        ])
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: rate limit exceeded".into()),
            });
        }
        match self.discover(args) {
            Ok(result) => Ok(result),
            Err(error) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::config::AutonomyLevel;
    use tempfile::TempDir;

    fn test_tool(dir: &std::path::Path) -> RepoDiscoveryTool {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: dir.to_path_buf(),
            ..SecurityPolicy::default()
        });
        RepoDiscoveryTool::new(security, dir.to_path_buf())
    }

    #[test]
    fn schema_is_replayable_workspace_discovery() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(tmp.path());
        let schema = tool.parameters_schema();

        assert!(schema["properties"]["root_path"].is_object());
        assert!(schema["properties"]["max_depth"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["properties"]["name_contains"].is_object());
        assert_eq!(
            tool.runtime_role(),
            Some(ToolRuntimeRole::WorkspaceDiscovery)
        );
        let contract = tool.tool_contract();
        assert!(contract.replayable);
        assert!(contract.argument("root_path").unwrap().replayable);
        assert!(contract.argument("include_bare").unwrap().replayable);
    }

    #[tokio::test]
    async fn finds_repositories_under_workspace() {
        let tmp = TempDir::new().unwrap();
        let matrix_dir = tmp.path().join("matrix");
        let other_dir = tmp.path().join("other");
        let nested_dir = tmp.path().join("nested").join("plain");
        std::fs::create_dir_all(&matrix_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&matrix_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&other_dir)
            .output()
            .unwrap();
        let tool = test_tool(tmp.path());

        let result = tool
            .execute(json!({
                "root_path": ".",
                "name_contains": "mat",
                "max_depth": 3,
                "limit": 10
            }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        let parsed: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let repositories = parsed["repositories"].as_array().unwrap();
        assert_eq!(repositories.len(), 1);
        assert_eq!(repositories[0]["name"], "matrix");
        assert_eq!(repositories[0]["relative_path"], "matrix");
        assert_eq!(repositories[0]["kind"], "worktree");
    }

    #[tokio::test]
    async fn blocks_root_outside_workspace() {
        let workspace = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let tool = test_tool(workspace.path());

        let result = tool
            .execute(json!({
                "root_path": outside.path().display().to_string()
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("outside the allowed workspace roots"));
    }
}
