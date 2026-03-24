//! IPC domain types and validation — inter-agent messaging.
//!
//! Phase 4.0 Slice 5: first-class IPC objects and ACL rules.

use std::fmt;

// ── Message kinds ────────────────────────────────────────────────

/// Valid message kinds that agents can send.
pub const VALID_KINDS: &[&str] = &["text", "task", "query", "result"];

/// System-generated escalation kind (not agent-sendable).
pub const ESCALATION_KIND: &str = "escalation";

/// Admin-promoted quarantine message kind.
pub const PROMOTED_KIND: &str = "promoted_quarantine";

// ── Domain types ─────────────────────────────────────────────────

/// An IPC message as a domain object.
#[derive(Debug, Clone)]
pub struct IpcMessage {
    pub id: i64,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub payload: String,
    pub session_id: Option<String>,
    pub from_trust_level: i32,
    pub priority: i32,
    pub created_at: i64,
    pub promoted: bool,
    pub read: bool,
    pub blocked: bool,
}

/// Validated send request before insertion.
#[derive(Debug, Clone)]
pub struct ValidatedSend {
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub payload: String,
    pub session_id: Option<String>,
    pub from_trust_level: i32,
    pub to_trust_level: i32,
    pub priority: i32,
}

/// ACL validation error with machine-readable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl fmt::Display for AclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AclError {}

// ── ACL validation rules ─────────────────────────────────────────

/// Validate an IPC send request against the trust/ACL policy.
///
/// Pure business logic — no I/O, no DB. Returns Ok(()) if allowed,
/// Err(AclError) with machine-readable code if denied.
///
/// Rules enforced (in order):
/// 1. Kind must be in VALID_KINDS
/// 2. L4 agents can only send "text"
/// 3. L4 agents can only send to allowed destinations
/// 4. Tasks can only be sent downward (higher trust → lower trust)
/// 5. "result" requires matching session_id with prior task/query
/// 6. No L4↔L4 lateral messaging
/// 7. L3↔L3 lateral "text" requires explicit allowlist
pub fn validate_send(
    from_agent: &str,
    to_agent: &str,
    kind: &str,
    from_trust_level: i32,
    to_trust_level: i32,
    session_id: Option<&str>,
    session_has_request: bool,
    lateral_text_pairs: &[(String, String)],
    l4_destinations: &[String],
) -> Result<(), AclError> {
    // Rule 1: kind whitelist
    if !VALID_KINDS.contains(&kind) {
        return Err(AclError {
            code: "invalid_kind".into(),
            message: format!("Invalid message kind '{kind}'. Valid: {VALID_KINDS:?}"),
            retryable: false,
        });
    }

    // Rule 2: L4 text-only
    if from_trust_level >= 4 && kind != "text" {
        return Err(AclError {
            code: "l4_text_only".into(),
            message: "Quarantined agents (L4) can only send 'text' messages".into(),
            retryable: false,
        });
    }

    // Rule 3: L4 destination check
    if from_trust_level >= 4 && !l4_destinations.contains(&to_agent.to_string()) {
        return Err(AclError {
            code: "l4_destination_denied".into(),
            message: format!("Quarantined agent cannot send to '{to_agent}'"),
            retryable: false,
        });
    }

    // Rule 4: no upward/lateral tasks
    if kind == "task" {
        if to_trust_level < from_trust_level {
            return Err(AclError {
                code: "task_upward_denied".into(),
                message: "Tasks cannot be sent upward (to higher trust level)".into(),
                retryable: false,
            });
        }
        if to_trust_level == from_trust_level {
            return Err(AclError {
                code: "task_lateral_denied".into(),
                message: "Tasks cannot be sent laterally (to same trust level)".into(),
                retryable: false,
            });
        }
    }

    // Rule 5: result requires session correlation
    if kind == "result" {
        if session_id.is_none() {
            return Err(AclError {
                code: "result_no_session".into(),
                message: "Results require a session_id".into(),
                retryable: false,
            });
        }
        if !session_has_request {
            return Err(AclError {
                code: "result_no_task".into(),
                message: "No matching task/query found for this session_id".into(),
                retryable: false,
            });
        }
    }

    // Rule 6: no L4↔L4
    if from_trust_level >= 4 && to_trust_level >= 4 {
        return Err(AclError {
            code: "l4_lateral_denied".into(),
            message: "Quarantined agents cannot message each other".into(),
            retryable: false,
        });
    }

    // Rule 7: L3↔L3 lateral text allowlist
    if from_trust_level == 3
        && to_trust_level == 3
        && kind == "text"
        && !lateral_text_pairs.iter().any(|(a, b)| {
            (a == from_agent && b == to_agent) || (a == to_agent && b == from_agent)
        })
    {
        return Err(AclError {
            code: "l3_lateral_denied".into(),
            message: "L3 lateral text not allowed for this agent pair".into(),
            retryable: false,
        });
    }

    Ok(())
}

/// Validate shared state write access.
///
/// Key format: `{scope}:{owner}:{name}`
/// - `public:*` — any agent can write to their own namespace
/// - `shared:*` — L0-L2 can write
/// - `secret:*` — L0-L1 only
pub fn validate_state_write(trust_level: i32, agent_id: &str, key: &str) -> Result<(), AclError> {
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    if parts.len() < 3 {
        return Err(AclError {
            code: "invalid_key_format".into(),
            message: "State key must be {scope}:{owner}:{name}".into(),
            retryable: false,
        });
    }

    let (scope, owner) = (parts[0], parts[1]);

    // Owner must match agent_id
    if owner != agent_id {
        return Err(AclError {
            code: "state_owner_mismatch".into(),
            message: format!("Cannot write to namespace owned by '{owner}'"),
            retryable: false,
        });
    }

    match scope {
        "public" => Ok(()),
        "shared" => {
            if trust_level > 2 {
                Err(AclError {
                    code: "state_shared_denied".into(),
                    message: "Only L0-L2 agents can write shared state".into(),
                    retryable: false,
                })
            } else {
                Ok(())
            }
        }
        "secret" => {
            if trust_level > 1 {
                Err(AclError {
                    code: "state_secret_denied".into(),
                    message: "Only L0-L1 agents can write secret state".into(),
                    retryable: false,
                })
            } else {
                Ok(())
            }
        }
        _ => Err(AclError {
            code: "invalid_scope".into(),
            message: format!("Unknown scope '{scope}'. Valid: public, shared, secret"),
            retryable: false,
        }),
    }
}

/// Validate shared state read access.
pub fn validate_state_read(trust_level: i32, key: &str) -> Result<(), AclError> {
    if key.starts_with("secret:") && trust_level > 1 {
        return Err(AclError {
            code: "state_secret_read_denied".into(),
            message: "Only L0-L1 agents can read secret state".into(),
            retryable: false,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_send_text() {
        assert!(validate_send("a", "b", "text", 2, 3, None, false, &[], &[]).is_ok());
    }

    #[test]
    fn invalid_kind() {
        let err = validate_send("a", "b", "invalid", 2, 3, None, false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "invalid_kind");
    }

    #[test]
    fn l4_text_only() {
        let err = validate_send("a", "b", "task", 4, 3, None, false, &[], &["b".into()]).unwrap_err();
        assert_eq!(err.code, "l4_text_only");
    }

    #[test]
    fn l4_destination_denied() {
        let err = validate_send("a", "b", "text", 4, 3, None, false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "l4_destination_denied");
    }

    #[test]
    fn l4_allowed_destination() {
        assert!(validate_send("a", "b", "text", 4, 3, None, false, &[], &["b".into()]).is_ok());
    }

    #[test]
    fn task_upward_denied() {
        let err = validate_send("a", "b", "task", 3, 2, None, false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "task_upward_denied");
    }

    #[test]
    fn task_lateral_denied() {
        let err = validate_send("a", "b", "task", 2, 2, None, false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "task_lateral_denied");
    }

    #[test]
    fn task_downward_allowed() {
        assert!(validate_send("a", "b", "task", 1, 3, None, false, &[], &[]).is_ok());
    }

    #[test]
    fn result_no_session() {
        let err = validate_send("a", "b", "result", 3, 1, None, false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "result_no_session");
    }

    #[test]
    fn result_no_matching_task() {
        let err = validate_send("a", "b", "result", 3, 1, Some("s1"), false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "result_no_task");
    }

    #[test]
    fn result_with_matching_task() {
        assert!(validate_send("a", "b", "result", 3, 1, Some("s1"), true, &[], &[]).is_ok());
    }

    #[test]
    fn l4_lateral_denied() {
        let err = validate_send("a", "b", "text", 4, 4, None, false, &[], &["b".into()]).unwrap_err();
        assert_eq!(err.code, "l4_lateral_denied");
    }

    #[test]
    fn l3_lateral_denied_without_allowlist() {
        let err = validate_send("a", "b", "text", 3, 3, None, false, &[], &[]).unwrap_err();
        assert_eq!(err.code, "l3_lateral_denied");
    }

    #[test]
    fn l3_lateral_allowed_with_pair() {
        let pairs = vec![("a".into(), "b".into())];
        assert!(validate_send("a", "b", "text", 3, 3, None, false, &pairs, &[]).is_ok());
    }

    // ── State validation ─────────────────────────────────────────

    #[test]
    fn state_write_own_public() {
        assert!(validate_state_write(3, "agent-a", "public:agent-a:key").is_ok());
    }

    #[test]
    fn state_write_other_namespace() {
        let err = validate_state_write(3, "agent-a", "public:agent-b:key").unwrap_err();
        assert_eq!(err.code, "state_owner_mismatch");
    }

    #[test]
    fn state_write_shared_l2() {
        assert!(validate_state_write(2, "agent-a", "shared:agent-a:key").is_ok());
    }

    #[test]
    fn state_write_shared_l3_denied() {
        let err = validate_state_write(3, "agent-a", "shared:agent-a:key").unwrap_err();
        assert_eq!(err.code, "state_shared_denied");
    }

    #[test]
    fn state_write_secret_l1() {
        assert!(validate_state_write(1, "agent-a", "secret:agent-a:key").is_ok());
    }

    #[test]
    fn state_write_secret_l2_denied() {
        let err = validate_state_write(2, "agent-a", "secret:agent-a:key").unwrap_err();
        assert_eq!(err.code, "state_secret_denied");
    }

    #[test]
    fn state_read_secret_l2_denied() {
        let err = validate_state_read(2, "secret:agent-a:key").unwrap_err();
        assert_eq!(err.code, "state_secret_read_denied");
    }

    #[test]
    fn state_read_public_any_level() {
        assert!(validate_state_read(4, "public:agent-a:key").is_ok());
    }
}
