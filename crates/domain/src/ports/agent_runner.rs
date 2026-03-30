//! Port: agent runner — high-level agent execution.
//!
//! Abstracts `agent::run()` and `agent::process_message()` so that
//! synapse_adapters modules (gateway, daemon, cron) can invoke the agent
//! without depending on the concrete agent implementation.

use crate::domain::tool_audit::RunContext;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

/// Port for running the agent — decouples synapse_adapters from `crate::agent`.
#[async_trait]
pub trait AgentRunnerPort: Send + Sync {
    /// Run the agent with a message (equivalent to `agent::run`).
    ///
    /// Returns the final response text.
    async fn run(
        &self,
        message: Option<String>,
        provider_override: Option<String>,
        model_override: Option<String>,
        temperature: f64,
        interactive: bool,
        session_state_file: Option<PathBuf>,
        allowed_tools: Option<Vec<String>>,
        run_ctx: Option<Arc<RunContext>>,
    ) -> Result<String>;

    /// Process a single message non-interactively (equivalent to `agent::process_message`).
    async fn process_message(&self, message: &str, session_id: Option<&str>) -> Result<String>;
}
