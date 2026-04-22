use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    ResourceFact, ResourceKind, ResourceMetadata, ResourceOperation, ToolFactPayload, TypedToolFact,
};
use synapse_domain::ports::runtime::RuntimeAdapter;
use synapse_domain::ports::tool::{
    ToolArgumentPolicy, ToolContract, ToolNonReplayableReason, ToolRuntimeRole,
};

/// Maximum shell command execution time before kill.
const SHELL_TIMEOUT_SECS: u64 = 60;
/// Maximum output size in bytes (1MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Environment variables safe to pass to shell commands.
/// Only functional variables are included — never API keys or secrets.
#[cfg(not(target_os = "windows"))]
const SAFE_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "USER",
    "SHELL",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// Environment variables safe to pass to shell commands on Windows.
/// Includes Windows-specific variables needed for cmd.exe and program resolution.
#[cfg(target_os = "windows")]
const SAFE_ENV_VARS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "TERM",
    "LANG",
    "USERNAME",
];

/// Shell command execution tool with sandboxing
pub struct ShellTool {
    security: Arc<SecurityPolicy>,
    runtime: Arc<dyn RuntimeAdapter>,
}

impl ShellTool {
    pub fn new(security: Arc<SecurityPolicy>, runtime: Arc<dyn RuntimeAdapter>) -> Self {
        Self { security, runtime }
    }
}

fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn collect_allowed_shell_env_vars(security: &SecurityPolicy) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for key in SAFE_ENV_VARS
        .iter()
        .copied()
        .chain(security.shell_env_passthrough.iter().map(|s| s.as_str()))
    {
        let candidate = key.trim();
        if candidate.is_empty() || !is_valid_env_var_name(candidate) {
            continue;
        }
        if seen.insert(candidate.to_string()) {
            out.push(candidate.to_string());
        }
    }
    out
}

fn configure_runtime_bus_env(cmd: &mut tokio::process::Command) {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let session_bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .ok()
        .filter(|value| !value.trim().is_empty());

    if let Some(runtime_dir) = runtime_dir.as_deref() {
        cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    }

    if let Some(session_bus) = session_bus.as_deref() {
        cmd.env("DBUS_SESSION_BUS_ADDRESS", session_bus);
        return;
    }

    if let Some(runtime_dir) = runtime_dir.as_deref() {
        let bus_path = std::path::Path::new(runtime_dir).join("bus");
        if bus_path.exists() {
            cmd.env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", bus_path.display()),
            );
        }
    }
}

fn extract_shell_focus_entities(
    workspace_dir: &std::path::Path,
    command: &str,
) -> Vec<FocusEntity> {
    let mut entities = vec![
        FocusEntity {
            kind: "workspace_directory".into(),
            name: workspace_dir.display().to_string(),
            metadata: Some("shell_cwd".into()),
        },
        FocusEntity {
            kind: "shell_command".into(),
            name: command.to_string(),
            metadata: Some(workspace_dir.display().to_string()),
        },
    ];

    if let Some(service_name) = extract_system_service(command) {
        entities.push(FocusEntity {
            kind: "system_service".into(),
            name: service_name,
            metadata: Some(service_command_label(command).into()),
        });
    }

    if looks_like_package_updates(command) {
        entities.push(FocusEntity {
            kind: "ops_workflow".into(),
            name: "package_updates".into(),
            metadata: Some("package_manager_check".into()),
        });
    }

    if looks_like_system_memory_check(command) {
        entities.push(FocusEntity {
            kind: "system_metric".into(),
            name: "memory".into(),
            metadata: Some("capacity_check".into()),
        });
    }

    if looks_like_disk_check(command) {
        entities.push(FocusEntity {
            kind: "system_metric".into(),
            name: "disk".into(),
            metadata: Some("capacity_check".into()),
        });
    }

    if looks_like_uptime_check(command) {
        entities.push(FocusEntity {
            kind: "system_metric".into(),
            name: "load_average".into(),
            metadata: Some("uptime".into()),
        });
    }

    if let Some(url) = extract_http_url(command) {
        entities.push(FocusEntity {
            kind: "network_endpoint".into(),
            name: url,
            metadata: Some(shell_network_label(command).into()),
        });
    }

    entities
}

fn build_shell_resource_facts(tool_name: &str, command: &str) -> Vec<TypedToolFact> {
    let mut facts = Vec::new();
    if let Some(url) = extract_http_url(command) {
        facts.push(TypedToolFact {
            tool_id: tool_name.to_string(),
            payload: ToolFactPayload::Resource(ResourceFact {
                kind: ResourceKind::NetworkEndpoint,
                operation: if url.contains("/health") {
                    ResourceOperation::Verify
                } else {
                    ResourceOperation::Fetch
                },
                host: extract_url_host(&url),
                locator: url,
                metadata: ResourceMetadata::default(),
            }),
        });
    }
    facts
}

fn extract_system_service(command: &str) -> Option<String> {
    static SERVICE_RE: OnceLock<regex::Regex> = OnceLock::new();
    let service_re = SERVICE_RE.get_or_init(|| {
        regex::Regex::new(r"(?P<service>[A-Za-z0-9_.@-]+\.service)\b")
            .expect("service regex must be valid")
    });
    service_re
        .captures(command)
        .and_then(|caps| caps.name("service").map(|m| m.as_str().to_string()))
}

fn service_command_label(command: &str) -> &'static str {
    if command.contains("journalctl") {
        "journalctl"
    } else if command.contains("systemctl") {
        "systemctl"
    } else {
        "shell"
    }
}

fn looks_like_package_updates(command: &str) -> bool {
    (command.contains("apt ") || command.contains("apt-get ") || command.contains("dnf "))
        && (command.contains("list --upgradable")
            || command.contains(" update")
            || command.contains(" upgrade"))
}

fn looks_like_system_memory_check(command: &str) -> bool {
    command.contains("free ") || command.trim() == "free" || command.contains("/proc/meminfo")
}

fn looks_like_disk_check(command: &str) -> bool {
    command.contains("df ") || command.trim() == "df"
}

fn looks_like_uptime_check(command: &str) -> bool {
    command.trim() == "uptime" || command.contains(" uptime")
}

fn extract_http_url(command: &str) -> Option<String> {
    command.split_whitespace().find_map(|token| {
        let candidate = token.trim_matches(|ch: char| "\"'()[]{};, ".contains(ch));
        (candidate.starts_with("http://") || candidate.starts_with("https://"))
            .then(|| candidate.to_string())
    })
}

fn extract_url_host(url: &str) -> Option<String> {
    let without_scheme = url.split_once("://")?.1;
    let host_port = without_scheme.split('/').next()?;
    Some(host_port.to_string())
}

fn shell_network_label(command: &str) -> &'static str {
    if command.contains("/health") {
        "health_check"
    } else if command.contains("curl ") {
        "curl"
    } else {
        "network_probe"
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the workspace directory. Keep commands simple and shell-policy friendly: avoid redirection (`>`, `<`, `2>/dev/null`), subshells, `set -o pipefail`, background jobs, and `tee`. Use short pipelines of allowed commands or split discovery into separate tool calls."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute. Do not use redirection (`>`, `<`, `2>/dev/null`), subshells, `set -o pipefail`, background jobs, or `tee`; retry blocked discovery commands by removing redirection and splitting pipelines."
                },
                "approved": {
                    "type": "boolean",
                    "description": "Set true to explicitly approve medium/high-risk commands in supervised mode",
                    "default": false
                }
            },
            "required": ["command"]
        })
    }

    fn runtime_role(&self) -> Option<synapse_domain::ports::tool::ToolRuntimeRole> {
        Some(ToolRuntimeRole::WorkspaceDiscovery)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(
            self.runtime_role(),
            ToolNonReplayableReason::FreeFormCommand,
        )
        .with_arguments(vec![
            ToolArgumentPolicy::sensitive("command"),
            ToolArgumentPolicy::blocked("approved"),
        ])
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;
        let approved = args
            .get("approved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        match self.security.validate_command_execution(command, approved) {
            Ok(_) => {}
            Err(reason) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(reason),
                });
            }
        }

        if let Some(path) = self.security.forbidden_path_argument(command) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path blocked by security policy: {path}")),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        // Execute with timeout to prevent hanging commands.
        // Clear the environment to prevent leaking API keys and other secrets
        // (CWE-200), then re-add only safe, functional variables.
        let mut cmd = match self
            .runtime
            .build_shell_command(command, &self.security.workspace_dir)
        {
            Ok(cmd) => cmd,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to build runtime command: {e}")),
                });
            }
        };
        cmd.env_clear();

        for var in collect_allowed_shell_env_vars(&self.security) {
            if let Ok(val) = std::env::var(&var) {
                cmd.env(&var, val);
            }
        }
        configure_runtime_bus_env(&mut cmd);

        let result =
            tokio::time::timeout(Duration::from_secs(SHELL_TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

                // Truncate output to prevent OOM
                if stdout.len() > MAX_OUTPUT_BYTES {
                    let mut b = MAX_OUTPUT_BYTES.min(stdout.len());
                    while b > 0 && !stdout.is_char_boundary(b) {
                        b -= 1;
                    }
                    stdout.truncate(b);
                    stdout.push_str("\n... [output truncated at 1MB]");
                }
                if stderr.len() > MAX_OUTPUT_BYTES {
                    let mut b = MAX_OUTPUT_BYTES.min(stderr.len());
                    while b > 0 && !stderr.is_char_boundary(b) {
                        b -= 1;
                    }
                    stderr.truncate(b);
                    stderr.push_str("\n... [stderr truncated at 1MB]");
                }

                Ok(ToolResult {
                    success: output.status.success(),
                    output: stdout,
                    error: if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute command: {e}")),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Command timed out after {SHELL_TIMEOUT_SECS}s and was killed"
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

        let command = match args.get("command").and_then(|value| value.as_str()) {
            Some(command) if !command.trim().is_empty() => command.trim(),
            _ => return Vec::new(),
        };

        let mut facts = vec![TypedToolFact::focus(
            self.name().to_string(),
            extract_shell_focus_entities(&self.security.workspace_dir, command),
            Vec::new(),
        )];
        facts.extend(build_shell_resource_facts(self.name(), command));
        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::config::AutonomyLevel;
    use synapse_domain::domain::security_policy::SecurityPolicy;
    use synapse_domain::ports::runtime::RuntimeAdapter;
    use synapse_infra::native::NativeRuntime;

    fn test_security(autonomy: AutonomyLevel) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    fn test_runtime() -> Arc<dyn RuntimeAdapter> {
        Arc::new(NativeRuntime::new())
    }

    #[test]
    fn shell_tool_name() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        assert_eq!(tool.name(), "shell");
    }

    #[test]
    fn shell_tool_description() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn shell_tool_schema_has_command() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["command"].is_object());
        assert!(schema["required"]
            .as_array()
            .expect("schema required field should be an array")
            .contains(&json!("command")));
        assert!(schema["properties"]["approved"].is_object());
    }

    #[tokio::test]
    async fn shell_executes_allowed_command() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "echo hello"}))
            .await
            .expect("echo command execution should succeed");
        assert!(result.success);
        assert!(result.output.trim().contains("hello"));
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn shell_blocks_disallowed_command() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "rm -rf /"}))
            .await
            .expect("disallowed command execution should return a result");
        assert!(!result.success);
        let error = result.error.as_deref().unwrap_or("");
        assert!(error.contains("not allowed") || error.contains("high-risk"));
    }

    #[tokio::test]
    async fn shell_blocks_readonly() {
        let tool = ShellTool::new(test_security(AutonomyLevel::ReadOnly), test_runtime());
        let result = tool
            .execute(json!({"command": "ls"}))
            .await
            .expect("readonly command execution should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .expect("error field should be present for blocked command")
            .contains("not allowed"));
    }

    #[tokio::test]
    async fn shell_missing_command_param() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("command"));
    }

    #[tokio::test]
    async fn shell_wrong_type_param() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool.execute(json!({"command": 123})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn shell_captures_exit_code() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "ls /nonexistent_dir_xyz"}))
            .await
            .expect("command with nonexistent path should return a result");
        assert!(!result.success);
    }

    #[tokio::test]
    async fn shell_blocks_absolute_path_argument() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "cat /etc/passwd"}))
            .await
            .expect("absolute path argument should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_option_assignment_path_argument() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "grep --file=/etc/passwd root ./src"}))
            .await
            .expect("option-assigned forbidden path should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_short_option_attached_path_argument() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "grep -f/etc/passwd root ./src"}))
            .await
            .expect("short option attached forbidden path should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_tilde_user_path_argument() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "cat ~root/.ssh/id_rsa"}))
            .await
            .expect("tilde-user path should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Path blocked"));
    }

    #[tokio::test]
    async fn shell_blocks_input_redirection_path_bypass() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let result = tool
            .execute(json!({"command": "cat </etc/passwd"}))
            .await
            .expect("input redirection bypass should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not allowed"));
    }

    fn test_security_with_env_cmd() -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: std::env::temp_dir(),
            allowed_commands: vec!["env".into(), "echo".into()],
            ..SecurityPolicy::default()
        })
    }

    fn test_security_with_env_passthrough(vars: &[&str]) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: std::env::temp_dir(),
            allowed_commands: vec!["env".into()],
            shell_env_passthrough: vars.iter().map(|v| (*v).to_string()).collect(),
            ..SecurityPolicy::default()
        })
    }

    /// RAII guard that restores an environment variable to its original state on drop,
    /// ensuring cleanup even if the test panics.
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(val) => std::env::set_var(self.key, val),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shell_does_not_leak_api_key() {
        let _g1 = EnvGuard::set("API_KEY", "sk-test-secret-12345");
        let _g2 = EnvGuard::set("SYNAPSECLAW_API_KEY", "sk-test-secret-67890");

        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime());
        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command execution should succeed");
        assert!(result.success);
        assert!(
            !result.output.contains("sk-test-secret-12345"),
            "API_KEY leaked to shell command output"
        );
        assert!(
            !result.output.contains("sk-test-secret-67890"),
            "SYNAPSECLAW_API_KEY leaked to shell command output"
        );
    }

    #[tokio::test]
    async fn shell_preserves_path_and_home_for_env_command() {
        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime());

        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command should succeed");
        assert!(result.success);
        assert!(
            result.output.contains("HOME="),
            "HOME should be available in shell environment"
        );
        assert!(
            result.output.contains("PATH="),
            "PATH should be available in shell environment"
        );
    }

    #[tokio::test]
    async fn shell_blocks_plain_variable_expansion() {
        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime());
        let result = tool
            .execute(json!({"command": "echo $HOME"}))
            .await
            .expect("plain variable expansion should be blocked");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not allowed"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shell_allows_configured_env_passthrough() {
        let _guard = EnvGuard::set("SYNAPSECLAW_TEST_PASSTHROUGH", "db://unit-test");
        let tool = ShellTool::new(
            test_security_with_env_passthrough(&["SYNAPSECLAW_TEST_PASSTHROUGH"]),
            test_runtime(),
        );

        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command execution should succeed");
        assert!(result.success);
        assert!(result
            .output
            .contains("SYNAPSECLAW_TEST_PASSTHROUGH=db://unit-test"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shell_reconstructs_session_bus_from_runtime_dir() {
        let runtime_dir = tempfile::tempdir().unwrap();
        std::fs::write(runtime_dir.path().join("bus"), "").unwrap();
        let _runtime_guard = EnvGuard::set(
            "XDG_RUNTIME_DIR",
            runtime_dir.path().to_string_lossy().as_ref(),
        );
        let _bus_guard = EnvGuard::set("DBUS_SESSION_BUS_ADDRESS", "");

        let tool = ShellTool::new(test_security_with_env_cmd(), test_runtime());
        let result = tool
            .execute(json!({"command": "env"}))
            .await
            .expect("env command execution should succeed");

        assert!(result.success);
        assert!(result
            .output
            .contains(&format!("XDG_RUNTIME_DIR={}", runtime_dir.path().display())));
        assert!(result.output.contains(&format!(
            "DBUS_SESSION_BUS_ADDRESS=unix:path={}/bus",
            runtime_dir.path().display()
        )));
    }

    #[test]
    fn invalid_shell_env_passthrough_names_are_filtered() {
        let security = SecurityPolicy {
            shell_env_passthrough: vec![
                "VALID_NAME".into(),
                "BAD-NAME".into(),
                "1NOPE".into(),
                "ALSO_VALID".into(),
            ],
            ..SecurityPolicy::default()
        };
        let vars = collect_allowed_shell_env_vars(&security);
        assert!(vars.contains(&"VALID_NAME".to_string()));
        assert!(vars.contains(&"ALSO_VALID".to_string()));
        assert!(!vars.contains(&"BAD-NAME".to_string()));
        assert!(!vars.contains(&"1NOPE".to_string()));
    }

    #[test]
    fn extract_facts_emits_shell_context() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let facts = tool.extract_facts(
            &json!({"command": "cargo test -q"}),
            Some(&ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            }),
        );

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].focus_entities()[0].kind, "workspace_directory");
        assert_eq!(
            facts[0].focus_entities()[0].metadata.as_deref(),
            Some("shell_cwd")
        );
        assert!(facts[0]
            .focus_entities()
            .iter()
            .any(|entity| entity.kind == "shell_command" && entity.name == "cargo test -q"));
        assert!(facts[0].subjects().is_empty());
    }

    #[test]
    fn extract_facts_emits_service_and_network_context() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Supervised), test_runtime());
        let facts = tool.extract_facts(
            &json!({"command": "systemctl status synapseclaw.service && curl http://127.0.0.1:42617/health"}),
            Some(&ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            }),
        );

        assert!(facts[0]
            .focus_entities()
            .iter()
            .any(|entity| entity.kind == "system_service"
                && entity.name == "synapseclaw.service"
                && entity.metadata.as_deref() == Some("systemctl")));
        assert!(facts[0]
            .focus_entities()
            .iter()
            .any(|entity| entity.kind == "network_endpoint"
                && entity.name == "http://127.0.0.1:42617/health"
                && entity.metadata.as_deref() == Some("health_check")));
        assert!(facts.iter().any(|fact| matches!(
            &fact.payload,
            ToolFactPayload::Resource(resource)
                if resource.kind == ResourceKind::NetworkEndpoint
                    && resource.operation == ResourceOperation::Verify
                    && resource.locator == "http://127.0.0.1:42617/health"
        )));
    }

    #[tokio::test]
    async fn shell_requires_approval_for_medium_risk_command() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            allowed_commands: vec!["touch".into()],
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });

        let tool = ShellTool::new(security.clone(), test_runtime());
        let denied = tool
            .execute(json!({"command": "touch synapseclaw_shell_approval_test"}))
            .await
            .expect("unapproved command should return a result");
        assert!(!denied.success);
        assert!(denied
            .error
            .as_deref()
            .unwrap_or("")
            .contains("explicit approval"));

        let allowed = tool
            .execute(json!({
                "command": "touch synapseclaw_shell_approval_test",
                "approved": true
            }))
            .await
            .expect("approved command execution should succeed");
        assert!(allowed.success);

        let _ =
            tokio::fs::remove_file(std::env::temp_dir().join("synapseclaw_shell_approval_test"))
                .await;
    }

    // ── shell timeout enforcement tests ─────────────────

    #[test]
    fn shell_timeout_constant_is_reasonable() {
        assert_eq!(SHELL_TIMEOUT_SECS, 60, "shell timeout must be 60 seconds");
    }

    #[test]
    fn shell_output_limit_is_1mb() {
        assert_eq!(
            MAX_OUTPUT_BYTES, 1_048_576,
            "max output must be 1 MB to prevent OOM"
        );
    }

    // ── Non-UTF8 binary output tests ────────────────────

    #[test]
    fn shell_safe_env_vars_excludes_secrets() {
        for var in SAFE_ENV_VARS {
            let lower = var.to_lowercase();
            assert!(
                !lower.contains("key") && !lower.contains("secret") && !lower.contains("token"),
                "SAFE_ENV_VARS must not include sensitive variable: {var}"
            );
        }
    }

    #[test]
    fn shell_safe_env_vars_includes_essentials() {
        assert!(
            SAFE_ENV_VARS.contains(&"PATH"),
            "PATH must be in safe env vars"
        );
        assert!(
            SAFE_ENV_VARS.contains(&"HOME") || SAFE_ENV_VARS.contains(&"USERPROFILE"),
            "HOME or USERPROFILE must be in safe env vars"
        );
        assert!(
            SAFE_ENV_VARS.contains(&"TERM"),
            "TERM must be in safe env vars"
        );
        assert!(
            SAFE_ENV_VARS.contains(&"XDG_RUNTIME_DIR"),
            "XDG_RUNTIME_DIR must be in safe env vars"
        );
    }

    #[tokio::test]
    async fn shell_blocks_rate_limited() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            max_actions_per_hour: 0,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = ShellTool::new(security, test_runtime());
        let result = tool
            .execute(json!({"command": "echo test"}))
            .await
            .expect("rate-limited command should return a result");
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("Rate limit"));
    }

    #[tokio::test]
    async fn shell_handles_nonexistent_command() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = ShellTool::new(security, test_runtime());
        let result = tool
            .execute(json!({"command": "nonexistent_binary_xyz_12345"}))
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn shell_captures_stderr_output() {
        let tool = ShellTool::new(test_security(AutonomyLevel::Full), test_runtime());
        let result = tool
            .execute(json!({"command": "echo error_msg >&2"}))
            .await
            .unwrap();
        assert!(result.error.as_deref().unwrap_or("").contains("error_msg"));
    }

    #[tokio::test]
    async fn shell_record_action_budget_exhaustion() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            max_actions_per_hour: 1,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = ShellTool::new(security, test_runtime());

        let r1 = tool
            .execute(json!({"command": "echo first"}))
            .await
            .unwrap();
        assert!(r1.success);

        let r2 = tool
            .execute(json!({"command": "echo second"}))
            .await
            .unwrap();
        assert!(!r2.success);
        assert!(
            r2.error.as_deref().unwrap_or("").contains("Rate limit")
                || r2.error.as_deref().unwrap_or("").contains("budget")
        );
    }
}
