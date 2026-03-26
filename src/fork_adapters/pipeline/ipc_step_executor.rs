//! IPC-based pipeline step executor adapter.
//!
//! Phase 4.1 Slice 2: dispatches pipeline steps to agents via the IPC broker.
//!
//! Flow:
//! 1. Build a task message from step definition + input data
//! 2. Send via `DispatchIpcMessage` (existing use case)
//! 3. Poll agent's response via `IpcBusPort::fetch_inbox`
//! 4. Parse and return the result

use async_trait::async_trait;
use fork_core::application::use_cases::dispatch_ipc_message::{self, DispatchParams};
use fork_core::ports::ipc_bus::IpcBusPort;
use fork_core::ports::pipeline_executor::{
    PipelineExecutorPort, StepExecutionError, StepExecutionResult,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

/// Adapter: executes pipeline steps by dispatching IPC messages.
///
/// The pipeline runner (in fork_core) calls this adapter, which translates
/// the step into an IPC `task` message, sends it through the broker, and
/// waits for the agent's `result` response.
pub struct IpcStepExecutor {
    /// IPC bus for sending/receiving messages.
    bus: Arc<dyn IpcBusPort>,
    /// Trust level of the pipeline runner (typically L1 — highest).
    runner_trust_level: i32,
    /// Agent ID of the pipeline runner (used as `from_agent`).
    runner_agent_id: String,
    /// Poll interval for checking agent responses (milliseconds).
    poll_interval_ms: u64,
}

impl IpcStepExecutor {
    /// Create a new IPC step executor.
    ///
    /// - `bus`: the IPC bus port (wrapping broker DB)
    /// - `runner_agent_id`: identity of the pipeline runner
    /// - `runner_trust_level`: trust level (1 = operator, can dispatch to any agent)
    pub fn new(
        bus: Arc<dyn IpcBusPort>,
        runner_agent_id: String,
        runner_trust_level: i32,
    ) -> Self {
        Self {
            bus,
            runner_agent_id,
            runner_trust_level,
            poll_interval_ms: 500,
        }
    }

    /// Override the poll interval (for testing).
    #[cfg(test)]
    pub fn with_poll_interval_ms(mut self, ms: u64) -> Self {
        self.poll_interval_ms = ms;
        self
    }

    /// Build the task payload JSON sent to the agent.
    fn build_task_payload(
        step_id: &str,
        input: &Value,
        tools: &[String],
        description: &str,
    ) -> String {
        let payload = serde_json::json!({
            "pipeline_step": step_id,
            "description": description,
            "input": input,
            "tools": tools,
        });
        payload.to_string()
    }

    /// Parse the agent's response payload into a JSON value.
    fn parse_response(payload: &str) -> Result<Value, StepExecutionError> {
        // Try to parse as JSON first
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            return Ok(value);
        }

        // If not valid JSON, wrap the raw text as a string value
        // This handles agents that return plain text instead of JSON
        Ok(Value::String(payload.to_string()))
    }
}

#[async_trait]
impl PipelineExecutorPort for IpcStepExecutor {
    async fn execute_step(
        &self,
        run_id: &str,
        step_id: &str,
        agent_id: &str,
        input: &Value,
        tools: &[String],
        description: &str,
        timeout_secs: Option<u64>,
    ) -> Result<StepExecutionResult, StepExecutionError> {
        // Session ID ties the task/result pair together
        let session_id = format!("pipeline:{}:{}", run_id, step_id);

        // Build and send the task message
        let payload = Self::build_task_payload(step_id, input, tools, description);
        let empty_pairs: Vec<(String, String)> = vec![];
        let empty_dests: HashMap<String, String> = HashMap::new();

        let dispatch_result = dispatch_ipc_message::execute(
            self.bus.as_ref(),
            &DispatchParams {
                from_agent: &self.runner_agent_id,
                to_agent: agent_id,
                kind: "task",
                payload: &payload,
                session_id: Some(&session_id),
                from_trust_level: self.runner_trust_level,
                priority: 5, // pipeline tasks get moderate priority
                lateral_text_pairs: &empty_pairs,
                l4_destinations: &empty_dests,
                max_session_exchanges: 0,
                session_message_count: 0,
            },
        )
        .await
        .map_err(|e| StepExecutionError {
            code: e.code.clone(),
            message: format!("IPC dispatch failed: {}", e.message),
            retryable: e.retryable,
        })?;

        debug!(
            run_id = %run_id,
            step = %step_id,
            agent = %agent_id,
            seq = dispatch_result.seq,
            session = %session_id,
            "step task dispatched"
        );

        // Wait for the agent's result response.
        // Safety net: if no timeout specified, default to 30 minutes to prevent
        // infinite polling if the agent never responds.
        const DEFAULT_STEP_TIMEOUT_SECS: u64 = 1800;
        let effective_timeout = timeout_secs.unwrap_or(DEFAULT_STEP_TIMEOUT_SECS);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(effective_timeout);

        let poll_duration = std::time::Duration::from_millis(self.poll_interval_ms);

        loop {
            // Check timeout
            if std::time::Instant::now() >= deadline {
                return Err(StepExecutionError {
                    code: "step_timeout".into(),
                    message: format!(
                        "step '{}' timed out waiting for agent '{}' ({}s)",
                        step_id, agent_id, effective_timeout
                    ),
                    retryable: true,
                });
            }

            // Poll the runner's inbox for a result from this session
            let messages = self
                .bus
                .fetch_inbox(&self.runner_agent_id, false, 50)
                .await
                .map_err(|e| StepExecutionError {
                    code: "inbox_error".into(),
                    message: format!("failed to fetch inbox: {e}"),
                    retryable: true,
                })?;

            // Find a result message matching our session
            if let Some(result_msg) = messages.iter().find(|m| {
                m.session_id.as_deref() == Some(&session_id)
                    && m.from_agent == agent_id
                    && m.kind == "result"
                    && !m.read
            }) {
                let message_seq = result_msg.id;

                // ACK the message
                if let Err(e) = self
                    .bus
                    .ack_messages(&self.runner_agent_id, &[result_msg.id])
                    .await
                {
                    warn!(
                        run_id = %run_id,
                        msg_id = result_msg.id,
                        error = %e,
                        "failed to ACK result message"
                    );
                }

                // Parse the response
                let output = Self::parse_response(&result_msg.payload)?;

                return Ok(StepExecutionResult {
                    output,
                    message_seq,
                });
            }

            // Not ready yet — wait and poll again
            tokio::time::sleep(poll_duration).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fork_core::domain::ipc::IpcMessage;
    use std::sync::Mutex;

    /// Mock IPC bus that records sent messages and returns predetermined responses.
    struct MockIpcBus {
        agents: Vec<(String, i32)>,
        inbox_responses: Mutex<Vec<IpcMessage>>,
        sent: Mutex<Vec<String>>,
    }

    impl MockIpcBus {
        fn with_response(agent_id: &str, session_id: &str, payload: &str) -> Self {
            Self {
                agents: vec![
                    ("pipeline-runner".into(), 1),
                    (agent_id.into(), 3),
                ],
                inbox_responses: Mutex::new(vec![IpcMessage {
                    id: 100,
                    from_agent: agent_id.into(),
                    to_agent: "pipeline-runner".into(),
                    kind: "result".into(),
                    payload: payload.into(),
                    session_id: Some(session_id.into()),
                    from_trust_level: 3,
                    priority: 0,
                    created_at: 0,
                    promoted: false,
                    read: false,
                    blocked: false,
                }]),
                sent: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl IpcBusPort for MockIpcBus {
        async fn send_message(
            &self, from: &str, to: &str, kind: &str, payload: &str,
            sid: Option<&str>, _ftl: i32, _pri: i32,
        ) -> anyhow::Result<i64> {
            self.sent.lock().unwrap().push(format!(
                "{from}->{to}:{kind}:{}",
                sid.unwrap_or("none")
            ));
            Ok(1)
        }

        async fn fetch_inbox(
            &self, _agent: &str, _q: bool, _limit: u32,
        ) -> anyhow::Result<Vec<IpcMessage>> {
            Ok(self.inbox_responses.lock().unwrap().clone())
        }

        async fn ack_messages(&self, _agent: &str, _ids: &[i64]) -> anyhow::Result<u64> {
            Ok(1)
        }

        async fn session_has_request(&self, _sid: &str, _from: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        async fn get_agent_trust_level(&self, agent_id: &str) -> Option<i32> {
            self.agents.iter().find(|(a, _)| a == agent_id).map(|(_, l)| *l)
        }
    }

    #[tokio::test]
    async fn execute_step_sends_and_receives() {
        let session_id = "pipeline:run-1:step1";
        let bus = Arc::new(MockIpcBus::with_response(
            "agent-a",
            session_id,
            r#"{"research": "done"}"#,
        ));

        let executor = IpcStepExecutor::new(
            bus.clone(),
            "pipeline-runner".into(),
            1,
        )
        .with_poll_interval_ms(10);

        let result = executor
            .execute_step(
                "run-1",
                "step1",
                "agent-a",
                &serde_json::json!({"topic": "test"}),
                &["web_search".into()],
                "Research step",
                Some(5),
            )
            .await
            .unwrap();

        assert_eq!(result.output, serde_json::json!({"research": "done"}));
        assert_eq!(result.message_seq, 100);

        // Verify the task was sent
        let sent = bus.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains("pipeline-runner->agent-a:task"));
    }

    #[tokio::test]
    async fn execute_step_plain_text_response() {
        let session_id = "pipeline:run-2:step1";
        let bus = Arc::new(MockIpcBus::with_response(
            "agent-b",
            session_id,
            "Just a plain text response",
        ));

        let executor = IpcStepExecutor::new(
            bus,
            "pipeline-runner".into(),
            1,
        )
        .with_poll_interval_ms(10);

        let result = executor
            .execute_step(
                "run-2",
                "step1",
                "agent-b",
                &serde_json::json!({}),
                &[],
                "test",
                Some(5),
            )
            .await
            .unwrap();

        // Plain text wrapped as string Value
        assert_eq!(
            result.output,
            Value::String("Just a plain text response".into())
        );
    }

    #[test]
    fn build_task_payload_structure() {
        let payload = IpcStepExecutor::build_task_payload(
            "research",
            &serde_json::json!({"topic": "Rust"}),
            &["web_search".into(), "memory_read".into()],
            "Research the topic",
        );
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["pipeline_step"], "research");
        assert_eq!(parsed["description"], "Research the topic");
        assert_eq!(parsed["input"]["topic"], "Rust");
        assert_eq!(parsed["tools"][0], "web_search");
    }
}
