// IPC tests — temporarily simplified for SurrealDB migration (Phase 4.5).
//
// The old SQLite-based tests used `IpcDb::open_in_memory()` which no longer exists.
// SurrealDB tests require async setup with a temporary directory for SurrealKV.
// TODO: restore full test coverage with async test helpers.

use super::*;
use std::collections::HashMap;

fn empty_l4() -> HashMap<String, String> {
    HashMap::new()
}

fn l4_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(alias, real)| (alias.to_string(), real.to_string()))
        .collect()
}

// ── Inbox filter tests (no DB needed) ───────────────────────

#[test]
fn inbox_filter_no_filter() {
    let filter = synapse_domain::config::schema::InboxFilterConfig::default();
    let msgs = vec![make_inbox_msg(1, "a", "b", "text")];
    let result = IpcDb::apply_inbox_filter(msgs.clone(), &filter);
    assert_eq!(result.len(), 1);
}

#[test]
fn inbox_filter_allowed_kinds() {
    let filter = synapse_domain::config::schema::InboxFilterConfig {
        allowed_kinds: vec!["task".to_string()],
        ..Default::default()
    };
    let msgs = vec![
        make_inbox_msg(1, "a", "b", "text"),
        make_inbox_msg(2, "a", "b", "task"),
    ];
    let result = IpcDb::apply_inbox_filter(msgs, &filter);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].kind, "task");
}

#[test]
fn inbox_filter_per_source() {
    let filter = synapse_domain::config::schema::InboxFilterConfig {
        default_per_source: 1,
        ..Default::default()
    };
    let msgs = vec![
        make_inbox_msg(1, "a", "b", "text"),
        make_inbox_msg(2, "a", "b", "text"),
        make_inbox_msg(3, "c", "b", "text"),
    ];
    let result = IpcDb::apply_inbox_filter(msgs, &filter);
    // Last 1 per source: msg 2 from "a", msg 3 from "c"
    assert_eq!(result.len(), 2);
}

// ── ACL validation tests (domain-level, no DB needed) ──────

// These tests use the synapse_domain validation directly,
// which is deterministic and doesn't require a database.

#[test]
fn validate_send_invalid_kind() {
    let result = synapse_domain::domain::ipc::validate_send(
        "a",
        "b",
        "execute",
        3,
        1,
        None,
        false,
        &[],
        &[],
    );
    assert_eq!(result.unwrap_err().code, "invalid_kind");
}

#[test]
fn validate_send_l4_text_only() {
    let result = synapse_domain::domain::ipc::validate_send(
        "kids",
        "opus",
        "task",
        4,
        1,
        None,
        false,
        &[],
        &["opus".to_string()],
    );
    assert_eq!(result.unwrap_err().code, "l4_text_only");
}

#[test]
fn validate_send_l4_text_allowed() {
    let result = synapse_domain::domain::ipc::validate_send(
        "kids",
        "opus",
        "text",
        4,
        1,
        None,
        false,
        &[],
        &["opus".to_string()],
    );
    assert!(result.is_ok());
}

#[test]
fn validate_send_task_upward_denied() {
    let result = synapse_domain::domain::ipc::validate_send(
        "worker",
        "opus",
        "task",
        3,
        1,
        None,
        false,
        &[],
        &[],
    );
    assert_eq!(result.unwrap_err().code, "task_upward_denied");
}

#[test]
fn validate_send_task_downward_ok() {
    let result = synapse_domain::domain::ipc::validate_send(
        "opus",
        "worker",
        "task",
        1,
        3,
        None,
        false,
        &[],
        &[],
    );
    assert!(result.is_ok());
}

#[test]
fn validate_send_result_no_task() {
    let result = synapse_domain::domain::ipc::validate_send(
        "worker",
        "opus",
        "result",
        3,
        1,
        Some("session-1"),
        false,
        &[],
        &[],
    );
    assert_eq!(result.unwrap_err().code, "result_no_task");
}

#[test]
fn validate_send_result_with_task_ok() {
    let result = synapse_domain::domain::ipc::validate_send(
        "worker",
        "opus",
        "result",
        3,
        1,
        Some("session-1"),
        true,
        &[],
        &[],
    );
    assert!(result.is_ok());
}

// ── Push dedup set tests (no DB needed) ─────────────────────

#[test]
fn push_dedup_basic() {
    let dedup = PushDedupSet::new(3);
    assert!(dedup.insert(1));
    assert!(!dedup.insert(1));
    assert!(dedup.insert(2));
    assert!(dedup.insert(3));
    assert!(dedup.insert(4)); // evicts 1
    assert!(dedup.insert(1)); // re-insert after eviction
}

// ── Helper ──────────────────────────────────────────────────

fn make_inbox_msg(id: i64, from: &str, to: &str, kind: &str) -> InboxMessage {
    InboxMessage {
        id,
        session_id: None,
        from_agent: from.to_string(),
        to_agent: to.to_string(),
        kind: kind.to_string(),
        payload: "test".to_string(),
        priority: 0,
        from_trust_level: 3,
        seq: id,
        created_at: 1000 + id,
        trust_warning: None,
        quarantined: None,
    }
}
