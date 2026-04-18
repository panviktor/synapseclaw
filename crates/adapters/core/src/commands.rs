//! CLI command enums — shared between synapse_adapters and the binary.

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// List learned/runtime-generated skills from memory
    Learned {
        /// Agent id whose learned skills should be listed
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of learned skills to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// List memory-backed user-authored/manual skills
    Authored {
        /// Agent id whose user-authored skills should be listed
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of user-authored skills to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Create a memory-backed user-authored skill from markdown text or file
    Create {
        /// Skill name. Optional when --from-file SKILL.md has frontmatter name.
        #[arg(long)]
        name: Option<String>,
        /// Short skill description. Optional; inferred from body when omitted.
        #[arg(long)]
        description: Option<String>,
        /// Markdown skill body. Use --from-file for larger skills.
        #[arg(long)]
        body: Option<String>,
        /// Read markdown skill body and optional frontmatter from a file.
        #[arg(long = "from-file")]
        from_file: Option<PathBuf>,
        /// Optional task family hint used by retrieval/governance.
        #[arg(long)]
        task_family: Option<String>,
        /// Tool hint; repeat for ordered tool patterns.
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// Tag; repeat for multiple tags.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Initial status: active or candidate.
        #[arg(long, default_value = "active")]
        status: String,
        /// Agent id that should own the skill.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Export a memory-backed skill as an editable SKILL.md package
    Export {
        /// Skill id or exact skill name to export
        skill: String,
        /// Agent id that owns the memory-backed skill
        #[arg(long)]
        agent: Option<String>,
        /// Optional package directory path. Defaults to workspace/skills/<skill-name>.
        #[arg(long = "to")]
        to: Option<PathBuf>,
        /// Optional package directory name under the workspace skills directory.
        #[arg(long = "name")]
        package_name: Option<String>,
        /// Overwrite an existing SKILL.md in the destination directory.
        #[arg(long)]
        overwrite: bool,
    },
    /// Update a memory-backed manual/learned skill and record a rollback version
    Update {
        /// Skill id or exact skill name to update
        skill: String,
        /// New short skill description
        #[arg(long)]
        description: Option<String>,
        /// New markdown skill body. Use --from-file for larger skills.
        #[arg(long)]
        body: Option<String>,
        /// Read replacement markdown body and optional frontmatter from a file.
        #[arg(long = "from-file")]
        from_file: Option<PathBuf>,
        /// Replacement task family hint.
        #[arg(long)]
        task_family: Option<String>,
        /// Replacement tool hint; repeat for ordered tool patterns.
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// Replacement tag; repeat for multiple tags.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Replacement status: active, candidate, or deprecated.
        #[arg(long)]
        status: Option<String>,
        /// Agent id that owns the skill.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Create a safe editable SKILL.md package skeleton
    Scaffold {
        /// Package directory name or skill name
        name: String,
        /// Short skill description
        #[arg(long)]
        description: Option<String>,
        /// Optional task family hint to include in frontmatter
        #[arg(long)]
        task_family: Option<String>,
        /// Tool hint; repeat for multiple tools
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// Tag; repeat for multiple tags
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Overwrite an existing empty/new scaffold SKILL.md
        #[arg(long)]
        overwrite: bool,
    },
    /// Review learned skills and print deterministic promotion/deprecation decisions
    Review {
        /// Agent id whose learned skills should be reviewed
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of learned skills to review
        #[arg(long, default_value = "100")]
        limit: usize,
        /// Apply deterministic review decisions to memory
        #[arg(long)]
        apply: bool,
    },
    /// Promote a learned skill candidate to active
    Promote {
        /// Skill id or exact skill name
        skill: String,
        /// Agent id that owns the learned skill
        #[arg(long)]
        agent: Option<String>,
    },
    /// Move a learned skill back to candidate status
    Demote {
        /// Skill id or exact skill name
        skill: String,
        /// Agent id that owns the learned skill
        #[arg(long)]
        agent: Option<String>,
    },
    /// Reject a learned skill by marking it deprecated
    Reject {
        /// Skill id or exact skill name
        skill: String,
        /// Agent id that owns the learned skill
        #[arg(long)]
        agent: Option<String>,
    },
    /// Show runtime governance status for installed skills
    Status,
    /// Show installed skills that are blocked by runtime governance
    Blocked,
    /// Show installed and learned skill candidates awaiting review
    Candidates {
        /// Agent id whose learned candidates should be listed
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of learned candidates to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Run replay/eval checks for a generated skill patch candidate
    Test {
        /// Skill patch candidate id or memory key
        candidate: String,
        /// Agent id whose patch candidate should be tested
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of patch candidates to search
        #[arg(long, default_value = "100")]
        limit: usize,
    },
    /// Show the runtime tool replay contract inventory used by skill replay/eval
    Tools,
    /// Show compact skill use traces recorded after live/replay skill execution
    Traces {
        /// Agent id whose skill use traces should be listed
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of traces to show
        #[arg(long, default_value = "25")]
        limit: usize,
    },
    /// Show read-only skill catalog health and cleanup recommendations
    Health {
        /// Agent id whose skill catalog should be inspected
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of skills to inspect
        #[arg(long, default_value = "100")]
        limit: usize,
        /// Maximum number of recent skill use traces to fold into the report
        #[arg(long = "trace-limit", default_value = "100")]
        trace_limit: usize,
        /// Apply eligible learned-skill cleanup lifecycle changes
        #[arg(long)]
        apply: bool,
    },
    /// Show a compact diff/review view for a generated skill patch candidate
    Diff {
        /// Skill patch candidate id or memory key
        candidate: String,
        /// Agent id whose patch candidate should be inspected
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of patch candidates to search
        #[arg(long, default_value = "100")]
        limit: usize,
    },
    /// Apply a generated skill patch candidate after replay/eval gates pass
    Apply {
        /// Skill patch candidate id or memory key
        candidate: String,
        /// Agent id whose patch candidate should be applied
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of patch candidates to search
        #[arg(long, default_value = "100")]
        limit: usize,
    },
    /// Show skill version records from patch applies, manual updates, and rollbacks
    Versions {
        /// Optional skill id or exact skill name to filter version records
        skill: Option<String>,
        /// Agent id whose skill versions should be inspected
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of change and rollback records to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Roll back a skill change using its change record or rollback snapshot id
    Rollback {
        /// Apply record id, candidate id, rollback snapshot id, or memory key
        rollback: String,
        /// Agent id whose skill should be rolled back
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of change records to search
        #[arg(long, default_value = "100")]
        limit: usize,
    },
    /// Evaluate or apply generated patch candidates through auto-promotion policy
    Autopromote {
        /// Agent id whose patch candidates should be inspected
        #[arg(long)]
        agent: Option<String>,
        /// Maximum number of patch candidates to inspect
        #[arg(long, default_value = "100")]
        limit: usize,
        /// Apply eligible patches; requires [skills.auto_promotion].enabled=true
        #[arg(long)]
        apply: bool,
    },
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

/// Re-export CronCommands from the cron crate.
pub use synapse_cron::commands::CronCommands;

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
    /// Migrate legacy SQLite brain.db to SurrealDB
    Migrate {
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

/// Pipeline subcommands (Phase 4.5)
#[derive(Subcommand, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PipelineCommands {
    /// Show pipeline graph (ASCII or Mermaid)
    Show {
        /// Pipeline name
        name: String,
        /// Output Mermaid syntax instead of ASCII
        #[arg(long)]
        mermaid: bool,
    },
    /// List dead letters (failed pipeline steps)
    DeadLetters {
        /// Max entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Show all (including retried/dismissed)
        #[arg(long)]
        all: bool,
    },
    /// Retry a dead letter
    Retry {
        /// Dead letter ID
        id: String,
    },
    /// Dismiss a dead letter without retrying
    Dismiss {
        /// Dead letter ID
        id: String,
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
