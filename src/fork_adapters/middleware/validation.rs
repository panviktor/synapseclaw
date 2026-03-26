//! Validation middleware — JSON Schema checks on tool arguments.
//!
//! Phase 4.1 Slice 3: rejects tool calls whose arguments don't match
//! a predefined JSON Schema.

use async_trait::async_trait;
use fork_core::domain::tool_middleware::{ToolBlock, ToolCallContext};
use fork_core::ports::tool_middleware::ToolMiddlewarePort;
use serde_json::Value;
use std::collections::HashMap;

/// Validation middleware: checks tool args against JSON Schemas.
///
/// Each tool can have an associated schema. Calls with invalid arguments
/// are rejected before execution.
pub struct ValidationMiddleware {
    /// Tool name → JSON Schema for its arguments.
    schemas: HashMap<String, Value>,
}

impl ValidationMiddleware {
    /// Create with a map of tool schemas.
    pub fn new(schemas: HashMap<String, Value>) -> Self {
        Self { schemas }
    }

    /// Create empty (no validation).
    pub fn empty() -> Self {
        Self::new(HashMap::new())
    }
}

#[async_trait]
impl ToolMiddlewarePort for ValidationMiddleware {
    async fn before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock> {
        let schema = match self.schemas.get(&ctx.tool_name) {
            Some(s) => s,
            None => return Ok(()), // no schema = no validation
        };

        let validator = match jsonschema::validator_for(schema) {
            Ok(v) => v,
            Err(e) => {
                return Err(ToolBlock::ValidationFailed {
                    tool: ctx.tool_name.clone(),
                    reason: format!("invalid schema: {e}"),
                });
            }
        };

        if validator.is_valid(&ctx.args) {
            return Ok(());
        }

        let errors: Vec<String> = validator
            .iter_errors(&ctx.args)
            .map(|e| e.to_string())
            .collect();

        Err(ToolBlock::ValidationFailed {
            tool: ctx.tool_name.clone(),
            reason: errors.join("; "),
        })
    }

    async fn after(
        &self,
        _ctx: &ToolCallContext,
        _result: &mut Value,
    ) -> Result<(), ToolBlock> {
        Ok(())
    }

    fn name(&self) -> &str {
        "validation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_with_args(tool: &str, args: Value) -> ToolCallContext {
        ToolCallContext {
            run_id: None,
            pipeline_name: None,
            step_id: None,
            agent_id: "agent".into(),
            tool_name: tool.into(),
            args,
            call_count: 0,
        }
    }

    #[tokio::test]
    async fn no_schema_passes() {
        let mw = ValidationMiddleware::empty();
        let ctx = ctx_with_args("any_tool", json!({"x": 1}));
        assert!(mw.before(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn valid_args_pass() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "web_search".into(),
            json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "minLength": 1 }
                }
            }),
        );
        let mw = ValidationMiddleware::new(schemas);
        let ctx = ctx_with_args("web_search", json!({"query": "rust async"}));
        assert!(mw.before(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn missing_required_field_blocked() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "web_search".into(),
            json!({
                "type": "object",
                "required": ["query"]
            }),
        );
        let mw = ValidationMiddleware::new(schemas);
        let ctx = ctx_with_args("web_search", json!({}));
        let err = mw.before(&ctx).await.unwrap_err();
        assert!(matches!(err, ToolBlock::ValidationFailed { .. }));
    }

    #[tokio::test]
    async fn wrong_type_blocked() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "memory_write".into(),
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string" }
                }
            }),
        );
        let mw = ValidationMiddleware::new(schemas);
        let ctx = ctx_with_args("memory_write", json!({"content": 42}));
        let err = mw.before(&ctx).await.unwrap_err();
        assert!(matches!(err, ToolBlock::ValidationFailed { .. }));
    }

    #[tokio::test]
    async fn unregistered_tool_passes() {
        let mut schemas = HashMap::new();
        schemas.insert("registered".into(), json!({"type": "object"}));
        let mw = ValidationMiddleware::new(schemas);
        let ctx = ctx_with_args("unregistered", json!("anything"));
        assert!(mw.before(&ctx).await.is_ok());
    }
}
