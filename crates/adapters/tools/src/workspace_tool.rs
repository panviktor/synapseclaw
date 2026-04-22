//! Tool for managing multi-client workspaces.
//!
//! Provides `workspace` subcommands: list, switch, create, info, export.

use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write;
use std::sync::Arc;
use synapse_domain::domain::config::ToolOperation;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    ToolFactPayload, TypedToolFact, WorkspaceAction, WorkspaceFact,
};
use synapse_domain::ports::tool::{ToolArgumentPolicy, ToolContract, ToolRuntimeRole};
use synapse_infra::workspace::WorkspaceManager;
use tokio::sync::RwLock;

/// Agent-callable tool for workspace management operations.
pub struct WorkspaceTool {
    manager: Arc<RwLock<WorkspaceManager>>,
    security: Arc<SecurityPolicy>,
}

impl WorkspaceTool {
    pub fn new(manager: Arc<RwLock<WorkspaceManager>>, security: Arc<SecurityPolicy>) -> Self {
        Self { manager, security }
    }
}

#[async_trait]
impl Tool for WorkspaceTool {
    fn name(&self) -> &str {
        "workspace"
    }

    fn description(&self) -> &str {
        "Manage multi-client workspaces. Subcommands: list, switch, create, info, export. Each workspace provides isolated memory, audit, secrets, and tool restrictions."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "switch", "create", "info", "export"],
                    "description": "Workspace action to perform"
                },
                "name": {
                    "type": "string",
                    "description": "Workspace name (required for switch, create, export)"
                }
            },
            "required": ["action"]
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::RuntimeStateInspection)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::replayable(self.runtime_role()).with_arguments(vec![
            ToolArgumentPolicy::replayable("action").with_values(["list", "info"]),
            ToolArgumentPolicy::replayable("name"),
        ])
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' parameter"))?;

        let name = args.get("name").and_then(|v| v.as_str());

        match action {
            "list" => {
                let mgr = self.manager.read().await;
                let names = mgr.list();
                let active = mgr.active_name();

                if names.is_empty() {
                    return Ok(ToolResult {
                        success: true,
                        output: "No workspaces configured.".to_string(),
                        error: None,
                    });
                }

                let mut output = format!("Workspaces ({}):\n", names.len());
                for ws_name in &names {
                    let marker = if Some(*ws_name) == active {
                        " (active)"
                    } else {
                        ""
                    };
                    let _ = writeln!(output, "  - {ws_name}{marker}");
                }
                Ok(ToolResult {
                    success: true,
                    output,
                    error: None,
                })
            }

            "switch" => {
                if let Err(error) = self
                    .security
                    .enforce_tool_operation(ToolOperation::Act, "workspace")
                {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(error),
                    });
                }

                let ws_name = name.ok_or_else(|| {
                    anyhow::anyhow!("'name' parameter is required for switch action")
                })?;

                let mut mgr = self.manager.write().await;
                match mgr.switch(ws_name) {
                    Ok(profile) => Ok(ToolResult {
                        success: true,
                        output: format!(
                            "Switched to workspace '{}'. Memory namespace: {}, Audit namespace: {}",
                            profile.name,
                            profile.effective_memory_namespace(),
                            profile.effective_audit_namespace()
                        ),
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    }),
                }
            }

            "create" => {
                if let Err(error) = self
                    .security
                    .enforce_tool_operation(ToolOperation::Act, "workspace")
                {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(error),
                    });
                }

                let ws_name = name.ok_or_else(|| {
                    anyhow::anyhow!("'name' parameter is required for create action")
                })?;

                let mut mgr = self.manager.write().await;
                match mgr.create(ws_name).await {
                    Ok(profile) => {
                        let name = profile.name.clone();
                        let dir = mgr.workspace_dir(ws_name);
                        Ok(ToolResult {
                            success: true,
                            output: format!("Created workspace '{}' at {}", name, dir.display()),
                            error: None,
                        })
                    }
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    }),
                }
            }

            "info" => {
                let mgr = self.manager.read().await;
                let target_name = name.or_else(|| mgr.active_name());

                match target_name {
                    Some(ws_name) => match mgr.get(ws_name) {
                        Some(profile) => {
                            let is_active = mgr.active_name() == Some(ws_name);
                            let mut output = format!("Workspace: {}\n", profile.name);
                            let _ = writeln!(
                                output,
                                "  Status: {}",
                                if is_active { "active" } else { "inactive" }
                            );
                            let _ = writeln!(
                                output,
                                "  Memory namespace: {}",
                                profile.effective_memory_namespace()
                            );
                            let _ = writeln!(
                                output,
                                "  Audit namespace: {}",
                                profile.effective_audit_namespace()
                            );
                            if !profile.allowed_domains.is_empty() {
                                let _ = writeln!(
                                    output,
                                    "  Allowed domains: {}",
                                    profile.allowed_domains.join(", ")
                                );
                            }
                            if !profile.tool_restrictions.is_empty() {
                                let _ = writeln!(
                                    output,
                                    "  Restricted tools: {}",
                                    profile.tool_restrictions.join(", ")
                                );
                            }
                            Ok(ToolResult {
                                success: true,
                                output,
                                error: None,
                            })
                        }
                        None => Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!("workspace '{}' not found", ws_name)),
                        }),
                    },
                    None => Ok(ToolResult {
                        success: true,
                        output: "No workspace is currently active. Use 'workspace switch <name>' to activate one.".to_string(),
                        error: None,
                    }),
                }
            }

            "export" => {
                let mgr = self.manager.read().await;
                let ws_name = name.or_else(|| mgr.active_name()).ok_or_else(|| {
                    anyhow::anyhow!("'name' parameter is required when no workspace is active")
                })?;

                match mgr.export(ws_name) {
                    Ok(toml_str) => Ok(ToolResult {
                        success: true,
                        output: format!(
                            "Exported workspace '{}' config (secrets redacted):\n\n{}",
                            ws_name, toml_str
                        ),
                        error: None,
                    }),
                    Err(e) => Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(e.to_string()),
                    }),
                }
            }

            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "unknown workspace action '{}'. Expected: list, switch, create, info, export",
                    other
                )),
            }),
        }
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        if matches!(result, Some(result) if !result.success) {
            return Vec::new();
        }

        let action = match args.get("action").and_then(|value| value.as_str()) {
            Some(action) if !action.trim().is_empty() => action.trim(),
            _ => return Vec::new(),
        };

        let action = match action {
            "list" => WorkspaceAction::List,
            "switch" => WorkspaceAction::Switch,
            "create" => WorkspaceAction::Create,
            "info" => WorkspaceAction::Info,
            "export" => WorkspaceAction::Export,
            _ => return Vec::new(),
        };

        vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Workspace(WorkspaceFact {
                action,
                name: args
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                item_count: None,
            }),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::security_policy::SecurityPolicy;
    use tempfile::TempDir;

    fn test_tool(tmp: &TempDir) -> WorkspaceTool {
        let mgr = WorkspaceManager::new(tmp.path().to_path_buf());
        WorkspaceTool::new(
            Arc::new(RwLock::new(mgr)),
            Arc::new(SecurityPolicy::default()),
        )
    }

    #[tokio::test]
    async fn workspace_tool_list_empty() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);
        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("No workspaces"));
    }

    #[tokio::test]
    async fn workspace_tool_create_and_list() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);

        let result = tool
            .execute(json!({"action": "create", "name": "test_client"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("test_client"));

        let result = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("test_client"));
    }

    #[tokio::test]
    async fn workspace_tool_switch_and_info() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);

        tool.execute(json!({"action": "create", "name": "ws_test"}))
            .await
            .unwrap();

        let result = tool
            .execute(json!({"action": "switch", "name": "ws_test"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("Switched to workspace"));

        let result = tool.execute(json!({"action": "info"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("ws_test"));
        assert!(result.output.contains("active"));
    }

    #[tokio::test]
    async fn workspace_tool_export_redacts() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);

        tool.execute(json!({"action": "create", "name": "export_ws"}))
            .await
            .unwrap();

        let result = tool
            .execute(json!({"action": "export", "name": "export_ws"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("export_ws"));
    }

    #[tokio::test]
    async fn workspace_tool_unknown_action() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);
        let result = tool.execute(json!({"action": "destroy"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("unknown workspace action"));
    }

    #[test]
    fn workspace_schema_marks_read_actions_as_replayable() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);
        let schema = tool.parameters_schema();

        assert!(schema["properties"]["action"].is_object());
        let contract = tool.tool_contract();
        assert!(contract.replayable);
        assert_eq!(
            contract.argument("action").unwrap().replayable_values,
            vec!["list".to_string(), "info".to_string()]
        );
        assert_eq!(
            tool.runtime_role(),
            Some(ToolRuntimeRole::RuntimeStateInspection)
        );
    }

    #[tokio::test]
    async fn workspace_tool_switch_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);
        let result = tool
            .execute(json!({"action": "switch", "name": "ghost"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[test]
    fn extract_facts_emits_workspace_context() {
        let tmp = TempDir::new().unwrap();
        let tool = test_tool(&tmp);
        let facts = tool.extract_facts(
            &json!({"action": "switch", "name": "client_a"}),
            Some(&ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            }),
        );

        assert_eq!(facts.len(), 1);
        assert!(facts[0]
            .projected_focus_entities()
            .iter()
            .any(|entity| entity.kind == "workspace" && entity.name == "client_a"));
        assert!(facts[0]
            .projected_subjects()
            .iter()
            .any(|subject| subject == "client_a"));
    }
}
