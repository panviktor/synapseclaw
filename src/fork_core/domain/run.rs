//! Run domain types for the fork-owned application core.
//!
//! A `Run` is a first-class execution record.  Phase 4.0 unifies chat runs,
//! IPC execution, spawn runs, and cron jobs under one model so they stop
//! inventing separate lifecycle tables.

use std::fmt;

/// Where the run originated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOrigin {
    /// Web dashboard chat.
    Web,
    /// Human messaging channel (Telegram, Matrix, etc.).
    Channel,
    /// Inter-agent IPC task/query.
    Ipc,
    /// Ephemeral agent spawn.
    Spawn,
    /// Cron/scheduler-triggered execution.
    Cron,
}

impl fmt::Display for RunOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Web => write!(f, "web"),
            Self::Channel => write!(f, "channel"),
            Self::Ipc => write!(f, "ipc"),
            Self::Spawn => write!(f, "spawn"),
            Self::Cron => write!(f, "cron"),
        }
    }
}

/// Execution lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Interrupted,
    Failed,
    Cancelled,
}

impl fmt::Display for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Interrupted => write!(f, "interrupted"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl RunState {
    /// Parse from string (e.g. from DB).
    #[allow(clippy::match_same_arms)] // explicit arms document known DB values
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "interrupted" => Self::Interrupted,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Failed,
        }
    }

    /// Whether this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Interrupted | Self::Failed | Self::Cancelled
        )
    }
}

/// A first-class execution record.
#[derive(Debug, Clone)]
pub struct Run {
    /// Unique run identifier (UUID).
    pub run_id: String,
    /// Associated conversation (if any).
    pub conversation_key: Option<String>,
    /// Where this run originated.
    pub origin: RunOrigin,
    /// Current lifecycle state.
    pub state: RunState,
    /// Start timestamp (unix seconds).
    pub started_at: u64,
    /// Completion timestamp (unix seconds).
    pub finished_at: Option<u64>,
}

/// Type of event within a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEventType {
    /// Progress update (partial result).
    Progress,
    /// Tool was invoked.
    ToolCall,
    /// Tool returned a result.
    ToolResult,
    /// Final result of the run.
    Result,
    /// Run failed with error.
    Failure,
}

impl fmt::Display for RunEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Progress => write!(f, "progress"),
            Self::ToolCall => write!(f, "tool_call"),
            Self::ToolResult => write!(f, "tool_result"),
            Self::Result => write!(f, "result"),
            Self::Failure => write!(f, "failure"),
        }
    }
}

impl RunEventType {
    /// Parse from string (e.g. from DB).
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "progress" => Self::Progress,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            "result" => Self::Result,
            _ => Self::Failure,
        }
    }
}

/// A single event within a run's lifecycle.
#[derive(Debug, Clone)]
pub struct RunEvent {
    /// Which run this event belongs to.
    pub run_id: String,
    /// Event type.
    pub event_type: RunEventType,
    /// Event content (tool output, progress text, error message).
    pub content: String,
    /// Tool name (for ToolCall/ToolResult events).
    pub tool_name: Option<String>,
    /// Event timestamp (unix seconds).
    pub created_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_origin_display() {
        assert_eq!(RunOrigin::Web.to_string(), "web");
        assert_eq!(RunOrigin::Spawn.to_string(), "spawn");
        assert_eq!(RunOrigin::Cron.to_string(), "cron");
    }

    #[test]
    fn run_state_terminal() {
        assert!(!RunState::Running.is_terminal());
        assert!(!RunState::Queued.is_terminal());
        assert!(RunState::Completed.is_terminal());
        assert!(RunState::Failed.is_terminal());
        assert!(RunState::Interrupted.is_terminal());
        assert!(RunState::Cancelled.is_terminal());
    }

    #[test]
    fn run_state_from_str_lossy() {
        assert_eq!(RunState::from_str_lossy("running"), RunState::Running);
        assert_eq!(RunState::from_str_lossy("completed"), RunState::Completed);
        assert_eq!(RunState::from_str_lossy("unknown"), RunState::Failed);
    }

    #[test]
    fn run_event_type_round_trip() {
        for t in &[
            RunEventType::Progress,
            RunEventType::ToolCall,
            RunEventType::ToolResult,
            RunEventType::Result,
            RunEventType::Failure,
        ] {
            assert_eq!(RunEventType::from_str_lossy(&t.to_string()), *t);
        }
    }
}
