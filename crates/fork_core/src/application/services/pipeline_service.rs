//! Pipeline runner service — deterministic multi-agent workflow execution.
//!
//! Phase 4.1 Slice 2: core pipeline execution loop.
//!
//! Orchestrates: load definition → validate input → dispatch step via executor
//! → validate output → advance state → checkpoint → repeat.
//!
//! This service only handles **sequential** pipelines (`StepTransition::Next`).
//! Conditional edges, FanOut, WaitForApproval, and SubPipeline are added in
//! later slices.

use crate::domain::pipeline::{PipelineDefinition, PipelineStep, StepTransition};
use crate::domain::pipeline_context::{PipelineContext, PipelineState, StepStatus};
use crate::domain::run::{Run, RunEvent, RunEventType, RunOrigin, RunState};
use crate::ports::pipeline_executor::PipelineExecutorPort;
use crate::ports::pipeline_store::PipelineStorePort;
use crate::ports::run_store::RunStorePort;
use serde_json::Value;
use tracing::{error, info, warn};

/// Ports required by the pipeline runner.
pub struct PipelineRunnerPorts<'a> {
    pub pipeline_store: &'a dyn PipelineStorePort,
    pub run_store: &'a dyn RunStorePort,
    pub executor: &'a dyn PipelineExecutorPort,
}

/// Result of starting a pipeline.
#[derive(Debug, Clone)]
pub struct PipelineRunResult {
    /// The pipeline run ID.
    pub run_id: String,
    /// Final state.
    pub state: PipelineState,
    /// Final accumulated data.
    pub data: Value,
    /// Number of completed steps.
    pub step_count: usize,
    /// Error message (if failed).
    pub error: Option<String>,
}

/// Parameters for starting a pipeline.
pub struct StartPipelineParams {
    /// Pipeline name to execute.
    pub pipeline_name: String,
    /// Initial input data (passed to first step).
    pub input: Value,
    /// Who/what triggered this run.
    pub triggered_by: String,
    /// Nesting depth (0 = top-level).
    pub depth: u8,
    /// Parent run ID (if sub-pipeline).
    pub parent_run_id: Option<String>,
}

/// Run a complete pipeline from entry_point to end.
///
/// This is the main execution loop. For each step:
/// 1. Validate input against step's input_schema
/// 2. Dispatch to agent via PipelineExecutorPort
/// 3. Validate output against step's output_schema
/// 4. Merge output into context
/// 5. Checkpoint via RunStorePort
/// 6. Advance to next step
pub async fn run_pipeline(
    ports: &PipelineRunnerPorts<'_>,
    params: StartPipelineParams,
) -> PipelineRunResult {
    // Load pipeline definition
    let definition = match ports.pipeline_store.get(&params.pipeline_name).await {
        Some(def) => def,
        None => {
            return PipelineRunResult {
                run_id: String::new(),
                state: PipelineState::Failed,
                data: Value::Null,
                step_count: 0,
                error: Some(format!(
                    "pipeline '{}' not found",
                    params.pipeline_name
                )),
            };
        }
    };

    // Create run context
    let run_id = uuid_v4();
    let mut ctx = PipelineContext::new(
        run_id.clone(),
        definition.name.clone(),
        definition.version.clone(),
        definition.entry_point.clone(),
        params.triggered_by.clone(),
        params.depth,
        params.parent_run_id.clone(),
    );

    // Store initial input as "_input" in context data
    ctx.merge_step_output("_input", params.input.clone());

    // Create Run record
    if let Err(e) = create_pipeline_run(ports.run_store, &ctx).await {
        error!(run_id = %run_id, error = %e, "failed to create pipeline run record");
        return PipelineRunResult {
            run_id,
            state: PipelineState::Failed,
            data: ctx.data,
            step_count: 0,
            error: Some(format!("failed to create run record: {e}")),
        };
    }

    // Checkpoint initial state
    checkpoint(ports.run_store, &ctx).await;

    info!(
        run_id = %run_id,
        pipeline = %definition.name,
        version = %definition.version,
        entry_point = %definition.entry_point,
        "pipeline started"
    );

    // Main execution loop
    let result = execute_loop(ports, &definition, &mut ctx, &params.input).await;

    // Final checkpoint
    checkpoint(ports.run_store, &ctx).await;

    // Update Run record to terminal state
    let run_state = match ctx.state {
        PipelineState::Completed => RunState::Completed,
        PipelineState::Cancelled => RunState::Cancelled,
        PipelineState::TimedOut => RunState::Failed,
        _ => RunState::Failed,
    };
    let finished_at = Some(chrono::Utc::now().timestamp() as u64);
    if let Err(e) = ports
        .run_store
        .update_state(&ctx.run_id, run_state, finished_at)
        .await
    {
        warn!(run_id = %ctx.run_id, error = %e, "failed to update run state");
    }

    info!(
        run_id = %run_id,
        pipeline = %definition.name,
        state = %ctx.state,
        steps = ctx.completed_step_count(),
        "pipeline finished"
    );

    result
}

/// Inner execution loop — iterates through steps until End or failure.
async fn execute_loop(
    ports: &PipelineRunnerPorts<'_>,
    definition: &PipelineDefinition,
    ctx: &mut PipelineContext,
    initial_input: &Value,
) -> PipelineRunResult {
    let mut current_step_id = definition.entry_point.clone();

    // Check global timeout
    let deadline = definition
        .timeout_secs
        .map(|t| ctx.started_at + t as i64);

    loop {
        // Global timeout check
        if let Some(dl) = deadline {
            if chrono::Utc::now().timestamp() > dl {
                ctx.state = PipelineState::TimedOut;
                ctx.error = Some("pipeline global timeout exceeded".into());
                return make_result(ctx);
            }
        }

        // Resolve the step definition
        let step = match definition.step(&current_step_id) {
            Some(s) => s,
            None => {
                ctx.fail(format!("step '{}' not found in definition", current_step_id));
                return make_result(ctx);
            }
        };

        // Determine input for this step
        let step_input = resolve_step_input(ctx, &current_step_id, initial_input, definition);

        // Validate input schema
        if let Some(ref schema) = step.input_schema {
            if let Err(errors) =
                crate::domain::pipeline_validation::validate_schema(&step_input, schema)
            {
                ctx.fail(format!(
                    "step '{}' input validation failed: {}",
                    current_step_id, errors
                ));
                return make_result(ctx);
            }
        }

        // Execute the step (with retries)
        let step_output = match execute_step_with_retries(ports, ctx, step, &step_input).await {
            Some(output) => output,
            None => {
                // ctx.state and error already set by execute_step_with_retries
                return make_result(ctx);
            }
        };

        // Validate output schema
        if let Some(ref schema) = step.output_schema {
            if let Err(errors) =
                crate::domain::pipeline_validation::validate_schema(&step_output, schema)
            {
                ctx.fail(format!(
                    "step '{}' output validation failed: {}",
                    current_step_id, errors
                ));
                return make_result(ctx);
            }
        }

        // Merge output into context
        ctx.merge_step_output(&current_step_id, step_output.clone());

        // Checkpoint after successful step
        checkpoint(ports.run_store, ctx).await;

        // Advance to next step
        match &step.next {
            StepTransition::Next(next_id) => {
                if next_id == "end" {
                    ctx.complete();
                    return make_result(ctx);
                }
                current_step_id = next_id.clone();
            }
            StepTransition::Complex(complex) => {
                // Slice 2: only sequential. Complex transitions handled in later slices.
                // For now, evaluate conditionals if possible, else fail.
                match complex.as_ref() {
                    crate::domain::pipeline::ComplexTransition::Conditional {
                        branches,
                        fallback,
                    } => {
                        let target = branches
                            .iter()
                            .find(|b| b.evaluate(&step_output))
                            .map(|b| b.target.as_str())
                            .unwrap_or(fallback.as_str());

                        if target == "end" {
                            ctx.complete();
                            return make_result(ctx);
                        }
                        current_step_id = target.to_string();
                    }
                    other => {
                        ctx.fail(format!(
                            "step '{}' uses unsupported transition type (not yet implemented): {:?}",
                            current_step_id,
                            std::mem::discriminant(other)
                        ));
                        return make_result(ctx);
                    }
                }
            }
        }
    }
}

/// Execute a step with retry logic.
/// Returns `Some(output)` on success, `None` on failure (ctx.state already set).
async fn execute_step_with_retries(
    ports: &PipelineRunnerPorts<'_>,
    ctx: &mut PipelineContext,
    step: &PipelineStep,
    input: &Value,
) -> Option<Value> {
    let max_attempts = step.max_retries as u32 + 1; // +1 for initial attempt

    for attempt in 0..max_attempts {
        ctx.record_step_start(&step.id, &step.agent_id, attempt as u8);
        ctx.state = PipelineState::WaitingForAgent(step.agent_id.clone());

        // Checkpoint before dispatch (so recovery knows we're waiting)
        checkpoint(ports.run_store, ctx).await;

        let result = ports
            .executor
            .execute_step(
                &ctx.run_id,
                &step.id,
                &step.agent_id,
                input,
                &step.tools,
                &step.description,
                step.timeout_secs,
            )
            .await;

        match result {
            Ok(exec_result) => {
                ctx.record_step_complete(&step.id, Some(exec_result.output.clone()));
                ctx.state = PipelineState::Running;
                info!(
                    run_id = %ctx.run_id,
                    step = %step.id,
                    agent = %step.agent_id,
                    attempt = attempt,
                    seq = exec_result.message_seq,
                    "step completed"
                );
                return Some(exec_result.output);
            }
            Err(err) => {
                ctx.record_step_failure(&step.id, err.message.clone());

                let is_last_attempt = attempt + 1 >= max_attempts;
                if is_last_attempt || !err.retryable {
                    ctx.fail(format!(
                        "step '{}' failed after {} attempt(s): {}",
                        step.id,
                        attempt + 1,
                        err.message
                    ));
                    error!(
                        run_id = %ctx.run_id,
                        step = %step.id,
                        agent = %step.agent_id,
                        attempt = attempt,
                        error = %err.message,
                        retryable = err.retryable,
                        "step failed (final)"
                    );
                    return None;
                }

                // Retry with backoff
                warn!(
                    run_id = %ctx.run_id,
                    step = %step.id,
                    agent = %step.agent_id,
                    attempt = attempt,
                    error = %err.message,
                    backoff_secs = step.retry_backoff_secs,
                    "step failed, retrying"
                );
                // Mark as Retrying in step history
                if let Some(record) = ctx
                    .step_history
                    .iter_mut()
                    .rev()
                    .find(|r| r.step_id == step.id && r.status == StepStatus::Failed)
                {
                    record.status = StepStatus::Retrying;
                }
                tokio::time::sleep(std::time::Duration::from_secs(step.retry_backoff_secs)).await;
            }
        }
    }
    None
}

/// Determine what input to pass to a step.
///
/// - First step: receives the initial pipeline input (from `_input`).
/// - Subsequent steps: receive the previous step's output.
fn resolve_step_input(
    ctx: &PipelineContext,
    step_id: &str,
    initial_input: &Value,
    definition: &PipelineDefinition,
) -> Value {
    // If this is the entry point, use initial input
    if step_id == definition.entry_point {
        return initial_input.clone();
    }

    // Find the most recent completed step in history and use its output
    if let Some(record) = ctx
        .step_history
        .iter()
        .rev()
        .find(|r| r.status == StepStatus::Completed && r.output.is_some())
    {
        return record.output.clone().unwrap_or(Value::Null);
    }

    // Fallback: pass the full accumulated data
    ctx.data.clone()
}

/// Checkpoint: serialize PipelineContext and store as RunEvent.
async fn checkpoint(run_store: &dyn RunStorePort, ctx: &PipelineContext) {
    let content = match serde_json::to_string(ctx) {
        Ok(s) => s,
        Err(e) => {
            warn!(run_id = %ctx.run_id, error = %e, "failed to serialize pipeline context");
            return;
        }
    };

    let event = RunEvent {
        run_id: ctx.run_id.clone(),
        event_type: RunEventType::Progress,
        content,
        tool_name: Some("pipeline_checkpoint".into()),
        created_at: chrono::Utc::now().timestamp() as u64,
    };

    if let Err(e) = run_store.append_event(&event).await {
        warn!(run_id = %ctx.run_id, error = %e, "failed to checkpoint pipeline context");
    }
}

/// Create the initial Run record for a pipeline execution.
async fn create_pipeline_run(
    run_store: &dyn RunStorePort,
    ctx: &PipelineContext,
) -> anyhow::Result<()> {
    let run = Run {
        run_id: ctx.run_id.clone(),
        conversation_key: Some(format!("pipeline:{}", ctx.pipeline_name)),
        origin: RunOrigin::Pipeline,
        state: RunState::Running,
        started_at: ctx.started_at as u64,
        finished_at: None,
    };
    run_store.create_run(&run).await
}

/// Build a PipelineRunResult from the current context.
fn make_result(ctx: &PipelineContext) -> PipelineRunResult {
    PipelineRunResult {
        run_id: ctx.run_id.clone(),
        state: ctx.state.clone(),
        data: ctx.data.clone(),
        step_count: ctx.completed_step_count(),
        error: ctx.error.clone(),
    }
}

/// Generate a UUID v4 string.
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple pseudo-UUID from timestamp + random-ish bits.
    // Good enough for run IDs; not cryptographic.
    let hash = nanos.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (hash >> 96) as u32,
        (hash >> 80) as u16,
        (hash >> 64) as u16 & 0xFFF,
        ((hash >> 48) as u16 & 0x3FFF) | 0x8000,
        hash as u64 & 0xFFFF_FFFF_FFFF,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pipeline::{
        ConditionalBranch, ComplexTransition, FanOutSpec, Operator, PipelineDefinition,
        PipelineStep, StepTransition,
    };
    use crate::ports::pipeline_executor::{StepExecutionError, StepExecutionResult};
    use crate::ports::pipeline_store::ReloadEvent;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    // -- Mock PipelineStore -------------------------------------------------

    struct MockPipelineStore {
        defs: Vec<PipelineDefinition>,
    }

    #[async_trait]
    impl PipelineStorePort for MockPipelineStore {
        async fn get(&self, name: &str) -> Option<PipelineDefinition> {
            self.defs.iter().find(|d| d.name == name).cloned()
        }
        async fn list(&self) -> Vec<String> {
            self.defs.iter().map(|d| d.name.clone()).collect()
        }
        async fn reload(&self) -> anyhow::Result<Vec<ReloadEvent>> {
            Ok(vec![])
        }
    }

    // -- Mock RunStore ------------------------------------------------------

    struct MockRunStore {
        runs: Mutex<Vec<Run>>,
        events: Mutex<Vec<RunEvent>>,
    }

    impl MockRunStore {
        fn new() -> Self {
            Self {
                runs: Mutex::new(vec![]),
                events: Mutex::new(vec![]),
            }
        }

        fn event_count(&self) -> usize {
            self.events.lock().unwrap().len()
        }

        fn last_checkpoint(&self) -> Option<PipelineContext> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|e| e.tool_name.as_deref() == Some("pipeline_checkpoint"))
                .and_then(|e| serde_json::from_str(&e.content).ok())
        }
    }

    #[async_trait]
    impl RunStorePort for MockRunStore {
        async fn create_run(&self, run: &Run) -> anyhow::Result<()> {
            self.runs.lock().unwrap().push(run.clone());
            Ok(())
        }
        async fn get_run(&self, run_id: &str) -> Option<Run> {
            self.runs.lock().unwrap().iter().find(|r| r.run_id == run_id).cloned()
        }
        async fn update_state(&self, run_id: &str, state: RunState, finished_at: Option<u64>) -> anyhow::Result<()> {
            let mut runs = self.runs.lock().unwrap();
            if let Some(run) = runs.iter_mut().find(|r| r.run_id == run_id) {
                run.state = state;
                run.finished_at = finished_at;
            }
            Ok(())
        }
        async fn list_runs(&self, _key: &str, _limit: usize) -> Vec<Run> { vec![] }
        async fn list_all_runs(&self, _limit: usize) -> Vec<Run> { vec![] }
        async fn append_event(&self, event: &RunEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
        async fn get_events(&self, run_id: &str, _limit: usize) -> Vec<RunEvent> {
            self.events.lock().unwrap().iter().filter(|e| e.run_id == run_id).cloned().collect()
        }
    }

    // -- Mock Executor ------------------------------------------------------

    struct MockExecutor {
        /// Map of step_id → output value. If missing, returns error.
        responses: Mutex<Vec<(String, Result<Value, StepExecutionError>)>>,
    }

    impl MockExecutor {
        fn new(responses: Vec<(String, Result<Value, StepExecutionError>)>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }

        fn succeeds(steps: Vec<(&str, Value)>) -> Self {
            Self::new(
                steps
                    .into_iter()
                    .map(|(id, val)| (id.to_string(), Ok(val)))
                    .collect(),
            )
        }
    }

    #[async_trait]
    impl PipelineExecutorPort for MockExecutor {
        async fn execute_step(
            &self,
            _run_id: &str,
            step_id: &str,
            _agent_id: &str,
            _input: &Value,
            _tools: &[String],
            _description: &str,
            _timeout_secs: Option<u64>,
        ) -> Result<StepExecutionResult, StepExecutionError> {
            let responses = self.responses.lock().unwrap();
            let result = responses
                .iter()
                .find(|(id, _)| id == step_id)
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| {
                    Err(StepExecutionError {
                        code: "not_found".into(),
                        message: format!("no mock response for step '{step_id}'"),
                        retryable: false,
                    })
                });
            result.map(|output| StepExecutionResult {
                output,
                message_seq: 1,
            })
        }
    }

    // -- Helper: build a simple 2-step pipeline -----------------------------

    fn two_step_pipeline() -> PipelineDefinition {
        PipelineDefinition {
            name: "test-two-step".into(),
            version: "1.0".into(),
            description: "Test pipeline".into(),
            steps: vec![
                PipelineStep {
                    id: "step1".into(),
                    agent_id: "agent-a".into(),
                    description: "First step".into(),
                    tools: vec!["web_search".into()],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("step2".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "step2".into(),
                    agent_id: "agent-b".into(),
                    description: "Second step".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("end".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
            ],
            entry_point: "step1".into(),
            max_depth: 5,
            timeout_secs: None,
        }
    }

    // -- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn two_step_sequential_pipeline() {
        let store = MockPipelineStore {
            defs: vec![two_step_pipeline()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("step1", json!({"research": "data"})),
            ("step2", json!({"draft": "text"})),
        ]);

        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = run_pipeline(
            &ports,
            StartPipelineParams {
                pipeline_name: "test-two-step".into(),
                input: json!({"topic": "Rust"}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        assert_eq!(result.step_count, 2);
        assert!(result.error.is_none());
        // Both step outputs should be in accumulated data
        assert_eq!(result.data.get("step1"), Some(&json!({"research": "data"})));
        assert_eq!(result.data.get("step2"), Some(&json!({"draft": "text"})));
    }

    #[tokio::test]
    async fn pipeline_not_found() {
        let store = MockPipelineStore { defs: vec![] };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![]);

        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = run_pipeline(
            &ports,
            StartPipelineParams {
                pipeline_name: "nonexistent".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Failed);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn step_failure_propagates() {
        let store = MockPipelineStore {
            defs: vec![two_step_pipeline()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::new(vec![
            (
                "step1".into(),
                Err(StepExecutionError {
                    code: "agent_error".into(),
                    message: "agent crashed".into(),
                    retryable: false,
                }),
            ),
        ]);

        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = run_pipeline(
            &ports,
            StartPipelineParams {
                pipeline_name: "test-two-step".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Failed);
        assert!(result.error.unwrap().contains("agent crashed"));
        assert_eq!(result.step_count, 0);
    }

    #[tokio::test]
    async fn retry_on_transient_failure() {
        let store = MockPipelineStore {
            defs: vec![{
                let mut p = two_step_pipeline();
                p.steps[0].max_retries = 2;
                p.steps[0].retry_backoff_secs = 0; // no delay in tests
                p
            }],
        };
        let run_store = MockRunStore::new();

        // Executor that fails first, then succeeds
        struct RetryExecutor {
            attempt: Mutex<u32>,
        }
        #[async_trait]
        impl PipelineExecutorPort for RetryExecutor {
            async fn execute_step(
                &self, _run_id: &str, step_id: &str, _agent_id: &str,
                _input: &Value, _tools: &[String], _desc: &str, _timeout: Option<u64>,
            ) -> Result<StepExecutionResult, StepExecutionError> {
                if step_id == "step1" {
                    let mut attempt = self.attempt.lock().unwrap();
                    *attempt += 1;
                    if *attempt < 2 {
                        return Err(StepExecutionError {
                            code: "transient".into(),
                            message: "temporary failure".into(),
                            retryable: true,
                        });
                    }
                }
                Ok(StepExecutionResult {
                    output: json!({"ok": true}),
                    message_seq: 1,
                })
            }
        }

        let executor = RetryExecutor {
            attempt: Mutex::new(0),
        };

        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = run_pipeline(
            &ports,
            StartPipelineParams {
                pipeline_name: "test-two-step".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        assert_eq!(result.step_count, 2);
    }

    #[tokio::test]
    async fn checkpointing_creates_events() {
        let store = MockPipelineStore {
            defs: vec![two_step_pipeline()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("step1", json!({"a": 1})),
            ("step2", json!({"b": 2})),
        ]);

        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = run_pipeline(
            &ports,
            StartPipelineParams {
                pipeline_name: "test-two-step".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        // Should have multiple checkpoint events
        assert!(run_store.event_count() >= 3); // initial + after step1 + after step2 + final

        // Last checkpoint should have completed state
        let last = run_store.last_checkpoint().unwrap();
        assert_eq!(last.state, PipelineState::Completed);
        assert_eq!(last.pipeline_name, "test-two-step");
    }

    #[tokio::test]
    async fn run_record_created_and_finalized() {
        let store = MockPipelineStore {
            defs: vec![two_step_pipeline()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("step1", json!({"a": 1})),
            ("step2", json!({"b": 2})),
        ]);

        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = run_pipeline(
            &ports,
            StartPipelineParams {
                pipeline_name: "test-two-step".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        let run = run_store.get_run(&result.run_id).await.unwrap();
        assert_eq!(run.origin, RunOrigin::Pipeline);
        assert_eq!(run.state, RunState::Completed);
        assert!(run.finished_at.is_some());
        assert_eq!(
            run.conversation_key,
            Some("pipeline:test-two-step".into())
        );
    }

    #[tokio::test]
    async fn conditional_branch_routes_correctly() {
        let pipeline = PipelineDefinition {
            name: "cond-test".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![
                PipelineStep {
                    id: "review".into(),
                    agent_id: "reviewer".into(),
                    description: "".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Complex(Box::new(
                        ComplexTransition::Conditional {
                            branches: vec![ConditionalBranch {
                                field: "/approved".into(),
                                operator: Operator::Eq,
                                value: json!(true),
                                target: "publish".into(),
                            }],
                            fallback: "revise".into(),
                        },
                    )),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "publish".into(),
                    agent_id: "publisher".into(),
                    description: "".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("end".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "revise".into(),
                    agent_id: "writer".into(),
                    description: "".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("end".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
            ],
            entry_point: "review".into(),
            max_depth: 5,
            timeout_secs: None,
        };

        // Test approved path
        let store = MockPipelineStore {
            defs: vec![pipeline.clone()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("review", json!({"approved": true, "feedback": "good"})),
            ("publish", json!({"published": true})),
        ]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: &store,
                run_store: &run_store,
                executor: &executor,
            },
            StartPipelineParams {
                pipeline_name: "cond-test".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        assert!(result.data.get("publish").is_some());
        assert!(result.data.get("revise").is_none()); // did NOT go to revise

        // Test denied path
        let store2 = MockPipelineStore {
            defs: vec![pipeline],
        };
        let run_store2 = MockRunStore::new();
        let executor2 = MockExecutor::succeeds(vec![
            ("review", json!({"approved": false, "feedback": "needs work"})),
            ("revise", json!({"revised": true})),
        ]);

        let result2 = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: &store2,
                run_store: &run_store2,
                executor: &executor2,
            },
            StartPipelineParams {
                pipeline_name: "cond-test".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result2.state, PipelineState::Completed);
        assert!(result2.data.get("revise").is_some());
        assert!(result2.data.get("publish").is_none()); // did NOT go to publish
    }

    #[tokio::test]
    async fn input_schema_validation_rejects_bad_input() {
        let pipeline = PipelineDefinition {
            name: "schema-test".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![PipelineStep {
                id: "step1".into(),
                agent_id: "agent-a".into(),
                description: "".into(),
                tools: vec![],
                input_schema: Some(json!({
                    "type": "object",
                    "required": ["topic"]
                })),
                output_schema: None,
                next: StepTransition::Next("end".into()),
                max_retries: 0,
                retry_backoff_secs: 1,
                timeout_secs: None,
            }],
            entry_point: "step1".into(),
            max_depth: 5,
            timeout_secs: None,
        };

        let store = MockPipelineStore {
            defs: vec![pipeline],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![("step1", json!({"result": "ok"}))]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: &store,
                run_store: &run_store,
                executor: &executor,
            },
            StartPipelineParams {
                pipeline_name: "schema-test".into(),
                input: json!({}), // missing "topic"
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Failed);
        assert!(result.error.unwrap().contains("input validation failed"));
    }
}
