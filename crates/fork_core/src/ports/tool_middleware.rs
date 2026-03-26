//! Port: tool call middleware.
//!
//! Phase 4.1 Slice 3: before/after hooks on tool execution.
//! Implementations: RateLimitMiddleware, ValidationMiddleware, ApprovalGateMiddleware.

use crate::domain::tool_middleware::{ToolBlock, ToolCallContext};
use async_trait::async_trait;
use serde_json::Value;

/// Port for intercepting tool calls.
///
/// The agent loop calls `before()` before executing a tool and `after()`
/// after execution. Middleware can block calls, modify results, or
/// collect metrics.
///
/// Multiple middlewares are chained in order by `ToolMiddlewareChain`.
#[async_trait]
pub trait ToolMiddlewarePort: Send + Sync {
    /// Called before tool execution.
    /// Return `Err(ToolBlock)` to prevent the tool from running.
    async fn before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock>;

    /// Called after tool execution.
    /// Can inspect or modify the result. Return `Err(ToolBlock)` to suppress
    /// the result (the LLM receives the block reason instead).
    async fn after(&self, ctx: &ToolCallContext, result: &mut Value) -> Result<(), ToolBlock>;

    /// Human-readable name for logging.
    fn name(&self) -> &str;
}
