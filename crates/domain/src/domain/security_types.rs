//! Security domain types — pure data enums/structs shared by security infra and adapters.

use std::str::FromStr;

/// Action to take when prompt guard detects suspicious content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuardAction {
    /// Log warning but allow the message.
    #[default]
    Warn,
    /// Block the message with an error.
    Block,
    /// Sanitize by removing/escaping dangerous patterns.
    Sanitize,
}

impl FromStr for GuardAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().trim() {
            "warn" => Ok(Self::Warn),
            "block" => Ok(Self::Block),
            "sanitize" => Ok(Self::Sanitize),
            other => Err(format!("unknown guard action: {other}")),
        }
    }
}

/// Result of prompt guard analysis.
#[derive(Debug, Clone)]
pub enum GuardResult {
    /// Message is safe.
    Safe,
    /// Message contains suspicious patterns (with detection details and score).
    Suspicious(Vec<String>, f64),
    /// Message should be blocked (with reason).
    Blocked(String),
}

/// Result of credential leak detection.
#[derive(Debug, Clone)]
pub enum LeakResult {
    /// No leaks detected.
    Clean,
    /// Potential leaks detected with redacted versions.
    Detected {
        /// Descriptions of detected leak patterns.
        patterns: Vec<String>,
        /// Content with sensitive values redacted.
        redacted: String,
    },
}

/// Autonomy level for execution boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryAutonomy {
    Full,
    Supervised,
    ReadOnly,
}
