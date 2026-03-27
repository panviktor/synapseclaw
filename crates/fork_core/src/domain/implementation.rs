//! Implementation domain types — external coding workers.
//!
//! Phase 4.0 Slice 7: defines the seam for external coding engines
//! (Codex, Claude Code, etc.) as bounded leaf executors.
//!
//! Design rule: an implementation task is a structured execution contract,
//! not a free-form chat prompt. The fork core remains the orchestration
//! authority — external workers are leaf executors only.

use std::fmt;

/// State of an implementation run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplementationState {
    Queued,
    Dispatching,
    Running,
    Blocked,
    ApprovalRequired,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl fmt::Display for ImplementationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queued => write!(f, "queued"),
            Self::Dispatching => write!(f, "dispatching"),
            Self::Running => write!(f, "running"),
            Self::Blocked => write!(f, "blocked"),
            Self::ApprovalRequired => write!(f, "approval_required"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Interrupted => write!(f, "interrupted"),
        }
    }
}

impl ImplementationState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

/// Expected output format from a coding worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedOutput {
    Patch,
    Branch,
    Report,
}

/// A bounded implementation task for an external coding worker.
#[derive(Debug, Clone)]
pub struct ImplementationTask {
    pub task_id: String,
    pub objective: String,
    pub repo_ref: String,
    pub worktree_ref: Option<String>,
    pub constraints: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub tests_to_run: Vec<String>,
    pub timeout_secs: u64,
    pub expected_output: ExpectedOutput,
}

/// Result from an external coding worker.
#[derive(Debug, Clone)]
pub struct CodingWorkerResult {
    pub task_id: String,
    pub state: ImplementationState,
    pub summary: String,
    pub changed_files: Vec<String>,
    pub test_results: Vec<String>,
    pub questions: Vec<String>,
    pub artifacts: Vec<String>,
}

/// Progress event from an external coding worker.
#[derive(Debug, Clone)]
pub struct ImplementationEvent {
    pub run_id: String,
    pub event_type: ImplementationEventType,
    pub content: String,
    pub created_at: u64,
}

/// Types of implementation run events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplementationEventType {
    Progress,
    Question,
    Artifact,
    Blocked,
    ApprovalRequired,
    Result,
    Failure,
}

impl fmt::Display for ImplementationEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Progress => write!(f, "progress"),
            Self::Question => write!(f, "question"),
            Self::Artifact => write!(f, "artifact"),
            Self::Blocked => write!(f, "blocked"),
            Self::ApprovalRequired => write!(f, "approval_required"),
            Self::Result => write!(f, "result"),
            Self::Failure => write!(f, "failure"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states() {
        assert!(ImplementationState::Completed.is_terminal());
        assert!(ImplementationState::Failed.is_terminal());
        assert!(ImplementationState::Cancelled.is_terminal());
        assert!(!ImplementationState::Running.is_terminal());
        assert!(!ImplementationState::Queued.is_terminal());
    }

    #[test]
    fn state_display() {
        assert_eq!(ImplementationState::Running.to_string(), "running");
        assert_eq!(
            ImplementationState::ApprovalRequired.to_string(),
            "approval_required"
        );
    }

    #[test]
    fn event_type_display() {
        assert_eq!(ImplementationEventType::Progress.to_string(), "progress");
        assert_eq!(ImplementationEventType::Artifact.to_string(), "artifact");
    }
}
