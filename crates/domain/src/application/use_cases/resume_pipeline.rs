//! Use case: ResumePipeline — recover incomplete pipeline runs after restart.
//!
//! Phase 4.1 Slice 5: on daemon startup, find all pipeline runs in
//! non-terminal state, deserialize their last checkpoint, and resume.

use crate::application::services::pipeline_service::{
    self, PipelineRunResult, PipelineRunnerPorts,
};
use crate::domain::pipeline_context::PipelineContext;
use crate::domain::run::{RunOrigin, RunState};
use tracing::{info, warn};

/// Result of recovering pipeline runs on startup.
#[derive(Debug)]
pub struct RecoveryReport {
    /// Number of incomplete runs found.
    pub found: usize,
    /// Number successfully resumed and completed.
    pub resumed: usize,
    /// Number that failed during resume.
    pub failed: usize,
    /// Number skipped (stale definition, no checkpoint, etc.).
    pub skipped: usize,
    /// Details per run.
    pub details: Vec<RecoveryDetail>,
}

/// Recovery outcome for a single run.
#[derive(Debug)]
pub struct RecoveryDetail {
    pub run_id: String,
    pub pipeline_name: String,
    pub outcome: RecoveryOutcome,
}

/// What happened to a recovered run.
#[derive(Debug)]
pub enum RecoveryOutcome {
    Resumed(PipelineRunResult),
    Skipped(String),
    Failed(String),
}

/// Recover all incomplete pipeline runs.
///
/// Called at daemon startup. Steps:
/// 1. Query RunStorePort for runs with origin=Pipeline in non-terminal state
/// 2. For each: load last checkpoint (PipelineContext from RunEvent)
/// 3. Verify pipeline definition still exists (may have been removed)
/// 4. Resume execution from the current step
pub async fn recover_all(ports: &PipelineRunnerPorts) -> RecoveryReport {
    // Find incomplete pipeline runs
    let non_terminal = &[RunState::Running, RunState::Queued];
    let runs = ports.run_store.list_by_state(non_terminal, 100).await;

    // Filter to pipeline-origin runs only
    let pipeline_runs: Vec<_> = runs
        .into_iter()
        .filter(|r| r.origin == RunOrigin::Pipeline)
        .collect();

    let found = pipeline_runs.len();
    if found == 0 {
        return RecoveryReport {
            found: 0,
            resumed: 0,
            failed: 0,
            skipped: 0,
            details: vec![],
        };
    }

    info!(count = found, "found incomplete pipeline runs to recover");

    let mut resumed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut details = Vec::new();

    for run in &pipeline_runs {
        let detail = recover_one(ports, &run.run_id).await;
        match &detail.outcome {
            RecoveryOutcome::Resumed(_) => resumed += 1,
            RecoveryOutcome::Skipped(_) => skipped += 1,
            RecoveryOutcome::Failed(_) => failed += 1,
        }
        details.push(detail);
    }

    info!(
        found = found,
        resumed = resumed,
        failed = failed,
        skipped = skipped,
        "pipeline recovery complete"
    );

    RecoveryReport {
        found,
        resumed,
        failed,
        skipped,
        details,
    }
}

/// Recover a single pipeline run.
async fn recover_one(ports: &PipelineRunnerPorts, run_id: &str) -> RecoveryDetail {
    // Load last checkpoint from run events
    let events = ports.run_store.get_events(run_id, 1000).await;
    let last_checkpoint = events
        .iter()
        .rev()
        .find(|e| e.tool_name.as_deref() == Some("pipeline_checkpoint"))
        .and_then(|e| {
            serde_json::from_str::<PipelineContext>(&e.content)
                .map_err(|err| {
                    warn!(
                        run_id = %run_id,
                        error = %err,
                        "checkpoint deserialization failed"
                    );
                })
                .ok()
        });

    let ctx = match last_checkpoint {
        Some(c) => c,
        None => {
            warn!(run_id = %run_id, "no checkpoint found, skipping");
            // Mark as failed — can't resume without checkpoint
            let _ = ports
                .run_store
                .update_state(run_id, RunState::Failed, Some(now_secs()))
                .await;
            return RecoveryDetail {
                run_id: run_id.into(),
                pipeline_name: "unknown".into(),
                outcome: RecoveryOutcome::Skipped("no checkpoint found".into()),
            };
        }
    };

    let pipeline_name = ctx.pipeline_name.clone();

    // Check if pipeline definition still exists
    let definition = match ports.pipeline_store.get(&ctx.pipeline_name).await {
        Some(def) => def,
        None => {
            warn!(
                run_id = %run_id,
                pipeline = %ctx.pipeline_name,
                "pipeline definition removed, cannot resume"
            );
            let _ = ports
                .run_store
                .update_state(run_id, RunState::Failed, Some(now_secs()))
                .await;
            return RecoveryDetail {
                run_id: run_id.into(),
                pipeline_name,
                outcome: RecoveryOutcome::Skipped("pipeline definition removed".into()),
            };
        }
    };

    // Refuse to resume if definition version changed — structural changes
    // (removed/renamed steps, changed transitions) would cause confusing failures.
    // Operator should re-run the pipeline manually on the new version.
    if definition.version != ctx.pipeline_version {
        warn!(
            run_id = %run_id,
            pipeline = %ctx.pipeline_name,
            checkpoint_version = %ctx.pipeline_version,
            current_version = %definition.version,
            "pipeline definition changed, refusing to resume"
        );
        let _ = ports
            .run_store
            .update_state(run_id, RunState::Failed, Some(now_secs()))
            .await;
        return RecoveryDetail {
            run_id: run_id.into(),
            pipeline_name,
            outcome: RecoveryOutcome::Skipped(format!(
                "definition version changed: {} -> {} (re-run manually)",
                ctx.pipeline_version, definition.version
            )),
        };
    }

    // Check that current step still exists in definition
    if definition.step(&ctx.current_step).is_none() {
        let msg = format!(
            "current step '{}' no longer exists in pipeline definition",
            ctx.current_step
        );
        warn!(run_id = %run_id, "{msg}");
        let _ = ports
            .run_store
            .update_state(run_id, RunState::Failed, Some(now_secs()))
            .await;
        return RecoveryDetail {
            run_id: run_id.into(),
            pipeline_name,
            outcome: RecoveryOutcome::Failed(msg),
        };
    }

    info!(
        run_id = %run_id,
        pipeline = %ctx.pipeline_name,
        current_step = %ctx.current_step,
        state = %ctx.state,
        "resuming pipeline"
    );

    // Resume: re-run pipeline from checkpoint
    let result = pipeline_service::resume_pipeline(ports, ctx, &definition).await;

    RecoveryDetail {
        run_id: run_id.into(),
        pipeline_name,
        outcome: RecoveryOutcome::Resumed(result),
    }
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pipeline::{PipelineDefinition, PipelineStep, StepTransition};
    use crate::domain::pipeline_context::{PipelineContext, PipelineState};
    use crate::domain::run::{Run, RunEvent, RunEventType, RunOrigin, RunState};
    use crate::ports::pipeline_executor::{
        PipelineExecutorPort, StepExecutionError, StepExecutionResult,
    };
    use crate::ports::pipeline_store::{PipelineStorePort, ReloadEvent};
    use crate::ports::run_store::RunStorePort;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    // -- Mocks (reusable) ---------------------------------------------------

    struct MockStore {
        defs: Vec<PipelineDefinition>,
    }

    #[async_trait]
    impl PipelineStorePort for MockStore {
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

        fn with_pipeline_run(run_id: &str, ctx: &PipelineContext) -> Self {
            let store = Self::new();
            store.runs.lock().unwrap().push(Run {
                run_id: run_id.into(),
                conversation_key: Some(format!("pipeline:{}", ctx.pipeline_name)),
                origin: RunOrigin::Pipeline,
                state: RunState::Running,
                started_at: ctx.started_at as u64,
                finished_at: None,
            });
            store.events.lock().unwrap().push(RunEvent {
                run_id: run_id.into(),
                event_type: RunEventType::Progress,
                content: serde_json::to_string(ctx).unwrap(),
                tool_name: Some("pipeline_checkpoint".into()),
                created_at: ctx.updated_at as u64,
            });
            store
        }
    }

    #[async_trait]
    impl RunStorePort for MockRunStore {
        async fn create_run(&self, run: &Run) -> anyhow::Result<()> {
            self.runs.lock().unwrap().push(run.clone());
            Ok(())
        }
        async fn get_run(&self, run_id: &str) -> Option<Run> {
            self.runs
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.run_id == run_id)
                .cloned()
        }
        async fn update_state(
            &self,
            run_id: &str,
            state: RunState,
            finished_at: Option<u64>,
        ) -> anyhow::Result<()> {
            let mut runs = self.runs.lock().unwrap();
            if let Some(run) = runs.iter_mut().find(|r| r.run_id == run_id) {
                run.state = state;
                run.finished_at = finished_at;
            }
            Ok(())
        }
        async fn list_runs(&self, _k: &str, _l: usize) -> Vec<Run> {
            vec![]
        }
        async fn list_all_runs(&self, _l: usize) -> Vec<Run> {
            self.runs.lock().unwrap().clone()
        }
        async fn append_event(&self, event: &RunEvent) -> anyhow::Result<()> {
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
        async fn get_events(&self, run_id: &str, _limit: usize) -> Vec<RunEvent> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.run_id == run_id)
                .cloned()
                .collect()
        }
    }

    struct OkExecutor;

    #[async_trait]
    impl PipelineExecutorPort for OkExecutor {
        async fn execute_step(
            &self,
            _run_id: &str,
            _step_id: &str,
            _agent_id: &str,
            _input: &Value,
            _tools: &[String],
            _desc: &str,
            _timeout: Option<u64>,
        ) -> Result<StepExecutionResult, StepExecutionError> {
            Ok(StepExecutionResult {
                output: json!({"ok": true}),
                message_seq: 1,
            })
        }
    }

    fn two_step_def() -> PipelineDefinition {
        PipelineDefinition {
            name: "test-pipe".into(),
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
                    next: StepTransition::Next("s2".into()),
                    max_retries: 0,
                    retry_backoff_secs: 1,
                    timeout_secs: None,
                },
                PipelineStep {
                    id: "s2".into(),
                    agent_id: "b".into(),
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
            max_depth: 5,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn recover_no_incomplete_runs() {
        let store = MockStore { defs: vec![] };
        let run_store = MockRunStore::new();
        let executor = OkExecutor;
        let ports = PipelineRunnerPorts {
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
            dead_letter: None,
        };

        let report = recover_all(&ports).await;
        assert_eq!(report.found, 0);
    }

    #[tokio::test]
    async fn recover_resumes_from_step2() {
        // Simulate: pipeline crashed after s1 completed, s2 not started
        let mut ctx = PipelineContext::new(
            "run-crash".into(),
            "test-pipe".into(),
            "1.0".into(),
            "s2".into(), // current_step = s2 (s1 already done)
            "test".into(),
            0,
            None,
        );
        ctx.merge_step_output("s1", json!({"done": true}));
        ctx.state = PipelineState::WaitingForAgent("b".into());

        let store = MockStore {
            defs: vec![two_step_def()],
        };
        let run_store = MockRunStore::with_pipeline_run("run-crash", &ctx);
        let executor = OkExecutor;
        let ports = PipelineRunnerPorts {
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
            dead_letter: None,
        };

        let report = recover_all(&ports).await;
        assert_eq!(report.found, 1);
        assert_eq!(report.resumed, 1);
        assert_eq!(report.failed, 0);

        if let RecoveryOutcome::Resumed(result) = &report.details[0].outcome {
            assert_eq!(result.state, PipelineState::Completed);
            // s1 output preserved from checkpoint
            assert!(result.data.get("s1").is_some());
        } else {
            panic!("expected Resumed");
        }
    }

    #[tokio::test]
    async fn recover_skips_removed_pipeline() {
        let ctx = PipelineContext::new(
            "run-orphan".into(),
            "deleted-pipe".into(),
            "1.0".into(),
            "s1".into(),
            "test".into(),
            0,
            None,
        );

        let store = MockStore { defs: vec![] }; // no definitions
        let run_store = MockRunStore::with_pipeline_run("run-orphan", &ctx);
        let executor = OkExecutor;
        let ports = PipelineRunnerPorts {
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
            dead_letter: None,
        };

        let report = recover_all(&ports).await;
        assert_eq!(report.found, 1);
        assert_eq!(report.skipped, 1);
    }

    #[tokio::test]
    async fn recover_skips_no_checkpoint() {
        // Run exists but no checkpoint events
        let run_store = MockRunStore::new();
        run_store.runs.lock().unwrap().push(Run {
            run_id: "run-nochk".into(),
            conversation_key: Some("pipeline:test".into()),
            origin: RunOrigin::Pipeline,
            state: RunState::Running,
            started_at: 1000,
            finished_at: None,
        });

        let store = MockStore { defs: vec![] };
        let executor = OkExecutor;
        let ports = PipelineRunnerPorts {
            pipeline_store: Arc::new(store),
            run_store: Arc::new(run_store),
            executor: Arc::new(executor),
            dead_letter: None,
        };

        let report = recover_all(&ports).await;
        assert_eq!(report.found, 1);
        assert_eq!(report.skipped, 1);
    }
}
