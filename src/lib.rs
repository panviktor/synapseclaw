#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::assigning_clones,
    clippy::bool_to_int_with_if,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::cast_possible_wrap,
    clippy::doc_markdown,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::implicit_clone,
    clippy::items_after_statements,
    clippy::map_unwrap_or,
    clippy::manual_let_else,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::new_without_default,
    clippy::needless_pass_by_value,
    clippy::needless_raw_string_hashes,
    clippy::redundant_closure_for_method_calls,
    clippy::return_self_not_must_use,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unnecessary_cast,
    clippy::unnecessary_lazy_evaluations,
    clippy::unnecessary_literal_bound,
    clippy::unnecessary_map_or,
    clippy::unused_self,
    clippy::cast_precision_loss,
    clippy::unnecessary_wraps,
    dead_code
)]

use clap::Subcommand;
use serde::{Deserialize, Serialize};

pub mod agent;
pub use crate::fork_adapters::channels;
pub mod config;
pub(crate) mod fork_adapters;
pub use crate::fork_adapters::gateway;
/// Re-export fork_core workspace crate so `crate::fork_core::` paths keep working.
pub use fork_core;
pub(crate) mod identity;
pub mod memory;
pub(crate) mod multimodal;
pub use crate::fork_adapters::providers;
pub mod runtime;
pub(crate) mod security;
pub(crate) mod skills;
pub use crate::fork_adapters::tools;
/// Re-export hooks for integration tests.
pub use crate::fork_adapters::hooks;
/// Re-export observability for tests/benches.
pub use crate::fork_adapters::observability;
pub(crate) mod util;

pub use config::Config;

/// Gateway management subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GatewayCommands {
    /// Start the gateway server (default if no subcommand specified)
    #[command(long_about = "\
Start the gateway server (webhooks, websockets).

Runs the HTTP/WebSocket gateway that accepts incoming webhook events \
and WebSocket connections. Bind address defaults to the values in \
your config file (gateway.host / gateway.port).

Examples:
  synapseclaw gateway start              # use config defaults
  synapseclaw gateway start -p 8080      # listen on port 8080
  synapseclaw gateway start --host 0.0.0.0   # requires [gateway].allow_public_bind=true or a tunnel
  synapseclaw gateway start -p 0         # random available port")]
    Start {
        /// Port to listen on (use 0 for random available port); defaults to config gateway.port
        #[arg(short, long)]
        port: Option<u16>,

        /// Host to bind to; defaults to config gateway.host
        /// Note: Binding to 0.0.0.0 requires `gateway.allow_public_bind = true` in config
        #[arg(long)]
        host: Option<String>,
    },
    /// Restart the gateway server
    #[command(long_about = "\
Restart the gateway server.

Stops the running gateway if present, then starts a new instance \
with the current configuration.

Examples:
  synapseclaw gateway restart            # restart with config defaults
  synapseclaw gateway restart -p 8080    # restart on port 8080")]
    Restart {
        /// Port to listen on (use 0 for random available port); defaults to config gateway.port
        #[arg(short, long)]
        port: Option<u16>,

        /// Host to bind to; defaults to config gateway.host
        /// Note: Binding to 0.0.0.0 requires `gateway.allow_public_bind = true` in config
        #[arg(long)]
        host: Option<String>,
    },
    /// Show or generate the pairing code without restarting
    #[command(long_about = "\
Show or generate the gateway pairing code.

Displays the pairing code for connecting new clients without \
restarting the gateway. Requires the gateway to be running.

With --new, generates a fresh pairing code even if the gateway \
was previously paired (useful for adding additional clients).

Examples:
  synapseclaw gateway get-paircode       # show current pairing code
  synapseclaw gateway get-paircode --new # generate a new pairing code")]
    GetPaircode {
        /// Generate a new pairing code (even if already paired)
        #[arg(long)]
        new: bool,
    },
}

/// Service management subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ServiceCommands {
    /// Install daemon service unit for auto-start and restart
    Install,
    /// Start daemon service
    Start,
    /// Stop daemon service
    Stop,
    /// Restart daemon service to apply latest config
    Restart,
    /// Check daemon service status
    Status,
    /// Uninstall daemon service unit
    Uninstall,
}

/// Channel management subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChannelCommands {
    /// List all configured channels
    List,
    /// Start all configured channels (handled in main.rs for async)
    Start,
    /// Run health checks for configured channels (handled in main.rs for async)
    Doctor,
    /// Add a new channel configuration
    #[command(long_about = "\
Add a new channel configuration.

Provide the channel type and a JSON object with the required \
configuration keys for that channel type.

Supported types: telegram, discord, slack, whatsapp, matrix, imessage, email.

Examples:
  synapseclaw channel add telegram '{\"bot_token\":\"...\",\"name\":\"my-bot\"}'
  synapseclaw channel add discord '{\"bot_token\":\"...\",\"name\":\"my-discord\"}'")]
    Add {
        /// Channel type (telegram, discord, slack, whatsapp, matrix, imessage, email)
        channel_type: String,
        /// Optional configuration as JSON
        config: String,
    },
    /// Remove a channel configuration
    Remove {
        /// Channel name to remove
        name: String,
    },
    /// Bind a Telegram identity (username or numeric user ID) into allowlist
    #[command(long_about = "\
Bind a Telegram identity into the allowlist.

Adds a Telegram username (without the '@' prefix) or numeric user \
ID to the channel allowlist so the agent will respond to messages \
from that identity.

Examples:
  synapseclaw channel bind-telegram synapseclaw_user
  synapseclaw channel bind-telegram 123456789")]
    BindTelegram {
        /// Telegram identity to allow (username without '@' or numeric user ID)
        identity: String,
    },
    /// Send a message to a configured channel
    #[command(long_about = "\
Send a one-off message to a configured channel.

Sends a text message through the specified channel without starting \
the full agent loop. Useful for scripted notifications, hardware \
sensor alerts, and automation pipelines.

The --channel-id selects the channel by its config section name \
(e.g. 'telegram', 'discord', 'slack'). The --recipient is the \
platform-specific destination (e.g. a Telegram chat ID).

Examples:
  synapseclaw channel send 'Someone is near your device.' --channel-id telegram --recipient 123456789
  synapseclaw channel send 'Build succeeded!' --channel-id discord --recipient 987654321")]
    Send {
        /// Message text to send
        message: String,
        /// Channel config name (e.g. telegram, discord, slack)
        #[arg(long)]
        channel_id: String,
        /// Recipient identifier (platform-specific, e.g. Telegram chat ID)
        #[arg(long)]
        recipient: String,
    },
}

/// Skills management subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SkillCommands {
    /// List all installed skills
    List,
    /// Audit a skill source directory or installed skill name
    Audit {
        /// Skill path or installed skill name
        source: String,
    },
    /// Install a new skill from a URL or local path
    Install {
        /// Source URL or local path
        source: String,
    },
    /// Remove an installed skill
    Remove {
        /// Skill name to remove
        name: String,
    },
}

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

/// Memory management subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryCommands {
    /// List memory entries with optional filters
    List {
        /// Filter by category (core, daily, conversation, or custom name)
        #[arg(long)]
        category: Option<String>,
        /// Filter by session ID
        #[arg(long)]
        session: Option<String>,
        /// Maximum number of entries to display
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Number of entries to skip (for pagination)
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Get a specific memory entry by key
    Get {
        /// Memory key to look up
        key: String,
    },
    /// Show memory backend statistics and health
    Stats,
    /// Clear memories by category, by key, or clear all
    Clear {
        /// Delete a single entry by key (supports prefix match)
        #[arg(long)]
        key: Option<String>,
        /// Only clear entries in this category
        #[arg(long)]
        category: Option<String>,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

/// Integration subcommands
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IntegrationCommands {
    /// Show details about a specific integration
    Info {
        /// Integration name
        name: String,
    },
}
