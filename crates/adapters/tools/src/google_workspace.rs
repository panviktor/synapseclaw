use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    ResourceFact, ResourceKind, ResourceMetadata, ResourceOperation, SearchDomain, SearchFact,
    ToolFactPayload, TypedToolFact,
};

/// Default `gws` command execution time before kill (overridden by config).
const DEFAULT_GWS_TIMEOUT_SECS: u64 = 30;
/// Maximum output size in bytes (1MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Allowed Google Workspace services that gws can target.
const DEFAULT_ALLOWED_SERVICES: &[&str] = &[
    "drive",
    "sheets",
    "gmail",
    "calendar",
    "docs",
    "slides",
    "tasks",
    "people",
    "chat",
    "classroom",
    "forms",
    "keep",
    "meet",
    "events",
];

/// Google Workspace CLI (`gws`) integration tool.
///
/// Wraps the `gws` CLI binary to give the agent structured access to
/// Google Workspace services (Drive, Gmail, Calendar, Sheets, etc.).
/// Requires `gws` to be installed and authenticated (`gws auth login`).
pub struct GoogleWorkspaceTool {
    security: Arc<SecurityPolicy>,
    allowed_services: Vec<String>,
    credentials_path: Option<String>,
    default_account: Option<String>,
    rate_limit_per_minute: u32,
    timeout_secs: u64,
    audit_log: bool,
}

impl GoogleWorkspaceTool {
    /// Create a new `GoogleWorkspaceTool`.
    ///
    /// If `allowed_services` is empty, the default service set is used.
    pub fn new(
        security: Arc<SecurityPolicy>,
        allowed_services: Vec<String>,
        credentials_path: Option<String>,
        default_account: Option<String>,
        rate_limit_per_minute: u32,
        timeout_secs: u64,
        audit_log: bool,
    ) -> Self {
        let services = if allowed_services.is_empty() {
            DEFAULT_ALLOWED_SERVICES
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            allowed_services
        };
        Self {
            security,
            allowed_services: services,
            credentials_path,
            default_account,
            rate_limit_per_minute,
            timeout_secs,
            audit_log,
        }
    }

    fn resource_locator(args: &serde_json::Value) -> Option<String> {
        let service = args.get("service")?.as_str()?.trim();
        let resource = args.get("resource")?.as_str()?.trim();
        if service.is_empty() || resource.is_empty() {
            return None;
        }

        let mut locator = format!("gws://{service}/{resource}");
        if let Some(sub_resource) = args.get("sub_resource").and_then(|value| value.as_str()) {
            let sub_resource = sub_resource.trim();
            if !sub_resource.is_empty() {
                locator.push('/');
                locator.push_str(sub_resource);
            }
        }
        Some(locator)
    }

    fn resource_operation(method: &str) -> ResourceOperation {
        match method {
            "list" | "search" | "query" => ResourceOperation::Search,
            "get" | "read" | "fetch" => ResourceOperation::Read,
            "create" | "insert" | "append" | "send" => ResourceOperation::Write,
            "update" | "patch" | "modify" | "replace" | "delete" | "remove" => {
                ResourceOperation::Edit
            }
            _ => ResourceOperation::Inspect,
        }
    }

    fn is_search_method(method: &str) -> bool {
        matches!(method, "list" | "search" | "query")
    }

    fn search_query(args: &serde_json::Value) -> Option<String> {
        let params = args.get("params")?;
        match params {
            serde_json::Value::Object(map) if !map.is_empty() => Some(params.to_string()),
            _ => None,
        }
    }

    fn result_item_count(result: &ToolResult, args: &serde_json::Value) -> Option<usize> {
        if !result.success {
            return None;
        }

        let format = args
            .get("format")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("json");
        if format != "json" {
            return None;
        }

        let value: serde_json::Value = serde_json::from_str(&result.output).ok()?;
        match value {
            serde_json::Value::Array(items) => Some(items.len()),
            serde_json::Value::Object(map) => map
                .values()
                .find_map(|value| value.as_array().map(Vec::len))
                .or_else(|| (!map.is_empty()).then_some(map.len())),
            _ => None,
        }
    }

    fn build_result_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        let result = match result {
            Some(result) if result.success => result,
            _ => return Vec::new(),
        };

        let method = match args.get("method").and_then(serde_json::Value::as_str) {
            Some(method) => method,
            None => return Vec::new(),
        };
        let locator = match Self::resource_locator(args) {
            Some(locator) => locator,
            None => return Vec::new(),
        };
        let item_count = Self::result_item_count(result, args);

        let mut facts = vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Resource(ResourceFact {
                kind: ResourceKind::WebResource,
                operation: Self::resource_operation(method),
                locator: locator.clone(),
                host: None,
                metadata: ResourceMetadata {
                    byte_count: Some(result.output.len()),
                    item_count,
                    include_base64: None,
                },
            }),
        }];

        if Self::is_search_method(method) {
            facts.push(TypedToolFact {
                tool_id: self.name().to_string(),
                payload: ToolFactPayload::Search(SearchFact {
                    domain: SearchDomain::Workspace,
                    query: Self::search_query(args),
                    result_count: item_count,
                    primary_locator: Some(locator),
                }),
            });
        }

        facts
    }
}

#[async_trait]
impl Tool for GoogleWorkspaceTool {
    fn name(&self) -> &str {
        "google_workspace"
    }

    fn description(&self) -> &str {
        "Interact with Google Workspace services (Drive, Gmail, Calendar, Sheets, Docs, etc.) \
         via the gws CLI. Requires gws to be installed and authenticated."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "service": {
                    "type": "string",
                    "description": "Google Workspace service (e.g. drive, gmail, calendar, sheets, docs, slides, tasks, people, chat, forms, keep, meet)"
                },
                "resource": {
                    "type": "string",
                    "description": "Service resource (e.g. files, messages, events, spreadsheets)"
                },
                "method": {
                    "type": "string",
                    "description": "Method to call on the resource (e.g. list, get, create, update, delete)"
                },
                "sub_resource": {
                    "type": "string",
                    "description": "Optional sub-resource for nested operations"
                },
                "params": {
                    "type": "object",
                    "description": "URL/query parameters as key-value pairs (passed as --params JSON)"
                },
                "body": {
                    "type": "object",
                    "description": "Request body for POST/PATCH/PUT operations (passed as --json JSON)"
                },
                "format": {
                    "type": "string",
                    "enum": ["json", "table", "yaml", "csv"],
                    "description": "Output format (default: json)"
                },
                "page_all": {
                    "type": "boolean",
                    "description": "Auto-paginate through all results"
                },
                "page_limit": {
                    "type": "integer",
                    "description": "Max pages to fetch when using page_all (default: 10)"
                }
            },
            "required": ["service", "resource", "method"]
        })
    }

    /// Execute a Google Workspace CLI command with input validation and security enforcement.
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let service = args
            .get("service")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'service' parameter"))?;
        let resource = args
            .get("resource")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'resource' parameter"))?;
        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'method' parameter"))?;

        // Security checks
        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        // Validate service is in the allowlist
        if !self.allowed_services.iter().any(|s| s == service) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Service '{service}' is not in the allowed services list. \
                     Allowed: {}",
                    self.allowed_services.join(", ")
                )),
            });
        }

        // Validate inputs contain no shell metacharacters
        for (label, value) in [
            ("service", service),
            ("resource", resource),
            ("method", method),
        ] {
            if !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Invalid characters in '{label}': only alphanumeric, underscore, and hyphen are allowed"
                    )),
                });
            }
        }

        // Build the gws command — validate all optional fields before consuming budget
        let mut cmd_args: Vec<String> = vec![service.to_string(), resource.to_string()];

        if let Some(sub_resource_value) = args.get("sub_resource") {
            let sub_resource = match sub_resource_value.as_str() {
                Some(s) => s,
                None => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("'sub_resource' must be a string".into()),
                    })
                }
            };
            if !sub_resource
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "Invalid characters in 'sub_resource': only alphanumeric, underscore, and hyphen are allowed"
                            .into(),
                    ),
                });
            }
            cmd_args.push(sub_resource.to_string());
        }

        cmd_args.push(method.to_string());

        if let Some(params) = args.get("params") {
            if !params.is_object() {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("'params' must be an object".into()),
                });
            }
            cmd_args.push("--params".into());
            cmd_args.push(params.to_string());
        }

        if let Some(body) = args.get("body") {
            if !body.is_object() {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("'body' must be an object".into()),
                });
            }
            cmd_args.push("--json".into());
            cmd_args.push(body.to_string());
        }

        if let Some(format_value) = args.get("format") {
            let format = match format_value.as_str() {
                Some(s) => s,
                None => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("'format' must be a string".into()),
                    })
                }
            };
            match format {
                "json" | "table" | "yaml" | "csv" => {
                    cmd_args.push("--format".into());
                    cmd_args.push(format.to_string());
                }
                _ => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!(
                            "Invalid format '{format}': must be json, table, yaml, or csv"
                        )),
                    });
                }
            }
        }

        let page_all = match args.get("page_all") {
            Some(v) => match v.as_bool() {
                Some(b) => b,
                None => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("'page_all' must be a boolean".into()),
                    })
                }
            },
            None => false,
        };
        if page_all {
            cmd_args.push("--page-all".into());
        }

        let page_limit = match args.get("page_limit") {
            Some(v) => match v.as_u64() {
                Some(n) => Some(n),
                None => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("'page_limit' must be a non-negative integer".into()),
                    })
                }
            },
            None => None,
        };
        if page_all || page_limit.is_some() {
            cmd_args.push("--page-limit".into());
            cmd_args.push(page_limit.unwrap_or(10).to_string());
        }

        // Charge action budget only after all validation passes
        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        let mut cmd = tokio::process::Command::new("gws");
        cmd.args(&cmd_args);
        cmd.env_clear();
        // gws needs PATH to find itself and HOME/APPDATA for credential storage
        for key in &["PATH", "HOME", "APPDATA", "USERPROFILE", "LANG", "TERM"] {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }

        // Apply credential path if configured
        if let Some(ref creds) = self.credentials_path {
            cmd.env("GOOGLE_APPLICATION_CREDENTIALS", creds);
        }

        // Apply default account if configured
        if let Some(ref account) = self.default_account {
            cmd.args(["--account", account]);
        }

        if self.audit_log {
            tracing::info!(
                tool = "google_workspace",
                service = service,
                resource = resource,
                method = method,
                "gws audit: executing API call"
            );
        }

        // Apply credential path if configured
        if let Some(ref creds) = self.credentials_path {
            cmd.env("GOOGLE_APPLICATION_CREDENTIALS", creds);
        }

        // Apply default account if configured
        if let Some(ref account) = self.default_account {
            cmd.args(["--account", account]);
        }

        if self.audit_log {
            tracing::info!(
                tool = "google_workspace",
                service = service,
                resource = resource,
                method = method,
                "gws audit: executing API call"
            );
        }

        let result =
            tokio::time::timeout(Duration::from_secs(self.timeout_secs), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if stdout.len() > MAX_OUTPUT_BYTES {
                    // Find a valid char boundary at or before MAX_OUTPUT_BYTES
                    let mut boundary = MAX_OUTPUT_BYTES;
                    while boundary > 0 && !stdout.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    stdout.truncate(boundary);
                    stdout.push_str("\n... [output truncated at 1MB]");
                }
                if stderr.len() > MAX_OUTPUT_BYTES {
                    let mut boundary = MAX_OUTPUT_BYTES;
                    while boundary > 0 && !stderr.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    stderr.truncate(boundary);
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
                error: Some(format!(
                    "Failed to execute gws: {e}. Is gws installed? Run: npm install -g @googleworkspace/cli"
                )),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "gws command timed out after {}s and was killed", self.timeout_secs
                )),
            }),
        }
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        self.build_result_facts(args, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::config::AutonomyLevel;
    use synapse_domain::domain::security_policy::SecurityPolicy;
    use synapse_domain::domain::tool_fact::{ToolFactPayload, TypedToolFact};

    fn test_security() -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    #[test]
    fn tool_name() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        assert_eq!(tool.name(), "google_workspace");
    }

    #[test]
    fn tool_description_non_empty() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn tool_schema_has_required_fields() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["service"].is_object());
        assert!(schema["properties"]["resource"].is_object());
        assert!(schema["properties"]["method"].is_object());
        let required = schema["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("service")));
        assert!(required.contains(&json!("resource")));
        assert!(required.contains(&json!("method")));
    }

    #[test]
    fn default_allowed_services_populated() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        assert!(!tool.allowed_services.is_empty());
        assert!(tool.allowed_services.contains(&"drive".to_string()));
        assert!(tool.allowed_services.contains(&"gmail".to_string()));
        assert!(tool.allowed_services.contains(&"calendar".to_string()));
    }

    #[test]
    fn custom_allowed_services_override_defaults() {
        let tool = GoogleWorkspaceTool::new(
            test_security(),
            vec!["drive".into(), "sheets".into()],
            None,
            None,
            60,
            30,
            false,
        );
        assert_eq!(tool.allowed_services.len(), 2);
        assert!(tool.allowed_services.contains(&"drive".to_string()));
        assert!(tool.allowed_services.contains(&"sheets".to_string()));
        assert!(!tool.allowed_services.contains(&"gmail".to_string()));
    }

    #[tokio::test]
    async fn rejects_disallowed_service() {
        let tool = GoogleWorkspaceTool::new(
            test_security(),
            vec!["drive".into()],
            None,
            None,
            60,
            30,
            false,
        );
        let result = tool
            .execute(json!({
                "service": "gmail",
                "resource": "users",
                "method": "list"
            }))
            .await
            .expect("disallowed service should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("not in the allowed"));
    }

    #[tokio::test]
    async fn rejects_shell_injection_in_service() {
        let tool = GoogleWorkspaceTool::new(
            test_security(),
            vec!["drive; rm -rf /".into()],
            None,
            None,
            60,
            30,
            false,
        );
        let result = tool
            .execute(json!({
                "service": "drive; rm -rf /",
                "resource": "files",
                "method": "list"
            }))
            .await
            .expect("shell injection should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Invalid characters"));
    }

    #[tokio::test]
    async fn rejects_shell_injection_in_resource() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files$(whoami)",
                "method": "list"
            }))
            .await
            .expect("shell injection should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Invalid characters"));
    }

    #[tokio::test]
    async fn rejects_invalid_format() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "list",
                "format": "xml"
            }))
            .await
            .expect("invalid format should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Invalid format"));
    }

    #[tokio::test]
    async fn rejects_wrong_type_params() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "list",
                "params": "not_an_object"
            }))
            .await
            .expect("wrong type params should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("'params' must be an object"));
    }

    #[tokio::test]
    async fn rejects_wrong_type_body() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "create",
                "body": "not_an_object"
            }))
            .await
            .expect("wrong type body should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("'body' must be an object"));
    }

    #[tokio::test]
    async fn rejects_wrong_type_page_all() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "list",
                "page_all": "yes"
            }))
            .await
            .expect("wrong type page_all should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("'page_all' must be a boolean"));
    }

    #[tokio::test]
    async fn rejects_wrong_type_page_limit() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "list",
                "page_limit": "ten"
            }))
            .await
            .expect("wrong type page_limit should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("'page_limit' must be a non-negative integer"));
    }

    #[tokio::test]
    async fn rejects_wrong_type_sub_resource() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "list",
                "sub_resource": 123
            }))
            .await
            .expect("wrong type sub_resource should return a result");
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("'sub_resource' must be a string"));
    }

    #[tokio::test]
    async fn missing_required_param_returns_error() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let result = tool.execute(json!({"service": "drive"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn rate_limited_returns_error() {
        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Full,
            max_actions_per_hour: 0,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        });
        let tool = GoogleWorkspaceTool::new(security, vec![], None, None, 60, 30, false);
        let result = tool
            .execute(json!({
                "service": "drive",
                "resource": "files",
                "method": "list"
            }))
            .await
            .expect("rate-limited should return a result");
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("Rate limit"));
    }

    #[test]
    fn gws_timeout_is_reasonable() {
        assert_eq!(DEFAULT_GWS_TIMEOUT_SECS, 30);
    }

    fn resource_fact<'a>(facts: &'a [TypedToolFact]) -> &'a ResourceFact {
        facts
            .iter()
            .find_map(|fact| match &fact.payload {
                ToolFactPayload::Resource(resource) => Some(resource),
                _ => None,
            })
            .expect("resource fact")
    }

    fn search_fact<'a>(facts: &'a [TypedToolFact]) -> &'a SearchFact {
        facts
            .iter()
            .find_map(|fact| match &fact.payload {
                ToolFactPayload::Search(search) => Some(search),
                _ => None,
            })
            .expect("search fact")
    }

    #[test]
    fn extract_facts_emits_workspace_resource_and_search_for_lists() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let args = json!({
            "service": "drive",
            "resource": "files",
            "method": "list",
            "params": {"q": "mimeType='application/pdf'"},
        });
        let result = ToolResult {
            success: true,
            output: r#"{"files":[{"id":"1"},{"id":"2"}]}"#.into(),
            error: None,
        };

        let facts = tool.extract_facts(&args, Some(&result));

        assert_eq!(facts.len(), 2);
        let resource = resource_fact(&facts);
        assert_eq!(resource.kind, ResourceKind::WebResource);
        assert_eq!(resource.operation, ResourceOperation::Search);
        assert_eq!(resource.locator, "gws://drive/files");
        assert_eq!(resource.metadata.item_count, Some(2));

        let search = search_fact(&facts);
        assert_eq!(search.domain, SearchDomain::Workspace);
        assert_eq!(
            search.query.as_deref(),
            Some(r#"{"q":"mimeType='application/pdf'"}"#)
        );
        assert_eq!(search.result_count, Some(2));
        assert_eq!(search.primary_locator.as_deref(), Some("gws://drive/files"));
    }

    #[test]
    fn extract_facts_emits_only_resource_for_mutations() {
        let tool = GoogleWorkspaceTool::new(test_security(), vec![], None, None, 60, 30, false);
        let args = json!({
            "service": "calendar",
            "resource": "events",
            "method": "create",
            "body": {"summary": "Weekly sync"},
        });
        let result = ToolResult {
            success: true,
            output: r#"{"id":"evt_123"}"#.into(),
            error: None,
        };

        let facts = tool.extract_facts(&args, Some(&result));

        assert_eq!(facts.len(), 1);
        let resource = resource_fact(&facts);
        assert_eq!(resource.operation, ResourceOperation::Write);
        assert_eq!(resource.locator, "gws://calendar/events");
    }
}
