use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{Db, JobType, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::security_policy::SecurityPolicy;

pub struct CronRunTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
    agent_runner: Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
    db: Arc<Surreal<Db>>,
}

impl CronRunTool {
    pub fn new(
        config: Arc<Config>,
        security: Arc<SecurityPolicy>,
        agent_runner: Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
        db: Arc<Surreal<Db>>,
    ) -> Self {
        Self {
            config,
            security,
            agent_runner,
            db,
        }
    }
}

#[async_trait]
impl Tool for CronRunTool {
    fn name(&self) -> &str {
        "cron_run"
    }

    fn description(&self) -> &str {
        "Force-run a cron job immediately and record run history"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" },
                "approved": {
                    "type": "boolean",
                    "description": "Set true to explicitly approve medium/high-risk shell commands in supervised mode",
                    "default": false
                }
            },
            "required": ["job_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.config.cron.enabled {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("cron is disabled by config (cron.enabled=false)".to_string()),
            });
        }

        let job_id = match args.get("job_id").and_then(serde_json::Value::as_str) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing 'job_id' parameter".to_string()),
                });
            }
        };
        let approved = args
            .get("approved")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if !self.security.can_act() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Security policy: read-only mode, cannot perform 'cron_run'".into()),
            });
        }

        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        let job = match synapse_cron::get_job(&self.db, job_id).await {
            Ok(job) => job,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(e.to_string()),
                });
            }
        };

        if matches!(job.job_type, JobType::Shell) {
            if let Err(reason) = self
                .security
                .validate_command_execution(&job.command, approved)
            {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(reason),
                });
            }
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        let started_at = Utc::now();
        let (success, output) = Box::pin(synapse_cron::scheduler::execute_job_now(
            &self.config,
            &job,
            self.agent_runner.as_ref(),
        ))
        .await;
        let finished_at = Utc::now();
        let duration_ms = (finished_at - started_at).num_milliseconds();
        let status = if success { "ok" } else { "error" };

        let _ = synapse_cron::record_run(
            &self.db,
            &job.id,
            started_at,
            finished_at,
            status,
            Some(&output),
            duration_ms,
            self.config.cron.max_run_history,
        )
        .await;
        let _ =
            synapse_cron::record_last_run(&self.db, &job.id, finished_at, success, &output).await;

        Ok(ToolResult {
            success,
            output: serde_json::to_string_pretty(&json!({
                "job_id": job.id,
                "status": status,
                "duration_ms": duration_ms,
                "output": output
            }))?,
            error: if success {
                None
            } else {
                Some("cron job execution failed".to_string())
            },
        })
    }
}

// Tests removed -- require SurrealDB setup (async integration tests).
