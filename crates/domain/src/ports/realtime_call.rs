//! Port for realtime audio/video call runtimes.
//!
//! Channel capabilities declare whether a transport has an implemented call
//! runtime. This port is the side-effect contract used by adapters that can
//! actually start, speak into, and hang up calls.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeCallKind {
    Audio,
    Video,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeCallDirection {
    Inbound,
    Outbound,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeCallTriggerSource {
    ChatRequest,
    ScheduledJob,
    ApiRequest,
    CliRequest,
    InboundTransport,
    #[default]
    Unknown,
}

impl RealtimeCallTriggerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatRequest => "chat_request",
            Self::ScheduledJob => "scheduled_job",
            Self::ApiRequest => "api_request",
            Self::CliRequest => "cli_request",
            Self::InboundTransport => "inbound_transport",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallOrigin {
    #[serde(default)]
    pub source: RealtimeCallTriggerSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_ref: Option<String>,
}

impl RealtimeCallOrigin {
    pub fn chat_request(
        conversation_id: Option<String>,
        channel: Option<String>,
        recipient: Option<String>,
        thread_ref: Option<String>,
    ) -> Self {
        Self {
            source: RealtimeCallTriggerSource::ChatRequest,
            conversation_id,
            channel,
            recipient,
            thread_ref,
        }
    }

    pub fn scheduled_job() -> Self {
        Self {
            source: RealtimeCallTriggerSource::ScheduledJob,
            ..Self::default()
        }
    }

    pub fn api_request() -> Self {
        Self {
            source: RealtimeCallTriggerSource::ApiRequest,
            ..Self::default()
        }
    }

    pub fn cli_request() -> Self {
        Self {
            source: RealtimeCallTriggerSource::CliRequest,
            ..Self::default()
        }
    }

    pub fn inbound_transport() -> Self {
        Self {
            source: RealtimeCallTriggerSource::InboundTransport,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeCallState {
    #[default]
    Created,
    Ringing,
    Connected,
    Listening,
    Thinking,
    Speaking,
    Ended,
    Failed,
}

impl RealtimeCallState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ended | Self::Failed)
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        if self.is_terminal() {
            return false;
        }
        if matches!(next, Self::Ended | Self::Failed) {
            return true;
        }
        match self {
            Self::Created => matches!(next, Self::Ringing | Self::Connected),
            Self::Ringing => matches!(next, Self::Connected),
            Self::Connected => matches!(next, Self::Listening | Self::Speaking),
            Self::Listening => matches!(next, Self::Thinking | Self::Speaking),
            Self::Thinking => matches!(next, Self::Speaking | Self::Listening),
            Self::Speaking => matches!(next, Self::Listening | Self::Thinking),
            Self::Ended | Self::Failed => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeCallTransitionError {
    pub from: RealtimeCallState,
    pub to: RealtimeCallState,
}

impl fmt::Display for RealtimeCallTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid realtime call state transition: {:?} -> {:?}",
            self.from, self.to
        )
    }
}

impl Error for RealtimeCallTransitionError {}

pub fn validate_realtime_call_transition(
    from: RealtimeCallState,
    to: RealtimeCallState,
) -> Result<(), RealtimeCallTransitionError> {
    if from.can_transition_to(to) {
        Ok(())
    } else {
        Err(RealtimeCallTransitionError { from, to })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallStartRequest {
    pub to: String,
    pub prompt: Option<String>,
    #[serde(default)]
    pub origin: RealtimeCallOrigin,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub agenda: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallStartResult {
    pub channel: String,
    pub call_control_id: String,
    pub call_leg_id: String,
    pub call_session_id: String,
    pub state: RealtimeCallState,
    pub origin: RealtimeCallOrigin,
    pub objective: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallSpeakRequest {
    pub call_control_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallAnswerRequest {
    pub call_control_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallHangupRequest {
    pub call_control_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallActionResult {
    pub channel: String,
    pub call_control_id: String,
    pub status: String,
    pub state: RealtimeCallState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallSessionSnapshot {
    pub channel: String,
    pub kind: RealtimeCallKind,
    pub direction: RealtimeCallDirection,
    pub origin: RealtimeCallOrigin,
    pub objective: Option<String>,
    pub call_control_id: String,
    pub call_leg_id: Option<String>,
    pub call_session_id: Option<String>,
    pub state: RealtimeCallState,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
    pub end_reason: Option<String>,
    pub summary: Option<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    pub message_count: u64,
    pub interruption_count: u64,
    pub last_sequence: Option<u64>,
}

#[async_trait]
pub trait RealtimeCallRuntimePort: Send + Sync {
    fn channel_name(&self) -> &'static str;

    fn supports_call_kind(&self, kind: RealtimeCallKind) -> bool;

    async fn start_audio_call(
        &self,
        request: RealtimeCallStartRequest,
    ) -> anyhow::Result<RealtimeCallStartResult>;

    async fn speak(
        &self,
        request: RealtimeCallSpeakRequest,
    ) -> anyhow::Result<RealtimeCallActionResult>;

    async fn answer(
        &self,
        request: RealtimeCallAnswerRequest,
    ) -> anyhow::Result<RealtimeCallActionResult>;

    async fn hangup(
        &self,
        request: RealtimeCallHangupRequest,
    ) -> anyhow::Result<RealtimeCallActionResult>;

    async fn list_sessions(&self) -> anyhow::Result<Vec<RealtimeCallSessionSnapshot>>;

    async fn get_session(
        &self,
        call_control_id: &str,
    ) -> anyhow::Result<Option<RealtimeCallSessionSnapshot>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_state_machine_accepts_normal_audio_flow() {
        let flow = [
            RealtimeCallState::Created,
            RealtimeCallState::Ringing,
            RealtimeCallState::Connected,
            RealtimeCallState::Listening,
            RealtimeCallState::Thinking,
            RealtimeCallState::Speaking,
            RealtimeCallState::Listening,
            RealtimeCallState::Ended,
        ];

        for window in flow.windows(2) {
            validate_realtime_call_transition(window[0], window[1]).unwrap();
        }
    }

    #[test]
    fn call_state_machine_rejects_terminal_reentry() {
        let error = validate_realtime_call_transition(
            RealtimeCallState::Ended,
            RealtimeCallState::Speaking,
        )
        .unwrap_err();
        assert_eq!(error.from, RealtimeCallState::Ended);
        assert_eq!(error.to, RealtimeCallState::Speaking);
    }

    #[test]
    fn call_state_machine_rejects_skipping_required_session_setup() {
        let error = validate_realtime_call_transition(
            RealtimeCallState::Created,
            RealtimeCallState::Speaking,
        )
        .unwrap_err();
        assert_eq!(error.from, RealtimeCallState::Created);
        assert_eq!(error.to, RealtimeCallState::Speaking);
    }
}
