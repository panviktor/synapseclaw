//! Adapter: implements `AgentRunnerPort` by delegating to `agent::run` / `agent::process_message`.

use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use synapse_domain::config::schema::Config;
use synapse_domain::domain::tool_audit::RunContext;
use synapse_domain::ports::agent_runner::AgentRunnerPort;

/// Concrete implementation of `AgentRunnerPort` backed by the real agent loop.
pub struct AgentRunner {
    config: Arc<Mutex<Config>>,
}

impl AgentRunner {
    pub fn new(config: Arc<Mutex<Config>>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentRunnerPort for AgentRunner {
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
    ) -> Result<String> {
        let config = self.config.lock().unwrap().clone();
        Box::pin(super::run(
            config,
            message,
            provider_override,
            model_override,
            temperature,
            interactive,
            session_state_file,
            allowed_tools,
            run_ctx,
        ))
        .await
    }

    async fn process_message(&self, message: &str, session_id: Option<&str>) -> Result<String> {
        let config = self.config.lock().unwrap().clone();
        Box::pin(super::process_message(config, message, session_id)).await
    }
}
