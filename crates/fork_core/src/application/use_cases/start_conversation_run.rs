//! Use case: StartConversationRun — create a run, execute agent, update state.
//!
//! Phase 4.0 Slice 3: extracts the run lifecycle from ws.rs handle_chat_send_rpc.
//!
//! Orchestrates: create run → persist user message → execute agent →
//! persist response → update tokens → complete/fail run → trigger summary.

use crate::application::services::conversation_service;
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::run_store::RunStorePort;
use anyhow::Result;
use std::sync::Arc;

/// Result of a conversation run.
#[derive(Debug, Clone)]
pub struct ConversationRunResult {
    pub run_id: String,
    pub response: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub state: crate::domain::run::RunState,
}

/// Execute a conversation run with full lifecycle management.
///
/// This use case owns the sequence:
/// 1. Create run record (Running)
/// 2. Delegate agent execution to caller (via callback result)
/// 3. Update token counts and message counts
/// 4. Update run state (Completed/Failed/Interrupted)
///
/// The actual agent execution is done by the caller — this use case
/// manages the surrounding lifecycle.
pub async fn create_and_track_run(
    conversation_store: &dyn ConversationStorePort,
    run_store: &dyn RunStorePort,
    session_key: &str,
) -> Result<String> {
    // Touch session to update last_active
    let _ = conversation_store.touch_session(session_key).await;

    // Create run
    conversation_service::create_web_run(run_store, session_key).await
}

/// Finalize a successful run — update counts, tokens, state.
pub async fn finalize_success(
    conversation_store: &dyn ConversationStorePort,
    run_store: &dyn RunStorePort,
    session_key: &str,
    run_id: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<()> {
    // Increment message count (+2: user + assistant)
    let _ = conversation_service::increment_message_count(conversation_store, session_key, 2).await;

    // Track token usage
    let _ =
        conversation_service::add_token_usage(conversation_store, session_key, input_tokens, output_tokens)
            .await;

    // Mark run completed
    conversation_service::complete_run(run_store, run_id).await
}

/// Finalize a failed run.
pub async fn finalize_failure(
    run_store: &dyn RunStorePort,
    conversation_store: &dyn ConversationStorePort,
    session_key: &str,
    run_id: &str,
) -> Result<()> {
    // Still count the messages (user turn was sent)
    let _ = conversation_service::increment_message_count(conversation_store, session_key, 2).await;

    conversation_service::fail_run(run_store, run_id).await
}

/// Finalize an interrupted (aborted) run.
pub async fn finalize_interrupted(
    run_store: &dyn RunStorePort,
    conversation_store: &dyn ConversationStorePort,
    session_key: &str,
    run_id: &str,
) -> Result<()> {
    let _ = conversation_service::increment_message_count(conversation_store, session_key, 2).await;

    conversation_service::interrupt_run(run_store, run_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::ConversationSession;
    use crate::domain::run::{Run, RunState};
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
            let mut runs = self.runs.lock().unwrap();
            if let Some(run) = runs.iter_mut().find(|r| r.run_id == run_id) {
                run.state = state;
                run.finished_at = finished_at;
            }
            Ok(())
        }
        async fn list_runs(&self, _key: &str, _limit: usize) -> Vec<Run> {
            self.runs.lock().unwrap().clone()
        }
        async fn list_all_runs(&self, _limit: usize) -> Vec<Run> {
            self.runs.lock().unwrap().clone()
        }
        async fn append_event(&self, _event: &crate::domain::run::RunEvent) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_events(&self, _run_id: &str, _limit: usize) -> Vec<crate::domain::run::RunEvent> {
            vec![]
        }
    }

    #[tokio::test]
    async fn create_and_track_creates_running_run() {
        let conv_store = MockConversationStore;
        let run_store = MockRunStore::new();
        let run_id = create_and_track_run(&conv_store, &run_store, "web:abc:123")
            .await
            .unwrap();
        let run = run_store.get_run(&run_id).await.unwrap();
        assert_eq!(run.state, RunState::Running);
        assert_eq!(run.conversation_key, Some("web:abc:123".to_string()));
    }

    #[tokio::test]
    async fn finalize_success_completes_run() {
        let conv_store = MockConversationStore;
        let run_store = MockRunStore::new();
        let run_id = create_and_track_run(&conv_store, &run_store, "web:abc:123")
            .await
            .unwrap();
        finalize_success(&conv_store, &run_store, "web:abc:123", &run_id, 100, 50)
            .await
            .unwrap();
        let run = run_store.get_run(&run_id).await.unwrap();
        assert_eq!(run.state, RunState::Completed);
        assert!(run.finished_at.is_some());
    }

    #[tokio::test]
    async fn finalize_failure_fails_run() {
        let conv_store = MockConversationStore;
        let run_store = MockRunStore::new();
        let run_id = create_and_track_run(&conv_store, &run_store, "web:abc:123")
            .await
            .unwrap();
        finalize_failure(&run_store, &conv_store, "web:abc:123", &run_id)
            .await
            .unwrap();
        let run = run_store.get_run(&run_id).await.unwrap();
        assert_eq!(run.state, RunState::Failed);
    }

    #[tokio::test]
    async fn finalize_interrupted_interrupts_run() {
        let conv_store = MockConversationStore;
        let run_store = MockRunStore::new();
        let run_id = create_and_track_run(&conv_store, &run_store, "web:abc:123")
            .await
            .unwrap();
        finalize_interrupted(&run_store, &conv_store, "web:abc:123", &run_id)
            .await
            .unwrap();
        let run = run_store.get_run(&run_id).await.unwrap();
        assert_eq!(run.state, RunState::Interrupted);
    }
}
