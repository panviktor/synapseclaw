//! File-backed standing order store.
//!
//! Small JSON persistence layer used by daemon/channel runtime so standing
//! orders survive process restarts.

use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::fs;
use std::path::{Path, PathBuf};
use synapse_domain::domain::standing_order::StandingOrder;
use synapse_domain::ports::standing_order_store::StandingOrderStorePort;

pub struct FileStandingOrderStore {
    path: PathBuf,
    orders: RwLock<Vec<StandingOrder>>,
}

impl FileStandingOrderStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let orders = if path.exists() {
            let bytes = fs::read(&path).with_context(|| {
                format!("failed to read standing order store {}", path.display())
            })?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(&bytes).with_context(|| {
                    format!("failed to parse standing order store {}", path.display())
                })?
            }
        } else {
            Vec::new()
        };

        Ok(Self {
            path,
            orders: RwLock::new(orders),
        })
    }

    fn persist(&self, orders: &[StandingOrder]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create standing order store directory {}",
                    parent.display()
                )
            })?;
        }
        let bytes =
            serde_json::to_vec_pretty(orders).context("failed to serialize standing orders")?;
        fs::write(&self.path, bytes).with_context(|| {
            format!(
                "failed to write standing order store {}",
                self.path.display()
            )
        })?;
        Ok(())
    }
}

impl StandingOrderStorePort for FileStandingOrderStore {
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
        self.persist(&orders)
    }

    fn remove(&self, id: &str) -> Result<bool> {
        let mut orders = self.orders.write();
        let before = orders.len();
        orders.retain(|order| order.id != id);
        let removed = orders.len() < before;
        if removed {
            self.persist(&orders)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::standing_order::StandingOrderKind;

    fn order(id: &str) -> StandingOrder {
        StandingOrder {
            id: id.into(),
            kind: StandingOrderKind::RestartReport,
            delivery_channel: "matrix".into(),
            delivery_recipient: "!room:example.com".into(),
            delivery_thread: Some("$thread".into()),
            enabled: true,
            created_by: "agent".into(),
            created_at: 1,
        }
    }

    #[test]
    fn persists_orders_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_orders.json");

        let store = FileStandingOrderStore::new(&path).unwrap();
        store.upsert(order("one")).unwrap();
        store.upsert(order("two")).unwrap();

        let reopened = FileStandingOrderStore::new(&path).unwrap();
        let ids: Vec<String> = reopened.list().into_iter().map(|o| o.id).collect();
        assert_eq!(ids, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn remove_updates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("standing_orders.json");

        let store = FileStandingOrderStore::new(&path).unwrap();
        store.upsert(order("one")).unwrap();
        assert!(store.remove("one").unwrap());

        let reopened = FileStandingOrderStore::new(&path).unwrap();
        assert!(reopened.list().is_empty());
    }
}
