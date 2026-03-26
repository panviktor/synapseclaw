//! Use case: CancelPipeline — cancel a running pipeline.
//!
//! Phase 4.1 Slice 5: operator or agent can cancel a pipeline run.

use crate::domain::run::RunState;
use crate::ports::run_store::RunStorePort;
use tracing::{info, warn};

/// Cancel a pipeline run by its run_id.
///
/// Sets the run state to Cancelled. If the pipeline is currently executing
/// a step, the step will complete but no further steps will be dispatched
/// (the pipeline runner checks state before advancing).
pub async fn execute(run_store: &dyn RunStorePort, run_id: &str) -> Result<(), String> {
    let run = run_store
        .get_run(run_id)
        .await
        .ok_or_else(|| format!("run '{run_id}' not found"))?;

    if run.state.is_terminal() {
        return Err(format!(
            "run '{run_id}' is already in terminal state: {}",
            run.state
        ));
    }

    let finished_at = Some(chrono::Utc::now().timestamp() as u64);
    run_store
        .update_state(run_id, RunState::Cancelled, finished_at)
        .await
        .map_err(|e| format!("failed to cancel run: {e}"))?;

    info!(run_id = %run_id, "pipeline cancelled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::run::{Run, RunEvent, RunOrigin, RunState};
    use crate::ports::run_store::RunStorePort;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockRunStore {
        runs: Mutex<Vec<Run>>,
    }

    impl MockRunStore {
        fn with_run(run_id: &str, state: RunState) -> Self {
            Self {
                runs: Mutex::new(vec![Run {
                    run_id: run_id.into(),
                    conversation_key: None,
                    origin: RunOrigin::Pipeline,
                    state,
                    started_at: 1000,
                    finished_at: None,
                }]),
            }
        }
    }

    #[async_trait]
    impl RunStorePort for MockRunStore {
        async fn create_run(&self, _run: &Run) -> anyhow::Result<()> { Ok(()) }
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
        async fn list_runs(&self, _k: &str, _l: usize) -> Vec<Run> { vec![] }
        async fn list_all_runs(&self, _l: usize) -> Vec<Run> { vec![] }
        async fn append_event(&self, _e: &RunEvent) -> anyhow::Result<()> { Ok(()) }
        async fn get_events(&self, _id: &str, _l: usize) -> Vec<RunEvent> { vec![] }
    }

    #[tokio::test]
    async fn cancel_running_pipeline() {
        let store = MockRunStore::with_run("run-1", RunState::Running);
        assert!(execute(&store, "run-1").await.is_ok());
        let run = store.get_run("run-1").await.unwrap();
        assert_eq!(run.state, RunState::Cancelled);
        assert!(run.finished_at.is_some());
    }

    #[tokio::test]
    async fn cancel_already_completed_fails() {
        let store = MockRunStore::with_run("run-2", RunState::Completed);
        let err = execute(&store, "run-2").await.unwrap_err();
        assert!(err.contains("terminal"));
    }

    #[tokio::test]
    async fn cancel_nonexistent_fails() {
        let store = MockRunStore::with_run("run-1", RunState::Running);
        let err = execute(&store, "run-999").await.unwrap_err();
        assert!(err.contains("not found"));
    }
}
