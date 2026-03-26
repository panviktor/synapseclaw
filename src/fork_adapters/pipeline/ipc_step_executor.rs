//! IPC-based pipeline step executor adapter.
//!
//! Phase 4.1: dispatches pipeline steps to agents via the IPC broker HTTP API.
//!
//! Uses the same HTTP path as all other agents:
//! - POST /api/ipc/send → triggers push notification to target agent
//! - GET /api/ipc/inbox → polls for result messages
//! - POST /api/ipc/ack → acknowledges processed messages
//!
//! This ensures push dispatcher, ACL, audit, and rate limiting all work
//! correctly — the pipeline runner is just another IPC client.

use async_trait::async_trait;
use fork_core::ports::pipeline_executor::{
    PipelineExecutorPort, StepExecutionError, StepExecutionResult,
};
use serde_json::Value;
use tracing::{debug, warn};

/// Adapter: executes pipeline steps via broker HTTP API.
pub struct IpcStepExecutor {
    /// Broker HTTP base URL (e.g. "http://127.0.0.1:42617").
    broker_url: String,
    /// Bearer token for authenticating with the broker.
    bearer_token: String,
    /// Agent ID of the pipeline runner (used as `from_agent`).
    runner_agent_id: String,
    /// HTTP client.
    client: reqwest::Client,
    /// Poll interval for checking agent responses (milliseconds).
    poll_interval_ms: u64,
}

impl IpcStepExecutor {
    /// Create a new IPC step executor.
    ///
    /// - `broker_url`: broker gateway URL
    /// - `bearer_token`: authentication token for the broker
    /// - `runner_agent_id`: pipeline runner's agent identity
    pub fn new(broker_url: String, bearer_token: String, runner_agent_id: String) -> Self {
        Self {
            broker_url,
            bearer_token,
            runner_agent_id,
            client: reqwest::Client::new(),
            poll_interval_ms: 2000,
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
            message: format!(
                "agent response is not valid JSON (model may be hallucinating): {e}"
            ),
            retryable: true,
        })
    }

    /// Send a message via broker HTTP API (POST /api/ipc/send).
    async fn send_message(
        &self,
        to_agent: &str,
        kind: &str,
        payload: &str,
        session_id: &str,
    ) -> Result<i64, StepExecutionError> {
        let body = serde_json::json!({
            "to": to_agent,
            "kind": kind,
            "payload": payload,
            "session_id": session_id,
            "priority": 5,
        });

        let resp = self
            .client
            .post(format!("{}/api/ipc/send", self.broker_url))
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .json(&body)
            .send()
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
        Ok(seq)
    }

    /// Fetch inbox messages via broker HTTP API (GET /api/ipc/inbox).
    async fn fetch_inbox(&self) -> Result<Vec<Value>, StepExecutionError> {
        let resp = self
            .client
            .get(format!("{}/api/ipc/inbox", self.broker_url))
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .send()
            .await
            .map_err(|e| StepExecutionError {
                code: "http_error".into(),
                message: format!("broker inbox fetch failed: {e}"),
                retryable: true,
            })?;

        if !resp.status().is_success() {
            return Ok(vec![]);
        }

        let json: Value = resp.json().await.unwrap_or(Value::Null);
        let messages = json["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(messages)
    }

    /// Acknowledge messages via broker HTTP API (POST /api/ipc/ack).
    async fn ack_messages(&self, message_ids: &[i64]) {
        let body = serde_json::json!({"message_ids": message_ids});
        let _ = self
            .client
            .post(format!("{}/api/ipc/ack", self.broker_url))
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .json(&body)
            .send()
            .await;
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

        // Send task via broker HTTP API (triggers push notification)
        let seq = self
            .send_message(agent_id, "task", &payload, &session_id)
            .await?;

        debug!(
            run_id = %run_id,
            step = %step_id,
            agent = %agent_id,
            seq = seq,
            session = %session_id,
            "step task dispatched via HTTP"
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

            let messages = self.fetch_inbox().await?;

            // Find a result matching our session
            if let Some(result_msg) = messages.iter().find(|m| {
                m["session_id"].as_str() == Some(&session_id)
                    && m["from_agent"].as_str() == Some(agent_id)
                    && m["kind"].as_str() == Some("result")
            }) {
                let msg_id = result_msg["id"].as_i64().unwrap_or(0);
                let payload_str = result_msg["payload"].as_str().unwrap_or("{}");

                // ACK the message
                self.ack_messages(&[msg_id]).await;

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
