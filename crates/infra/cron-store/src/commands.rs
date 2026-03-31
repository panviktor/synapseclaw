//! CLI command enum and handler for cron subcommands.

use crate::*;
use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use synapse_domain::config::schema::Config;

/// Cron subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CronCommands {
    /// List all scheduled tasks
    List,
    /// Add a new scheduled task
    #[command(long_about = "\
Add a new recurring scheduled task.

Uses standard 5-field cron syntax: 'min hour day month weekday'. \
Times are evaluated in UTC by default; use --tz with an IANA \
timezone name to override.

Examples:
  synapseclaw cron add '0 9 * * 1-5' 'Good morning' --tz America/New_York --agent
  synapseclaw cron add '*/30 * * * *' 'Check system health' --agent
  synapseclaw cron add '*/5 * * * *' 'echo ok'")]
    Add {
        /// Cron expression
        expression: String,
        /// Optional IANA timezone (e.g. America/Los_Angeles)
        #[arg(long)]
        tz: Option<String>,
        /// Treat the argument as an agent prompt instead of a shell command
        #[arg(long)]
        agent: bool,
        /// Command (shell) or prompt (agent) to run
        command: String,
    },
    /// Add a one-shot scheduled task at an RFC3339 timestamp
    #[command(long_about = "\
Add a one-shot task that fires at a specific UTC timestamp.

The timestamp must be in RFC 3339 format (e.g. 2025-01-15T14:00:00Z).

Examples:
  synapseclaw cron add-at 2025-01-15T14:00:00Z 'Send reminder'
  synapseclaw cron add-at 2025-12-31T23:59:00Z 'Happy New Year!'")]
    AddAt {
        /// One-shot timestamp in RFC3339 format
        at: String,
        /// Treat the argument as an agent prompt instead of a shell command
        #[arg(long)]
        agent: bool,
        /// Command (shell) or prompt (agent) to run
        command: String,
    },
    /// Add a fixed-interval scheduled task
    #[command(long_about = "\
Add a task that repeats at a fixed interval.

Interval is specified in milliseconds. For example, 60000 = 1 minute.

Examples:
  synapseclaw cron add-every 60000 'Ping heartbeat'     # every minute
  synapseclaw cron add-every 3600000 'Hourly report'    # every hour")]
    AddEvery {
        /// Interval in milliseconds
        every_ms: u64,
        /// Treat the argument as an agent prompt instead of a shell command
        #[arg(long)]
        agent: bool,
        /// Command (shell) or prompt (agent) to run
        command: String,
    },
    /// Add a one-shot delayed task (e.g. "30m", "2h", "1d")
    #[command(long_about = "\
Add a one-shot task that fires after a delay from now.

Accepts human-readable durations: s (seconds), m (minutes), \
h (hours), d (days).

Examples:
  synapseclaw cron once 30m 'Run backup in 30 minutes'
  synapseclaw cron once 2h 'Follow up on deployment'
  synapseclaw cron once 1d 'Daily check'")]
    Once {
        /// Delay duration
        delay: String,
        /// Treat the argument as an agent prompt instead of a shell command
        #[arg(long)]
        agent: bool,
        /// Command (shell) or prompt (agent) to run
        command: String,
    },
    /// Remove a scheduled task
    Remove {
        /// Task ID
        id: String,
    },
    /// Update a scheduled task
    #[command(long_about = "\
Update one or more fields of an existing scheduled task.

Only the fields you specify are changed; others remain unchanged.

Examples:
  synapseclaw cron update <task-id> --expression '0 8 * * *'
  synapseclaw cron update <task-id> --tz Europe/London --name 'Morning check'
  synapseclaw cron update <task-id> --command 'Updated message'")]
    Update {
        /// Task ID
        id: String,
        /// New cron expression
        #[arg(long)]
        expression: Option<String>,
        /// New IANA timezone
        #[arg(long)]
        tz: Option<String>,
        /// New command to run
        #[arg(long)]
        command: Option<String>,
        /// New job name
        #[arg(long)]
        name: Option<String>,
    },
    /// Pause a scheduled task
    Pause {
        /// Task ID
        id: String,
    },
    /// Resume a paused task
    Resume {
        /// Task ID
        id: String,
    },
}

#[allow(clippy::needless_pass_by_value)]
pub fn handle_command(command: CronCommands, config: &Config) -> Result<()> {
    match command {
        CronCommands::List => {
            let jobs = list_jobs(config)?;
            if jobs.is_empty() {
                println!("No scheduled tasks yet.");
                println!("\nUsage:");
                println!("  synapseclaw cron add '0 9 * * *' 'agent -m \"Good morning!\"'");
                return Ok(());
            }

            println!("🕒 Scheduled jobs ({}):", jobs.len());
            for job in jobs {
                let last_run = job
                    .last_run
                    .map_or_else(|| "never".into(), |d| d.to_rfc3339());
                let last_status = job.last_status.unwrap_or_else(|| "n/a".into());
                println!(
                    "- {} | {:?} | next={} | last={} ({})",
                    job.id,
                    job.schedule,
                    job.next_run.to_rfc3339(),
                    last_run,
                    last_status,
                );
                if !job.command.is_empty() {
                    println!("    cmd: {}", job.command);
                }
                if let Some(prompt) = &job.prompt {
                    println!("    prompt: {prompt}");
                }
            }
            Ok(())
        }
        CronCommands::Add {
            expression,
            tz,
            agent,
            command,
        } => {
            let schedule = Schedule::Cron {
                expr: expression,
                tz,
            };
            if agent {
                let job = add_agent_job(
                    config,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    false,
                )?;
                println!("✅ Added agent cron job {}", job.id);
                println!("  Expr  : {}", job.expression);
                println!("  Next  : {}", job.next_run.to_rfc3339());
                println!("  Prompt: {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = add_shell_job(config, None, schedule, &command)?;
                println!("✅ Added cron job {}", job.id);
                println!("  Expr: {}", job.expression);
                println!("  Next: {}", job.next_run.to_rfc3339());
                println!("  Cmd : {}", job.command);
            }
            Ok(())
        }
        CronCommands::AddAt { at, agent, command } => {
            let at = chrono::DateTime::parse_from_rfc3339(&at)
                .map_err(|e| anyhow::anyhow!("Invalid RFC3339 timestamp for --at: {e}"))?
                .with_timezone(&chrono::Utc);
            let schedule = Schedule::At { at };
            if agent {
                let job = add_agent_job(
                    config,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    true,
                )?;
                println!("✅ Added one-shot agent cron job {}", job.id);
                println!("  At    : {}", job.next_run.to_rfc3339());
                println!("  Prompt: {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = add_shell_job(config, None, schedule, &command)?;
                println!("✅ Added one-shot cron job {}", job.id);
                println!("  At  : {}", job.next_run.to_rfc3339());
                println!("  Cmd : {}", job.command);
            }
            Ok(())
        }
        CronCommands::AddEvery {
            every_ms,
            agent,
            command,
        } => {
            let schedule = Schedule::Every { every_ms };
            if agent {
                let job = add_agent_job(
                    config,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    false,
                )?;
                println!("✅ Added interval agent cron job {}", job.id);
                println!("  Every(ms): {every_ms}");
                println!("  Next     : {}", job.next_run.to_rfc3339());
                println!("  Prompt   : {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = add_shell_job(config, None, schedule, &command)?;
                println!("✅ Added interval cron job {}", job.id);
                println!("  Every(ms): {every_ms}");
                println!("  Next     : {}", job.next_run.to_rfc3339());
                println!("  Cmd      : {}", job.command);
            }
            Ok(())
        }
        CronCommands::Once {
            delay,
            agent,
            command,
        } => {
            if agent {
                let duration = parse_delay(&delay)?;
                let at = chrono::Utc::now() + duration;
                let schedule = Schedule::At { at };
                let job = add_agent_job(
                    config,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    true,
                )?;
                println!("✅ Added one-shot agent cron job {}", job.id);
                println!("  At    : {}", job.next_run.to_rfc3339());
                println!("  Prompt: {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = add_once(config, &delay, &command)?;
                println!("✅ Added one-shot cron job {}", job.id);
                println!("  At  : {}", job.next_run.to_rfc3339());
                println!("  Cmd : {}", job.command);
            }
            Ok(())
        }
        CronCommands::Update {
            id,
            expression,
            tz,
            command,
            name,
        } => {
            if expression.is_none() && tz.is_none() && command.is_none() && name.is_none() {
                bail!("At least one of --expression, --tz, --command, or --name must be provided");
            }

            // Merge expression/tz with the existing schedule so that
            // --tz alone updates the timezone and --expression alone
            // preserves the existing timezone.
            let schedule = if expression.is_some() || tz.is_some() {
                let existing = get_job(config, &id)?;
                let (existing_expr, existing_tz) = match existing.schedule {
                    Schedule::Cron {
                        expr,
                        tz: existing_tz,
                    } => (expr, existing_tz),
                    _ => bail!("Cannot update expression/tz on a non-cron schedule"),
                };
                Some(Schedule::Cron {
                    expr: expression.unwrap_or(existing_expr),
                    tz: tz.or(existing_tz),
                })
            } else {
                None
            };

            let patch = CronJobPatch {
                schedule,
                command,
                name,
                ..CronJobPatch::default()
            };

            let job = update_shell_job_with_approval(config, &id, patch, false)?;
            println!("\u{2705} Updated cron job {}", job.id);
            println!("  Expr: {}", job.expression);
            println!("  Next: {}", job.next_run.to_rfc3339());
            println!("  Cmd : {}", job.command);
            Ok(())
        }
        CronCommands::Remove { id } => remove_job(config, &id),
        CronCommands::Pause { id } => {
            pause_job(config, &id)?;
            println!("⏸️  Paused cron job {id}");
            Ok(())
        }
        CronCommands::Resume { id } => {
            resume_job(config, &id)?;
            println!("▶️  Resumed cron job {id}");
            Ok(())
        }
    }
}

pub(crate) fn add_once(config: &Config, delay: &str, command: &str) -> Result<CronJob> {
    add_once_validated(config, delay, command, false)
}

pub(crate) fn add_once_at(
    config: &Config,
    at: chrono::DateTime<chrono::Utc>,
    command: &str,
) -> Result<CronJob> {
    add_once_at_validated(config, at, command, false)
}

pub fn pause_job(config: &Config, id: &str) -> Result<CronJob> {
    update_job(
        config,
        id,
        CronJobPatch {
            enabled: Some(false),
            ..CronJobPatch::default()
        },
    )
}

pub fn resume_job(config: &Config, id: &str) -> Result<CronJob> {
    update_job(
        config,
        id,
        CronJobPatch {
            enabled: Some(true),
            ..CronJobPatch::default()
        },
    )
}

pub(crate) fn parse_delay(input: &str) -> Result<chrono::Duration> {
    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("delay must not be empty");
    }
    let split = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    let (num, unit) = input.split_at(split);
    let amount: i64 = num.parse()?;
    let unit = if unit.is_empty() { "m" } else { unit };
    let duration = match unit {
        "s" => chrono::Duration::seconds(amount),
        "m" => chrono::Duration::minutes(amount),
        "h" => chrono::Duration::hours(amount),
        "d" => chrono::Duration::days(amount),
        _ => anyhow::bail!("unsupported delay unit '{unit}', use s/m/h/d"),
    };
    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        config
    }

    fn make_job(config: &Config, expr: &str, tz: Option<&str>, cmd: &str) -> CronJob {
        add_shell_job(
            config,
            None,
            Schedule::Cron {
                expr: expr.into(),
                tz: tz.map(Into::into),
            },
            cmd,
        )
        .unwrap()
    }

    fn run_update(
        config: &Config,
        id: &str,
        expression: Option<&str>,
        tz: Option<&str>,
        command: Option<&str>,
        name: Option<&str>,
    ) -> Result<()> {
        handle_command(
            CronCommands::Update {
                id: id.into(),
                expression: expression.map(Into::into),
                tz: tz.map(Into::into),
                command: command.map(Into::into),
                name: name.map(Into::into),
            },
            config,
        )
    }

    #[test]
    fn update_changes_command_via_handler() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = make_job(&config, "*/5 * * * *", None, "echo original");

        run_update(&config, &job.id, None, None, Some("echo updated"), None).unwrap();

        let updated = get_job(&config, &job.id).unwrap();
        assert_eq!(updated.command, "echo updated");
        assert_eq!(updated.id, job.id);
    }

    #[test]
    fn update_changes_expression_via_handler() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = make_job(&config, "*/5 * * * *", None, "echo test");

        run_update(&config, &job.id, Some("0 9 * * *"), None, None, None).unwrap();

        let updated = get_job(&config, &job.id).unwrap();
        assert_eq!(updated.expression, "0 9 * * *");
    }

    #[test]
    fn update_changes_name_via_handler() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = make_job(&config, "*/5 * * * *", None, "echo test");

        run_update(&config, &job.id, None, None, None, Some("new-name")).unwrap();

        let updated = get_job(&config, &job.id).unwrap();
        assert_eq!(updated.name.as_deref(), Some("new-name"));
    }

    #[test]
    fn update_tz_alone_sets_timezone() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = make_job(&config, "*/5 * * * *", None, "echo test");

        run_update(
            &config,
            &job.id,
            None,
            Some("America/Los_Angeles"),
            None,
            None,
        )
        .unwrap();

        let updated = get_job(&config, &job.id).unwrap();
        assert_eq!(
            updated.schedule,
            Schedule::Cron {
                expr: "*/5 * * * *".into(),
                tz: Some("America/Los_Angeles".into()),
            }
        );
    }

    #[test]
    fn update_expression_preserves_existing_tz() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = make_job(
            &config,
            "*/5 * * * *",
            Some("America/Los_Angeles"),
            "echo test",
        );

        run_update(&config, &job.id, Some("0 9 * * *"), None, None, None).unwrap();

        let updated = get_job(&config, &job.id).unwrap();
        assert_eq!(
            updated.schedule,
            Schedule::Cron {
                expr: "0 9 * * *".into(),
                tz: Some("America/Los_Angeles".into()),
            }
        );
    }

    #[test]
    fn update_preserves_unchanged_fields() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = add_shell_job(
            &config,
            Some("original-name".into()),
            Schedule::Cron {
                expr: "*/5 * * * *".into(),
                tz: None,
            },
            "echo original",
        )
        .unwrap();

        run_update(&config, &job.id, None, None, Some("echo changed"), None).unwrap();

        let updated = get_job(&config, &job.id).unwrap();
        assert_eq!(updated.command, "echo changed");
        assert_eq!(updated.name.as_deref(), Some("original-name"));
        assert_eq!(updated.expression, "*/5 * * * *");
    }

    #[test]
    fn update_no_flags_fails() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let job = make_job(&config, "*/5 * * * *", None, "echo test");

        let result = run_update(&config, &job.id, None, None, None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("At least one of"));
    }

    #[test]
    fn update_nonexistent_job_fails() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let result = run_update(
            &config,
            "nonexistent-id",
            None,
            None,
            Some("echo test"),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn update_security_allows_safe_command() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);
        assert!(security.is_command_allowed("echo safe"));
    }

    #[test]
    fn add_shell_job_requires_explicit_approval_for_medium_risk() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into(), "touch".into()];

        let denied = add_shell_job(
            &config,
            None,
            Schedule::Cron {
                expr: "*/5 * * * *".into(),
                tz: None,
            },
            "touch cron-medium-risk",
        );
        assert!(denied.is_err());
        assert!(denied
            .unwrap_err()
            .to_string()
            .contains("explicit approval"));

        let approved = add_shell_job_with_approval(
            &config,
            None,
            Schedule::Cron {
                expr: "*/5 * * * *".into(),
                tz: None,
            },
            "touch cron-medium-risk",
            true,
        );
        assert!(approved.is_ok(), "{approved:?}");
    }

    #[test]
    fn update_requires_explicit_approval_for_medium_risk() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into(), "touch".into()];
        let job = make_job(&config, "*/5 * * * *", None, "echo original");

        let denied = update_shell_job_with_approval(
            &config,
            &job.id,
            CronJobPatch {
                command: Some("touch cron-medium-risk-update".into()),
                ..CronJobPatch::default()
            },
            false,
        );
        assert!(denied.is_err());
        assert!(denied
            .unwrap_err()
            .to_string()
            .contains("explicit approval"));

        let approved = update_shell_job_with_approval(
            &config,
            &job.id,
            CronJobPatch {
                command: Some("touch cron-medium-risk-update".into()),
                ..CronJobPatch::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(approved.command, "touch cron-medium-risk-update");
    }

    #[test]
    fn cli_update_requires_explicit_approval_for_medium_risk() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into(), "touch".into()];
        let job = make_job(&config, "*/5 * * * *", None, "echo original");

        let result = run_update(
            &config,
            &job.id,
            None,
            None,
            Some("touch cron-cli-medium-risk"),
            None,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("explicit approval"));
    }

    #[test]
    fn add_once_validated_creates_one_shot_job() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let job = add_once_validated(&config, "1h", "echo one-shot", false).unwrap();
        assert_eq!(job.command, "echo one-shot");
        assert!(matches!(job.schedule, Schedule::At { .. }));
    }

    #[test]
    fn add_once_validated_blocks_disallowed_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into()];
        config.autonomy.level = synapse_domain::domain::config::AutonomyLevel::Supervised;

        let result = add_once_validated(&config, "1h", "curl https://example.com", false);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("blocked by security policy"));
    }

    #[test]
    fn add_once_at_validated_creates_one_shot_job() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);
        let at = chrono::Utc::now() + chrono::Duration::hours(1);

        let job = add_once_at_validated(&config, at, "echo at-shot", false).unwrap();
        assert_eq!(job.command, "echo at-shot");
        assert!(matches!(job.schedule, Schedule::At { .. }));
    }

    #[test]
    fn add_once_at_validated_blocks_medium_risk_without_approval() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into(), "touch".into()];
        let at = chrono::Utc::now() + chrono::Duration::hours(1);

        let denied = add_once_at_validated(&config, at, "touch at-medium", false);
        assert!(denied.is_err());
        assert!(denied
            .unwrap_err()
            .to_string()
            .contains("explicit approval"));

        let approved = add_once_at_validated(&config, at, "touch at-medium", true);
        assert!(approved.is_ok(), "{approved:?}");
    }

    #[test]
    fn gateway_api_path_validates_shell_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into()];
        config.autonomy.level = synapse_domain::domain::config::AutonomyLevel::Supervised;

        // Simulate gateway API path: add_shell_job_with_approval(approved=false)
        let result = add_shell_job_with_approval(
            &config,
            None,
            Schedule::Cron {
                expr: "*/5 * * * *".into(),
                tz: None,
            },
            "curl https://example.com",
            false,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("blocked by security policy"));
    }

    #[test]
    fn scheduler_path_validates_shell_command() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into()];
        config.autonomy.level = synapse_domain::domain::config::AutonomyLevel::Supervised;

        let security = security_policy_from_config(&config.autonomy, &config.workspace_dir);
        // Simulate scheduler validation path
        let result =
            validate_shell_command_with_security(&security, "curl https://example.com", false);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("blocked by security policy"));
    }

    #[test]
    fn cli_agent_flag_creates_agent_job() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            CronCommands::Add {
                expression: "*/15 * * * *".into(),
                tz: None,
                agent: true,
                command: "Check server health: disk space, memory, CPU load".into(),
            },
            &config,
        )
        .unwrap();

        let jobs = list_jobs(&config).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_type, JobType::Agent);
        assert_eq!(
            jobs[0].prompt.as_deref(),
            Some("Check server health: disk space, memory, CPU load")
        );
    }

    #[test]
    fn cli_agent_flag_bypasses_shell_security_validation() {
        let tmp = TempDir::new().unwrap();
        let mut config = test_config(&tmp);
        config.autonomy.allowed_commands = vec!["echo".into()];
        config.autonomy.level = synapse_domain::domain::config::AutonomyLevel::Supervised;

        // Without --agent, a natural language string would be blocked by shell
        // security policy. With --agent, it routes to agent job and skips
        // shell validation entirely.
        let result = handle_command(
            CronCommands::Add {
                expression: "*/15 * * * *".into(),
                tz: None,
                agent: true,
                command: "Check server health: disk space, memory, CPU load".into(),
            },
            &config,
        );
        assert!(result.is_ok());

        let jobs = list_jobs(&config).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_type, JobType::Agent);
    }

    #[test]
    fn cli_without_agent_flag_defaults_to_shell_job() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        handle_command(
            CronCommands::Add {
                expression: "*/5 * * * *".into(),
                tz: None,
                agent: false,
                command: "echo ok".into(),
            },
            &config,
        )
        .unwrap();

        let jobs = list_jobs(&config).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_type, JobType::Shell);
        assert_eq!(jobs[0].command, "echo ok");
    }
}
