//! Spawn domain types for ephemeral child agent provisioning.
//!
//! Phase 4.0: models the lifecycle of broker-backed child agent spawns.

/// Request to spawn a child agent.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    /// Prompt / instructions for the child agent.
    pub prompt: String,
    /// Requested trust level (clamped to parent's level by broker).
    pub child_trust_level: u8,
    /// Spawn timeout in seconds (10-3600).
    pub timeout_secs: u32,
    /// Workload profile (e.g. "read_only", "supervised").
    pub workload_profile: Option<String>,
    /// Model override for the child agent.
    pub model_override: Option<String>,
    /// Whether the caller should block until completion.
    pub wait_for_completion: bool,
}

/// Result of provisioning an ephemeral child agent.
#[derive(Debug, Clone)]
pub struct EphemeralAgent {
    /// Spawn session ID (UUID).
    pub session_id: String,
    /// Generated child agent ID ("eph-{parent}-{uuid}").
    pub child_agent_id: String,
    /// Runtime-only bearer token for the child.
    pub child_token: String,
    /// Token/spawn expiry (unix seconds).
    pub expires_at: i64,
    /// Parent agent ID that requested the spawn.
    pub parent_id: String,
    /// Effective trust level after clamping.
    pub effective_trust_level: u8,
}

/// Spawn lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnStatus {
    /// Identity provisioned, subprocess not yet started.
    Provisioned,
    /// Child is executing.
    Running,
    /// Child completed successfully.
    Completed,
    /// Spawn exceeded timeout.
    TimedOut,
    /// Child failed with error.
    Failed,
    /// Token revoked before completion.
    Revoked,
    /// Interrupted by parent or operator.
    Interrupted,
}

impl SpawnStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::TimedOut | Self::Failed | Self::Revoked | Self::Interrupted
        )
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "provisioned" => Self::Provisioned,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "timeout" => Self::TimedOut,
            "failed" => Self::Failed,
            "revoked" => Self::Revoked,
            "interrupted" => Self::Interrupted,
            _ => Self::Failed,
        }
    }
}

impl std::fmt::Display for SpawnStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provisioned => write!(f, "provisioned"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::TimedOut => write!(f, "timeout"),
            Self::Failed => write!(f, "failed"),
            Self::Revoked => write!(f, "revoked"),
            Self::Interrupted => write!(f, "interrupted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_status_terminal() {
        assert!(!SpawnStatus::Provisioned.is_terminal());
        assert!(!SpawnStatus::Running.is_terminal());
        assert!(SpawnStatus::Completed.is_terminal());
        assert!(SpawnStatus::TimedOut.is_terminal());
        assert!(SpawnStatus::Failed.is_terminal());
    }

    #[test]
    fn spawn_status_round_trip() {
        for s in &[
            SpawnStatus::Provisioned,
            SpawnStatus::Running,
            SpawnStatus::Completed,
            SpawnStatus::TimedOut,
            SpawnStatus::Failed,
        ] {
            assert_eq!(SpawnStatus::from_str_lossy(&s.to_string()), *s);
        }
    }
}
