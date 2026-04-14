//! Port: tool abstraction — domain-owned trait for agent capabilities.
//!
//! Tools are capabilities the agent can invoke (shell, file read, memory, etc.).
//! The trait lives in the domain so application services can reason about tools
//! without depending on concrete infrastructure implementations.

use crate::domain::tool_fact::TypedToolFact;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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
