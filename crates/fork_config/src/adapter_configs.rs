//! Adapter-owned config types that the Config struct references.
//!
//! These types are defined here (not in adapters) so that fork_config
//! can be compiled independently without depending on fork_adapters.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::channel_traits::ChannelConfig;

// ── Browser Delegate ─────────────────────────────────────────────

/// Browser delegation tool configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserDelegateConfig {
    /// Enable browser delegation tool.
    #[serde(default)]
    pub enabled: bool,
    /// CLI binary to use for browser tasks (default: `"claude"`).
    #[serde(default = "default_browser_cli")]
    pub cli_binary: String,
    /// Chrome profile directory for persistent SSO sessions.
    #[serde(default)]
    pub chrome_profile_dir: String,
    /// Allowed domains for browser navigation (empty = allow all non-blocked).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Blocked domains for browser navigation.
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    /// Task timeout in seconds.
    #[serde(default = "default_browser_task_timeout")]
    pub task_timeout_secs: u64,
}

fn default_browser_cli() -> String {
    "claude".into()
}

fn default_browser_task_timeout() -> u64 {
    120
}

impl Default for BrowserDelegateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cli_binary: default_browser_cli(),
            chrome_profile_dir: String::new(),
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            task_timeout_secs: default_browser_task_timeout(),
        }
    }
}

// ── Email Channel ────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

/// Email channel configuration (IMAP/SMTP).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmailConfig {
    /// IMAP server hostname
    pub imap_host: String,
    /// IMAP server port (default: 993 for TLS)
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// IMAP folder to poll (default: INBOX)
    #[serde(default = "default_imap_folder")]
    pub imap_folder: String,
    /// SMTP server hostname
    pub smtp_host: String,
    /// SMTP server port (default: 465 for TLS)
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// Use TLS for SMTP (default: true)
    #[serde(default = "default_true")]
    pub smtp_tls: bool,
    /// Email username for authentication
    pub username: String,
    /// Email password for authentication
    pub password: String,
    /// From address for outgoing emails
    pub from_address: String,
    /// IDLE timeout in seconds before re-establishing connection (default: 1740 = 29 minutes)
    #[serde(default = "default_idle_timeout", alias = "poll_interval_secs")]
    pub idle_timeout_secs: u64,
    /// Allowed sender addresses/domains (empty = deny all, ["*"] = allow all)
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    /// Default subject line for outgoing emails (default: "SynapseClaw Message")
    #[serde(default = "default_subject")]
    pub default_subject: String,
}

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    465
}

fn default_imap_folder() -> String {
    "INBOX".into()
}

fn default_idle_timeout() -> u64 {
    1740
}

fn default_subject() -> String {
    "SynapseClaw Message".into()
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: default_imap_port(),
            imap_folder: default_imap_folder(),
            smtp_host: String::new(),
            smtp_port: default_smtp_port(),
            smtp_tls: true,
            username: String::new(),
            password: String::new(),
            from_address: String::new(),
            idle_timeout_secs: default_idle_timeout(),
            allowed_senders: Vec::new(),
            default_subject: default_subject(),
        }
    }
}

impl ChannelConfig for EmailConfig {
    fn name() -> &'static str {
        "Email"
    }
    fn desc() -> &'static str {
        "Email over IMAP/SMTP"
    }
}

// ── ClawdTalk Channel ────────────────────────────────────────────

/// Voice channel configuration (Telnyx SIP).
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ClawdTalkConfig {
    /// Telnyx API key
    pub api_key: String,
    /// Telnyx connection ID for SIP
    pub connection_id: String,
    /// Phone number to call from (E.164 format)
    pub from_number: String,
    /// Allowed destination numbers or patterns
    #[serde(default)]
    pub allowed_destinations: Vec<String>,
    /// Webhook secret for signature verification
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

impl ChannelConfig for ClawdTalkConfig {
    fn name() -> &'static str {
        "ClawdTalk"
    }
    fn desc() -> &'static str {
        "Voice channel via Telnyx SIP"
    }
}
