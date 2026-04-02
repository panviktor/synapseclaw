use crate::{
    next_run_for_schedule, schedule_cron_expression, validate_schedule, CronJob, CronJobPatch,
    CronRun, DeliveryConfig, ExecutionMode, JobType, Schedule, SessionTarget,
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use uuid::Uuid;

const MAX_CRON_OUTPUT_BYTES: usize = 16 * 1024;
const TRUNCATED_OUTPUT_MARKER: &str = "\n...[truncated]";

/// Internal row representation for SurrealDB serialization/deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronJobRow {
    job_id: Option<String>,
    expression: Option<String>,
    command: Option<String>,
    schedule: Option<String>,
    job_type: Option<String>,
    prompt: Option<String>,
    name: Option<String>,
    session_target: Option<String>,
    model: Option<String>,
    enabled: Option<bool>,
    delivery: Option<String>,
    delete_after_run: Option<bool>,
    execution_mode: Option<String>,
    env_overlay: Option<String>,
    created_at: Option<String>,
    next_run: Option<String>,
    last_run: Option<String>,
    last_status: Option<String>,
    last_output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronRunRow {
    job_id: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    status: Option<String>,
    output: Option<String>,
    duration_ms: Option<i64>,
}

pub async fn add_job(db: &Surreal<Db>, expression: &str, command: &str) -> Result<CronJob> {
    let schedule = Schedule::Cron {
        expr: expression.to_string(),
        tz: None,
    };
    add_shell_job(db, None, schedule, command).await
}

pub async fn add_shell_job(
    db: &Surreal<Db>,
    name: Option<String>,
    schedule: Schedule,
    command: &str,
) -> Result<CronJob> {
    let now = Utc::now();
    validate_schedule(&schedule, now)?;
    let next_run = next_run_for_schedule(&schedule, now)?;
    let id = Uuid::new_v4().to_string();
    let expression = schedule_cron_expression(&schedule).unwrap_or_default();
    let schedule_json = serde_json::to_string(&schedule)?;
    let delete_after_run = matches!(schedule, Schedule::At { .. });

    let delivery_json = serde_json::to_string(&DeliveryConfig::default())?;

    let _ = db
        .query(
            "CREATE cron_job SET
                job_id = $job_id, expression = $expression, command = $command,
                schedule = $schedule, job_type = 'shell', prompt = NONE,
                name = $name, session_target = 'isolated', model = NONE,
                enabled = true, delivery = $delivery,
                delete_after_run = $delete_after_run,
                execution_mode = NONE, env_overlay = NONE,
                created_at = $created_at, next_run = $next_run,
                last_run = NONE, last_status = NONE, last_output = NONE",
        )
        .bind(("job_id", id.clone()))
        .bind(("expression", expression))
        .bind(("command", command.to_string()))
        .bind(("schedule", schedule_json))
        .bind(("name", name))
        .bind(("delivery", delivery_json))
        .bind(("delete_after_run", delete_after_run))
        .bind(("created_at", now.to_rfc3339()))
        .bind(("next_run", next_run.to_rfc3339()))
        .await
        .context("Failed to insert cron shell job")?;

    get_job(db, &id).await
}

#[allow(clippy::too_many_arguments)]
pub async fn add_agent_job(
    db: &Surreal<Db>,
    name: Option<String>,
    schedule: Schedule,
    prompt: &str,
    session_target: SessionTarget,
    model: Option<String>,
    delivery: Option<DeliveryConfig>,
    delete_after_run: bool,
) -> Result<CronJob> {
    add_agent_job_full(
        db,
        name,
        schedule,
        prompt,
        session_target,
        model,
        delivery,
        delete_after_run,
        ExecutionMode::InProcess,
        HashMap::new(),
    )
    .await
}

/// Extended version of `add_agent_job` that accepts execution mode and env overlay.
///
/// - `execution_mode`: `InProcess` (legacy) or `Subprocess` (Phase 3A ephemeral agents).
/// - `env_overlay`: extra environment variables for the subprocess (e.g. broker token, agent ID).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::implicit_hasher)]
pub async fn add_agent_job_full(
    db: &Surreal<Db>,
    name: Option<String>,
    schedule: Schedule,
    prompt: &str,
    session_target: SessionTarget,
    model: Option<String>,
    delivery: Option<DeliveryConfig>,
    delete_after_run: bool,
    execution_mode: ExecutionMode,
    env_overlay: HashMap<String, String>,
) -> Result<CronJob> {
    let now = Utc::now();
    validate_schedule(&schedule, now)?;
    let next_run = next_run_for_schedule(&schedule, now)?;
    let id = Uuid::new_v4().to_string();
    let expression = schedule_cron_expression(&schedule).unwrap_or_default();
    let schedule_json = serde_json::to_string(&schedule)?;
    let delivery = delivery.unwrap_or_default();

    let delivery_json = serde_json::to_string(&delivery)?;
    let execution_mode_json = serde_json::to_string(&execution_mode)?;
    let env_overlay_json = serde_json::to_string(&env_overlay)?;

    let _ = db
        .query(
            "CREATE cron_job SET
                job_id = $job_id, expression = $expression, command = '',
                schedule = $schedule, job_type = 'agent', prompt = $prompt,
                name = $name, session_target = $session_target, model = $model,
                enabled = true, delivery = $delivery,
                delete_after_run = $delete_after_run,
                execution_mode = $execution_mode, env_overlay = $env_overlay,
                created_at = $created_at, next_run = $next_run,
                last_run = NONE, last_status = NONE, last_output = NONE",
        )
        .bind(("job_id", id.clone()))
        .bind(("expression", expression))
        .bind(("schedule", schedule_json))
        .bind(("prompt", prompt.to_string()))
        .bind(("name", name))
        .bind(("session_target", session_target.as_str().to_string()))
        .bind(("model", model))
        .bind(("delivery", delivery_json))
        .bind(("delete_after_run", delete_after_run))
        .bind(("execution_mode", execution_mode_json))
        .bind(("env_overlay", env_overlay_json))
        .bind(("created_at", now.to_rfc3339()))
        .bind(("next_run", next_run.to_rfc3339()))
        .await
        .context("Failed to insert cron agent job")?;

    get_job(db, &id).await
}

pub async fn list_jobs(db: &Surreal<Db>) -> Result<Vec<CronJob>> {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT * FROM cron_job ORDER BY next_run ASC")
        .await
        .context("Failed to query cron jobs")?
        .take(0)
        .context("Failed to take cron job results")?;

    let mut jobs = Vec::new();
    for val in rows {
        match serde_json::from_value::<CronJobRow>(val) {
            Ok(row) => match row_to_cron_job(row) {
                Ok(job) => jobs.push(job),
                Err(e) => tracing::warn!("Skipping cron job with unparseable row data: {e}"),
            },
            Err(e) => tracing::warn!("Skipping cron job with unparseable JSON: {e}"),
        }
    }
    Ok(jobs)
}

pub async fn get_job(db: &Surreal<Db>, job_id: &str) -> Result<CronJob> {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT * FROM cron_job WHERE job_id = $job_id LIMIT 1")
        .bind(("job_id", job_id.to_string()))
        .await
        .context("Failed to query cron job")?
        .take(0)
        .context("Failed to take cron job result")?;

    match rows.into_iter().next() {
        Some(val) => {
            let row: CronJobRow =
                serde_json::from_value(val).context("Failed to deserialize cron job row")?;
            row_to_cron_job(row)
        }
        None => anyhow::bail!("Cron job '{job_id}' not found"),
    }
}

pub async fn remove_job(db: &Surreal<Db>, id: &str) -> Result<()> {
    let result: Vec<serde_json::Value> = db
        .query("DELETE FROM cron_job WHERE job_id = $job_id RETURN BEFORE")
        .bind(("job_id", id.to_string()))
        .await
        .context("Failed to delete cron job")?
        .take(0)
        .context("Failed to take delete result")?;

    if result.is_empty() {
        anyhow::bail!("Cron job '{id}' not found");
    }

    // Also delete associated runs
    let _ = db
        .query("DELETE FROM cron_run WHERE job_id = $job_id")
        .bind(("job_id", id.to_string()))
        .await
        .context("Failed to delete cron runs for job")?;

    println!("Removed cron job {id}");
    Ok(())
}

pub async fn due_jobs(
    db: &Surreal<Db>,
    now: DateTime<Utc>,
    max_tasks: usize,
) -> Result<Vec<CronJob>> {
    let lim = max_tasks.max(1);
    let now_str = now.to_rfc3339();

    let rows: Vec<serde_json::Value> = db
        .query(
            "SELECT * FROM cron_job
             WHERE enabled = true AND next_run <= $now
             ORDER BY next_run ASC
             LIMIT $lim",
        )
        .bind(("now", now_str))
        .bind(("lim", lim))
        .await
        .context("Failed to query due cron jobs")?
        .take(0)
        .context("Failed to take due job results")?;

    let mut jobs = Vec::new();
    for val in rows {
        match serde_json::from_value::<CronJobRow>(val) {
            Ok(row) => match row_to_cron_job(row) {
                Ok(job) => jobs.push(job),
                Err(e) => tracing::warn!("Skipping cron job with unparseable row data: {e}"),
            },
            Err(e) => tracing::warn!("Skipping cron job with unparseable JSON: {e}"),
        }
    }
    Ok(jobs)
}

pub async fn update_job(db: &Surreal<Db>, job_id: &str, patch: CronJobPatch) -> Result<CronJob> {
    let mut job = get_job(db, job_id).await?;
    let mut schedule_changed = false;

    if let Some(schedule) = patch.schedule {
        validate_schedule(&schedule, Utc::now())?;
        job.schedule = schedule;
        job.expression = schedule_cron_expression(&job.schedule).unwrap_or_default();
        schedule_changed = true;
    }
    if let Some(command) = patch.command {
        job.command = command;
    }
    if let Some(prompt) = patch.prompt {
        job.prompt = Some(prompt);
    }
    if let Some(name) = patch.name {
        job.name = Some(name);
    }
    if let Some(enabled) = patch.enabled {
        job.enabled = enabled;
    }
    if let Some(delivery) = patch.delivery {
        job.delivery = delivery;
    }
    if let Some(model) = patch.model {
        job.model = Some(model);
    }
    if let Some(target) = patch.session_target {
        job.session_target = target;
    }
    if let Some(delete_after_run) = patch.delete_after_run {
        job.delete_after_run = delete_after_run;
    }

    if schedule_changed {
        job.next_run = next_run_for_schedule(&job.schedule, Utc::now())?;
    }

    let _ = db
        .query(
            "UPDATE cron_job SET
                expression = $expression,
                command = $command,
                schedule = $schedule,
                job_type = $job_type,
                prompt = $prompt,
                name = $name,
                session_target = $session_target,
                model = $model,
                enabled = $enabled,
                delivery = $delivery,
                delete_after_run = $delete_after_run,
                execution_mode = $execution_mode,
                env_overlay = $env_overlay,
                next_run = $next_run
             WHERE job_id = $job_id",
        )
        .bind(("expression", job.expression.clone()))
        .bind(("command", job.command.clone()))
        .bind(("schedule", serde_json::to_string(&job.schedule)?))
        .bind((
            "job_type",
            <JobType as Into<&str>>::into(job.job_type).to_string(),
        ))
        .bind(("prompt", job.prompt.clone()))
        .bind(("name", job.name.clone()))
        .bind(("session_target", job.session_target.as_str().to_string()))
        .bind(("model", job.model.clone()))
        .bind(("enabled", job.enabled))
        .bind(("delivery", serde_json::to_string(&job.delivery)?))
        .bind(("delete_after_run", job.delete_after_run))
        .bind((
            "execution_mode",
            serde_json::to_string(&job.execution_mode)?,
        ))
        .bind(("env_overlay", serde_json::to_string(&job.env_overlay)?))
        .bind(("next_run", job.next_run.to_rfc3339()))
        .bind(("job_id", job_id.to_string()))
        .await
        .context("Failed to update cron job")?;

    get_job(db, job_id).await
}

pub async fn record_last_run(
    db: &Surreal<Db>,
    job_id: &str,
    finished_at: DateTime<Utc>,
    success: bool,
    output: &str,
) -> Result<()> {
    let status = if success { "ok" } else { "error" };
    let bounded_output = truncate_cron_output(output);
    let _ = db
        .query(
            "UPDATE cron_job SET
                last_run = $last_run,
                last_status = $last_status,
                last_output = $last_output
             WHERE job_id = $job_id",
        )
        .bind(("last_run", finished_at.to_rfc3339()))
        .bind(("last_status", status.to_string()))
        .bind(("last_output", bounded_output))
        .bind(("job_id", job_id.to_string()))
        .await
        .context("Failed to update cron last run fields")?;
    Ok(())
}

pub async fn reschedule_after_run(
    db: &Surreal<Db>,
    job: &CronJob,
    success: bool,
    output: &str,
) -> Result<()> {
    let now = Utc::now();
    let next_run = next_run_for_schedule(&job.schedule, now)?;
    let status = if success { "ok" } else { "error" };
    let bounded_output = truncate_cron_output(output);

    let _ = db
        .query(
            "UPDATE cron_job SET
                next_run = $next_run,
                last_run = $last_run,
                last_status = $last_status,
                last_output = $last_output
             WHERE job_id = $job_id",
        )
        .bind(("next_run", next_run.to_rfc3339()))
        .bind(("last_run", now.to_rfc3339()))
        .bind(("last_status", status.to_string()))
        .bind(("last_output", bounded_output))
        .bind(("job_id", job.id.clone()))
        .await
        .context("Failed to update cron job run state")?;
    Ok(())
}

pub async fn record_run(
    db: &Surreal<Db>,
    job_id: &str,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    status: &str,
    output: Option<&str>,
    duration_ms: i64,
    max_run_history: u32,
) -> Result<()> {
    let bounded_output = output.map(truncate_cron_output);

    let _ = db
        .query(
            "CREATE cron_run SET
                job_id = $job_id, started_at = $started_at,
                finished_at = $finished_at, status = $status,
                output = $output, duration_ms = $duration_ms",
        )
        .bind(("job_id", job_id.to_string()))
        .bind(("started_at", started_at.to_rfc3339()))
        .bind(("finished_at", finished_at.to_rfc3339()))
        .bind(("status", status.to_string()))
        .bind(("output", bounded_output))
        .bind(("duration_ms", duration_ms))
        .await
        .context("Failed to insert cron run")?;

    // Prune old runs beyond max_run_history
    let keep = max_run_history.max(1) as usize;
    let _ = db
        .query(
            "LET $keep_ids = (SELECT id FROM cron_run WHERE job_id = $job_id ORDER BY started_at DESC LIMIT $keep);
             DELETE FROM cron_run WHERE job_id = $job_id AND id NOT IN $keep_ids",
        )
        .bind(("job_id", job_id.to_string()))
        .bind(("keep", keep))
        .await
        .context("Failed to prune cron run history")?;

    Ok(())
}

fn truncate_cron_output(output: &str) -> String {
    if output.len() <= MAX_CRON_OUTPUT_BYTES {
        return output.to_string();
    }

    if MAX_CRON_OUTPUT_BYTES <= TRUNCATED_OUTPUT_MARKER.len() {
        return TRUNCATED_OUTPUT_MARKER.to_string();
    }

    let mut cutoff = MAX_CRON_OUTPUT_BYTES - TRUNCATED_OUTPUT_MARKER.len();
    while cutoff > 0 && !output.is_char_boundary(cutoff) {
        cutoff -= 1;
    }

    let mut truncated = output[..cutoff].to_string();
    truncated.push_str(TRUNCATED_OUTPUT_MARKER);
    truncated
}

pub async fn list_runs(db: &Surreal<Db>, job_id: &str, limit: usize) -> Result<Vec<CronRun>> {
    let lim = limit.max(1);

    let rows: Vec<serde_json::Value> = db
        .query(
            "SELECT * FROM cron_run
             WHERE job_id = $job_id
             ORDER BY started_at DESC
             LIMIT $lim",
        )
        .bind(("job_id", job_id.to_string()))
        .bind(("lim", lim))
        .await
        .context("Failed to query cron runs")?
        .take(0)
        .context("Failed to take cron run results")?;

    let mut runs = Vec::new();
    for val in rows {
        let row: CronRunRow =
            serde_json::from_value(val).context("Failed to deserialize cron run row")?;
        runs.push(run_row_to_cron_run(row)?);
    }
    Ok(runs)
}

fn parse_rfc3339(raw: &str) -> Result<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(raw)
        .with_context(|| format!("Invalid RFC3339 timestamp in cron DB: {raw}"))?;
    Ok(parsed.with_timezone(&Utc))
}

fn row_to_cron_job(row: CronJobRow) -> Result<CronJob> {
    let expression = row.expression.unwrap_or_default();
    let schedule = decode_schedule(row.schedule.as_deref(), &expression)?;
    let delivery = decode_delivery(row.delivery.as_deref())?;
    let execution_mode = decode_execution_mode(row.execution_mode.as_deref())?;
    let env_overlay = decode_env_overlay(row.env_overlay.as_deref())?;

    let created_at_raw = row
        .created_at
        .ok_or_else(|| anyhow::anyhow!("Missing created_at"))?;
    let next_run_raw = row
        .next_run
        .ok_or_else(|| anyhow::anyhow!("Missing next_run"))?;

    let job_type_str = row.job_type.unwrap_or_else(|| "shell".to_string());
    let job_type = JobType::try_from(job_type_str.as_str())
        .map_err(|e| anyhow::anyhow!("Invalid job_type: {e}"))?;

    Ok(CronJob {
        id: row
            .job_id
            .ok_or_else(|| anyhow::anyhow!("Missing job_id"))?,
        expression,
        schedule,
        command: row.command.unwrap_or_default(),
        job_type,
        prompt: row.prompt,
        name: row.name,
        session_target: SessionTarget::parse(row.session_target.as_deref().unwrap_or("isolated")),
        model: row.model,
        enabled: row.enabled.unwrap_or(true),
        delivery,
        delete_after_run: row.delete_after_run.unwrap_or(false),
        execution_mode,
        env_overlay,
        created_at: parse_rfc3339(&created_at_raw)?,
        next_run: parse_rfc3339(&next_run_raw)?,
        last_run: match row.last_run {
            Some(raw) => Some(parse_rfc3339(&raw)?),
            None => None,
        },
        last_status: row.last_status,
        last_output: row.last_output,
        allowed_tools: None,
    })
}

fn run_row_to_cron_run(row: CronRunRow) -> Result<CronRun> {
    Ok(CronRun {
        id: 0, // SurrealDB uses string IDs; legacy i64 field kept for compat
        job_id: row
            .job_id
            .ok_or_else(|| anyhow::anyhow!("Missing job_id in cron_run"))?,
        started_at: parse_rfc3339(
            row.started_at
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Missing started_at"))?,
        )?,
        finished_at: parse_rfc3339(
            row.finished_at
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Missing finished_at"))?,
        )?,
        status: row
            .status
            .ok_or_else(|| anyhow::anyhow!("Missing status in cron_run"))?,
        output: row.output,
        duration_ms: row.duration_ms,
    })
}

fn decode_schedule(schedule_raw: Option<&str>, expression: &str) -> Result<Schedule> {
    if let Some(raw) = schedule_raw {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return serde_json::from_str(trimmed)
                .with_context(|| format!("Failed to parse cron schedule JSON: {trimmed}"));
        }
    }

    if expression.trim().is_empty() {
        anyhow::bail!("Missing schedule and legacy expression for cron job")
    }

    Ok(Schedule::Cron {
        expr: expression.to_string(),
        tz: None,
    })
}

fn decode_delivery(delivery_raw: Option<&str>) -> Result<DeliveryConfig> {
    if let Some(raw) = delivery_raw {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return serde_json::from_str(trimmed)
                .with_context(|| format!("Failed to parse cron delivery JSON: {trimmed}"));
        }
    }
    Ok(DeliveryConfig::default())
}

fn decode_execution_mode(raw: Option<&str>) -> Result<ExecutionMode> {
    if let Some(s) = raw {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return serde_json::from_str(trimmed)
                .with_context(|| format!("Failed to parse execution_mode JSON: {trimmed}"));
        }
    }
    Ok(ExecutionMode::default())
}

fn decode_env_overlay(raw: Option<&str>) -> Result<HashMap<String, String>> {
    if let Some(s) = raw {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return serde_json::from_str(trimmed)
                .with_context(|| format!("Failed to parse env_overlay JSON: {trimmed}"));
        }
    }
    Ok(HashMap::new())
}
