//! Port for storing standing orders across tool calls and runtime events.
//!
//! Orders must survive beyond a single tool invocation so runtime-native
//! features like "after restart, report here" can fire from daemon workers.

use crate::domain::standing_order::{StandingOrder, SystemEvent};
use anyhow::Result;

/// Durable or shared storage for standing orders.
pub trait StandingOrderStorePort: Send + Sync {
    /// Return all known standing orders.
    fn list(&self) -> Vec<StandingOrder>;

    /// Insert or replace an order by ID.
    fn upsert(&self, order: StandingOrder) -> Result<()>;

    /// Remove an order by ID. Returns true if it existed.
    fn remove(&self, id: &str) -> Result<bool>;

    /// Return enabled orders matching a runtime event.
    fn matching(&self, event: &SystemEvent) -> Vec<StandingOrder> {
        self.list()
            .into_iter()
            .filter(|order| order.matches_event(event))
            .collect()
    }
}

/// Lightweight in-memory store for tests and non-persistent contexts.
pub struct InMemoryStandingOrderStore {
    orders: parking_lot::RwLock<Vec<StandingOrder>>,
}

impl InMemoryStandingOrderStore {
    pub fn new() -> Self {
        Self {
            orders: parking_lot::RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryStandingOrderStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StandingOrderStorePort for InMemoryStandingOrderStore {
    fn list(&self) -> Vec<StandingOrder> {
        self.orders.read().clone()
    }

    fn upsert(&self, order: StandingOrder) -> Result<()> {
        let mut orders = self.orders.write();
        if let Some(existing) = orders.iter_mut().find(|existing| existing.id == order.id) {
            *existing = order;
        } else {
            orders.push(order);
        }
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<bool> {
        let mut orders = self.orders.write();
        let before = orders.len();
        orders.retain(|order| order.id != id);
        Ok(orders.len() < before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::standing_order::StandingOrderKind;

    fn order(id: &str, kind: StandingOrderKind) -> StandingOrder {
        StandingOrder {
            id: id.into(),
            kind,
            delivery_channel: "matrix".into(),
            delivery_recipient: "!room:example.com".into(),
            delivery_thread: Some("$thread".into()),
            enabled: true,
            created_by: "agent".into(),
            created_at: 1,
        }
    }

    #[test]
    fn upsert_and_matching_work() {
        let store = InMemoryStandingOrderStore::new();
        store
            .upsert(order("restart", StandingOrderKind::RestartReport))
            .unwrap();
        store
            .upsert(order("heartbeat", StandingOrderKind::HeartbeatReport))
            .unwrap();

        let restart = store.matching(&SystemEvent::RuntimeRestarted);
        assert_eq!(restart.len(), 1);
        assert_eq!(restart[0].id, "restart");

        let heartbeat = store.matching(&SystemEvent::HeartbeatTick);
        assert_eq!(heartbeat.len(), 1);
        assert_eq!(heartbeat[0].id, "heartbeat");
    }

    #[test]
    fn remove_reports_existence() {
        let store = InMemoryStandingOrderStore::new();
        store
            .upsert(order("restart", StandingOrderKind::RestartReport))
            .unwrap();
        assert!(store.remove("restart").unwrap());
        assert!(!store.remove("restart").unwrap());
    }
}
