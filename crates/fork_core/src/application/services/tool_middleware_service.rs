//! Tool middleware chain — ordered execution of middleware hooks.
//!
//! Phase 4.1 Slice 3: chains multiple `ToolMiddlewarePort` implementations
//! and runs them in sequence (before: first→last, after: last→first).

use crate::domain::tool_middleware::{ToolBlock, ToolCallContext};
use crate::ports::tool_middleware::ToolMiddlewarePort;
use serde_json::Value;
use tracing::{debug, info};

/// Ordered chain of tool middleware.
///
/// `before()` hooks run first→last (rate limit → validation → approval).
/// `after()` hooks run last→first (reverse order).
pub struct ToolMiddlewareChain {
    middlewares: Vec<Box<dyn ToolMiddlewarePort>>,
}

impl ToolMiddlewareChain {
    /// Create an empty chain.
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Add a middleware to the end of the chain.
    pub fn push(&mut self, mw: Box<dyn ToolMiddlewarePort>) {
        self.middlewares.push(mw);
    }

    /// Number of middlewares in the chain.
    pub fn len(&self) -> usize {
        self.middlewares.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }

    /// Run all `before()` hooks in order.
    /// Returns `Err(ToolBlock)` on first rejection.
    pub async fn run_before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock> {
        for mw in &self.middlewares {
            if let Err(block) = mw.before(ctx).await {
                info!(
                    tool = %ctx.tool_name,
                    middleware = mw.name(),
                    block = %block,
                    "tool call blocked by middleware"
                );
                return Err(block);
            }
            debug!(tool = %ctx.tool_name, middleware = mw.name(), "before hook passed");
        }
        Ok(())
    }

    /// Run all `after()` hooks in reverse order.
    /// Returns `Err(ToolBlock)` on first rejection.
    pub async fn run_after(
        &self,
        ctx: &ToolCallContext,
        result: &mut Value,
    ) -> Result<(), ToolBlock> {
        for mw in self.middlewares.iter().rev() {
            if let Err(block) = mw.after(ctx, result).await {
                info!(
                    tool = %ctx.tool_name,
                    middleware = mw.name(),
                    block = %block,
                    "tool result blocked by middleware"
                );
                return Err(block);
            }
        }
        Ok(())
    }
}

impl Default for ToolMiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct PassMiddleware {
        name: &'static str,
        before_count: Arc<AtomicU32>,
        after_count: Arc<AtomicU32>,
    }

    impl PassMiddleware {
        fn new(name: &'static str) -> (Self, Arc<AtomicU32>, Arc<AtomicU32>) {
            let before = Arc::new(AtomicU32::new(0));
            let after = Arc::new(AtomicU32::new(0));
            (
                Self {
                    name,
                    before_count: before.clone(),
                    after_count: after.clone(),
                },
                before,
                after,
            )
        }
    }

    #[async_trait]
    impl ToolMiddlewarePort for PassMiddleware {
        async fn before(&self, _ctx: &ToolCallContext) -> Result<(), ToolBlock> {
            self.before_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        async fn after(&self, _ctx: &ToolCallContext, _result: &mut Value) -> Result<(), ToolBlock> {
            self.after_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn name(&self) -> &str {
            self.name
        }
    }

    struct BlockMiddleware {
        name: &'static str,
    }

    #[async_trait]
    impl ToolMiddlewarePort for BlockMiddleware {
        async fn before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock> {
            Err(ToolBlock::Denied {
                tool: ctx.tool_name.clone(),
                reason: format!("blocked by {}", self.name),
            })
        }
        async fn after(&self, _ctx: &ToolCallContext, _result: &mut Value) -> Result<(), ToolBlock> {
            Ok(())
        }
        fn name(&self) -> &str {
            self.name
        }
    }

    fn test_ctx() -> ToolCallContext {
        ToolCallContext {
            run_id: None,
            pipeline_name: None,
            step_id: None,
            agent_id: "test-agent".into(),
            tool_name: "web_search".into(),
            args: json!({}),
            call_count: 0,
        }
    }

    #[tokio::test]
    async fn empty_chain_passes() {
        let chain = ToolMiddlewareChain::new();
        assert!(chain.run_before(&test_ctx()).await.is_ok());
        assert!(chain.run_after(&test_ctx(), &mut json!({})).await.is_ok());
    }

    #[tokio::test]
    async fn all_pass_middlewares_run() {
        let mut chain = ToolMiddlewareChain::new();
        let (mw1, before1, after1) = PassMiddleware::new("mw1");
        let (mw2, before2, after2) = PassMiddleware::new("mw2");
        chain.push(Box::new(mw1));
        chain.push(Box::new(mw2));

        assert!(chain.run_before(&test_ctx()).await.is_ok());
        assert_eq!(before1.load(Ordering::Relaxed), 1);
        assert_eq!(before2.load(Ordering::Relaxed), 1);

        assert!(chain.run_after(&test_ctx(), &mut json!({})).await.is_ok());
        assert_eq!(after1.load(Ordering::Relaxed), 1);
        assert_eq!(after2.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn block_middleware_stops_chain() {
        let mut chain = ToolMiddlewareChain::new();
        let (mw1, before1, _) = PassMiddleware::new("mw1");
        chain.push(Box::new(mw1));
        chain.push(Box::new(BlockMiddleware { name: "blocker" }));
        let (mw3, before3, _) = PassMiddleware::new("mw3");
        chain.push(Box::new(mw3));

        let err = chain.run_before(&test_ctx()).await.unwrap_err();
        assert!(matches!(err, ToolBlock::Denied { .. }));

        // mw1 ran, mw3 did NOT run
        assert_eq!(before1.load(Ordering::Relaxed), 1);
        assert_eq!(before3.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn chain_len() {
        let mut chain = ToolMiddlewareChain::new();
        assert!(chain.is_empty());
        let (mw, _, _) = PassMiddleware::new("x");
        chain.push(Box::new(mw));
        assert_eq!(chain.len(), 1);
    }
}
