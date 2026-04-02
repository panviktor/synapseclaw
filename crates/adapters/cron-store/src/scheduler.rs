use crate::{
    due_jobs, next_run_for_schedule, record_last_run, record_run, remove_job, reschedule_after_run,
    update_job, CronJob, CronJobPatch, ExecutionMode, JobType, Schedule, SessionTarget,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt};
use std::process::Stdio;
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use synapse_domain::config::schema::Config;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_security::security_factory::security_policy_from_config;
use tokio::process::Command;
use tokio::time::{self, Duration};

const MIN_POLL_SECONDS: u64 = 5;
const SHELL_JOB_TIMEOUT_SECS: u64 = 120;
const SCHEDULER_COMPONENT: &str = "scheduler";

/// Health-check reporter -- injected by the caller so the scheduler doesn't
/// depend on the concrete `health` module in adapters.
pub trait HealthReporter: Send + Sync {
    fn mark_ok(&self, component: &str);
    fn mark_error(&self, component: &str, error: String);
    /// Snapshot of all component health for diagnostics.
    fn snapshot_json(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

/// No-op reporter for tests and contexts where health tracking is irrelevant.
pub struct NoopHealthReporter;
impl HealthReporter for NoopHealthReporter {
    fn mark_ok(&self, _component: &str) {}
    fn mark_error(&self, _component: &str, _error: String) {}
}

/// Cron output delivery -- the scheduler needs to send job results somewhere.
/// Adapters implement this with the real `DeliveryService`.
#[async_trait::async_trait]
pub trait CronDeliveryPort: Send + Sync {
    async fn deliver_cron_output(
        &self,
        delivery: &synapse_domain::domain::config::CronDeliveryConfig,
        output: &str,
    ) -> anyhow::Result<()>;
}

/// No-op delivery for tests.
pub struct NoopCronDelivery;
#[async_trait::async_trait]
impl CronDeliveryPort for NoopCronDelivery {
    async fn deliver_cron_output(
        &self,
        _delivery: &synapse_domain::domain::config::CronDeliveryConfig,
        _output: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

pub async fn run(
    config: Config,
    db: Arc<Surreal<Db>>,
    delivery_service: Arc<dyn CronDeliveryPort>,
    agent_runner: std::sync::Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
    health: Arc<dyn HealthReporter>,
) -> Result<()> {
    let poll_secs = config.reliability.scheduler_poll_secs.max(MIN_POLL_SECONDS);
    let mut interval = time::interval(Duration::from_secs(poll_secs));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let security = Arc::new(security_policy_from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    health.mark_ok(SCHEDULER_COMPONENT);

    loop {
        interval.tick().await;
        health.mark_ok(SCHEDULER_COMPONENT);

        let max_tasks = config.scheduler.max_tasks;
        let jobs = match due_jobs(&db, Utc::now(), max_tasks).await {
            Ok(jobs) => jobs,
            Err(e) => {
                health.mark_error(SCHEDULER_COMPONENT, e.to_string());
                tracing::warn!("Scheduler query failed: {e}");
                continue;
            }
        };

        process_due_jobs(
            &config,
            &db,
            &security,
            jobs,
            SCHEDULER_COMPONENT,
            delivery_service.clone(),
            agent_runner.clone(),
            &health,
        )
        .await;
    }
}

pub async fn execute_job_now(
    config: &Config,
    job: &CronJob,
    agent_runner: &dyn synapse_domain::ports::agent_runner::AgentRunnerPort,
) -> (bool, String) {
    let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);
    Box::pin(execute_job_with_retry(config, &security, job, agent_runner)).await
}

async fn execute_job_with_retry(
    config: &Config,
    security: &SecurityPolicy,
    job: &CronJob,
    agent_runner: &dyn synapse_domain::ports::agent_runner::AgentRunnerPort,
) -> (bool, String) {
    tracing::info!(
        job_name = ?job.name,
        job_type = ?job.job_type,
        "cron.job.start"
    );
    let job_start = std::time::Instant::now();
    let mut last_output = String::new();
    let retries = config.reliability.scheduler_retries;
    let mut backoff_ms = config.reliability.provider_backoff_ms.max(200);

    for attempt in 0..=retries {
        let (success, output) = match job.job_type {
            JobType::Shell => run_job_command(config, security, job).await,
            JobType::Agent => Box::pin(run_agent_job(config, security, job, agent_runner)).await,
        };
        last_output = output;

        if success {
            tracing::info!(
                job_name = ?job.name,
                success = true,
                attempt,
                duration_ms = job_start.elapsed().as_millis() as u64,
                "cron.job.complete"
            );
            return (true, last_output);
        }

        if last_output.starts_with("blocked by security policy:") {
            // Deterministic policy violations are not retryable.
            return (false, last_output);
        }

        if attempt < retries {
            let jitter_ms = u64::from(Utc::now().timestamp_subsec_millis() % 250);
            time::sleep(Duration::from_millis(backoff_ms + jitter_ms)).await;
            backoff_ms = (backoff_ms.saturating_mul(2)).min(30_000);
        }
    }

    tracing::info!(
        job_name = ?job.name,
        success = false,
        duration_ms = job_start.elapsed().as_millis() as u64,
        "cron.job.complete"
    );
    (false, last_output)
}

async fn process_due_jobs(
    config: &Config,
    db: &Surreal<Db>,
    security: &Arc<SecurityPolicy>,
    jobs: Vec<CronJob>,
    component: &str,
    delivery_service: Arc<dyn CronDeliveryPort>,
    agent_runner: std::sync::Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
    health: &Arc<dyn HealthReporter>,
) {
    // Refresh scheduler health on every successful poll cycle, including idle cycles.
    health.mark_ok(component);

    let max_concurrent = config.scheduler.max_concurrent.max(1);
    // Execute jobs concurrently, then persist results sequentially
    // (execute_job_with_retry does not need the DB handle).
    let mut results = Vec::new();
    let mut in_flight = stream::iter(jobs.into_iter().map(|job| {
        let config = config.clone();
        let security = Arc::clone(security);
        let component = component.to_owned();
        let ds = delivery_service.clone();
        let ar = agent_runner.clone();
        let h = Arc::clone(health);
        async move {
            h.mark_ok(&component);
            warn_if_high_frequency_agent_job(&job);

            let started_at = Utc::now();
            let (success, output) = Box::pin(execute_job_with_retry(
                &config,
                security.as_ref(),
                &job,
                ar.as_ref(),
            ))
            .await;
            let finished_at = Utc::now();

            (job, success, output, started_at, finished_at, ds)
        }
    }))
    .buffer_unordered(max_concurrent);

    while let Some(tuple) = in_flight.next().await {
        results.push(tuple);
    }

    // Persist results sequentially (shared SurrealDB handle)
    for (job, success, output, started_at, finished_at, delivery_service) in results {
        let final_success = Box::pin(persist_job_result(
            config,
            db,
            &job,
            success,
            &output,
            started_at,
            finished_at,
            delivery_service,
        ))
        .await;

        if !final_success {
            tracing::warn!("Scheduler job '{}' failed: {output}", job.id);
        }
    }
}

async fn run_agent_job(
    config: &Config,
    security: &SecurityPolicy,
    job: &CronJob,
    agent_runner: &dyn synapse_domain::ports::agent_runner::AgentRunnerPort,
) -> (bool, String) {
    if !security.can_act() {
        return (
            false,
            "blocked by security policy: autonomy is read-only".to_string(),
        );
    }

    if security.is_rate_limited() {
        return (
            false,
            "blocked by security policy: rate limit exceeded".to_string(),
        );
    }

    if !security.record_action() {
        return (
            false,
            "blocked by security policy: action budget exhausted".to_string(),
        );
    }

    match job.execution_mode {
        ExecutionMode::InProcess => {
            Box::pin(run_agent_job_in_process(config, job, agent_runner)).await
        }
        ExecutionMode::Subprocess => run_agent_job_subprocess(config, job).await,
    }
}

async fn run_agent_job_in_process(
    config: &Config,
    job: &CronJob,
    agent_runner: &dyn synapse_domain::ports::agent_runner::AgentRunnerPort,
) -> (bool, String) {
    let name = job.name.clone().unwrap_or_else(|| "cron-job".to_string());
    let prompt = job.prompt.clone().unwrap_or_default();
    let prefixed_prompt = format!("[cron:{} {name}] {prompt}", job.id);
    let model_override = job.model.clone();

    let run_result = match job.session_target {
        SessionTarget::Main | SessionTarget::Isolated => {
            agent_runner
                .run(
                    Some(prefixed_prompt),
                    None,
                    model_override,
                    config.default_temperature,
                    false,
                    None,
                    job.allowed_tools.clone(),
                    None,
                )
                .await
        }
    };

    match run_result {
        Ok(response) => (
            true,
            if response.trim().is_empty() {
                "agent job executed".to_string()
            } else {
                response
            },
        ),
        Err(e) => (false, format!("agent job failed: {e}")),
    }
}

/// Timeout for subprocess agent jobs (seconds).
const SUBPROCESS_AGENT_TIMEOUT_SECS: u64 = 600;

async fn run_agent_job_subprocess(config: &Config, job: &CronJob) -> (bool, String) {
    let prompt = job.prompt.clone().unwrap_or_default();

    // Resolve the synapseclaw binary: prefer current executable, then PATH.
    let binary = match std::env::current_exe() {
        Ok(exe) if exe.exists() => exe,
        _ => match which::which("synapseclaw") {
            Ok(path) => path,
            Err(_) => {
                return (
                    false,
                    "subprocess spawn failed: cannot find synapseclaw binary".to_string(),
                )
            }
        },
    };

    // Build std::process::Command first so sandbox can wrap it
    let mut std_cmd = std::process::Command::new(&binary);
    std_cmd.arg("agent").arg("-m").arg(&prompt);

    // Model override
    if let Some(ref model) = job.model {
        std_cmd.arg("--model").arg(model);
    }

    // Working directory
    std_cmd.current_dir(&config.workspace_dir);

    // Environment overlay (broker token, agent_id, session_id, etc.)
    for (key, value) in &job.env_overlay {
        std_cmd.env(key, value);
    }

    // Apply sandbox wrapping (OS-level isolation)
    let sandbox = synapse_security::detect::create_sandbox(&config.security);
    if let Err(e) = sandbox.wrap_command(&mut std_cmd) {
        return (
            false,
            format!("sandbox wrap failed ({}): {e}", sandbox.name()),
        );
    }

    // Convert to tokio Command for async execution
    let mut cmd = Command::from(std_cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return (
                false,
                format!("subprocess spawn error ({}): {e}", binary.display()),
            )
        }
    };

    let timeout_secs = job
        .env_overlay
        .get("SYNAPSECLAW_TIMEOUT_SECS")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(SUBPROCESS_AGENT_TIMEOUT_SECS);

    match time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!(
                "status={}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                stdout.trim(),
                stderr.trim()
            );
            (output.status.success(), combined)
        }
        Ok(Err(e)) => (false, format!("subprocess spawn error: {e}")),
        Err(_) => (
            false,
            format!("subprocess agent timed out after {timeout_secs}s"),
        ),
    }
}

async fn persist_job_result(
    config: &Config,
    db: &Surreal<Db>,
    job: &CronJob,
    mut success: bool,
    output: &str,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    delivery_service: Arc<dyn CronDeliveryPort>,
) -> bool {
    let duration_ms = (finished_at - started_at).num_milliseconds();

    let core_delivery = crate::cron_delivery_config_from(&job.delivery);

    if let Err(e) = delivery_service
        .deliver_cron_output(&core_delivery, output)
        .await
    {
        if job.delivery.best_effort {
            tracing::warn!("Cron delivery failed (best_effort): {e}");
        } else {
            success = false;
            tracing::warn!("Cron delivery failed: {e}");
        }
    }

    let _ = record_run(
        db,
        &job.id,
        started_at,
        finished_at,
        if success { "ok" } else { "error" },
        Some(output),
        duration_ms,
        config.cron.max_run_history,
    )
    .await;

    if is_one_shot_auto_delete(job) {
        if success {
            if let Err(e) = remove_job(db, &job.id).await {
                tracing::warn!("Failed to remove one-shot cron job after success: {e}");
            }
        } else {
            let _ = record_last_run(db, &job.id, finished_at, false, output).await;
            if let Err(e) = update_job(
                db,
                &job.id,
                CronJobPatch {
                    enabled: Some(false),
                    ..CronJobPatch::default()
                },
            )
            .await
            {
                tracing::warn!("Failed to disable failed one-shot cron job: {e}");
            }
        }
        return success;
    }

    if let Err(e) = reschedule_after_run(db, job, success, output).await {
        tracing::warn!("Failed to persist scheduler run result: {e}");
    }

    success
}

fn is_one_shot_auto_delete(job: &CronJob) -> bool {
    job.delete_after_run && matches!(job.schedule, Schedule::At { .. })
}

fn warn_if_high_frequency_agent_job(job: &CronJob) {
    if !matches!(job.job_type, JobType::Agent) {
        return;
    }
    let too_frequent = match &job.schedule {
        Schedule::Every { every_ms } => *every_ms < 5 * 60 * 1000,
        Schedule::Cron { .. } => {
            let now = Utc::now();
            match (
                next_run_for_schedule(&job.schedule, now),
                next_run_for_schedule(&job.schedule, now + chrono::Duration::seconds(1)),
            ) {
                (Ok(a), Ok(b)) => (b - a).num_minutes() < 5,
                _ => false,
            }
        }
        Schedule::At { .. } => false,
    };

    if too_frequent {
        tracing::warn!(
            "Cron agent job '{}' is scheduled more frequently than every 5 minutes",
            job.id
        );
    }
}

async fn run_job_command(
    config: &Config,
    security: &SecurityPolicy,
    job: &CronJob,
) -> (bool, String) {
    run_job_command_with_timeout(
        config,
        security,
        job,
        Duration::from_secs(SHELL_JOB_TIMEOUT_SECS),
    )
    .await
}

async fn run_job_command_with_timeout(
    config: &Config,
    security: &SecurityPolicy,
    job: &CronJob,
    timeout: Duration,
) -> (bool, String) {
    if !security.can_act() {
        return (
            false,
            "blocked by security policy: autonomy is read-only".to_string(),
        );
    }

    if security.is_rate_limited() {
        return (
            false,
            "blocked by security policy: rate limit exceeded".to_string(),
        );
    }

    // Unified command validation: allowlist + risk + path checks in one call.
    // Jobs created via the validated helpers were already checked at creation
    // time, but we re-validate at execution time to catch policy changes and
    // manually-edited job stores.
    let approved = false; // scheduler runs are never pre-approved
    if let Err(error) =
        crate::validate_shell_command_with_security(security, &job.command, approved)
    {
        return (false, error.to_string());
    }

    if let Some(path) = security.forbidden_path_argument(&job.command) {
        return (
            false,
            format!("blocked by security policy: forbidden path argument: {path}"),
        );
    }

    if !security.record_action() {
        return (
            false,
            "blocked by security policy: action budget exhausted".to_string(),
        );
    }

    let child = match Command::new("sh")
        .arg("-lc")
        .arg(&job.command)
        .current_dir(&config.workspace_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return (false, format!("spawn error: {e}")),
    };

    match time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!(
                "status={}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                stdout.trim(),
                stderr.trim()
            );
            (output.status.success(), combined)
        }
        Ok(Err(e)) => (false, format!("spawn error: {e}")),
        Err(_) => (
            false,
            format!("job timed out after {}s", timeout.as_secs_f64()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeliveryConfig;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use synapse_domain::config::schema::Config;
    use synapse_security::security_factory::security_policy_from_config;
    use tempfile::TempDir;

    /// Test health reporter that tracks component state in memory.
    struct MockHealthReporter {
        state: Mutex<HashMap<String, (String, Option<String>)>>,
    }
    impl MockHealthReporter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(HashMap::new()),
            })
        }
    }
    impl HealthReporter for MockHealthReporter {
        fn mark_ok(&self, component: &str) {
            self.state
                .lock()
                .unwrap()
                .insert(component.to_string(), ("ok".to_string(), None));
        }
        fn mark_error(&self, component: &str, error: String) {
            self.state
                .lock()
                .unwrap()
                .insert(component.to_string(), ("error".to_string(), Some(error)));
        }
        fn snapshot_json(&self) -> serde_json::Value {
            let guard = self.state.lock().unwrap();
            let mut components = serde_json::Map::new();
            for (k, (status, _err)) in guard.iter() {
                let mut entry = serde_json::Map::new();
                entry.insert("status".to_string(), serde_json::json!(status));
                if status == "ok" {
                    entry.insert(
                        "last_ok".to_string(),
                        serde_json::json!(chrono::Utc::now().to_rfc3339()),
                    );
                    entry.insert("last_error".to_string(), serde_json::Value::Null);
                }
                components.insert(k.clone(), serde_json::Value::Object(entry));
            }
            serde_json::json!({ "components": components })
        }
    }

    struct NoopRunner;
    #[async_trait::async_trait]
    impl synapse_domain::ports::agent_runner::AgentRunnerPort for NoopRunner {
        async fn run(
            &self,
            _: Option<String>,
            _: Option<String>,
            _: Option<String>,
            _: f64,
            _: bool,
            _: Option<std::path::PathBuf>,
            _: Option<Vec<String>>,
            _: Option<Arc<synapse_domain::domain::tool_audit::RunContext>>,
        ) -> anyhow::Result<String> {
            Ok("noop".into())
        }
        async fn process_message(&self, _: &str, _: Option<&str>) -> anyhow::Result<String> {
            Ok("noop".into())
        }
    }

    #[allow(dead_code)]
    fn noop_runner() -> &'static dyn synapse_domain::ports::agent_runner::AgentRunnerPort {
        &NoopRunner
    }

    /// Runner that always fails -- for tests that expect agent errors.
    struct FailRunner;
    #[async_trait::async_trait]
    impl synapse_domain::ports::agent_runner::AgentRunnerPort for FailRunner {
        async fn run(
            &self,
            _: Option<String>,
            _: Option<String>,
            _: Option<String>,
            _: f64,
            _: bool,
            _: Option<std::path::PathBuf>,
            _: Option<Vec<String>>,
            _: Option<Arc<synapse_domain::domain::tool_audit::RunContext>>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("no provider API key configured")
        }
        async fn process_message(&self, _: &str, _: Option<&str>) -> anyhow::Result<String> {
            anyhow::bail!("no provider API key configured")
        }
    }

    fn fail_runner() -> &'static dyn synapse_domain::ports::agent_runner::AgentRunnerPort {
        &FailRunner
    }

    /// No-op delivery for tests that don't exercise delivery.
    #[allow(dead_code)]
    fn noop_delivery_service() -> Arc<dyn CronDeliveryPort> {
        Arc::new(NoopCronDelivery)
    }

    async fn test_config(tmp: &TempDir) -> Config {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        tokio::fs::create_dir_all(&config.workspace_dir)
            .await
            .unwrap();
        config
    }

    fn test_job(command: &str) -> CronJob {
        CronJob {
            id: "test-job".into(),
            expression: "* * * * *".into(),
            schedule: crate::Schedule::Cron {
                expr: "* * * * *".into(),
                tz: None,
            },
            command: command.into(),
            prompt: None,
            name: None,
            job_type: JobType::Shell,
            session_target: SessionTarget::Isolated,
            model: None,
            enabled: true,
            delivery: DeliveryConfig::default(),
            delete_after_run: false,
            execution_mode: ExecutionMode::InProcess,
            env_overlay: std::collections::HashMap::new(),
            allowed_tools: None,
            created_at: Utc::now(),
            next_run: Utc::now(),
            last_run: None,
            last_status: None,
            last_output: None,
        }
    }

    fn unique_component(prefix: &str) -> String {
        format!("{prefix}-{}", uuid::Uuid::new_v4())
    }

    #[tokio::test]
    async fn run_job_command_success() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp).await;
        let job = test_job("echo scheduler-ok");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(success);
        assert!(output.contains("scheduler-ok"));
        assert!(output.contains("status=exit status: 0"));
    }

    #[tokio::test]
    async fn run_job_command_failure() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp).await;
        let job = test_job("ls definitely_missing_file_for_scheduler_test");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("definitely_missing_file_for_scheduler_test"));
        assert!(output.contains("status=exit status:"));
    }

    #[tokio::test]
    async fn run_job_command_times_out() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["sleep".into()];
        let job = test_job("sleep 1");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) =
            run_job_command_with_timeout(&config, &security, &job, Duration::from_millis(50)).await;
        assert!(!success);
        assert!(output.contains("job timed out after"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_disallowed_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["echo".into()];
        let job = test_job("curl https://evil.example");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.to_lowercase().contains("not allowed"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_forbidden_path_argument() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["cat".into()];
        let job = test_job("cat /etc/passwd");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("forbidden path argument"));
        assert!(output.contains("/etc/passwd"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_forbidden_option_assignment_path_argument() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["grep".into()];
        let job = test_job("grep --file=/etc/passwd root ./src");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("forbidden path argument"));
        assert!(output.contains("/etc/passwd"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_forbidden_short_option_attached_path_argument() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["grep".into()];
        let job = test_job("grep -f/etc/passwd root ./src");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("forbidden path argument"));
        assert!(output.contains("/etc/passwd"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_tilde_user_path_argument() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["cat".into()];
        let job = test_job("cat ~root/.ssh/id_rsa");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("forbidden path argument"));
        assert!(output.contains("~root/.ssh/id_rsa"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_input_redirection_path_bypass() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.allowed_commands = vec!["cat".into()];
        let job = test_job("cat </etc/passwd");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.to_lowercase().contains("not allowed"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_readonly_mode() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.level = synapse_domain::domain::config::AutonomyLevel::ReadOnly;
        let job = test_job("echo should-not-run");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("read-only"));
    }

    #[tokio::test]
    async fn run_job_command_blocks_rate_limited() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.max_actions_per_hour = 0;
        let job = test_job("echo should-not-run");
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) = run_job_command(&config, &security, &job).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("rate limit exceeded"));
    }

    #[tokio::test]
    async fn run_agent_job_blocks_readonly_mode() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.level = synapse_domain::domain::config::AutonomyLevel::ReadOnly;
        let mut job = test_job("");
        job.job_type = JobType::Agent;
        job.prompt = Some("Say hello".into());
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) =
            Box::pin(run_agent_job(&config, &security, &job, noop_runner())).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("read-only"));
    }

    #[tokio::test]
    async fn run_agent_job_blocks_rate_limited() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.autonomy.max_actions_per_hour = 0;
        let mut job = test_job("");
        job.job_type = JobType::Agent;
        job.prompt = Some("Say hello".into());
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) =
            Box::pin(run_agent_job(&config, &security, &job, noop_runner())).await;
        assert!(!success);
        assert!(output.contains("blocked by security policy"));
        assert!(output.contains("rate limit exceeded"));
    }

    #[tokio::test]
    async fn run_agent_job_returns_error_without_provider_key() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp).await;
        let mut job = test_job("");
        job.job_type = JobType::Agent;
        job.prompt = Some("Say hello".into());
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let (success, output) =
            Box::pin(run_agent_job(&config, &security, &job, fail_runner())).await;
        assert!(!success);
        assert!(output.contains("agent job failed:"));
    }

    #[tokio::test]
    async fn execute_job_with_retry_exhausts_attempts() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.reliability.scheduler_retries = 1;
        config.reliability.provider_backoff_ms = 1;
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        let job = test_job("ls always_missing_for_retry_test");

        let (success, output) = Box::pin(execute_job_with_retry(
            &config,
            &security,
            &job,
            noop_runner(),
        ))
        .await;
        assert!(!success);
        assert!(output.contains("always_missing_for_retry_test"));
    }

    #[tokio::test]
    async fn execute_job_with_retry_recovers_after_first_failure() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp).await;
        config.reliability.scheduler_retries = 1;
        config.reliability.provider_backoff_ms = 1;
        config.autonomy.allowed_commands = vec!["sh".into()];
        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);

        tokio::fs::write(
            config.workspace_dir.join("retry-once.sh"),
            "#!/bin/sh\nif [ -f retry-ok.flag ]; then\n  echo recovered\n  exit 0\nfi\ntouch retry-ok.flag\nexit 1\n",
        )
        .await
        .unwrap();
        let job = test_job("sh ./retry-once.sh");

        let (success, output) = Box::pin(execute_job_with_retry(
            &config,
            &security,
            &job,
            noop_runner(),
        ))
        .await;
        assert!(success);
        assert!(output.contains("recovered"));
    }
}
