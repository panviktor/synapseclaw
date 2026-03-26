//! Use case: StartPipeline — trigger a pipeline run.
//!
//! Phase 4.1 Slice 2: top-level entry point for pipeline execution.
//!
//! Orchestrates: validate params → resolve pipeline → delegate to pipeline_service.

use crate::application::services::pipeline_service::{
    self, PipelineRunResult, PipelineRunnerPorts, StartPipelineParams,
};
use crate::domain::pipeline_context::PipelineState;
use serde_json::Value;

/// Parameters for the start_pipeline use case.
pub struct StartPipelineInput {
    /// Pipeline name to execute.
    pub pipeline_name: String,
    /// Initial input data for the first step.
    pub input: Value,
    /// Who/what triggered this run (agent_id, "operator", "cron:job_name", etc.).
    pub triggered_by: String,
}

/// Error from starting a pipeline.
#[derive(Debug)]
pub struct StartPipelineError {
    pub message: String,
}

impl std::fmt::Display for StartPipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for StartPipelineError {}

/// Start a pipeline run.
///
/// This is the primary entry point for triggering pipelines from:
/// - IPC commands (agent triggers pipeline)
/// - Web dashboard
/// - Cron jobs
/// - Other pipelines (nested, handled in Slice 9)
pub async fn execute(
    ports: &PipelineRunnerPorts<'_>,
    input: StartPipelineInput,
) -> Result<PipelineRunResult, StartPipelineError> {
    // Validate pipeline exists
    if ports.pipeline_store.get(&input.pipeline_name).await.is_none() {
        return Err(StartPipelineError {
            message: format!("pipeline '{}' not found", input.pipeline_name),
        });
    }

    let result = pipeline_service::run_pipeline(
        ports,
        StartPipelineParams {
            pipeline_name: input.pipeline_name,
            input: input.input,
            triggered_by: input.triggered_by,
            depth: 0,
            parent_run_id: None,
        },
    )
    .await;

    if result.state == PipelineState::Failed {
        // Still return Ok — the pipeline ran but failed.
        // StartPipelineError is for cases where we can't even start.
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pipeline::{PipelineDefinition, PipelineStep, StepTransition};
    use crate::ports::pipeline_executor::{
        PipelineExecutorPort, StepExecutionError, StepExecutionResult,
    };
    use crate::ports::pipeline_store::{PipelineStorePort, ReloadEvent};
    use crate::ports::run_store::RunStorePort;
    use crate::domain::run::{Run, RunEvent, RunState};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

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

    struct MockRunStore;

    #[async_trait]
    impl RunStorePort for MockRunStore {
        async fn create_run(&self, _run: &Run) -> anyhow::Result<()> { Ok(()) }
        async fn get_run(&self, _run_id: &str) -> Option<Run> { None }
        async fn update_state(&self, _id: &str, _s: RunState, _f: Option<u64>) -> anyhow::Result<()> { Ok(()) }
        async fn list_runs(&self, _k: &str, _l: usize) -> Vec<Run> { vec![] }
        async fn list_all_runs(&self, _l: usize) -> Vec<Run> { vec![] }
        async fn append_event(&self, _e: &RunEvent) -> anyhow::Result<()> { Ok(()) }
        async fn get_events(&self, _id: &str, _l: usize) -> Vec<RunEvent> { vec![] }
    }

    struct OkExecutor;

    #[async_trait]
    impl PipelineExecutorPort for OkExecutor {
        async fn execute_step(
            &self, _run_id: &str, _step_id: &str, _agent_id: &str,
            _input: &Value, _tools: &[String], _desc: &str, _timeout: Option<u64>,
        ) -> Result<StepExecutionResult, StepExecutionError> {
            Ok(StepExecutionResult {
                output: json!({"ok": true}),
                message_seq: 1,
            })
        }
    }

    fn one_step_pipeline() -> PipelineDefinition {
        PipelineDefinition {
            name: "one-step".into(),
            version: "1.0".into(),
            description: "".into(),
            steps: vec![PipelineStep {
                id: "s1".into(),
                agent_id: "a".into(),
                description: "".into(),
                tools: vec![],
                input_schema: None,
                output_schema: None,
                next: StepTransition::Next("end".into()),
                max_retries: 0,
                retry_backoff_secs: 1,
                timeout_secs: None,
            }],
            entry_point: "s1".into(),
            max_depth: 5,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn start_nonexistent_pipeline_errors() {
        let store = MockStore { defs: vec![] };
        let run_store = MockRunStore;
        let executor = OkExecutor;
        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let err = execute(
            &ports,
            StartPipelineInput {
                pipeline_name: "nope".into(),
                input: json!({}),
                triggered_by: "test".into(),
            },
        )
        .await
        .unwrap_err();

        assert!(err.message.contains("not found"));
    }

    #[tokio::test]
    async fn start_existing_pipeline_succeeds() {
        let store = MockStore {
            defs: vec![one_step_pipeline()],
        };
        let run_store = MockRunStore;
        let executor = OkExecutor;
        let ports = PipelineRunnerPorts {
            pipeline_store: &store,
            run_store: &run_store,
            executor: &executor,
        };

        let result = execute(
            &ports,
            StartPipelineInput {
                pipeline_name: "one-step".into(),
                input: json!({"topic": "test"}),
                triggered_by: "test".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.state, PipelineState::Completed);
        assert_eq!(result.step_count, 1);
    }
}
