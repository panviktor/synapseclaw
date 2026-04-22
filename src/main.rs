#![recursion_limit = "256"]
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
    clippy::needless_pass_by_value,
    clippy::needless_raw_string_hashes,
    clippy::redundant_closure_for_method_calls,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
    clippy::unused_self,
    clippy::cast_precision_loss,
    clippy::unnecessary_cast,
    clippy::unnecessary_lazy_evaluations,
    clippy::unnecessary_literal_bound,
    clippy::unnecessary_map_or,
    clippy::unnecessary_wraps,
    dead_code
)]

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use dialoguer::{Input, Password};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use tracing::{info, warn};
use tracing_subscriber::{fmt, EnvFilter};

fn parse_temperature(s: &str) -> std::result::Result<f64, String> {
    let t: f64 = s.parse().map_err(|e| format!("{e}"))?;
    config::schema::validate_temperature(t)
}

fn print_no_command_help() -> Result<()> {
    println!("No command provided.");
    println!("Try `synapseclaw onboard` to initialize your workspace.");
    println!();

    let mut cmd = Cli::command();
    cmd.print_help()?;
    println!();

    #[cfg(windows)]
    pause_after_no_command_help();

    Ok(())
}

#[cfg(windows)]
fn pause_after_no_command_help() {
    println!();
    print!("Press Enter to exit...");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}

// Use lib.rs exports — all code in workspace crates.
#[allow(unused_imports)]
use synapse_adapters::runtime;
pub use synapse_domain;
use synapse_security as security;
use synapseclaw::config::{self, Config, ConfigIO};
#[allow(unused_imports)]
use synapseclaw::{adapters, agent, memory};
#[allow(unused_imports)]
use synapseclaw::{
    ChannelCommands, CronCommands, GatewayCommands, IntegrationCommands, MemoryCommands,
    PipelineCommands, ServiceCommands, SkillCommands,
};

#[allow(unused_imports)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum CompletionShell {
    #[value(name = "bash")]
    Bash,
    #[value(name = "fish")]
    Fish,
    #[value(name = "zsh")]
    Zsh,
    #[value(name = "powershell")]
    PowerShell,
    #[value(name = "elvish")]
    Elvish,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum EstopLevelArg {
    #[value(name = "kill-all")]
    KillAll,
    #[value(name = "network-kill")]
    NetworkKill,
    #[value(name = "domain-block")]
    DomainBlock,
    #[value(name = "tool-freeze")]
    ToolFreeze,
}

/// `SynapseClaw` - Zero overhead. Zero compromise. 100% Rust.
#[derive(Parser, Debug)]
#[command(name = "synapseclaw")]
#[command(author = "panviktor")]
#[command(version)]
#[command(about = "The fastest, smallest AI assistant.", long_about = None)]
struct Cli {
    #[arg(long, global = true)]
    config_dir: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize your workspace and configuration
    Onboard {
        /// Overwrite existing config without confirmation
        #[arg(long)]
        force: bool,

        /// Reinitialize from scratch (backup and reset all configuration)
        #[arg(long)]
        reinit: bool,

        /// Reconfigure channels only (fast repair flow)
        #[arg(long)]
        channels_only: bool,

        /// API key for provider configuration
        #[arg(long)]
        api_key: Option<String>,

        /// Provider name (used in quick mode, default: openrouter)
        #[arg(long)]
        provider: Option<String>,
        /// Model ID override (used in quick mode)
        #[arg(long)]
        model: Option<String>,
        /// Memory backend (sqlite, lucid, markdown, none) - used in quick mode, default: sqlite
        #[arg(long)]
        memory: Option<String>,
    },

    /// Start the AI agent loop
    #[command(long_about = "\
Start the AI agent loop.

Launches an interactive chat session with the configured AI provider. \
Use --message for single-shot queries without entering interactive mode.

Examples:
  synapseclaw agent                              # interactive session
  synapseclaw agent -m \"Summarize today's logs\"  # single message
  synapseclaw agent -p <provider> --model <model-id>")]
    Agent {
        /// Single message mode (don't enter interactive mode)
        #[arg(short, long)]
        message: Option<String>,

        /// Load and save interactive session state in this JSON file
        #[arg(long)]
        session_state_file: Option<PathBuf>,

        /// Provider to use (openrouter, anthropic, openai, openai-codex)
        #[arg(short, long)]
        provider: Option<String>,

        /// Model to use
        #[arg(long)]
        model: Option<String>,

        /// Temperature (0.0 - 2.0, defaults to config default_temperature)
        #[arg(short, long, value_parser = parse_temperature)]
        temperature: Option<f64>,
    },

    /// Start/manage the gateway server (webhooks, websockets)
    #[command(long_about = "\
Manage the gateway server (webhooks, websockets).

Start, restart, or inspect the HTTP/WebSocket gateway that accepts \
incoming webhook events and WebSocket connections.

Examples:
  synapseclaw gateway start              # start gateway
  synapseclaw gateway restart            # restart gateway
  synapseclaw gateway get-paircode       # show pairing code")]
    Gateway {
        #[command(subcommand)]
        gateway_command: Option<synapseclaw::GatewayCommands>,
    },

    /// Start long-running autonomous runtime (gateway + channels + heartbeat + scheduler)
    #[command(long_about = "\
Start the long-running autonomous daemon.

Launches the full SynapseClaw runtime: gateway server, all configured \
channels (Telegram, Discord, Slack, etc.), heartbeat monitor, and \
the cron scheduler. This is the recommended way to run SynapseClaw in \
production or as an always-on assistant.

Use 'synapseclaw service install' to register the daemon as an OS \
service (systemd/launchd) for auto-start on boot.

Examples:
  synapseclaw daemon                   # use config defaults
  synapseclaw daemon -p 9090           # gateway on port 9090
  synapseclaw daemon --host 127.0.0.1  # localhost only")]
    Daemon {
        /// Port to listen on (use 0 for random available port); defaults to config gateway.port
        #[arg(short, long)]
        port: Option<u16>,

        /// Host to bind to; defaults to config gateway.host
        #[arg(long)]
        host: Option<String>,

        /// Run as a named agent instance (config dir: ~/.synapseclaw/agents/<name>/)
        #[arg(long)]
        instance: Option<String>,
    },

    /// Manage OS service lifecycle (launchd/systemd user service)
    Service {
        /// Init system to use: auto (detect), systemd, or openrc
        #[arg(long, default_value = "auto", value_parser = ["auto", "systemd", "openrc"])]
        service_init: String,

        /// Manage a named agent instance service (default: broker)
        #[arg(long)]
        instance: Option<String>,

        #[command(subcommand)]
        service_command: ServiceCommands,
    },

    /// Run diagnostics for daemon/scheduler/channel freshness
    Doctor {
        #[command(subcommand)]
        doctor_command: Option<DoctorCommands>,
    },

    /// Show system status (full details)
    Status,

    /// Engage, inspect, and resume emergency-stop states.
    ///
    /// Examples:
    /// - `synapseclaw estop`
    /// - `synapseclaw estop --level network-kill`
    /// - `synapseclaw estop --level domain-block --domain "*.chase.com"`
    /// - `synapseclaw estop --level tool-freeze --tool shell --tool browser`
    /// - `synapseclaw estop status`
    /// - `synapseclaw estop resume --network`
    /// - `synapseclaw estop resume --domain "*.chase.com"`
    /// - `synapseclaw estop resume --tool shell`
    Estop {
        #[command(subcommand)]
        estop_command: Option<EstopSubcommands>,

        /// Level used when engaging estop from `synapseclaw estop`.
        #[arg(long, value_enum)]
        level: Option<EstopLevelArg>,

        /// Domain pattern(s) for `domain-block` (repeatable).
        #[arg(long = "domain")]
        domains: Vec<String>,

        /// Tool name(s) for `tool-freeze` (repeatable).
        #[arg(long = "tool")]
        tools: Vec<String>,
    },

    /// Configure and manage scheduled tasks
    #[command(long_about = "\
Configure and manage scheduled tasks.

Schedule recurring, one-shot, or interval-based tasks using cron \
expressions, RFC 3339 timestamps, durations, or fixed intervals.

Cron expressions use the standard 5-field format: \
'min hour day month weekday'. Timezones default to UTC; \
override with --tz and an IANA timezone name.

Examples:
  synapseclaw cron list
  synapseclaw cron add '0 9 * * 1-5' 'Good morning' --tz America/New_York --agent
  synapseclaw cron add '*/30 * * * *' 'Check system health' --agent
  synapseclaw cron add '*/5 * * * *' 'echo ok'
  synapseclaw cron add-at 2025-01-15T14:00:00Z 'Send reminder' --agent
  synapseclaw cron add-every 60000 'Ping heartbeat'
  synapseclaw cron once 30m 'Run backup in 30 minutes' --agent
  synapseclaw cron pause <task-id>
  synapseclaw cron update <task-id> --expression '0 8 * * *' --tz Europe/London")]
    Cron {
        #[command(subcommand)]
        cron_command: CronCommands,
    },

    /// Manage provider model catalogs
    Models {
        #[command(subcommand)]
        model_command: ModelCommands,
    },

    /// Inspect and configure speech synthesis voices
    Voice {
        #[command(subcommand)]
        voice_command: VoiceCommands,
    },

    /// List supported AI providers
    Providers,

    /// Manage channels (telegram, discord, slack)
    #[command(long_about = "\
Manage communication channels.

Add, remove, list, send, and health-check channels that connect SynapseClaw \
to messaging platforms. Supported channel types: telegram, discord, \
slack, whatsapp, matrix, imessage, email.

Examples:
  synapseclaw channel list
  synapseclaw channel doctor
  synapseclaw channel add telegram '{\"bot_token\":\"...\",\"name\":\"my-bot\"}'
  synapseclaw channel remove my-bot
  synapseclaw channel bind-telegram synapseclaw_user
  synapseclaw channel send 'Alert!' --channel-id telegram --recipient 123456789")]
    Channel {
        #[command(subcommand)]
        channel_command: ChannelCommands,
    },

    /// Browse 50+ integrations
    Integrations {
        #[command(subcommand)]
        integration_command: IntegrationCommands,
    },

    /// Manage skills (user-defined capabilities)
    Skills {
        #[command(subcommand)]
        skill_command: SkillCommands,
    },

    /// Manage provider subscription authentication profiles
    Auth {
        #[command(subcommand)]
        auth_command: AuthCommands,
    },

    /// Manage agent memory (list, get, stats, clear)
    #[command(long_about = "\
Manage agent memory entries.

List, inspect, and clear memory entries stored by the agent. \
Supports filtering by category and session, pagination, and \
batch clearing with confirmation.

Examples:
  synapseclaw memory stats
  synapseclaw memory list
  synapseclaw memory list --category core --limit 10
  synapseclaw memory get <key>
  synapseclaw memory clear --category conversation --yes")]
    Memory {
        #[command(subcommand)]
        memory_command: MemoryCommands,
    },

    /// Pipeline management: show graph, dead letters, retry, dismiss
    Pipeline {
        #[command(subcommand)]
        pipeline_command: PipelineCommands,
    },

    /// Manage configuration
    #[command(long_about = "\
Manage SynapseClaw configuration.

Inspect and export configuration settings. Use 'schema' to dump \
the full JSON Schema for the config file, which documents every \
available key, type, and default value.

Examples:
  synapseclaw config schema              # print JSON Schema to stdout
  synapseclaw config schema > schema.json")]
    Config {
        #[command(subcommand)]
        config_command: ConfigCommands,
    },

    /// Generate shell completion script to stdout
    #[command(long_about = "\
Generate shell completion scripts for `synapseclaw`.

The script is printed to stdout so it can be sourced directly:

Examples:
  source <(synapseclaw completions bash)
  synapseclaw completions zsh > ~/.zfunc/_synapseclaw
  synapseclaw completions fish > ~/.config/fish/completions/synapseclaw.fish")]
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: CompletionShell,
    },

    /// Verify the HMAC audit chain integrity
    #[command(name = "audit")]
    Audit {
        #[command(subcommand)]
        audit_command: AuditCommands,
    },
}

#[derive(Subcommand, Debug)]
enum AuditCommands {
    /// Verify the HMAC chain in the audit log
    Verify,
}

#[derive(Subcommand, Debug)]
enum ConfigCommands {
    /// Dump the full configuration JSON Schema to stdout
    Schema,
}

#[derive(Subcommand, Debug)]
enum EstopSubcommands {
    /// Print current estop status.
    Status,
    /// Resume from an engaged estop level.
    Resume {
        /// Resume only network kill.
        #[arg(long)]
        network: bool,
        /// Resume one or more blocked domain patterns.
        #[arg(long = "domain")]
        domains: Vec<String>,
        /// Resume one or more frozen tools.
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// OTP code. If omitted and OTP is required, a prompt is shown.
        #[arg(long)]
        otp: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum AuthCommands {
    /// Login with OAuth (OpenAI Codex or Gemini)
    Login {
        /// Provider (`openai-codex` or `gemini`)
        #[arg(long)]
        provider: String,
        /// Profile name (default: default)
        #[arg(long, default_value = "default")]
        profile: String,
        /// Use OAuth device-code flow
        #[arg(long)]
        device_code: bool,
    },
    /// Complete OAuth by pasting redirect URL or auth code
    PasteRedirect {
        /// Provider (`openai-codex`)
        #[arg(long)]
        provider: String,
        /// Profile name (default: default)
        #[arg(long, default_value = "default")]
        profile: String,
        /// Full redirect URL or raw OAuth code
        #[arg(long)]
        input: Option<String>,
    },
    /// Paste setup token / auth token (for Anthropic subscription auth)
    PasteToken {
        /// Provider (`anthropic`)
        #[arg(long)]
        provider: String,
        /// Profile name (default: default)
        #[arg(long, default_value = "default")]
        profile: String,
        /// Token value (if omitted, read interactively)
        #[arg(long)]
        token: Option<String>,
        /// Auth kind override (`authorization` or `api-key`)
        #[arg(long)]
        auth_kind: Option<String>,
    },
    /// Alias for `paste-token` (interactive by default)
    SetupToken {
        /// Provider (`anthropic`)
        #[arg(long)]
        provider: String,
        /// Profile name (default: default)
        #[arg(long, default_value = "default")]
        profile: String,
    },
    /// Refresh OpenAI Codex access token using refresh token
    Refresh {
        /// Provider (`openai-codex`)
        #[arg(long)]
        provider: String,
        /// Profile name or profile id
        #[arg(long)]
        profile: Option<String>,
    },
    /// Remove auth profile
    Logout {
        /// Provider
        #[arg(long)]
        provider: String,
        /// Profile name (default: default)
        #[arg(long, default_value = "default")]
        profile: String,
    },
    /// Set active profile for a provider
    Use {
        /// Provider
        #[arg(long)]
        provider: String,
        /// Profile name or full profile id
        #[arg(long)]
        profile: String,
    },
    /// List auth profiles
    List,
    /// Show auth status with active profile and token expiry info
    Status,
}

#[derive(Subcommand, Debug)]
enum ModelCommands {
    /// Refresh and cache provider models
    Refresh {
        /// Provider name (defaults to configured default provider)
        #[arg(long)]
        provider: Option<String>,

        /// Refresh all providers that support live model discovery
        #[arg(long)]
        all: bool,

        /// Force live refresh and ignore fresh cache
        #[arg(long)]
        force: bool,
    },
    /// List cached models for a provider
    List {
        /// Provider name (defaults to configured default provider)
        #[arg(long)]
        provider: Option<String>,
    },
    /// Set the default model in config
    Set {
        /// Model name to set as default
        model: String,
    },
    /// Show current model configuration and cache status
    Status,
    /// Manage the local editable model catalog override
    Catalog {
        #[command(subcommand)]
        catalog_command: ModelCatalogCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ModelCatalogCommands {
    /// Initialize a local editable model catalog next to config.toml
    Init {
        /// Overwrite an existing local catalog file
        #[arg(long)]
        force: bool,
    },
    /// Show the local catalog path and whether an override is active
    Status,
    /// Print only the local catalog path
    Path,
}

#[derive(Subcommand, Debug)]
enum VoiceCommands {
    /// Show resolved speech synthesis configuration and candidate failover order
    Status,
    /// Inspect voice runtime prerequisites for CLI and local operator workflows
    Doctor {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Print channel delivery profiles used for voice/audio artifacts
    Profiles {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List supported voices for every resolved speech_synthesis candidate
    Voices {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Synthesize text to a local audio file using the speech_synthesis lane
    Synthesize {
        /// Text to synthesize
        #[arg(long)]
        text: String,
        /// Output path; defaults to workspace/voice_out/voice_<uuid>.<ext>
        #[arg(long)]
        output: Option<PathBuf>,
        /// Preferred speech synthesis provider
        #[arg(long)]
        provider: Option<String>,
        /// Preferred speech synthesis model
        #[arg(long)]
        model: Option<String>,
        /// Voice id
        #[arg(long)]
        voice: Option<String>,
        /// Preferred provider output format
        #[arg(long)]
        format: Option<String>,
    },
    /// Transcribe a local audio file using the speech_transcription lane
    Transcribe {
        /// Audio file path
        #[arg(long)]
        file: PathBuf,
        /// Optional provider override among configured STT providers
        #[arg(long)]
        provider: Option<String>,
    },
    /// Persist default voice and optional speech_synthesis lane selection
    Set {
        /// Voice id to store as the default voice
        #[arg(long)]
        voice: Option<String>,
        /// TTS provider for the first speech_synthesis lane candidate
        #[arg(long)]
        provider: Option<String>,
        /// TTS model for the first speech_synthesis lane candidate
        #[arg(long)]
        model: Option<String>,
        /// Provider output format preference, for example opus, mp3, wav, pcm, or flac
        #[arg(long)]
        format: Option<String>,
        /// Maximum text length passed to TTS
        #[arg(long)]
        max_text_length: Option<usize>,
    },
    /// Manage durable scoped voice preferences used by voice_reply
    Preference {
        /// Action: get, set, clear, or list
        action: String,
        /// Scope: global, channel, or conversation
        #[arg(long, default_value = "global")]
        scope: String,
        /// Channel adapter name for channel/conversation scope
        #[arg(long)]
        channel: Option<String>,
        /// Recipient/chat/room id for conversation scope
        #[arg(long)]
        recipient: Option<String>,
        /// Preferred speech synthesis provider
        #[arg(long)]
        provider: Option<String>,
        /// Preferred speech synthesis model
        #[arg(long)]
        model: Option<String>,
        /// Preferred voice id
        #[arg(long)]
        voice: Option<String>,
        /// Preferred output format
        #[arg(long)]
        format: Option<String>,
        /// Auto TTS policy: inherit, off, always, inbound_voice, tagged, channel_default, conversation_default
        #[arg(long)]
        auto_tts_policy: Option<String>,
    },
    /// Manage local CLI voice mode and run one-shot voice turns from local audio
    Mode {
        #[command(subcommand)]
        mode_command: VoiceModeCommands,
    },
    /// Start, speak into, or hang up realtime voice calls
    Call {
        #[command(subcommand)]
        call_command: VoiceCallCommands,
    },
}

#[derive(Subcommand, Debug)]
enum VoiceModeCommands {
    /// Show persisted CLI voice-mode settings and current environment readiness
    Status {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Enable local CLI voice mode
    On {
        /// Remember this session id for future one-shot turns
        #[arg(long)]
        session: Option<String>,
        /// Force local audio playback after synthesized replies
        #[arg(long)]
        playback: bool,
        /// Disable local audio playback after synthesized replies
        #[arg(long)]
        no_playback: bool,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Disable local CLI voice mode
    Off {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Run one local voice turn from an audio file through STT -> agent -> TTS
    Turn {
        /// Input audio file path
        #[arg(long)]
        file: PathBuf,
        /// Override remembered session id
        #[arg(long)]
        session: Option<String>,
        /// Force local audio playback after synthesized replies
        #[arg(long)]
        playback: bool,
        /// Disable local audio playback after synthesized replies
        #[arg(long)]
        no_playback: bool,
        /// Skip synthesis and return text only
        #[arg(long)]
        text_only: bool,
        /// Output path for synthesized reply audio
        #[arg(long)]
        output: Option<PathBuf>,
        /// Optional TTS provider override among configured speech_synthesis candidates
        #[arg(long)]
        provider: Option<String>,
        /// Optional TTS model override among configured speech_synthesis candidates
        #[arg(long)]
        model: Option<String>,
        /// Optional TTS voice override
        #[arg(long)]
        voice: Option<String>,
        /// Optional TTS format override
        #[arg(long)]
        format: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum VoiceCallCommands {
    /// Show configured and process-local realtime call runtime status
    Status {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List recent realtime call sessions from the shared runtime ledger
    Sessions {
        /// Realtime call runtime channel; required when multiple call transports are configured
        #[arg(long)]
        channel: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show one realtime call session from the shared runtime ledger
    Get {
        /// Realtime call runtime channel; required when multiple call transports are configured
        #[arg(long)]
        channel: Option<String>,
        /// Call id returned by call start
        #[arg(long)]
        call_control_id: String,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Start a confirmed realtime audio call
    Start {
        /// Realtime call runtime channel; required when multiple call transports are configured
        #[arg(long)]
        channel: Option<String>,
        /// Destination for the selected transport, for example a phone number, SIP URI, or Matrix user id
        #[arg(long)]
        to: String,
        /// Optional prompt metadata for the call runtime
        #[arg(long)]
        prompt: Option<String>,
        /// Primary goal of the call
        #[arg(long)]
        objective: Option<String>,
        /// Supporting context, constraints, or facts for the call
        #[arg(long)]
        context: Option<String>,
        /// Ordered agenda item; repeat the flag for multiple items
        #[arg(long)]
        agenda: Vec<String>,
        /// Confirm external telephony side effects
        #[arg(long)]
        confirm: bool,
    },
    /// Speak text into an active realtime call
    Speak {
        /// Realtime call runtime channel; required when multiple call transports are configured
        #[arg(long)]
        channel: Option<String>,
        /// Call control id returned by call start
        #[arg(long)]
        call_control_id: String,
        /// Text to speak into the call
        #[arg(long)]
        text: String,
        /// Confirm external telephony side effects
        #[arg(long)]
        confirm: bool,
    },
    /// Answer or attach to an inbound realtime call
    Answer {
        /// Realtime call runtime channel; required when multiple call transports are configured
        #[arg(long)]
        channel: Option<String>,
        /// Call control id for the inbound call session
        #[arg(long)]
        call_control_id: String,
        /// Confirm external call side effects or transport attachment
        #[arg(long)]
        confirm: bool,
    },
    /// Hang up an active realtime call
    Hangup {
        /// Realtime call runtime channel; required when multiple call transports are configured
        #[arg(long)]
        channel: Option<String>,
        /// Call control id returned by call start
        #[arg(long)]
        call_control_id: String,
        /// Confirm external telephony side effects
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Subcommand, Debug)]
enum DoctorCommands {
    /// Probe model catalogs across providers and report availability
    Models {
        /// Probe a specific provider only (default: all known providers)
        #[arg(long)]
        provider: Option<String>,

        /// Prefer cached catalogs when available (skip forced live refresh)
        #[arg(long)]
        use_cache: bool,
    },
    /// Query runtime trace events (tool diagnostics and model replies)
    Traces {
        /// Show a specific trace event by id
        #[arg(long)]
        id: Option<String>,
        /// Filter list output by event type
        #[arg(long)]
        event: Option<String>,
        /// Case-insensitive text match across message/payload
        #[arg(long)]
        contains: Option<String>,
        /// Maximum number of events to display
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

async fn handle_voice_command(config: &mut Config, command: VoiceCommands) -> Result<()> {
    use synapse_domain::application::services::media_artifact_delivery::{
        realtime_call_channel_profiles, tts_output_extension, tts_output_mime,
        tts_provider_output_format, voice_delivery_channel_profiles,
    };
    use synapse_domain::application::services::voice_preference_service::{
        read_voice_settings, write_voice_settings, VoicePreference, VoiceSettings,
    };
    use synapse_domain::config::schema::{
        ModelCandidateProfileConfig, ModelFeature, ModelLaneCandidateConfig,
    };
    use synapse_domain::ports::realtime_call::{
        RealtimeCallAnswerRequest, RealtimeCallHangupRequest, RealtimeCallOrigin,
        RealtimeCallRuntimePort, RealtimeCallSpeakRequest, RealtimeCallStartRequest,
    };
    use synapse_domain::ports::user_profile_store::UserProfileStorePort;

    match command {
        VoiceCommands::Status => {
            println!("Voice synthesis: {}", enabled_label(config.tts.enabled));
            println!("Default voice:    {}", config.tts.default_voice);
            println!("Base provider:    {}", config.tts.default_provider);
            println!("Base format:      {}", config.tts.default_format);
            println!("Max text length:  {}", config.tts.max_text_length);
            println!();

            match synapseclaw::channels::lane_selected_tts_candidate_configs(config) {
                Ok(candidates) => {
                    println!("Resolved speech_synthesis candidates:");
                    for (position, (lane_index, tts)) in candidates.iter().enumerate() {
                        let format = tts_provider_output_format(tts);
                        println!(
                            "  {}. lane_candidate={} provider={} model={} voice={} format={} ext=.{} mime={}",
                            position + 1,
                            lane_index,
                            tts.default_provider,
                            voice_selected_model(tts),
                            tts.default_voice,
                            format,
                            tts_output_extension(&format),
                            tts_output_mime(&format)
                        );
                    }
                }
                Err(error) => {
                    println!("Resolved speech_synthesis candidates: not ready");
                    println!("Reason: {error}");
                }
            }
            println!();
            match synapseclaw::channels::lane_selected_transcription_config(config) {
                Ok(transcription) => {
                    let providers =
                        match synapseclaw::channels::transcription::TranscriptionManager::new(
                            &transcription,
                        ) {
                            Ok(manager) => {
                                let mut providers = manager
                                    .available_providers()
                                    .into_iter()
                                    .map(ToString::to_string)
                                    .collect::<Vec<_>>();
                                providers.sort();
                                providers
                            }
                            Err(_) => Vec::new(),
                        };
                    println!("Resolved speech_transcription:");
                    println!("  provider: {}", transcription.default_provider);
                    println!("  model:    {}", transcription.model);
                    println!("  language: {:?}", transcription.language);
                    println!("  max_secs: {}", transcription.max_duration_secs);
                    println!("  available providers: {}", providers.join(", "));
                }
                Err(error) => {
                    println!("Resolved speech_transcription: not ready");
                    println!("Reason: {error}");
                }
            }
            let store_path = config
                .config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("user_profiles.json");
            if let Ok(store) =
                synapse_infra::user_profile_store::FileUserProfileStore::new(&store_path)
            {
                let mut items = store
                    .list()
                    .into_iter()
                    .filter(|(key, _)| key.starts_with("voice:"))
                    .map(|(key, profile)| {
                        serde_json::json!({
                            "key": key,
                            "settings": read_voice_settings(Some(profile)),
                        })
                    })
                    .collect::<Vec<_>>();
                items.sort_by(|a, b| {
                    a.get("key")
                        .and_then(|value| value.as_str())
                        .cmp(&b.get("key").and_then(|value| value.as_str()))
                });
                println!();
                if items.is_empty() {
                    println!("Stored voice preferences: none");
                } else {
                    println!("Stored voice preferences:");
                    for item in items {
                        println!("  {}", serde_json::to_string(&item)?);
                    }
                }
            }
            Ok(())
        }
        VoiceCommands::Doctor { json } => {
            let report = synapseclaw::channels::voice_doctor_report(config);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("CLI voice environment:");
                println!("  stdin tty:       {}", report.stdin_tty);
                println!("  stdout tty:      {}", report.stdout_tty);
                println!("  stderr tty:      {}", report.stderr_tty);
                println!("  ssh session:     {}", report.ssh_session);
                println!("  display:         {}", report.display);
                println!("  wayland:         {}", report.wayland);
                println!("  pulse socket:    {}", report.pulse_socket);
                println!("  pipewire socket: {}", report.pipewire_socket);
                println!(
                    "  runtime dir:     {}",
                    report
                        .audio_runtime_dir
                        .as_deref()
                        .unwrap_or("not detected")
                );
                println!(
                    "  playback bins:   {}",
                    if report.playback_binaries.is_empty() {
                        "none".into()
                    } else {
                        report.playback_binaries.join(", ")
                    }
                );
                println!(
                    "  record bins:     {}",
                    if report.recording_binaries.is_empty() {
                        "none".into()
                    } else {
                        report.recording_binaries.join(", ")
                    }
                );
                println!();
                println!("Speech synthesis ready: {}", report.speech_synthesis_ready);
                if report.speech_synthesis_candidates.is_empty() {
                    println!("  candidates: none");
                } else {
                    for candidate in &report.speech_synthesis_candidates {
                        println!(
                            "  lane_candidate={} provider={} model={} voice={} format={}",
                            candidate.lane_candidate_index,
                            candidate.provider,
                            candidate.model,
                            candidate.voice,
                            candidate.format
                        );
                    }
                }
                if let Some(error) = report.speech_synthesis_error.as_deref() {
                    println!("  error: {error}");
                }
                println!();
                println!(
                    "Speech transcription ready: {}",
                    report.speech_transcription_ready
                );
                if let Some(transcription) = report.speech_transcription.as_ref() {
                    println!("  provider: {}", transcription.provider);
                    println!("  model:    {}", transcription.model);
                    println!(
                        "  language: {}",
                        transcription.language.as_deref().unwrap_or("auto")
                    );
                    println!("  max_secs: {}", transcription.max_duration_secs);
                } else {
                    println!("  provider: not resolved");
                }
                if let Some(error) = report.speech_transcription_error.as_deref() {
                    println!("  error: {error}");
                }
                println!();
                println!("Notes:");
                for note in &report.notes {
                    println!("  - {note}");
                }
            }
            Ok(())
        }
        VoiceCommands::Profiles { json } => {
            let profiles = voice_delivery_channel_profiles(
                synapseclaw::channels::declared_channel_capability_profiles(),
            );
            let call_profiles = realtime_call_channel_profiles(
                synapseclaw::channels::declared_channel_capability_profiles(),
            );
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "voice_profiles": profiles,
                        "call_profiles": call_profiles,
                    }))?
                );
            } else {
                println!("Voice delivery profiles:");
                for profile in profiles {
                    println!(
                        "  {}: native_voice={:?}; fallback={:?}; notes={:?}",
                        profile.channel,
                        profile.native_voice_formats,
                        profile.fallback_mode,
                        profile.notes
                    );
                }
                println!("Realtime call profiles:");
                for profile in call_profiles {
                    println!(
                        "  {}: audio_call={}; video_call={}; notes={:?}",
                        profile.channel, profile.audio_call, profile.video_call, profile.notes
                    );
                }
            }
            Ok(())
        }
        VoiceCommands::Voices { json } => {
            let candidates = synapseclaw::channels::lane_selected_tts_candidate_configs(config)?;
            let voices = candidates
                .into_iter()
                .filter(|(_, tts)| tts.enabled)
                .map(|(lane_candidate_index, tts)| {
                    let manager = synapseclaw::channels::TtsManager::new(&tts)?;
                    let provider = tts.default_provider.clone();
                    let voices = manager.supported_voices(&provider)?;
                    let format = tts_provider_output_format(&tts);
                    Ok::<_, anyhow::Error>(serde_json::json!({
                        "lane_candidate_index": lane_candidate_index,
                        "provider": provider,
                        "model": voice_selected_model(&tts),
                        "default_voice": tts.default_voice,
                        "format": format,
                        "extension": tts_output_extension(&format),
                        "mime_type": tts_output_mime(&format),
                        "voices": voices,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&voices)?);
            } else {
                println!("Configured TTS voices:");
                for candidate in voices {
                    println!(
                        "  lane_candidate={} provider={} model={} default={} voices={}",
                        candidate["lane_candidate_index"],
                        candidate["provider"].as_str().unwrap_or("unknown"),
                        candidate["model"].as_str().unwrap_or("unknown"),
                        candidate["default_voice"].as_str().unwrap_or("unknown"),
                        candidate["voices"]
                            .as_array()
                            .map(|items| items.len())
                            .unwrap_or_default()
                    );
                    if let Some(list) = candidate["voices"].as_array() {
                        for voice in list {
                            if let Some(voice) = voice.as_str() {
                                println!("    {voice}");
                            }
                        }
                    }
                }
            }
            Ok(())
        }
        VoiceCommands::Synthesize {
            text,
            output,
            provider,
            model,
            voice,
            format,
        } => {
            let text = non_empty_cli_value("--text", text)?;
            let mut preference = VoicePreference {
                provider,
                model,
                voice,
                format,
            }
            .normalized();
            let mut tts = select_cli_tts_config(config, &preference)?;
            if let Some(voice) = preference.voice.take() {
                tts.default_voice = voice;
            }
            let manager = synapseclaw::channels::TtsManager::new(&tts)?;
            let bytes = manager.synthesize(&text).await?;
            if bytes.is_empty() {
                bail!("voice synthesis returned empty audio");
            }
            let provider_format = tts_provider_output_format(&tts);
            let extension = tts_output_extension(&provider_format);
            let output = match output {
                Some(path) => path,
                None => {
                    let dir = config.workspace_dir.join("voice_out");
                    tokio::fs::create_dir_all(&dir)
                        .await
                        .with_context(|| format!("failed to create {}", dir.display()))?;
                    dir.join(format!("voice_{}.{}", uuid::Uuid::new_v4(), extension))
                }
            };
            if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            tokio::fs::write(&output, &bytes)
                .await
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "ok",
                    "path": output,
                    "bytes": bytes.len(),
                    "provider": tts.default_provider,
                    "model": voice_selected_model(&tts),
                    "voice": tts.default_voice,
                    "format": provider_format,
                    "mime_type": tts_output_mime(&provider_format),
                }))?
            );
            Ok(())
        }
        VoiceCommands::Transcribe { file, provider } => {
            let transcription = synapseclaw::channels::lane_selected_transcription_config(config)?;
            if !transcription.enabled {
                bail!("voice transcription is not enabled");
            }
            let file_name = file
                .file_name()
                .and_then(|name| name.to_str())
                .context("--file must include a valid file name")?
                .to_string();
            let audio = tokio::fs::read(&file)
                .await
                .with_context(|| format!("failed to read {}", file.display()))?;
            let manager =
                synapseclaw::channels::transcription::TranscriptionManager::new(&transcription)?;
            let selected_provider = provider
                .as_deref()
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .unwrap_or(transcription.default_provider.as_str())
                .to_string();
            let text = if provider.is_some() {
                manager
                    .transcribe_with_provider(&audio, &file_name, &selected_provider)
                    .await?
            } else {
                manager.transcribe(&audio, &file_name).await?
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "ok",
                    "file": file,
                    "provider": selected_provider,
                    "text": text,
                }))?
            );
            Ok(())
        }
        VoiceCommands::Set {
            voice,
            provider,
            model,
            format,
            max_text_length,
        } => {
            if voice.is_none()
                && provider.is_none()
                && model.is_none()
                && format.is_none()
                && max_text_length.is_none()
            {
                bail!(
                    "`voice set` needs at least one of --voice, --provider, --model, --format, or --max-text-length"
                );
            }

            config.tts.enabled = true;

            if let Some(voice) = voice {
                let voice = non_empty_cli_value("--voice", voice)?;
                config.tts.default_voice = voice;
            }
            if let Some(format) = format {
                config.tts.default_format = non_empty_cli_value("--format", format)?;
            }
            if let Some(max_text_length) = max_text_length {
                if max_text_length == 0 {
                    bail!("--max-text-length must be greater than zero");
                }
                config.tts.max_text_length = max_text_length;
            }

            if provider.is_some() || model.is_some() {
                let provider = provider
                    .map(|value| non_empty_cli_value("--provider", value))
                    .transpose()?;
                let model = model
                    .map(|value| non_empty_cli_value("--model", value))
                    .transpose()?;
                let default_provider = {
                    let lane = ensure_speech_synthesis_lane(config);
                    if lane.candidates.is_empty() {
                        let Some(provider) = provider.clone() else {
                            bail!("--provider is required when creating the first speech_synthesis candidate");
                        };
                        let Some(model) = model.clone() else {
                            bail!("--model is required when creating the first speech_synthesis candidate");
                        };
                        lane.candidates.push(ModelLaneCandidateConfig {
                            provider,
                            model,
                            api_key: None,
                            api_key_env: None,
                            dimensions: None,
                            profile: ModelCandidateProfileConfig {
                                features: vec![ModelFeature::SpeechSynthesis],
                                ..ModelCandidateProfileConfig::default()
                            },
                        });
                    } else {
                        let candidate = &mut lane.candidates[0];
                        if let Some(provider) = provider {
                            candidate.provider = provider;
                        }
                        if let Some(model) = model {
                            candidate.model = model;
                        }
                        if !candidate
                            .profile
                            .features
                            .contains(&ModelFeature::SpeechSynthesis)
                        {
                            candidate
                                .profile
                                .features
                                .push(ModelFeature::SpeechSynthesis);
                        }
                    }
                    lane.candidates
                        .first()
                        .map(|candidate| candidate.provider.clone())
                };
                if let Some(provider) = default_provider {
                    config.tts.default_provider = provider;
                };
            }

            config.save().await?;
            println!(
                "Voice configuration saved to {}",
                config.config_path.display()
            );
            Ok(())
        }
        VoiceCommands::Preference {
            action,
            scope,
            channel,
            recipient,
            provider,
            model,
            voice,
            format,
            auto_tts_policy,
        } => {
            let scope = parse_voice_scope(&scope)?;
            let target = cli_voice_target(scope, channel, recipient)?;
            let store_path = config
                .config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("user_profiles.json");
            let store = synapse_infra::user_profile_store::FileUserProfileStore::new(&store_path)?;

            if action.eq_ignore_ascii_case("list") {
                let items = store
                    .list()
                    .into_iter()
                    .filter(|(key, _)| key.starts_with("voice:"))
                    .map(|(key, profile)| {
                        serde_json::json!({
                            "key": key,
                            "settings": read_voice_settings(Some(profile)),
                        })
                    })
                    .collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&items)?);
                return Ok(());
            }

            let key = target.storage_key().map_err(anyhow::Error::msg)?;
            if action.eq_ignore_ascii_case("get") {
                let settings = read_voice_settings(store.load(&key));
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key": key,
                        "target": target,
                        "settings": settings,
                    }))?
                );
                return Ok(());
            }

            if action.eq_ignore_ascii_case("clear") {
                let removed = store.remove(&key)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "ok",
                        "key": key,
                        "removed": removed,
                    }))?
                );
                return Ok(());
            }

            if !action.eq_ignore_ascii_case("set") {
                bail!("voice preference action must be get, set, clear, or list");
            }

            let preference = VoicePreference {
                provider,
                model,
                voice,
                format,
            }
            .normalized();
            let mut settings = read_voice_settings(store.load(&key));
            if !preference.is_empty() {
                validate_cli_voice_preference(config, &preference)?;
                settings.preference = Some(preference);
            }
            if let Some(policy) = auto_tts_policy {
                settings.auto_tts_policy = parse_auto_tts_policy(&policy)?;
            }
            if settings == VoiceSettings::default() {
                bail!("voice preference set requires a preference field or --auto-tts-policy");
            }
            if let Some(profile) = write_voice_settings(settings.clone()) {
                store.upsert(&key, profile)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "ok",
                    "key": key,
                    "target": target,
                    "settings": settings,
                }))?
            );
            Ok(())
        }
        VoiceCommands::Mode { mode_command } => match mode_command {
            VoiceModeCommands::Status { json } => {
                let settings = load_cli_voice_mode_settings(config)?;
                let doctor = synapseclaw::channels::voice_doctor_report(config);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "settings": settings,
                            "doctor": doctor,
                        }))?
                    );
                } else {
                    println!("CLI voice mode: {}", enabled_label(settings.enabled));
                    println!("Auto playback:  {}", settings.auto_playback);
                    println!(
                        "Session id:     {}",
                        settings.session_id.as_deref().unwrap_or("(none)")
                    );
                    println!();
                    println!("Environment notes:");
                    for note in doctor.notes {
                        println!("  - {note}");
                    }
                }
                Ok(())
            }
            VoiceModeCommands::On {
                session,
                playback,
                no_playback,
                json,
            } => {
                let mut settings = load_cli_voice_mode_settings(config)?;
                settings.enabled = true;
                settings.auto_playback = resolve_voice_mode_playback(
                    if settings == synapse_domain::application::services::voice_mode_service::VoiceModeSettings::default()
                    {
                        default_cli_voice_mode_playback(config)
                    } else {
                        settings.auto_playback
                    },
                    playback,
                    no_playback,
                )?;
                if let Some(session) = normalize_optional_cli_string(session) {
                    settings.session_id = Some(session);
                }
                let settings = settings.normalized();
                save_cli_voice_mode_settings(config, settings.clone())?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "settings": settings,
                        }))?
                    );
                } else {
                    println!("CLI voice mode enabled.");
                    println!("  auto_playback: {}", settings.auto_playback);
                    println!(
                        "  session_id:    {}",
                        settings.session_id.as_deref().unwrap_or("(none)")
                    );
                }
                Ok(())
            }
            VoiceModeCommands::Off { json } => {
                let mut settings = load_cli_voice_mode_settings(config)?;
                settings.enabled = false;
                let settings = settings.normalized();
                save_cli_voice_mode_settings(config, settings.clone())?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "settings": settings,
                        }))?
                    );
                } else {
                    println!("CLI voice mode disabled.");
                }
                Ok(())
            }
            VoiceModeCommands::Turn {
                file,
                session,
                playback,
                no_playback,
                text_only,
                output,
                provider,
                model,
                voice,
                format,
                json,
            } => {
                if text_only && output.is_some() {
                    bail!("--output cannot be used together with --text-only");
                }
                let settings = load_cli_voice_mode_settings(config)?;
                let session_id = normalize_optional_cli_string(session).or(settings.session_id);
                let playback_enabled =
                    resolve_voice_mode_playback(settings.auto_playback, playback, no_playback)?;
                let transcript = transcribe_cli_audio(config, &file, None).await?;
                let reply =
                    agent::process_message(config.clone(), &transcript.text, session_id.as_deref())
                        .await?;

                let audio = if text_only {
                    None
                } else {
                    let preference =
                        merge_voice_mode_preference(config, provider, model, voice, format)?;
                    Some(
                        synthesize_cli_audio(config, &reply, &preference, output, playback_enabled)
                            .await?,
                    )
                };

                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "mode_enabled": settings.enabled,
                            "session_id": session_id,
                            "transcript": transcript,
                            "reply": {
                                "text": reply,
                                "audio": audio,
                            }
                        }))?
                    );
                } else {
                    println!("Transcript:");
                    println!("{}", transcript.text);
                    println!();
                    println!("Reply:");
                    println!("{reply}");
                    if let Some(audio) = audio {
                        println!();
                        println!("Audio:");
                        println!("  path: {}", audio.path.display());
                        println!(
                            "  provider/model/voice: {}/{}/{}",
                            audio.provider, audio.model, audio.voice
                        );
                        if let Some(player) = audio.playback_player.as_deref() {
                            println!("  playback: {player}");
                        }
                        if let Some(error) = audio.playback_error.as_deref() {
                            println!("  playback_error: {error}");
                        }
                    }
                }
                Ok(())
            }
        },
        VoiceCommands::Call { call_command } => match call_command {
            VoiceCallCommands::Status { json } => {
                let report =
                    synapseclaw::channels::realtime_call_status_report_live_with_synapseclaw_dir(
                        &config.channels_config,
                        config.config_path.parent().map(|path| path.to_path_buf()),
                        synapseclaw::channels::lane_selected_tts_config(&config).ok(),
                        synapseclaw::channels::lane_selected_transcription_config(&config).ok(),
                    )
                    .await;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "report": report,
                        }))?
                    );
                } else {
                    println!(
                        "Default runtime: {}",
                        report.default_channel.as_deref().unwrap_or("none")
                    );
                    for status in report.channels {
                        let health = status.health.as_ref();
                        println!(
                            "{} transport_configured={} audio={:?} video={:?} media_attached={} selected={} ready={} active_calls={}",
                            status.channel,
                            status.transport_configured,
                            status.audio_call_runtime,
                            status.video_call_runtime,
                            status.media_attached,
                            status.runtime_selected_by_default,
                            status.runtime_ready,
                            health.map(|value| value.active_calls.len()).unwrap_or(0)
                        );
                        println!(
                            "  actions start={} answer={} speak={} hangup={} inspect={}",
                            status.action_support.start,
                            status.action_support.answer,
                            status.action_support.speak,
                            status.action_support.hangup,
                            status.action_support.inspect
                        );
                        if let Some(details) = status.details.as_ref() {
                            match details {
                                synapseclaw::channels::RealtimeCallTransportDetails::ClawdTalk {
                                    api_key_configured,
                                    websocket_configured,
                                    websocket_url,
                                    api_base_url,
                                    assistant_configured,
                                    bridge_ready,
                                    outbound_start_ready,
                                    call_control_ready,
                                } => {
                                    println!(
                                        "  clawdtalk api_key_configured={} websocket_configured={} assistant_configured={} bridge_ready={} outbound_start_ready={} call_control_ready={}",
                                        api_key_configured,
                                        websocket_configured,
                                        assistant_configured,
                                        bridge_ready,
                                        outbound_start_ready,
                                        call_control_ready
                                    );
                                    if let Some(url) = websocket_url.as_deref() {
                                        println!("  websocket_url={url}");
                                    }
                                    if let Some(url) = api_base_url.as_deref() {
                                        println!("  api_base_url={url}");
                                    }
                                }
                                synapseclaw::channels::RealtimeCallTransportDetails::Matrix {
                                    auth_mode,
                                    auth_source,
                                    widget_support_enabled,
                                    room_reference,
                                    resolved_room_id,
                                    room_accessible,
                                    room_encrypted,
                                    rtc_bootstrap,
                                    turn_engine,
                                } => {
                                    println!(
                                        "  matrix auth_mode={:?} auth_source={:?} widget_support_enabled={} room_accessible={} room_encrypted={}",
                                        auth_mode,
                                        auth_source,
                                        widget_support_enabled,
                                        room_accessible.map(|value| value.to_string()).unwrap_or_else(|| "unknown".into()),
                                        room_encrypted.map(|value| value.to_string()).unwrap_or_else(|| "unknown".into())
                                    );
                                    if let Some(room) = room_reference.as_deref() {
                                        println!("  room_reference={room}");
                                    }
                                    if let Some(room) = resolved_room_id.as_deref() {
                                        println!("  resolved_room_id={room}");
                                    }
                                    if let Some(bootstrap) = rtc_bootstrap.as_ref() {
                                        println!(
                                            "  rtc_bootstrap focus_source={:?} authorizer_api={:?} media_bootstrap_ready={}",
                                            bootstrap.focus_source,
                                            bootstrap.authorizer_api,
                                            bootstrap.media_bootstrap_ready
                                        );
                                        if let Some(url) = bootstrap.focus_url.as_deref() {
                                            println!("  rtc_focus_url={url}");
                                        }
                                        if let Some(path) = bootstrap.transports_api_path.as_deref() {
                                            println!("  rtc_transports_api_path={path}");
                                        }
                                        if let Some(supported) = bootstrap.transports_api_supported {
                                            println!("  rtc_transports_supported={supported}");
                                        }
                                        if let Some(healthy) = bootstrap.authorizer_healthy {
                                            println!("  rtc_authorizer_healthy={healthy}");
                                        }
                                        if let Some(openid_ready) = bootstrap.openid_token_ready {
                                            println!("  rtc_openid_token_ready={openid_ready}");
                                        }
                                        if let Some(grant_ready) = bootstrap.authorizer_grant_ready {
                                            println!("  rtc_authorizer_grant_ready={grant_ready}");
                                        }
                                        if let Some(url) = bootstrap.livekit_service_url.as_deref() {
                                            println!("  rtc_livekit_service_url={url}");
                                        }
                                        if let Some(error) = bootstrap.last_probe_error.as_deref() {
                                            println!("  rtc_last_probe_error={error}");
                                        }
                                    }
                                    if let Some(turn_engine) = turn_engine.as_ref() {
                                        println!(
                                            "  turn_engine provider={} configured={} ready={}",
                                            turn_engine.provider,
                                            turn_engine.configured,
                                            turn_engine.ready
                                        );
                                        if let Some(model) = turn_engine.model.as_deref() {
                                            println!("  turn_engine_model={model}");
                                        }
                                        if !turn_engine.language_hints.is_empty() {
                                            println!(
                                                "  turn_engine_language_hints={}",
                                                turn_engine.language_hints.join(",")
                                            );
                                        }
                                        if let Some(error) = turn_engine.last_error.as_deref() {
                                            println!("  turn_engine_last_error={error}");
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(health) = health {
                            if let Some(connected) = health.connected {
                                println!("  connected={connected}");
                            }
                            if let Some(reconnect_attempts) = health.reconnect_attempts {
                                println!("  reconnect_attempts={reconnect_attempts}");
                            }
                            if let Some(error) = health.last_error.as_deref() {
                                println!("  last_error={error}");
                            }
                        }
                    }
                }
                Ok(())
            }
            VoiceCallCommands::Sessions { channel, json } => {
                let channel =
                    synapseclaw::channels::resolve_realtime_audio_call_inspection_channel(
                        channel.as_deref(),
                        &config.channels_config,
                    )?;
                let sessions =
                    synapseclaw::channels::list_realtime_audio_call_sessions_with_synapseclaw_dir(
                        &channel,
                        &config.channels_config,
                        config.config_path.parent().map(|path| path.to_path_buf()),
                    )?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "channel": channel,
                            "sessions": sessions,
                        }))?
                    );
                } else if sessions.is_empty() {
                    println!("No realtime call sessions.");
                } else {
                    for session in sessions {
                        println!(
                            "{} {} {:?} source={}{}",
                            session.call_control_id,
                            session.channel,
                            session.state,
                            session.origin.source.as_str(),
                            session
                                .objective
                                .as_deref()
                                .map(|objective| format!(" objective={objective}"))
                                .unwrap_or_default()
                        );
                    }
                }
                Ok(())
            }
            VoiceCallCommands::Get {
                channel,
                call_control_id,
                json,
            } => {
                let channel =
                    synapseclaw::channels::resolve_realtime_audio_call_inspection_channel(
                        channel.as_deref(),
                        &config.channels_config,
                    )?;
                let call_control_id = non_empty_cli_value("--call-control-id", call_control_id)?;
                let session =
                    synapseclaw::channels::get_realtime_audio_call_session_with_synapseclaw_dir(
                        &channel,
                        &call_control_id,
                        &config.channels_config,
                        config.config_path.parent().map(|path| path.to_path_buf()),
                    )?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "ok",
                            "channel": channel,
                            "session": session,
                        }))?
                    );
                } else if let Some(session) = session {
                    println!("Call: {}", session.call_control_id);
                    println!("State: {:?}", session.state);
                    println!("Direction: {:?}", session.direction);
                    println!("Triggered by: {}", session.origin.source.as_str());
                    if let (Some(channel), Some(recipient)) = (
                        session.origin.channel.as_deref(),
                        session.origin.recipient.as_deref(),
                    ) {
                        match session.origin.thread_ref.as_deref() {
                            Some(thread_ref) => {
                                println!(
                                    "Trigger conversation: {channel}:{recipient}#{thread_ref}"
                                );
                            }
                            None => println!("Trigger conversation: {channel}:{recipient}"),
                        }
                    }
                    if let Some(objective) = session.objective.as_deref() {
                        println!("Objective: {objective}");
                    }
                    if let Some(end_reason) = session.end_reason.as_deref() {
                        println!("End reason: {end_reason}");
                    }
                    if let Some(summary) = session.summary.as_deref() {
                        println!("Summary: {summary}");
                    }
                    if !session.decisions.is_empty() {
                        println!("Decisions:");
                        for decision in &session.decisions {
                            println!("  - {decision}");
                        }
                    }
                    println!("Messages: {}", session.message_count);
                    println!("Interruptions: {}", session.interruption_count);
                } else {
                    bail!("realtime call session not found");
                }
                Ok(())
            }
            VoiceCallCommands::Start {
                channel,
                to,
                prompt,
                objective,
                context,
                agenda,
                confirm,
            } => {
                synapseclaw::channels::require_realtime_call_confirmation(confirm)?;
                let channel = synapseclaw::channels::resolve_realtime_audio_call_channel(
                    channel.as_deref(),
                    &config.channels_config,
                )?;
                let runtime = synapseclaw::channels::configured_realtime_audio_call_runtime_with_support_configs(
                    &channel,
                    &config.channels_config,
                    config.config_path.parent().map(|path| path.to_path_buf()),
                    synapseclaw::channels::lane_selected_tts_config(&config).ok(),
                    synapseclaw::channels::lane_selected_transcription_config(&config).ok(),
                )?;
                let to = non_empty_cli_value("--to", to)?;
                let result = runtime
                    .start_audio_call(RealtimeCallStartRequest {
                        to,
                        prompt: prompt
                            .map(|value| non_empty_cli_value("--prompt", value))
                            .transpose()?,
                        origin: RealtimeCallOrigin::cli_request(),
                        objective: objective
                            .map(|value| non_empty_cli_value("--objective", value))
                            .transpose()?,
                        context: context
                            .map(|value| non_empty_cli_value("--context", value))
                            .transpose()?,
                        agenda: agenda
                            .into_iter()
                            .map(|value| non_empty_cli_value("--agenda", value))
                            .collect::<Result<Vec<_>>>()?,
                    })
                    .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "ok",
                        "channel": channel,
                        "call": result,
                    }))?
                );
                Ok(())
            }
            VoiceCallCommands::Speak {
                channel,
                call_control_id,
                text,
                confirm,
            } => {
                synapseclaw::channels::require_realtime_call_confirmation(confirm)?;
                let channel = synapseclaw::channels::resolve_realtime_audio_call_channel(
                    channel.as_deref(),
                    &config.channels_config,
                )?;
                let runtime = synapseclaw::channels::configured_realtime_audio_call_runtime_with_support_configs(
                    &channel,
                    &config.channels_config,
                    config.config_path.parent().map(|path| path.to_path_buf()),
                    synapseclaw::channels::lane_selected_tts_config(&config).ok(),
                    synapseclaw::channels::lane_selected_transcription_config(&config).ok(),
                )?;
                let result = RealtimeCallRuntimePort::speak(
                    runtime.as_ref(),
                    RealtimeCallSpeakRequest {
                        call_control_id: non_empty_cli_value("--call-control-id", call_control_id)?,
                        text: non_empty_cli_value("--text", text)?,
                    },
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "ok",
                        "channel": channel,
                        "result": result,
                    }))?
                );
                Ok(())
            }
            VoiceCallCommands::Answer {
                channel,
                call_control_id,
                confirm,
            } => {
                synapseclaw::channels::require_realtime_call_confirmation(confirm)?;
                let channel = synapseclaw::channels::resolve_realtime_audio_call_channel(
                    channel.as_deref(),
                    &config.channels_config,
                )?;
                let runtime = synapseclaw::channels::configured_realtime_audio_call_runtime_with_support_configs(
                    &channel,
                    &config.channels_config,
                    config.config_path.parent().map(|path| path.to_path_buf()),
                    synapseclaw::channels::lane_selected_tts_config(&config).ok(),
                    synapseclaw::channels::lane_selected_transcription_config(&config).ok(),
                )?;
                let result = RealtimeCallRuntimePort::answer(
                    runtime.as_ref(),
                    RealtimeCallAnswerRequest {
                        call_control_id: non_empty_cli_value("--call-control-id", call_control_id)?,
                    },
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "ok",
                        "channel": channel,
                        "result": result,
                    }))?
                );
                Ok(())
            }
            VoiceCallCommands::Hangup {
                channel,
                call_control_id,
                confirm,
            } => {
                synapseclaw::channels::require_realtime_call_confirmation(confirm)?;
                let channel = synapseclaw::channels::resolve_realtime_audio_call_channel(
                    channel.as_deref(),
                    &config.channels_config,
                )?;
                let runtime = synapseclaw::channels::configured_realtime_audio_call_runtime_with_support_configs(
                    &channel,
                    &config.channels_config,
                    config.config_path.parent().map(|path| path.to_path_buf()),
                    synapseclaw::channels::lane_selected_tts_config(&config).ok(),
                    synapseclaw::channels::lane_selected_transcription_config(&config).ok(),
                )?;
                let result = RealtimeCallRuntimePort::hangup(
                    runtime.as_ref(),
                    RealtimeCallHangupRequest {
                        call_control_id: non_empty_cli_value("--call-control-id", call_control_id)?,
                    },
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "ok",
                        "channel": channel,
                        "result": result,
                    }))?
                );
                Ok(())
            }
        },
    }
}

fn parse_voice_scope(
    raw: &str,
) -> Result<synapse_domain::application::services::voice_preference_service::VoicePreferenceScope> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "global" => Ok(synapse_domain::application::services::voice_preference_service::VoicePreferenceScope::Global),
        "channel" => Ok(synapse_domain::application::services::voice_preference_service::VoicePreferenceScope::Channel),
        "conversation" => Ok(synapse_domain::application::services::voice_preference_service::VoicePreferenceScope::Conversation),
        _ => bail!("--scope must be global, channel, or conversation"),
    }
}

fn parse_auto_tts_policy(
    raw: &str,
) -> Result<synapse_domain::application::services::voice_preference_service::AutoTtsPolicy> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "inherit" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::Inherit),
        "off" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::Off),
        "always" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::Always),
        "inbound_voice" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::InboundVoice),
        "tagged" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::Tagged),
        "channel_default" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::ChannelDefault),
        "conversation_default" => Ok(synapse_domain::application::services::voice_preference_service::AutoTtsPolicy::ConversationDefault),
        _ => bail!("--auto-tts-policy must be inherit, off, always, inbound_voice, tagged, channel_default, or conversation_default"),
    }
}

fn cli_voice_target(
    scope: synapse_domain::application::services::voice_preference_service::VoicePreferenceScope,
    channel: Option<String>,
    recipient: Option<String>,
) -> Result<synapse_domain::application::services::voice_preference_service::VoicePreferenceTarget>
{
    use synapse_domain::application::services::voice_preference_service::VoicePreferenceTarget;
    match scope {
        synapse_domain::application::services::voice_preference_service::VoicePreferenceScope::Global => {
            VoicePreferenceTarget::global().normalized().map_err(anyhow::Error::msg)
        }
        synapse_domain::application::services::voice_preference_service::VoicePreferenceScope::Channel => {
            let Some(channel) = channel else {
                bail!("--scope channel requires --channel");
            };
            VoicePreferenceTarget::channel(channel)
                .normalized()
                .map_err(anyhow::Error::msg)
        }
        synapse_domain::application::services::voice_preference_service::VoicePreferenceScope::Conversation => {
            let Some(channel) = channel else {
                bail!("--scope conversation requires --channel");
            };
            let Some(recipient) = recipient else {
                bail!("--scope conversation requires --recipient");
            };
            VoicePreferenceTarget::conversation(channel, recipient)
                .normalized()
                .map_err(anyhow::Error::msg)
        }
    }
}

#[derive(Debug, Serialize)]
struct VoiceModeTurnTranscript {
    provider: String,
    model: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct VoiceModeTurnAudio {
    path: PathBuf,
    bytes: usize,
    provider: String,
    model: String,
    voice: String,
    format: String,
    mime_type: String,
    playback_player: Option<String>,
    playback_error: Option<String>,
}

fn cli_user_profile_store_path(config: &Config) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("user_profiles.json")
}

fn cli_user_profile_store(
    config: &Config,
) -> Result<synapse_infra::user_profile_store::FileUserProfileStore> {
    synapse_infra::user_profile_store::FileUserProfileStore::new(cli_user_profile_store_path(
        config,
    ))
}

fn load_cli_voice_mode_settings(
    config: &Config,
) -> Result<synapse_domain::application::services::voice_mode_service::VoiceModeSettings> {
    use synapse_domain::application::services::voice_mode_service::{
        read_voice_mode_settings, VOICE_MODE_PROFILE_KEY,
    };
    use synapse_domain::ports::user_profile_store::UserProfileStorePort;

    let store = cli_user_profile_store(config)?;
    Ok(read_voice_mode_settings(store.load(VOICE_MODE_PROFILE_KEY)))
}

fn save_cli_voice_mode_settings(
    config: &Config,
    settings: synapse_domain::application::services::voice_mode_service::VoiceModeSettings,
) -> Result<()> {
    use synapse_domain::application::services::voice_mode_service::{
        write_voice_mode_settings, VOICE_MODE_PROFILE_KEY,
    };
    use synapse_domain::ports::user_profile_store::UserProfileStorePort;

    let store = cli_user_profile_store(config)?;
    if let Some(profile) = write_voice_mode_settings(settings) {
        store.upsert(VOICE_MODE_PROFILE_KEY, profile)?;
    } else {
        let _ = store.remove(VOICE_MODE_PROFILE_KEY)?;
    }
    Ok(())
}

fn normalize_optional_cli_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_voice_mode_playback(default: bool, playback: bool, no_playback: bool) -> Result<bool> {
    if playback && no_playback {
        bail!("--playback and --no-playback cannot be used together");
    }
    if playback {
        Ok(true)
    } else if no_playback {
        Ok(false)
    } else {
        Ok(default)
    }
}

fn default_cli_voice_mode_playback(config: &Config) -> bool {
    !synapseclaw::channels::voice_doctor_report(config)
        .playback_binaries
        .is_empty()
}

fn merge_voice_mode_preference(
    config: &Config,
    provider: Option<String>,
    model: Option<String>,
    voice: Option<String>,
    format: Option<String>,
) -> Result<synapse_domain::application::services::voice_preference_service::VoicePreference> {
    use synapse_domain::application::services::voice_preference_service::read_voice_settings;
    use synapse_domain::ports::user_profile_store::UserProfileStorePort;

    let store = cli_user_profile_store(config)?;
    let mut preference = read_voice_settings(store.load("voice:global"))
        .preference
        .unwrap_or_default();
    if let Some(provider) = normalize_optional_cli_string(provider) {
        preference.provider = Some(provider);
    }
    if let Some(model) = normalize_optional_cli_string(model) {
        preference.model = Some(model);
    }
    if let Some(voice) = normalize_optional_cli_string(voice) {
        preference.voice = Some(voice);
    }
    if let Some(format) = normalize_optional_cli_string(format) {
        preference.format = Some(format);
    }
    let preference = preference.normalized();
    validate_cli_voice_preference(config, &preference)?;
    Ok(preference)
}

async fn transcribe_cli_audio(
    config: &Config,
    file: &std::path::Path,
    provider: Option<String>,
) -> Result<VoiceModeTurnTranscript> {
    let transcription = synapseclaw::channels::lane_selected_transcription_config(config)?;
    if !transcription.enabled {
        bail!("voice transcription is not enabled");
    }
    let file_name = file
        .file_name()
        .and_then(|name| name.to_str())
        .context("--file must include a valid file name")?
        .to_string();
    let audio = tokio::fs::read(file)
        .await
        .with_context(|| format!("failed to read {}", file.display()))?;
    let manager = synapseclaw::channels::transcription::TranscriptionManager::new(&transcription)?;
    let selected_provider = provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .unwrap_or(transcription.default_provider.as_str())
        .to_string();
    let text = if provider.is_some() {
        manager
            .transcribe_with_provider(&audio, &file_name, &selected_provider)
            .await?
    } else {
        manager.transcribe(&audio, &file_name).await?
    };
    Ok(VoiceModeTurnTranscript {
        provider: selected_provider,
        model: transcription.model,
        text,
    })
}

async fn synthesize_cli_audio(
    config: &Config,
    text: &str,
    preference: &synapse_domain::application::services::voice_preference_service::VoicePreference,
    output: Option<PathBuf>,
    playback: bool,
) -> Result<VoiceModeTurnAudio> {
    use synapse_domain::application::services::media_artifact_delivery::{
        tts_output_extension, tts_output_mime, tts_provider_output_format,
    };

    let mut tts = select_cli_tts_config(config, preference)?;
    if let Some(voice) = preference.voice.clone() {
        tts.default_voice = voice;
    }
    if let Some(format) = preference.format.clone() {
        tts.default_format = format;
    }
    let manager = synapseclaw::channels::TtsManager::new(&tts)?;
    let bytes = manager.synthesize(text).await?;
    if bytes.is_empty() {
        bail!("voice synthesis returned empty audio");
    }

    let provider_format = tts_provider_output_format(&tts);
    let provider = tts.default_provider.clone();
    let model = voice_selected_model(&tts);
    let voice = tts.default_voice.clone();
    let extension = tts_output_extension(&provider_format);
    let output = match output {
        Some(path) => path,
        None => {
            let dir = config.workspace_dir.join("voice_out");
            tokio::fs::create_dir_all(&dir)
                .await
                .with_context(|| format!("failed to create {}", dir.display()))?;
            dir.join(format!("voice_turn_{}.{}", uuid::Uuid::new_v4(), extension))
        }
    };
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    tokio::fs::write(&output, &bytes)
        .await
        .with_context(|| format!("failed to write {}", output.display()))?;

    let (playback_player, playback_error) = if playback {
        match play_local_audio_file(&output) {
            Ok(player) => (Some(player), None),
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };

    Ok(VoiceModeTurnAudio {
        path: output,
        bytes: bytes.len(),
        provider,
        model,
        voice,
        format: provider_format.to_string(),
        mime_type: tts_output_mime(&provider_format).to_string(),
        playback_player,
        playback_error,
    })
}

fn play_local_audio_file(path: &std::path::Path) -> Result<String> {
    let playback_commands: [(&str, &[&str]); 5] = [
        ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "error"]),
        ("paplay", &[]),
        ("pw-play", &[]),
        ("play", &["-q"]),
        ("aplay", &[]),
    ];
    let mut last_error = None;

    for (binary, args) in playback_commands {
        let mut command = std::process::Command::new(binary);
        command.args(args).arg(path);
        match command.status() {
            Ok(status) if status.success() => return Ok(binary.to_string()),
            Ok(status) => {
                last_error = Some(format!("{binary} exited with status {status}"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                last_error = Some(format!("{binary} failed: {error}"));
            }
        }
    }

    Err(anyhow::anyhow!(
        "{}",
        last_error.unwrap_or_else(|| "no local playback binary is available".into())
    ))
}

fn validate_cli_voice_preference(
    config: &Config,
    preference: &synapse_domain::application::services::voice_preference_service::VoicePreference,
) -> Result<()> {
    let _ = select_cli_tts_config(config, preference)?;
    Ok(())
}

fn select_cli_tts_config(
    config: &Config,
    preference: &synapse_domain::application::services::voice_preference_service::VoicePreference,
) -> Result<synapse_domain::config::schema::TtsConfig> {
    use synapse_domain::application::services::voice_preference_service::candidate_matches_preference;
    let candidates = synapseclaw::channels::lane_selected_tts_candidate_configs(config)?;
    let matching = candidates
        .into_iter()
        .map(|(_, tts)| tts)
        .filter(|tts| tts.enabled)
        .filter(|tts| {
            preference
                .provider
                .as_deref()
                .is_none_or(|provider| tts.default_provider.eq_ignore_ascii_case(provider))
        })
        .filter(|tts| {
            preference
                .model
                .as_deref()
                .is_none_or(|model| voice_selected_model(tts).eq_ignore_ascii_case(model))
        })
        .filter(|tts| {
            candidate_matches_preference(tts, Some(voice_selected_model(tts).as_str()), preference)
        })
        .collect::<Vec<_>>();

    if matching.is_empty() {
        bail!("no active speech_synthesis lane candidate matches the requested preference");
    }
    if let Some(voice) = preference.voice.as_deref() {
        for tts in matching {
            let manager = synapseclaw::channels::TtsManager::new(&tts)?;
            let voices = manager.supported_voices(&tts.default_provider)?;
            if voices
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(voice))
            {
                return Ok(tts);
            }
        }
        bail!("voice `{voice}` is not supported by matching speech_synthesis candidates");
    }
    matching
        .into_iter()
        .next()
        .context("no active speech_synthesis lane candidate matches the requested preference")
}

fn ensure_speech_synthesis_lane(config: &mut Config) -> &mut config::schema::ModelLaneConfig {
    if let Some(index) = config
        .model_lanes
        .iter()
        .position(|lane| lane.lane == config::schema::CapabilityLane::SpeechSynthesis)
    {
        return &mut config.model_lanes[index];
    }
    config.model_lanes.push(config::schema::ModelLaneConfig {
        lane: config::schema::CapabilityLane::SpeechSynthesis,
        candidates: Vec::new(),
    });
    config
        .model_lanes
        .last_mut()
        .expect("speech_synthesis lane was just pushed")
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled {
        "enabled"
    } else {
        "disabled"
    }
}

fn non_empty_cli_value(flag: &str, value: String) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{flag} cannot be empty");
    }
    Ok(trimmed.to_string())
}

fn voice_selected_model(config: &config::schema::TtsConfig) -> String {
    match config.default_provider.as_str() {
        "openai" => config
            .openai
            .as_ref()
            .map(|cfg| cfg.model.clone())
            .unwrap_or_else(|| "tts-1".to_string()),
        "groq" => config
            .groq
            .as_ref()
            .map(|cfg| cfg.model.clone())
            .unwrap_or_else(|| "canopylabs/orpheus-v1-english".to_string()),
        "elevenlabs" => "elevenlabs".to_string(),
        "google" => "google-cloud-tts".to_string(),
        "edge" => "edge-tts".to_string(),
        "minimax" => config
            .minimax
            .as_ref()
            .map(|cfg| cfg.model.clone())
            .unwrap_or_else(|| "speech-02-hd".to_string()),
        "mistral" => config
            .mistral
            .as_ref()
            .map(|cfg| cfg.model.clone())
            .unwrap_or_else(|| "voxtral-mini-tts-2603".to_string()),
        "xai" => "tts".to_string(),
        _ => config.default_provider.clone(),
    }
}

// MemoryCommands imported from synapseclaw::commands (defined in src/commands.rs)

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    // Install default crypto provider for Rustls TLS.
    // This prevents the error: "could not automatically determine the process-level CryptoProvider"
    // when both aws-lc-rs and ring features are available (or neither is explicitly selected).
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Warning: Failed to install default crypto provider: {e:?}");
    }

    if std::env::args_os().len() <= 1 {
        return print_no_command_help();
    }

    let cli = Cli::parse();

    if let Some(config_dir) = &cli.config_dir {
        if config_dir.trim().is_empty() {
            bail!("--config-dir cannot be empty");
        }
        std::env::set_var("SYNAPSECLAW_CONFIG_DIR", config_dir);
    }

    // --instance flag on daemon/service resolves to ~/.synapseclaw/agents/<name>/
    // Sets SYNAPSECLAW_CONFIG_DIR before config loading (lower precedence than explicit --config-dir)
    if cli.config_dir.is_none() {
        let instance = match &cli.command {
            Commands::Daemon { instance, .. } | Commands::Service { instance, .. } => {
                instance.as_deref()
            }
            _ => None,
        };
        if let Some(name) = instance {
            // Validate instance name: ^[a-z0-9][a-z0-9_-]{0,30}$
            if name.is_empty()
                || name.len() > 31
                || !{
                    let c = name.chars().next().unwrap_or(' ');
                    c.is_ascii_lowercase() || c.is_ascii_digit()
                }
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                bail!("Invalid instance name '{name}'. Must match [a-z0-9][a-z0-9_-]{{0,30}}");
            }
            let home = directories::UserDirs::new()
                .map(|u| u.home_dir().to_path_buf())
                .context("Could not find home directory")?;
            let instance_dir = home.join(".synapseclaw").join("agents").join(name);
            std::env::set_var(
                "SYNAPSECLAW_CONFIG_DIR",
                instance_dir.to_string_lossy().as_ref(),
            );
        }
    }

    // Completions must remain stdout-only and should not load config or initialize logging.
    // This avoids warnings/log lines corrupting sourced completion scripts.
    if let Commands::Completions { shell } = &cli.command {
        let mut stdout = std::io::stdout().lock();
        write_shell_completion(*shell, &mut stdout)?;
        return Ok(());
    }

    // Initialize logging - respects RUST_LOG env var, defaults to INFO
    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    if let Some(path) =
        synapse_infra::model_catalog_io::install_runtime_model_catalog_override_if_present().await?
    {
        tracing::info!(
            path = %path.display(),
            "Loaded user model catalog override"
        );
    }

    // Onboard auto-detects the environment: if stdin/stdout are a TTY and no
    // provider flags were given, it runs the full interactive wizard; otherwise
    // it runs the quick (scriptable) setup.  This means `curl … | bash` and
    // `synapseclaw onboard --api-key …` both take the fast path, while a bare
    // `synapseclaw onboard` in a terminal launches the wizard.
    if let Commands::Onboard {
        force,
        reinit,
        channels_only,
        api_key,
        provider,
        model,
        memory,
    } = &cli.command
    {
        let force = *force;
        let reinit = *reinit;
        let channels_only = *channels_only;
        let api_key = api_key.clone();
        let provider = provider.clone();
        let model = model.clone();
        let memory = memory.clone();

        if reinit && channels_only {
            bail!("--reinit and --channels-only cannot be used together");
        }
        if channels_only
            && (api_key.is_some() || provider.is_some() || model.is_some() || memory.is_some())
        {
            bail!("--channels-only does not accept --api-key, --provider, --model, or --memory");
        }
        if channels_only && force {
            bail!("--channels-only does not accept --force");
        }

        // Handle --reinit: backup and reset configuration
        if reinit {
            let (synapseclaw_dir, _) =
                synapse_infra::workspace_io::resolve_runtime_dirs_for_onboarding().await?;

            if synapseclaw_dir.exists() {
                let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S");
                let backup_dir = format!("{}.backup.{}", synapseclaw_dir.display(), timestamp);

                println!("⚠️  Reinitializing SynapseClaw configuration...");
                println!("   Current config directory: {}", synapseclaw_dir.display());
                println!(
                    "   This will back up your existing config to: {}",
                    backup_dir
                );
                println!();
                print!("Continue? [y/N] ");
                std::io::stdout()
                    .flush()
                    .context("Failed to flush stdout")?;

                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !answer.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
                println!();

                // Rename existing directory as backup
                tokio::fs::rename(&synapseclaw_dir, &backup_dir)
                    .await
                    .with_context(|| {
                        format!("Failed to backup existing config to {}", backup_dir)
                    })?;

                println!("   Backup created successfully.");
                println!("   Starting fresh initialization...\n");
            }
        }

        // Auto-detect: run the interactive wizard when in a TTY with no
        // provider flags, quick setup otherwise (scriptable path).
        let has_provider_flags =
            api_key.is_some() || provider.is_some() || model.is_some() || memory.is_some();
        let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

        let config = if channels_only {
            Box::pin(synapse_onboard::run_channels_repair_wizard()).await
        } else if is_tty && !has_provider_flags {
            Box::pin(synapse_onboard::run_wizard(force)).await
        } else {
            Box::pin(synapse_onboard::run_quick_setup(
                api_key.as_deref(),
                provider.as_deref(),
                model.as_deref(),
                memory.as_deref(),
                force,
            ))
            .await
        }?;

        if config.gateway.require_pairing {
            println!();
            println!("  Pairing is enabled. A one-time pairing code will be");
            println!("  displayed when the gateway starts.");
            println!("  Dashboard: http://127.0.0.1:{}", config.gateway.port);
            println!();
        }

        // Auto-start channels if user said yes during wizard
        if std::env::var("SYNAPSECLAW_AUTOSTART_CHANNELS").as_deref() == Ok("1") {
            Box::pin(adapters::channels::start_channels(
                config, None, None, None, None, None, None,
            ))
            .await?;
        }
        return Ok(());
    }

    // All other commands need config loaded first
    let mut config = Box::pin(Config::load_or_init()).await?;
    config.apply_env_overrides();

    // Build agent runner port — shared by gateway, daemon, cron
    let config_for_runner = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));
    let agent_runner: std::sync::Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort> =
        std::sync::Arc::new(crate::agent::runner_adapter::AgentRunner::new(
            config_for_runner,
        ));
    synapse_observability::runtime_trace::init_from_config(
        &config.observability,
        &config.workspace_dir,
    );
    if config.security.otp.enabled {
        let config_dir = config
            .config_path
            .parent()
            .context("Config path must have a parent directory")?;
        let store = security::SecretStore::new(config_dir, config.secrets.encrypt);
        let (_validator, enrollment_uri) =
            security::OtpValidator::from_config(&config.security.otp, config_dir, &store)?;
        if let Some(uri) = enrollment_uri {
            println!("Initialized OTP secret for SynapseClaw.");
            println!("Enrollment URI: {uri}");
        }
    }

    match cli.command {
        Commands::Onboard { .. } | Commands::Completions { .. } => unreachable!(),

        Commands::Agent {
            message,
            session_state_file,
            provider,
            model,
            temperature,
        } => {
            let final_temperature = temperature.unwrap_or(config.default_temperature);

            Box::pin(agent::run(
                config,
                message,
                provider,
                model,
                final_temperature,
                true,
                session_state_file,
                None,
                None,
            ))
            .await
            .map(|_| ())
        }

        Commands::Gateway { gateway_command } => {
            match gateway_command {
                Some(synapseclaw::GatewayCommands::Restart { port, host }) => {
                    let (port, host) = resolve_gateway_addr(&config, port, host);
                    let addr = format!("{host}:{port}");
                    info!("🔄 Restarting SynapseClaw Gateway on {addr}");

                    // Try to gracefully shutdown existing gateway via admin endpoint
                    match shutdown_gateway(&host, port).await {
                        Ok(()) => {
                            info!("   ✓ Existing gateway on {addr} shut down gracefully");
                            // Poll until the port is free (connection refused) or timeout
                            let deadline =
                                tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
                            loop {
                                match tokio::net::TcpStream::connect(&addr).await {
                                    Err(_) => break, // port is free
                                    Ok(_) if tokio::time::Instant::now() >= deadline => {
                                        warn!(
                                            "   Timed out waiting for port {port} to be released"
                                        );
                                        break;
                                    }
                                    Ok(_) => {
                                        tokio::time::sleep(tokio::time::Duration::from_millis(50))
                                            .await;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            info!("   No existing gateway to shut down: {e}");
                        }
                    }

                    log_gateway_start(&host, port);
                    Box::pin(adapters::gateway::run_gateway(
                        &host,
                        port,
                        config,
                        None,
                        None,
                        None,
                        agent_runner.clone(),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    ))
                    .await
                }
                Some(synapseclaw::GatewayCommands::GetPaircode { new }) => {
                    let port = config.gateway.port;
                    let host = &config.gateway.host;

                    // Fetch live pairing code from running gateway
                    // If --new is specified, generate a fresh pairing code
                    match fetch_paircode(host, port, new).await {
                        Ok(Some(code)) => {
                            println!("🔐 Gateway pairing is enabled.");
                            println!();
                            println!("  ┌──────────────┐");
                            println!("  │  {code}  │");
                            println!("  └──────────────┘");
                            println!();
                            println!("  Use this one-time code to pair a new device:");
                            println!("    POST /pair with header X-Pairing-Code: {code}");
                        }
                        Ok(None) => {
                            if config.gateway.require_pairing {
                                println!("🔐 Gateway pairing is enabled, but no active pairing code available.");
                                println!("   The gateway may already be paired, or the code has been used.");
                                println!("   Restart the gateway to generate a new pairing code.");
                            } else {
                                println!("⚠️  Gateway pairing is disabled in config.");
                                println!(
                                    "   All requests will be accepted without authentication."
                                );
                                println!(
                                    "   To enable pairing, set [gateway] require_pairing = true"
                                );
                            }
                        }
                        Err(e) => {
                            println!(
                                "❌ Failed to fetch pairing code from gateway at {host}:{port}"
                            );
                            println!("   Error: {e}");
                            println!();
                            println!("   Is the gateway running? Start it with:");
                            println!("     synapseclaw gateway start");
                        }
                    }
                    Ok(())
                }
                Some(synapseclaw::GatewayCommands::Start { port, host }) => {
                    let (port, host) = resolve_gateway_addr(&config, port, host);
                    log_gateway_start(&host, port);
                    Box::pin(adapters::gateway::run_gateway(
                        &host,
                        port,
                        config,
                        None,
                        None,
                        None,
                        agent_runner.clone(),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    ))
                    .await
                }
                None => {
                    let port = config.gateway.port;
                    let host = config.gateway.host.clone();
                    log_gateway_start(&host, port);
                    Box::pin(adapters::gateway::run_gateway(
                        &host,
                        port,
                        config,
                        None,
                        None,
                        None,
                        agent_runner.clone(),
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    ))
                    .await
                }
            }
        }

        Commands::Daemon {
            port,
            host,
            instance: _,
        } => {
            // Auto-generate proxy_token for broker→agent auth if IPC enabled and not yet set
            if config.agents_ipc.enabled && config.agents_ipc.proxy_token.is_none() {
                let token = format!("zc_proxy_{}", uuid::Uuid::new_v4().simple());
                config.agents_ipc.proxy_token = Some(token.clone());
                // Add to paired_tokens BEFORE save so it's persisted to disk
                if !config.gateway.paired_tokens.iter().any(|t| t == &token) {
                    config.gateway.paired_tokens.push(token);
                }
                match config.save().await {
                    Ok(()) => {
                        tracing::info!(
                            "Generated proxy_token for broker→agent auth (saved to config)"
                        );
                    }
                    Err(e) => {
                        // Revert both in-memory changes
                        config.agents_ipc.proxy_token = None;
                        config
                            .gateway
                            .paired_tokens
                            .retain(|t| !t.starts_with("zc_proxy_"));
                        tracing::warn!("Failed to save auto-generated proxy_token: {e}");
                    }
                }
            }
            // Reconcile on every start: ensure proxy_token is in paired_tokens
            // (covers cold restart where config was saved but paired_tokens wasn't updated)
            if let Some(ref pt) = config.agents_ipc.proxy_token {
                if !config.gateway.paired_tokens.iter().any(|t| t == pt) {
                    config.gateway.paired_tokens.push(pt.clone());
                    // Persist the reconciliation
                    if let Err(e) = config.save().await {
                        tracing::warn!("Failed to persist proxy_token reconciliation: {e}");
                    }
                }
            }

            // Auto-detect gateway_url if not set
            if config.agents_ipc.enabled && config.agents_ipc.gateway_url.is_none() {
                let gw_host = &config.gateway.host;
                let gw_port = config.gateway.port;
                config.agents_ipc.gateway_url = Some(format!("http://{gw_host}:{gw_port}"));
            }

            let port = port.unwrap_or(config.gateway.port);
            let host = host.unwrap_or_else(|| config.gateway.host.clone());
            if port == 0 {
                info!("🧠 Starting SynapseClaw Daemon on {host} (random port)");
            } else {
                info!("🧠 Starting SynapseClaw Daemon on {host}:{port}");
            }
            Box::pin(crate::adapters::daemon::run(
                config,
                host,
                port,
                agent_runner.clone(),
            ))
            .await
        }

        Commands::Status => {
            println!("🦀 SynapseClaw Status");
            println!();
            println!("Version:     {}", env!("CARGO_PKG_VERSION"));
            println!("Workspace:   {}", config.workspace_dir.display());
            println!("Config:      {}", config.config_path.display());
            println!();
            println!(
                "🤖 Provider:      {}",
                config.default_provider.as_deref().unwrap_or("openrouter")
            );
            println!(
                "   Model:         {}",
                config.default_model.as_deref().unwrap_or("(default)")
            );
            println!("📊 Observability:  {}", config.observability.backend);
            println!(
                "🧾 Trace storage:  {} ({})",
                config.observability.runtime_trace_mode, config.observability.runtime_trace_path
            );
            println!("🛡️  Autonomy:      {:?}", config.autonomy.level);
            println!("⚙️  Runtime:       {}", config.runtime.kind);
            println!(
                "💓 Heartbeat:      {}",
                if config.heartbeat.enabled {
                    format!("every {}min", config.heartbeat.interval_minutes)
                } else {
                    "disabled".into()
                }
            );
            println!(
                "🧠 Memory:         {} (auto-save: {})",
                &config.memory.backend,
                if config.memory.auto_save { "on" } else { "off" }
            );

            println!();
            println!("Security:");
            println!("  Workspace only:    {}", config.autonomy.workspace_only);
            println!(
                "  Allowed roots:     {}",
                if config.autonomy.allowed_roots.is_empty() {
                    "(none)".to_string()
                } else {
                    config.autonomy.allowed_roots.join(", ")
                }
            );
            println!(
                "  Allowed commands:  {}",
                config.autonomy.allowed_commands.join(", ")
            );
            println!(
                "  Max actions/hour:  {}",
                config.autonomy.max_actions_per_hour
            );
            println!(
                "  Max cost/day:      ${:.2}",
                f64::from(config.autonomy.max_cost_per_day_cents) / 100.0
            );
            println!("  OTP enabled:       {}", config.security.otp.enabled);
            println!("  E-stop enabled:    {}", config.security.estop.enabled);
            println!();
            println!("Channels:");
            println!("  CLI:      ✅ always");
            for (channel, configured) in config.channels_config.channels() {
                println!(
                    "  {:9} {}",
                    channel.name(),
                    if configured {
                        "✅ configured"
                    } else {
                        "❌ not configured"
                    }
                );
            }

            Ok(())
        }

        Commands::Estop {
            estop_command,
            level,
            domains,
            tools,
        } => handle_estop_command(&config, estop_command, level, domains, tools),

        Commands::Cron { cron_command } => {
            let resolved_agent_id = synapse_adapters::agent::resolve_agent_id(&config);
            let mem_backend =
                synapse_memory::create_memory(&config, &config.workspace_dir, &resolved_agent_id)
                    .await?;
            let db = mem_backend
                .surreal
                .ok_or_else(|| anyhow::anyhow!("SurrealDB not available for cron commands"))?;
            synapse_cron::commands::handle_command(cron_command, &db, &config).await
        }

        Commands::Models { model_command } => match model_command {
            ModelCommands::Refresh {
                provider,
                all,
                force,
            } => {
                if all {
                    if provider.is_some() {
                        bail!("`models refresh --all` cannot be combined with --provider");
                    }
                    synapse_onboard::run_models_refresh_all(&config, force).await
                } else {
                    synapse_onboard::run_models_refresh(&config, provider.as_deref(), force).await
                }
            }
            ModelCommands::List { provider } => {
                synapse_onboard::run_models_list(&config, provider.as_deref()).await
            }
            ModelCommands::Set { model } => {
                Box::pin(synapse_onboard::run_models_set(&config, &model)).await
            }
            ModelCommands::Status => synapse_onboard::run_models_status(&config).await,
            ModelCommands::Catalog { catalog_command } => match catalog_command {
                ModelCatalogCommands::Init { force } => {
                    synapse_onboard::run_models_catalog_init(force).await
                }
                ModelCatalogCommands::Status => synapse_onboard::run_models_catalog_status().await,
                ModelCatalogCommands::Path => synapse_onboard::run_models_catalog_path().await,
            },
        },

        Commands::Voice { voice_command } => handle_voice_command(&mut config, voice_command).await,

        Commands::Providers => {
            let providers = synapse_providers::list_providers();
            let current = config
                .default_provider
                .as_deref()
                .unwrap_or("openrouter")
                .trim()
                .to_ascii_lowercase();
            println!("Supported providers ({} total):\n", providers.len());
            println!("  ID (use in config)  DESCRIPTION");
            println!("  ─────────────────── ───────────");
            for p in &providers {
                let is_active = p.name.eq_ignore_ascii_case(&current)
                    || p.aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(&current));
                let marker = if is_active { " (active)" } else { "" };
                let local_tag = if p.local { " [local]" } else { "" };
                let aliases = if p.aliases.is_empty() {
                    String::new()
                } else {
                    format!("  (aliases: {})", p.aliases.join(", "))
                };
                println!(
                    "  {:<19} {}{}{}{}",
                    p.name, p.display_name, local_tag, marker, aliases
                );
            }
            println!("\n  custom:<URL>   Any OpenAI-compatible endpoint");
            println!("  anthropic-custom:<URL>  Any Anthropic-compatible endpoint");
            Ok(())
        }

        Commands::Service {
            service_command,
            service_init,
            instance,
        } => {
            let init_system = service_init.parse()?;
            crate::adapters::service::handle_command(
                &service_command,
                &config,
                init_system,
                instance.as_deref(),
            )
        }

        Commands::Doctor { doctor_command } => match doctor_command {
            Some(DoctorCommands::Models {
                provider,
                use_cache,
            }) => {
                crate::adapters::doctor::run_models(&config, provider.as_deref(), use_cache).await
            }
            Some(DoctorCommands::Traces {
                id,
                event,
                contains,
                limit,
            }) => crate::adapters::doctor::run_traces(
                &config,
                id.as_deref(),
                event.as_deref(),
                contains.as_deref(),
                limit,
            ),
            None => crate::adapters::doctor::run(&config),
        },

        Commands::Channel { channel_command } => match channel_command {
            ChannelCommands::Start => {
                Box::pin(adapters::channels::start_channels(
                    config, None, None, None, None, None, None,
                ))
                .await
            }
            ChannelCommands::Doctor => Box::pin(adapters::channels::doctor_channels(config)).await,
            other => Box::pin(adapters::channels::handle_command(other, &config)).await,
        },

        Commands::Integrations {
            integration_command,
        } => crate::adapters::integrations::handle_command(integration_command, &config),

        Commands::Skills { skill_command } => {
            synapse_adapters::skills::handle_command(skill_command, &config).await
        }

        Commands::Memory { memory_command } => {
            synapse_adapters::memory_adapters::cli::handle_command(memory_command, &config).await
        }

        Commands::Pipeline { pipeline_command } => {
            handle_pipeline_command(pipeline_command, &config).await
        }

        Commands::Auth { auth_command } => handle_auth_command(auth_command, &config).await,

        Commands::Config { config_command } => match config_command {
            ConfigCommands::Schema => {
                let schema = schemars::schema_for!(config::Config);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&schema).expect("failed to serialize JSON Schema")
                );
                Ok(())
            }
        },

        Commands::Audit { audit_command } => match audit_command {
            AuditCommands::Verify => {
                let log_path = config.workspace_dir.join(&config.security.audit.log_path);
                let key_path = config
                    .config_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("audit.key");

                match security::audit::verify_audit_chain(&log_path, &key_path) {
                    Ok(count) => {
                        println!("Audit chain verified: {count} entries, no breaks detected.");
                        Ok(())
                    }
                    Err(e) => {
                        eprintln!("Audit chain verification FAILED: {e}");
                        std::process::exit(1);
                    }
                }
            }
        },
    }
}

fn handle_estop_command(
    config: &Config,
    estop_command: Option<EstopSubcommands>,
    level: Option<EstopLevelArg>,
    domains: Vec<String>,
    tools: Vec<String>,
) -> Result<()> {
    if !config.security.estop.enabled {
        bail!("Emergency stop is disabled. Enable [security.estop].enabled = true in config.toml");
    }

    let config_dir = config
        .config_path
        .parent()
        .context("Config path must have a parent directory")?;
    let mut manager = security::EstopManager::load(&config.security.estop, config_dir)?;

    match estop_command {
        Some(EstopSubcommands::Status) => {
            print_estop_status(&manager.status());
            Ok(())
        }
        Some(EstopSubcommands::Resume {
            network,
            domains,
            tools,
            otp,
        }) => {
            let selector = build_resume_selector(network, domains, tools)?;
            let mut otp_code = otp;
            let otp_validator = if config.security.estop.require_otp_to_resume {
                if !config.security.otp.enabled {
                    bail!(
                        "security.estop.require_otp_to_resume=true but security.otp.enabled=false"
                    );
                }
                if otp_code.is_none() {
                    let entered = Password::new()
                        .with_prompt("Enter OTP code")
                        .allow_empty_password(false)
                        .interact()?;
                    otp_code = Some(entered);
                }

                let store = security::SecretStore::new(config_dir, config.secrets.encrypt);
                let (validator, enrollment_uri) =
                    security::OtpValidator::from_config(&config.security.otp, config_dir, &store)?;
                if let Some(uri) = enrollment_uri {
                    println!("Initialized OTP secret for SynapseClaw.");
                    println!("Enrollment URI: {uri}");
                }
                Some(validator)
            } else {
                None
            };

            manager.resume(selector, otp_code.as_deref(), otp_validator.as_ref())?;
            println!("Estop resume completed.");
            print_estop_status(&manager.status());
            Ok(())
        }
        None => {
            let engage_level = build_engage_level(level, domains, tools)?;
            manager.engage(engage_level)?;
            println!("Estop engaged.");
            print_estop_status(&manager.status());
            Ok(())
        }
    }
}

fn build_engage_level(
    level: Option<EstopLevelArg>,
    domains: Vec<String>,
    tools: Vec<String>,
) -> Result<security::EstopLevel> {
    let requested = level.unwrap_or(EstopLevelArg::KillAll);
    match requested {
        EstopLevelArg::KillAll => {
            if !domains.is_empty() || !tools.is_empty() {
                bail!("--domain/--tool are only valid with --level domain-block/tool-freeze");
            }
            Ok(security::EstopLevel::KillAll)
        }
        EstopLevelArg::NetworkKill => {
            if !domains.is_empty() || !tools.is_empty() {
                bail!("--domain/--tool are not valid with --level network-kill");
            }
            Ok(security::EstopLevel::NetworkKill)
        }
        EstopLevelArg::DomainBlock => {
            if domains.is_empty() {
                bail!("--level domain-block requires at least one --domain");
            }
            if !tools.is_empty() {
                bail!("--tool is not valid with --level domain-block");
            }
            Ok(security::EstopLevel::DomainBlock(domains))
        }
        EstopLevelArg::ToolFreeze => {
            if tools.is_empty() {
                bail!("--level tool-freeze requires at least one --tool");
            }
            if !domains.is_empty() {
                bail!("--domain is not valid with --level tool-freeze");
            }
            Ok(security::EstopLevel::ToolFreeze(tools))
        }
    }
}

fn build_resume_selector(
    network: bool,
    domains: Vec<String>,
    tools: Vec<String>,
) -> Result<security::ResumeSelector> {
    let selected =
        usize::from(network) + usize::from(!domains.is_empty()) + usize::from(!tools.is_empty());
    if selected > 1 {
        bail!("Use only one of --network, --domain, or --tool for estop resume");
    }
    if network {
        return Ok(security::ResumeSelector::Network);
    }
    if !domains.is_empty() {
        return Ok(security::ResumeSelector::Domains(domains));
    }
    if !tools.is_empty() {
        return Ok(security::ResumeSelector::Tools(tools));
    }
    Ok(security::ResumeSelector::KillAll)
}

fn print_estop_status(state: &security::EstopState) {
    println!("Estop status:");
    println!(
        "  engaged:        {}",
        if state.is_engaged() { "yes" } else { "no" }
    );
    println!(
        "  kill_all:       {}",
        if state.kill_all { "active" } else { "inactive" }
    );
    println!(
        "  network_kill:   {}",
        if state.network_kill {
            "active"
        } else {
            "inactive"
        }
    );
    if state.blocked_domains.is_empty() {
        println!("  domain_blocks:  (none)");
    } else {
        println!("  domain_blocks:  {}", state.blocked_domains.join(", "));
    }
    if state.frozen_tools.is_empty() {
        println!("  tool_freeze:    (none)");
    } else {
        println!("  tool_freeze:    {}", state.frozen_tools.join(", "));
    }
    if let Some(updated_at) = &state.updated_at {
        println!("  updated_at:     {updated_at}");
    }
}

fn write_shell_completion<W: Write>(shell: CompletionShell, writer: &mut W) -> Result<()> {
    use clap_complete::generate;
    use clap_complete::shells;

    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();

    match shell {
        CompletionShell::Bash => generate(shells::Bash, &mut cmd, bin_name.clone(), writer),
        CompletionShell::Fish => generate(shells::Fish, &mut cmd, bin_name.clone(), writer),
        CompletionShell::Zsh => generate(shells::Zsh, &mut cmd, bin_name.clone(), writer),
        CompletionShell::PowerShell => {
            generate(shells::PowerShell, &mut cmd, bin_name.clone(), writer);
        }
        CompletionShell::Elvish => generate(shells::Elvish, &mut cmd, bin_name, writer),
    }

    writer.flush()?;
    Ok(())
}

// ─── Gateway helper functions ───────────────────────────────────────────────

/// Resolve gateway host and port from CLI args or config.
fn resolve_gateway_addr(config: &Config, port: Option<u16>, host: Option<String>) -> (u16, String) {
    let port = port.unwrap_or(config.gateway.port);
    let host = host.unwrap_or_else(|| config.gateway.host.clone());
    (port, host)
}

/// Log gateway startup message.
fn log_gateway_start(host: &str, port: u16) {
    if port == 0 {
        info!("🚀 Starting SynapseClaw Gateway on {host} (random port)");
    } else {
        info!("🚀 Starting SynapseClaw Gateway on {host}:{port}");
    }
}

/// Gracefully shutdown a running gateway via the admin endpoint.
async fn shutdown_gateway(host: &str, port: u16) -> Result<()> {
    let url = format!("http://{host}:{port}/admin/shutdown");
    let client = reqwest::Client::new();

    match client
        .post(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => Err(anyhow::anyhow!(
            "Gateway responded with status: {}",
            response.status()
        )),
        Err(e) => Err(anyhow::anyhow!("Failed to connect to gateway: {e}")),
    }
}

/// Fetch the current pairing code from a running gateway.
/// If `new` is true, generates a fresh pairing code via POST request.
async fn fetch_paircode(host: &str, port: u16, new: bool) -> Result<Option<String>> {
    let client = reqwest::Client::new();

    let response = if new {
        // Generate a new pairing code via POST
        let url = format!("http://{host}:{port}/admin/paircode/new");
        client
            .post(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
    } else {
        // Get existing pairing code via GET
        let url = format!("http://{host}:{port}/admin/paircode");
        client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
    };

    let response = response.map_err(|e| anyhow::anyhow!("Failed to connect to gateway: {e}"))?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Gateway responded with status: {}",
            response.status()
        ));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {e}"))?;

    if json.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(None);
    }

    Ok(json
        .get("pairing_code")
        .and_then(|v| v.as_str())
        .map(String::from))
}

// ─── Generic Pending OAuth Login ────────────────────────────────────────────

/// Generic pending OAuth login state, shared across providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingOAuthLogin {
    provider: String,
    profile: String,
    code_verifier: String,
    state: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingOAuthLoginFile {
    #[serde(default)]
    provider: Option<String>,
    profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted_code_verifier: Option<String>,
    state: String,
    created_at: String,
}

fn pending_oauth_login_path(config: &Config, provider: &str) -> std::path::PathBuf {
    let filename = format!("auth-{}-pending.json", provider);
    synapse_providers::auth::state_dir_from_config(config).join(filename)
}

fn pending_oauth_secret_store(config: &Config) -> security::secrets::SecretStore {
    security::secrets::SecretStore::new(
        &synapse_providers::auth::state_dir_from_config(config),
        config.secrets.encrypt,
    )
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

fn save_pending_oauth_login(config: &Config, pending: &PendingOAuthLogin) -> Result<()> {
    let path = pending_oauth_login_path(config, &pending.provider);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let secret_store = pending_oauth_secret_store(config);
    let encrypted_code_verifier = secret_store.encrypt(&pending.code_verifier)?;
    let persisted = PendingOAuthLoginFile {
        provider: Some(pending.provider.clone()),
        profile: pending.profile.clone(),
        code_verifier: None,
        encrypted_code_verifier: Some(encrypted_code_verifier),
        state: pending.state.clone(),
        created_at: pending.created_at.clone(),
    };
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let json = serde_json::to_vec_pretty(&persisted)?;
    std::fs::write(&tmp, json)?;
    set_owner_only_permissions(&tmp)?;
    std::fs::rename(tmp, &path)?;
    set_owner_only_permissions(&path)?;
    Ok(())
}

fn load_pending_oauth_login(config: &Config, provider: &str) -> Result<Option<PendingOAuthLogin>> {
    let path = pending_oauth_login_path(config, provider);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let persisted: PendingOAuthLoginFile = serde_json::from_slice(&bytes)?;
    let secret_store = pending_oauth_secret_store(config);
    let code_verifier = if let Some(encrypted) = persisted.encrypted_code_verifier {
        secret_store.decrypt(&encrypted)?
    } else if let Some(plaintext) = persisted.code_verifier {
        plaintext
    } else {
        bail!("Pending {} login is missing code verifier", provider);
    };
    Ok(Some(PendingOAuthLogin {
        provider: persisted.provider.unwrap_or_else(|| provider.to_string()),
        profile: persisted.profile,
        code_verifier,
        state: persisted.state,
        created_at: persisted.created_at,
    }))
}

fn clear_pending_oauth_login(config: &Config, provider: &str) {
    let path = pending_oauth_login_path(config, provider);
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) {
        let _ = file.set_len(0);
        let _ = file.sync_all();
    }
    let _ = std::fs::remove_file(path);
}

fn read_auth_input(prompt: &str) -> Result<String> {
    let input = Password::new()
        .with_prompt(prompt)
        .allow_empty_password(false)
        .interact()?;
    Ok(input.trim().to_string())
}

fn read_plain_input(prompt: &str) -> Result<String> {
    let input: String = Input::new().with_prompt(prompt).interact_text()?;
    Ok(input.trim().to_string())
}

fn extract_openai_account_id_for_profile(access_token: &str) -> Option<String> {
    let account_id =
        synapse_providers::auth::openai_oauth::extract_account_id_from_jwt(access_token);
    if account_id.is_none() {
        warn!(
            "Could not extract OpenAI account id from OAuth access token; \
             requests may fail until re-authentication."
        );
    }
    account_id
}

fn format_expiry(profile: &synapse_providers::auth::profiles::AuthProfile) -> String {
    match profile
        .token_set
        .as_ref()
        .and_then(|token_set| token_set.expires_at)
    {
        Some(ts) => {
            let now = chrono::Utc::now();
            if ts <= now {
                format!("expired at {}", ts.to_rfc3339())
            } else {
                let mins = (ts - now).num_minutes();
                format!("expires in {mins}m ({})", ts.to_rfc3339())
            }
        }
        None => "n/a".to_string(),
    }
}

#[allow(clippy::too_many_lines)]
async fn handle_auth_command(auth_command: AuthCommands, config: &Config) -> Result<()> {
    let auth_service = synapse_providers::auth::AuthService::from_config(config);

    match auth_command {
        AuthCommands::Login {
            provider,
            profile,
            device_code,
        } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;
            let client = reqwest::Client::new();

            match provider.as_str() {
                "gemini" => {
                    // Gemini OAuth flow
                    if device_code {
                        match synapse_providers::auth::gemini_oauth::start_device_code_flow(&client)
                            .await
                        {
                            Ok(device) => {
                                println!("Google/Gemini device-code login started.");
                                println!("Visit: {}", device.verification_uri);
                                println!("Code:  {}", device.user_code);
                                if let Some(uri_complete) = &device.verification_uri_complete {
                                    println!("Fast link: {uri_complete}");
                                }

                                let token_set =
                                    synapse_providers::auth::gemini_oauth::poll_device_code_tokens(
                                        &client, &device,
                                    )
                                    .await?;
                                let account_id = token_set.id_token.as_deref().and_then(
                                    synapse_providers::auth::gemini_oauth::extract_account_email_from_id_token,
                                );

                                auth_service
                                    .store_gemini_tokens(&profile, token_set, account_id, true)
                                    .await?;

                                println!("Saved profile {profile}");
                                println!("Active profile for gemini: {profile}");
                                return Ok(());
                            }
                            Err(e) => {
                                println!(
                                    "Device-code flow unavailable: {e}. Falling back to browser flow."
                                );
                            }
                        }
                    }

                    let pkce = synapse_providers::auth::gemini_oauth::generate_pkce_state();
                    let authorize_url =
                        synapse_providers::auth::gemini_oauth::build_authorize_url(&pkce)?;

                    // Save pending login for paste-redirect fallback
                    let pending = PendingOAuthLogin {
                        provider: "gemini".to_string(),
                        profile: profile.clone(),
                        code_verifier: pkce.code_verifier.clone(),
                        state: pkce.state.clone(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    save_pending_oauth_login(config, &pending)?;

                    println!("Open this URL in your browser and authorize access:");
                    println!("{authorize_url}");
                    println!();

                    let code = match synapse_providers::auth::gemini_oauth::receive_loopback_code(
                        &pkce.state,
                        std::time::Duration::from_secs(180),
                    )
                    .await
                    {
                        Ok(code) => {
                            clear_pending_oauth_login(config, "gemini");
                            code
                        }
                        Err(e) => {
                            println!("Callback capture failed: {e}");
                            println!(
                                "Run `synapseclaw auth paste-redirect --provider gemini --profile {profile}`"
                            );
                            return Ok(());
                        }
                    };

                    let token_set =
                        synapse_providers::auth::gemini_oauth::exchange_code_for_tokens(
                            &client, &code, &pkce,
                        )
                        .await?;
                    let account_id = token_set.id_token.as_deref().and_then(
                        synapse_providers::auth::gemini_oauth::extract_account_email_from_id_token,
                    );

                    auth_service
                        .store_gemini_tokens(&profile, token_set, account_id, true)
                        .await?;

                    println!("Saved profile {profile}");
                    println!("Active profile for gemini: {profile}");
                    Ok(())
                }
                "openai-codex" => {
                    // OpenAI Codex OAuth flow
                    if device_code {
                        match synapse_providers::auth::openai_oauth::start_device_code_flow(&client)
                            .await
                        {
                            Ok(device) => {
                                println!("OpenAI device-code login started.");
                                println!("Visit: {}", device.verification_uri);
                                println!("Code:  {}", device.user_code);
                                if let Some(uri_complete) = &device.verification_uri_complete {
                                    println!("Fast link: {uri_complete}");
                                }
                                if let Some(message) = &device.message {
                                    println!("{message}");
                                }

                                let token_set =
                                    synapse_providers::auth::openai_oauth::poll_device_code_tokens(
                                        &client, &device,
                                    )
                                    .await?;
                                let account_id =
                                    extract_openai_account_id_for_profile(&token_set.access_token);

                                auth_service
                                    .store_openai_tokens(&profile, token_set, account_id, true)
                                    .await?;
                                clear_pending_oauth_login(config, "openai");

                                println!("Saved profile {profile}");
                                println!("Active profile for openai-codex: {profile}");
                                return Ok(());
                            }
                            Err(e) => {
                                println!(
                                    "Device-code flow unavailable: {e}. Falling back to browser/paste flow."
                                );
                            }
                        }
                    }

                    let pkce = synapse_providers::auth::openai_oauth::generate_pkce_state();
                    let pending = PendingOAuthLogin {
                        provider: "openai".to_string(),
                        profile: profile.clone(),
                        code_verifier: pkce.code_verifier.clone(),
                        state: pkce.state.clone(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    save_pending_oauth_login(config, &pending)?;

                    let authorize_url =
                        synapse_providers::auth::openai_oauth::build_authorize_url(&pkce);
                    println!("Open this URL in your browser and authorize access:");
                    println!("{authorize_url}");
                    println!();
                    println!("Waiting for callback at http://localhost:1455/auth/callback ...");

                    let code = match synapse_providers::auth::openai_oauth::receive_loopback_code(
                        &pkce.state,
                        std::time::Duration::from_secs(180),
                    )
                    .await
                    {
                        Ok(code) => code,
                        Err(e) => {
                            println!("Callback capture failed: {e}");
                            println!(
                                "Run `synapseclaw auth paste-redirect --provider openai-codex --profile {profile}`"
                            );
                            return Ok(());
                        }
                    };

                    let token_set =
                        synapse_providers::auth::openai_oauth::exchange_code_for_tokens(
                            &client, &code, &pkce,
                        )
                        .await?;
                    let account_id = extract_openai_account_id_for_profile(&token_set.access_token);

                    auth_service
                        .store_openai_tokens(&profile, token_set, account_id, true)
                        .await?;
                    clear_pending_oauth_login(config, "openai");

                    println!("Saved profile {profile}");
                    println!("Active profile for openai-codex: {profile}");
                    Ok(())
                }
                _ => {
                    bail!(
                        "`auth login` supports --provider openai-codex or gemini, got: {provider}"
                    );
                }
            }
        }

        AuthCommands::PasteRedirect {
            provider,
            profile,
            input,
        } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;

            match provider.as_str() {
                "openai-codex" => {
                    let pending = load_pending_oauth_login(config, "openai")?.ok_or_else(|| {
                        anyhow::anyhow!(
                            "No pending OpenAI login found. Run `synapseclaw auth login --provider openai-codex` first."
                        )
                    })?;

                    if pending.profile != profile {
                        bail!(
                            "Pending login profile mismatch: pending={}, requested={}",
                            pending.profile,
                            profile
                        );
                    }

                    let redirect_input = match input {
                        Some(value) => value,
                        None => read_plain_input("Paste redirect URL or OAuth code")?,
                    };

                    let code = synapse_providers::auth::openai_oauth::parse_code_from_redirect(
                        &redirect_input,
                        Some(&pending.state),
                    )?;

                    let pkce = synapse_providers::auth::openai_oauth::PkceState {
                        code_verifier: pending.code_verifier.clone(),
                        code_challenge: String::new(),
                        state: pending.state.clone(),
                    };

                    let client = reqwest::Client::new();
                    let token_set =
                        synapse_providers::auth::openai_oauth::exchange_code_for_tokens(
                            &client, &code, &pkce,
                        )
                        .await?;
                    let account_id = extract_openai_account_id_for_profile(&token_set.access_token);

                    auth_service
                        .store_openai_tokens(&profile, token_set, account_id, true)
                        .await?;
                    clear_pending_oauth_login(config, "openai");

                    println!("Saved profile {profile}");
                    println!("Active profile for openai-codex: {profile}");
                }
                "gemini" => {
                    let pending = load_pending_oauth_login(config, "gemini")?.ok_or_else(|| {
                        anyhow::anyhow!(
                            "No pending Gemini login found. Run `synapseclaw auth login --provider gemini` first."
                        )
                    })?;

                    if pending.profile != profile {
                        bail!(
                            "Pending login profile mismatch: pending={}, requested={}",
                            pending.profile,
                            profile
                        );
                    }

                    let redirect_input = match input {
                        Some(value) => value,
                        None => read_plain_input("Paste redirect URL or OAuth code")?,
                    };

                    let code = synapse_providers::auth::gemini_oauth::parse_code_from_redirect(
                        &redirect_input,
                        Some(&pending.state),
                    )?;

                    let pkce = synapse_providers::auth::gemini_oauth::PkceState {
                        code_verifier: pending.code_verifier.clone(),
                        code_challenge: String::new(),
                        state: pending.state.clone(),
                    };

                    let client = reqwest::Client::new();
                    let token_set =
                        synapse_providers::auth::gemini_oauth::exchange_code_for_tokens(
                            &client, &code, &pkce,
                        )
                        .await?;
                    let account_id = token_set.id_token.as_deref().and_then(
                        synapse_providers::auth::gemini_oauth::extract_account_email_from_id_token,
                    );

                    auth_service
                        .store_gemini_tokens(&profile, token_set, account_id, true)
                        .await?;
                    clear_pending_oauth_login(config, "gemini");

                    println!("Saved profile {profile}");
                    println!("Active profile for gemini: {profile}");
                }
                _ => {
                    bail!("`auth paste-redirect` supports --provider openai-codex or gemini");
                }
            }
            Ok(())
        }

        AuthCommands::PasteToken {
            provider,
            profile,
            token,
            auth_kind,
        } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;
            let token = match token {
                Some(token) => token.trim().to_string(),
                None => read_auth_input("Paste token")?,
            };
            if token.is_empty() {
                bail!("Token cannot be empty");
            }

            let kind = synapse_providers::auth::anthropic_token::detect_auth_kind(
                &token,
                auth_kind.as_deref(),
            );
            let mut metadata = std::collections::HashMap::new();
            metadata.insert(
                "auth_kind".to_string(),
                kind.as_metadata_value().to_string(),
            );

            auth_service
                .store_provider_token(&provider, &profile, &token, metadata, true)
                .await?;
            println!("Saved profile {profile}");
            println!("Active profile for {provider}: {profile}");
            Ok(())
        }

        AuthCommands::SetupToken { provider, profile } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;
            let token = read_auth_input("Paste token")?;
            if token.is_empty() {
                bail!("Token cannot be empty");
            }

            let kind = synapse_providers::auth::anthropic_token::detect_auth_kind(
                &token,
                Some("authorization"),
            );
            let mut metadata = std::collections::HashMap::new();
            metadata.insert(
                "auth_kind".to_string(),
                kind.as_metadata_value().to_string(),
            );

            auth_service
                .store_provider_token(&provider, &profile, &token, metadata, true)
                .await?;
            println!("Saved profile {profile}");
            println!("Active profile for {provider}: {profile}");
            Ok(())
        }

        AuthCommands::Refresh { provider, profile } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;

            match provider.as_str() {
                "openai-codex" => {
                    match auth_service
                        .get_valid_openai_access_token(profile.as_deref())
                        .await?
                    {
                        Some(_) => {
                            println!("OpenAI Codex token is valid (refresh completed if needed).");
                            Ok(())
                        }
                        None => {
                            bail!(
                                "No OpenAI Codex auth profile found. Run `synapseclaw auth login --provider openai-codex`."
                            )
                        }
                    }
                }
                "gemini" => {
                    match auth_service
                        .get_valid_gemini_access_token(profile.as_deref())
                        .await?
                    {
                        Some(_) => {
                            let profile_name = profile.as_deref().unwrap_or("default");
                            println!("✓ Gemini token refreshed successfully");
                            println!("  Profile: gemini:{}", profile_name);
                            Ok(())
                        }
                        None => {
                            bail!(
                                "No Gemini auth profile found. Run `synapseclaw auth login --provider gemini`."
                            )
                        }
                    }
                }
                _ => bail!("`auth refresh` supports --provider openai-codex or gemini"),
            }
        }

        AuthCommands::Logout { provider, profile } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;
            let removed = auth_service.remove_profile(&provider, &profile).await?;
            if removed {
                println!("Removed auth profile {provider}:{profile}");
            } else {
                println!("Auth profile not found: {provider}:{profile}");
            }
            Ok(())
        }

        AuthCommands::Use { provider, profile } => {
            let provider = synapse_providers::auth::normalize_provider(&provider)?;
            auth_service.set_active_profile(&provider, &profile).await?;
            println!("Active profile for {provider}: {profile}");
            Ok(())
        }

        AuthCommands::List => {
            let data = auth_service.load_profiles().await?;
            if data.profiles.is_empty() {
                println!("No auth profiles configured.");
                return Ok(());
            }

            for (id, profile) in &data.profiles {
                let active = data
                    .active_profiles
                    .get(&profile.provider)
                    .is_some_and(|active_id| active_id == id);
                let marker = if active { "*" } else { " " };
                println!("{marker} {id}");
            }

            Ok(())
        }

        AuthCommands::Status => {
            let data = auth_service.load_profiles().await?;
            if data.profiles.is_empty() {
                println!("No auth profiles configured.");
                return Ok(());
            }

            for (id, profile) in &data.profiles {
                let active = data
                    .active_profiles
                    .get(&profile.provider)
                    .is_some_and(|active_id| active_id == id);
                let marker = if active { "*" } else { " " };
                println!(
                    "{} {} kind={:?} account={} expires={}",
                    marker,
                    id,
                    profile.kind,
                    crate::security::redact(profile.account_id.as_deref().unwrap_or("unknown")),
                    format_expiry(profile)
                );
            }

            println!();
            println!("Active profiles:");
            for (provider, profile_id) in &data.active_profiles {
                println!("  {provider}: {profile_id}");
            }

            Ok(())
        }
    }
}

// ── Pipeline CLI handler (Phase 4.5) ────────────────────────────

async fn handle_pipeline_command(
    cmd: PipelineCommands,
    config: &config::Config,
) -> anyhow::Result<()> {
    use synapse_domain::ports::pipeline_store::PipelineStorePort;
    let mem_backend = synapse_memory::create_memory(config, &config.workspace_dir, "cli").await?;

    match cmd {
        PipelineCommands::Show { name, mermaid } => {
            let pipeline_dir = config
                .pipelines
                .directory
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| config.workspace_dir.join("pipelines"));
            let store =
                synapse_adapters::pipeline::toml_loader::TomlPipelineLoader::new(&pipeline_dir);
            if let Err(e) = store.reload().await {
                anyhow::bail!("Failed to load pipelines: {e}");
            }
            let def = store
                .get(&name)
                .await
                .ok_or_else(|| anyhow::anyhow!("Pipeline '{name}' not found"))?;
            if mermaid {
                println!("{}", def.to_mermaid());
            } else {
                println!("{}", def.to_ascii());
            }
        }
        PipelineCommands::DeadLetters { limit, all } => {
            let dlq = mem_backend.dead_letter;
            let letters = if all {
                dlq.list_all(limit).await?
            } else {
                dlq.list_pending(limit).await?
            };
            if letters.is_empty() {
                println!("No dead letters found.");
            } else {
                println!(
                    "{:<36} {:<20} {:<15} {:<10} {:<10} {}",
                    "ID", "PIPELINE", "STEP", "AGENT", "ATTEMPTS", "ERROR"
                );
                println!("{}", "-".repeat(110));
                for dl in &letters {
                    let error_short = if dl.error.len() > 40 {
                        format!("{}...", &dl.error[..40])
                    } else {
                        dl.error.clone()
                    };
                    println!(
                        "{:<36} {:<20} {:<15} {:<10} {:<10} {}",
                        dl.id,
                        dl.pipeline_run_id.chars().take(20).collect::<String>(),
                        dl.step_id,
                        dl.agent_id,
                        dl.attempt,
                        error_short,
                    );
                }
                println!("\n{} dead letter(s)", letters.len());
            }
        }
        PipelineCommands::Retry { id } => {
            let dlq = mem_backend.dead_letter;
            dlq.mark_retried(&id).await?;
            println!("Dead letter '{id}' marked as retried.");
        }
        PipelineCommands::Dismiss { id } => {
            let dlq = mem_backend.dead_letter;
            dlq.dismiss(&id, "cli").await?;
            println!("Dead letter '{id}' dismissed.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_definition_has_no_flag_conflicts() {
        Cli::command().debug_assert();
    }

    #[test]
    fn onboard_help_includes_model_flag() {
        let cmd = Cli::command();
        let onboard = cmd
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "onboard")
            .expect("onboard subcommand must exist");

        let has_model_flag = onboard
            .get_arguments()
            .any(|arg| arg.get_id().as_str() == "model" && arg.get_long() == Some("model"));

        assert!(
            has_model_flag,
            "onboard help should include --model for quick setup overrides"
        );
    }

    #[test]
    fn onboard_cli_accepts_model_provider_and_api_key_in_quick_mode() {
        let cli = Cli::try_parse_from([
            "synapseclaw",
            "onboard",
            "--provider",
            "openrouter",
            "--model",
            "custom-model-946",
            "--api-key",
            "sk-issue946",
        ])
        .expect("quick onboard invocation should parse");

        match cli.command {
            Commands::Onboard {
                force,
                channels_only,
                api_key,
                provider,
                model,
                ..
            } => {
                assert!(!force);
                assert!(!channels_only);
                assert_eq!(provider.as_deref(), Some("openrouter"));
                assert_eq!(model.as_deref(), Some("custom-model-946"));
                assert_eq!(api_key.as_deref(), Some("sk-issue946"));
            }
            other => panic!("expected onboard command, got {other:?}"),
        }
    }

    #[test]
    fn completions_cli_parses_supported_shells() {
        for shell in ["bash", "fish", "zsh", "powershell", "elvish"] {
            let cli = Cli::try_parse_from(["synapseclaw", "completions", shell])
                .expect("completions invocation should parse");
            match cli.command {
                Commands::Completions { .. } => {}
                other => panic!("expected completions command, got {other:?}"),
            }
        }
    }

    #[test]
    fn completion_generation_mentions_binary_name() {
        let mut output = Vec::new();
        write_shell_completion(CompletionShell::Bash, &mut output)
            .expect("completion generation should succeed");
        let script = String::from_utf8(output).expect("completion output should be valid utf-8");
        assert!(
            script.contains("synapseclaw"),
            "completion script should reference binary name"
        );
    }

    #[test]
    fn onboard_cli_accepts_force_flag() {
        let cli = Cli::try_parse_from(["synapseclaw", "onboard", "--force"])
            .expect("onboard --force should parse");

        match cli.command {
            Commands::Onboard { force, .. } => assert!(force),
            other => panic!("expected onboard command, got {other:?}"),
        }
    }

    #[test]
    fn onboard_cli_rejects_removed_interactive_flag() {
        // --interactive was removed; onboard auto-detects TTY instead.
        assert!(Cli::try_parse_from(["synapseclaw", "onboard", "--interactive"]).is_err());
    }

    #[test]
    fn onboard_cli_bare_parses() {
        let cli =
            Cli::try_parse_from(["synapseclaw", "onboard"]).expect("bare onboard should parse");

        match cli.command {
            Commands::Onboard { .. } => {}
            other => panic!("expected onboard command, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_estop_default_engage() {
        let cli =
            Cli::try_parse_from(["synapseclaw", "estop"]).expect("estop command should parse");

        match cli.command {
            Commands::Estop {
                estop_command,
                level,
                domains,
                tools,
            } => {
                assert!(estop_command.is_none());
                assert!(level.is_none());
                assert!(domains.is_empty());
                assert!(tools.is_empty());
            }
            other => panic!("expected estop command, got {other:?}"),
        }
    }

    #[test]
    fn cli_parses_estop_resume_domain() {
        let cli =
            Cli::try_parse_from(["synapseclaw", "estop", "resume", "--domain", "*.chase.com"])
                .expect("estop resume command should parse");

        match cli.command {
            Commands::Estop {
                estop_command: Some(EstopSubcommands::Resume { domains, .. }),
                ..
            } => assert_eq!(domains, vec!["*.chase.com".to_string()]),
            other => panic!("expected estop resume command, got {other:?}"),
        }
    }

    #[test]
    fn agent_command_parses_with_temperature() {
        let cli = Cli::try_parse_from(["synapseclaw", "agent", "--temperature", "0.5"])
            .expect("agent command with temperature should parse");

        match cli.command {
            Commands::Agent { temperature, .. } => {
                assert_eq!(temperature, Some(0.5));
            }
            other => panic!("expected agent command, got {other:?}"),
        }
    }

    #[test]
    fn agent_command_parses_without_temperature() {
        let cli = Cli::try_parse_from(["synapseclaw", "agent", "--message", "hello"])
            .expect("agent command without temperature should parse");

        match cli.command {
            Commands::Agent { temperature, .. } => {
                assert_eq!(temperature, None);
            }
            other => panic!("expected agent command, got {other:?}"),
        }
    }

    #[test]
    fn agent_command_parses_session_state_file() {
        let cli = Cli::try_parse_from([
            "synapseclaw",
            "agent",
            "--session-state-file",
            "session.json",
        ])
        .expect("agent command with session state file should parse");

        match cli.command {
            Commands::Agent {
                session_state_file, ..
            } => {
                assert_eq!(session_state_file, Some(PathBuf::from("session.json")));
            }
            other => panic!("expected agent command, got {other:?}"),
        }
    }

    #[test]
    fn agent_fallback_uses_config_default_temperature() {
        // Test that when user doesn't provide --temperature,
        // the fallback logic works correctly
        let mut config = Config::default(); // default_temperature = 0.7
        config.default_temperature = 1.5;

        // Simulate None temperature (user didn't provide --temperature)
        let user_temperature: Option<f64> = std::hint::black_box(None);
        let final_temperature = user_temperature.unwrap_or(config.default_temperature);

        assert!((final_temperature - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn agent_fallback_uses_hardcoded_when_config_uses_default() {
        // Test that when config uses default value (0.7), fallback still works
        let config = Config::default(); // default_temperature = 0.7

        // Simulate None temperature (user didn't provide --temperature)
        let user_temperature: Option<f64> = std::hint::black_box(None);
        let final_temperature = user_temperature.unwrap_or(config.default_temperature);

        assert!((final_temperature - 0.7).abs() < f64::EPSILON);
    }
}
