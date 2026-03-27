//! Port: message router.
//!
//! Phase 4.1 Slice 6: deterministic routing for inbound messages.
//! Implementations load routing rules from TOML and evaluate them.

use crate::domain::routing::{RoutingInput, RoutingResult, RoutingTable};
use async_trait::async_trait;

/// Port for resolving which agent should handle an inbound message.
///
/// The inbound message handler calls `route()` before dispatching.
/// Implementations manage the routing table (loading, hot-reload).
#[async_trait]
pub trait MessageRouterPort: Send + Sync {
    /// Route an inbound message to a target agent.
    async fn route(&self, input: &RoutingInput) -> RoutingResult;

    /// Reload routing rules from source (e.g. re-read TOML).
    async fn reload(&self) -> anyhow::Result<()>;
}

/// Simple in-memory router backed by a RoutingTable.
pub struct InMemoryRouter {
    table: tokio::sync::RwLock<RoutingTable>,
}

impl InMemoryRouter {
    pub fn new(table: RoutingTable) -> Self {
        Self {
            table: tokio::sync::RwLock::new(table),
        }
    }

    /// Replace the routing table (used by hot-reload).
    pub async fn replace(&self, table: RoutingTable) {
        *self.table.write().await = table;
    }
}

#[async_trait]
impl MessageRouterPort for InMemoryRouter {
    async fn route(&self, input: &RoutingInput) -> RoutingResult {
        self.table.read().await.resolve(input)
    }

    async fn reload(&self) -> anyhow::Result<()> {
        // In-memory router doesn't auto-reload — call replace() directly.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::{Route, RoutingRule, RoutingTable};
    use std::collections::HashMap;

    #[tokio::test]
    async fn in_memory_router_routes() {
        let table = RoutingTable {
            routes: vec![Route {
                name: "test".into(),
                rule: RoutingRule::Command("/test".into()),
                target: "test-agent".into(),
                pipeline: None,
                priority: 10,
            }],
            fallback: "default".into(),
        };
        let router = InMemoryRouter::new(table);

        let input = RoutingInput {
            content: "/test hello".into(),
            source_kind: "channel".into(),
            metadata: HashMap::new(),
        };
        let result = router.route(&input).await;
        assert_eq!(result.target, "test-agent");
    }

    #[tokio::test]
    async fn in_memory_router_replace() {
        let table1 = RoutingTable {
            routes: vec![],
            fallback: "old".into(),
        };
        let router = InMemoryRouter::new(table1);

        let input = RoutingInput {
            content: "anything".into(),
            source_kind: "web".into(),
            metadata: HashMap::new(),
        };
        assert_eq!(router.route(&input).await.target, "old");

        let table2 = RoutingTable {
            routes: vec![],
            fallback: "new".into(),
        };
        router.replace(table2).await;
        assert_eq!(router.route(&input).await.target, "new");
    }
}
