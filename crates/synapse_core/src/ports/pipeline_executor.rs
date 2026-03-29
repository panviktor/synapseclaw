//! Port: pipeline step executor.
//!
//! Phase 4.1 Slice 2: abstracts how a pipeline step is dispatched to an agent.
//! The default adapter sends IPC messages through the broker; tests use mocks.

use async_trait::async_trait;
use serde_json::Value;

/// Result of executing a single pipeline step.
#[derive(Debug, Clone)]
pub struct StepExecutionResult {
    /// Agent's response payload (JSON).
    pub output: Value,
    /// IPC message sequence number (for audit trail).
    pub message_seq: i64,
}

/// Error from step execution.
#[derive(Debug, Clone)]
pub struct StepExecutionError {
    /// Machine-readable error code.
    pub code: String,
    /// Human-readable error description.
    pub message: String,
    /// Whether this error is transient (worth retrying).
    pub retryable: bool,
}

impl std::fmt::Display for StepExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for StepExecutionError {}

/// Port for executing a pipeline step on a remote agent.
///
/// The pipeline runner calls `execute_step()` for each step.
/// The adapter translates this into an IPC message dispatch + response wait.
#[async_trait]
pub trait PipelineExecutorPort: Send + Sync {
    /// Dispatch a step to an agent and wait for the result.
    ///
    /// - `run_id`: pipeline run identifier (used as IPC session_id)
    /// - `step_id`: which step in the pipeline
    /// - `agent_id`: target agent to execute the step
    /// - `input`: data to send to the agent (JSON)
    /// - `tools`: scoped tool allowlist for this step (empty = all)
    /// - `description`: human-readable step description (included in prompt)
    /// - `timeout_secs`: max wait time (None = no timeout)
    async fn execute_step(
        &self,
        run_id: &str,
        step_id: &str,
        agent_id: &str,
        input: &Value,
        tools: &[String],
        description: &str,
        timeout_secs: Option<u64>,
    ) -> Result<StepExecutionResult, StepExecutionError>;
}
