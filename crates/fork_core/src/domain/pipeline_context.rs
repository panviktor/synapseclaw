//! Pipeline execution context — shared state for a running pipeline.
//!
//! Phase 4.1 Slice 1: `PipelineContext` tracks the current state of a
//! pipeline run, accumulates step outputs, and records step history.
//! Persisted through `RunStorePort` for checkpointing and recovery.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

// ---------------------------------------------------------------------------
// PipelineContext
// ---------------------------------------------------------------------------

/// Shared state for a single pipeline run.  Serialized to JSON and stored
/// as a `RunEvent` for checkpointing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineContext {
    /// Unique run ID (UUID).
    pub run_id: String,
    /// Which pipeline this run belongs to.
    pub pipeline_name: String,
    /// Version of the pipeline definition when this run started.
    /// Used for stale-definition detection on recovery.
    pub pipeline_version: String,
    /// ID of the step currently being executed (or last completed).
    pub current_step: String,
    /// Current execution state.
    pub state: PipelineState,
    /// Accumulated data: step outputs merged here, keyed by step ID.
    /// e.g. `{ "research": { "topic": "...", "sources": [...] }, "draft": { ... } }`
    pub data: Value,
    /// Nesting depth (0 = top-level, incremented for sub-pipelines).
    pub depth: u8,
    /// Unix timestamp when the pipeline run started.
    pub started_at: i64,
    /// Unix timestamp of the last state change.
    pub updated_at: i64,
    /// History of completed (and in-progress) steps.
    pub step_history: Vec<StepRecord>,
    /// Error message if the pipeline failed.
    pub error: Option<String>,
    /// Who/what triggered this pipeline run.
    pub triggered_by: String,
    /// Parent run ID if this is a sub-pipeline.
    pub parent_run_id: Option<String>,
}

impl PipelineContext {
    /// Create a new context for a fresh pipeline run.
    pub fn new(
        run_id: String,
        pipeline_name: String,
        pipeline_version: String,
        entry_step: String,
        triggered_by: String,
        depth: u8,
        parent_run_id: Option<String>,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            run_id,
            pipeline_name,
            pipeline_version,
            current_step: entry_step,
            state: PipelineState::Running,
            data: Value::Object(serde_json::Map::new()),
            depth,
            started_at: now,
            updated_at: now,
            step_history: Vec::new(),
            error: None,
            triggered_by,
            parent_run_id,
        }
    }

    /// Maximum serialized size of accumulated data (10 MB).
    const MAX_DATA_SIZE: usize = 10 * 1024 * 1024;

    /// Merge a step's output into accumulated data under the step's ID.
    /// If the output would push accumulated data over the size cap,
    /// the output is truncated to a summary.
    pub fn merge_step_output(&mut self, step_id: &str, output: Value) {
        // Check size before inserting to prevent unbounded growth
        let output_size = output.to_string().len();
        let current_size = self.data.to_string().len();
        let truncated = current_size + output_size > Self::MAX_DATA_SIZE;

        if let Value::Object(ref mut map) = self.data {
            if truncated {
                map.insert(
                    step_id.to_string(),
                    Value::String(format!(
                        "[output truncated: {output_size} bytes exceeded {}-byte cap]",
                        Self::MAX_DATA_SIZE
                    )),
                );
            } else {
                map.insert(step_id.to_string(), output);
            }
        }
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Merge fan-out branch results: `fanout.<result_key>` = output.
    pub fn merge_fanout_output(&mut self, result_key: &str, output: Value) {
        if let Value::Object(ref mut map) = self.data {
            let fanout = map
                .entry("fanout")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(ref mut fanout_map) = fanout {
                fanout_map.insert(result_key.to_string(), output);
            }
        }
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Get accumulated data for a specific step.
    pub fn step_output(&self, step_id: &str) -> Option<&Value> {
        self.data.get(step_id)
    }

    /// Maximum step history entries to prevent unbounded growth.
    const MAX_STEP_HISTORY: usize = 500;

    /// Record a step as started.
    pub fn record_step_start(&mut self, step_id: &str, agent_id: &str, attempt: u8) {
        // Evict oldest entries if history is too large
        if self.step_history.len() >= Self::MAX_STEP_HISTORY {
            self.step_history.drain(..self.step_history.len() / 2);
        }
        let now = chrono::Utc::now().timestamp();
        self.step_history.push(StepRecord {
            step_id: step_id.to_string(),
            agent_id: agent_id.to_string(),
            started_at: now,
            finished_at: None,
            attempt,
            status: StepStatus::Running,
            output: None,
            error: None,
        });
        self.current_step = step_id.to_string();
        self.updated_at = now;
    }

    /// Record a step as completed.
    pub fn record_step_complete(&mut self, step_id: &str, output: Option<Value>) {
        let now = chrono::Utc::now().timestamp();
        if let Some(record) = self
            .step_history
            .iter_mut()
            .rev()
            .find(|r| r.step_id == step_id && r.status == StepStatus::Running)
        {
            record.finished_at = Some(now);
            record.status = StepStatus::Completed;
            record.output = output;
        }
        self.updated_at = now;
    }

    /// Record a step as failed.
    pub fn record_step_failure(&mut self, step_id: &str, error: String) {
        let now = chrono::Utc::now().timestamp();
        if let Some(record) = self
            .step_history
            .iter_mut()
            .rev()
            .find(|r| r.step_id == step_id && r.status == StepStatus::Running)
        {
            record.finished_at = Some(now);
            record.status = StepStatus::Failed;
            record.error = Some(error);
        }
        self.updated_at = now;
    }

    /// Mark the pipeline as completed.
    pub fn complete(&mut self) {
        self.state = PipelineState::Completed;
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Mark the pipeline as failed.
    pub fn fail(&mut self, error: String) {
        self.state = PipelineState::Failed;
        self.error = Some(error);
        self.updated_at = chrono::Utc::now().timestamp();
    }

    /// Total number of completed steps.
    pub fn completed_step_count(&self) -> usize {
        self.step_history
            .iter()
            .filter(|r| r.status == StepStatus::Completed)
            .count()
    }

    /// Duration in milliseconds from start to now (or last update).
    pub fn duration_ms(&self) -> i64 {
        (self.updated_at - self.started_at) * 1000
    }
}

// ---------------------------------------------------------------------------
// PipelineState
// ---------------------------------------------------------------------------

/// Current execution state of a pipeline run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineState {
    /// Actively executing steps.
    Running,
    /// Waiting for an agent to respond to a dispatched step.
    WaitingForAgent(String),
    /// Waiting for human approval.
    WaitingForApproval(String),
    /// Waiting for parallel fan-out branches to complete.
    WaitingForFanOut(Vec<String>),
    /// All steps completed successfully.
    Completed,
    /// Pipeline failed (see `error` field in context).
    Failed,
    /// Pipeline was cancelled by operator.
    Cancelled,
    /// Pipeline exceeded its timeout.
    TimedOut,
}

impl PipelineState {
    /// Whether this is a terminal state (no further transitions possible).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

impl fmt::Display for PipelineState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::WaitingForAgent(id) => write!(f, "waiting_for_agent:{id}"),
            Self::WaitingForApproval(id) => write!(f, "waiting_for_approval:{id}"),
            Self::WaitingForFanOut(ids) => write!(f, "waiting_for_fanout:{}", ids.join(",")),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::TimedOut => write!(f, "timed_out"),
        }
    }
}

// ---------------------------------------------------------------------------
// StepRecord + StepStatus
// ---------------------------------------------------------------------------

/// Record of a step execution attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    /// Step ID.
    pub step_id: String,
    /// Agent that executed (or is executing) this step.
    pub agent_id: String,
    /// Unix timestamp when the step started.
    pub started_at: i64,
    /// Unix timestamp when the step finished (None if still running).
    pub finished_at: Option<i64>,
    /// Attempt number (0-based, incremented on retry).
    pub attempt: u8,
    /// Current status of this step attempt.
    pub status: StepStatus,
    /// Step output (only set on completion).
    pub output: Option<Value>,
    /// Error message (only set on failure).
    pub error: Option<String>,
}

impl StepRecord {
    /// Duration in milliseconds (None if still running).
    pub fn duration_ms(&self) -> Option<i64> {
        self.finished_at.map(|f| (f - self.started_at) * 1000)
    }
}

/// Status of a single step execution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Running,
    Completed,
    Failed,
    Retrying,
    Skipped,
    TimedOut,
}

impl fmt::Display for StepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Retrying => write!(f, "retrying"),
            Self::Skipped => write!(f, "skipped"),
            Self::TimedOut => write!(f, "timed_out"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_ctx() -> PipelineContext {
        PipelineContext::new(
            "run-1".into(),
            "test-pipeline".into(),
            "1.0".into(),
            "step1".into(),
            "operator".into(),
            0,
            None,
        )
    }

    #[test]
    fn new_context_defaults() {
        let ctx = make_ctx();
        assert_eq!(ctx.pipeline_name, "test-pipeline");
        assert_eq!(ctx.pipeline_version, "1.0");
        assert_eq!(ctx.current_step, "step1");
        assert_eq!(ctx.state, PipelineState::Running);
        assert_eq!(ctx.depth, 0);
        assert!(ctx.step_history.is_empty());
        assert!(ctx.error.is_none());
        assert!(ctx.parent_run_id.is_none());
    }

    #[test]
    fn merge_step_output() {
        let mut ctx = make_ctx();
        ctx.merge_step_output("research", json!({"topic": "rust"}));
        assert_eq!(ctx.step_output("research"), Some(&json!({"topic": "rust"})));
        assert_eq!(ctx.step_output("nonexistent"), None);
    }

    #[test]
    fn merge_fanout_output() {
        let mut ctx = make_ctx();
        ctx.merge_fanout_output("news", json!({"headlines": []}));
        ctx.merge_fanout_output("trends", json!({"top": "AI"}));
        let fanout = ctx.data.get("fanout").unwrap();
        assert_eq!(fanout.get("news"), Some(&json!({"headlines": []})));
        assert_eq!(fanout.get("trends"), Some(&json!({"top": "AI"})));
    }

    #[test]
    fn step_recording() {
        let mut ctx = make_ctx();
        ctx.record_step_start("step1", "agent-a", 0);
        assert_eq!(ctx.step_history.len(), 1);
        assert_eq!(ctx.step_history[0].status, StepStatus::Running);

        ctx.record_step_complete("step1", Some(json!({"result": "ok"})));
        assert_eq!(ctx.step_history[0].status, StepStatus::Completed);
        assert!(ctx.step_history[0].finished_at.is_some());
        assert_eq!(ctx.completed_step_count(), 1);
    }

    #[test]
    fn step_failure_recording() {
        let mut ctx = make_ctx();
        ctx.record_step_start("step1", "agent-a", 0);
        ctx.record_step_failure("step1", "timeout".into());
        assert_eq!(ctx.step_history[0].status, StepStatus::Failed);
        assert_eq!(ctx.step_history[0].error, Some("timeout".into()));
    }

    #[test]
    fn pipeline_state_terminal() {
        assert!(!PipelineState::Running.is_terminal());
        assert!(!PipelineState::WaitingForAgent("a".into()).is_terminal());
        assert!(PipelineState::Completed.is_terminal());
        assert!(PipelineState::Failed.is_terminal());
        assert!(PipelineState::Cancelled.is_terminal());
        assert!(PipelineState::TimedOut.is_terminal());
    }

    #[test]
    fn complete_and_fail() {
        let mut ctx = make_ctx();
        ctx.complete();
        assert_eq!(ctx.state, PipelineState::Completed);

        let mut ctx2 = make_ctx();
        ctx2.fail("something broke".into());
        assert_eq!(ctx2.state, PipelineState::Failed);
        assert_eq!(ctx2.error, Some("something broke".into()));
    }

    #[test]
    fn context_serialization_roundtrip() {
        let mut ctx = make_ctx();
        ctx.merge_step_output("step1", json!({"x": 1}));
        ctx.record_step_start("step1", "agent-a", 0);
        ctx.record_step_complete("step1", Some(json!({"x": 1})));

        let json = serde_json::to_string(&ctx).unwrap();
        let ctx2: PipelineContext = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx2.run_id, ctx.run_id);
        assert_eq!(ctx2.pipeline_name, ctx.pipeline_name);
        assert_eq!(ctx2.step_history.len(), 1);
        assert_eq!(ctx2.step_history[0].status, StepStatus::Completed);
    }
}
