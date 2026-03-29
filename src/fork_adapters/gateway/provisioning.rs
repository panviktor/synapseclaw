//! UI agent provisioning handlers (Phase 3.8 Step 11).
//!
//! Broker-only, disabled by default, mode-gated, arm-required.
//! Creates agent config dirs and optionally installs OS services.

use super::{require_localhost, AppState};
use crate::fork_adapters::gateway::api::extract_bearer_token;
use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use fork_security::audit::{AuditEvent, AuditEventType};
use parking_lot::Mutex;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Runtime provisioning state (in-memory, not persisted).
pub struct ProvisioningState {
    armed_until: Mutex<Option<Instant>>,
}

impl ProvisioningState {
    pub fn new() -> Self {
        Self {
            armed_until: Mutex::new(None),
        }
    }

    pub fn is_armed(&self) -> bool {
        self.armed_until
            .lock()
            .map_or(false, |deadline| Instant::now() < deadline)
    }

    pub fn arm(&self, minutes: u32) {
        let deadline = Instant::now() + std::time::Duration::from_secs(u64::from(minutes) * 60);
        *self.armed_until.lock() = Some(deadline);
    }

    pub fn disarm(&self) {
        *self.armed_until.lock() = None;
    }

    pub fn remaining_secs(&self) -> u64 {
        self.armed_until.lock().map_or(0, |deadline| {
            deadline
                .checked_duration_since(Instant::now())
                .map_or(0, |d| d.as_secs())
        })
    }
}

// ── Auth helper ─────────────────────────────────────────────────

/// Require human operator token (no TokenMetadata, or trust_level ≤ 1).
/// Agents (L2+) are rejected even from localhost.
fn require_admin_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let token = extract_bearer_token(headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Missing Authorization header"})),
        )
    })?;

    if !state.pairing.is_authenticated(token) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid token"})),
        ));
    }

    // Check token metadata — reject agent tokens (L2+)
    let token_hash = fork_security::pairing::hash_token(token);
    let config = state.config.lock();
    if let Some(meta) = config.gateway.token_metadata.get(&token_hash) {
        if meta.trust_level > 1 {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "Agent tokens cannot access provisioning. Human operator required."
                })),
            ));
        }
    }
    // No metadata = human token, or L0/L1 = allowed
    Ok(())
}

/// Check that provisioning is enabled and armed.
fn require_provisioning_active(
    state: &AppState,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let config = state.config.lock();
    if !config.gateway.ui_provisioning.enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "UI provisioning is disabled"})),
        ));
    }
    drop(config);

    if !state.provisioning_state.is_armed() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "Provisioning not armed. POST /admin/provisioning/arm first."
            })),
        ));
    }
    Ok(())
}

/// Validate instance name: ^[a-z0-9][a-z0-9_-]{0,30}$
fn validate_instance_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 31 {
        return Err("Instance name must be 1-31 characters".into());
    }
    if !name.chars().next().unwrap_or(' ').is_ascii_alphanumeric() {
        return Err("Instance name must start with a letter or digit".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err("Instance name must contain only [a-z0-9_-]".into());
    }
    Ok(())
}

/// Resolve agents_root, expanding ~ to home dir.
fn resolve_agents_root(raw: &str) -> PathBuf {
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return home.join(stripped);
        }
    }
    PathBuf::from(raw)
}

fn log_audit(state: &AppState, event_type: AuditEventType, detail: &str) {
    if let Some(ref logger) = state.audit_logger {
        let event = AuditEvent::ipc(event_type, "system", None, detail);
        let _ = logger.log(&event);
    }
}

// ── Handlers ────────────────────────────────────────────────────

/// POST /admin/provisioning/arm — arm provisioning for N minutes.
pub async fn handle_provisioning_arm(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;

    let config = state.config.lock();
    if !config.gateway.ui_provisioning.enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "UI provisioning is disabled in config"})),
        ));
    }
    let mode = config.gateway.ui_provisioning.mode.clone();
    drop(config);

    let minutes = body["minutes"].as_u64().unwrap_or(30).min(120) as u32;
    state.provisioning_state.arm(minutes);

    log_audit(
        &state,
        AuditEventType::ProvisioningArmed,
        &format!("Armed for {minutes}min, mode={mode}"),
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "armed": true,
        "minutes": minutes,
        "mode": mode,
    })))
}

/// GET /admin/provisioning/status — check arming state.
pub async fn handle_provisioning_status(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;

    let config = state.config.lock();
    let enabled = config.gateway.ui_provisioning.enabled;
    let mode = config.gateway.ui_provisioning.mode.clone();
    drop(config);

    Ok(Json(serde_json::json!({
        "enabled": enabled,
        "armed": state.provisioning_state.is_armed(),
        "remaining_secs": state.provisioning_state.remaining_secs(),
        "mode": mode,
    })))
}

/// POST /admin/provisioning/create — create agent dir + write config.toml.
pub async fn handle_provisioning_create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;
    require_provisioning_active(&state)?;

    let instance = body["instance"]
        .as_str()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'instance'"})),
            )
        })?
        .to_string();

    validate_instance_name(&instance).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    let config_toml = body["config_toml"].as_str().unwrap_or("").to_string();
    let instructions_md = body["instructions_md"].as_str().map(String::from);

    if config_toml.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "missing 'config_toml'"})),
        ));
    }

    let agents_root = {
        let config = state.config.lock();
        resolve_agents_root(&config.gateway.ui_provisioning.agents_root)
    };

    let instance_dir = agents_root.join(&instance);
    let config_path = instance_dir.join("config.toml");
    let workspace_dir = instance_dir.join("workspace");

    // Create directories
    if let Err(e) = std::fs::create_dir_all(&workspace_dir) {
        log_audit(
            &state,
            AuditEventType::ProvisioningFailed,
            &format!("Failed to create dir for {instance}: {e}"),
        );
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to create directory: {e}")})),
        ));
    }

    // Write config.toml
    if let Err(e) = std::fs::write(&config_path, &config_toml) {
        log_audit(
            &state,
            AuditEventType::ProvisioningFailed,
            &format!("Failed to write config for {instance}: {e}"),
        );
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to write config: {e}")})),
        ));
    }

    // Set restrictive permissions on config (contains secrets)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    // Write instructions.md if provided
    if let Some(instructions) = instructions_md {
        let instructions_path = workspace_dir.join("instructions.md");
        let _ = std::fs::write(&instructions_path, &instructions);
    }

    log_audit(
        &state,
        AuditEventType::ProvisioningAgentCreated,
        &format!("instance={instance}, path={}", config_path.display()),
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "instance": instance,
        "config_path": config_path.display().to_string(),
    })))
}

/// POST /admin/provisioning/install — install OS service for agent instance.
pub async fn handle_provisioning_install(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;
    require_provisioning_active(&state)?;

    // Check mode allows service_install
    {
        let config = state.config.lock();
        if config.gateway.ui_provisioning.mode != "service_install" {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "Mode is 'config_only'. Set mode='service_install' in config to enable."
                })),
            ));
        }
    }

    let instance = body["instance"]
        .as_str()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'instance'"})),
            )
        })?
        .to_string();

    validate_instance_name(&instance).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    // Run: synapseclaw service --instance <name> install
    // Note: --instance must precede the subcommand (clap ordering)
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("synapseclaw"));
    let output = std::process::Command::new(&exe)
        .args(["service", "--instance", &instance, "install"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            log_audit(
                &state,
                AuditEventType::ProvisioningServiceInstalled,
                &format!("instance={instance}"),
            );
            Ok(Json(serde_json::json!({
                "ok": true,
                "instance": instance,
                "stdout": String::from_utf8_lossy(&out.stdout).to_string(),
            })))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            log_audit(
                &state,
                AuditEventType::ProvisioningFailed,
                &format!("install failed for {instance}: {stderr}"),
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Service install failed: {stderr}")})),
            ))
        }
        Err(e) => {
            log_audit(
                &state,
                AuditEventType::ProvisioningFailed,
                &format!("install exec failed for {instance}: {e}"),
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to execute: {e}")})),
            ))
        }
    }
}

/// POST /admin/provisioning/start — start agent service.
pub async fn handle_provisioning_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;
    require_provisioning_active(&state)?;

    {
        let config = state.config.lock();
        if config.gateway.ui_provisioning.mode != "service_install" {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Mode is 'config_only'"})),
            ));
        }
    }

    let instance = body["instance"]
        .as_str()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'instance'"})),
            )
        })?
        .to_string();

    validate_instance_name(&instance).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("synapseclaw"));
    let output = std::process::Command::new(&exe)
        .args(["service", "--instance", &instance, "start"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            log_audit(
                &state,
                AuditEventType::ProvisioningServiceStarted,
                &format!("instance={instance}"),
            );
            Ok(Json(serde_json::json!({"ok": true, "instance": instance})))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            log_audit(
                &state,
                AuditEventType::ProvisioningFailed,
                &format!("start failed for {instance}: {stderr}"),
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Service start failed: {stderr}")})),
            ))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to execute: {e}")})),
        )),
    }
}

/// POST /admin/provisioning/stop — stop agent service.
pub async fn handle_provisioning_stop(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;
    require_provisioning_active(&state)?;

    {
        let config = state.config.lock();
        if config.gateway.ui_provisioning.mode != "service_install" {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Mode is 'config_only'"})),
            ));
        }
    }

    let instance = body["instance"]
        .as_str()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'instance'"})),
            )
        })?
        .to_string();

    validate_instance_name(&instance).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("synapseclaw"));
    let output = std::process::Command::new(&exe)
        .args(["service", "--instance", &instance, "stop"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            log_audit(
                &state,
                AuditEventType::ProvisioningServiceStopped,
                &format!("instance={instance}"),
            );
            Ok(Json(serde_json::json!({"ok": true, "instance": instance})))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            log_audit(
                &state,
                AuditEventType::ProvisioningFailed,
                &format!("stop failed for {instance}: {stderr}"),
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Service stop failed: {stderr}")})),
            ))
        }
        Err(e) => {
            log_audit(
                &state,
                AuditEventType::ProvisioningFailed,
                &format!("stop exec failed for {instance}: {e}"),
            );
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to execute: {e}")})),
            ))
        }
    }
}

/// POST /admin/provisioning/uninstall — uninstall agent service and remove config dir.
pub async fn handle_provisioning_uninstall(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;
    require_provisioning_active(&state)?;

    {
        let config = state.config.lock();
        if config.gateway.ui_provisioning.mode != "service_install" {
            return Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": "Mode is 'config_only'"})),
            ));
        }
    }

    let instance = body["instance"]
        .as_str()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'instance'"})),
            )
        })?
        .to_string();

    validate_instance_name(&instance).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    // 1. Uninstall systemd service (best-effort — may not exist)
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("synapseclaw"));
    let _ = std::process::Command::new(&exe)
        .args(["service", "--instance", &instance, "uninstall"])
        .output();

    // 2. Remove agent config directory
    let agents_root = {
        let config = state.config.lock();
        resolve_agents_root(&config.gateway.ui_provisioning.agents_root)
    };
    let instance_dir = agents_root.join(&instance);
    if instance_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&instance_dir) {
            log_audit(
                &state,
                AuditEventType::ProvisioningFailed,
                &format!("Failed to remove dir for {instance}: {e}"),
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to remove directory: {e}")})),
            ));
        }
    }

    log_audit(
        &state,
        AuditEventType::ProvisioningFailed, // reuse — no dedicated uninstall event type
        &format!("Uninstalled and removed instance={instance}"),
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "instance": instance,
    })))
}

/// POST /admin/provisioning/patch-broker — merge TOML snippet into broker's agents_ipc config.
///
/// Accepts `{ "patch_toml": "..." }` — expected to contain `[agents_ipc]` keys like
/// `lateral_text_pairs` and `[agents_ipc.l4_destinations]`.
/// Merges into the running config and persists to disk.
pub async fn handle_provisioning_patch_broker(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;
    require_provisioning_active(&state)?;

    let patch_toml = body["patch_toml"]
        .as_str()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing 'patch_toml'"})),
            )
        })?
        .to_string();

    // Parse the patch as a TOML document (supports [table] headers)
    let patch_table: toml::map::Map<String, toml::Value> =
        toml::from_str(&patch_toml).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
            )
        })?;

    // Read current config file, merge the agents_ipc section, write back
    let config_path = {
        let config = state.config.lock();
        config.config_path.clone()
    };

    let raw = std::fs::read_to_string(&config_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to read config: {e}")})),
        )
    })?;

    let mut doc: toml::Value = raw.parse().map_err(|e: toml::de::Error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to parse config: {e}")})),
        )
    })?;

    // Merge patch into doc — only agents_ipc keys
    if let Some(patch_ipc) = patch_table.get("agents_ipc") {
        let doc_table = doc.as_table_mut().ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Config root is not a table"})),
            )
        })?;

        let doc_ipc = doc_table
            .entry("agents_ipc")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

        if let (Some(dst), Some(src)) = (doc_ipc.as_table_mut(), patch_ipc.as_table()) {
            for (key, value) in src {
                dst.insert(key.clone(), value.clone());
            }
        }
    }

    // Write back
    let new_content = toml::to_string_pretty(&doc).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to serialize config: {e}")})),
        )
    })?;

    std::fs::write(&config_path, &new_content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to write config: {e}")})),
        )
    })?;

    // Reload agents_ipc section in-memory from the patched TOML
    {
        let mut config = state.config.lock();
        if let Ok(raw) = std::fs::read_to_string(&config_path) {
            if let Ok(val) = raw.parse::<toml::Value>() {
                if let Some(ipc_val) = val.get("agents_ipc") {
                    if let Ok(ipc) = ipc_val
                        .clone()
                        .try_into::<fork_config::schema::AgentsIpcConfig>()
                    {
                        // Preserve runtime fields that aren't in the TOML patch
                        let old = &config.agents_ipc;
                        let broker_token = old.broker_token.clone();
                        let proxy_token = old.proxy_token.clone();
                        let gateway_url = old.gateway_url.clone();
                        config.agents_ipc = ipc;
                        if config.agents_ipc.broker_token.is_none() {
                            config.agents_ipc.broker_token = broker_token;
                        }
                        if config.agents_ipc.proxy_token.is_none() {
                            config.agents_ipc.proxy_token = proxy_token;
                        }
                        if config.agents_ipc.gateway_url.is_none() {
                            config.agents_ipc.gateway_url = gateway_url;
                        }
                    }
                }
            }
        }
    }

    log_audit(
        &state,
        AuditEventType::ProvisioningArmed, // reuse — no dedicated patch event type
        "Broker config patched with agents_ipc keys",
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "Broker config patched and reloaded",
    })))
}

/// GET /admin/provisioning/used-ports — scan agent configs and return used gateway ports.
pub async fn handle_provisioning_used_ports(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;

    let agents_root = {
        let config = state.config.lock();
        resolve_agents_root(&config.gateway.ui_provisioning.agents_root)
    };

    let mut ports: Vec<u16> = Vec::new();

    // Also include the broker's own port
    {
        let config = state.config.lock();
        ports.push(config.gateway.port);
    }

    // Scan agent dirs for [gateway].port in config.toml
    if let Ok(entries) = std::fs::read_dir(&agents_root) {
        for entry in entries.flatten() {
            let config_path = entry.path().join("config.toml");
            if config_path.exists() {
                if let Ok(raw) = std::fs::read_to_string(&config_path) {
                    if let Ok(val) = raw.parse::<toml::Value>() {
                        if let Some(port) = val
                            .get("gateway")
                            .and_then(|g| g.get("port"))
                            .and_then(|p| p.as_integer())
                        {
                            if let Ok(p) = u16::try_from(port) {
                                ports.push(p);
                            }
                        }
                    }
                }
            }
        }
    }

    ports.sort_unstable();
    ports.dedup();

    Ok(Json(serde_json::json!({
        "ports": ports,
        "next_available": ports.iter().max().map(|p| p + 1).unwrap_or(42618),
    })))
}

/// GET /admin/provisioning/topology — merged agent list + communication edges.
///
/// Combines gateway registry (Phase 3.8 registered agents) with IPC DB agents,
/// plus communication topology from config (lateral_text_pairs, l4_destinations).
#[derive(Debug, Deserialize)]
pub struct TopologyQuery {
    pub include_traffic: Option<bool>,
    pub include_ephemeral: Option<bool>,
    pub traffic_hours: Option<u64>,
    pub traffic_min_count: Option<u32>,
}

fn is_ephemeral_agent_id(agent_id: &str) -> bool {
    agent_id.starts_with("eph-")
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn handle_provisioning_topology(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<TopologyQuery>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    require_localhost(&peer, &state.admin_cidrs)?;
    require_admin_auth(&state, &headers)?;

    let include_traffic = query.include_traffic.unwrap_or(false);
    let include_ephemeral = query.include_ephemeral.unwrap_or(false);
    let traffic_hours = query.traffic_hours.unwrap_or(24).clamp(1, 24 * 30);
    let traffic_min_count = query.traffic_min_count.unwrap_or(2).clamp(1, 100);

    let mut agents_map: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();

    // Source 1: gateway registry (agents that registered their gateways)
    for info in state.agent_registry.list() {
        agents_map.insert(
            info.agent_id.clone(),
            serde_json::json!({
                "agent_id": info.agent_id,
                "role": info.role,
                "trust_level": info.trust_level,
                "status": format!("{:?}", info.status).to_lowercase(),
                "gateway_url": info.gateway_url,
                "model": info.model,
                "last_seen": info.last_seen,
                "uptime_seconds": info.uptime_seconds,
                "channels": info.channels,
                "source": "registry",
            }),
        );
    }

    // Source 2: IPC DB (agents that have done IPC operations)
    if let Some(ref ipc_db) = state.ipc_db {
        let staleness = state.config.lock().agents_ipc.staleness_secs;
        for agent in ipc_db.list_agents(staleness) {
            agents_map
                .entry(agent.agent_id.clone())
                .and_modify(|existing| {
                    // Merge IPC DB fields into registry entry
                    if let Some(obj) = existing.as_object_mut() {
                        if agent.public_key.is_some() {
                            obj.insert("public_key".into(), serde_json::json!(agent.public_key));
                        }
                        // IPC DB status may be more accurate
                        obj.insert("ipc_status".into(), serde_json::json!(agent.status));
                    }
                })
                .or_insert_with(|| {
                    serde_json::json!({
                        "agent_id": agent.agent_id,
                        "role": agent.role,
                        "trust_level": agent.trust_level,
                        "status": agent.status,
                        "gateway_url": null,
                        "model": null,
                        "last_seen": agent.last_seen,
                        "public_key": agent.public_key,
                        "source": "ipc_db",
                    })
                });
        }
    }

    if !include_ephemeral {
        agents_map.retain(|agent_id, _| !is_ephemeral_agent_id(agent_id));
    }

    // Build edges from config topology + actual message history
    let config = state.config.lock();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    let mut seen_pairs: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    // lateral_text_pairs — bidirectional L3 peer communication (config-declared)
    for pair in &config.agents_ipc.lateral_text_pairs {
        edges.push(serde_json::json!({
            "from": pair[0],
            "to": pair[1],
            "type": "lateral",
        }));
        seen_pairs.insert((pair[0].clone(), pair[1].clone()));
        seen_pairs.insert((pair[1].clone(), pair[0].clone()));
    }

    // l4_destinations — directed: L4 agent → destination (via alias)
    for (alias, target) in &config.agents_ipc.l4_destinations {
        edges.push(serde_json::json!({
            "from": alias,
            "to": target,
            "type": "l4_destination",
            "alias": alias,
        }));
        seen_pairs.insert((alias.clone(), target.clone()));
    }
    drop(config);

    // Message-based edges — actual recent communication observed in IPC history.
    // Hidden by default on the main fleet graph because policy topology and
    // historical traffic are different concepts and become unreadable when
    // flattened into one graph.
    if include_traffic {
        let now = unix_now();
        let since_ts = now - (traffic_hours as i64 * 3600);
        if let Some(ref ipc_db) = state.ipc_db {
            for (from, to, count) in ipc_db.communication_pairs_filtered(
                Some(since_ts),
                i64::from(traffic_min_count),
                100,
            ) {
                if !include_ephemeral
                    && (is_ephemeral_agent_id(&from) || is_ephemeral_agent_id(&to))
                {
                    continue;
                }
                // Skip if already covered by config-declared edges
                if seen_pairs.contains(&(from.clone(), to.clone())) {
                    continue;
                }
                // Only show edges where both agents are known
                if agents_map.contains_key(&from) && agents_map.contains_key(&to) {
                    edges.push(serde_json::json!({
                        "from": from,
                        "to": to,
                        "type": "message",
                        "count": count,
                    }));
                    seen_pairs.insert((from, to));
                }
            }
        }
    }

    let agents: Vec<serde_json::Value> = agents_map.into_values().collect();

    Ok(Json(serde_json::json!({
        "agents": agents,
        "edges": edges,
    })))
}
