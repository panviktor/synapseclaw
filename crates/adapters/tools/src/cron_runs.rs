use super::traits::{Tool, ToolResult};
use crate::cron_facts;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{Db, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::ports::tool::ToolExecution;

const MAX_RUN_OUTPUT_CHARS: usize = 500;

pub struct CronRunsTool {
    config: Arc<Config>,
    db: Arc<Surreal<Db>>,
}

impl CronRunsTool {
    pub fn new(config: Arc<Config>, db: Arc<Surreal<Db>>) -> Self {
        Self { config, db }
    }

    async fn execute_action(&self, args: &serde_json::Value) -> anyhow::Result<ToolExecution> {
        if !self.config.cron.enabled {
            return Ok(ToolExecution {
                result: ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("cron is disabled by config (cron.enabled=false)".to_string()),
                },
                facts: Vec::new(),
            });
        }

        let job_id = match args.get("job_id").and_then(serde_json::Value::as_str) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Ok(ToolExecution {
                    result: ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("Missing 'job_id' parameter".to_string()),
                    },
                    facts: Vec::new(),
                });
            }
        };

        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(10, |v| usize::try_from(v).unwrap_or(10));

        match synapse_cron::list_runs(&self.db, job_id, limit).await {
            Ok(runs) => {
                let latest_status = runs.first().map(|run| run.status.clone());
                let latest_duration_ms = runs.first().and_then(|run| run.duration_ms);
                let runs: Vec<RunView> = runs
                    .into_iter()
                    .map(|run| RunView {
                        id: run.id,
                        job_id: run.job_id,
                        started_at: run.started_at,
                        finished_at: run.finished_at,
                        status: run.status,
                        output: run.output.map(|out| truncate(&out, MAX_RUN_OUTPUT_CHARS)),
                        duration_ms: run.duration_ms,
                    })
                    .collect();

                let fact = cron_facts::build_job_run_history_fact(
                    self.name(),
                    job_id,
                    runs.len(),
                    latest_status.as_deref(),
                    latest_duration_ms,
                );

                Ok(ToolExecution {
                    result: ToolResult {
                        success: true,
                        output: serde_json::to_string_pretty(&runs)?,
                        error: None,
                    },
                    facts: vec![fact],
                })
            }
            Err(e) => Ok(ToolExecution {
                result: ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(e.to_string()),
                },
                facts: Vec::new(),
            }),
        }
    }
}

#[derive(Serialize)]
struct RunView {
    id: i64,
    job_id: String,
    started_at: chrono::DateTime<chrono::Utc>,
    finished_at: chrono::DateTime<chrono::Utc>,
    status: String,
    output: Option<String>,
    duration_ms: Option<i64>,
}

#[async_trait]
impl Tool for CronRunsTool {
    fn name(&self) -> &str {
        "cron_runs"
    }

    fn description(&self) -> &str {
        "List recent run history for a cron job"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" },
                "limit": { "type": "integer" }
            },
            "required": ["job_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action(&args).await?.result)
    }

    async fn execute_with_facts(
        &self,
        args: serde_json::Value,
    ) -> anyhow::Result<ToolExecution> {
        self.execute_action(&args).await
    }
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

// Tests removed -- require SurrealDB setup (async integration tests).
