//! Use case: DelegateImplementationTask — submit work to an external coding worker.
//!
//! Phase 4.0 Slice 7: orchestrates task submission + run tracking.

use crate::domain::implementation::{CodingWorkerResult, ImplementationState, ImplementationTask};
use crate::domain::run::{Run, RunOrigin, RunState};
use crate::ports::coding_worker::CodingWorkerPort;
use crate::ports::run_store::RunStorePort;
use anyhow::Result;

/// Submit an implementation task and track it via RunStorePort.
///
/// Creates a Run record, submits the task to the worker, and links them.
pub async fn execute(
    worker: &dyn CodingWorkerPort,
    run_store: &dyn RunStorePort,
    task: &ImplementationTask,
    conversation_key: Option<&str>,
) -> Result<String> {
    // Create run record
    let run_id = uuid::Uuid::new_v4().to_string();
    let run = Run {
        run_id: run_id.clone(),
        conversation_key: conversation_key.map(String::from),
        origin: RunOrigin::Ipc, // coding workers attach via IPC
        state: RunState::Running,
        started_at: chrono::Utc::now().timestamp() as u64,
        finished_at: None,
    };
    run_store.create_run(&run).await?;

    // Submit to worker
    let worker_run_id = worker.submit_task(task).await?;

    tracing::info!(
        run_id = %run_id,
        worker_run_id = %worker_run_id,
        task_id = %task.task_id,
        "Implementation task delegated to coding worker"
    );

    Ok(run_id)
}

/// Finalize a coding worker run based on the result.
pub async fn finalize(
    run_store: &dyn RunStorePort,
    run_id: &str,
    result: &CodingWorkerResult,
) -> Result<()> {
    let state = match result.state {
        ImplementationState::Completed => RunState::Completed,
        ImplementationState::Failed => RunState::Failed,
        ImplementationState::Cancelled | ImplementationState::Interrupted => RunState::Interrupted,
        _ => return Ok(()), // non-terminal — don't finalize
    };

    let now = chrono::Utc::now().timestamp() as u64;
    run_store.update_state(run_id, state, Some(now)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::implementation::ExpectedOutput;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockWorker;

    #[async_trait]
    impl CodingWorkerPort for MockWorker {
        async fn submit_task(&self, _task: &ImplementationTask) -> Result<String> {
            Ok("worker-run-123".into())
        }
        async fn poll_result(&self, _run_id: &str) -> Result<Option<CodingWorkerResult>> {
            Ok(None)
        }
        async fn get_events(
            &self,
            _run_id: &str,
            _limit: usize,
        ) -> Result<Vec<crate::domain::implementation::ImplementationEvent>> {
            Ok(vec![])
        }
        async fn cancel(&self, _run_id: &str) -> Result<()> {
            Ok(())
        }
    }

    struct MockRunStore {
        runs: Mutex<Vec<Run>>,
    }
    impl MockRunStore {
        fn new() -> Self {
            Self {
                runs: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl RunStorePort for MockRunStore {
        async fn create_run(&self, run: &Run) -> Result<()> {
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
        ) -> Result<()> {
            let mut runs = self.runs.lock()?;
            if let Some(run) = runs.iter_mut().find(|r| r.run_id == run_id) {
                run.state = state;
                run.finished_at = finished_at;
            }
            Ok(())
        }
        async fn list_runs(&self, _key: &str, _limit: usize) -> Vec<Run> {
            vec![]
        }
        async fn list_all_runs(&self, _limit: usize) -> Vec<Run> {
            vec![]
        }
        async fn append_event(&self, _event: &crate::domain::run::RunEvent) -> Result<()> {
            Ok(())
        }
        async fn get_events(
            &self,
            _run_id: &str,
            _limit: usize,
        ) -> Vec<crate::domain::run::RunEvent> {
            vec![]
        }
    }

    fn test_task() -> ImplementationTask {
        ImplementationTask {
            task_id: "task-1".into(),
            objective: "Fix the bug".into(),
            repo_ref: "main".into(),
            worktree_ref: None,
            constraints: vec![],
            allowed_paths: vec!["src/".into()],
            allowed_tools: vec!["shell".into()],
            tests_to_run: vec!["cargo test".into()],
            timeout_secs: 300,
            expected_output: ExpectedOutput::Patch,
        }
    }

    #[tokio::test]
    async fn execute_creates_run_and_submits() {
        let worker = MockWorker;
        let store = MockRunStore::new();
        let run_id = execute(&worker, &store, &test_task(), Some("conv-1"))
            .await
            .unwrap();

        let run = store.get_run(&run_id).await.unwrap();
        assert_eq!(run.state, RunState::Running);
        assert_eq!(run.conversation_key, Some("conv-1".into()));
    }

    #[tokio::test]
    async fn finalize_completed() {
        let store = MockRunStore::new();
        let run = Run {
            run_id: "r1".into(),
            conversation_key: None,
            origin: RunOrigin::Ipc,
            state: RunState::Running,
            started_at: 0,
            finished_at: None,
        };
        store.create_run(&run).await.unwrap();

        let result = CodingWorkerResult {
            task_id: "t1".into(),
            state: ImplementationState::Completed,
            summary: "Done".into(),
            changed_files: vec!["src/main.rs".into()],
            test_results: vec!["ok".into()],
            questions: vec![],
            artifacts: vec![],
        };
        finalize(&store, "r1", &result).await.unwrap();

        let updated = store.get_run("r1").await.unwrap();
        assert_eq!(updated.state, RunState::Completed);
        assert!(updated.finished_at.is_some());
    }
}
