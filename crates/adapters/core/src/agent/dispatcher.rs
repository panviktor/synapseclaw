use anyhow::Result;
use serde_json::Value;
use std::fmt::Write;
use synapse_domain::domain::tool_fact::TypedToolFact;
use synapse_domain::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, ToolRepairTrace,
};
use synapse_domain::domain::util::truncate_with_ellipsis;
use synapse_providers::{ChatMessage, ChatResponse, ConversationMessage, ToolResultMessage};

#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    pub name: String,
    pub arguments: Value,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub name: String,
    pub output: String,
    pub success: bool,
    pub tool_call_id: Option<String>,
    pub tool_facts: Vec<TypedToolFact>,
    pub repair_trace: Option<ToolRepairTrace>,
}

pub trait ToolDispatcher: Send + Sync {
    fn parse_response(&self, response: &ChatResponse) -> Result<(String, Vec<ParsedToolCall>)>;
    fn format_results(&self, results: &[ToolExecutionResult]) -> Result<ConversationMessage>;
    fn to_provider_messages(&self, history: &[ConversationMessage]) -> Vec<ChatMessage>;
    fn should_send_tool_specs(&self) -> bool;
}

pub struct NativeToolDispatcher;

fn format_tool_result_content(result: &ToolExecutionResult) -> String {
    if result.success {
        return result.output.clone();
    }

    let Some(trace) = result.repair_trace.as_ref() else {
        return result.output.clone();
    };

    let mut content = result.output.clone();
    let _ = write!(
        content,
        "\n[tool_repair]\nkind={}\naction={}",
        tool_failure_kind_name(trace.failure_kind),
        tool_repair_action_name(trace.suggested_action),
    );
    if let Some(detail) = trace.detail.as_deref() {
        let _ = write!(content, "\ndetail={}", truncate_with_ellipsis(detail, 160));
    }
    content.push_str("\n[/tool_repair]");
    content
}

impl ToolDispatcher for NativeToolDispatcher {
    fn parse_response(&self, response: &ChatResponse) -> Result<(String, Vec<ParsedToolCall>)> {
        let text = response.text.clone().unwrap_or_default();
        let mut calls = Vec::with_capacity(response.tool_calls.len());
        for tc in &response.tool_calls {
            let name = tc.name.trim();
            if name.is_empty() {
                anyhow::bail!("native tool call missing function name");
            }

            let id = tc.id.trim();
            if id.is_empty() {
                anyhow::bail!("native tool call '{name}' missing call id");
            }

            let arguments = serde_json::from_str(&tc.arguments).map_err(|error| {
                anyhow::anyhow!("native tool call '{name}' had invalid JSON arguments: {error}")
            })?;

            calls.push(ParsedToolCall {
                name: name.to_string(),
                arguments,
                tool_call_id: Some(id.to_string()),
            });
        }
        Ok((text, calls))
    }

    fn format_results(&self, results: &[ToolExecutionResult]) -> Result<ConversationMessage> {
        let mut messages = Vec::with_capacity(results.len());
        for result in results {
            let tool_call_id = result
                .tool_call_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("native tool result for '{}' missing call id", result.name)
                })?;
            messages.push(ToolResultMessage {
                tool_call_id: tool_call_id.to_string(),
                content: format_tool_result_content(result),
            });
        }
        Ok(ConversationMessage::ToolResults(messages))
    }

    fn to_provider_messages(&self, history: &[ConversationMessage]) -> Vec<ChatMessage> {
        history
            .iter()
            .flat_map(|msg| match msg {
                ConversationMessage::Chat(chat) => vec![chat.clone()],
                ConversationMessage::AssistantToolCalls {
                    text,
                    tool_calls,
                    reasoning_content,
                } => {
                    let mut payload = serde_json::json!({
                        "content": text,
                        "tool_calls": tool_calls,
                    });
                    if let Some(rc) = reasoning_content {
                        payload["reasoning_content"] = serde_json::json!(rc);
                    }
                    vec![ChatMessage::assistant(payload.to_string())]
                }
                ConversationMessage::ToolResults(results) => results
                    .iter()
                    .map(|result| {
                        ChatMessage::tool(
                            serde_json::json!({
                                "tool_call_id": result.tool_call_id,
                                "content": result.content,
                            })
                            .to_string(),
                        )
                    })
                    .collect(),
            })
            .collect()
    }

    fn should_send_tool_specs(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_dispatcher_roundtrip() {
        let response = ChatResponse {
            text: Some("ok".into()),
            tool_calls: vec![synapse_providers::ToolCall {
                id: "tc1".into(),
                name: "file_read".into(),
                arguments: "{\"path\":\"a.txt\"}".into(),
            }],
            usage: None,
            reasoning_content: None,
        };
        let dispatcher = NativeToolDispatcher;
        let (_, calls) = dispatcher.parse_response(&response).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_call_id.as_deref(), Some("tc1"));

        let msg = dispatcher
            .format_results(&[ToolExecutionResult {
                name: "file_read".into(),
                output: "hello".into(),
                success: true,
                tool_call_id: Some("tc1".into()),
                tool_facts: vec![],
                repair_trace: None,
            }])
            .unwrap();
        match msg {
            ConversationMessage::ToolResults(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].tool_call_id, "tc1");
            }
            _ => panic!("expected tool results"),
        }
    }

    #[test]
    fn native_dispatcher_does_not_recover_text_tool_call_envelopes() {
        let response = ChatResponse {
            text: Some(
                "Checking\n<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"pwd\"}}</tool_call>"
                    .into(),
            ),
            tool_calls: vec![],
            usage: None,
            reasoning_content: None,
        };
        let dispatcher = NativeToolDispatcher;
        let (text, calls) = dispatcher.parse_response(&response).unwrap();
        assert_eq!(
            text,
            "Checking\n<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"pwd\"}}</tool_call>"
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn native_dispatcher_rejects_invalid_argument_json() {
        let response = ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![synapse_providers::ToolCall {
                id: "tc_bad".into(),
                name: "shell".into(),
                arguments: "{not-json".into(),
            }],
            usage: None,
            reasoning_content: None,
        };
        let dispatcher = NativeToolDispatcher;

        let error = dispatcher.parse_response(&response).unwrap_err();

        assert!(error.to_string().contains("invalid JSON arguments"));
    }

    #[test]
    fn native_format_results_rejects_missing_tool_call_id() {
        let dispatcher = NativeToolDispatcher;

        let error = dispatcher
            .format_results(&[ToolExecutionResult {
                name: "shell".into(),
                output: "ok".into(),
                success: true,
                tool_call_id: None,
                tool_facts: vec![],
                repair_trace: None,
            }])
            .unwrap_err();

        assert!(error.to_string().contains("missing call id"));
    }

    #[test]
    fn native_format_results_keeps_tool_call_id() {
        let dispatcher = NativeToolDispatcher;
        let msg = dispatcher
            .format_results(&[ToolExecutionResult {
                name: "shell".into(),
                output: "ok".into(),
                success: true,
                tool_call_id: Some("tc-1".into()),
                tool_facts: vec![],
                repair_trace: None,
            }])
            .unwrap();

        match msg {
            ConversationMessage::ToolResults(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].tool_call_id, "tc-1");
            }
            _ => panic!("expected ToolResults variant"),
        }
    }

    #[test]
    fn native_format_results_includes_tool_repair_footer_for_failures() {
        let dispatcher = NativeToolDispatcher;
        let msg = dispatcher.format_results(&[ToolExecutionResult {
            name: "file_read".into(),
            output: "Error: missing file".into(),
            success: false,
            tool_call_id: Some("tc-2".into()),
            tool_facts: vec![],
            repair_trace: Some(ToolRepairTrace {
                observed_at_unix: 1,
                tool_name: "file_read".into(),
                failure_kind: synapse_domain::domain::tool_repair::ToolFailureKind::MissingResource,
                suggested_action:
                    synapse_domain::domain::tool_repair::ToolRepairAction::AdjustArgumentsOrTarget,
                detail: Some("No such file or directory".into()),
            }),
        }])
        .unwrap();

        match msg {
            ConversationMessage::ToolResults(results) => {
                assert_eq!(results.len(), 1);
                assert!(results[0].content.contains("[tool_repair]"));
                assert!(results[0].content.contains("kind=missing_resource"));
                assert!(results[0]
                    .content
                    .contains("action=adjust_arguments_or_target"));
            }
            _ => panic!("expected ToolResults variant"),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // reasoning_content pass-through tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn native_to_provider_messages_includes_reasoning_content() {
        let dispatcher = NativeToolDispatcher;
        let history = vec![ConversationMessage::AssistantToolCalls {
            text: Some("answer".into()),
            tool_calls: vec![synapse_providers::ToolCall {
                id: "tc_1".into(),
                name: "shell".into(),
                arguments: "{}".into(),
            }],
            reasoning_content: Some("thinking step".into()),
        }];

        let messages = dispatcher.to_provider_messages(&history);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "assistant");

        let payload: serde_json::Value = serde_json::from_str(&messages[0].content).unwrap();
        assert_eq!(payload["reasoning_content"].as_str(), Some("thinking step"));
        assert_eq!(payload["content"].as_str(), Some("answer"));
        assert!(payload["tool_calls"].is_array());
    }

    #[test]
    fn native_to_provider_messages_omits_reasoning_content_when_none() {
        let dispatcher = NativeToolDispatcher;
        let history = vec![ConversationMessage::AssistantToolCalls {
            text: Some("answer".into()),
            tool_calls: vec![synapse_providers::ToolCall {
                id: "tc_1".into(),
                name: "shell".into(),
                arguments: "{}".into(),
            }],
            reasoning_content: None,
        }];

        let messages = dispatcher.to_provider_messages(&history);
        assert_eq!(messages.len(), 1);

        let payload: serde_json::Value = serde_json::from_str(&messages[0].content).unwrap();
        assert!(payload.get("reasoning_content").is_none());
    }
}
