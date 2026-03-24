//! Port: coding worker — delegate implementation tasks to external engines.
//!
//! Phase 4.0 Slice 7: narrow seam for Codex, Claude Code, or similar.
//!
//! Design rule: external workers are leaf executors, not replacement cores.
//! The fork core owns orchestration, trust, approval, and memory.

use crate::fork_core::domain::implementation::{
    CodingWorkerResult, ImplementationEvent, ImplementationTask,
};
use anyhow::Result;
use async_trait::async_trait;

/// Port for delegating implementation tasks to external coding workers.
#[async_trait]
pub trait CodingWorkerPort: Send + Sync {
    /// Submit a bounded implementation task.
    /// Returns the run_id for tracking.
    async fn submit_task(&self, task: &ImplementationTask) -> Result<String>;

    /// Poll for the latest result (non-blocking).
    async fn poll_result(&self, run_id: &str) -> Result<Option<CodingWorkerResult>>;

    /// Get events for a run (progress, questions, artifacts).
    async fn get_events(&self, run_id: &str, limit: usize) -> Result<Vec<ImplementationEvent>>;

    /// Cancel a running task.
    async fn cancel(&self, run_id: &str) -> Result<()>;
}
