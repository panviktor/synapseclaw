#![allow(dead_code)]

use anyhow::{anyhow, bail, Result};
pub use surrealdb::engine::local::Db;
pub use surrealdb::Surreal;
use synapse_domain::config::schema::Config;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_security::security_factory::security_policy_from_config;

pub mod commands;
mod schedule;
mod store;
mod types;

pub mod scheduler;

#[allow(unused_imports)]
pub use schedule::{
    next_run_for_schedule, normalize_expression, schedule_cron_expression, validate_schedule,
};
#[allow(unused_imports)]
pub use store::{
    add_agent_job, add_agent_job_full, add_job, due_jobs, get_job, list_jobs, list_runs,
    record_last_run, record_run, remove_job, reschedule_after_run, update_job,
};
pub use types::{
    CronJob, CronJobPatch, CronRun, DeliveryConfig, ExecutionMode, JobType, Schedule, SessionTarget,
};

/// Validate a shell command against the full security policy (allowlist + risk gate).
///
/// Returns `Ok(())` if the command passes all checks, or an error describing
/// why it was blocked.
pub fn validate_shell_command(config: &Config, command: &str, approved: bool) -> Result<()> {
    let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);
    validate_shell_command_with_security(&security, command, approved)
}

/// Validate a shell command using an existing `SecurityPolicy` instance.
///
/// Preferred when the caller already holds a `SecurityPolicy` (e.g. scheduler).
pub fn validate_shell_command_with_security(
    security: &SecurityPolicy,
    command: &str,
    approved: bool,
) -> Result<()> {
    security
        .validate_command_execution(command, approved)
        .map(|_| ())
        .map_err(|reason| anyhow!("blocked by security policy: {reason}"))
}

/// Create a validated shell job, enforcing security policy before persistence.
///
/// All entrypoints that create shell cron jobs should route through this
/// function to guarantee consistent policy enforcement.
pub async fn add_shell_job_with_approval(
    db: &Surreal<Db>,
    config: &Config,
    name: Option<String>,
    schedule: Schedule,
    command: &str,
    approved: bool,
) -> Result<CronJob> {
    validate_shell_command(config, command, approved)?;
    store::add_shell_job(db, name, schedule, command).await
}

/// Update a shell job's command with security validation.
///
/// Validates the new command (if changed) before persisting.
pub async fn update_shell_job_with_approval(
    db: &Surreal<Db>,
    config: &Config,
    job_id: &str,
    patch: CronJobPatch,
    approved: bool,
) -> Result<CronJob> {
    if let Some(command) = patch.command.as_deref() {
        validate_shell_command(config, command, approved)?;
    }
    update_job(db, job_id, patch).await
}

/// Create a one-shot validated shell job from a delay string (e.g. "30m").
use commands::parse_delay;

/// Convenience wrapper -- creates a shell job with default `approved=false`.
/// This validates the command against the security policy before persisting.
pub async fn add_shell_job(
    db: &Surreal<Db>,
    config: &Config,
    name: Option<String>,
    schedule: Schedule,
    command: &str,
) -> Result<CronJob> {
    add_shell_job_with_approval(db, config, name, schedule, command, false).await
}

pub async fn add_once_validated(
    db: &Surreal<Db>,
    config: &Config,
    delay: &str,
    command: &str,
    approved: bool,
) -> Result<CronJob> {
    let duration = parse_delay(delay)?;
    let at = chrono::Utc::now() + duration;
    add_once_at_validated(db, config, at, command, approved).await
}

/// Create a one-shot validated shell job at an absolute timestamp.
pub async fn add_once_at_validated(
    db: &Surreal<Db>,
    config: &Config,
    at: chrono::DateTime<chrono::Utc>,
    command: &str,
    approved: bool,
) -> Result<CronJob> {
    let schedule = Schedule::At { at };
    add_shell_job_with_approval(db, config, None, schedule, command, approved).await
}

pub async fn pause_job(db: &Surreal<Db>, id: &str) -> Result<CronJob> {
    update_job(
        db,
        id,
        CronJobPatch {
            enabled: Some(false),
            ..CronJobPatch::default()
        },
    )
    .await
}

pub async fn resume_job(db: &Surreal<Db>, id: &str) -> Result<CronJob> {
    update_job(
        db,
        id,
        CronJobPatch {
            enabled: Some(true),
            ..CronJobPatch::default()
        },
    )
    .await
}

/// Convert a cron `DeliveryConfig` to the domain `CronDeliveryConfig`.
pub fn cron_delivery_config_from(
    delivery: &DeliveryConfig,
) -> synapse_domain::domain::config::CronDeliveryConfig {
    synapse_domain::domain::config::CronDeliveryConfig {
        mode: delivery.mode.clone(),
        channel: delivery.channel.clone(),
        to: delivery.to.clone(),
    }
}
