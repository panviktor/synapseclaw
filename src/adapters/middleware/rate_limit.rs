//! Rate limit middleware — caps tool calls per run.
//!
//! Phase 4.1 Slice 3: prevents feedback loops by limiting how many times
//! a tool can be called within a single pipeline run or agent session.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use synapse_core::domain::tool_middleware::{ToolBlock, ToolCallContext};
use synapse_core::ports::tool_middleware::ToolMiddlewarePort;

/// Rate limit middleware: blocks tool calls that exceed configured limits.
///
/// Limits are per-tool per-run. If no run_id is present (standalone agent),
/// limits apply per-agent globally.
/// Maximum number of tracked run scopes before eviction.
const MAX_TRACKED_SCOPES: usize = 10_000;

pub struct RateLimitMiddleware {
    /// Max calls per tool per run. Key = tool name, value = max calls.
    /// Tools not in the map have no limit.
    limits: HashMap<String, u32>,
    /// Default limit for tools not in the map (0 = unlimited).
    default_limit: u32,
    /// Counters: key = (run_or_agent_id, tool_name), value = call count.
    /// Evicted when exceeding MAX_TRACKED_SCOPES to prevent memory leak.
    counters: Mutex<HashMap<(String, String), u32>>,
}

impl RateLimitMiddleware {
    /// Create with per-tool limits.
    pub fn new(limits: HashMap<String, u32>, default_limit: u32) -> Self {
        Self {
            limits,
            default_limit,
            counters: Mutex::new(HashMap::new()),
        }
    }

    /// Create with a single default limit for all tools.
    pub fn with_default_limit(limit: u32) -> Self {
        Self::new(HashMap::new(), limit)
    }

    fn scope_key(ctx: &ToolCallContext) -> String {
        ctx.run_id.as_deref().unwrap_or(&ctx.agent_id).to_string()
    }

    fn limit_for(&self, tool_name: &str) -> u32 {
        self.limits
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_limit)
    }
}

#[async_trait]
impl ToolMiddlewarePort for RateLimitMiddleware {
    async fn before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock> {
        let limit = self.limit_for(&ctx.tool_name);
        if limit == 0 {
            return Ok(()); // unlimited
        }

        let key = (Self::scope_key(ctx), ctx.tool_name.clone());
        let mut counters = self.counters.lock().unwrap();

        // Evict old entries to prevent unbounded memory growth
        if counters.len() > MAX_TRACKED_SCOPES {
            counters.clear();
        }

        let count = counters.entry(key).or_insert(0);
        *count += 1;

        if *count > limit {
            return Err(ToolBlock::RateLimited {
                tool: ctx.tool_name.clone(),
                limit,
                window_secs: 0, // per-run, not time-based
            });
        }

        Ok(())
    }

    async fn after(&self, _ctx: &ToolCallContext, _result: &mut Value) -> Result<(), ToolBlock> {
        Ok(())
    }

    fn name(&self) -> &str {
        "rate_limit"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_with_run(run_id: &str, tool: &str) -> ToolCallContext {
        ToolCallContext {
            run_id: Some(run_id.into()),
            pipeline_name: None,
            step_id: None,
            agent_id: "agent".into(),
            tool_name: tool.into(),
            args: json!({}),
            call_count: 0,
        }
    }

    #[tokio::test]
    async fn within_limit_passes() {
        let mw = RateLimitMiddleware::with_default_limit(3);
        let ctx = ctx_with_run("run-1", "web_search");

        assert!(mw.before(&ctx).await.is_ok());
        assert!(mw.before(&ctx).await.is_ok());
        assert!(mw.before(&ctx).await.is_ok());
    }

    #[tokio::test]
    async fn exceeding_limit_blocks() {
        let mw = RateLimitMiddleware::with_default_limit(2);
        let ctx = ctx_with_run("run-1", "web_search");

        assert!(mw.before(&ctx).await.is_ok());
        assert!(mw.before(&ctx).await.is_ok());
        let err = mw.before(&ctx).await.unwrap_err();
        assert!(matches!(err, ToolBlock::RateLimited { limit: 2, .. }));
    }

    #[tokio::test]
    async fn different_runs_have_separate_counters() {
        let mw = RateLimitMiddleware::with_default_limit(1);

        assert!(mw.before(&ctx_with_run("run-1", "search")).await.is_ok());
        assert!(mw.before(&ctx_with_run("run-2", "search")).await.is_ok());
        // run-1 exhausted
        assert!(mw.before(&ctx_with_run("run-1", "search")).await.is_err());
    }

    #[tokio::test]
    async fn per_tool_limits() {
        let mut limits = HashMap::new();
        limits.insert("dangerous_tool".into(), 1);
        let mw = RateLimitMiddleware::new(limits, 100);

        let ctx1 = ctx_with_run("run-1", "dangerous_tool");
        let ctx2 = ctx_with_run("run-1", "safe_tool");

        assert!(mw.before(&ctx1).await.is_ok());
        assert!(mw.before(&ctx1).await.is_err()); // blocked

        // safe_tool has default limit of 100
        assert!(mw.before(&ctx2).await.is_ok());
    }

    #[tokio::test]
    async fn zero_limit_means_unlimited() {
        let mw = RateLimitMiddleware::with_default_limit(0);
        let ctx = ctx_with_run("run-1", "any_tool");

        for _ in 0..100 {
            assert!(mw.before(&ctx).await.is_ok());
        }
    }
}
