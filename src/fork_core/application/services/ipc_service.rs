//! IPC service — owns inter-agent messaging policy.
//!
//! Phase 4.0 Slice 5: extracts routing/validation from gateway/ipc.rs.
//!
//! Business rules this service owns:
//! - ACL validation (trust levels, direction rules, lateral allowlists)
//! - State access validation (namespace, scope)
//! - Message routing decisions
//! - Session exchange limits

use crate::fork_core::domain::ipc::{self, AclError, ValidatedSend};
use crate::fork_core::ports::ipc_bus::IpcBusPort;
use anyhow::Result;

/// Validate and send an IPC message through the bus.
///
/// Orchestrates: ACL check → session correlation → send via port.
pub async fn send_message(
    bus: &dyn IpcBusPort,
    from_agent: &str,
    to_agent: &str,
    kind: &str,
    payload: &str,
    session_id: Option<&str>,
    from_trust_level: i32,
    priority: i32,
    lateral_text_pairs: &[(String, String)],
    l4_destinations: &[String],
) -> Result<i64, AclError> {
    // Resolve recipient trust level
    let to_trust_level = bus
        .get_agent_trust_level(to_agent)
        .await
        .ok_or_else(|| AclError {
            code: "agent_not_found".into(),
            message: format!("Recipient agent '{to_agent}' not found"),
            retryable: false,
        })?;

    // Check session correlation for "result" kind
    let session_has_request = if kind == "result" {
        if let Some(sid) = session_id {
            bus.session_has_request(sid, from_agent).await.unwrap_or(false)
        } else {
            false
        }
    } else {
        false // not relevant for non-result kinds
    };

    // ACL validation (pure business logic)
    ipc::validate_send(
        from_agent,
        to_agent,
        kind,
        from_trust_level,
        to_trust_level,
        session_id,
        session_has_request,
        lateral_text_pairs,
        l4_destinations,
    )?;

    // Send via port
    bus.send_message(
        from_agent,
        to_agent,
        kind,
        payload,
        session_id,
        from_trust_level,
        priority,
    )
    .await
    .map_err(|e| AclError {
        code: "send_failed".into(),
        message: e.to_string(),
        retryable: true,
    })
}

/// Check session exchange limit for lateral same-level messaging.
///
/// Returns true if limit is exceeded.
pub fn check_session_limit(
    message_count: usize,
    max_exchanges: usize,
) -> bool {
    message_count >= max_exchanges
}

/// Check if session limit applies (same-level lateral exchange, L2+).
pub fn session_limit_applies(from_trust: i32, to_trust: i32) -> bool {
    from_trust == to_trust && from_trust >= 2
}

// ── Recipient resolution ─────────────────────────────────────────

/// Resolve recipient for L4 agents (alias → real agent_id).
///
/// L4 agents use logical aliases (e.g. "supervisor", "escalation").
/// L0-L3 agents use real agent_id directly.
pub fn resolve_recipient(
    to: &str,
    from_trust_level: i32,
    l4_destinations: &std::collections::HashMap<String, String>,
) -> Result<String, AclError> {
    if from_trust_level >= 4 {
        l4_destinations
            .get(to)
            .cloned()
            .ok_or_else(|| AclError {
                code: "unknown_recipient".into(),
                message: format!("Unknown destination alias '{to}'"),
                retryable: false,
            })
    } else {
        Ok(to.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fork_core::domain::ipc::IpcMessage;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockBus {
        agents: Vec<(String, i32)>, // (agent_id, trust_level)
        sessions: Vec<(String, String)>, // (session_id, from_agent)
        sent: Mutex<Vec<String>>,
    }

    impl MockBus {
        fn new(agents: Vec<(String, i32)>, sessions: Vec<(String, String)>) -> Self {
            Self { agents, sessions, sent: Mutex::new(vec![]) }
        }
    }

    #[async_trait]
    impl IpcBusPort for MockBus {
        async fn send_message(&self, _from: &str, to: &str, kind: &str, _payload: &str, _sid: Option<&str>, _ftl: i32, _pri: i32) -> Result<i64> {
            self.sent.lock().unwrap().push(format!("{to}:{kind}"));
            Ok(1)
        }
        async fn fetch_inbox(&self, _agent: &str, _quarantine: bool, _limit: u32) -> Result<Vec<IpcMessage>> {
            Ok(vec![])
        }
        async fn ack_messages(&self, _agent: &str, _ids: &[i64]) -> Result<u64> { Ok(0) }
        async fn session_has_request(&self, sid: &str, from: &str) -> Result<bool> {
            Ok(self.sessions.iter().any(|(s, f)| s == sid && f == from))
        }
        async fn get_agent_trust_level(&self, agent_id: &str) -> Option<i32> {
            self.agents.iter().find(|(a, _)| a == agent_id).map(|(_, l)| *l)
        }
    }

    #[tokio::test]
    async fn send_text_succeeds() {
        let bus = MockBus::new(vec![("b".into(), 3)], vec![]);
        let seq = send_message(&bus, "a", "b", "text", "hello", None, 2, 0, &[], &[]).await.unwrap();
        assert_eq!(seq, 1);
    }

    #[tokio::test]
    async fn send_to_unknown_agent_fails() {
        let bus = MockBus::new(vec![], vec![]);
        let err = send_message(&bus, "a", "unknown", "text", "hello", None, 2, 0, &[], &[]).await.unwrap_err();
        assert_eq!(err.code, "agent_not_found");
    }

    #[tokio::test]
    async fn send_task_downward_succeeds() {
        let bus = MockBus::new(vec![("worker".into(), 3)], vec![]);
        let seq = send_message(&bus, "lead", "worker", "task", "do this", None, 1, 0, &[], &[]).await.unwrap();
        assert_eq!(seq, 1);
    }

    #[tokio::test]
    async fn send_task_upward_denied() {
        let bus = MockBus::new(vec![("lead".into(), 1)], vec![]);
        let err = send_message(&bus, "worker", "lead", "task", "do this", None, 3, 0, &[], &[]).await.unwrap_err();
        assert_eq!(err.code, "task_upward_denied");
    }

    #[tokio::test]
    async fn send_result_with_session() {
        let bus = MockBus::new(
            vec![("lead".into(), 1)],
            vec![("s1".into(), "worker".into())],
        );
        let seq = send_message(&bus, "worker", "lead", "result", "done", Some("s1"), 3, 0, &[], &[]).await.unwrap();
        assert_eq!(seq, 1);
    }

    #[test]
    fn session_limit_check() {
        assert!(!check_session_limit(5, 10));
        assert!(check_session_limit(10, 10));
        assert!(check_session_limit(15, 10));
    }

    #[test]
    fn session_limit_applies_same_level_l2_plus() {
        assert!(session_limit_applies(2, 2));
        assert!(session_limit_applies(3, 3));
        assert!(!session_limit_applies(1, 1)); // L1 exempt
        assert!(!session_limit_applies(2, 3)); // different levels
    }

    #[test]
    fn resolve_recipient_l4_alias() {
        let mut dests = std::collections::HashMap::new();
        dests.insert("supervisor".into(), "marketing-lead".into());
        let resolved = resolve_recipient("supervisor", 4, &dests).unwrap();
        assert_eq!(resolved, "marketing-lead");
    }

    #[test]
    fn resolve_recipient_l4_unknown_alias() {
        let dests = std::collections::HashMap::new();
        let err = resolve_recipient("unknown", 4, &dests).unwrap_err();
        assert_eq!(err.code, "unknown_recipient");
    }

    #[test]
    fn resolve_recipient_non_l4_passthrough() {
        let dests = std::collections::HashMap::new();
        let resolved = resolve_recipient("agent-b", 2, &dests).unwrap();
        assert_eq!(resolved, "agent-b");
    }
}
