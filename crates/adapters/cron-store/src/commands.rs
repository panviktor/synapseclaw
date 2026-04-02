//! CLI command enum and handler for cron subcommands.

use crate::*;
use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use surrealdb::engine::local::Db;
use surrealdb::Surreal;
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
pub async fn handle_command(
    command: CronCommands,
    db: &Surreal<Db>,
    config: &Config,
) -> Result<()> {
    match command {
        CronCommands::List => {
            let jobs = list_jobs(db).await?;
            if jobs.is_empty() {
                println!("No scheduled tasks yet.");
                println!("\nUsage:");
                println!("  synapseclaw cron add '0 9 * * *' 'agent -m \"Good morning!\"'");
                return Ok(());
            }

            println!("Scheduled jobs ({}):", jobs.len());
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
                    db,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    false,
                )
                .await?;
                println!("Added agent cron job {}", job.id);
                println!("  Expr  : {}", job.expression);
                println!("  Next  : {}", job.next_run.to_rfc3339());
                println!("  Prompt: {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = crate::add_shell_job(db, config, None, schedule, &command).await?;
                println!("Added cron job {}", job.id);
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
                    db,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    true,
                )
                .await?;
                println!("Added one-shot agent cron job {}", job.id);
                println!("  At    : {}", job.next_run.to_rfc3339());
                println!("  Prompt: {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = crate::add_shell_job(db, config, None, schedule, &command).await?;
                println!("Added one-shot cron job {}", job.id);
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
                    db,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    false,
                )
                .await?;
                println!("Added interval agent cron job {}", job.id);
                println!("  Every(ms): {every_ms}");
                println!("  Next     : {}", job.next_run.to_rfc3339());
                println!("  Prompt   : {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = crate::add_shell_job(db, config, None, schedule, &command).await?;
                println!("Added interval cron job {}", job.id);
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
                    db,
                    None,
                    schedule,
                    &command,
                    SessionTarget::Isolated,
                    None,
                    None,
                    true,
                )
                .await?;
                println!("Added one-shot agent cron job {}", job.id);
                println!("  At    : {}", job.next_run.to_rfc3339());
                println!("  Prompt: {}", job.prompt.as_deref().unwrap_or_default());
            } else {
                let job = add_once(db, config, &delay, &command).await?;
                println!("Added one-shot cron job {}", job.id);
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
                let existing = get_job(db, &id).await?;
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

            let job = update_shell_job_with_approval(db, config, &id, patch, false).await?;
            println!("Updated cron job {}", job.id);
            println!("  Expr: {}", job.expression);
            println!("  Next: {}", job.next_run.to_rfc3339());
            println!("  Cmd : {}", job.command);
            Ok(())
        }
        CronCommands::Remove { id } => remove_job(db, &id).await,
        CronCommands::Pause { id } => {
            pause_job(db, &id).await?;
            println!("Paused cron job {id}");
            Ok(())
        }
        CronCommands::Resume { id } => {
            resume_job(db, &id).await?;
            println!("Resumed cron job {id}");
            Ok(())
        }
    }
}

pub(crate) async fn add_once(
    db: &Surreal<Db>,
    config: &Config,
    delay: &str,
    command: &str,
) -> Result<CronJob> {
    add_once_validated(db, config, delay, command, false).await
}

pub(crate) async fn add_once_at(
    db: &Surreal<Db>,
    config: &Config,
    at: chrono::DateTime<chrono::Utc>,
    command: &str,
) -> Result<CronJob> {
    add_once_at_validated(db, config, at, command, false).await
}

pub fn parse_delay(input: &str) -> Result<chrono::Duration> {
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

    #[test]
    fn parse_delay_minutes() {
        let d = parse_delay("30m").unwrap();
        assert_eq!(d.num_minutes(), 30);
    }

    #[test]
    fn parse_delay_hours() {
        let d = parse_delay("2h").unwrap();
        assert_eq!(d.num_hours(), 2);
    }

    #[test]
    fn parse_delay_seconds() {
        let d = parse_delay("90s").unwrap();
        assert_eq!(d.num_seconds(), 90);
    }

    #[test]
    fn parse_delay_days() {
        let d = parse_delay("1d").unwrap();
        assert_eq!(d.num_days(), 1);
    }

    #[test]
    fn parse_delay_default_unit_is_minutes() {
        let d = parse_delay("10").unwrap();
        assert_eq!(d.num_minutes(), 10);
    }

    #[test]
    fn parse_delay_empty_fails() {
        assert!(parse_delay("").is_err());
    }

    #[test]
    fn parse_delay_unknown_unit_fails() {
        assert!(parse_delay("5x").is_err());
    }
}
