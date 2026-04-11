use super::traits::{Tool, ToolResult};
use crate::cron_facts;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{CronJobPatch, Db, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::tool::ToolExecution;

pub struct CronUpdateTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
    db: Arc<Surreal<Db>>,
}

impl CronUpdateTool {
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

        let patch_val = match args.get("patch") {
            Some(v) => v.clone(),
            None => {
                return Ok(ToolExecution {
                    result: ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("Missing 'patch' parameter".to_string()),
                    },
                    facts: Vec::new(),
                });
            }
        };

        let patch = match serde_json::from_value::<CronJobPatch>(patch_val) {
            Ok(patch) => patch,
            Err(e) => {
                return Ok(ToolExecution {
                    result: ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Invalid patch payload: {e}")),
                    },
                    facts: Vec::new(),
                });
            }
        };
        let approved = args
            .get("approved")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if let Some(blocked) = self.enforce_mutation_allowed("cron_update") {
            return Ok(ToolExecution {
                result: blocked,
                facts: Vec::new(),
            });
        }

        match synapse_cron::update_shell_job_with_approval(
            &self.db,
            &self.config,
            job_id,
            patch,
            approved,
        )
        .await
        {
            Ok(job) => Ok(ToolExecution {
                result: ToolResult {
                    success: true,
                    output: serde_json::to_string_pretty(&job)?,
                    error: None,
                },
                facts: vec![cron_facts::build_job_fact(self.name(), "update", &job)],
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
impl Tool for CronUpdateTool {
    fn name(&self) -> &str {
        "cron_update"
    }

    fn description(&self) -> &str {
        "Patch an existing cron job (schedule, command, prompt, enabled, delivery, model, etc.)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "job_id": {
                    "type": "string",
                    "description": "ID of the cron job to update, as returned by cron_add or cron_list"
                },
                "patch": {
                    "type": "object",
                    "description": "Fields to update. Only include fields you want to change; omitted fields are left as-is.",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "New human-readable name for the job"
                        },
                        "enabled": {
                            "type": "boolean",
                            "description": "Enable or disable the job without deleting it"
                        },
                        "command": {
                            "type": "string",
                            "description": "New shell command (for shell jobs)"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "New agent prompt (for agent jobs)"
                        },
                        "model": {
                            "type": "string",
                            "description": "Model override for agent jobs, e.g. a configured route alias or provider-specific model id"
                        },
                        "session_target": {
                            "type": "string",
                            "enum": ["isolated", "main"],
                            "description": "Agent session context: 'isolated' starts fresh each run, 'main' reuses the primary session"
                        },
                        "delete_after_run": {
                            "type": "boolean",
                            "description": "If true, delete the job automatically after its first successful run"
                        },
                        // NOTE: oneOf is correct for OpenAI-compatible APIs (including OpenRouter).
                        // Gemini does not support oneOf in tool schemas; if Gemini native tool calling
                        // is ever wired up, SchemaCleanr::clean_for_gemini must be applied before
                        // tool specs are sent. See src/tools/schema.rs.
                        "schedule": {
                            "description": "New schedule for the job. Exactly one of three forms must be used.",
                            "oneOf": [
                                {
                                    "type": "object",
                                    "description": "Cron expression schedule (repeating). Example: {\"kind\":\"cron\",\"expr\":\"0 9 * * 1-5\",\"tz\":\"America/New_York\"}",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["cron"] },
                                        "expr": { "type": "string", "description": "Standard 5-field cron expression, e.g. '*/5 * * * *'" },
                                        "tz": { "type": "string", "description": "Optional IANA timezone name, e.g. 'America/New_York'. Defaults to UTC." }
                                    },
                                    "required": ["kind", "expr"]
                                },
                                {
                                    "type": "object",
                                    "description": "One-shot schedule at a specific UTC datetime. Example: {\"kind\":\"at\",\"at\":\"2025-12-31T23:59:00Z\"}",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["at"] },
                                        "at": { "type": "string", "description": "ISO 8601 UTC datetime string, e.g. '2025-12-31T23:59:00Z'" }
                                    },
                                    "required": ["kind", "at"]
                                },
                                {
                                    "type": "object",
                                    "description": "Repeating interval schedule in milliseconds. Example: {\"kind\":\"every\",\"every_ms\":3600000} runs every hour.",
                                    "properties": {
                                        "kind": { "type": "string", "enum": ["every"] },
                                        "every_ms": { "type": "integer", "description": "Interval in milliseconds, e.g. 3600000 for every hour" }
                                    },
                                    "required": ["kind", "every_ms"]
                                }
                            ]
                        },
                        "delivery": {
                            "type": "object",
                            "description": "Delivery config to send job output to a channel after each run. When provided, mode, channel, and to are all expected.",
                            "properties": {
                                "mode": {
                                    "type": "string",
                                    "enum": ["none", "announce"],
                                    "description": "'announce' sends output to the specified channel; 'none' disables delivery"
                                },
                                "channel": {
                                    "type": "string",
                                    "enum": ["telegram", "discord", "slack", "mattermost", "matrix"],
                                    "description": "Channel type to deliver output to"
                                },
                                "to": {
                                    "type": "string",
                                    "description": "Destination ID: Discord channel ID, Telegram chat ID, Slack channel name, etc."
                                },
                                "best_effort": {
                                    "type": "boolean",
                                    "description": "If true, a delivery failure does not fail the job itself. Defaults to true."
                                }
                            }
                        }
                    }
                },
                "approved": {
                    "type": "boolean",
                    "description": "Set true to explicitly approve medium/high-risk shell commands in supervised mode",
                    "default": false
                }
            },
            "required": ["job_id", "patch"]
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
