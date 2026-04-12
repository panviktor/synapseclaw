//! Port for accessing resolved per-turn defaults during tool execution.
//!
//! Set before a turn starts; read by tools that need typed implicit defaults
//! like delivery targets; cleared after the turn completes.

use crate::domain::turn_defaults::ResolvedTurnDefaults;
use std::collections::HashMap;

pub trait TurnDefaultsContextPort: Send + Sync {
    fn get_current(&self) -> Option<ResolvedTurnDefaults>;

    fn set_current(&self, defaults: Option<ResolvedTurnDefaults>);
}

pub struct InMemoryTurnDefaultsContext {
    by_task: parking_lot::RwLock<HashMap<tokio::task::Id, ResolvedTurnDefaults>>,
    sync_slot: parking_lot::RwLock<Option<ResolvedTurnDefaults>>,
}

impl InMemoryTurnDefaultsContext {
    pub fn new() -> Self {
        Self {
            by_task: parking_lot::RwLock::new(HashMap::new()),
            sync_slot: parking_lot::RwLock::new(None),
        }
    }
}

impl Default for InMemoryTurnDefaultsContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnDefaultsContextPort for InMemoryTurnDefaultsContext {
    fn get_current(&self) -> Option<ResolvedTurnDefaults> {
        if let Some(task_id) = tokio::task::try_id() {
            if let Some(defaults) = self.by_task.read().get(&task_id) {
                return Some(defaults.clone());
            }
        }
        self.sync_slot.read().clone()
    }

    fn set_current(&self, defaults: Option<ResolvedTurnDefaults>) {
        if let Some(task_id) = tokio::task::try_id() {
            let mut by_task = self.by_task.write();
            if let Some(defaults) = defaults {
                by_task.insert(task_id, defaults);
            } else {
                by_task.remove(&task_id);
            }
            return;
        }

        *self.sync_slot.write() = defaults;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::turn_defaults::{ResolvedDeliveryTarget, TurnDefaultSource};

    fn make_defaults(label: &str) -> ResolvedTurnDefaults {
        ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: format!("!{label}:example.com"),
                    thread_ref: None,
                },
                source: TurnDefaultSource::DialogueState,
            }),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn isolates_defaults_per_async_task() {
        let port = std::sync::Arc::new(InMemoryTurnDefaultsContext::new());

        let left = {
            let port = std::sync::Arc::clone(&port);
            tokio::spawn(async move {
                port.set_current(Some(make_defaults("alpha")));
                tokio::task::yield_now().await;
                port.get_current()
                    .and_then(|defaults| defaults.delivery_target)
                    .and_then(|target| match target.target {
                        ConversationDeliveryTarget::Explicit { recipient, .. } => Some(recipient),
                        ConversationDeliveryTarget::CurrentConversation => None,
                    })
            })
        };

        let right = {
            let port = std::sync::Arc::clone(&port);
            tokio::spawn(async move {
                port.set_current(Some(make_defaults("beta")));
                tokio::task::yield_now().await;
                port.get_current()
                    .and_then(|defaults| defaults.delivery_target)
                    .and_then(|target| match target.target {
                        ConversationDeliveryTarget::Explicit { recipient, .. } => Some(recipient),
                        ConversationDeliveryTarget::CurrentConversation => None,
                    })
            })
        };

        assert_eq!(left.await.unwrap().as_deref(), Some("!alpha:example.com"));
        assert_eq!(right.await.unwrap().as_deref(), Some("!beta:example.com"));
    }

    #[test]
    fn sync_defaults_work_outside_tokio() {
        let port = InMemoryTurnDefaultsContext::new();
        port.set_current(Some(make_defaults("sync")));
        let recipient = port
            .get_current()
            .and_then(|defaults| defaults.delivery_target)
            .and_then(|target| match target.target {
                ConversationDeliveryTarget::Explicit { recipient, .. } => Some(recipient),
                ConversationDeliveryTarget::CurrentConversation => None,
            });
        assert_eq!(recipient.as_deref(), Some("!sync:example.com"));
        port.set_current(None);
        assert!(port.get_current().is_none());
    }
}
