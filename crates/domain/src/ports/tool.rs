//! Port: tool abstraction — domain-owned trait for agent capabilities.
//!
//! Tools are capabilities the agent can invoke (shell, file read, memory, etc.).
//! The trait lives in the domain so application services can reason about tools
//! without depending on concrete infrastructure implementations.

use crate::domain::tool_fact::TypedToolFact;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Result of a tool execution plus explicit structured facts.
#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub result: ToolResult,
    pub facts: Vec<TypedToolFact>,
}

/// Description of a tool for the LLM (function-calling spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRuntimeRole {
    DirectDelivery,
    DelegatedDelivery,
    HistoricalLookup,
    WorkspaceDiscovery,
    RuntimeStateInspection,
    ProfileMutation,
    MemoryMutation,
    ExternalLookup,
}

pub fn tool_runtime_role_name(role: ToolRuntimeRole) -> &'static str {
    match role {
        ToolRuntimeRole::DirectDelivery => "direct_delivery",
        ToolRuntimeRole::DelegatedDelivery => "delegated_delivery",
        ToolRuntimeRole::HistoricalLookup => "historical_lookup",
        ToolRuntimeRole::WorkspaceDiscovery => "workspace_discovery",
        ToolRuntimeRole::RuntimeStateInspection => "runtime_state_inspection",
        ToolRuntimeRole::ProfileMutation => "profile_mutation",
        ToolRuntimeRole::MemoryMutation => "memory_mutation",
        ToolRuntimeRole::ExternalLookup => "external_lookup",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgumentTransform {
    UrlOriginPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolNonReplayableReason {
    NotReplayableByDefault,
    FreeFormCommand,
    MutatesState,
    ExternalSideEffect,
    RuntimeActivation,
    LargeOrPrivatePayload,
    PendingPrivacyPolicy,
    ProviderNative,
    Other(String),
}

impl ToolNonReplayableReason {
    pub fn label(&self) -> String {
        match self {
            Self::NotReplayableByDefault => "not_replayable_by_default".into(),
            Self::FreeFormCommand => "free_form_command".into(),
            Self::MutatesState => "mutates_state".into(),
            Self::ExternalSideEffect => "external_side_effect".into(),
            Self::RuntimeActivation => "runtime_activation".into(),
            Self::LargeOrPrivatePayload => "large_or_private_payload".into(),
            Self::PendingPrivacyPolicy => "pending_privacy_policy".into(),
            Self::ProviderNative => "provider_native".into(),
            Self::Other(reason) => reason.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPrivacyClass {
    Public,
    WorkspaceLocal,
    UserPrivate,
    SessionPrivate,
    Secret,
    Unknown,
}

impl ToolPrivacyClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::WorkspaceLocal => "workspace_local",
            Self::UserPrivate => "user_private",
            Self::SessionPrivate => "session_private",
            Self::Secret => "secret",
            Self::Unknown => "unknown",
        }
    }

    pub fn replay_safe(self) -> bool {
        matches!(self, Self::Public | Self::WorkspaceLocal)
    }
}

fn default_tool_privacy_class() -> ToolPrivacyClass {
    ToolPrivacyClass::Unknown
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolArgumentPolicy {
    pub name: String,
    pub replayable: bool,
    pub sensitive: bool,
    #[serde(default = "default_tool_privacy_class")]
    pub privacy: ToolPrivacyClass,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replayable_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<ToolArgumentTransform>,
}

impl ToolArgumentPolicy {
    pub fn replayable(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            replayable: true,
            sensitive: false,
            privacy: ToolPrivacyClass::Public,
            replayable_values: Vec::new(),
            transform: None,
        }
    }

    pub fn workspace_local(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            replayable: true,
            sensitive: false,
            privacy: ToolPrivacyClass::WorkspaceLocal,
            replayable_values: Vec::new(),
            transform: None,
        }
    }

    pub fn blocked(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            replayable: false,
            sensitive: false,
            privacy: ToolPrivacyClass::Unknown,
            replayable_values: Vec::new(),
            transform: None,
        }
    }

    pub fn sensitive(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            replayable: false,
            sensitive: true,
            privacy: ToolPrivacyClass::Secret,
            replayable_values: Vec::new(),
            transform: None,
        }
    }

    pub fn user_private(mut self) -> Self {
        self.privacy = ToolPrivacyClass::UserPrivate;
        self
    }

    pub fn session_private(mut self) -> Self {
        self.privacy = ToolPrivacyClass::SessionPrivate;
        self
    }

    pub fn secret(mut self) -> Self {
        self.privacy = ToolPrivacyClass::Secret;
        self.sensitive = true;
        self.replayable = false;
        self
    }

    pub fn with_values(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.replayable_values = values.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_transform(mut self, transform: ToolArgumentTransform) -> Self {
        self.transform = Some(transform);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContract {
    pub runtime_role: Option<ToolRuntimeRole>,
    pub replayable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_replayable_reason: Option<ToolNonReplayableReason>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ToolArgumentPolicy>,
}

impl ToolContract {
    pub fn replayable(runtime_role: Option<ToolRuntimeRole>) -> Self {
        Self {
            runtime_role,
            replayable: true,
            non_replayable_reason: None,
            arguments: Vec::new(),
        }
    }

    pub fn non_replayable(
        runtime_role: Option<ToolRuntimeRole>,
        reason: ToolNonReplayableReason,
    ) -> Self {
        Self {
            runtime_role,
            replayable: false,
            non_replayable_reason: Some(reason),
            arguments: Vec::new(),
        }
    }

    pub fn with_arguments(mut self, arguments: Vec<ToolArgumentPolicy>) -> Self {
        self.arguments = arguments;
        self
    }

    pub fn argument(&self, name: &str) -> Option<&ToolArgumentPolicy> {
        self.arguments.iter().find(|argument| argument.name == name)
    }

    pub fn is_default_unclassified(&self) -> bool {
        !self.replayable
            && self.non_replayable_reason == Some(ToolNonReplayableReason::NotReplayableByDefault)
            && self.arguments.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProtocolContract {
    pub tool_name: String,
    pub runtime_role: Option<ToolRuntimeRole>,
    pub contract: ToolContract,
}

impl ToolProtocolContract {
    pub fn from_tool<T>(tool: &T) -> Self
    where
        T: Tool + ?Sized,
    {
        Self {
            tool_name: tool.name().to_string(),
            runtime_role: tool.runtime_role(),
            contract: tool.tool_contract(),
        }
    }

    pub fn is_classified(&self) -> bool {
        !self.contract.is_default_unclassified()
    }
}

pub trait ToolProtocolImplementation {
    fn protocol_contract(&self) -> ToolProtocolContract;
}

impl<T> ToolProtocolImplementation for T
where
    T: Tool + ?Sized,
{
    fn protocol_contract(&self) -> ToolProtocolContract {
        ToolProtocolContract::from_tool(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContractIssue {
    pub tool_name: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContractInventoryRow {
    pub tool_name: String,
    pub runtime_role: Option<ToolRuntimeRole>,
    pub replayable: bool,
    pub non_replayable_reason: Option<ToolNonReplayableReason>,
    pub replayable_args: Vec<String>,
    pub blocked_args: Vec<String>,
    pub sensitive_args: Vec<String>,
    pub privacy_args: Vec<String>,
}

impl ToolContractInventoryRow {
    pub fn line(&self) -> String {
        let role = self
            .runtime_role
            .map(tool_runtime_role_name)
            .unwrap_or("none");
        if self.replayable {
            format!(
                "{} | role={} | replayable | args={} | privacy={}",
                self.tool_name,
                role,
                csv_or_dash(&self.replayable_args),
                csv_or_dash(&self.privacy_args)
            )
        } else {
            let reason = self
                .non_replayable_reason
                .as_ref()
                .map(ToolNonReplayableReason::label)
                .unwrap_or_else(|| "unspecified".into());
            format!(
                "{} | role={} | non_replayable | reason={reason} | privacy={}",
                self.tool_name,
                role,
                csv_or_dash(&self.privacy_args)
            )
        }
    }
}

pub fn tool_contract_inventory_row(
    tool_name: &str,
    runtime_role: Option<ToolRuntimeRole>,
    contract: &ToolContract,
) -> ToolContractInventoryRow {
    let mut replayable_args = Vec::new();
    let mut blocked_args = Vec::new();
    let mut sensitive_args = Vec::new();
    let mut privacy_args = Vec::new();

    for argument in &contract.arguments {
        privacy_args.push(format!("{}:{}", argument.name, argument.privacy.label()));
        if argument.sensitive {
            sensitive_args.push(argument.name.clone());
        } else if argument.replayable {
            replayable_args.push(argument.name.clone());
        } else {
            blocked_args.push(argument.name.clone());
        }
    }
    replayable_args.sort();
    blocked_args.sort();
    sensitive_args.sort();
    privacy_args.sort();

    ToolContractInventoryRow {
        tool_name: tool_name.to_string(),
        runtime_role,
        replayable: contract.replayable,
        non_replayable_reason: contract.non_replayable_reason.clone(),
        replayable_args,
        blocked_args,
        sensitive_args,
        privacy_args,
    }
}

pub fn validate_tool_contract(
    tool_name: &str,
    schema: &Value,
    runtime_role: Option<ToolRuntimeRole>,
    contract: &ToolContract,
) -> Vec<ToolContractIssue> {
    let mut issues = Vec::new();

    fn push_issue(
        issues: &mut Vec<ToolContractIssue>,
        tool_name: &str,
        message: impl Into<String>,
    ) {
        issues.push(ToolContractIssue {
            tool_name: tool_name.to_string(),
            message: message.into(),
        });
    }

    if contract.runtime_role != runtime_role {
        push_issue(
            &mut issues,
            tool_name,
            format!(
                "contract runtime_role {:?} does not match tool runtime_role {:?}",
                contract.runtime_role, runtime_role
            ),
        );
    }

    if contract.is_default_unclassified() {
        push_issue(
            &mut issues,
            tool_name,
            "tool contract is using the default unclassified policy",
        );
    }

    if contract.replayable && contract.non_replayable_reason.is_some() {
        push_issue(
            &mut issues,
            tool_name,
            "replayable contract must not carry a non_replayable_reason",
        );
    }
    if !contract.replayable && contract.non_replayable_reason.is_none() {
        push_issue(
            &mut issues,
            tool_name,
            "non-replayable contract must declare a reason",
        );
    }

    if replay_extension_path(schema).is_some() {
        push_issue(
            &mut issues,
            tool_name,
            "provider-facing schema must not contain x-synapse replay extensions",
        );
    }

    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        push_issue(
            &mut issues,
            tool_name,
            "schema must declare an object properties map",
        );
        return issues;
    };

    let mut seen = BTreeSet::new();
    let mut replayable_arg_count = 0usize;
    for argument in &contract.arguments {
        if !seen.insert(argument.name.clone()) {
            push_issue(
                &mut issues,
                tool_name,
                format!("duplicate contract argument '{}'", argument.name),
            );
        }
        let Some(property_schema) = properties.get(&argument.name) else {
            push_issue(
                &mut issues,
                tool_name,
                format!(
                    "contract argument '{}' is not present in schema properties",
                    argument.name
                ),
            );
            continue;
        };
        if argument.replayable {
            replayable_arg_count += 1;
        }
        if argument.sensitive && argument.replayable {
            push_issue(
                &mut issues,
                tool_name,
                format!(
                    "contract argument '{}' cannot be both sensitive and replayable",
                    argument.name
                ),
            );
        }
        if argument.replayable && !argument.privacy.replay_safe() {
            push_issue(
                &mut issues,
                tool_name,
                format!(
                    "replayable argument '{}' must declare public or workspace_local privacy",
                    argument.name
                ),
            );
        }
        if !argument.replayable && !argument.replayable_values.is_empty() {
            push_issue(
                &mut issues,
                tool_name,
                format!(
                    "blocked argument '{}' must not declare replayable values",
                    argument.name
                ),
            );
        }
        if argument.transform.is_some() && !argument.replayable {
            push_issue(
                &mut issues,
                tool_name,
                format!(
                    "blocked argument '{}' must not declare a replay transform",
                    argument.name
                ),
            );
        }
        if argument.transform.is_some() && !schema_type_includes(property_schema, "string") {
            push_issue(
                &mut issues,
                tool_name,
                format!(
                    "argument '{}' declares a string replay transform but schema type is not string",
                    argument.name
                ),
            );
        }
    }

    if contract.replayable {
        if !is_replayable_contract_role(contract.runtime_role) {
            push_issue(
                &mut issues,
                tool_name,
                "replayable contract uses a runtime role that is not replay-safe",
            );
        }
        let _ = replayable_arg_count;
        for required in required_schema_properties(schema) {
            match contract.argument(&required) {
                Some(argument)
                    if argument.replayable
                        && !argument.sensitive
                        && argument.privacy.replay_safe() => {}
                Some(_) => push_issue(
                    &mut issues,
                    tool_name,
                    format!("required schema argument '{required}' is not replayable in contract"),
                ),
                None => push_issue(
                    &mut issues,
                    tool_name,
                    format!("required schema argument '{required}' is missing from contract"),
                ),
            }
        }
    }

    issues
}

fn is_replayable_contract_role(role: Option<ToolRuntimeRole>) -> bool {
    matches!(
        role,
        Some(
            ToolRuntimeRole::HistoricalLookup
                | ToolRuntimeRole::WorkspaceDiscovery
                | ToolRuntimeRole::RuntimeStateInspection
                | ToolRuntimeRole::ExternalLookup
        )
    )
}

fn required_schema_properties(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn schema_type_includes(schema: &Value, expected: &str) -> bool {
    match schema.get("type") {
        Some(Value::String(value)) => value == expected,
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .any(|value| value == expected),
        _ => true,
    }
}

fn replay_extension_path(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key.starts_with(provider_replay_extension_prefix()) {
                    return Some(key.clone());
                }
                if let Some(path) = replay_extension_path(child) {
                    return Some(format!("{key}.{path}"));
                }
            }
            None
        }
        Value::Array(values) => values.iter().enumerate().find_map(|(idx, child)| {
            replay_extension_path(child).map(|path| format!("{idx}.{path}"))
        }),
        _ => None,
    }
}

fn provider_replay_extension_prefix() -> &'static str {
    concat!("x-synapse-", "replay")
}

fn csv_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".into()
    } else {
        values.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replayable_contract_validates_against_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "operation": { "type": "string" },
                "message": { "type": "string" }
            },
            "required": ["operation"]
        });
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::WorkspaceDiscovery))
            .with_arguments(vec![
                ToolArgumentPolicy::replayable("operation").with_values(["status"]),
                ToolArgumentPolicy::blocked("message"),
            ]);

        let issues = validate_tool_contract(
            "git_operations",
            &schema,
            Some(ToolRuntimeRole::WorkspaceDiscovery),
            &contract,
        );

        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn audit_rejects_provider_schema_replay_extensions() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" }
            },
            "required": ["url"]
        });
        schema.as_object_mut().unwrap().insert(
            format!("{}able", provider_replay_extension_prefix()),
            Value::Bool(true),
        );
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::ExternalLookup))
            .with_arguments(vec![ToolArgumentPolicy::replayable("url")]);

        let issues = validate_tool_contract(
            "web_fetch",
            &schema,
            Some(ToolRuntimeRole::ExternalLookup),
            &contract,
        );

        assert!(issues
            .iter()
            .any(|issue| issue.message.contains("x-synapse replay extensions")));
    }

    #[test]
    fn audit_requires_contract_arguments_to_exist_in_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" }
            }
        });
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::ExternalLookup))
            .with_arguments(vec![ToolArgumentPolicy::replayable("missing")]);

        let issues = validate_tool_contract(
            "lookup",
            &schema,
            Some(ToolRuntimeRole::ExternalLookup),
            &contract,
        );

        assert!(issues
            .iter()
            .any(|issue| issue.message.contains("not present in schema properties")));
    }

    #[test]
    fn audit_requires_replayable_required_arguments() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" }
            },
            "required": ["url"]
        });
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::ExternalLookup))
            .with_arguments(vec![ToolArgumentPolicy::sensitive("url")]);

        let issues = validate_tool_contract(
            "lookup",
            &schema,
            Some(ToolRuntimeRole::ExternalLookup),
            &contract,
        );

        assert!(issues
            .iter()
            .any(|issue| issue.message.contains("required schema argument 'url'")));
    }

    #[test]
    fn inventory_row_formats_replayable_args() {
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::WorkspaceDiscovery))
            .with_arguments(vec![ToolArgumentPolicy::replayable("path")]);
        let row = tool_contract_inventory_row(
            "file_read",
            Some(ToolRuntimeRole::WorkspaceDiscovery),
            &contract,
        );

        assert_eq!(
            row.line(),
            "file_read | role=workspace_discovery | replayable | args=path | privacy=path:public"
        );
    }

    #[test]
    fn audit_rejects_replayable_private_arguments() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        });
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::HistoricalLookup))
            .with_arguments(vec![ToolArgumentPolicy::replayable("query").user_private()]);

        let issues = validate_tool_contract(
            "memory_recall",
            &schema,
            Some(ToolRuntimeRole::HistoricalLookup),
            &contract,
        );

        assert!(issues
            .iter()
            .any(|issue| issue.message.contains("public or workspace_local privacy")));
        assert!(issues
            .iter()
            .any(|issue| issue.message.contains("required schema argument 'query'")));
    }
}

/// Description of a tool for the LLM (function-calling spec).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_role: Option<ToolRuntimeRole>,
}

/// Core tool trait — implement for any capability the agent can invoke.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used in LLM function calling).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON schema for parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Typed runtime role for intent narrowing and context-engine policy.
    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        None
    }

    /// Typed runtime contract used by internal policy engines.
    ///
    /// JSON schema remains the provider-facing function-call shape; this
    /// contract is the programmatic source for replay and safety policy.
    fn tool_contract(&self) -> ToolContract;

    /// Execute the tool with given arguments.
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult>;

    /// Execute the tool and return both the result and explicit runtime facts.
    ///
    /// The default implementation executes the tool and then asks the tool
    /// for typed facts. Tools that know result semantics should override this
    /// to emit facts directly from structured results instead of reconstructing
    /// them afterward.
    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        let result = self.execute(args.clone()).await?;
        let facts = self.extract_facts(&args, Some(&result));
        Ok(ToolExecution { result, facts })
    }

    /// Emit explicit structured runtime facts for dialogue state / resolution.
    ///
    /// Generic slot collection happens outside the tool. Override this only when
    /// the tool owns real semantic meaning and can expose it without inferring
    /// it from arbitrary JSON key names.
    fn extract_facts(
        &self,
        _args: &serde_json::Value,
        _result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        Vec::new()
    }

    /// Get the full spec for LLM registration.
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
            runtime_role: self.runtime_role(),
        }
    }
}

/// Thin wrapper that makes an `Arc<dyn Tool>` usable as `Box<dyn Tool>`.
pub struct ArcToolRef(pub Arc<dyn Tool>);

#[async_trait]
impl Tool for ArcToolRef {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.0.parameters_schema()
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        self.0.runtime_role()
    }

    fn tool_contract(&self) -> ToolContract {
        self.0.tool_contract()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.0.execute(args).await
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.0.execute_with_facts(args).await
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        self.0.extract_facts(args, result)
    }
}
