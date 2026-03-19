//! UI agent provisioning handlers (Phase 3.8 Step 11).
//!
//! Broker-only, disabled by default, mode-gated, arm-required.
//! Creates agent config dirs and optionally installs OS services.

use super::{require_localhost, AppState};
use crate::gateway::api::extract_bearer_token;
use crate::security::audit::{AuditEvent, AuditEventType};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

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
    let token_hash = crate::security::pairing::hash_token(token);
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
    require_localhost(&peer)?;
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
    require_localhost(&peer)?;
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
    require_localhost(&peer)?;
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
    require_localhost(&peer)?;
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

    // Run: zeroclaw service --instance <name> install
    // Note: --instance must precede the subcommand (clap ordering)
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zeroclaw"));
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
    require_localhost(&peer)?;
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

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zeroclaw"));
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
    require_localhost(&peer)?;
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

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zeroclaw"));
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
    require_localhost(&peer)?;
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
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zeroclaw"));
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
