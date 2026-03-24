//! Use case: DispatchIpcMessage — validate, resolve, and send an IPC message.
//!
//! Phase 4.0: top-level entry point for inter-agent messaging.
//!
//! Orchestrates:
//! 1. Recipient resolution (L4 alias → real agent_id)
//! 2. Session limit check (lateral same-level exchanges)
//! 3. ACL validation + send via ipc_service

use crate::application::services::ipc_service;
use crate::domain::ipc::AclError;
use crate::ports::ipc_bus::IpcBusPort;
use std::collections::HashMap;

/// Parameters for dispatching an IPC message.
pub struct DispatchParams<'a> {
    pub from_agent: &'a str,
    pub to_agent: &'a str,
    pub kind: &'a str,
    pub payload: &'a str,
    pub session_id: Option<&'a str>,
    pub from_trust_level: i32,
    pub priority: i32,
    /// Lateral text allowlist pairs (agent_a, agent_b).
    pub lateral_text_pairs: &'a [(String, String)],
    /// L4 logical destination aliases → real agent_id.
    pub l4_destinations: &'a HashMap<String, String>,
    /// Max lateral exchanges per session (0 = unlimited).
    pub max_session_exchanges: usize,
    /// Current session message count (for limit check).
    pub session_message_count: usize,
}

/// Result of a successful dispatch.
#[derive(Debug)]
pub struct DispatchResult {
    /// Message sequence number from the bus.
    pub seq: i64,
    /// Resolved recipient (may differ from input if L4 alias).
    pub resolved_recipient: String,
}

/// Dispatch an IPC message with full validation pipeline.
///
/// Steps:
/// 1. Resolve recipient (L4 alias → real agent_id)
/// 2. Check session exchange limit (lateral same-level)
/// 3. ACL validate + send via ipc_service
pub async fn execute(
    bus: &dyn IpcBusPort,
    params: &DispatchParams<'_>,
) -> Result<DispatchResult, AclError> {
    // Step 1: Resolve recipient
    let resolved = ipc_service::resolve_recipient(
        params.to_agent,
        params.from_trust_level,
        params.l4_destinations,
    )?;

    // Step 2: Session limit check for lateral exchanges
    if params.max_session_exchanges > 0 {
        // Look up resolved recipient trust level
        let to_trust = bus
            .get_agent_trust_level(&resolved)
            .await
            .ok_or_else(|| AclError {
                code: "agent_not_found".into(),
                message: format!("Recipient agent '{}' not found", resolved),
                retryable: false,
            })?;

        if ipc_service::session_limit_applies(params.from_trust_level, to_trust)
            && ipc_service::check_session_limit(
                params.session_message_count,
                params.max_session_exchanges,
            )
        {
            return Err(AclError {
                code: "session_limit_exceeded".into(),
                message: format!(
                    "Session exchange limit ({}) exceeded for lateral messaging",
                    params.max_session_exchanges
                ),
                retryable: false,
            });
        }
    }

    // Step 3: ACL validate + send
    // L4 destinations: pass resolved agent IDs (values) for ACL check
    let l4_dest_list: Vec<String> = params.l4_destinations.values().cloned().collect();

    let seq = ipc_service::send_message(
        bus,
        params.from_agent,
        &resolved,
        params.kind,
        params.payload,
        params.session_id,
        params.from_trust_level,
        params.priority,
        params.lateral_text_pairs,
        &l4_dest_list,
    )
    .await?;

    Ok(DispatchResult {
        seq,
        resolved_recipient: resolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ipc::IpcMessage;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockBus {
        agents: Vec<(String, i32)>,
        sessions: Vec<(String, String)>,
        sent: Mutex<Vec<String>>,
    }

    impl MockBus {
        fn new(agents: Vec<(String, i32)>) -> Self {
            Self {
                agents,
                sessions: vec![],
                sent: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl IpcBusPort for MockBus {
        async fn send_message(
            &self, _from: &str, to: &str, kind: &str, _payload: &str,
            _sid: Option<&str>, _ftl: i32, _pri: i32,
        ) -> Result<i64> {
            self.sent.lock().unwrap().push(format!("{to}:{kind}"));
            Ok(42)
        }
        async fn fetch_inbox(&self, _agent: &str, _q: bool, _limit: u32) -> Result<Vec<IpcMessage>> {
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

    fn default_params<'a>(
        from: &'a str,
        to: &'a str,
        kind: &'a str,
        l4_dests: &'a HashMap<String, String>,
    ) -> DispatchParams<'a> {
        DispatchParams {
            from_agent: from,
            to_agent: to,
            kind,
            payload: "hello",
            session_id: None,
            from_trust_level: 2,
            priority: 0,
            lateral_text_pairs: &[],
            l4_destinations: l4_dests,
            max_session_exchanges: 0,
            session_message_count: 0,
        }
    }

    #[tokio::test]
    async fn dispatch_text_succeeds() {
        let bus = MockBus::new(vec![("b".into(), 3)]);
        let dests = HashMap::new();
        let params = default_params("a", "b", "text", &dests);

        let result = execute(&bus, &params).await.unwrap();
        assert_eq!(result.seq, 42);
        assert_eq!(result.resolved_recipient, "b");
    }

    #[tokio::test]
    async fn dispatch_l4_alias_resolves() {
        // L4 sends "text" to a lower-level agent via alias — allowed when destination is in l4_destinations
        let bus = MockBus::new(vec![("real-agent".into(), 2)]);
        let mut dests = HashMap::new();
        dests.insert("supervisor".into(), "real-agent".into());

        let mut params = default_params("quarantined", "supervisor", "text", &dests);
        params.from_trust_level = 4;

        let result = execute(&bus, &params).await.unwrap();
        assert_eq!(result.resolved_recipient, "real-agent");
    }

    #[tokio::test]
    async fn dispatch_session_limit_exceeded() {
        let bus = MockBus::new(vec![("b".into(), 2)]);
        let dests = HashMap::new();
        let mut params = default_params("a", "b", "text", &dests);
        params.from_trust_level = 2;
        params.max_session_exchanges = 10;
        params.session_message_count = 15;

        let err = execute(&bus, &params).await.unwrap_err();
        assert_eq!(err.code, "session_limit_exceeded");
    }

    #[tokio::test]
    async fn dispatch_session_limit_not_applied_for_different_levels() {
        let bus = MockBus::new(vec![("b".into(), 3)]);
        let dests = HashMap::new();
        let mut params = default_params("a", "b", "text", &dests);
        params.from_trust_level = 2; // different from b's level 3
        params.max_session_exchanges = 10;
        params.session_message_count = 15;

        // Should succeed — limit only applies to same-level lateral
        let result = execute(&bus, &params).await.unwrap();
        assert_eq!(result.seq, 42);
    }

    #[tokio::test]
    async fn dispatch_unknown_recipient_fails() {
        let bus = MockBus::new(vec![]);
        let dests = HashMap::new();
        let params = default_params("a", "nonexistent", "text", &dests);

        let err = execute(&bus, &params).await.unwrap_err();
        assert_eq!(err.code, "agent_not_found");
    }
}
