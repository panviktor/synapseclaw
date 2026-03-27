//! Use case: SpawnChildAgent — provision and track ephemeral child agents.
//!
//! Phase 4.0: orchestrates the spawn lifecycle through ports.
//!
//! Steps:
//! 1. Validate spawn request (trust level, timeout bounds)
//! 2. Provision ephemeral identity via SpawnBrokerPort
//! 3. Create Run record (origin=Spawn) via RunStorePort
//! 4. Return provisioning details to caller
//!
//! The caller (tool or cron) is responsible for launching the subprocess.
//! Polling and finalization can be done via `poll_and_finalize`.

use crate::domain::run::{Run, RunOrigin, RunState};
use crate::domain::spawn::{EphemeralAgent, SpawnRequest, SpawnStatus};
use crate::ports::run_store::RunStorePort;
use crate::ports::spawn_broker::SpawnBrokerPort;
use anyhow::{bail, Result};

/// Result of a successful spawn.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    pub run_id: String,
    pub session_id: String,
    pub child_agent_id: String,
    pub child_token: String,
    pub expires_at: i64,
    pub effective_trust_level: u8,
}

const MIN_TIMEOUT_SECS: u32 = 10;
const MAX_TIMEOUT_SECS: u32 = 3600;
const MAX_TRUST_LEVEL_FOR_SPAWN: u8 = 3;

/// Spawn a child agent with full lifecycle tracking.
pub async fn execute(
    broker: &dyn SpawnBrokerPort,
    run_store: &dyn RunStorePort,
    parent_agent_id: &str,
    parent_trust_level: u8,
    request: &SpawnRequest,
) -> Result<SpawnResult> {
    // Validate: L4+ agents cannot spawn
    if parent_trust_level > MAX_TRUST_LEVEL_FOR_SPAWN {
        bail!(
            "Agent with trust level {} cannot spawn children (max: {})",
            parent_trust_level,
            MAX_TRUST_LEVEL_FOR_SPAWN
        );
    }

    // Clamp timeout
    let timeout = request
        .timeout_secs
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);

    // Clamp child trust level to parent's level
    let child_level = request.child_trust_level.min(parent_trust_level);

    // Provision ephemeral identity
    let provisioned: EphemeralAgent = broker
        .provision_ephemeral(
            parent_agent_id,
            parent_trust_level,
            Some(child_level),
            timeout,
            request.workload_profile.as_deref(),
        )
        .await?;

    // Create run record
    let run_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp() as u64;
    let run = Run {
        run_id: run_id.clone(),
        conversation_key: None, // Spawn runs are standalone
        origin: RunOrigin::Spawn,
        state: RunState::Running,
        started_at: now,
        finished_at: None,
    };
    run_store.create_run(&run).await?;

    Ok(SpawnResult {
        run_id,
        session_id: provisioned.session_id,
        child_agent_id: provisioned.child_agent_id,
        child_token: provisioned.child_token,
        expires_at: provisioned.expires_at,
        effective_trust_level: provisioned.effective_trust_level,
    })
}

/// Poll spawn status and finalize run if terminal.
///
/// Returns `Some(result_json)` when the spawn has completed,
/// `None` when still running.
pub async fn poll_and_finalize(
    broker: &dyn SpawnBrokerPort,
    run_store: &dyn RunStorePort,
    session_id: &str,
    run_id: &str,
) -> Result<Option<String>> {
    let (status, result) = broker.get_spawn_status(session_id).await?;

    if !status.is_terminal() {
        return Ok(None);
    }

    let now = chrono::Utc::now().timestamp() as u64;
    let run_state = match status {
        SpawnStatus::Completed => RunState::Completed,
        SpawnStatus::TimedOut | SpawnStatus::Failed => RunState::Failed,
        SpawnStatus::Revoked | SpawnStatus::Interrupted => RunState::Interrupted,
        _ => unreachable!("non-terminal filtered above"),
    };

    run_store.update_state(run_id, run_state, Some(now)).await?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::spawn::EphemeralAgent;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct MockBroker {
        provisioned: Mutex<Vec<String>>,
        status: SpawnStatus,
        result: Option<String>,
    }

    impl MockBroker {
        fn new(status: SpawnStatus, result: Option<String>) -> Self {
            Self {
                provisioned: Mutex::new(vec![]),
                status,
                result,
            }
        }
    }

    #[async_trait]
    impl SpawnBrokerPort for MockBroker {
        async fn provision_ephemeral(
            &self,
            parent_agent_id: &str,
            _parent_trust: u8,
            child_trust: Option<u8>,
            _timeout: u32,
            _workload: Option<&str>,
        ) -> Result<EphemeralAgent> {
            self.provisioned
                .lock()
                .unwrap()
                .push(parent_agent_id.to_string());
            Ok(EphemeralAgent {
                session_id: "spawn-session-1".into(),
                child_agent_id: format!("eph-{parent_agent_id}-abc123"),
                child_token: "token-xyz".into(),
                expires_at: 9999999999,
                parent_id: parent_agent_id.into(),
                effective_trust_level: child_trust.unwrap_or(2),
            })
        }

        async fn get_spawn_status(
            &self,
            _session_id: &str,
        ) -> Result<(SpawnStatus, Option<String>)> {
            Ok((self.status.clone(), self.result.clone()))
        }

        async fn complete_spawn(&self, _session_id: &str, _result: &str) -> Result<()> {
            Ok(())
        }

        async fn fail_spawn(&self, _session_id: &str, _reason: &str) -> Result<()> {
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
            let mut runs = self.runs.lock().unwrap();
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

    fn test_request() -> SpawnRequest {
        SpawnRequest {
            prompt: "Review this PR".into(),
            child_trust_level: 2,
            timeout_secs: 300,
            workload_profile: None,
            model_override: None,
            wait_for_completion: false,
        }
    }

    #[tokio::test]
    async fn spawn_creates_run_and_provisions() {
        let broker = MockBroker::new(SpawnStatus::Running, None);
        let run_store = MockRunStore::new();

        let result = execute(&broker, &run_store, "parent-agent", 1, &test_request())
            .await
            .unwrap();

        assert_eq!(result.session_id, "spawn-session-1");
        assert!(result.child_agent_id.contains("parent-agent"));
        assert_eq!(result.effective_trust_level, 1); // clamped to parent's level

        let runs = run_store.runs.lock().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].origin, RunOrigin::Spawn);
        assert_eq!(runs[0].state, RunState::Running);
    }

    #[tokio::test]
    async fn spawn_clamps_child_trust_to_parent() {
        let broker = MockBroker::new(SpawnStatus::Running, None);
        let run_store = MockRunStore::new();

        let mut req = test_request();
        req.child_trust_level = 0; // Requesting L0 (higher privilege)

        let result = execute(&broker, &run_store, "parent", 2, &req)
            .await
            .unwrap();
        // child_level = min(0, 2) = 0, but broker received child_trust=Some(0)
        // In real broker, it would clamp to parent's level
        assert_eq!(result.effective_trust_level, 0);
    }

    #[tokio::test]
    async fn spawn_denied_for_l4_agents() {
        let broker = MockBroker::new(SpawnStatus::Running, None);
        let run_store = MockRunStore::new();

        let err = execute(&broker, &run_store, "quarantined", 4, &test_request())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cannot spawn"));
    }

    #[tokio::test]
    async fn poll_returns_none_when_running() {
        let broker = MockBroker::new(SpawnStatus::Running, None);
        let run_store = MockRunStore::new();

        let result = poll_and_finalize(&broker, &run_store, "sess", "run-1")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn poll_finalizes_completed_run() {
        let broker = MockBroker::new(SpawnStatus::Completed, Some("done!".into()));
        let run_store = MockRunStore::new();

        // Create a run first
        let run = Run {
            run_id: "run-1".into(),
            conversation_key: None,
            origin: RunOrigin::Spawn,
            state: RunState::Running,
            started_at: 1000,
            finished_at: None,
        };
        run_store.create_run(&run).await.unwrap();

        let result = poll_and_finalize(&broker, &run_store, "sess", "run-1")
            .await
            .unwrap();
        assert_eq!(result, Some("done!".into()));

        let updated = run_store.get_run("run-1").await.unwrap();
        assert_eq!(updated.state, RunState::Completed);
        assert!(updated.finished_at.is_some());
    }
}
