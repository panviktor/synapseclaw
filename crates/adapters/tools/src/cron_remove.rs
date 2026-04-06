use super::traits::{Tool, ToolResult};
use crate::cron_facts;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{Db, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::tool::ToolExecution;

pub struct CronRemoveTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
    db: Arc<Surreal<Db>>,
}

impl CronRemoveTool {
    pub fn new(config: Arc<Config>, security: Arc<SecurityPolicy>, db: Arc<Surreal<Db>>) -> Self {
        Self {
            config,
            security,
            db,
        }
    }

    fn enforce_mutation_allowed(&self, action: &str) -> Option<ToolResult> {
        if !self.security.can_act() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Security policy: read-only mode, cannot perform '{action}'"
                )),
            });
        }

        if self.security.is_rate_limited() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".to_string()),
            });
        }

        if !self.security.record_action() {
            return Some(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".to_string()),
            });
        }

        None
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

        if let Some(blocked) = self.enforce_mutation_allowed("cron_remove") {
            return Ok(ToolExecution {
                result: blocked,
                facts: Vec::new(),
            });
        }

        match synapse_cron::remove_job(&self.db, job_id).await {
            Ok(()) => Ok(ToolExecution {
                result: ToolResult {
                    success: true,
                    output: format!("Removed cron job {job_id}"),
                    error: None,
                },
                facts: vec![cron_facts::build_removed_job_fact(
                    self.name(),
                    "remove",
                    job_id,
                )],
            }),
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

#[async_trait]
impl Tool for CronRemoveTool {
    fn name(&self) -> &str {
        "cron_remove"
    }

    fn description(&self) -> &str {
        "Remove a cron job by id"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": { "type": "string" }
            },
            "required": ["job_id"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action(&args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.execute_action(&args).await
    }
}

// Tests removed -- require SurrealDB setup (async integration tests).
