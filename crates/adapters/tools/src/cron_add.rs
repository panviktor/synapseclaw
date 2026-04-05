use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_cron::{Db, DeliveryConfig, JobType, Schedule, SessionTarget, Surreal};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::dialogue_state::DialogueSlot;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::ports::agent_runtime::AgentToolFact;
use synapse_domain::ports::conversation_context::ConversationContextPort;

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

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        _result: Option<&ToolResult>,
    ) -> Vec<AgentToolFact> {
        let mut fact = AgentToolFact {
            tool_name: self.name().to_string(),
            focus_entities: Vec::new(),
            slots: Vec::new(),
        };

        if let Some(job_type) = args.get("job_type").and_then(serde_json::Value::as_str) {
            fact.slots
                .push(DialogueSlot::observed("job_type", job_type.to_string()));
        }

        if let Some(session_target) = args
            .get("session_target")
            .and_then(serde_json::Value::as_str)
        {
            fact.slots.push(DialogueSlot::observed(
                "session_target",
                session_target.to_string(),
            ));
        }

        if let Some(schedule) = args.get("schedule").and_then(serde_json::Value::as_object) {
            if let Some(kind) = schedule.get("kind").and_then(serde_json::Value::as_str) {
                fact.slots
                    .push(DialogueSlot::observed("schedule_kind", kind.to_string()));
            }
            if let Some(tz) = schedule.get("tz").and_then(serde_json::Value::as_str) {
                fact.slots
                    .push(DialogueSlot::observed("schedule_timezone", tz.to_string()));
            }
        }

        if let Ok(Some(delivery)) = self.resolve_delivery_config(args) {
            if let Some(mode) = (!delivery.mode.trim().is_empty()).then_some(delivery.mode) {
                fact.slots
                    .push(DialogueSlot::observed("delivery_mode", mode));
            }
            if let Some(channel) = delivery.channel {
                fact.slots
                    .push(DialogueSlot::observed("delivery_channel", channel));
            }
            if let Some(to) = delivery.to {
                fact.slots.push(DialogueSlot::observed("delivery_to", to));
            }
            if let Some(thread_ref) = delivery.thread_ref {
                fact.slots
                    .push(DialogueSlot::observed("delivery_thread_ref", thread_ref));
            }
        }

        if fact.slots.is_empty() {
            Vec::new()
        } else {
            vec![fact]
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.config.cron.enabled {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("cron is disabled by config (cron.enabled=false)".to_string()),
            });
        }

        let schedule = match args.get("schedule") {
            Some(v) => match serde_json::from_value::<Schedule>(v.clone()) {
                Ok(schedule) => schedule,
                Err(e) => {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Invalid schedule: {e}")),
                    });
                }
            },
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing 'schedule' parameter".to_string()),
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
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid job_type: {other}")),
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
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'command' for shell job".to_string()),
                        });
                    }
                };

                if let Err(reason) = self.security.validate_command_execution(command, approved) {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(reason),
                    });
                }

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
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
                        return Ok(ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some("Missing 'prompt' for agent job".to_string()),
                        });
                    }
                };

                let session_target = match args.get("session_target") {
                    Some(v) => match serde_json::from_value::<SessionTarget>(v.clone()) {
                        Ok(target) => target,
                        Err(e) => {
                            return Ok(ToolResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Invalid session_target: {e}")),
                            });
                        }
                    },
                    None => SessionTarget::Isolated,
                };

                let model = args
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                let delivery = match self.resolve_delivery_config(&args) {
                    Ok(cfg) => cfg,
                    Err(result) => return Ok(result),
                };

                if let Some(blocked) = self.enforce_mutation_allowed("cron_add") {
                    return Ok(blocked);
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
            Ok(job) => Ok(ToolResult {
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
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}
