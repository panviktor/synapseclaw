//! Use case: AbortConversationRun — cancel a running execution.
//!
//! Phase 4.0: provides clean abort for web chat, channel, and IPC runs.
//!
//! Business rules:
//! - Only non-terminal runs can be aborted
//! - Aborted runs get Cancelled state (operator-initiated) vs Interrupted (timeout/new-message)
//! - Message count is still incremented (user turn was sent)

use crate::domain::run::RunState;
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::run_store::RunStorePort;
use anyhow::{bail, Result};

/// Abort a conversation run by its run_id.
///
/// Returns the final run state. Fails if the run is already terminal.
pub async fn execute(
    run_store: &dyn RunStorePort,
    conversation_store: &dyn ConversationStorePort,
    run_id: &str,
) -> Result<RunState> {
    // Fetch current run
    let run = run_store
        .get_run(run_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("Run '{run_id}' not found"))?;

    // Guard: cannot abort a terminal run
    if run.state.is_terminal() {
        bail!(
            "Run '{run_id}' is already in terminal state: {}",
            run.state
        );
    }

    let now = chrono::Utc::now().timestamp() as u64;

    // Transition to Cancelled
    run_store
        .update_state(run_id, RunState::Cancelled, Some(now))
        .await?;

    // Increment message count if we have a conversation
    if let Some(ref key) = run.conversation_key {
        let _ = conversation_store.increment_message_count(key).await;
    }

    Ok(RunState::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::ConversationSession;
    use crate::domain::run::{Run, RunOrigin, RunState};
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockConversationStore;

    #[async_trait]
    impl ConversationStorePort for MockConversationStore {
        async fn get_session(&self, _key: &str) -> Option<ConversationSession> { None }
        async fn upsert_session(&self, _session: &ConversationSession) -> Result<()> { Ok(()) }
        async fn delete_session(&self, _key: &str) -> Result<bool> { Ok(true) }
        async fn list_sessions(&self, _prefix: Option<&str>) -> Vec<ConversationSession> { vec![] }
        async fn touch_session(&self, _key: &str) -> Result<()> { Ok(()) }
        async fn append_event(&self, _key: &str, _event: &crate::domain::conversation::ConversationEvent) -> Result<()> { Ok(()) }
        async fn get_events(&self, _key: &str, _limit: usize) -> Vec<crate::domain::conversation::ConversationEvent> { vec![] }
        async fn clear_events(&self, _key: &str) -> Result<()> { Ok(()) }
        async fn update_label(&self, _key: &str, _label: &str) -> Result<()> { Ok(()) }
        async fn update_goal(&self, _key: &str, _goal: &str) -> Result<()> { Ok(()) }
        async fn increment_message_count(&self, _key: &str) -> Result<()> { Ok(()) }
        async fn add_token_usage(&self, _key: &str, _input: i64, _output: i64) -> Result<()> { Ok(()) }
        async fn set_summary(&self, _key: &str, _summary: &str) -> Result<()> { Ok(()) }
        async fn get_summary(&self, _key: &str) -> Option<String> { None }
    }

    struct MockRunStore {
        runs: Mutex<Vec<Run>>,
    }

    impl MockRunStore {
        fn with_run(run: Run) -> Self {
            Self {
                runs: Mutex::new(vec![run]),
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
            self.runs.lock().unwrap().iter().find(|r| r.run_id == run_id).cloned()
        }
        async fn update_state(&self, run_id: &str, state: RunState, finished_at: Option<u64>) -> Result<()> {
            let mut runs = self.runs.lock().unwrap();
            if let Some(run) = runs.iter_mut().find(|r| r.run_id == run_id) {
                run.state = state;
                run.finished_at = finished_at;
            }
            Ok(())
        }
        async fn list_runs(&self, _key: &str, _limit: usize) -> Vec<Run> { vec![] }
        async fn list_all_runs(&self, _limit: usize) -> Vec<Run> { vec![] }
        async fn append_event(&self, _event: &crate::domain::run::RunEvent) -> Result<()> { Ok(()) }
        async fn get_events(&self, _run_id: &str, _limit: usize) -> Vec<crate::domain::run::RunEvent> { vec![] }
    }

    fn running_run(id: &str, conv_key: Option<&str>) -> Run {
        Run {
            run_id: id.to_string(),
            conversation_key: conv_key.map(String::from),
            origin: RunOrigin::Web,
            state: RunState::Running,
            started_at: 1000,
            finished_at: None,
        }
    }

    #[tokio::test]
    async fn abort_running_run_succeeds() {
        let run_store = MockRunStore::with_run(running_run("r1", Some("web:abc")));
        let conv_store = MockConversationStore;

        let result = execute(&run_store, &conv_store, "r1").await.unwrap();
        assert_eq!(result, RunState::Cancelled);

        let run = run_store.get_run("r1").await.unwrap();
        assert_eq!(run.state, RunState::Cancelled);
        assert!(run.finished_at.is_some());
    }

    #[tokio::test]
    async fn abort_terminal_run_fails() {
        let mut run = running_run("r2", Some("web:abc"));
        run.state = RunState::Completed;
        run.finished_at = Some(2000);
        let run_store = MockRunStore::with_run(run);
        let conv_store = MockConversationStore;

        let err = execute(&run_store, &conv_store, "r2").await.unwrap_err();
        assert!(err.to_string().contains("terminal state"));
    }

    #[tokio::test]
    async fn abort_unknown_run_fails() {
        let run_store = MockRunStore::with_run(running_run("r1", None));
        let conv_store = MockConversationStore;

        let err = execute(&run_store, &conv_store, "nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn abort_run_without_conversation() {
        let run_store = MockRunStore::with_run(running_run("r3", None));
        let conv_store = MockConversationStore;

        let result = execute(&run_store, &conv_store, "r3").await.unwrap();
        assert_eq!(result, RunState::Cancelled);
    }
}
