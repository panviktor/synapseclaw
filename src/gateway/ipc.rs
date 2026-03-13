//! IPC broker handlers for inter-agent communication.
//!
//! All IPC communication is broker-mediated: agents authenticate with bearer
//! tokens, and the broker resolves trust levels from token metadata. The broker
//! owns the SQLite database — agents never access it directly.

use super::AppState;
use crate::config::TokenMetadata;
use crate::gateway::api::extract_bearer_token;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
// tracing::{info, warn} will be used in Steps 5-7 when handlers are fleshed out

// ── IpcDb (broker-owned SQLite) ─────────────────────────────────

/// Broker-owned SQLite database for IPC messages, agent registry, and shared state.
///
/// Initialized when `agents_ipc.enabled = true`. The database is WAL-mode
/// and only accessible by the broker process.
pub struct IpcDb {
    conn: Arc<Mutex<Connection>>,
}

impl IpcDb {
    /// Open (or create) the IPC database at the given path.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS agents (
                agent_id    TEXT PRIMARY KEY,
                role        TEXT,
                trust_level INTEGER NOT NULL DEFAULT 3,
                status      TEXT DEFAULT 'online',
                metadata    TEXT,
                last_seen   INTEGER
            );

            CREATE TABLE IF NOT EXISTS messages (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id       TEXT,
                from_agent       TEXT NOT NULL,
                to_agent         TEXT NOT NULL,
                kind             TEXT NOT NULL DEFAULT 'text',
                payload          TEXT NOT NULL,
                priority         INTEGER DEFAULT 0,
                from_trust_level INTEGER NOT NULL,
                seq              INTEGER NOT NULL,
                blocked          INTEGER DEFAULT 0,
                block_reason     TEXT,
                created_at       INTEGER NOT NULL,
                read             INTEGER DEFAULT 0,
                expires_at       INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_messages_inbox
                ON messages(to_agent, read, created_at);
            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id) WHERE session_id IS NOT NULL;

            CREATE TABLE IF NOT EXISTS shared_state (
                key        TEXT PRIMARY KEY,
                value      TEXT NOT NULL,
                owner      TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS message_sequences (
                agent_id TEXT PRIMARY KEY,
                last_seq INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
    }

    /// Upsert agent record and update `last_seen` timestamp.
    pub fn update_last_seen(&self, agent_id: &str, trust_level: u8, role: &str) {
        let now = unix_now();
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT INTO agents (agent_id, trust_level, role, last_seen)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(agent_id) DO UPDATE SET
                trust_level = ?2, role = ?3, last_seen = ?4, status = 'online'",
            params![agent_id, trust_level, role, now],
        );
    }

    /// Check whether a session contains a task directed at the given agent.
    pub fn session_has_task_for(&self, session_id: &str, agent_id: &str) -> bool {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE session_id = ?1 AND to_agent = ?2 AND kind = 'task' AND blocked = 0",
            params![session_id, agent_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            > 0
    }

    /// Allocate the next monotonic sequence number for a sender.
    pub fn next_seq(&self, agent_id: &str) -> i64 {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO message_sequences (agent_id, last_seq) VALUES (?1, 1)
             ON CONFLICT(agent_id) DO UPDATE SET last_seq = last_seq + 1",
            params![agent_id],
        )
        .ok();
        conn.query_row(
            "SELECT last_seq FROM message_sequences WHERE agent_id = ?1",
            params![agent_id],
            |row| row.get(0),
        )
        .unwrap_or(1)
    }
}

// ── ACL validation ──────────────────────────────────────────────

/// Allowed message kinds.
const VALID_KINDS: &[&str] = &["text", "task", "result", "query", "notify"];

/// Validate whether a send operation is permitted by the ACL rules.
///
/// Rules:
/// 0. Kind must be in the whitelist.
/// 1. L4 agents can only send `text`.
/// 2. `task` cannot be sent upward (to lower trust_level number = higher trust).
/// 3. `result` requires a correlated task in the same session.
/// 4. L4↔L4 direct messaging is denied (must go through a higher-trust agent).
/// 5. L3 lateral `text` requires an explicit allowlist entry.
pub fn validate_send(
    from_level: u8,
    to_level: u8,
    kind: &str,
    from_agent: &str,
    to_agent: &str,
    session_id: Option<&str>,
    lateral_text_pairs: &[[String; 2]],
    l4_destinations: &[String],
    db: &IpcDb,
) -> Result<(), IpcError> {
    // Rule 0: kind whitelist
    if !VALID_KINDS.contains(&kind) {
        return Err(IpcError {
            status: StatusCode::BAD_REQUEST,
            error: format!("Invalid message kind: {kind}"),
            code: "invalid_kind".into(),
            retryable: false,
        });
    }

    // Rule 1: L4 can only send text
    if from_level >= 4 && kind != "text" {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "Restricted agents can only send text".into(),
            code: "l4_text_only".into(),
            retryable: false,
        });
    }

    // L4 destination whitelist
    if from_level >= 4 && !l4_destinations.contains(&to_agent.to_string()) {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "Destination not in L4 allowlist".into(),
            code: "l4_destination_denied".into(),
            retryable: false,
        });
    }

    // Rule 2: task cannot be sent upward
    if kind == "task" && to_level < from_level {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "Cannot assign tasks to higher-trust agents".into(),
            code: "task_upward_denied".into(),
            retryable: false,
        });
    }

    // Rule 2b: task cannot be sent to same level
    if kind == "task" && to_level == from_level {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "Cannot assign tasks to same-trust agents".into(),
            code: "task_lateral_denied".into(),
            retryable: false,
        });
    }

    // Rule 3: result requires correlated task
    if kind == "result" {
        match session_id {
            Some(sid) if db.session_has_task_for(sid, from_agent) => {}
            _ => {
                return Err(IpcError {
                    status: StatusCode::FORBIDDEN,
                    error: "Result requires a correlated task in the same session".into(),
                    code: "result_no_task".into(),
                    retryable: false,
                });
            }
        }
    }

    // Rule 4: L4↔L4 denied
    if from_level >= 4 && to_level >= 4 {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "L4 agents cannot message each other directly".into(),
            code: "l4_lateral_denied".into(),
            retryable: false,
        });
    }

    // Rule 5: L3 lateral text requires allowlist
    if from_level == 3 && to_level == 3 && kind == "text" {
        let pair_allowed = lateral_text_pairs.iter().any(|pair| {
            (pair[0] == from_agent && pair[1] == to_agent)
                || (pair[0] == to_agent && pair[1] == from_agent)
        });
        if !pair_allowed {
            return Err(IpcError {
                status: StatusCode::FORBIDDEN,
                error: "L3 lateral text requires allowlist entry".into(),
                code: "l3_lateral_denied".into(),
                retryable: false,
            });
        }
    }

    Ok(())
}

/// Validate whether a state write is permitted.
///
/// Key format: `{scope}:{owner}:{key}`
/// - L4: only `agent:{self}:*`
/// - L3: + `public:*`
/// - L2: + `team:*`
/// - L1: + `global:*`
/// - `secret:*` denied for all (reserved for Phase 2)
pub fn validate_state_set(trust_level: u8, agent_id: &str, key: &str) -> Result<(), IpcError> {
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    if parts.len() < 2 {
        return Err(IpcError {
            status: StatusCode::BAD_REQUEST,
            error: "Key must be in format scope:owner:key".into(),
            code: "invalid_key_format".into(),
            retryable: false,
        });
    }

    let scope = parts[0];

    if scope == "secret" {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "Secret namespace is reserved".into(),
            code: "secret_denied".into(),
            retryable: false,
        });
    }

    match scope {
        "agent" => {
            let owner = parts.get(1).unwrap_or(&"");
            if *owner != agent_id {
                return Err(IpcError {
                    status: StatusCode::FORBIDDEN,
                    error: "Can only write to own agent namespace".into(),
                    code: "agent_namespace_denied".into(),
                    retryable: false,
                });
            }
        }
        "public" => {
            if trust_level > 3 {
                return Err(IpcError {
                    status: StatusCode::FORBIDDEN,
                    error: "L4 agents cannot write to public namespace".into(),
                    code: "public_denied".into(),
                    retryable: false,
                });
            }
        }
        "team" => {
            if trust_level > 2 {
                return Err(IpcError {
                    status: StatusCode::FORBIDDEN,
                    error: "Only L1-L2 can write to team namespace".into(),
                    code: "team_denied".into(),
                    retryable: false,
                });
            }
        }
        "global" => {
            if trust_level > 1 {
                return Err(IpcError {
                    status: StatusCode::FORBIDDEN,
                    error: "Only L1 can write to global namespace".into(),
                    code: "global_denied".into(),
                    retryable: false,
                });
            }
        }
        _ => {
            return Err(IpcError {
                status: StatusCode::BAD_REQUEST,
                error: format!("Unknown scope: {scope}"),
                code: "unknown_scope".into(),
                retryable: false,
            });
        }
    }

    Ok(())
}

/// Validate whether a state read is permitted.
/// All agents can read all keys except `secret:*` (L0-L1 only).
pub fn validate_state_get(trust_level: u8, key: &str) -> Result<(), IpcError> {
    if key.starts_with("secret:") && trust_level > 1 {
        return Err(IpcError {
            status: StatusCode::FORBIDDEN,
            error: "Secret namespace requires L0-L1".into(),
            code: "secret_read_denied".into(),
            retryable: false,
        });
    }
    Ok(())
}

// ── Auth helper ─────────────────────────────────────────────────

/// Extract and verify bearer token, returning the agent's metadata.
fn require_ipc_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<TokenMetadata, (StatusCode, Json<serde_json::Value>)> {
    let token = extract_bearer_token(headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Missing Authorization header",
                "code": "missing_auth"
            })),
        )
    })?;

    state.pairing.authenticate(token).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Invalid or unknown token",
                "code": "invalid_token"
            })),
        )
    })
}

// ── Structured error ────────────────────────────────────────────

/// Structured IPC error with machine-readable code and retryable hint.
#[derive(Debug, Clone, Serialize)]
pub struct IpcError {
    #[serde(skip)]
    pub status: StatusCode,
    pub error: String,
    pub code: String,
    pub retryable: bool,
}

impl IpcError {
    fn into_response_pair(self, caller_trust: u8) -> (StatusCode, Json<serde_json::Value>) {
        let mut body = serde_json::json!({
            "error": if caller_trust <= 2 { &self.error } else { "Forbidden" },
            "code": self.code,
            "retryable": self.retryable,
        });
        // Only L1-L2 get detailed error messages
        if caller_trust > 2 {
            body.as_object_mut().unwrap().remove("error").ok_or(()).ok();
            body["error"] = serde_json::json!("Forbidden");
        }
        (self.status, Json(body))
    }
}

// ── IPC endpoint handlers (stubs) ───────────────────────────────

/// GET /api/ipc/agents — list known agents with their status and trust level.
pub async fn handle_ipc_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = require_ipc_db(&state)?;
    let meta = require_ipc_auth(&state, &headers)?;
    db.update_last_seen(&meta.agent_id, meta.trust_level, &meta.role);
    Ok(Json(serde_json::json!({ "agents": [] })))
}

/// POST /api/ipc/send — send a message to another agent via the broker.
pub async fn handle_ipc_send(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = require_ipc_db(&state)?;
    let meta = require_ipc_auth(&state, &headers)?;
    db.update_last_seen(&meta.agent_id, meta.trust_level, &meta.role);
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/ipc/inbox — retrieve messages for the authenticated agent.
pub async fn handle_ipc_inbox(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = require_ipc_db(&state)?;
    let meta = require_ipc_auth(&state, &headers)?;
    db.update_last_seen(&meta.agent_id, meta.trust_level, &meta.role);
    Ok(Json(serde_json::json!({ "messages": [] })))
}

/// GET /api/ipc/state — read a shared state key.
pub async fn handle_ipc_state_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = require_ipc_db(&state)?;
    let meta = require_ipc_auth(&state, &headers)?;
    db.update_last_seen(&meta.agent_id, meta.trust_level, &meta.role);
    Ok(Json(serde_json::json!({ "value": null })))
}

/// POST /api/ipc/state — write a shared state key.
pub async fn handle_ipc_state_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(_body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let db = require_ipc_db(&state)?;
    let meta = require_ipc_auth(&state, &headers)?;
    db.update_last_seen(&meta.agent_id, meta.trust_level, &meta.role);
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── IPC admin endpoint handlers (stubs) ─────────────────────────

/// GET /admin/ipc/agents — full agent list with metadata (localhost only).
pub async fn handle_admin_ipc_agents(
    State(state): State<AppState>,
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_ipc_db(&state)?;
    Ok(Json(serde_json::json!({ "agents": [] })))
}

/// POST /admin/ipc/revoke — revoke an agent's token (localhost only).
pub async fn handle_admin_ipc_revoke(
    State(state): State<AppState>,
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_ipc_db(&state)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /admin/ipc/disable — disable an agent without revoking its token (localhost only).
pub async fn handle_admin_ipc_disable(
    State(state): State<AppState>,
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_ipc_db(&state)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /admin/ipc/quarantine — quarantine an agent (localhost only).
pub async fn handle_admin_ipc_quarantine(
    State(state): State<AppState>,
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_ipc_db(&state)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /admin/ipc/downgrade — downgrade an agent's trust level (localhost only).
pub async fn handle_admin_ipc_downgrade(
    State(state): State<AppState>,
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_ipc_db(&state)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Helpers ─────────────────────────────────────────────────────

fn require_ipc_db(state: &AppState) -> Result<&Arc<IpcDb>, (StatusCode, Json<serde_json::Value>)> {
    state.ipc_db.as_ref().ok_or_else(ipc_disabled_error)
}

fn ipc_disabled_error() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "IPC is not enabled",
            "code": "ipc_disabled"
        })),
    )
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> IpcDb {
        IpcDb::open_in_memory().expect("in-memory DB")
    }

    // ── validate_send tests ─────────────────────────────────────

    #[test]
    fn validate_send_invalid_kind() {
        let db = test_db();
        let result = validate_send(3, 1, "execute", "a", "b", None, &[], &[], &db);
        assert_eq!(result.unwrap_err().code, "invalid_kind");
    }

    #[test]
    fn validate_send_l4_text_only() {
        let db = test_db();
        let l4_dests = vec!["opus".to_string()];
        let result = validate_send(4, 1, "task", "kids", "opus", None, &[], &l4_dests, &db);
        assert_eq!(result.unwrap_err().code, "l4_text_only");
    }

    #[test]
    fn validate_send_l4_text_allowed() {
        let db = test_db();
        let l4_dests = vec!["opus".to_string()];
        let result = validate_send(4, 1, "text", "kids", "opus", None, &[], &l4_dests, &db);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_send_l4_destination_denied() {
        let db = test_db();
        let result = validate_send(4, 1, "text", "kids", "opus", None, &[], &[], &db);
        assert_eq!(result.unwrap_err().code, "l4_destination_denied");
    }

    #[test]
    fn validate_send_task_upward_denied() {
        let db = test_db();
        let result = validate_send(3, 1, "task", "worker", "opus", None, &[], &[], &db);
        assert_eq!(result.unwrap_err().code, "task_upward_denied");
    }

    #[test]
    fn validate_send_task_lateral_denied() {
        let db = test_db();
        let result = validate_send(2, 2, "task", "a", "b", None, &[], &[], &db);
        assert_eq!(result.unwrap_err().code, "task_lateral_denied");
    }

    #[test]
    fn validate_send_task_downward_ok() {
        let db = test_db();
        let result = validate_send(1, 3, "task", "opus", "worker", None, &[], &[], &db);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_send_result_no_task() {
        let db = test_db();
        let result = validate_send(
            3,
            1,
            "result",
            "worker",
            "opus",
            Some("session-1"),
            &[],
            &[],
            &db,
        );
        assert_eq!(result.unwrap_err().code, "result_no_task");
    }

    #[test]
    fn validate_send_result_without_session() {
        let db = test_db();
        let result = validate_send(3, 1, "result", "worker", "opus", None, &[], &[], &db);
        assert_eq!(result.unwrap_err().code, "result_no_task");
    }

    #[test]
    fn validate_send_l4_lateral_denied() {
        let db = test_db();
        let l4_dests = vec!["other_kid".to_string()];
        let result = validate_send(4, 4, "text", "kids", "other_kid", None, &[], &l4_dests, &db);
        assert_eq!(result.unwrap_err().code, "l4_lateral_denied");
    }

    #[test]
    fn validate_send_l3_lateral_text_denied() {
        let db = test_db();
        let result = validate_send(3, 3, "text", "agent_a", "agent_b", None, &[], &[], &db);
        assert_eq!(result.unwrap_err().code, "l3_lateral_denied");
    }

    #[test]
    fn validate_send_l3_lateral_text_allowed() {
        let db = test_db();
        let pairs = vec![["agent_a".to_string(), "agent_b".to_string()]];
        let result = validate_send(3, 3, "text", "agent_a", "agent_b", None, &pairs, &[], &db);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_send_l3_lateral_text_reverse() {
        let db = test_db();
        let pairs = vec![["agent_b".to_string(), "agent_a".to_string()]];
        let result = validate_send(3, 3, "text", "agent_a", "agent_b", None, &pairs, &[], &db);
        assert!(result.is_ok());
    }

    // ── validate_state_set tests ────────────────────────────────

    #[test]
    fn state_set_l4_own_namespace() {
        assert!(validate_state_set(4, "kids", "agent:kids:mood").is_ok());
    }

    #[test]
    fn state_set_l4_other_namespace_denied() {
        assert_eq!(
            validate_state_set(4, "kids", "agent:opus:x")
                .unwrap_err()
                .code,
            "agent_namespace_denied"
        );
    }

    #[test]
    fn state_set_l4_public_denied() {
        assert_eq!(
            validate_state_set(4, "kids", "public:status")
                .unwrap_err()
                .code,
            "public_denied"
        );
    }

    #[test]
    fn state_set_l3_public_ok() {
        assert!(validate_state_set(3, "worker", "public:status").is_ok());
    }

    #[test]
    fn state_set_l3_team_denied() {
        assert_eq!(
            validate_state_set(3, "worker", "team:config")
                .unwrap_err()
                .code,
            "team_denied"
        );
    }

    #[test]
    fn state_set_l2_team_ok() {
        assert!(validate_state_set(2, "sentinel", "team:config").is_ok());
    }

    #[test]
    fn state_set_l2_global_denied() {
        assert_eq!(
            validate_state_set(2, "sentinel", "global:flag")
                .unwrap_err()
                .code,
            "global_denied"
        );
    }

    #[test]
    fn state_set_l1_global_ok() {
        assert!(validate_state_set(1, "opus", "global:flag").is_ok());
    }

    #[test]
    fn state_set_secret_denied() {
        assert_eq!(
            validate_state_set(1, "opus", "secret:key")
                .unwrap_err()
                .code,
            "secret_denied"
        );
    }

    #[test]
    fn state_set_invalid_format() {
        assert_eq!(
            validate_state_set(1, "opus", "nocolon").unwrap_err().code,
            "invalid_key_format"
        );
    }

    // ── validate_state_get tests ────────────────────────────────

    #[test]
    fn state_get_public_all_levels() {
        for level in 0..=4 {
            assert!(validate_state_get(level, "public:status").is_ok());
        }
    }

    #[test]
    fn state_get_secret_l1_ok() {
        assert!(validate_state_get(1, "secret:api_key").is_ok());
    }

    #[test]
    fn state_get_secret_l2_denied() {
        assert_eq!(
            validate_state_get(2, "secret:api_key").unwrap_err().code,
            "secret_read_denied"
        );
    }

    // ── IpcDb tests ─────────────────────────────────────────────

    #[test]
    fn session_has_task_for_false() {
        let db = test_db();
        assert!(!db.session_has_task_for("s1", "worker"));
    }

    #[test]
    fn session_has_task_for_true() {
        let db = test_db();
        let conn = db.conn.lock();
        conn.execute(
            "INSERT INTO messages (session_id, from_agent, to_agent, kind, payload,
             from_trust_level, seq, created_at)
             VALUES ('s1', 'opus', 'worker', 'task', 'do work', 1, 1, 100)",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(db.session_has_task_for("s1", "worker"));
    }

    #[test]
    fn session_has_task_for_blocked_ignored() {
        let db = test_db();
        let conn = db.conn.lock();
        conn.execute(
            "INSERT INTO messages (session_id, from_agent, to_agent, kind, payload,
             from_trust_level, seq, created_at, blocked)
             VALUES ('s1', 'opus', 'worker', 'task', 'do work', 1, 1, 100, 1)",
            [],
        )
        .unwrap();
        drop(conn);
        assert!(!db.session_has_task_for("s1", "worker"));
    }

    #[test]
    fn next_seq_monotonic() {
        let db = test_db();
        assert_eq!(db.next_seq("agent_a"), 1);
        assert_eq!(db.next_seq("agent_a"), 2);
        assert_eq!(db.next_seq("agent_a"), 3);
        assert_eq!(db.next_seq("agent_b"), 1);
    }

    #[test]
    fn update_last_seen_upsert() {
        let db = test_db();
        db.update_last_seen("opus", 1, "coordinator");
        db.update_last_seen("opus", 1, "coordinator");
        let conn = db.conn.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agents WHERE agent_id = 'opus'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
