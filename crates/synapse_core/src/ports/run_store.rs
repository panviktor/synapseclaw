//! Port: run execution store for durable lifecycle tracking.
//!
//! Unifies chat runs, IPC execution, spawn runs, and cron jobs under
//! a single contract.  Phase 4.0 Step 4.

use crate::domain::run::{Run, RunEvent, RunState};
use async_trait::async_trait;

/// Port for storing and querying execution runs and their events.
///
/// Implementations: `ChatDbRunStore` (extends existing `ChatDb` with
/// `runs` + `run_events` tables).
#[async_trait]
pub trait RunStorePort: Send + Sync {
    /// Create a new run record.
    async fn create_run(&self, run: &Run) -> anyhow::Result<()>;

    /// Get a run by its ID.
    async fn get_run(&self, run_id: &str) -> Option<Run>;

    /// Update run state and optionally set finished_at.
    async fn update_state(
        &self,
        run_id: &str,
        state: RunState,
        finished_at: Option<u64>,
    ) -> anyhow::Result<()>;

    /// List runs for a conversation, newest first.
    async fn list_runs(&self, conversation_key: &str, limit: usize) -> Vec<Run>;

    /// List all runs across all conversations, newest first.
    async fn list_all_runs(&self, limit: usize) -> Vec<Run>;

    /// Append an event to a run.
    async fn append_event(&self, event: &RunEvent) -> anyhow::Result<()>;

    /// Get events for a run, chronological order.
    async fn get_events(&self, run_id: &str, limit: usize) -> Vec<RunEvent>;

    /// List runs in any of the given states (for pipeline recovery on startup).
    /// Default impl falls back to list_all_runs + filter.
    async fn list_by_state(&self, states: &[RunState], limit: usize) -> Vec<Run> {
        self.list_all_runs(limit)
            .await
            .into_iter()
            .filter(|r| states.contains(&r.state))
            .collect()
    }
}

/// No-op run store for contexts where persistence is not needed
/// (e.g. channel-triggered pipeline runs without a backing DB).
pub struct NoOpRunStore;

#[async_trait]
impl RunStorePort for NoOpRunStore {
    async fn create_run(&self, _run: &Run) -> anyhow::Result<()> {
        Ok(())
    }
    async fn get_run(&self, _run_id: &str) -> Option<Run> {
        None
    }
    async fn update_state(
        &self,
        _run_id: &str,
        _state: RunState,
        _finished_at: Option<u64>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn list_runs(&self, _conversation_key: &str, _limit: usize) -> Vec<Run> {
        vec![]
    }
    async fn list_all_runs(&self, _limit: usize) -> Vec<Run> {
        vec![]
    }
    async fn append_event(&self, _event: &RunEvent) -> anyhow::Result<()> {
        Ok(())
    }
    async fn get_events(&self, _run_id: &str, _limit: usize) -> Vec<RunEvent> {
        vec![]
    }
}
