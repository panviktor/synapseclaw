//! Pipeline observability events.
//!
//! Phase 4.1 Slice 10: structured events emitted at each pipeline lifecycle
//! point, routed through the existing Observer trait.

use serde::{Deserialize, Serialize};

/// Structured event for pipeline observability.
///
/// These events are emitted by the pipeline runner and can be observed
/// through the Observer trait for logging, metrics, alerting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PipelineEvent {
    PipelineStarted {
        run_id: String,
        pipeline_name: String,
        version: String,
        triggered_by: String,
        depth: u8,
    },
    StepStarted {
        run_id: String,
        step_id: String,
        agent_id: String,
        attempt: u8,
    },
    StepCompleted {
        run_id: String,
        step_id: String,
        agent_id: String,
        duration_ms: i64,
    },
    StepFailed {
        run_id: String,
        step_id: String,
        agent_id: String,
        error: String,
        will_retry: bool,
    },
    StepRetrying {
        run_id: String,
        step_id: String,
        agent_id: String,
        attempt: u8,
        backoff_secs: u64,
    },
    FanOutStarted {
        run_id: String,
        step_id: String,
        branch_count: usize,
    },
    FanOutJoined {
        run_id: String,
        step_id: String,
        completed: usize,
        failed: usize,
    },
    ApprovalRequested {
        run_id: String,
        step_id: String,
        prompt: String,
    },
    ApprovalReceived {
        run_id: String,
        step_id: String,
        approved: bool,
    },
    SubPipelineStarted {
        run_id: String,
        parent_step: String,
        sub_pipeline: String,
        depth: u8,
    },
    SubPipelineCompleted {
        run_id: String,
        sub_pipeline: String,
        success: bool,
    },
    PipelineCompleted {
        run_id: String,
        pipeline_name: String,
        duration_ms: i64,
        step_count: usize,
    },
    PipelineFailed {
        run_id: String,
        pipeline_name: String,
        error: String,
        last_step: String,
    },
    PipelineCancelled {
        run_id: String,
        pipeline_name: String,
    },
    PipelineTimedOut {
        run_id: String,
        pipeline_name: String,
        timeout_secs: u64,
    },
    PipelineReloaded {
        pipeline_name: String,
        old_version: Option<String>,
        new_version: String,
    },
    PipelineReloadFailed {
        pipeline_name: String,
        error: String,
    },
    ToolBlocked {
        run_id: String,
        tool_name: String,
        reason: String,
    },
    MessageRouted {
        content_preview: String,
        rule_name: Option<String>,
        target_agent: String,
        is_fallback: bool,
    },
}

impl PipelineEvent {
    /// Human-readable one-line summary for logging.
    pub fn summary(&self) -> String {
        match self {
            Self::PipelineStarted {
                run_id,
                pipeline_name,
                ..
            } => {
                format!("pipeline '{pipeline_name}' started (run {run_id})")
            }
            Self::StepStarted {
                run_id,
                step_id,
                agent_id,
                attempt,
            } => {
                format!("step '{step_id}' started on {agent_id} (run {run_id}, attempt {attempt})")
            }
            Self::StepCompleted {
                step_id,
                agent_id,
                duration_ms,
                ..
            } => {
                format!("step '{step_id}' completed on {agent_id} ({duration_ms}ms)")
            }
            Self::StepFailed {
                step_id,
                error,
                will_retry,
                ..
            } => {
                format!("step '{step_id}' failed: {error} (retry: {will_retry})")
            }
            Self::StepRetrying {
                step_id,
                attempt,
                backoff_secs,
                ..
            } => {
                format!("step '{step_id}' retrying (attempt {attempt}, backoff {backoff_secs}s)")
            }
            Self::FanOutStarted {
                step_id,
                branch_count,
                ..
            } => {
                format!("fan-out '{step_id}' started ({branch_count} branches)")
            }
            Self::FanOutJoined {
                step_id,
                completed,
                failed,
                ..
            } => {
                format!("fan-out '{step_id}' joined ({completed} ok, {failed} failed)")
            }
            Self::ApprovalRequested { step_id, .. } => {
                format!("approval requested at step '{step_id}'")
            }
            Self::ApprovalReceived {
                step_id, approved, ..
            } => {
                format!("approval received at step '{step_id}': {approved}")
            }
            Self::SubPipelineStarted {
                sub_pipeline,
                depth,
                ..
            } => {
                format!("sub-pipeline '{sub_pipeline}' started (depth {depth})")
            }
            Self::SubPipelineCompleted {
                sub_pipeline,
                success,
                ..
            } => {
                format!("sub-pipeline '{sub_pipeline}' completed (success: {success})")
            }
            Self::PipelineCompleted {
                pipeline_name,
                duration_ms,
                step_count,
                ..
            } => {
                format!(
                    "pipeline '{pipeline_name}' completed ({step_count} steps, {duration_ms}ms)"
                )
            }
            Self::PipelineFailed {
                pipeline_name,
                error,
                ..
            } => {
                format!("pipeline '{pipeline_name}' failed: {error}")
            }
            Self::PipelineCancelled { pipeline_name, .. } => {
                format!("pipeline '{pipeline_name}' cancelled")
            }
            Self::PipelineTimedOut {
                pipeline_name,
                timeout_secs,
                ..
            } => {
                format!("pipeline '{pipeline_name}' timed out ({timeout_secs}s)")
            }
            Self::PipelineReloaded {
                pipeline_name,
                old_version,
                new_version,
            } => {
                format!(
                    "pipeline '{pipeline_name}' reloaded: {} -> {new_version}",
                    old_version.as_deref().unwrap_or("(new)")
                )
            }
            Self::PipelineReloadFailed {
                pipeline_name,
                error,
            } => {
                format!("pipeline '{pipeline_name}' reload failed: {error}")
            }
            Self::ToolBlocked {
                tool_name, reason, ..
            } => {
                format!("tool '{tool_name}' blocked: {reason}")
            }
            Self::MessageRouted {
                target_agent,
                rule_name,
                is_fallback,
                ..
            } => {
                if *is_fallback {
                    format!("message routed to {target_agent} (fallback)")
                } else {
                    format!(
                        "message routed to {target_agent} (rule: {})",
                        rule_name.as_deref().unwrap_or("unknown")
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serialization_roundtrip() {
        let event = PipelineEvent::PipelineStarted {
            run_id: "run-1".into(),
            pipeline_name: "test".into(),
            version: "1.0".into(),
            triggered_by: "operator".into(),
            depth: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("pipeline_started"));
        let _: PipelineEvent = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn summary_messages() {
        let e = PipelineEvent::StepCompleted {
            run_id: "r".into(),
            step_id: "s1".into(),
            agent_id: "a".into(),
            duration_ms: 1500,
        };
        assert!(e.summary().contains("1500ms"));

        let e2 = PipelineEvent::ToolBlocked {
            run_id: "r".into(),
            tool_name: "shell".into(),
            reason: "rate limited".into(),
        };
        assert!(e2.summary().contains("shell"));
    }
}
