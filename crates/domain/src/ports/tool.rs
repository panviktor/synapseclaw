//! Port: tool abstraction — domain-owned trait for agent capabilities.
//!
//! Tools are capabilities the agent can invoke (shell, file read, memory, etc.).
//! The trait lives in the domain so application services can reason about tools
//! without depending on concrete infrastructure implementations.

use crate::ports::agent_runtime::AgentToolFact;
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

/// Description of a tool for the LLM (function-calling spec).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
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

    /// Execute the tool with given arguments.
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult>;

    /// Emit explicit structured runtime facts for dialogue state / resolution.
    ///
    /// Generic slot collection happens outside the tool. Override this only when
    /// the tool owns real semantic meaning and can expose it without inferring
    /// it from arbitrary JSON key names.
    fn extract_facts(
        &self,
        _args: &serde_json::Value,
        _result: Option<&ToolResult>,
    ) -> Vec<AgentToolFact> {
        Vec::new()
    }

    /// Get the full spec for LLM registration.
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
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

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.0.execute(args).await
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<AgentToolFact> {
        self.0.extract_facts(args, result)
    }
}
