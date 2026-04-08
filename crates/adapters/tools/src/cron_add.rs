use super::traits::{Tool, ToolResult};
use crate::cron_facts;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{Db, DeliveryConfig, JobType, Schedule, SessionTarget, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    ScheduleAction, ScheduleFact, ScheduleJobType, ScheduleTarget, ToolFactPayload, TypedToolFact,
};
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::tool::ToolExecution;

pub struct CronAddTool {
    config: Arc<Config>,
    security: Arc<SecurityPolicy>,
    db: Arc<Surreal<Db>>,
    conversation_context: Option<Arc<dyn ConversationContextPort>>,
}

impl CronAddTool {
    pub fn new(
        config: Arc<Config>,
        security: Arc<SecurityPolicy>,
        db: Arc<Surreal<Db>>,
        conversation_context: Option<Arc<dyn ConversationContextPort>>,
    ) -> Self {
        Self {
            config,
            security,
            db,
            conversation_context,
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

    fn resolve_delivery_config(
        &self,
        args: &serde_json::Value,
    ) -> Result<Option<DeliveryConfig>, ToolResult> {
        let Some(raw) = args.get("delivery") else {
            return Ok(None);
        };

        let mut delivery =
            serde_json::from_value::<DeliveryConfig>(raw.clone()).map_err(|e| ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Invalid delivery config: {e}")),
            })?;

        if raw
            .get("target")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == "current_conversation")
        {
            let Some(ctx) = self
                .conversation_context
                .as_ref()
                .and_then(|port| port.get_current())
            else {
                return Err(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(
                        "No current conversation context available for delivery.target='current_conversation'"
                            .to_string(),
                    ),
                });
            };

            delivery.channel = Some(ctx.source_adapter);
            delivery.to = Some(ctx.reply_ref);
            delivery.thread_ref = ctx.thread_ref;
        }

        if delivery.mode.eq_ignore_ascii_case("announce")
            && (delivery.channel.as_deref().unwrap_or("").trim().is_empty()
                || delivery.to.as_deref().unwrap_or("").trim().is_empty())
        {
            return Err(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "delivery.mode='announce' requires either {channel,to} or target='current_conversation'"
                        .to_string(),
                ),
            });
        }

        Ok(Some(delivery))
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

        let schedule = match args.get("schedule") {
            Some(v) => match serde_json::from_value::<Schedule>(v.clone()) {
                Ok(schedule) => schedule,
                Err(e) => {
                    return Ok(ToolExecution {
                        result: ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!("Invalid schedule: {e}")),
                        },
                        facts: Vec::new(),
                    });
                }
            },
            None => {
                return Ok(ToolExecution {
                    result: ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some("Missing 'schedule' parameter".to_string()),
                    },
                    facts: Vec::new(),
                });
            }
        };

        let name = args
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let job_type = match args.get("job_type").and_then(serde_json::Value::as_str) {
            Some("agent") => JobType::Agent,
            Some("shell") => JobType::Shell,
            Some(other) => {
                return Ok(ToolExecution {
                    result: ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Invalid job_type: {other}")),
                    },
                    facts: Vec::new(),
                });
            }
            None => {
                if args.get("prompt").is_some() {
                    JobType::Agent
                } else {
                    JobType::Shell
                }
            }
        };

        let default_delete_after_run = matches!(schedule, Schedule::At { .. });
        let delete_after_run = args
            .get("delete_after_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default_delete_after_run);
        let approved = args
            .get("approved")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let result = match job_type {
            JobType::Shell => {
                let command = match args.get("command").and_then(serde_json::Value::as_str) {
                    Some(command) if !command.trim().is_empty() => command,
                    _ => {
                        return Ok(ToolExecution {
                            result: ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some("Missing 'command' for shell job".to_string()),
                            },
                            facts: Vec::new(),
                        });
                    }
                };

                if let Err(reason) = self.security.validate_command_execution(command, approved) {
                    return Ok(ToolExecution {
                        result: ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(reason),
                        },
                        facts: Vec::new(),
                    });
                }

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(ToolExecution {
                        result: blocked,
                        facts: Vec::new(),
                    });
                }

                synapse_cron::add_shell_job_with_approval(
                    &self.db,
                    &self.config,
                    name,
                    schedule,
                    command,
                    approved,
                )
                .await
            }
            JobType::Agent => {
                let prompt = match args.get("prompt").and_then(serde_json::Value::as_str) {
                    Some(prompt) if !prompt.trim().is_empty() => prompt,
                    _ => {
                        return Ok(ToolExecution {
                            result: ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some("Missing 'prompt' for agent job".to_string()),
                            },
                            facts: Vec::new(),
                        });
                    }
                };

                let session_target = match args.get("session_target") {
                    Some(v) => match serde_json::from_value::<SessionTarget>(v.clone()) {
                        Ok(target) => target,
                        Err(e) => {
                            return Ok(ToolExecution {
                                result: ToolResult {
                                    success: false,
                                    output: String::new(),
                                    error: Some(format!("Invalid session_target: {e}")),
                                },
                                facts: Vec::new(),
                            });
                        }
                    },
                    None => SessionTarget::Isolated,
                };

                let model = args
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                let delivery = match self.resolve_delivery_config(args) {
                    Ok(cfg) => cfg,
                    Err(result) => {
                        return Ok(ToolExecution {
                            result,
                            facts: Vec::new(),
                        });
                    }
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(ToolExecution {
                        result: blocked,
                        facts: Vec::new(),
                    });
                }

                synapse_cron::add_agent_job(
                    &self.db,
                    name,
                    schedule,
                    prompt,
                    session_target,
                    model,
                    delivery,
                    delete_after_run,
                )
                .await
            }
        };

        match result {
            Ok(job) => Ok(ToolExecution {
                result: ToolResult {
                    success: true,
                    output: serde_json::to_string_pretty(&json!({
                        "id": job.id,
                        "name": job.name,
                        "job_type": job.job_type,
                        "schedule": job.schedule,
                        "next_run": job.next_run,
                        "enabled": job.enabled
                    }))?,
                    error: None,
                },
                facts: vec![cron_facts::build_job_fact(self.name(), "create", &job)],
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
impl Tool for CronAddTool {
    fn name(&self) -> &str {
        "cron_add"
    }

    fn description(&self) -> &str {
        "Create a scheduled cron job (shell or agent) with cron/at/every schedules. \
         Use job_type='agent' with a prompt to run the AI agent on schedule. \
         To deliver output to a channel (Discord, Telegram, Slack, Mattermost, Matrix), set \
         delivery={\"mode\":\"announce\",\"channel\":\"discord\",\"to\":\"<channel_id_or_chat_id>\"} \
         or delivery={\"mode\":\"announce\",\"target\":\"current_conversation\"}. \
         This is the preferred tool for sending scheduled/delayed messages to users via channels."
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        if matches!(result, Some(result) if !result.success) {
            return Vec::new();
        }

        let job_type = match args.get("job_type").and_then(serde_json::Value::as_str) {
            Some("agent") => Some(ScheduleJobType::Agent),
            Some("shell") => Some(ScheduleJobType::Shell),
            None if args.get("prompt").is_some() => Some(ScheduleJobType::Agent),
            None if args.get("command").is_some() => Some(ScheduleJobType::Shell),
            _ => None,
        };

        let timezone = args
            .get("schedule")
            .and_then(|value| value.get("tz"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let session = args
            .get("session_target")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let delivery = args.get("delivery").and_then(|value| {
            value
                .get("channel")
                .and_then(serde_json::Value::as_str)
                .zip(value.get("to").and_then(serde_json::Value::as_str))
                .map(
                    |(channel, recipient)| ConversationDeliveryTarget::Explicit {
                        channel: channel.to_string(),
                        recipient: recipient.to_string(),
                        thread_ref: value
                            .get("thread_ref")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    },
                )
        });

        vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Schedule(ScheduleFact {
                action: ScheduleAction::Create,
                job_type,
                schedule_kind: None,
                job_id: None,
                annotation: None,
                timezone,
                target: if session.is_some() || delivery.is_some() {
                    Some(ScheduleTarget { session, delivery })
                } else {
                    None
                },
                run_count: None,
                last_status: None,
                last_duration_ms: None,
            }),
        }]
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Optional human-readable name for the job"
                },
                // NOTE: oneOf is correct for OpenAI-compatible APIs (including OpenRouter).
                // Gemini does not support oneOf in tool schemas; if Gemini native tool calling
                // is ever wired up, SchemaCleanr::clean_for_gemini must be applied before
                // tool specs are sent. See src/tools/schema.rs.
                "schedule": {
                    "description": "When to run the job. Exactly one of three forms must be used.",
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
                "job_type": {
                    "type": "string",
                    "enum": ["shell", "agent"],
                    "description": "Type of job: 'shell' runs a command, 'agent' runs the AI agent with a prompt"
                },
                "command": {
                    "type": "string",
                    "description": "Shell command to run (required when job_type is 'shell')"
                },
                "prompt": {
                    "type": "string",
                    "description": "Agent prompt to run on schedule (required when job_type is 'agent')"
                },
                "session_target": {
                    "type": "string",
                    "enum": ["isolated", "main"],
                    "description": "Agent session context: 'isolated' starts a fresh session each run, 'main' reuses the primary session"
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override for agent jobs, e.g. 'x-ai/grok-4-1-fast'"
                },
                "delivery": {
                    "type": "object",
                    "description": "Optional delivery config to send job output to a channel after each run. Use explicit channel/to or target='current_conversation' to deliver back here.",
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
                        "thread_ref": {
                            "type": "string",
                            "description": "Optional thread/topic reference for thread-capable channels"
                        },
                        "target": {
                            "type": "string",
                            "enum": ["current_conversation"],
                            "description": "Resolve delivery back to the current live conversation at creation time"
                        },
                        "best_effort": {
                            "type": "boolean",
                            "description": "If true, a delivery failure does not fail the job itself. Defaults to true."
                        }
                    }
                },
                "delete_after_run": {
                    "type": "boolean",
                    "description": "If true, the job is automatically deleted after its first successful run. Defaults to true for 'at' schedules."
                },
                "approved": {
                    "type": "boolean",
                    "description": "Set true to explicitly approve medium/high-risk shell commands in supervised mode",
                    "default": false
                }
            },
            "required": ["schedule"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(self.execute_action(&args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> anyhow::Result<ToolExecution> {
        self.execute_action(&args).await
    }
}

#[cfg(test)]
mod tests {
    use crate::cron_facts;
    use chrono::Utc;
    use std::collections::HashMap;
    use synapse_cron::{CronJob, DeliveryConfig, ExecutionMode, JobType, Schedule, SessionTarget};

    fn sample_job() -> CronJob {
        CronJob {
            id: "job_123".into(),
            expression: "0 9 * * *".into(),
            schedule: Schedule::Cron {
                expr: "0 9 * * *".into(),
                tz: Some("Europe/Berlin".into()),
            },
            command: "echo hi".into(),
            prompt: Some("Report status".into()),
            name: Some("report".into()),
            job_type: JobType::Agent,
            session_target: SessionTarget::Main,
            model: Some("openai/gpt-5.4".into()),
            enabled: true,
            delivery: DeliveryConfig {
                mode: "announce".into(),
                channel: Some("matrix".into()),
                to: Some("!room:example.org".into()),
                thread_ref: Some("thread-1".into()),
                best_effort: true,
            },
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
    fn build_job_fact_emits_typed_schedule_fields() {
        let fact = cron_facts::build_job_fact("cron_add", "create", &sample_job());

        assert_eq!(fact.tool_id, "cron_add");
        let projected = fact.projected_focus_entities();
        assert!(projected
            .iter()
            .any(|entity| entity.kind == "scheduled_job" && entity.name == "job_123"));
        assert!(projected
            .iter()
            .any(|entity| entity.kind == "session_target" && entity.name == "main"));
        assert!(fact
            .projected_subjects()
            .iter()
            .any(|subject| subject == "job_123"));
    }
}
