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
use std::sync::Arc;
use tracing::{error, info, warn};

/// Ports required by the pipeline runner.
pub struct PipelineRunnerPorts {
    pub pipeline_store: Arc<dyn PipelineStorePort>,
    pub run_store: Arc<dyn RunStorePort>,
    pub executor: Arc<dyn PipelineExecutorPort>,
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
///
/// Uses `Box::pin` internally to support recursive sub-pipeline calls.
pub fn run_pipeline(
    ports: &PipelineRunnerPorts,
    params: StartPipelineParams,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = PipelineRunResult> + Send + '_>> {
    Box::pin(run_pipeline_inner(ports, params))
}

async fn run_pipeline_inner(
    ports: &PipelineRunnerPorts,
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
    if let Err(e) = create_pipeline_run(ports.run_store.as_ref(), &ctx).await {
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
    checkpoint(ports.run_store.as_ref(), &ctx).await;

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
    checkpoint(ports.run_store.as_ref(), &ctx).await;

    // Update Run record to terminal state
    let run_state = match ctx.state {
        PipelineState::Completed => RunState::Completed,
        PipelineState::Cancelled => RunState::Cancelled,
        PipelineState::TimedOut => RunState::Failed,
        _ => RunState::Failed,
    };
    let finished_at = Some(chrono::Utc::now().timestamp().max(0) as u64);
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

/// Resume a pipeline from a recovered checkpoint.
///
/// Phase 4.1 Slice 5: called by `resume_pipeline` use case after daemon restart.
/// Takes an existing `PipelineContext` (deserialized from last checkpoint)
/// and continues execution from `ctx.current_step`.
pub async fn resume_pipeline(
    ports: &PipelineRunnerPorts,
    mut ctx: PipelineContext,
    definition: &PipelineDefinition,
) -> PipelineRunResult {
    info!(
        run_id = %ctx.run_id,
        pipeline = %ctx.pipeline_name,
        current_step = %ctx.current_step,
        checkpoint_state = %ctx.state,
        "resuming pipeline from checkpoint"
    );

    // Reset state to Running (was WaitingForAgent/WaitingForFanOut at crash time)
    ctx.state = PipelineState::Running;

    // Use accumulated data as "initial input" for the resumed execution
    let input = ctx.data.clone();

    let result = execute_loop(ports, definition, &mut ctx, &input).await;

    // Final checkpoint + Run state update
    checkpoint(ports.run_store.as_ref(), &ctx).await;

    let run_state = match ctx.state {
        PipelineState::Completed => RunState::Completed,
        PipelineState::Cancelled => RunState::Cancelled,
        PipelineState::TimedOut => RunState::Failed,
        _ => RunState::Failed,
    };
    let finished_at = Some(chrono::Utc::now().timestamp().max(0) as u64);
    if let Err(e) = ports
        .run_store
        .update_state(&ctx.run_id, run_state, finished_at)
        .await
    {
        warn!(run_id = %ctx.run_id, error = %e, "failed to update run state after resume");
    }

    info!(
        run_id = %ctx.run_id,
        pipeline = %ctx.pipeline_name,
        state = %ctx.state,
        steps = ctx.completed_step_count(),
        "resumed pipeline finished"
    );

    result
}

/// Inner execution loop — iterates through steps until End or failure.
async fn execute_loop(
    ports: &PipelineRunnerPorts,
    definition: &PipelineDefinition,
    ctx: &mut PipelineContext,
    initial_input: &Value,
) -> PipelineRunResult {
    // Start from ctx.current_step (resume) or entry_point (fresh run).
    // PipelineContext is initialized with entry_point in new(), so this
    // is always correct for both fresh and resumed runs.
    let mut current_step_id = ctx.current_step.clone();

    // Global timeout: explicit or safety-net default (2 hours).
    const DEFAULT_PIPELINE_TIMEOUT_SECS: u64 = 7200;
    let timeout = definition.timeout_secs.unwrap_or(DEFAULT_PIPELINE_TIMEOUT_SECS);
    let deadline = ctx.started_at + timeout as i64;

    loop {
        // Cancellation check: read run state from store
        if let Some(run) = ports.run_store.get_run(&ctx.run_id).await {
            if run.state == RunState::Cancelled {
                ctx.state = PipelineState::Cancelled;
                ctx.error = Some("pipeline cancelled by operator".into());
                info!(run_id = %ctx.run_id, "pipeline cancelled");
                return make_result(ctx);
            }
        }

        // Global timeout check
        if chrono::Utc::now().timestamp() > deadline {
            ctx.state = PipelineState::TimedOut;
            ctx.error = Some(format!(
                "pipeline global timeout exceeded ({timeout}s)"
            ));
            return make_result(ctx);
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

        // Skip execution for pseudo-steps (fan-out orchestration nodes).
        // They exist only for their transition; the actual work happens in branches.
        let is_pseudo_step = step.agent_id.starts_with('_');

        let step_output = if is_pseudo_step {
            step_input.clone()
        } else {
            // Execute the step (with retries)
            match execute_step_with_retries(ports, ctx, step, &step_input).await {
                Some(output) => output,
                None => {
                    // ctx.state and error already set by execute_step_with_retries
                    return make_result(ctx);
                }
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
        checkpoint(ports.run_store.as_ref(), ctx).await;

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
                let ct = complex.as_ref();

                if !ct.conditional.is_empty() {
                    // -- Conditional --
                    let fallback = ct.fallback.as_deref().unwrap_or("end");
                    let target = ct.conditional
                        .iter()
                        .find(|b| b.evaluate(&step_output))
                        .map(|b| b.target.as_str())
                        .unwrap_or(fallback);

                    if target == "end" {
                        ctx.complete();
                        return make_result(ctx);
                    }
                    current_step_id = target.to_string();
                } else if let Some(ref spec) = ct.fan_out {
                    // -- FanOut --
                    match execute_fan_out(ports, ctx, definition, spec).await {
                        Ok(join_step) => {
                            if join_step == "end" {
                                ctx.complete();
                                return make_result(ctx);
                            }
                            current_step_id = join_step;
                        }
                        Err(error) => {
                            ctx.fail(error);
                            return make_result(ctx);
                        }
                    }
                } else if let Some(ref wfa) = ct.wait_for_approval {
                    // -- WaitForApproval --
                    info!(
                        run_id = %ctx.run_id,
                        step = %current_step_id,
                        "pipeline waiting for approval"
                    );
                    ctx.state = PipelineState::WaitingForApproval(current_step_id.clone());
                    checkpoint(ports.run_store.as_ref(), ctx).await;

                    let approved = ports
                        .executor
                        .execute_step(
                            &ctx.run_id,
                            &format!("{current_step_id}__approval"),
                            "_approval_gate",
                            &serde_json::json!({
                                "prompt": &wfa.prompt,
                                "step_id": current_step_id,
                                "pipeline": ctx.pipeline_name,
                            }),
                            &[],
                            &wfa.prompt,
                            Some(wfa.timeout_secs),
                        )
                        .await;

                    ctx.state = PipelineState::Running;

                    match approved {
                        Ok(result) => {
                            let is_approved = result.output.get("approved")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            info!(
                                run_id = %ctx.run_id,
                                step = %current_step_id,
                                approved = is_approved,
                                "approval received"
                            );

                            let target = if is_approved { &wfa.next_approved } else { &wfa.next_denied };
                            if target == "end" {
                                ctx.complete();
                                return make_result(ctx);
                            }
                            current_step_id = target.clone();
                        }
                        Err(err) => {
                            ctx.fail(format!(
                                "approval request failed for step '{}': {}",
                                current_step_id, err.message
                            ));
                            return make_result(ctx);
                        }
                    }
                } else if let Some(ref sp) = ct.sub_pipeline {
                    // -- SubPipeline --
                    let pipeline_name = &sp.pipeline_name;
                    let next = &sp.next;
                        // Check depth limit
                        if ctx.depth >= definition.max_depth {
                            ctx.fail(format!(
                                "sub-pipeline depth limit exceeded ({}) at step '{}'",
                                definition.max_depth, current_step_id
                            ));
                            return make_result(ctx);
                        }

                        info!(
                            run_id = %ctx.run_id,
                            step = %current_step_id,
                            sub_pipeline = %pipeline_name,
                            depth = ctx.depth + 1,
                            "starting sub-pipeline"
                        );

                        // Run sub-pipeline with incremented depth
                        let sub_result = run_pipeline(
                            ports,
                            StartPipelineParams {
                                pipeline_name: pipeline_name.clone(),
                                input: step_output.clone(),
                                triggered_by: format!(
                                    "pipeline:{}:{}",
                                    ctx.pipeline_name, current_step_id
                                ),
                                depth: ctx.depth + 1,
                                parent_run_id: Some(ctx.run_id.clone()),
                            },
                        )
                        .await;

                        if sub_result.state != PipelineState::Completed {
                            ctx.fail(format!(
                                "sub-pipeline '{}' failed: {}",
                                pipeline_name,
                                sub_result.error.unwrap_or_else(|| "unknown".into())
                            ));
                            return make_result(ctx);
                        }

                        // Merge sub-pipeline output into context
                        ctx.merge_step_output(
                            &format!("sub:{pipeline_name}"),
                            sub_result.data,
                        );
                        checkpoint(ports.run_store.as_ref(), ctx).await;

                        if next == "end" {
                            ctx.complete();
                            return make_result(ctx);
                        }
                        current_step_id = next.clone();
                    } else {
                        ctx.fail(format!(
                            "step '{}' has empty complex transition",
                            current_step_id
                        ));
                        return make_result(ctx);
                    }
                }
            }
        }
    }

/// Execute a step with retry logic.
/// Returns `Some(output)` on success, `None` on failure (ctx.state already set).
async fn execute_step_with_retries(
    ports: &PipelineRunnerPorts,
    ctx: &mut PipelineContext,
    step: &PipelineStep,
    input: &Value,
) -> Option<Value> {
    let max_attempts = step.max_retries as u32 + 1; // +1 for initial attempt

    for attempt in 0..max_attempts {
        ctx.record_step_start(&step.id, &step.agent_id, attempt as u8);
        ctx.state = PipelineState::WaitingForAgent(step.agent_id.clone());

        // Checkpoint before dispatch (so recovery knows we're waiting)
        checkpoint(ports.run_store.as_ref(), ctx).await;

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

/// Execute a FanOut transition: dispatch all branches concurrently, wait for
/// all (or partial) results, merge into context, return the join_step ID.
///
/// Phase 4.1 Slice 4.
async fn execute_fan_out(
    ports: &PipelineRunnerPorts,
    ctx: &mut PipelineContext,
    definition: &PipelineDefinition,
    spec: &crate::domain::pipeline::FanOutSpec,
) -> Result<String, String> {
    let branch_count = spec.branches.len();
    info!(
        run_id = %ctx.run_id,
        branches = branch_count,
        join_step = %spec.join_step,
        "fan-out started"
    );

    // Track which branches we're waiting for
    let branch_ids: Vec<String> = spec
        .branches
        .iter()
        .map(|b| b.step_id.clone())
        .collect();
    ctx.state = PipelineState::WaitingForFanOut(branch_ids);
    checkpoint(ports.run_store.as_ref(), ctx).await;

    // Build fan-out timeout
    let fan_out_deadline = spec.timeout_secs.map(|t| {
        std::time::Instant::now() + std::time::Duration::from_secs(t)
    });

    // Input for all branches: current accumulated context data
    let branch_input = ctx.data.clone();

    // Validate all branch steps exist before dispatching any
    for branch in &spec.branches {
        if definition.step(&branch.step_id).is_none() {
            return Err(format!(
                "fan-out branch step '{}' not found",
                branch.step_id
            ));
        }
    }

    // Dispatch all branches concurrently via JoinSet.
    let mut join_set = tokio::task::JoinSet::new();

    for branch in &spec.branches {
        let step = definition.step(&branch.step_id).unwrap().clone();
        let result_key = branch.result_key.clone();
        let run_id = ctx.run_id.clone();
        let input = branch_input.clone();
        let executor = ports.executor.clone();

        join_set.spawn(async move {
            let exec_result = executor
                .execute_step(
                    &run_id,
                    &step.id,
                    &step.agent_id,
                    &input,
                    &step.tools,
                    &step.description,
                    step.timeout_secs,
                )
                .await;
            (result_key, step.id.clone(), step.agent_id.clone(), exec_result)
        });
    }

    // Collect results (with optional fan-out timeout)
    let mut results: Vec<(String, Result<Value, String>)> = Vec::new();

    while let Some(join_result) = if let Some(dl) = fan_out_deadline {
        let remaining = dl.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remaining, join_set.join_next()).await {
            Ok(r) => r,
            Err(_) => {
                // Timeout — abort remaining branches
                join_set.abort_all();
                return Err(format!(
                    "fan-out timed out after {} of {} branches",
                    results.len(), branch_count
                ));
            }
        }
    } else {
        join_set.join_next().await
    } {
        match join_result {
            Ok((result_key, step_id, agent_id, exec_result)) => {
                match exec_result {
                    Ok(result) => {
                        ctx.record_step_start(&step_id, &agent_id, 0);
                        ctx.record_step_complete(&step_id, Some(result.output.clone()));
                        info!(
                            run_id = %ctx.run_id,
                            branch = %result_key,
                            step = %step_id,
                            "fan-out branch completed"
                        );
                        results.push((result_key, Ok(result.output)));
                    }
                    Err(err) => {
                        ctx.record_step_start(&step_id, &agent_id, 0);
                        ctx.record_step_failure(&step_id, err.message.clone());
                        warn!(
                            run_id = %ctx.run_id,
                            branch = %result_key,
                            step = %step_id,
                            error = %err.message,
                            "fan-out branch failed"
                        );
                        results.push((result_key, Err(err.message)));
                    }
                }
            }
            Err(join_err) => {
                // JoinError — task panicked
                warn!(run_id = %ctx.run_id, error = %join_err, "fan-out branch task panicked");
                results.push(("_panic".into(), Err(join_err.to_string())));
            }
        }
    }

    // Check results
    let failures: Vec<&str> = results
        .iter()
        .filter_map(|(key, r)| if r.is_err() { Some(key.as_str()) } else { None })
        .collect();

    if spec.require_all && !failures.is_empty() {
        return Err(format!(
            "fan-out failed: branches [{}] failed (require_all=true)",
            failures.join(", ")
        ));
    }

    // Merge results into context under fanout.<key>
    for (key, result) in &results {
        if let Ok(output) = result {
            ctx.merge_fanout_output(key, output.clone());
        }
    }

    ctx.state = PipelineState::Running;
    checkpoint(ports.run_store.as_ref(), ctx).await;

    info!(
        run_id = %ctx.run_id,
        completed = results.iter().filter(|(_, r)| r.is_ok()).count(),
        failed = failures.len(),
        "fan-out joined"
    );

    Ok(spec.join_step.clone())
}

/// Determine what input to pass to a step.
///
/// Strategy:
/// - Entry point: receives the initial pipeline input.
/// - After fan-out join: receives the full accumulated data (including
///   `fanout.*` namespace with merged branch results).
/// - Normal step: receives the previous step's output.
/// - Fallback: full accumulated data.
fn resolve_step_input(
    ctx: &PipelineContext,
    step_id: &str,
    initial_input: &Value,
    definition: &PipelineDefinition,
) -> Value {
    // If this is the entry point (or current_step on fresh run), use initial input
    if step_id == definition.entry_point {
        return initial_input.clone();
    }

    // If this step is a fan-out join target, pass full accumulated data
    // (including fanout.* namespace). Check by looking for a FanOut
    // transition in any step that references this step_id as join_step.
    let is_join_step = definition.steps.iter().any(|s| {
        if let StepTransition::Complex(ref c) = s.next {
            if let Some(ref spec) = c.fan_out {
                return spec.join_step == step_id;
            }
        }
        false
    });
    if is_join_step {
        return ctx.data.clone();
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
        created_at: chrono::Utc::now().timestamp().max(0) as u64,
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
    uuid::Uuid::new_v4().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pipeline::{
        ConditionalBranch, ComplexTransition, FanOutSpec, Operator, PipelineDefinition,
        PipelineStep, StepTransition, SubPipelineSpec, WaitForApprovalSpec,
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
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
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
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
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
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
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
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
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
        let run_store = Arc::new(MockRunStore::new());
        let executor = MockExecutor::succeeds(vec![
            ("step1", json!({"a": 1})),
            ("step2", json!({"b": 2})),
        ]);

        let ports = PipelineRunnerPorts {
            pipeline_store: Arc::new(store),
            run_store: run_store.clone(),
            executor: Arc::new(executor),
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
        let run_store = Arc::new(MockRunStore::new());
        let executor = MockExecutor::succeeds(vec![
            ("step1", json!({"a": 1})),
            ("step2", json!({"b": 2})),
        ]);

        let ports = PipelineRunnerPorts {
            pipeline_store: Arc::new(store),
            run_store: run_store.clone(),
            executor: Arc::new(executor),
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
                        ComplexTransition::conditional(
                            vec![ConditionalBranch {
                                field: "/approved".into(),
                                operator: Operator::Eq,
                                value: json!(true),
                                target: "publish".into(),
                            }],
                            "revise".into(),
                        ),
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
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
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
                pipeline_store: Arc::new(store2),
                run_store: Arc::new(run_store2),
                executor: Arc::new(executor2),
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
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
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

    // -- FanOut tests (Slice 4) -----------------------------------------

    fn fan_out_pipeline() -> PipelineDefinition {
        use crate::domain::pipeline::{FanOutBranch, FanOutSpec};

        PipelineDefinition {
            name: "fan-out-test".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![
                PipelineStep {
                    id: "gather".into(),
                    agent_id: "_fanout".into(),
                    description: "fan-out trigger".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Complex(Box::new(
                        ComplexTransition::fan_out(FanOutSpec {
                            branches: vec![
                                FanOutBranch {
                                    step_id: "fetch-news".into(),
                                    result_key: "news".into(),
                                },
                                FanOutBranch {
                                    step_id: "fetch-trends".into(),
                                    result_key: "trends".into(),
                                },
                            ],
                            join_step: "draft".into(),
                            timeout_secs: None,
                            require_all: true,
                        }),
                    )),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "fetch-news".into(),
                    agent_id: "news-reader".into(),
                    description: "Fetch news".into(),
                    tools: vec!["web_search".into()],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("_join".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "fetch-trends".into(),
                    agent_id: "trend-aggregator".into(),
                    description: "Fetch trends".into(),
                    tools: vec!["web_search".into()],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("_join".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "draft".into(),
                    agent_id: "copywriter".into(),
                    description: "Draft from merged data".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Next("end".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
            ],
            entry_point: "gather".into(),
            max_depth: 5,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn fan_out_both_branches_succeed() {
        let store = MockPipelineStore {
            defs: vec![fan_out_pipeline()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("gather", json!({"trigger": true})),
            ("fetch-news", json!({"headlines": ["Rust 2026"]})),
            ("fetch-trends", json!({"top": "AI agents"})),
            ("draft", json!({"post": "content here"})),
        ]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "fan-out-test".into(),
                input: json!({"topic": "tech"}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        // Fan-out results should be under fanout.<key>
        let fanout = result.data.get("fanout").expect("fanout key missing");
        assert_eq!(
            fanout.get("news"),
            Some(&json!({"headlines": ["Rust 2026"]}))
        );
        assert_eq!(
            fanout.get("trends"),
            Some(&json!({"top": "AI agents"}))
        );
        // Draft step should have run after join
        assert!(result.data.get("draft").is_some());
    }

    #[tokio::test]
    async fn fan_out_branch_failure_with_require_all() {
        let store = MockPipelineStore {
            defs: vec![fan_out_pipeline()],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::new(vec![
            ("gather".into(), Ok(json!({"trigger": true}))),
            ("fetch-news".into(), Ok(json!({"headlines": []}))),
            (
                "fetch-trends".into(),
                Err(StepExecutionError {
                    code: "timeout".into(),
                    message: "agent timed out".into(),
                    retryable: false,
                }),
            ),
        ]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "fan-out-test".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Failed);
        assert!(result.error.unwrap().contains("require_all"));
    }

    #[tokio::test]
    async fn fan_out_partial_success_without_require_all() {
        use crate::domain::pipeline::{FanOutBranch, FanOutSpec};

        let mut pipeline = fan_out_pipeline();
        // Change require_all to false
        if let StepTransition::Complex(ref mut complex) = pipeline.steps[0].next {
            if let Some(ref mut spec) = complex.fan_out {
                spec.require_all = false;
            }
        }

        let store = MockPipelineStore {
            defs: vec![pipeline],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::new(vec![
            ("gather".into(), Ok(json!({"trigger": true}))),
            ("fetch-news".into(), Ok(json!({"headlines": ["ok"]}))),
            (
                "fetch-trends".into(),
                Err(StepExecutionError {
                    code: "error".into(),
                    message: "failed".into(),
                    retryable: false,
                }),
            ),
            ("draft".into(), Ok(json!({"post": "partial"}))),
        ]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "fan-out-test".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        // Should complete even with one failed branch
        assert_eq!(result.state, PipelineState::Completed);
        let fanout = result.data.get("fanout").unwrap();
        assert!(fanout.get("news").is_some()); // succeeded
        assert!(fanout.get("trends").is_none()); // failed, not merged
    }

    // -- WaitForApproval tests (Slice 7) ------------------------------------

    fn approval_pipeline(approved: bool) -> (PipelineDefinition, MockExecutor) {
        let pipeline = PipelineDefinition {
            name: "approval-test".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![
                PipelineStep {
                    id: "draft".into(),
                    agent_id: "copywriter".into(),
                    description: "Write draft".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Complex(Box::new(
                        ComplexTransition::wait_for_approval(WaitForApprovalSpec {
                            prompt: "Approve this draft?".into(),
                            next_approved: "publish".into(),
                            next_denied: "revise".into(),
                            timeout_secs: 3600,
                        }),
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
                    agent_id: "copywriter".into(),
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
            entry_point: "draft".into(),
            max_depth: 5,
            timeout_secs: None,
        };

        let executor = MockExecutor::new(vec![
            ("draft".into(), Ok(json!({"text": "my draft"}))),
            (
                "draft__approval".into(),
                Ok(json!({"approved": approved})),
            ),
            ("publish".into(), Ok(json!({"published": true}))),
            ("revise".into(), Ok(json!({"revised": true}))),
        ]);

        (pipeline, executor)
    }

    #[tokio::test]
    async fn wait_for_approval_approved_path() {
        let (pipeline, executor) = approval_pipeline(true);
        let store = MockPipelineStore {
            defs: vec![pipeline],
        };
        let run_store = MockRunStore::new();

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "approval-test".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        assert!(result.data.get("publish").is_some());
        assert!(result.data.get("revise").is_none());
    }

    #[tokio::test]
    async fn wait_for_approval_denied_path() {
        let (pipeline, executor) = approval_pipeline(false);
        let store = MockPipelineStore {
            defs: vec![pipeline],
        };
        let run_store = MockRunStore::new();

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "approval-test".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        assert!(result.data.get("revise").is_some());
        assert!(result.data.get("publish").is_none());
    }

    // -- Nested pipeline tests (Slice 9) ------------------------------------

    #[tokio::test]
    async fn sub_pipeline_executes_and_returns() {
        // Parent: step1 → sub_pipeline("child") → step3
        // Child:  c1 → end
        let parent = PipelineDefinition {
            name: "parent".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![
                PipelineStep {
                    id: "s1".into(),
                    agent_id: "a".into(),
                    description: "".into(),
                    tools: vec![],
                    input_schema: None,
                    output_schema: None,
                    next: StepTransition::Complex(Box::new(
                        ComplexTransition::sub_pipeline(SubPipelineSpec {
                            pipeline_name: "child".into(),
                            next: "s3".into(),
                        }),
                    )),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "s3".into(),
                    agent_id: "c".into(),
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
            entry_point: "s1".into(),
            max_depth: 3,
            timeout_secs: None,
        };

        let child = PipelineDefinition {
            name: "child".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![PipelineStep {
                id: "c1".into(),
                agent_id: "b".into(),
                description: "".into(),
                tools: vec![],
                input_schema: None,
                output_schema: None,
                next: StepTransition::Next("end".into()),
                max_retries: 0,
                retry_backoff_secs: 1,
                timeout_secs: None,
            }],
            entry_point: "c1".into(),
            max_depth: 3,
            timeout_secs: None,
        };

        let store = MockPipelineStore {
            defs: vec![parent, child],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("s1", json!({"parent_step": true})),
            ("c1", json!({"child_result": "done"})),
            ("s3", json!({"final": true})),
        ]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "parent".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Completed);
        assert!(result.data.get("s1").is_some());
        assert!(result.data.get("sub:child").is_some()); // sub-pipeline output
        assert!(result.data.get("s3").is_some());
    }

    #[tokio::test]
    async fn sub_pipeline_depth_limit() {
        // Pipeline calls itself → depth limit should stop recursion
        let recursive = PipelineDefinition {
            name: "recursive".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![PipelineStep {
                id: "s1".into(),
                agent_id: "a".into(),
                description: "".into(),
                tools: vec![],
                input_schema: None,
                output_schema: None,
                next: StepTransition::Complex(Box::new(
                    ComplexTransition::sub_pipeline(SubPipelineSpec {
                        pipeline_name: "recursive".into(),
                        next: "end".into(),
                    }),
                )),
                max_retries: 0,
                retry_backoff_secs: 1,
                timeout_secs: None,
            }],
            entry_point: "s1".into(),
            max_depth: 2,
            timeout_secs: None,
        };

        let store = MockPipelineStore {
            defs: vec![recursive],
        };
        let run_store = MockRunStore::new();
        let executor = MockExecutor::succeeds(vec![
            ("s1", json!({"x": 1})),
        ]);

        let result = run_pipeline(
            &PipelineRunnerPorts {
                pipeline_store: Arc::new(store),
                run_store: Arc::new(run_store),
                executor: Arc::new(executor),
            },
            StartPipelineParams {
                pipeline_name: "recursive".into(),
                input: json!({}),
                triggered_by: "test".into(),
                depth: 0,
                parent_run_id: None,
            },
        )
        .await;

        assert_eq!(result.state, PipelineState::Failed);
        assert!(result.error.unwrap().contains("depth limit"));
    }
}
