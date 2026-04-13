use crate::ports::agent_runtime::AgentRuntimeErrorKind;
use crate::ports::provider::MediaArtifact;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputDeliveryHints {
    pub reply_ref: Option<String>,
    pub thread_ref: Option<String>,
    pub already_delivered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentedOutput {
    pub text: String,
    pub media_artifacts: Vec<MediaArtifact>,
    pub tool_summary: String,
    pub tools_used: bool,
    pub failure_kind: Option<AgentRuntimeErrorKind>,
    pub delivery_hints: OutputDeliveryHints,
}

pub struct AssistantOutputPresenter;

impl AssistantOutputPresenter {
    pub fn success(
        text: impl Into<String>,
        media_artifacts: Vec<MediaArtifact>,
        tool_summary: impl Into<String>,
        tools_used: bool,
        delivery_hints: OutputDeliveryHints,
    ) -> PresentedOutput {
        PresentedOutput {
            text: text.into(),
            media_artifacts,
            tool_summary: tool_summary.into(),
            tools_used,
            failure_kind: None,
            delivery_hints,
        }
    }

    pub fn failure(
        text: impl Into<String>,
        kind: AgentRuntimeErrorKind,
        delivery_hints: OutputDeliveryHints,
    ) -> PresentedOutput {
        PresentedOutput {
            text: text.into(),
            media_artifacts: Vec::new(),
            tool_summary: String::new(),
            tools_used: false,
            failure_kind: Some(kind),
            delivery_hints,
        }
    }
}
