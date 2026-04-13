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
    /// Returns `None` when no explicit route matches.
    async fn route(&self, input: &RoutingInput) -> Option<RoutingResult>;

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
    async fn route(&self, input: &RoutingInput) -> Option<RoutingResult> {
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
        };
        let router = InMemoryRouter::new(table);

        let input = RoutingInput {
            content: "/test hello".into(),
            source_kind: "channel".into(),
            metadata: HashMap::new(),
        };
        let result = router.route(&input).await.unwrap();
        assert_eq!(result.target, "test-agent");
    }

    #[tokio::test]
    async fn in_memory_router_replace() {
        let table1 = RoutingTable { routes: vec![] };
        let router = InMemoryRouter::new(table1);

        let input = RoutingInput {
            content: "anything".into(),
            source_kind: "web".into(),
            metadata: HashMap::new(),
        };
        assert!(router.route(&input).await.is_none());

        let table2 = RoutingTable { routes: vec![] };
        router.replace(table2).await;
        assert!(router.route(&input).await.is_none());
    }
}
