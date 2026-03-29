//! IPC-based pipeline step executor adapter.
//!
//! Phase 4.1: dispatches pipeline steps to agents via the IPC broker.
//!
//! Delegates all IPC communication to the existing `IpcClient`, which
//! handles Ed25519 signing, sender_seq, replay protection, and key
//! registration — no duplication.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use synapse_core::ports::pipeline_executor::{
    PipelineExecutorPort, StepExecutionError, StepExecutionResult,
};
use tracing::debug;

/// Adapter: executes pipeline steps via the existing IpcClient.
pub struct IpcStepExecutor {
    /// Shared IPC client (same instance the broker uses for its own IPC).
    ipc_client: Arc<crate::adapters::tools::agents_ipc::IpcClient>,
    /// Poll interval for checking agent responses (milliseconds).
    poll_interval_ms: u64,
}

impl IpcStepExecutor {
    /// Create a new executor wrapping an existing IpcClient.
    pub fn new(ipc_client: Arc<crate::adapters::tools::agents_ipc::IpcClient>) -> Self {
        Self {
            ipc_client,
            poll_interval_ms: 2000,
        }
    }

    /// Build the task payload JSON sent to the agent.
    fn build_task_payload(
        step_id: &str,
        input: &Value,
        tools: &[String],
        description: &str,
    ) -> String {
        serde_json::json!({
            "pipeline_step": step_id,
            "description": description,
            "input": input,
            "tools": tools,
        })
        .to_string()
    }

    /// Parse the agent's response payload as JSON.
    fn parse_response(payload: &str) -> Result<Value, StepExecutionError> {
        serde_json::from_str::<Value>(payload).map_err(|e| StepExecutionError {
            code: "invalid_json".into(),
            message: format!("agent response is not valid JSON (model may be hallucinating): {e}"),
            retryable: true,
        })
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
        let session_id = format!("pipeline:{}:{}", run_id, step_id);
        let payload = Self::build_task_payload(step_id, input, tools, description);

        // Send task via IpcClient
        let mut body = serde_json::json!({
            "to": agent_id,
            "kind": "task",
            "payload": payload,
            "session_id": session_id,
            "priority": 5,
        });

        // Sign the message (adds signature, sender_seq, sender_timestamp)
        self.ipc_client.sign_send_body(&mut body);

        let resp = self
            .ipc_client
            .send_message(&body)
            .await
            .map_err(|e| StepExecutionError {
                code: "http_error".into(),
                message: format!("broker send failed: {e}"),
                retryable: true,
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(StepExecutionError {
                code: format!("broker_{}", status.as_u16()),
                message: format!("broker returned {status}: {text}"),
                retryable: status.is_server_error(),
            });
        }

        let json: Value = resp.json().await.unwrap_or(Value::Null);
        let seq = json["seq"].as_i64().unwrap_or(0);

        debug!(
            run_id = %run_id,
            step = %step_id,
            agent = %agent_id,
            seq = seq,
            session = %session_id,
            "step task dispatched"
        );

        // Poll for result
        const DEFAULT_STEP_TIMEOUT_SECS: u64 = 1800;
        let effective_timeout = timeout_secs.unwrap_or(DEFAULT_STEP_TIMEOUT_SECS);
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(effective_timeout);
        let poll_duration = std::time::Duration::from_millis(self.poll_interval_ms);

        loop {
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

            let messages = self
                .ipc_client
                .peek_inbox(Some(agent_id), Some(&["result"]), 50)
                .await
                .unwrap_or_default();

            // Find a result matching our session
            if let Some(result_msg) = messages
                .iter()
                .find(|m| m["session_id"].as_str() == Some(&session_id))
            {
                let msg_id = result_msg["id"].as_i64().unwrap_or(0);
                let payload_str = result_msg["payload"].as_str().unwrap_or("{}");

                // ACK the message
                let _ = self.ipc_client.ack_messages(&[msg_id]).await;

                // Parse response
                let output = Self::parse_response(payload_str)?;

                return Ok(StepExecutionResult {
                    output,
                    message_seq: msg_id,
                });
            }

            tokio::time::sleep(poll_duration).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn parse_valid_json() {
        let result = IpcStepExecutor::parse_response(r#"{"topic": "test"}"#);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_invalid_json_fails() {
        let result = IpcStepExecutor::parse_response("not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().retryable);
    }
}
