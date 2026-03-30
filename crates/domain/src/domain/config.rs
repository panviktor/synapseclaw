//! Core-owned config types — minimal projections of upstream Config.
//!
//! Adapters convert from the full `Config` struct to these lean types.
//! This keeps fork_core free from upstream config dependencies.

/// Heartbeat delivery configuration — fields needed by DeliveryService.
#[derive(Debug, Clone, Default)]
pub struct HeartbeatConfig {
    /// Explicit target channel name (e.g. "matrix", "telegram").
    pub target: Option<String>,
    /// Explicit delivery recipient (room ID, chat ID).
    pub to: Option<String>,
    /// Deadman alert channel override.
    pub deadman_channel: Option<String>,
    /// Deadman alert recipient override.
    pub deadman_to: Option<String>,
}

/// Cron job delivery configuration.
#[derive(Debug, Clone, Default)]
pub struct CronDeliveryConfig {
    /// Delivery mode: "announce" triggers delivery, anything else is a no-op.
    pub mode: String,
    /// Target channel name.
    pub channel: Option<String>,
    /// Target recipient.
    pub to: Option<String>,
}

/// A candidate for auto-detection: channel name + optional recipient.
///
/// DeliveryService iterates these and picks the first with a recipient
/// and `SendText` capability.
#[derive(Debug, Clone)]
pub struct AutoDetectCandidate {
    pub channel_name: String,
    pub recipient: Option<String>,
}

/// Autonomy level for tool approval decisions.
///
/// Single source of truth — used by both fork_core and security::policy.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Can observe but not act.
    ReadOnly,
    /// Acts but requires approval for risky operations.
    #[default]
    Supervised,
    /// Autonomous execution within policy bounds.
    Full,
}

/// Classifies whether a tool operation is read-only or side-effecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOperation {
    Read,
    Act,
}

/// Risk score for shell command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRiskLevel {
    Low,
    Medium,
    High,
}
