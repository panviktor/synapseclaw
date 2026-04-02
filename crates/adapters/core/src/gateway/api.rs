//! REST API handlers for the web dashboard.
//!
//! All `/api/*` routes require bearer token authentication (PairingGuard).

use super::AppState;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use synapse_infra::config_io::ConfigIO;

const MASKED_SECRET: &str = "***MASKED***";

// ── Bearer token auth extractor ─────────────────────────────────

/// Extract and validate bearer token from Authorization header.
pub(crate) fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
}

/// Verify bearer token against PairingGuard. Returns error response if unauthorized.
fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if !state.pairing.require_pairing() {
        return Ok(());
    }

    let token = extract_bearer_token(headers).unwrap_or("");
    if state.pairing.is_authenticated(token) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Unauthorized — pair first via POST /pair, then send Authorization: Bearer <token>"
            })),
        ))
    }
}

// ── Query parameters ─────────────────────────────────────────────

#[derive(Deserialize)]
pub struct MemoryQuery {
    pub query: Option<String>,
    pub category: Option<String>,
}

#[derive(Deserialize)]
pub struct MemoryStoreBody {
    pub key: String,
    pub content: String,
    pub category: Option<String>,
}

#[derive(Deserialize)]
pub struct CronRunsQuery {
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct CronAddBody {
    pub name: Option<String>,
    pub schedule: String,
    pub command: String,
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    pub limit: Option<u32>,
    pub from_ts: Option<i64>,
    pub event_type: Option<String>,
    pub surface: Option<String>,
}

#[derive(Deserialize)]
pub struct ChatSessionsQuery {
    pub prefix: Option<String>,
}

#[derive(Deserialize)]
pub struct ChatMessagesQuery {
    pub limit: Option<i64>,
}

// ── Handlers ────────────────────────────────────────────────────

/// GET /api/status — system status overview
pub async fn handle_api_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let health = crate::health::snapshot();

    let mut channels = serde_json::Map::new();

    for (channel, present) in config.channels_config.channels() {
        channels.insert(channel.name().to_string(), serde_json::Value::Bool(present));
    }

    let body = serde_json::json!({
        "provider": config.default_provider,
        "model": state.model,
        "summary_model": config.summary.model.as_ref().or(config.summary_model.as_ref()),
        "temperature": state.temperature,
        "uptime_seconds": health.uptime_seconds,
        "gateway_port": config.gateway.port,
        "locale": "en",
        "memory_backend": state.mem.name(),
        "paired": state.pairing.is_paired(),
        "channels": channels,
        "health": health,
    });

    Json(body).into_response()
}

/// GET /api/agents — list registered agent daemons with live status (Phase 3.8).
pub async fn handle_api_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agents: Vec<serde_json::Value> = state
        .agent_registry
        .list()
        .iter()
        .map(|a| {
            serde_json::json!({
                "agent_id": a.agent_id,
                "gateway_url": a.gateway_url,
                "trust_level": a.trust_level,
                "role": a.role,
                "model": a.model,
                "status": a.status,
                "last_seen": a.last_seen,
                "uptime_seconds": a.uptime_seconds,
                "channels": a.channels,
            })
        })
        .collect();

    Json(serde_json::json!({ "agents": agents })).into_response()
}

/// GET /api/agents/:agent_id/status — proxy status request to a specific agent (Phase 3.8).
pub async fn handle_api_agent_status_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => {
            return (StatusCode::NOT_FOUND, "Agent not found").into_response();
        }
    };

    // Proxy GET to agent's /api/status
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/status", agent.gateway_url);
    match client
        .get(&url)
        .bearer_auth(&agent.proxy_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

/// PUT /api/agents/:agent_id/summary-model — proxy summary model change to a specific agent.
pub async fn handle_api_agent_summary_model_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/summary-model", agent.gateway_url);
    match client
        .put(&url)
        .bearer_auth(&agent.proxy_token)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

/// PUT /api/summary-model — switch the summary model on the fly
pub async fn handle_api_summary_model_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let model = payload["model"].as_str().map(String::from);

    // Update AppState (summary_model is behind Arc so we need interior mutability)
    // Since AppState.summary_model is not behind a lock, we store it in config
    {
        let mut config = state.config.lock();
        config.summary_model = model.clone();
    }

    Json(serde_json::json!({
        "ok": true,
        "summary_model": model,
    }))
    .into_response()
}

/// GET /api/config — current config (api_key masked)
pub async fn handle_api_config_get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();

    // Serialize to TOML after masking sensitive fields.
    let masked_config = mask_sensitive_fields(&config);
    let toml_str = match toml::to_string_pretty(&masked_config) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to serialize config: {e}")})),
            )
                .into_response();
        }
    };

    Json(serde_json::json!({
        "format": "toml",
        "content": toml_str,
    }))
    .into_response()
}

/// PUT /api/config — update config from TOML body
pub async fn handle_api_config_put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    // Parse the incoming TOML
    let incoming: synapse_domain::config::schema::Config = match toml::from_str(&body) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid TOML: {e}")})),
            )
                .into_response();
        }
    };

    let current_config = state.config.lock().clone();
    let mut new_config = hydrate_config_for_save(incoming, &current_config);

    // Security: ui_provisioning and admin_cidrs are immutable via /api/config.
    // Restore the original values to prevent escalation via bearer-only API.
    new_config.gateway.ui_provisioning = current_config.gateway.ui_provisioning.clone();
    new_config.gateway.admin_cidrs = current_config.gateway.admin_cidrs.clone();

    if let Err(e) = new_config.validate() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid config: {e}")})),
        )
            .into_response();
    }

    // Save to disk
    if let Err(e) = new_config.save().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to save config: {e}")})),
        )
            .into_response();
    }

    // Update in-memory config
    *state.config.lock() = new_config;

    Json(serde_json::json!({"status": "ok"})).into_response()
}

/// GET /api/tools — list registered tool specs
pub async fn handle_api_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let tools: Vec<serde_json::Value> = state
        .tools_registry
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.parameters,
            })
        })
        .collect();

    Json(serde_json::json!({"tools": tools})).into_response()
}

/// GET /api/cron — list cron jobs
pub async fn handle_api_cron_list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let db = match &state.surreal {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "SurrealDB not available for cron"})),
            )
                .into_response();
        }
    };
    match synapse_cron::list_jobs(db).await {
        Ok(jobs) => {
            let jobs_json: Vec<serde_json::Value> = jobs
                .iter()
                .map(|job| {
                    serde_json::json!({
                        "id": job.id,
                        "name": job.name,
                        "command": job.command,
                        "next_run": job.next_run.to_rfc3339(),
                        "last_run": job.last_run.map(|t| t.to_rfc3339()),
                        "last_status": job.last_status,
                        "enabled": job.enabled,
                    })
                })
                .collect();
            Json(serde_json::json!({"jobs": jobs_json})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to list cron jobs: {e}")})),
        )
            .into_response(),
    }
}

/// POST /api/cron — add a new cron job
pub async fn handle_api_cron_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CronAddBody>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let db = match &state.surreal {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "SurrealDB not available for cron"})),
            )
                .into_response();
        }
    };
    let schedule = synapse_cron::Schedule::Cron {
        expr: body.schedule,
        tz: None,
    };

    match synapse_cron::add_shell_job_with_approval(
        db,
        &config,
        body.name,
        schedule,
        &body.command,
        false,
    )
    .await
    {
        Ok(job) => Json(serde_json::json!({
            "status": "ok",
            "job": {
                "id": job.id,
                "name": job.name,
                "command": job.command,
                "enabled": job.enabled,
            }
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to add cron job: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/cron/:id/runs — list recent runs for a cron job
pub async fn handle_api_cron_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<CronRunsQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;
    let db = match &state.surreal {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "SurrealDB not available for cron"})),
            )
                .into_response();
        }
    };

    // Verify the job exists before listing runs.
    if let Err(e) = synapse_cron::get_job(db, &id).await {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("Cron job not found: {e}")})),
        )
            .into_response();
    }

    match synapse_cron::list_runs(db, &id, limit).await {
        Ok(runs) => {
            let runs_json: Vec<serde_json::Value> = runs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "job_id": r.job_id,
                        "started_at": r.started_at.to_rfc3339(),
                        "finished_at": r.finished_at.to_rfc3339(),
                        "status": r.status,
                        "output": r.output,
                        "duration_ms": r.duration_ms,
                    })
                })
                .collect();
            Json(serde_json::json!({"runs": runs_json})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to list cron runs: {e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/cron/:id — remove a cron job
pub async fn handle_api_cron_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let db = match &state.surreal {
        Some(db) => db,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "SurrealDB not available for cron"})),
            )
                .into_response();
        }
    };
    match synapse_cron::remove_job(db, &id).await {
        Ok(()) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to remove cron job: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/integrations — list all integrations with status
pub async fn handle_api_integrations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let entries = crate::integrations::registry::all_integrations();

    let integrations: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let status = (entry.status_fn)(&config);
            serde_json::json!({
                "name": entry.name,
                "description": entry.description,
                "category": entry.category,
                "status": status,
            })
        })
        .collect();

    Json(serde_json::json!({"integrations": integrations})).into_response()
}

/// GET /api/integrations/settings — return per-integration settings (enabled + category)
pub async fn handle_api_integrations_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let entries = crate::integrations::registry::all_integrations();

    let mut settings = serde_json::Map::new();
    for entry in &entries {
        let status = (entry.status_fn)(&config);
        let enabled = matches!(status, crate::integrations::IntegrationStatus::Active);
        settings.insert(
            entry.name.to_string(),
            serde_json::json!({
                "enabled": enabled,
                "category": entry.category,
                "status": status,
            }),
        );
    }

    Json(serde_json::json!({"settings": settings})).into_response()
}

/// POST /api/doctor — run diagnostics
pub async fn handle_api_doctor(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let config = state.config.lock().clone();
    let results = crate::doctor::diagnose(&config);

    let ok_count = results
        .iter()
        .filter(|r| r.severity == crate::doctor::Severity::Ok)
        .count();
    let warn_count = results
        .iter()
        .filter(|r| r.severity == crate::doctor::Severity::Warn)
        .count();
    let error_count = results
        .iter()
        .filter(|r| r.severity == crate::doctor::Severity::Error)
        .count();

    Json(serde_json::json!({
        "results": results,
        "summary": {
            "ok": ok_count,
            "warnings": warn_count,
            "errors": error_count,
        }
    }))
    .into_response()
}

/// GET /api/memory — list or search memory entries
pub async fn handle_api_memory_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<MemoryQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    if let Some(ref query) = params.query {
        // Search mode
        match state.mem.recall(query, 50, None).await {
            Ok(entries) => Json(serde_json::json!({"entries": entries})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Memory recall failed: {e}")})),
            )
                .into_response(),
        }
    } else {
        // List mode
        let category = params.category.as_deref().map(|cat| match cat {
            "core" => synapse_domain::domain::memory::MemoryCategory::Core,
            "daily" => synapse_domain::domain::memory::MemoryCategory::Daily,
            "conversation" => synapse_domain::domain::memory::MemoryCategory::Conversation,
            other => synapse_domain::domain::memory::MemoryCategory::Custom(other.to_string()),
        });

        // UnifiedMemoryPort uses recall() for listing — category name as query term.
        let query = category.as_ref().map(|c| c.to_string()).unwrap_or_default();
        match state.mem.recall(&query, 100, None).await {
            Ok(entries) => Json(serde_json::json!({"entries": entries})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Memory list failed: {e}")})),
            )
                .into_response(),
        }
    }
}

/// POST /api/memory — store a memory entry
pub async fn handle_api_memory_store(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MemoryStoreBody>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let category = body
        .category
        .as_deref()
        .map(|cat| match cat {
            "core" => synapse_domain::domain::memory::MemoryCategory::Core,
            "daily" => synapse_domain::domain::memory::MemoryCategory::Daily,
            "conversation" => synapse_domain::domain::memory::MemoryCategory::Conversation,
            other => synapse_domain::domain::memory::MemoryCategory::Custom(other.to_string()),
        })
        .unwrap_or(synapse_domain::domain::memory::MemoryCategory::Core);

    match state
        .mem
        .store(&body.key, &body.content, &category, None)
        .await
    {
        Ok(()) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Memory store failed: {e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/memory/:key — delete a memory entry
pub async fn handle_api_memory_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    match state.mem.forget(&key).await {
        Ok(deleted) => {
            Json(serde_json::json!({"status": "ok", "deleted": deleted})).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Memory forget failed: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/cost — cost summary
pub async fn handle_api_cost(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    if let Some(ref tracker) = state.cost_tracker {
        match tracker.get_summary() {
            Ok(summary) => Json(serde_json::json!({"cost": summary})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Cost summary failed: {e}")})),
            )
                .into_response(),
        }
    } else {
        Json(serde_json::json!({
            "cost": {
                "session_cost_usd": 0.0,
                "daily_cost_usd": 0.0,
                "monthly_cost_usd": 0.0,
                "total_tokens": 0,
                "request_count": 0,
                "by_model": {},
            }
        }))
        .into_response()
    }
}

/// GET /api/cli-tools — discovered CLI tools
pub async fn handle_api_cli_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let tools = crate::tools::cli_discovery::discover_cli_tools(&[], &[]);

    Json(serde_json::json!({"cli_tools": tools})).into_response()
}

/// GET /api/health — component health snapshot
pub async fn handle_api_health(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let snapshot = crate::health::snapshot();
    Json(serde_json::json!({"health": snapshot})).into_response()
}

// ── Activity feed (Phase 3.9) ────────────────────────────────────

// ── Chat session REST endpoints (Phase 3.9) ──────────────────────

/// GET /api/chat/sessions — list chat sessions (REST alternative to WS RPC).
pub async fn handle_api_chat_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ChatSessionsQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let prefix = params.prefix.as_deref().unwrap_or("");
    match &state.chat_db {
        Some(db) => match db.list_sessions(prefix).await {
            Ok(sessions) => Json(serde_json::json!({"sessions": sessions})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response(),
        },
        None => Json(serde_json::json!({"sessions": []})).into_response(),
    }
}

/// GET /api/chat/sessions/:key/messages — get messages for a chat session.
pub async fn handle_api_chat_session_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(params): Query<ChatMessagesQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    match &state.chat_db {
        Some(db) => match db.get_messages(&key, limit).await {
            Ok(messages) => Json(serde_json::json!({"messages": messages})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("{e}")})),
            )
                .into_response(),
        },
        None => Json(serde_json::json!({"messages": []})).into_response(),
    }
}

// ── Channel sessions (Phase 3.12) ───────────────────────────────

/// GET /api/channel/sessions — list channel conversation sessions with metadata.
///
/// Returns all channel sessions visible to the authenticated operator.
/// This is a single-operator endpoint — the gateway token already scopes
/// access to this agent instance. Multi-tenant filtering would require
/// Phase 4.0's capability model.
pub async fn handle_api_channel_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let backend = match &state.channel_session_backend {
        Some(b) => b,
        None => return Json(serde_json::json!({"sessions": []})).into_response(),
    };

    let metadata = backend.list_sessions_with_metadata();
    let sessions: Vec<serde_json::Value> = metadata
        .into_iter()
        .map(|m| {
            let (channel, sender) = m.key.split_once('_').unwrap_or(("unknown", m.key.as_str()));
            let summary = backend.load_summary(&m.key);
            serde_json::json!({
                "key": m.key,
                "channel": channel,
                "sender": sender,
                "created_at": m.created_at.timestamp(),
                "last_activity": m.last_activity.timestamp(),
                "message_count": m.message_count,
                "summary": summary.map(|s| s.summary),
            })
        })
        .collect();

    Json(serde_json::json!({"sessions": sessions})).into_response()
}

/// GET /api/channel/sessions/:key/messages — get messages for a channel session.
pub async fn handle_api_channel_session_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let backend = match &state.channel_session_backend {
        Some(b) => b,
        None => return Json(serde_json::json!({"messages": []})).into_response(),
    };

    let messages = backend.load(&key);
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "content": m.content,
            })
        })
        .collect();

    Json(serde_json::json!({"messages": msgs})).into_response()
}

/// DELETE /api/channel/sessions/:key — delete a channel session and its summary.
pub async fn handle_api_channel_session_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let backend = match &state.channel_session_backend {
        Some(b) => b,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Channel sessions not available"})),
            )
                .into_response()
        }
    };

    match backend.delete(&key) {
        Ok(true) => Json(serde_json::json!({"deleted": true})).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Session not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

// ── Phase 4.0: Channel capabilities + deliver ───────────────────

/// GET /api/channels/capabilities — list capabilities for all known channels.
pub async fn handle_api_channel_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let registry = match &state.channel_registry {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Channel registry not available"})),
            )
                .into_response();
        }
    };

    let channels = [
        "telegram",
        "discord",
        "slack",
        "matrix",
        "signal",
        "email",
        "mattermost",
        "webhook",
    ];
    let mut result = serde_json::Map::new();
    for name in &channels {
        let caps = registry.capabilities(name);
        if !caps.is_empty() {
            let cap_names: Vec<&str> = caps
                .iter()
                .map(|c| match c {
                    synapse_domain::domain::channel::ChannelCapability::SendText => "SendText",
                    synapse_domain::domain::channel::ChannelCapability::ReceiveText => {
                        "ReceiveText"
                    }
                    synapse_domain::domain::channel::ChannelCapability::Threads => "Threads",
                    synapse_domain::domain::channel::ChannelCapability::Reactions => "Reactions",
                    synapse_domain::domain::channel::ChannelCapability::Typing => "Typing",
                    synapse_domain::domain::channel::ChannelCapability::Attachments => {
                        "Attachments"
                    }
                    synapse_domain::domain::channel::ChannelCapability::RichFormatting => {
                        "RichFormatting"
                    }
                    synapse_domain::domain::channel::ChannelCapability::EditMessage => {
                        "EditMessage"
                    }
                    synapse_domain::domain::channel::ChannelCapability::RuntimeCommands => {
                        "RuntimeCommands"
                    }
                    synapse_domain::domain::channel::ChannelCapability::InterruptOnNewMessage => {
                        "InterruptOnNewMessage"
                    }
                    synapse_domain::domain::channel::ChannelCapability::ToolContextDisplay => {
                        "ToolContextDisplay"
                    }
                })
                .collect();
            result.insert((*name).to_string(), serde_json::json!(cap_names));
        }
    }
    Json(serde_json::Value::Object(result)).into_response()
}

/// POST /api/channels/deliver — deliver a message to a channel via OutboundIntent.
pub async fn handle_api_channel_deliver(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let registry = match &state.channel_registry {
        Some(r) => r,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Channel registry not available"})),
            )
                .into_response();
        }
    };

    let channel = match body["channel"].as_str() {
        Some(c) if !c.is_empty() => c,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing 'channel' field"})),
            )
                .into_response();
        }
    };
    let recipient = match body["recipient"].as_str() {
        Some(r) if !r.is_empty() => r,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing 'recipient' field"})),
            )
                .into_response();
        }
    };
    let content = match body["content"].as_str() {
        Some(c) if !c.is_empty() => c,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing 'content' field"})),
            )
                .into_response();
        }
    };
    let thread_ref = body["thread_ref"].as_str().map(String::from);

    let mut intent = synapse_domain::domain::channel::OutboundIntent::notify(
        channel,
        recipient,
        content.to_string(),
    );
    intent.thread_ref = thread_ref;

    match registry.deliver(&intent).await {
        Ok(()) => Json(serde_json::json!({"delivered": true, "channel": channel})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

// ── Phase 4.0: Conversation REST API ─────────────────────────────

#[derive(Deserialize)]
pub struct ConversationListParams {
    pub prefix: Option<String>,
}

/// GET /api/conversations — list conversation sessions.
pub async fn handle_api_conversations_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ConversationListParams>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let store = match &state.conversation_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Conversation store not available"})),
            )
                .into_response();
        }
    };

    let sessions = store.list_sessions(params.prefix.as_deref()).await;
    let result: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "key": s.key,
                "kind": s.kind.to_string(),
                "label": s.label,
                "summary": s.summary,
                "current_goal": s.current_goal,
                "created_at": s.created_at,
                "last_active": s.last_active,
                "message_count": s.message_count,
                "input_tokens": s.input_tokens,
                "output_tokens": s.output_tokens,
            })
        })
        .collect();
    Json(serde_json::json!({"sessions": result})).into_response()
}

#[derive(Deserialize)]
pub struct ConversationEventsParams {
    pub limit: Option<usize>,
}

/// GET /api/conversations/:key — get a session with recent events.
pub async fn handle_api_conversations_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(params): Query<ConversationEventsParams>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let store = match &state.conversation_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Conversation store not available"})),
            )
                .into_response();
        }
    };

    let session = match store.get_session(&key).await {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Session not found"})),
            )
                .into_response();
        }
    };

    let limit = params.limit.unwrap_or(50);
    let events = store.get_events(&key, limit).await;
    let event_json: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "event_type": e.event_type.to_string(),
                "actor": e.actor,
                "content": e.content,
                "tool_name": e.tool_name,
                "run_id": e.run_id,
                "input_tokens": e.input_tokens,
                "output_tokens": e.output_tokens,
                "timestamp": e.timestamp,
            })
        })
        .collect();

    Json(serde_json::json!({
        "key": session.key,
        "kind": session.kind.to_string(),
        "label": session.label,
        "summary": session.summary,
        "current_goal": session.current_goal,
        "created_at": session.created_at,
        "last_active": session.last_active,
        "message_count": session.message_count,
        "input_tokens": session.input_tokens,
        "output_tokens": session.output_tokens,
        "events": event_json,
    }))
    .into_response()
}

/// DELETE /api/conversations/:key — delete a conversation session.
pub async fn handle_api_conversations_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let store = match &state.conversation_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Conversation store not available"})),
            )
                .into_response();
        }
    };

    match store.delete_session(&key).await {
        Ok(true) => Json(serde_json::json!({"deleted": true, "key": key})).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Session not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

// ── Phase 4.0: Runs REST API ─────────────────────────────────────

#[derive(Deserialize)]
pub struct RunsListParams {
    pub conversation_key: Option<String>,
    pub limit: Option<usize>,
}

/// GET /api/runs — list runs, optionally filtered by conversation_key.
pub async fn handle_api_runs_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<RunsListParams>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let store = match &state.run_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Run store not available"})),
            )
                .into_response();
        }
    };

    let limit = params.limit.unwrap_or(50);

    let runs = if let Some(ref conv_key) = params.conversation_key {
        store.list_runs(conv_key, limit).await
    } else {
        store.list_all_runs(limit).await
    };
    let result: Vec<serde_json::Value> = runs
        .iter()
        .map(|r| {
            serde_json::json!({
                "run_id": r.run_id,
                "conversation_key": r.conversation_key,
                "origin": r.origin.to_string(),
                "state": r.state.to_string(),
                "started_at": r.started_at,
                "finished_at": r.finished_at,
            })
        })
        .collect();
    Json(serde_json::json!({"runs": result})).into_response()
}

/// GET /api/runs/:run_id — get a run with its events.
pub async fn handle_api_runs_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Query(params): Query<ConversationEventsParams>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let store = match &state.run_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Run store not available"})),
            )
                .into_response();
        }
    };

    let run = match store.get_run(&run_id).await {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Run not found"})),
            )
                .into_response();
        }
    };

    let limit = params.limit.unwrap_or(100);
    let events = store.get_events(&run_id, limit).await;
    let event_json: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "event_type": e.event_type.to_string(),
                "content": e.content,
                "tool_name": e.tool_name,
                "created_at": e.created_at,
            })
        })
        .collect();

    Json(serde_json::json!({
        "run_id": run.run_id,
        "conversation_key": run.conversation_key,
        "origin": run.origin.to_string(),
        "state": run.state.to_string(),
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "events": event_json,
    }))
    .into_response()
}

// ── Activity feed (Phase 3.9) ────────────────────────────────────

/// Known channel name prefixes for distinguishing channel vs web_chat sessions.
const CHANNEL_PREFIXES: &[&str] = &[
    "telegram",
    "discord",
    "slack",
    "matrix",
    "webhook",
    "whatsapp",
    "mattermost",
    "irc",
    "lark",
    "feishu",
    "dingtalk",
    "qq",
    "nextcloud",
    "wati",
    "linq",
    "clawdtalk",
    "email",
    "nostr",
];

/// GET /api/activity — agent-local activity feed (cron, chat, channel events).
/// IPC/spawn events are NOT included here — the broker has those in its own ipc_db.
pub async fn handle_api_activity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ActivityQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    use crate::gateway::ipc::{ActivityEvent, TraceRef};

    let limit = params.limit.unwrap_or(50).clamp(1, 200) as usize;
    let from_ts = params.from_ts.unwrap_or(0);
    let mut events: Vec<ActivityEvent> = Vec::new();

    // Derive agent_id from config
    let config = state.config.lock().clone();
    let agent_id = config
        .agents_ipc
        .agent_id
        .clone()
        .unwrap_or_else(|| "local".to_string());

    // 1. Chat/channel messages from chat_db (real message-level events, not session summaries)
    if let Some(ref db) = state.chat_db {
        if let Ok(sessions) = db.list_sessions("").await {
            for session in sessions {
                if session.last_active < from_ts {
                    continue;
                }

                // Determine surface from session key prefix
                let key = &session.key;
                let mut surface = "web_chat";
                let mut channel_name: Option<String> = None;

                for prefix in CHANNEL_PREFIXES {
                    if key.starts_with(prefix) {
                        surface = "channel";
                        channel_name = Some(prefix.to_string());
                        break;
                    }
                }

                // Fetch recent messages for this session (real turns, not summaries)
                let msg_limit = 10i64; // per session
                if let Ok(messages) = db.get_messages(key, msg_limit).await {
                    for msg in &messages {
                        if msg.timestamp < from_ts {
                            continue;
                        }
                        // Only emit user and assistant turns (skip tool_call/tool_result/system)
                        if msg.kind != "user" && msg.kind != "assistant" {
                            continue;
                        }

                        let preview = if msg.content.len() > 100 {
                            format!("{}…", &msg.content[..100])
                        } else {
                            msg.content.clone()
                        };
                        let event_type = if surface == "channel" {
                            "channel_message"
                        } else {
                            "chat_message"
                        };
                        let label = session.label.as_deref().unwrap_or("session");
                        let summary = if surface == "channel" {
                            format!(
                                "{}/{}: [{}] {}",
                                channel_name.as_deref().unwrap_or("channel"),
                                label,
                                msg.kind,
                                preview
                            )
                        } else {
                            format!("chat/{}: [{}] {}", label, msg.kind, preview)
                        };

                        events.push(ActivityEvent {
                            event_type: event_type.to_string(),
                            agent_id: agent_id.clone(),
                            timestamp: msg.timestamp,
                            summary,
                            trace_ref: TraceRef {
                                surface: surface.to_string(),
                                session_id: None,
                                message_id: Some(msg.id),
                                from_agent: None,
                                to_agent: None,
                                spawn_run_id: None,
                                parent_agent_id: None,
                                child_agent_id: None,
                                chat_session_key: if surface == "web_chat" {
                                    Some(key.clone())
                                } else {
                                    None
                                },
                                run_id: msg.run_id.clone(),
                                channel_name: channel_name.clone(),
                                channel_session_key: if surface == "channel" {
                                    Some(key.clone())
                                } else {
                                    None
                                },
                                job_id: None,
                                job_name: None,
                            },
                        });
                    }
                }
            }
        }
    }

    // 2. Cron runs
    if let Some(ref db) = state.surreal {
        if let Ok(jobs) = synapse_cron::list_jobs(db).await {
            for job in &jobs {
                if let Ok(runs) = synapse_cron::list_runs(db, &job.id, 10).await {
                    for run in &runs {
                        let ts = run.started_at.timestamp();
                        if ts < from_ts {
                            continue;
                        }
                        let name = job.name.as_deref().unwrap_or(&job.id);
                        let dur = run.duration_ms.unwrap_or(0);
                        let summary = format!("cron: {} [{}] ({dur}ms)", name, run.status);

                        events.push(ActivityEvent {
                            event_type: "cron_run".to_string(),
                            agent_id: agent_id.clone(),
                            timestamp: ts,
                            summary,
                            trace_ref: TraceRef {
                                surface: "cron".to_string(),
                                session_id: None,
                                message_id: None,
                                from_agent: None,
                                to_agent: None,
                                spawn_run_id: None,
                                parent_agent_id: None,
                                child_agent_id: None,
                                chat_session_key: None,
                                run_id: None,
                                channel_name: None,
                                channel_session_key: None,
                                job_id: Some(job.id.clone()),
                                job_name: job.name.clone(),
                            },
                        });
                    }
                }
            }
        }
    } // if let Some(ref db)

    // Filter by event_type and surface if specified
    if let Some(ref et) = params.event_type {
        events.retain(|e| e.event_type == *et);
    }
    if let Some(ref sf) = params.surface {
        events.retain(|e| e.trace_ref.surface == *sf);
    }

    // Sort by timestamp desc, truncate
    events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    events.truncate(limit);

    Json(serde_json::json!({"events": events})).into_response()
}

// ── Cron proxy (Phase 3.9) ───────────────────────────────────────

/// GET /api/agents/:agent_id/cron — proxy cron list to agent.
pub async fn handle_api_agent_cron_list_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/cron", agent.gateway_url);
    match client
        .get(&url)
        .bearer_auth(&agent.proxy_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

/// POST /api/agents/:agent_id/cron — proxy cron creation to agent.
pub async fn handle_api_agent_cron_add_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/cron", agent.gateway_url);
    match client
        .post(&url)
        .bearer_auth(&agent.proxy_token)
        .header("Content-Type", "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

/// DELETE /api/agents/:agent_id/cron/:job_id — proxy cron deletion to agent.
pub async fn handle_api_agent_cron_delete_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((agent_id, job_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!(
        "{}/api/cron/{}",
        agent.gateway_url,
        urlencoding::encode(&job_id)
    );
    match client
        .delete(&url)
        .bearer_auth(&agent.proxy_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

/// GET /api/agents/:agent_id/cron/:job_id/runs — proxy cron runs listing to agent.
pub async fn handle_api_agent_cron_runs_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((agent_id, job_id)): Path<(String, String)>,
    Query(params): Query<CronRunsQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let limit = params.limit.unwrap_or(20);
    let url = format!(
        "{}/api/cron/{}/runs?limit={limit}",
        agent.gateway_url,
        urlencoding::encode(&job_id)
    );
    match client
        .get(&url)
        .bearer_auth(&agent.proxy_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

// ── Chat session proxy (Phase 3.9) ───────────────────────────────

/// GET /api/agents/:agent_id/chat/sessions — proxy chat sessions list to agent.
pub async fn handle_api_agent_chat_sessions_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{}/api/chat/sessions", agent.gateway_url);
    match client
        .get(&url)
        .bearer_auth(&agent.proxy_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

/// GET /api/agents/:agent_id/chat/sessions/:key/messages — proxy chat messages to agent.
pub async fn handle_api_agent_chat_messages_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((agent_id, key)): Path<(String, String)>,
    Query(params): Query<ChatMessagesQuery>,
) -> impl IntoResponse {
    if let Err(e) = require_auth(&state, &headers) {
        return e.into_response();
    }

    let agent = match state.agent_registry.get(&agent_id) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let limit = params.limit.unwrap_or(50);
    let url = format!(
        "{}/api/chat/sessions/{}/messages?limit={limit}",
        agent.gateway_url,
        urlencoding::encode(&key)
    );
    match client
        .get(&url)
        .bearer_auth(&agent.proxy_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(_) => (StatusCode::BAD_GATEWAY, "Invalid response from agent").into_response(),
        },
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("Agent returned {}", resp.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::BAD_GATEWAY, "Agent unreachable").into_response(),
    }
}

// ── Helpers ─────────────────────────────────────────────────────

fn is_masked_secret(value: &str) -> bool {
    value == MASKED_SECRET
}

fn mask_optional_secret(value: &mut Option<String>) {
    if value.is_some() {
        *value = Some(MASKED_SECRET.to_string());
    }
}

fn mask_required_secret(value: &mut String) {
    if !value.is_empty() {
        *value = MASKED_SECRET.to_string();
    }
}

fn mask_vec_secrets(values: &mut [String]) {
    for value in values.iter_mut() {
        if !value.is_empty() {
            *value = MASKED_SECRET.to_string();
        }
    }
}

#[allow(clippy::ref_option)]
fn restore_optional_secret(value: &mut Option<String>, current: &Option<String>) {
    if value.as_deref().is_some_and(is_masked_secret) {
        *value = current.clone();
    }
}

fn restore_required_secret(value: &mut String, current: &str) {
    if is_masked_secret(value) {
        *value = current.to_string();
    }
}

fn restore_vec_secrets(values: &mut [String], current: &[String]) {
    for (idx, value) in values.iter_mut().enumerate() {
        if is_masked_secret(value) {
            if let Some(existing) = current.get(idx) {
                *value = existing.clone();
            }
        }
    }
}

fn normalize_route_field(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn model_route_identity_matches(
    incoming: &synapse_domain::config::schema::ModelRouteConfig,
    current: &synapse_domain::config::schema::ModelRouteConfig,
) -> bool {
    normalize_route_field(&incoming.hint) == normalize_route_field(&current.hint)
        && normalize_route_field(&incoming.provider) == normalize_route_field(&current.provider)
        && normalize_route_field(&incoming.model) == normalize_route_field(&current.model)
}

fn model_route_provider_model_matches(
    incoming: &synapse_domain::config::schema::ModelRouteConfig,
    current: &synapse_domain::config::schema::ModelRouteConfig,
) -> bool {
    normalize_route_field(&incoming.provider) == normalize_route_field(&current.provider)
        && normalize_route_field(&incoming.model) == normalize_route_field(&current.model)
}

fn embedding_route_identity_matches(
    incoming: &synapse_domain::config::schema::EmbeddingRouteConfig,
    current: &synapse_domain::config::schema::EmbeddingRouteConfig,
) -> bool {
    normalize_route_field(&incoming.hint) == normalize_route_field(&current.hint)
        && normalize_route_field(&incoming.provider) == normalize_route_field(&current.provider)
        && normalize_route_field(&incoming.model) == normalize_route_field(&current.model)
}

fn embedding_route_provider_model_matches(
    incoming: &synapse_domain::config::schema::EmbeddingRouteConfig,
    current: &synapse_domain::config::schema::EmbeddingRouteConfig,
) -> bool {
    normalize_route_field(&incoming.provider) == normalize_route_field(&current.provider)
        && normalize_route_field(&incoming.model) == normalize_route_field(&current.model)
}

fn restore_model_route_api_keys(
    incoming: &mut [synapse_domain::config::schema::ModelRouteConfig],
    current: &[synapse_domain::config::schema::ModelRouteConfig],
) {
    let mut used_current = vec![false; current.len()];
    for incoming_route in incoming {
        if !incoming_route
            .api_key
            .as_deref()
            .is_some_and(is_masked_secret)
        {
            continue;
        }

        let exact_match_idx = current
            .iter()
            .enumerate()
            .find(|(idx, current_route)| {
                !used_current[*idx] && model_route_identity_matches(incoming_route, current_route)
            })
            .map(|(idx, _)| idx);

        let match_idx = exact_match_idx.or_else(|| {
            current
                .iter()
                .enumerate()
                .find(|(idx, current_route)| {
                    !used_current[*idx]
                        && model_route_provider_model_matches(incoming_route, current_route)
                })
                .map(|(idx, _)| idx)
        });

        if let Some(idx) = match_idx {
            used_current[idx] = true;
            incoming_route.api_key = current[idx].api_key.clone();
        } else {
            // Never persist UI placeholders to disk when no safe restore target exists.
            incoming_route.api_key = None;
        }
    }
}

fn restore_embedding_route_api_keys(
    incoming: &mut [synapse_domain::config::schema::EmbeddingRouteConfig],
    current: &[synapse_domain::config::schema::EmbeddingRouteConfig],
) {
    let mut used_current = vec![false; current.len()];
    for incoming_route in incoming {
        if !incoming_route
            .api_key
            .as_deref()
            .is_some_and(is_masked_secret)
        {
            continue;
        }

        let exact_match_idx = current
            .iter()
            .enumerate()
            .find(|(idx, current_route)| {
                !used_current[*idx]
                    && embedding_route_identity_matches(incoming_route, current_route)
            })
            .map(|(idx, _)| idx);

        let match_idx = exact_match_idx.or_else(|| {
            current
                .iter()
                .enumerate()
                .find(|(idx, current_route)| {
                    !used_current[*idx]
                        && embedding_route_provider_model_matches(incoming_route, current_route)
                })
                .map(|(idx, _)| idx)
        });

        if let Some(idx) = match_idx {
            used_current[idx] = true;
            incoming_route.api_key = current[idx].api_key.clone();
        } else {
            // Never persist UI placeholders to disk when no safe restore target exists.
            incoming_route.api_key = None;
        }
    }
}

fn mask_sensitive_fields(
    config: &synapse_domain::config::schema::Config,
) -> synapse_domain::config::schema::Config {
    let mut masked = config.clone();

    mask_optional_secret(&mut masked.api_key);
    mask_vec_secrets(&mut masked.reliability.api_keys);
    mask_vec_secrets(&mut masked.gateway.paired_tokens);
    mask_optional_secret(&mut masked.composio.api_key);
    mask_optional_secret(&mut masked.browser.computer_use.api_key);
    mask_optional_secret(&mut masked.web_search.brave_api_key);
    mask_optional_secret(&mut masked.storage.provider.config.db_url);
    if let Some(cloudflare) = masked.tunnel.cloudflare.as_mut() {
        mask_required_secret(&mut cloudflare.token);
    }
    if let Some(ngrok) = masked.tunnel.ngrok.as_mut() {
        mask_required_secret(&mut ngrok.auth_token);
    }

    for agent in masked.agents.values_mut() {
        mask_optional_secret(&mut agent.api_key);
    }
    mask_optional_secret(&mut masked.agents_ipc.broker_token);
    for route in &mut masked.model_routes {
        mask_optional_secret(&mut route.api_key);
    }
    for route in &mut masked.embedding_routes {
        mask_optional_secret(&mut route.api_key);
    }

    if let Some(telegram) = masked.channels_config.telegram.as_mut() {
        mask_required_secret(&mut telegram.bot_token);
    }
    if let Some(discord) = masked.channels_config.discord.as_mut() {
        mask_required_secret(&mut discord.bot_token);
    }
    if let Some(slack) = masked.channels_config.slack.as_mut() {
        mask_required_secret(&mut slack.bot_token);
        mask_optional_secret(&mut slack.app_token);
    }
    if let Some(mattermost) = masked.channels_config.mattermost.as_mut() {
        mask_required_secret(&mut mattermost.bot_token);
    }
    if let Some(webhook) = masked.channels_config.webhook.as_mut() {
        mask_optional_secret(&mut webhook.secret);
    }
    if let Some(matrix) = masked.channels_config.matrix.as_mut() {
        mask_optional_secret(&mut matrix.access_token);
    }
    if let Some(whatsapp) = masked.channels_config.whatsapp.as_mut() {
        mask_optional_secret(&mut whatsapp.access_token);
        mask_optional_secret(&mut whatsapp.app_secret);
        mask_optional_secret(&mut whatsapp.verify_token);
    }
    if let Some(linq) = masked.channels_config.linq.as_mut() {
        mask_required_secret(&mut linq.api_token);
        mask_optional_secret(&mut linq.signing_secret);
    }
    if let Some(nextcloud) = masked.channels_config.nextcloud_talk.as_mut() {
        mask_required_secret(&mut nextcloud.app_token);
        mask_optional_secret(&mut nextcloud.webhook_secret);
    }
    if let Some(wati) = masked.channels_config.wati.as_mut() {
        mask_required_secret(&mut wati.api_token);
    }
    if let Some(irc) = masked.channels_config.irc.as_mut() {
        mask_optional_secret(&mut irc.server_password);
        mask_optional_secret(&mut irc.nickserv_password);
        mask_optional_secret(&mut irc.sasl_password);
    }
    if let Some(lark) = masked.channels_config.lark.as_mut() {
        mask_required_secret(&mut lark.app_secret);
        mask_optional_secret(&mut lark.encrypt_key);
        mask_optional_secret(&mut lark.verification_token);
    }
    if let Some(feishu) = masked.channels_config.feishu.as_mut() {
        mask_required_secret(&mut feishu.app_secret);
        mask_optional_secret(&mut feishu.encrypt_key);
        mask_optional_secret(&mut feishu.verification_token);
    }
    if let Some(dingtalk) = masked.channels_config.dingtalk.as_mut() {
        mask_required_secret(&mut dingtalk.client_secret);
    }
    if let Some(qq) = masked.channels_config.qq.as_mut() {
        mask_required_secret(&mut qq.app_secret);
    }
    #[cfg(feature = "channel-nostr")]
    if let Some(nostr) = masked.channels_config.nostr.as_mut() {
        mask_required_secret(&mut nostr.private_key);
    }
    if let Some(clawdtalk) = masked.channels_config.clawdtalk.as_mut() {
        mask_required_secret(&mut clawdtalk.api_key);
        mask_optional_secret(&mut clawdtalk.webhook_secret);
    }
    if let Some(email) = masked.channels_config.email.as_mut() {
        mask_required_secret(&mut email.password);
    }
    masked
}

fn restore_masked_sensitive_fields(
    incoming: &mut synapse_domain::config::schema::Config,
    current: &synapse_domain::config::schema::Config,
) {
    restore_optional_secret(&mut incoming.api_key, &current.api_key);
    restore_vec_secrets(
        &mut incoming.gateway.paired_tokens,
        &current.gateway.paired_tokens,
    );
    restore_vec_secrets(
        &mut incoming.reliability.api_keys,
        &current.reliability.api_keys,
    );
    restore_optional_secret(&mut incoming.composio.api_key, &current.composio.api_key);
    restore_optional_secret(
        &mut incoming.agents_ipc.broker_token,
        &current.agents_ipc.broker_token,
    );
    restore_optional_secret(
        &mut incoming.browser.computer_use.api_key,
        &current.browser.computer_use.api_key,
    );
    restore_optional_secret(
        &mut incoming.web_search.brave_api_key,
        &current.web_search.brave_api_key,
    );
    restore_optional_secret(
        &mut incoming.storage.provider.config.db_url,
        &current.storage.provider.config.db_url,
    );
    if let (Some(incoming_tunnel), Some(current_tunnel)) = (
        incoming.tunnel.cloudflare.as_mut(),
        current.tunnel.cloudflare.as_ref(),
    ) {
        restore_required_secret(&mut incoming_tunnel.token, &current_tunnel.token);
    }
    if let (Some(incoming_tunnel), Some(current_tunnel)) = (
        incoming.tunnel.ngrok.as_mut(),
        current.tunnel.ngrok.as_ref(),
    ) {
        restore_required_secret(&mut incoming_tunnel.auth_token, &current_tunnel.auth_token);
    }

    for (name, agent) in &mut incoming.agents {
        if let Some(current_agent) = current.agents.get(name) {
            restore_optional_secret(&mut agent.api_key, &current_agent.api_key);
        }
    }
    restore_model_route_api_keys(&mut incoming.model_routes, &current.model_routes);
    restore_embedding_route_api_keys(&mut incoming.embedding_routes, &current.embedding_routes);

    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.telegram.as_mut(),
        current.channels_config.telegram.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.bot_token, &current_ch.bot_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.discord.as_mut(),
        current.channels_config.discord.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.bot_token, &current_ch.bot_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.slack.as_mut(),
        current.channels_config.slack.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.bot_token, &current_ch.bot_token);
        restore_optional_secret(&mut incoming_ch.app_token, &current_ch.app_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.mattermost.as_mut(),
        current.channels_config.mattermost.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.bot_token, &current_ch.bot_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.webhook.as_mut(),
        current.channels_config.webhook.as_ref(),
    ) {
        restore_optional_secret(&mut incoming_ch.secret, &current_ch.secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.matrix.as_mut(),
        current.channels_config.matrix.as_ref(),
    ) {
        restore_optional_secret(&mut incoming_ch.access_token, &current_ch.access_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.whatsapp.as_mut(),
        current.channels_config.whatsapp.as_ref(),
    ) {
        restore_optional_secret(&mut incoming_ch.access_token, &current_ch.access_token);
        restore_optional_secret(&mut incoming_ch.app_secret, &current_ch.app_secret);
        restore_optional_secret(&mut incoming_ch.verify_token, &current_ch.verify_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.linq.as_mut(),
        current.channels_config.linq.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.api_token, &current_ch.api_token);
        restore_optional_secret(&mut incoming_ch.signing_secret, &current_ch.signing_secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.nextcloud_talk.as_mut(),
        current.channels_config.nextcloud_talk.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.app_token, &current_ch.app_token);
        restore_optional_secret(&mut incoming_ch.webhook_secret, &current_ch.webhook_secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.wati.as_mut(),
        current.channels_config.wati.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.api_token, &current_ch.api_token);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.irc.as_mut(),
        current.channels_config.irc.as_ref(),
    ) {
        restore_optional_secret(
            &mut incoming_ch.server_password,
            &current_ch.server_password,
        );
        restore_optional_secret(
            &mut incoming_ch.nickserv_password,
            &current_ch.nickserv_password,
        );
        restore_optional_secret(&mut incoming_ch.sasl_password, &current_ch.sasl_password);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.lark.as_mut(),
        current.channels_config.lark.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.app_secret, &current_ch.app_secret);
        restore_optional_secret(&mut incoming_ch.encrypt_key, &current_ch.encrypt_key);
        restore_optional_secret(
            &mut incoming_ch.verification_token,
            &current_ch.verification_token,
        );
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.feishu.as_mut(),
        current.channels_config.feishu.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.app_secret, &current_ch.app_secret);
        restore_optional_secret(&mut incoming_ch.encrypt_key, &current_ch.encrypt_key);
        restore_optional_secret(
            &mut incoming_ch.verification_token,
            &current_ch.verification_token,
        );
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.dingtalk.as_mut(),
        current.channels_config.dingtalk.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.client_secret, &current_ch.client_secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.qq.as_mut(),
        current.channels_config.qq.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.app_secret, &current_ch.app_secret);
    }
    #[cfg(feature = "channel-nostr")]
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.nostr.as_mut(),
        current.channels_config.nostr.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.private_key, &current_ch.private_key);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.clawdtalk.as_mut(),
        current.channels_config.clawdtalk.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.api_key, &current_ch.api_key);
        restore_optional_secret(&mut incoming_ch.webhook_secret, &current_ch.webhook_secret);
    }
    if let (Some(incoming_ch), Some(current_ch)) = (
        incoming.channels_config.email.as_mut(),
        current.channels_config.email.as_ref(),
    ) {
        restore_required_secret(&mut incoming_ch.password, &current_ch.password);
    }
}

fn hydrate_config_for_save(
    mut incoming: synapse_domain::config::schema::Config,
    current: &synapse_domain::config::schema::Config,
) -> synapse_domain::config::schema::Config {
    restore_masked_sensitive_fields(&mut incoming, current);
    // These are runtime-computed fields skipped from TOML serialization.
    incoming.config_path = current.config_path.clone();
    incoming.workspace_dir = current.workspace_dir.clone();
    incoming
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking_keeps_toml_valid_and_preserves_api_keys_type() {
        let mut cfg = synapse_domain::config::schema::Config::default();
        cfg.api_key = Some("sk-live-123".to_string());
        cfg.reliability.api_keys = vec!["rk-1".to_string(), "rk-2".to_string()];
        cfg.gateway.paired_tokens = vec!["pair-token-1".to_string()];
        cfg.tunnel.cloudflare = Some(synapse_domain::config::schema::CloudflareTunnelConfig {
            token: "cf-token".to_string(),
        });
        cfg.channels_config.wati = Some(synapse_domain::config::schema::WatiConfig {
            api_token: "wati-token".to_string(),
            api_url: "https://live-mt-server.wati.io".to_string(),
            tenant_id: None,
            allowed_numbers: vec![],
        });
        cfg.channels_config.feishu = Some(synapse_domain::config::schema::FeishuConfig {
            app_id: "cli_aabbcc".to_string(),
            app_secret: "feishu-secret".to_string(),
            encrypt_key: Some("feishu-encrypt".to_string()),
            verification_token: Some("feishu-verify".to_string()),
            allowed_users: vec!["*".to_string()],
            receive_mode: synapse_domain::config::schema::LarkReceiveMode::Websocket,
            port: None,
        });
        cfg.channels_config.email = Some(crate::channels::email_channel::EmailConfig {
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            imap_folder: "INBOX".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 465,
            smtp_tls: true,
            username: "agent@example.com".to_string(),
            password: "email-password-secret".to_string(),
            from_address: "agent@example.com".to_string(),
            idle_timeout_secs: 1740,
            allowed_senders: vec!["*".to_string()],
            default_subject: "SynapseClaw Message".to_string(),
        });
        cfg.model_routes = vec![synapse_domain::config::schema::ModelRouteConfig {
            hint: "reasoning".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-sonnet-4.6".to_string(),
            api_key: Some("route-model-key".to_string()),
        }];
        cfg.embedding_routes = vec![synapse_domain::config::schema::EmbeddingRouteConfig {
            hint: "semantic".to_string(),
            provider: "openai".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimensions: Some(1536),
            api_key: Some("route-embed-key".to_string()),
        }];

        let masked = mask_sensitive_fields(&cfg);
        let toml = toml::to_string_pretty(&masked).expect("masked config should serialize");
        let parsed: synapse_domain::config::schema::Config =
            toml::from_str(&toml).expect("masked config should remain valid TOML for Config");

        assert_eq!(parsed.api_key.as_deref(), Some(MASKED_SECRET));
        assert_eq!(
            parsed.reliability.api_keys,
            vec![MASKED_SECRET.to_string(), MASKED_SECRET.to_string()]
        );
        assert_eq!(
            parsed.gateway.paired_tokens,
            vec![MASKED_SECRET.to_string()]
        );
        assert_eq!(
            parsed.tunnel.cloudflare.as_ref().map(|v| v.token.as_str()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .channels_config
                .wati
                .as_ref()
                .map(|v| v.api_token.as_str()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .channels_config
                .feishu
                .as_ref()
                .map(|v| v.app_secret.as_str()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .channels_config
                .feishu
                .as_ref()
                .and_then(|v| v.encrypt_key.as_deref()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .channels_config
                .feishu
                .as_ref()
                .and_then(|v| v.verification_token.as_deref()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .model_routes
                .first()
                .and_then(|v| v.api_key.as_deref()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .embedding_routes
                .first()
                .and_then(|v| v.api_key.as_deref()),
            Some(MASKED_SECRET)
        );
        assert_eq!(
            parsed
                .channels_config
                .email
                .as_ref()
                .map(|v| v.password.as_str()),
            Some(MASKED_SECRET)
        );
    }

    #[test]
    fn hydrate_config_for_save_restores_masked_secrets_and_paths() {
        let mut current = synapse_domain::config::schema::Config::default();
        current.config_path = std::path::PathBuf::from("/tmp/current/config.toml");
        current.workspace_dir = std::path::PathBuf::from("/tmp/current/workspace");
        current.api_key = Some("real-key".to_string());
        current.reliability.api_keys = vec!["r1".to_string(), "r2".to_string()];
        current.gateway.paired_tokens = vec!["pair-1".to_string(), "pair-2".to_string()];
        current.tunnel.cloudflare = Some(synapse_domain::config::schema::CloudflareTunnelConfig {
            token: "cf-token-real".to_string(),
        });
        current.tunnel.ngrok = Some(synapse_domain::config::schema::NgrokTunnelConfig {
            auth_token: "ngrok-token-real".to_string(),
            domain: None,
        });
        current.channels_config.wati = Some(synapse_domain::config::schema::WatiConfig {
            api_token: "wati-real".to_string(),
            api_url: "https://live-mt-server.wati.io".to_string(),
            tenant_id: None,
            allowed_numbers: vec![],
        });
        current.channels_config.feishu = Some(synapse_domain::config::schema::FeishuConfig {
            app_id: "cli_current".to_string(),
            app_secret: "feishu-secret-real".to_string(),
            encrypt_key: Some("feishu-encrypt-real".to_string()),
            verification_token: Some("feishu-verify-real".to_string()),
            allowed_users: vec!["*".to_string()],
            receive_mode: synapse_domain::config::schema::LarkReceiveMode::Websocket,
            port: None,
        });
        current.channels_config.email = Some(crate::channels::email_channel::EmailConfig {
            imap_host: "imap.example.com".to_string(),
            imap_port: 993,
            imap_folder: "INBOX".to_string(),
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 465,
            smtp_tls: true,
            username: "agent@example.com".to_string(),
            password: "email-password-real".to_string(),
            from_address: "agent@example.com".to_string(),
            idle_timeout_secs: 1740,
            allowed_senders: vec!["*".to_string()],
            default_subject: "SynapseClaw Message".to_string(),
        });
        current.model_routes = vec![
            synapse_domain::config::schema::ModelRouteConfig {
                hint: "reasoning".to_string(),
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4.6".to_string(),
                api_key: Some("route-model-key-1".to_string()),
            },
            synapse_domain::config::schema::ModelRouteConfig {
                hint: "fast".to_string(),
                provider: "openrouter".to_string(),
                model: "openai/gpt-4.1-mini".to_string(),
                api_key: Some("route-model-key-2".to_string()),
            },
        ];
        current.embedding_routes = vec![
            synapse_domain::config::schema::EmbeddingRouteConfig {
                hint: "semantic".to_string(),
                provider: "openai".to_string(),
                model: "text-embedding-3-small".to_string(),
                dimensions: Some(1536),
                api_key: Some("route-embed-key-1".to_string()),
            },
            synapse_domain::config::schema::EmbeddingRouteConfig {
                hint: "archive".to_string(),
                provider: "custom:https://emb.example.com/v1".to_string(),
                model: "bge-m3".to_string(),
                dimensions: Some(1024),
                api_key: Some("route-embed-key-2".to_string()),
            },
        ];

        let mut incoming = mask_sensitive_fields(&current);
        incoming.default_model = Some("gpt-4.1-mini".to_string());
        // Simulate UI changing only one key and keeping the first masked.
        incoming.reliability.api_keys = vec![MASKED_SECRET.to_string(), "r2-new".to_string()];
        incoming.gateway.paired_tokens = vec![MASKED_SECRET.to_string(), "pair-2-new".to_string()];
        if let Some(cloudflare) = incoming.tunnel.cloudflare.as_mut() {
            cloudflare.token = MASKED_SECRET.to_string();
        }
        if let Some(ngrok) = incoming.tunnel.ngrok.as_mut() {
            ngrok.auth_token = MASKED_SECRET.to_string();
        }
        if let Some(wati) = incoming.channels_config.wati.as_mut() {
            wati.api_token = MASKED_SECRET.to_string();
        }
        if let Some(feishu) = incoming.channels_config.feishu.as_mut() {
            feishu.app_secret = MASKED_SECRET.to_string();
            feishu.encrypt_key = Some(MASKED_SECRET.to_string());
            feishu.verification_token = Some("feishu-verify-new".to_string());
        }
        if let Some(email) = incoming.channels_config.email.as_mut() {
            email.password = MASKED_SECRET.to_string();
        }
        incoming.model_routes[1].api_key = Some("route-model-key-2-new".to_string());
        incoming.embedding_routes[1].api_key = Some("route-embed-key-2-new".to_string());

        let hydrated = hydrate_config_for_save(incoming, &current);

        assert_eq!(hydrated.config_path, current.config_path);
        assert_eq!(hydrated.workspace_dir, current.workspace_dir);
        assert_eq!(hydrated.api_key, current.api_key);
        assert_eq!(hydrated.default_model.as_deref(), Some("gpt-4.1-mini"));
        assert_eq!(
            hydrated.reliability.api_keys,
            vec!["r1".to_string(), "r2-new".to_string()]
        );
        assert_eq!(
            hydrated.gateway.paired_tokens,
            vec!["pair-1".to_string(), "pair-2-new".to_string()]
        );
        assert_eq!(
            hydrated
                .tunnel
                .cloudflare
                .as_ref()
                .map(|v| v.token.as_str()),
            Some("cf-token-real")
        );
        assert_eq!(
            hydrated
                .tunnel
                .ngrok
                .as_ref()
                .map(|v| v.auth_token.as_str()),
            Some("ngrok-token-real")
        );
        assert_eq!(
            hydrated
                .channels_config
                .wati
                .as_ref()
                .map(|v| v.api_token.as_str()),
            Some("wati-real")
        );
        assert_eq!(
            hydrated
                .channels_config
                .feishu
                .as_ref()
                .map(|v| v.app_secret.as_str()),
            Some("feishu-secret-real")
        );
        assert_eq!(
            hydrated
                .channels_config
                .feishu
                .as_ref()
                .and_then(|v| v.encrypt_key.as_deref()),
            Some("feishu-encrypt-real")
        );
        assert_eq!(
            hydrated
                .channels_config
                .feishu
                .as_ref()
                .and_then(|v| v.verification_token.as_deref()),
            Some("feishu-verify-new")
        );
        assert_eq!(
            hydrated.model_routes[0].api_key.as_deref(),
            Some("route-model-key-1")
        );
        assert_eq!(
            hydrated.model_routes[1].api_key.as_deref(),
            Some("route-model-key-2-new")
        );
        assert_eq!(
            hydrated.embedding_routes[0].api_key.as_deref(),
            Some("route-embed-key-1")
        );
        assert_eq!(
            hydrated.embedding_routes[1].api_key.as_deref(),
            Some("route-embed-key-2-new")
        );
        assert_eq!(
            hydrated
                .channels_config
                .email
                .as_ref()
                .map(|v| v.password.as_str()),
            Some("email-password-real")
        );
    }

    #[test]
    fn hydrate_config_for_save_restores_route_keys_by_identity_and_clears_unmatched_masks() {
        let mut current = synapse_domain::config::schema::Config::default();
        current.model_routes = vec![
            synapse_domain::config::schema::ModelRouteConfig {
                hint: "reasoning".to_string(),
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4.6".to_string(),
                api_key: Some("route-model-key-1".to_string()),
            },
            synapse_domain::config::schema::ModelRouteConfig {
                hint: "fast".to_string(),
                provider: "openrouter".to_string(),
                model: "openai/gpt-4.1-mini".to_string(),
                api_key: Some("route-model-key-2".to_string()),
            },
        ];
        current.embedding_routes = vec![
            synapse_domain::config::schema::EmbeddingRouteConfig {
                hint: "semantic".to_string(),
                provider: "openai".to_string(),
                model: "text-embedding-3-small".to_string(),
                dimensions: Some(1536),
                api_key: Some("route-embed-key-1".to_string()),
            },
            synapse_domain::config::schema::EmbeddingRouteConfig {
                hint: "archive".to_string(),
                provider: "custom:https://emb.example.com/v1".to_string(),
                model: "bge-m3".to_string(),
                dimensions: Some(1024),
                api_key: Some("route-embed-key-2".to_string()),
            },
        ];

        let mut incoming = mask_sensitive_fields(&current);
        incoming.model_routes.swap(0, 1);
        incoming.embedding_routes.swap(0, 1);
        incoming
            .model_routes
            .push(synapse_domain::config::schema::ModelRouteConfig {
                hint: "new".to_string(),
                provider: "openai".to_string(),
                model: "gpt-4.1".to_string(),
                api_key: Some(MASKED_SECRET.to_string()),
            });
        incoming
            .embedding_routes
            .push(synapse_domain::config::schema::EmbeddingRouteConfig {
                hint: "new-embed".to_string(),
                provider: "custom:https://emb2.example.com/v1".to_string(),
                model: "bge-small".to_string(),
                dimensions: Some(768),
                api_key: Some(MASKED_SECRET.to_string()),
            });

        let hydrated = hydrate_config_for_save(incoming, &current);

        assert_eq!(
            hydrated.model_routes[0].api_key.as_deref(),
            Some("route-model-key-2")
        );
        assert_eq!(
            hydrated.model_routes[1].api_key.as_deref(),
            Some("route-model-key-1")
        );
        assert_eq!(hydrated.model_routes[2].api_key, None);
        assert_eq!(
            hydrated.embedding_routes[0].api_key.as_deref(),
            Some("route-embed-key-2")
        );
        assert_eq!(
            hydrated.embedding_routes[1].api_key.as_deref(),
            Some("route-embed-key-1")
        );
        assert_eq!(hydrated.embedding_routes[2].api_key, None);
        assert!(hydrated
            .model_routes
            .iter()
            .all(|route| route.api_key.as_deref() != Some(MASKED_SECRET)));
        assert!(hydrated
            .embedding_routes
            .iter()
            .all(|route| route.api_key.as_deref() != Some(MASKED_SECRET)));
    }
}
