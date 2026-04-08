use super::traits::{Tool, ToolResult};
use crate::cron_facts;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{Db, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::tool_fact::TypedToolFact;
use synapse_domain::ports::tool::ToolExecution;

pub struct CronListTool {
    config: Arc<Config>,
    db: Arc<Surreal<Db>>,
}

impl CronListTool {
    pub fn new(config: Arc<Config>, db: Arc<Surreal<Db>>) -> Self {
        Self { config, db }
    }

    async fn execute_action(&self) -> anyhow::Result<ToolExecution> {
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

        match synapse_cron::list_jobs(&self.db).await {
            Ok(jobs) => {
                let facts = collect_list_facts(self.name(), &jobs);
                Ok(ToolExecution {
                    result: ToolResult {
                        success: true,
                        output: serde_json::to_string_pretty(&jobs)?,
                        error: None,
                    },
                    facts,
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

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str {
        "cron_list"
    }

    fn description(&self) -> &str {
        "List all scheduled cron jobs"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action().await?.result)
    }

    async fn execute_with_facts(&self, _args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.execute_action().await
    }
}

fn collect_list_facts(tool_name: &str, jobs: &[synapse_cron::CronJob]) -> Vec<TypedToolFact> {
    jobs.iter()
        .take(3)
        .map(|job| cron_facts::build_job_fact(tool_name, "list", job))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::collect_list_facts;
    use chrono::Utc;
    use std::collections::HashMap;
    use synapse_cron::{CronJob, DeliveryConfig, ExecutionMode, JobType, Schedule, SessionTarget};

    fn sample_job(id: &str) -> CronJob {
        CronJob {
            id: id.into(),
            expression: "0 9 * * *".into(),
            schedule: Schedule::Cron {
                expr: "0 9 * * *".into(),
                tz: None,
            },
            command: "echo hi".into(),
            prompt: None,
            name: None,
            job_type: JobType::Shell,
            session_target: SessionTarget::Isolated,
            model: None,
            enabled: true,
            delivery: DeliveryConfig::default(),
            delete_after_run: false,
            execution_mode: ExecutionMode::InProcess,
            env_overlay: HashMap::new(),
            allowed_tools: None,
            created_at: Utc::now(),
            next_run: Utc::now(),
            last_run: None,
            last_status: None,
            last_output: None,
        }
    }

    #[test]
    fn collect_list_facts_limits_and_labels_jobs() {
        let facts = collect_list_facts(
            "cron_list",
            &[
                sample_job("job_1"),
                sample_job("job_2"),
                sample_job("job_3"),
                sample_job("job_4"),
            ],
        );

        assert_eq!(facts.len(), 3);
        assert!(facts.iter().all(|fact| fact.tool_id == "cron_list"));
        assert_eq!(facts[0].projected_focus_entities()[0].kind, "scheduled_job");
    }
}
