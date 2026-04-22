use anyhow::Result;
use chrono::{DateTime, Utc};
use std::io::Write;
use std::path::Path;
use synapse_domain::application::services::auxiliary_model_resolution::{
    resolve_auxiliary_model, AuxiliaryLane, AuxiliaryModelResolutionError,
};
use synapse_domain::application::services::capability_doctor::CapabilityDoctorSeverity;
use synapse_domain::application::services::model_preset_resolution::resolve_effective_model_lanes;
use synapse_domain::config::schema::{CapabilityLane, Config};
use synapse_domain::ports::route_selection::RouteSelection;

const DAEMON_STALE_SECONDS: i64 = 30;
const SCHEDULER_STALE_SECONDS: i64 = 120;
const CHANNEL_STALE_SECONDS: i64 = 300;
const COMMAND_VERSION_PREVIEW_CHARS: usize = 60;

// ── Diagnostic item ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warn,
    Error,
}

/// Structured diagnostic result for programmatic consumption (web dashboard, API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiagResult {
    pub severity: Severity,
    pub category: String,
    pub message: String,
}

struct DiagItem {
    severity: Severity,
    category: &'static str,
    message: String,
}

impl DiagItem {
    fn ok(category: &'static str, msg: impl Into<String>) -> Self {
        Self {
            severity: Severity::Ok,
            category,
            message: msg.into(),
        }
    }
    fn warn(category: &'static str, msg: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warn,
            category,
            message: msg.into(),
        }
    }
    fn error(category: &'static str, msg: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            category,
            message: msg.into(),
        }
    }

    fn icon(&self) -> &'static str {
        match self.severity {
            Severity::Ok => "✅",
            Severity::Warn => "⚠️ ",
            Severity::Error => "❌",
        }
    }

    fn into_result(self) -> DiagResult {
        DiagResult {
            severity: self.severity,
            category: self.category.to_string(),
            message: self.message,
        }
    }
}

// ── Public entry points ──────────────────────────────────────────

/// Run diagnostics and return structured results (for API/web dashboard).
pub fn diagnose(config: &Config) -> Vec<DiagResult> {
    let mut items: Vec<DiagItem> = Vec::new();

    check_config_semantics(config, &mut items);
    check_runtime_capabilities(config, &mut items);
    check_workspace(config, &mut items);
    check_daemon_state(config, &mut items);
    check_environment(&mut items);
    check_cli_tools(&mut items);

    items.into_iter().map(DiagItem::into_result).collect()
}

fn check_runtime_capabilities(config: &Config, items: &mut Vec<DiagItem>) {
    let cat = "capability";
    let provider = config.default_provider.clone().unwrap_or_default();
    let model = config.default_model.clone().unwrap_or_default();
    let route = RouteSelection {
        provider: provider.clone(),
        model,
        lane: None,
        candidate_index: None,
        last_admission: None,
        recent_admissions: Vec::new(),
        last_tool_repair: None,
        recent_tool_repairs: Vec::new(),
        context_cache: None,
        assumptions: Vec::new(),
        calibrations: Vec::new(),
        watchdog_alerts: Vec::new(),
        handoff_artifacts: Vec::new(),
        runtime_decision_traces: Vec::new(),
        usage_ledger: Default::default(),
    };
    let provider_capabilities = if provider.trim().is_empty() {
        Default::default()
    } else {
        synapse_providers::create_provider(provider.as_str(), config.api_key.as_deref())
            .map(|provider| provider.capabilities())
            .unwrap_or_default()
    };
    let report = crate::runtime_routes::build_runtime_capability_doctor_report(
        crate::runtime_routes::RuntimeCapabilityDoctorInput {
            route: &route,
            config,
            provider_capabilities,
            provider_plan_denial: None,
            tool_registry_count: 0,
            memory_backend_name: None,
            memory_backend_healthy: None,
            memory_backend_configured: false,
            embedding_profile: None,
            channel_name: None,
            channel_available: None,
        },
    );

    for node in report.nodes {
        let readiness = crate::runtime_routes::capability_doctor_readiness_name(node.readiness);
        let subsystem = crate::runtime_routes::capability_doctor_subsystem_name(node.subsystem);
        let message = if let Some(recommendation) = node.recommendation {
            format!(
                "{subsystem} {}: {readiness} ({recommendation})",
                node.subject
            )
        } else {
            format!("{subsystem} {}: {readiness}", node.subject)
        };
        match node.severity {
            CapabilityDoctorSeverity::Ok => items.push(DiagItem::ok(cat, message)),
            CapabilityDoctorSeverity::Warn => items.push(DiagItem::warn(cat, message)),
            CapabilityDoctorSeverity::Error => items.push(DiagItem::error(cat, message)),
        }
    }
}

/// Run diagnostics and print human-readable report to stdout.
pub fn run(config: &Config) -> Result<()> {
    let results = diagnose(config);

    // Print report
    println!("🩺 SynapseClaw Doctor (enhanced)");
    println!();

    let mut current_cat = "";
    for item in &results {
        if item.category != current_cat {
            current_cat = &item.category;
            println!("  [{current_cat}]");
        }
        let icon = match item.severity {
            Severity::Ok => "✅",
            Severity::Warn => "⚠️ ",
            Severity::Error => "❌",
        };
        println!("    {} {}", icon, item.message);
    }

    let errors = results
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    let warns = results
        .iter()
        .filter(|i| i.severity == Severity::Warn)
        .count();
    let oks = results
        .iter()
        .filter(|i| i.severity == Severity::Ok)
        .count();

    println!();
    println!("  Summary: {oks} ok, {warns} warnings, {errors} errors");

    if errors > 0 {
        println!("  💡 Fix the errors above, then run `synapseclaw doctor` again.");
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelProbeOutcome {
    Ok,
    Skipped,
    AuthOrAccess,
    Error,
}

fn model_probe_status_label(outcome: ModelProbeOutcome) -> &'static str {
    match outcome {
        ModelProbeOutcome::Ok => "ok",
        ModelProbeOutcome::Skipped => "skipped",
        ModelProbeOutcome::AuthOrAccess => "auth/access",
        ModelProbeOutcome::Error => "error",
    }
}

fn classify_model_probe_error(err_message: &str) -> ModelProbeOutcome {
    let lower = err_message.to_lowercase();

    if lower.contains("does not support live model discovery") {
        return ModelProbeOutcome::Skipped;
    }

    if [
        "401",
        "403",
        "429",
        "unauthorized",
        "forbidden",
        "api key",
        "token",
        "insufficient balance",
        "insufficient quota",
        "plan does not include",
        "rate limit",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
    {
        return ModelProbeOutcome::AuthOrAccess;
    }

    ModelProbeOutcome::Error
}

fn doctor_model_targets(provider_override: Option<&str>) -> Vec<String> {
    if let Some(provider) = provider_override.map(str::trim).filter(|p| !p.is_empty()) {
        return vec![provider.to_string()];
    }

    synapse_providers::list_providers()
        .into_iter()
        .map(|provider| provider.name.to_string())
        .collect()
}

pub async fn run_models(
    config: &Config,
    provider_override: Option<&str>,
    use_cache: bool,
) -> Result<()> {
    let targets = doctor_model_targets(provider_override);

    if targets.is_empty() {
        anyhow::bail!("No providers available for model probing");
    }

    println!("🩺 SynapseClaw Doctor — Model Catalog Probe");
    println!("  Providers to probe: {}", targets.len());
    println!(
        "  Mode: {}",
        if use_cache {
            "cache-first"
        } else {
            "force live refresh"
        }
    );
    println!();

    let mut ok_count = 0usize;
    let mut skipped_count = 0usize;
    let mut auth_count = 0usize;
    let mut error_count = 0usize;
    let mut matrix_rows: Vec<(String, ModelProbeOutcome, Option<usize>, String)> = Vec::new();

    for provider_name in &targets {
        println!("  [{}]", provider_name);

        match synapse_onboard::run_models_refresh(config, Some(provider_name), !use_cache).await {
            Ok(()) => {
                ok_count += 1;
                println!("    ✅ model catalog check passed");
                let models_count =
                    synapse_onboard::wizard::cached_model_catalog_stats(config, provider_name)
                        .await?
                        .map(|(count, _)| count);
                matrix_rows.push((
                    provider_name.clone(),
                    ModelProbeOutcome::Ok,
                    models_count,
                    "catalog refreshed".to_string(),
                ));
            }
            Err(error) => {
                let error_text = format_error_chain(&error);
                match classify_model_probe_error(&error_text) {
                    ModelProbeOutcome::Skipped => {
                        skipped_count += 1;
                        println!("    ⚪ skipped: {}", truncate_for_display(&error_text, 160));
                        matrix_rows.push((
                            provider_name.clone(),
                            ModelProbeOutcome::Skipped,
                            None,
                            truncate_for_display(&error_text, 120),
                        ));
                    }
                    ModelProbeOutcome::AuthOrAccess => {
                        auth_count += 1;
                        println!(
                            "    ⚠️  auth/access: {}",
                            truncate_for_display(&error_text, 160)
                        );
                        matrix_rows.push((
                            provider_name.clone(),
                            ModelProbeOutcome::AuthOrAccess,
                            None,
                            truncate_for_display(&error_text, 120),
                        ));
                    }
                    ModelProbeOutcome::Error => {
                        error_count += 1;
                        println!("    ❌ error: {}", truncate_for_display(&error_text, 160));
                        matrix_rows.push((
                            provider_name.clone(),
                            ModelProbeOutcome::Error,
                            None,
                            truncate_for_display(&error_text, 120),
                        ));
                    }
                    ModelProbeOutcome::Ok => {
                        ok_count += 1;
                        matrix_rows.push((
                            provider_name.clone(),
                            ModelProbeOutcome::Ok,
                            None,
                            "catalog refreshed".to_string(),
                        ));
                    }
                }
            }
        }

        println!();
    }

    println!(
        "  Summary: {} ok, {} skipped, {} auth/access, {} errors",
        ok_count, skipped_count, auth_count, error_count
    );

    if !matrix_rows.is_empty() {
        println!();
        println!("  Connectivity matrix:");
        println!(
            "  {:<18} {:<12} {:<8} detail",
            "provider", "status", "models"
        );
        println!(
            "  {:<18} {:<12} {:<8} ------",
            "------------------", "------------", "--------"
        );
        for (provider, outcome, models_count, detail) in matrix_rows {
            let models_text = models_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "  {:<18} {:<12} {:<8} {}",
                provider,
                model_probe_status_label(outcome),
                models_text,
                detail
            );
        }
    }

    if auth_count > 0 {
        println!(
            "  💡 Some providers need valid API keys/plan access before `/models` can be fetched."
        );
    }

    if provider_override.is_some() && ok_count == 0 {
        anyhow::bail!("Model probe failed for target provider")
    }

    Ok(())
}

pub fn run_traces(
    config: &Config,
    id: Option<&str>,
    event_filter: Option<&str>,
    contains: Option<&str>,
    limit: usize,
) -> Result<()> {
    let path = synapse_observability::runtime_trace::resolve_trace_path(
        &config.observability,
        &config.workspace_dir,
    );

    if let Some(target_id) = id.map(str::trim).filter(|value| !value.is_empty()) {
        match synapse_observability::runtime_trace::find_event_by_id(&path, target_id)? {
            Some(event) => {
                println!("{}", serde_json::to_string_pretty(&event)?);
            }
            None => {
                println!(
                    "No runtime trace event found for id '{}' (path: {}).",
                    target_id,
                    path.display()
                );
            }
        }
        return Ok(());
    }

    if !path.exists() {
        println!(
            "Runtime trace file not found: {}.\n\
             Enable [observability] runtime_trace_mode = \"rolling\" or \"full\", then reproduce the issue.",
            path.display()
        );
        return Ok(());
    }

    let safe_limit = limit.max(1);
    let events = synapse_observability::runtime_trace::load_events(
        &path,
        safe_limit,
        event_filter,
        contains,
    )?;

    if events.is_empty() {
        println!(
            "No runtime trace events matched query (path: {}).",
            path.display()
        );
        return Ok(());
    }

    println!("Runtime traces (newest first)");
    println!("Path: {}", path.display());
    println!(
        "Filters: event={} contains={} limit={}",
        event_filter.unwrap_or("*"),
        contains.unwrap_or("*"),
        safe_limit
    );
    println!();

    for event in events {
        let success = match event.success {
            Some(true) => "ok",
            Some(false) => "fail",
            None => "-",
        };
        let message = event.message.unwrap_or_default();
        let preview = truncate_for_display(&message, 80);
        println!(
            "- {} | {} | {} | {} | {}",
            event.timestamp, event.id, event.event_type, success, preview
        );
    }

    println!();
    println!("Use `synapseclaw doctor traces --id <trace-id>` to inspect a full event payload.");
    Ok(())
}

// ── Config semantic validation ───────────────────────────────────

fn check_config_semantics(config: &Config, items: &mut Vec<DiagItem>) {
    let cat = "config";

    // Config file exists
    if config.config_path.exists() {
        items.push(DiagItem::ok(
            cat,
            format!("config file: {}", config.config_path.display()),
        ));
    } else {
        items.push(DiagItem::error(
            cat,
            format!("config file not found: {}", config.config_path.display()),
        ));
    }

    // Provider validity
    if let Some(ref provider) = config.default_provider {
        if let Some(reason) = provider_validation_error(provider) {
            items.push(DiagItem::error(
                cat,
                format!("default provider \"{provider}\" is invalid: {reason}"),
            ));
        } else {
            items.push(DiagItem::ok(
                cat,
                format!("provider \"{provider}\" is valid"),
            ));
        }
    } else {
        items.push(DiagItem::error(cat, "no default_provider configured"));
    }

    // API key presence
    if config.default_provider.as_deref() != Some("ollama") {
        if config.api_key.is_some() {
            items.push(DiagItem::ok(cat, "API key configured"));
        } else {
            items.push(DiagItem::warn(
                cat,
                "no api_key set (may rely on env vars or provider defaults)",
            ));
        }
    }

    // Model configured
    if config.default_model.is_some() {
        items.push(DiagItem::ok(
            cat,
            format!(
                "default model: {}",
                config.default_model.as_deref().unwrap_or("?")
            ),
        ));
    } else {
        items.push(DiagItem::warn(cat, "no default_model configured"));
    }

    check_primary_reasoning_lane(config, items);
    check_auxiliary_model_lanes(config, items);

    // Temperature range
    if config.default_temperature >= 0.0 && config.default_temperature <= 2.0 {
        items.push(DiagItem::ok(
            cat,
            format!(
                "temperature {:.1} (valid range 0.0–2.0)",
                config.default_temperature
            ),
        ));
    } else {
        items.push(DiagItem::error(
            cat,
            format!(
                "temperature {:.1} is out of range (expected 0.0–2.0)",
                config.default_temperature
            ),
        ));
    }

    // Gateway port range
    let port = config.gateway.port;
    if port > 0 {
        items.push(DiagItem::ok(cat, format!("gateway port: {port}")));
    } else {
        items.push(DiagItem::error(cat, "gateway port is 0 (invalid)"));
    }

    // Route aliases validation
    for route in &config.route_aliases {
        if route.hint.is_empty() {
            items.push(DiagItem::warn(cat, "route alias with empty hint"));
        }
        if let Some(reason) = provider_validation_error(&route.provider) {
            items.push(DiagItem::warn(
                cat,
                format!(
                    "route alias \"{}\" uses invalid provider \"{}\": {}",
                    route.hint, route.provider, reason
                ),
            ));
        }
        if route.model.is_empty() {
            items.push(DiagItem::warn(
                cat,
                format!("route alias \"{}\" has empty model", route.hint),
            ));
        }
    }

    // Channel: at least one configured
    let cc = &config.channels_config;
    let has_channel = cc.channels().iter().any(|(_, ok)| *ok);

    if has_channel {
        items.push(DiagItem::ok(cat, "at least one channel configured"));
    } else {
        items.push(DiagItem::warn(
            cat,
            "no channels configured — run `synapseclaw onboard` to set one up",
        ));
    }

    // Delegate agents: provider validity
    let mut agent_names: Vec<_> = config.agents.keys().collect();
    agent_names.sort();
    for name in agent_names {
        let agent = config.agents.get(name).unwrap();
        if let Some(reason) = provider_validation_error(&agent.provider) {
            items.push(DiagItem::warn(
                cat,
                format!(
                    "agent \"{name}\" uses invalid provider \"{}\": {}",
                    agent.provider, reason
                ),
            ));
        }
    }
}

fn check_primary_reasoning_lane(config: &Config, items: &mut Vec<DiagItem>) {
    let cat = "config";
    let Some(lane) = resolve_effective_model_lanes(config)
        .into_iter()
        .find(|lane| lane.lane == CapabilityLane::Reasoning)
    else {
        return;
    };
    let Some(first) = lane.candidates.first() else {
        items.push(DiagItem::warn(
            cat,
            "primary reasoning lane is configured without candidates",
        ));
        return;
    };

    let default_provider = config.default_provider.as_deref().unwrap_or("");
    let default_model = config.default_model.as_deref().unwrap_or("");
    let message = format!(
        "primary reasoning lane: {}/{} selected (candidate #0 of {})",
        first.provider,
        first.model,
        lane.candidates.len()
    );

    if first.provider == default_provider && first.model == default_model {
        if lane.candidates.len() > 1 {
            items.push(DiagItem::ok(
                cat,
                format!("{message}; failover candidates available"),
            ));
        } else {
            items.push(DiagItem::ok(
                cat,
                format!("{message}; no failover candidate"),
            ));
        }
    } else {
        items.push(DiagItem::warn(
            cat,
            format!(
                "{message}; first candidate does not match default_provider/default_model, so normal turns will not use this failover chain"
            ),
        ));
    }
}

fn check_auxiliary_model_lanes(config: &Config, items: &mut Vec<DiagItem>) {
    const CORE_LANES: [AuxiliaryLane; 2] = [AuxiliaryLane::Compaction, AuxiliaryLane::Embedding];
    const OPTIONAL_LANES: [AuxiliaryLane; 10] = [
        AuxiliaryLane::CheapReasoning,
        AuxiliaryLane::WebExtraction,
        AuxiliaryLane::ToolValidator,
        AuxiliaryLane::MultimodalUnderstanding,
        AuxiliaryLane::SpeechTranscription,
        AuxiliaryLane::SpeechSynthesis,
        AuxiliaryLane::ImageGeneration,
        AuxiliaryLane::AudioGeneration,
        AuxiliaryLane::VideoGeneration,
        AuxiliaryLane::MusicGeneration,
    ];

    for lane in CORE_LANES {
        check_auxiliary_model_lane(config, items, lane, true);
    }
    for lane in OPTIONAL_LANES {
        check_auxiliary_model_lane(config, items, lane, false);
    }
}

fn check_auxiliary_model_lane(
    config: &Config,
    items: &mut Vec<DiagItem>,
    lane: AuxiliaryLane,
    report_missing: bool,
) {
    let cat = "config";

    match resolve_auxiliary_model(config, lane, None) {
        Ok(resolution) => {
            let selected = &resolution.selected;
            let dimensions = selected
                .dimensions
                .map(|value| format!(", dimensions={value}"))
                .unwrap_or_default();
            let message = format!(
                "auxiliary lane \"{}\": {}/{} selected (candidate #{} of {}{})",
                lane.as_str(),
                selected.provider,
                selected.model,
                resolution.selected_index,
                resolution.candidates.len(),
                dimensions
            );

            if lane == AuxiliaryLane::Embedding && selected.dimensions.unwrap_or_default() == 0 {
                items.push(DiagItem::warn(
                    cat,
                    format!("{message}; embedding dimensions must be explicit"),
                ));
            } else {
                items.push(DiagItem::ok(cat, message));
            }
        }
        Err(AuxiliaryModelResolutionError::LaneNotConfigured { .. }) if !report_missing => {}
        Err(AuxiliaryModelResolutionError::LaneNotConfigured { .. }) => {
            items.push(DiagItem::warn(
                cat,
                format!(
                    "auxiliary lane \"{}\" is not configured in [[model_lanes]]",
                    lane.as_str()
                ),
            ));
        }
        Err(AuxiliaryModelResolutionError::NoSupportedCandidate { candidates, .. }) => {
            let rejected = candidates
                .iter()
                .take(3)
                .map(|candidate| {
                    let reason = candidate
                        .skip_reason
                        .as_ref()
                        .map(|reason| reason.as_str())
                        .unwrap_or("not_selected");
                    format!(
                        "#{} {}/{}: {}",
                        candidate.index, candidate.provider, candidate.model, reason
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let suffix = if rejected.is_empty() {
                String::new()
            } else {
                format!(" ({rejected})")
            };
            items.push(DiagItem::warn(
                cat,
                format!(
                    "auxiliary lane \"{}\" has no supported candidate{}",
                    lane.as_str(),
                    suffix
                ),
            ));
        }
    }
}

fn provider_validation_error(name: &str) -> Option<String> {
    match synapse_providers::create_provider(name, None) {
        Ok(_) => None,
        Err(err) => Some(
            err.to_string()
                .lines()
                .next()
                .unwrap_or("invalid provider")
                .into(),
        ),
    }
}

// ── Workspace integrity ──────────────────────────────────────────

fn check_workspace(config: &Config, items: &mut Vec<DiagItem>) {
    let cat = "workspace";
    let ws = &config.workspace_dir;

    if ws.exists() {
        items.push(DiagItem::ok(
            cat,
            format!("directory exists: {}", ws.display()),
        ));
    } else {
        items.push(DiagItem::error(
            cat,
            format!("directory missing: {}", ws.display()),
        ));
        return;
    }

    // Writable check
    let probe = workspace_probe_path(ws);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(mut probe_file) => {
            let write_result = probe_file.write_all(b"probe");
            drop(probe_file);
            let _ = std::fs::remove_file(&probe);
            match write_result {
                Ok(()) => items.push(DiagItem::ok(cat, "directory is writable")),
                Err(e) => items.push(DiagItem::error(
                    cat,
                    format!("directory write probe failed: {e}"),
                )),
            }
        }
        Err(e) => {
            items.push(DiagItem::error(
                cat,
                format!("directory is not writable: {e}"),
            ));
        }
    }

    // Disk space (best-effort via `df`)
    if let Some(avail_mb) = disk_available_mb(ws) {
        if avail_mb >= 100 {
            items.push(DiagItem::ok(
                cat,
                format!("disk space: {avail_mb} MB available"),
            ));
        } else {
            items.push(DiagItem::warn(
                cat,
                format!("low disk space: only {avail_mb} MB available"),
            ));
        }
    }

    // Key workspace files
    check_file_exists(ws, "SOUL.md", false, cat, items);
    check_file_exists(ws, "AGENTS.md", false, cat, items);
}

fn check_file_exists(
    base: &Path,
    name: &str,
    required: bool,
    cat: &'static str,
    items: &mut Vec<DiagItem>,
) {
    let path = base.join(name);
    if path.is_file() {
        items.push(DiagItem::ok(cat, format!("{name} present")));
    } else if required {
        items.push(DiagItem::error(cat, format!("{name} missing")));
    } else {
        items.push(DiagItem::warn(cat, format!("{name} not found (optional)")));
    }
}

fn disk_available_mb(path: &Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-m")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_df_available_mb(&stdout)
}

fn parse_df_available_mb(stdout: &str) -> Option<u64> {
    let line = stdout.lines().rev().find(|line| !line.trim().is_empty())?;
    let avail = line.split_whitespace().nth(3)?;
    avail.parse::<u64>().ok()
}

fn workspace_probe_path(workspace_dir: &Path) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    workspace_dir.join(format!(
        ".synapseclaw_doctor_probe_{}_{}",
        std::process::id(),
        nanos
    ))
}

// ── Daemon state (original logic, preserved) ─────────────────────

fn check_daemon_state(config: &Config, items: &mut Vec<DiagItem>) {
    let cat = "daemon";
    let state_file = crate::daemon::state_file_path(config);

    if !state_file.exists() {
        items.push(DiagItem::error(
            cat,
            format!(
                "state file not found: {} — is the daemon running?",
                state_file.display()
            ),
        ));
        return;
    }

    let raw = match std::fs::read_to_string(&state_file) {
        Ok(r) => r,
        Err(e) => {
            items.push(DiagItem::error(cat, format!("cannot read state file: {e}")));
            return;
        }
    };

    let snapshot: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            items.push(DiagItem::error(cat, format!("invalid state JSON: {e}")));
            return;
        }
    };

    // Daemon heartbeat freshness
    let updated_at = snapshot
        .get("updated_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    if let Ok(ts) = DateTime::parse_from_rfc3339(updated_at) {
        let age = Utc::now()
            .signed_duration_since(ts.with_timezone(&Utc))
            .num_seconds();
        if age <= DAEMON_STALE_SECONDS {
            items.push(DiagItem::ok(cat, format!("heartbeat fresh ({age}s ago)")));
        } else {
            items.push(DiagItem::error(
                cat,
                format!("heartbeat stale ({age}s ago)"),
            ));
        }
    } else {
        items.push(DiagItem::error(
            cat,
            format!("invalid daemon timestamp: {updated_at}"),
        ));
    }

    // Components
    if let Some(components) = snapshot
        .get("components")
        .and_then(serde_json::Value::as_object)
    {
        // Scheduler
        if let Some(scheduler) = components.get("scheduler") {
            let scheduler_ok = scheduler
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| s == "ok");
            let scheduler_age = scheduler
                .get("last_ok")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_rfc3339)
                .map_or(i64::MAX, |dt| {
                    Utc::now().signed_duration_since(dt).num_seconds()
                });

            if scheduler_ok && scheduler_age <= SCHEDULER_STALE_SECONDS {
                items.push(DiagItem::ok(
                    cat,
                    format!("scheduler healthy (last ok {scheduler_age}s ago)"),
                ));
            } else {
                items.push(DiagItem::error(
                    cat,
                    format!("scheduler unhealthy (ok={scheduler_ok}, age={scheduler_age}s)"),
                ));
            }
        } else {
            items.push(DiagItem::warn(cat, "scheduler component not tracked yet"));
        }

        // Channels
        let mut channel_count = 0u32;
        let mut stale = 0u32;
        for (name, component) in components {
            if !name.starts_with("channel:") {
                continue;
            }
            channel_count += 1;
            let status_ok = component
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| s == "ok");
            let age = component
                .get("last_ok")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_rfc3339)
                .map_or(i64::MAX, |dt| {
                    Utc::now().signed_duration_since(dt).num_seconds()
                });

            if status_ok && age <= CHANNEL_STALE_SECONDS {
                items.push(DiagItem::ok(cat, format!("{name} fresh ({age}s ago)")));
            } else {
                stale += 1;
                items.push(DiagItem::error(
                    cat,
                    format!("{name} stale (ok={status_ok}, age={age}s)"),
                ));
            }
        }

        if channel_count == 0 {
            items.push(DiagItem::warn(cat, "no channel components tracked yet"));
        } else if stale > 0 {
            items.push(DiagItem::warn(
                cat,
                format!("{channel_count} channels, {stale} stale"),
            ));
        }
    }
}

// ── Environment checks ───────────────────────────────────────────

fn check_environment(items: &mut Vec<DiagItem>) {
    let cat = "environment";

    // git
    check_command_available("git", &["--version"], cat, items);

    // Shell
    let shell = std::env::var("SHELL").unwrap_or_default();
    if shell.is_empty() {
        items.push(DiagItem::warn(cat, "$SHELL not set"));
    } else {
        items.push(DiagItem::ok(cat, format!("shell: {shell}")));
    }

    // HOME
    if std::env::var("HOME").is_ok() || std::env::var("USERPROFILE").is_ok() {
        items.push(DiagItem::ok(cat, "home directory env set"));
    } else {
        items.push(DiagItem::error(
            cat,
            "neither $HOME nor $USERPROFILE is set",
        ));
    }

    // Optional tools
    check_command_available("curl", &["--version"], cat, items);
}

fn check_cli_tools(items: &mut Vec<DiagItem>) {
    let cat = "cli-tools";

    let discovered = crate::tools::cli_discovery::discover_cli_tools(&[], &[]);

    if discovered.is_empty() {
        items.push(DiagItem::warn(cat, "No CLI tools found in PATH"));
    } else {
        for cli in &discovered {
            let version_info = cli
                .version
                .as_deref()
                .map(|v| truncate_for_display(v, COMMAND_VERSION_PREVIEW_CHARS))
                .unwrap_or_else(|| "unknown version".to_string());
            items.push(DiagItem::ok(
                cat,
                format!("{} ({}) — {}", cli.name, cli.category, version_info),
            ));
        }
        items.push(DiagItem::ok(
            cat,
            format!("{} CLI tools discovered", discovered.len()),
        ));
    }
}

fn check_command_available(cmd: &str, args: &[&str], cat: &'static str, items: &mut Vec<DiagItem>) {
    match std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(output) if output.status.success() => {
            let ver = String::from_utf8_lossy(&output.stdout);
            let first_line = ver.lines().next().unwrap_or("").trim();
            let display = truncate_for_display(first_line, COMMAND_VERSION_PREVIEW_CHARS);
            items.push(DiagItem::ok(cat, format!("{cmd}: {display}")));
        }
        Ok(_) => {
            items.push(DiagItem::warn(
                cat,
                format!("{cmd} found but returned non-zero"),
            ));
        }
        Err(_) => {
            items.push(DiagItem::warn(cat, format!("{cmd} not found in PATH")));
        }
    }
}

fn format_error_chain(error: &anyhow::Error) -> String {
    let mut parts = Vec::new();
    for cause in error.chain() {
        let message = cause.to_string();
        if !message.is_empty() {
            parts.push(message);
        }
    }

    if parts.is_empty() {
        return String::new();
    }

    parts.join(": ")
}

fn truncate_for_display(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let preview: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

// ── Helpers ──────────────────────────────────────────────────────

fn parse_rfc3339(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn provider_validation_checks_custom_url_shape() {
        assert!(provider_validation_error("openrouter").is_none());
        assert!(provider_validation_error("custom:https://example.com").is_none());
        assert!(provider_validation_error("anthropic-custom:https://example.com").is_none());

        let invalid_custom = provider_validation_error("custom:").unwrap_or_default();
        assert!(invalid_custom.contains("requires a URL"));

        let invalid_unknown = provider_validation_error("totally-fake").unwrap_or_default();
        assert!(invalid_unknown.contains("Unknown provider"));
    }

    #[test]
    fn diag_item_icons() {
        assert_eq!(DiagItem::ok("t", "m").icon(), "✅");
        assert_eq!(DiagItem::warn("t", "m").icon(), "⚠️ ");
        assert_eq!(DiagItem::error("t", "m").icon(), "❌");
    }

    #[test]
    fn classify_model_probe_error_marks_unsupported_as_skipped() {
        let outcome = classify_model_probe_error(
            "Provider 'copilot' does not support live model discovery yet",
        );
        assert_eq!(outcome, ModelProbeOutcome::Skipped);
    }

    #[test]
    fn classify_model_probe_error_marks_auth_and_plan_issues() {
        let auth_outcome = classify_model_probe_error("OpenAI API error (401): unauthorized");
        assert_eq!(auth_outcome, ModelProbeOutcome::AuthOrAccess);

        let plan_outcome = classify_model_probe_error(
            "Z.AI API error (429): plan does not include requested model",
        );
        assert_eq!(plan_outcome, ModelProbeOutcome::AuthOrAccess);
    }

    #[test]
    fn config_validation_catches_bad_temperature() {
        let mut config = Config::default();
        config.default_temperature = 5.0;
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);
        let temp_item = items.iter().find(|i| i.message.contains("temperature"));
        assert!(temp_item.is_some());
        assert_eq!(temp_item.unwrap().severity, Severity::Error);
    }

    #[test]
    fn config_validation_accepts_valid_temperature() {
        let mut config = Config::default();
        config.default_temperature = 0.7;
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);
        let temp_item = items.iter().find(|i| i.message.contains("temperature"));
        assert!(temp_item.is_some());
        assert_eq!(temp_item.unwrap().severity, Severity::Ok);
    }

    #[test]
    fn config_validation_reports_auxiliary_lane_selection() {
        let mut config = Config::default();
        config.model_lanes = vec![
            synapse_domain::config::schema::ModelLaneConfig {
                lane: synapse_domain::config::schema::CapabilityLane::Compaction,
                candidates: vec![synapse_domain::config::schema::ModelLaneCandidateConfig {
                    provider: "openrouter".into(),
                    model: "qwen/qwen3.6-plus".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: Default::default(),
                }],
            },
            synapse_domain::config::schema::ModelLaneConfig {
                lane: synapse_domain::config::schema::CapabilityLane::Embedding,
                candidates: vec![synapse_domain::config::schema::ModelLaneCandidateConfig {
                    provider: "openrouter".into(),
                    model: "qwen/qwen3-embedding-8b".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: Some(4096),
                    profile: Default::default(),
                }],
            },
        ];

        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);

        assert!(items.iter().any(|item| {
            item.severity == Severity::Ok
                && item.message.contains("auxiliary lane \"compaction\"")
                && item.message.contains("qwen/qwen3.6-plus")
        }));
        assert!(items.iter().any(|item| {
            item.severity == Severity::Ok
                && item.message.contains("auxiliary lane \"embedding\"")
                && item.message.contains("dimensions=4096")
        }));
    }

    #[test]
    fn config_validation_reports_primary_reasoning_failover_candidates() {
        let mut config = Config::default();
        config.default_provider = Some("openai-codex".into());
        config.default_model = Some("gpt-5.4".into());
        config.model_lanes = vec![synapse_domain::config::schema::ModelLaneConfig {
            lane: synapse_domain::config::schema::CapabilityLane::Reasoning,
            candidates: vec![
                synapse_domain::config::schema::ModelLaneCandidateConfig {
                    provider: "openai-codex".into(),
                    model: "gpt-5.4".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: Default::default(),
                },
                synapse_domain::config::schema::ModelLaneCandidateConfig {
                    provider: "anthropic".into(),
                    model: "claude-sonnet-4-6".into(),
                    api_key: None,
                    api_key_env: Some("ANTHROPIC_API_KEY".into()),
                    dimensions: None,
                    profile: Default::default(),
                },
            ],
        }];

        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);

        assert!(items.iter().any(|item| {
            item.severity == Severity::Ok
                && item.message.contains("primary reasoning lane")
                && item.message.contains("candidate #0 of 2")
                && item.message.contains("failover candidates available")
        }));
    }

    #[test]
    fn config_validation_warns_when_core_auxiliary_lane_is_missing() {
        let config = Config::default();
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);

        assert!(items.iter().any(|item| {
            item.severity == Severity::Warn
                && item
                    .message
                    .contains("auxiliary lane \"embedding\" is not configured")
        }));
    }

    #[test]
    fn config_validation_reports_optional_auxiliary_lanes_from_preset() {
        let mut config = Config::default();
        config.model_preset = Some("openrouter".into());
        config.default_provider = Some("openrouter".into());
        config.default_model = Some("anthropic/claude-sonnet-4-6".into());

        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);

        assert!(items.iter().any(|item| {
            item.severity == Severity::Ok
                && item.message.contains("auxiliary lane \"cheap_reasoning\"")
        }));
        assert!(items.iter().any(|item| {
            item.severity == Severity::Ok
                && item.message.contains("auxiliary lane \"image_generation\"")
        }));
    }

    #[test]
    fn config_validation_warns_no_channels() {
        let config = Config::default();
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);
        let ch_item = items.iter().find(|i| i.message.contains("channel"));
        assert!(ch_item.is_some());
        assert_eq!(ch_item.unwrap().severity, Severity::Warn);
    }

    #[test]
    fn config_validation_catches_unknown_provider() {
        let mut config = Config::default();
        config.default_provider = Some("totally-fake".into());
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);
        let prov_item = items
            .iter()
            .find(|i| i.message.contains("default provider"));
        assert!(prov_item.is_some());
        assert_eq!(prov_item.unwrap().severity, Severity::Error);
    }

    #[test]
    fn config_validation_catches_malformed_custom_provider() {
        let mut config = Config::default();
        config.default_provider = Some("custom:".into());
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);

        let prov_item = items.iter().find(|item| {
            item.message
                .contains("default provider \"custom:\" is invalid")
        });
        assert!(prov_item.is_some());
        assert_eq!(prov_item.unwrap().severity, Severity::Error);
    }

    #[test]
    fn config_validation_accepts_custom_provider() {
        let mut config = Config::default();
        config.default_provider = Some("custom:https://my-api.com".into());
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);
        let prov_item = items.iter().find(|i| i.message.contains("is valid"));
        assert!(prov_item.is_some());
        assert_eq!(prov_item.unwrap().severity, Severity::Ok);
    }

    #[test]
    fn config_validation_warns_empty_route_alias() {
        let mut config = Config::default();
        config.route_aliases = vec![synapse_domain::config::schema::ModelRouteConfig {
            hint: "fast".into(),
            provider: "groq".into(),
            model: String::new(),
            api_key: None,
            capability: None,
            profile: Default::default(),
        }];
        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);
        let route_item = items
            .iter()
            .find(|i| i.message.contains("route alias") && i.message.contains("empty model"));
        assert!(route_item.is_some());
        assert_eq!(route_item.unwrap().severity, Severity::Warn);
    }

    #[test]
    fn environment_check_finds_git() {
        let mut items = Vec::new();
        check_environment(&mut items);
        let git_item = items.iter().find(|i| i.message.starts_with("git:"));
        // git should be available in any CI/dev environment
        assert!(git_item.is_some());
        assert_eq!(git_item.unwrap().severity, Severity::Ok);
    }

    #[test]
    fn parse_df_available_mb_uses_last_data_line() {
        let stdout =
            "Filesystem 1M-blocks Used Available Use% Mounted on\n/dev/sda1 1000 500 500 50% /\n";
        assert_eq!(parse_df_available_mb(stdout), Some(500));
    }

    #[test]
    fn truncate_for_display_preserves_utf8_boundaries() {
        let preview = truncate_for_display("🙂example-alpha-build", 3);
        assert_eq!(preview, "🙂ex…");
    }

    #[test]
    fn workspace_probe_path_is_hidden_and_unique() {
        let tmp = TempDir::new().unwrap();
        let first = workspace_probe_path(tmp.path());
        let second = workspace_probe_path(tmp.path());

        assert_ne!(first, second);
        assert!(first
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".synapseclaw_doctor_probe_")));
    }

    #[test]
    fn config_validation_reports_delegate_agents_in_sorted_order() {
        let mut config = Config::default();
        config.agents.insert(
            "zeta".into(),
            synapse_domain::config::schema::DelegateAgentConfig {
                provider: "totally-fake".into(),
                model: "model-z".into(),
                system_prompt: None,
                api_key: None,
                temperature: None,
                max_depth: 3,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: 10,
            },
        );
        config.agents.insert(
            "alpha".into(),
            synapse_domain::config::schema::DelegateAgentConfig {
                provider: "totally-fake".into(),
                model: "model-a".into(),
                system_prompt: None,
                api_key: None,
                temperature: None,
                max_depth: 3,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: 10,
            },
        );

        let mut items = Vec::new();
        check_config_semantics(&config, &mut items);

        let agent_messages: Vec<_> = items
            .iter()
            .filter(|item| item.message.starts_with("agent \""))
            .map(|item| item.message.as_str())
            .collect();

        assert_eq!(agent_messages.len(), 2);
        assert!(agent_messages[0].contains("agent \"alpha\""));
        assert!(agent_messages[1].contains("agent \"zeta\""));
    }
}
