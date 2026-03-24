//! Port: spawn broker — ephemeral child agent provisioning and lifecycle.
//!
//! Phase 4.0: abstracts the IPC broker's spawn machinery behind a port.

use crate::domain::spawn::{EphemeralAgent, SpawnStatus};
use anyhow::Result;
use async_trait::async_trait;

/// Port for provisioning ephemeral child agents and tracking their lifecycle.
///
/// Implementations wrap the IPC broker's provision-ephemeral endpoint
/// and spawn_runs table.
#[async_trait]
pub trait SpawnBrokerPort: Send + Sync {
    /// Provision an ephemeral identity for a child agent.
    ///
    /// The broker creates agent_id, token, session, and spawn_run record.
    async fn provision_ephemeral(
        &self,
        parent_agent_id: &str,
        parent_trust_level: u8,
        child_trust_level: Option<u8>,
        timeout_secs: u32,
        workload: Option<&str>,
    ) -> Result<EphemeralAgent>;

    /// Poll spawn run status.
    ///
    /// Returns (status, optional_result_json).
    /// Non-terminal statuses return None for result.
    async fn get_spawn_status(
        &self,
        session_id: &str,
    ) -> Result<(SpawnStatus, Option<String>)>;

    /// Report successful completion of a spawn run.
    async fn complete_spawn(
        &self,
        session_id: &str,
        result: &str,
    ) -> Result<()>;

    /// Report spawn failure.
    async fn fail_spawn(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<()>;
}
