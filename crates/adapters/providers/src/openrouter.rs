use crate::multimodal;
use crate::traits::ToolSpec;
use crate::traits::{
    ChatMessage, ChatRequest as ProviderChatRequest, ChatResponse as ProviderChatResponse,
    Provider, ProviderCapabilities, TokenUsage, ToolCall as ProviderToolCall,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use synapse_domain::config::model_catalog;

pub struct OpenRouterProvider {
    credential: Option<String>,
    reasoning_enabled: Option<bool>,
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OpenRouterReasoningOptions>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: MessageContent,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Parts(Vec<MessagePart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MessagePart {
    Text { text: String },
    ImageUrl { image_url: ImageUrlPart },
}

#[derive(Debug, Serialize)]
struct ImageUrlPart {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ApiChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

#[derive(Debug, Serialize)]
struct NativeChatRequest {
    model: String,
    messages: Vec<NativeMessage>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OpenRouterReasoningOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<NativeToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct OpenRouterReasoningOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct NativeMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<NativeToolCall>>,
    /// Raw reasoning content from thinking models; pass-through for providers
    /// that require it in assistant tool-call history messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Serialize)]
struct NativeToolSpec {
    #[serde(rename = "type")]
    kind: String,
    function: NativeToolFunctionSpec,
}

#[derive(Debug, Serialize)]
struct NativeToolFunctionSpec {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct NativeToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    function: NativeFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct NativeFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct NativeChatResponse {
    choices: Vec<NativeChoice>,
    #[serde(default)]
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
struct UsageInfo {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct NativeChoice {
    message: NativeResponseMessage,
}

#[derive(Debug, Deserialize)]
struct NativeResponseMessage {
    #[serde(default)]
    content: Option<String>,
    /// Reasoning/thinking models may return output in `reasoning_content`.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// OpenRouter exposes normalized reasoning text as `reasoning`; keep
    /// mapping it into the shared provider response field.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<NativeToolCall>>,
}

impl OpenRouterProvider {
    pub fn new(credential: Option<&str>) -> Self {
        Self::new_with_reasoning(credential, None, None)
    }

    pub fn new_with_reasoning(
        credential: Option<&str>,
        reasoning_enabled: Option<bool>,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            credential: credential.map(ToString::to_string),
            reasoning_enabled,
            reasoning_effort,
        }
    }

    fn reasoning_options(&self, model: &str) -> Option<OpenRouterReasoningOptions> {
        if self.reasoning_enabled == Some(false) {
            return Some(OpenRouterReasoningOptions {
                enabled: Some(false),
                effort: None,
            });
        }

        let policy = model_catalog::model_request_policy("openrouter", model)?;
        let effort = self
            .reasoning_effort
            .as_deref()
            .and_then(|requested| policy.resolve_reasoning_effort(requested));

        match (self.reasoning_enabled, effort) {
            (Some(false), _) => Some(OpenRouterReasoningOptions {
                enabled: Some(false),
                effort: None,
            }),
            (Some(true), Some(effort)) => Some(OpenRouterReasoningOptions {
                enabled: Some(true),
                effort: Some(effort),
            }),
            (Some(true), None) => Some(OpenRouterReasoningOptions {
                enabled: Some(true),
                effort: None,
            }),
            (None, Some(effort)) => Some(OpenRouterReasoningOptions {
                enabled: None,
                effort: Some(effort),
            }),
            (None, None) => None,
        }
    }

    fn convert_tools(tools: Option<&[ToolSpec]>) -> Option<Vec<NativeToolSpec>> {
        let items = tools?;
        if items.is_empty() {
            return None;
        }
        Some(
            items
                .iter()
                .map(|tool| NativeToolSpec {
                    kind: "function".to_string(),
                    function: NativeToolFunctionSpec {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: tool.parameters.clone(),
                    },
                })
                .collect(),
        )
    }

    fn convert_messages(messages: &[ChatMessage]) -> Vec<NativeMessage> {
        let converted =
            messages
                .iter()
                .map(|m| {
                    if m.role == "assistant" {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&m.content) {
                            if let Some(tool_calls_value) = value.get("tool_calls") {
                                if let Ok(parsed_calls) =
                                    serde_json::from_value::<Vec<ProviderToolCall>>(
                                        tool_calls_value.clone(),
                                    )
                                {
                                    let tool_calls = parsed_calls
                                        .into_iter()
                                        .map(|tc| NativeToolCall {
                                            id: Some(tc.id),
                                            kind: Some("function".to_string()),
                                            function: NativeFunctionCall {
                                                name: tc.name,
                                                arguments: tc.arguments,
                                            },
                                        })
                                        .collect::<Vec<_>>();
                                    let content = value
                                        .get("content")
                                        .and_then(serde_json::Value::as_str)
                                        .map(|value| MessageContent::Text(value.to_string()));
                                    let reasoning_content = value
                                        .get("reasoning_content")
                                        .and_then(serde_json::Value::as_str)
                                        .map(ToString::to_string);
                                    return NativeMessage {
                                        role: "assistant".to_string(),
                                        content,
                                        tool_call_id: None,
                                        tool_calls: Some(tool_calls),
                                        reasoning_content,
                                    };
                                }
                            }
                        }
                    }

                    if m.role == "tool" {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&m.content) {
                            let tool_call_id = value
                                .get("tool_call_id")
                                .and_then(serde_json::Value::as_str)
                                .map(ToString::to_string);
                            let content = value
                                .get("content")
                                .and_then(serde_json::Value::as_str)
                                .map(|value| MessageContent::Text(value.to_string()))
                                .or_else(|| Some(MessageContent::Text(m.content.clone())));
                            return NativeMessage {
                                role: "tool".to_string(),
                                content,
                                tool_call_id,
                                tool_calls: None,
                                reasoning_content: None,
                            };
                        }
                    }

                    NativeMessage {
                        role: m.role.clone(),
                        content: Some(Self::to_message_content(&m.role, &m.content)),
                        tool_call_id: None,
                        tool_calls: None,
                        reasoning_content: None,
                    }
                })
                .collect::<Vec<_>>();

        Self::normalize_system_messages_first(converted)
    }

    fn normalize_system_messages_first(messages: Vec<NativeMessage>) -> Vec<NativeMessage> {
        let mut system_parts = Vec::new();
        let mut rest_messages = Vec::new();
        for message in messages {
            if message.role == "system" {
                if let Some(content) = message.content {
                    system_parts.push(Self::message_content_to_text(content));
                }
            } else {
                rest_messages.push(message);
            }
        }

        if system_parts.is_empty() {
            return rest_messages;
        }

        let mut normalized = vec![NativeMessage {
            role: "system".to_string(),
            content: Some(MessageContent::Text(system_parts.join("\n\n"))),
            tool_call_id: None,
            tool_calls: None,
            reasoning_content: None,
        }];
        normalized.extend(rest_messages);
        normalized
    }

    fn message_content_to_text(content: MessageContent) -> String {
        match content {
            MessageContent::Text(value) => value,
            MessageContent::Parts(parts) => parts
                .into_iter()
                .filter_map(|part| match part {
                    MessagePart::Text { text } => Some(text),
                    MessagePart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    fn to_message_content(role: &str, content: &str) -> MessageContent {
        if role != "user" {
            return MessageContent::Text(content.to_string());
        }

        let (cleaned_text, image_refs) = multimodal::parse_image_markers(content);
        if image_refs.is_empty() {
            return MessageContent::Text(content.to_string());
        }

        let mut parts = Vec::with_capacity(image_refs.len() + 1);
        let trimmed_text = cleaned_text.trim();
        if !trimmed_text.is_empty() {
            parts.push(MessagePart::Text {
                text: trimmed_text.to_string(),
            });
        }

        for image_ref in image_refs {
            parts.push(MessagePart::ImageUrl {
                image_url: ImageUrlPart { url: image_ref },
            });
        }

        MessageContent::Parts(parts)
    }

    fn parse_native_response(
        message: NativeResponseMessage,
    ) -> anyhow::Result<ProviderChatResponse> {
        let reasoning_content = message.reasoning_content.or(message.reasoning);
        let tool_calls = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let id = tc.id.filter(|id| !id.trim().is_empty()).ok_or_else(|| {
                    anyhow::anyhow!("OpenRouter native tool call missing call id")
                })?;
                Ok(ProviderToolCall {
                    id,
                    name: tc.function.name,
                    arguments: tc.function.arguments,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(ProviderChatResponse {
            text: message.content,
            tool_calls,
            usage: None,
            reasoning_content,
        })
    }

    fn http_client(&self) -> Client {
        crate::proxy::build_runtime_proxy_client_with_timeouts("provider.openrouter", 120, 10)
    }

    async fn decode_chat_response<T: DeserializeOwned>(
        response: reqwest::Response,
        label: &str,
    ) -> anyhow::Result<T> {
        let payload: serde_json::Value = response.json().await.map_err(|error| {
            anyhow::anyhow!("failed to decode OpenRouter {label} response body as JSON: {error}")
        })?;

        if let Some(error) = payload.get("error") {
            return Err(anyhow::anyhow!(
                "OpenRouter API error: {}",
                Self::format_error_payload(error)
            ));
        }

        serde_json::from_value(payload).map_err(|error| {
            anyhow::anyhow!("failed to decode OpenRouter {label} response schema: {error}")
        })
    }

    fn format_error_payload(error: &serde_json::Value) -> String {
        let message = error.get("message").and_then(serde_json::Value::as_str);
        let code = error.get("code").and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        });

        match (message, code) {
            (Some(message), Some(code)) => format!("{message} (code: {code})"),
            (Some(message), None) => message.to_string(),
            _ => error.to_string(),
        }
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            native_tool_calling: true,
            vision: true,
            prompt_caching: false,
        }
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        // Hit a lightweight endpoint to establish TLS + HTTP/2 connection pool.
        // This prevents the first real chat request from timing out on cold start.
        if let Some(credential) = self.credential.as_ref() {
            self.http_client()
                .get("https://openrouter.ai/api/v1/auth/key")
                .header("Authorization", format!("Bearer {credential}"))
                .send()
                .await?
                .error_for_status()?;
        }
        Ok(())
    }

    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let credential = self.credential.as_ref()
            .ok_or_else(|| anyhow::anyhow!("OpenRouter API key not set. Run `synapseclaw onboard` or set OPENROUTER_API_KEY env var."))?;

        let mut messages = Vec::new();

        if let Some(sys) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: MessageContent::Text(sys.to_string()),
            });
        }

        messages.push(Message {
            role: "user".to_string(),
            content: Self::to_message_content("user", message),
        });

        let request = ChatRequest {
            model: model.to_string(),
            messages,
            temperature,
            reasoning: self.reasoning_options(model),
        };

        let response = self
            .http_client()
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {credential}"))
            .header("HTTP-Referer", "https://github.com/panviktor/synapseclaw")
            .header("X-Title", "SynapseClaw")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(super::api_error("OpenRouter", response).await);
        }

        let chat_response: ApiChatResponse =
            Self::decode_chat_response(response, "chat_with_system").await?;

        chat_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow::anyhow!("No response from OpenRouter"))
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let credential = self.credential.as_ref()
            .ok_or_else(|| anyhow::anyhow!("OpenRouter API key not set. Run `synapseclaw onboard` or set OPENROUTER_API_KEY env var."))?;

        let api_messages: Vec<Message> = messages
            .iter()
            .map(|m| Message {
                role: m.role.clone(),
                content: Self::to_message_content(&m.role, &m.content),
            })
            .collect();

        let request = ChatRequest {
            model: model.to_string(),
            messages: api_messages,
            temperature,
            reasoning: self.reasoning_options(model),
        };

        let response = self
            .http_client()
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {credential}"))
            .header("HTTP-Referer", "https://github.com/panviktor/synapseclaw")
            .header("X-Title", "SynapseClaw")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(super::api_error("OpenRouter", response).await);
        }

        let chat_response: ApiChatResponse =
            Self::decode_chat_response(response, "chat_with_history").await?;

        chat_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow::anyhow!("No response from OpenRouter"))
    }

    async fn chat(
        &self,
        request: ProviderChatRequest<'_>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ProviderChatResponse> {
        let credential = self.credential.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
            "OpenRouter API key not set. Run `synapseclaw onboard` or set OPENROUTER_API_KEY env var."
        )
        })?;

        let tools = Self::convert_tools(request.tools);
        let native_request = NativeChatRequest {
            model: model.to_string(),
            messages: Self::convert_messages(request.messages),
            temperature,
            reasoning: self.reasoning_options(model),
            tool_choice: tools.as_ref().map(|_| "auto".to_string()),
            tools,
        };

        let response = self
            .http_client()
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {credential}"))
            .header("HTTP-Referer", "https://github.com/panviktor/synapseclaw")
            .header("X-Title", "SynapseClaw")
            .json(&native_request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(super::api_error("OpenRouter", response).await);
        }

        let native_response: NativeChatResponse =
            Self::decode_chat_response(response, "native chat").await?;
        let usage = native_response.usage.map(|u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cached_input_tokens: None,
        });
        let message = native_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message)
            .ok_or_else(|| anyhow::anyhow!("No response from OpenRouter"))?;
        let mut result = Self::parse_native_response(message)?;
        result.usage = usage;
        Ok(result)
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ProviderChatResponse> {
        let credential = self.credential.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "OpenRouter API key not set. Run `synapseclaw onboard` or set OPENROUTER_API_KEY env var."
            )
        })?;

        // Convert tool JSON values to NativeToolSpec
        let native_tools: Option<Vec<NativeToolSpec>> = if tools.is_empty() {
            None
        } else {
            let specs: Vec<NativeToolSpec> = tools
                .iter()
                .filter_map(|t| {
                    let func = t.get("function")?;
                    Some(NativeToolSpec {
                        kind: "function".to_string(),
                        function: NativeToolFunctionSpec {
                            name: func.get("name")?.as_str()?.to_string(),
                            description: func
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string(),
                            parameters: func
                                .get("parameters")
                                .cloned()
                                .unwrap_or(serde_json::json!({})),
                        },
                    })
                })
                .collect();
            if specs.is_empty() {
                None
            } else {
                Some(specs)
            }
        };

        // Convert ChatMessage to NativeMessage, preserving structured assistant/tool entries
        // when history contains native tool-call metadata.
        let native_messages = Self::convert_messages(messages);

        let native_request = NativeChatRequest {
            model: model.to_string(),
            messages: native_messages,
            temperature,
            reasoning: self.reasoning_options(model),
            tool_choice: native_tools.as_ref().map(|_| "auto".to_string()),
            tools: native_tools,
        };

        let response = self
            .http_client()
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {credential}"))
            .header("HTTP-Referer", "https://github.com/panviktor/synapseclaw")
            .header("X-Title", "SynapseClaw")
            .json(&native_request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(super::api_error("OpenRouter", response).await);
        }

        let native_response: NativeChatResponse =
            Self::decode_chat_response(response, "chat_with_tools").await?;
        let usage = native_response.usage.map(|u| TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cached_input_tokens: None,
        });
        let message = native_response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message)
            .ok_or_else(|| anyhow::anyhow!("No response from OpenRouter"))?;
        let mut result = Self::parse_native_response(message)?;
        result.usage = usage;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{ChatMessage, Provider};

    #[test]
    fn capabilities_report_vision_support() {
        let provider = OpenRouterProvider::new(Some("openrouter-test-credential"));
        let caps = <OpenRouterProvider as Provider>::capabilities(&provider);
        assert!(caps.native_tool_calling);
        assert!(caps.vision);
    }

    #[test]
    fn creates_with_key() {
        let provider = OpenRouterProvider::new(Some("openrouter-test-credential"));
        assert_eq!(
            provider.credential.as_deref(),
            Some("openrouter-test-credential")
        );
    }

    #[test]
    fn creates_without_key() {
        let provider = OpenRouterProvider::new(None);
        assert!(provider.credential.is_none());
    }

    #[test]
    fn reasoning_options_follow_runtime_override() {
        let provider =
            OpenRouterProvider::new_with_reasoning(None, Some(true), Some("high".to_string()));

        assert_eq!(
            provider.reasoning_options("x-ai/grok-4.20"),
            Some(OpenRouterReasoningOptions {
                enabled: Some(true),
                effort: Some("high".to_string()),
            })
        );
        assert_eq!(provider.reasoning_options("unknown/no-policy"), None);
    }

    #[test]
    fn reasoning_disabled_override_wins_over_effort() {
        let provider =
            OpenRouterProvider::new_with_reasoning(None, Some(false), Some("high".to_string()));

        assert_eq!(
            provider.reasoning_options("unknown/no-policy"),
            Some(OpenRouterReasoningOptions {
                enabled: Some(false),
                effort: None,
            })
        );
    }

    #[tokio::test]
    async fn warmup_without_key_is_noop() {
        let provider = OpenRouterProvider::new(None);
        let result = provider.warmup().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn chat_with_system_fails_without_key() {
        let provider = OpenRouterProvider::new(None);
        let result = provider
            .chat_with_system(Some("system"), "hello", "openai/gpt-4o", 0.2)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API key not set"));
    }

    #[tokio::test]
    async fn chat_with_history_fails_without_key() {
        let provider = OpenRouterProvider::new(None);
        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: "be concise".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            },
        ];

        let result = provider
            .chat_with_history(&messages, "anthropic/claude-sonnet-4", 0.7)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API key not set"));
    }

    #[test]
    fn chat_request_serializes_with_system_and_user() {
        let request = ChatRequest {
            model: "anthropic/claude-sonnet-4".into(),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: MessageContent::Text("You are helpful".into()),
                },
                Message {
                    role: "user".into(),
                    content: MessageContent::Text("Summarize this".into()),
                },
            ],
            temperature: 0.5,
            reasoning: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("anthropic/claude-sonnet-4"));
        assert!(json.contains("\"role\":\"system\""));
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"temperature\":0.5"));
    }

    #[test]
    fn chat_request_serializes_history_messages() {
        let messages = [
            ChatMessage {
                role: "assistant".into(),
                content: "Previous answer".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Follow-up".into(),
            },
        ];

        let request = ChatRequest {
            model: "google/gemini-2.5-pro".into(),
            messages: messages
                .iter()
                .map(|msg| Message {
                    role: msg.role.clone(),
                    content: MessageContent::Text(msg.content.clone()),
                })
                .collect(),
            temperature: 0.0,
            reasoning: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"role\":\"assistant\""));
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("google/gemini-2.5-pro"));
    }

    #[test]
    fn native_request_serializes_reasoning_options_when_configured() {
        let request = NativeChatRequest {
            model: "x-ai/grok-4.20".into(),
            messages: vec![NativeMessage {
                role: "user".into(),
                content: Some(MessageContent::Text("think carefully".into())),
                tool_call_id: None,
                tool_calls: None,
                reasoning_content: None,
            }],
            temperature: 0.1,
            reasoning: Some(OpenRouterReasoningOptions {
                enabled: Some(true),
                effort: Some("high".into()),
            }),
            tools: None,
            tool_choice: None,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["reasoning"]["enabled"], true);
        assert_eq!(json["reasoning"]["effort"], "high");
    }

    #[test]
    fn response_deserializes_single_choice() {
        let json = r#"{"choices":[{"message":{"content":"Hi from OpenRouter"}}]}"#;

        let response: ApiChatResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].message.content, "Hi from OpenRouter");
    }

    #[test]
    fn response_deserializes_empty_choices() {
        let json = r#"{"choices":[]}"#;

        let response: ApiChatResponse = serde_json::from_str(json).unwrap();

        assert!(response.choices.is_empty());
    }

    #[tokio::test]
    async fn chat_with_tools_fails_without_key() {
        let provider = OpenRouterProvider::new(None);
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "What is the date?".into(),
        }];
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "Run a shell command",
                "parameters": {"type": "object", "properties": {"command": {"type": "string"}}}
            }
        })];

        let result = provider
            .chat_with_tools(&messages, &tools, "deepseek/deepseek-chat", 0.5)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("API key not set"));
    }

    #[test]
    fn native_response_deserializes_with_tool_calls() {
        let json = r#"{
            "choices":[{
                "message":{
                    "content":null,
                    "tool_calls":[
                        {"id":"call_123","type":"function","function":{"name":"get_price","arguments":"{\"symbol\":\"BTC\"}"}}
                    ]
                }
            }]
        }"#;

        let response: NativeChatResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.choices.len(), 1);
        let message = &response.choices[0].message;
        assert!(message.content.is_none());
        let tool_calls = message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id.as_deref(), Some("call_123"));
        assert_eq!(tool_calls[0].function.name, "get_price");
        assert_eq!(tool_calls[0].function.arguments, "{\"symbol\":\"BTC\"}");
    }

    #[test]
    fn native_response_deserializes_with_text_and_tool_calls() {
        let json = r#"{
            "choices":[{
                "message":{
                    "content":"I'll get that for you.",
                    "tool_calls":[
                        {"id":"call_456","type":"function","function":{"name":"shell","arguments":"{\"command\":\"date\"}"}}
                    ]
                }
            }]
        }"#;

        let response: NativeChatResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.choices.len(), 1);
        let message = &response.choices[0].message;
        assert_eq!(message.content.as_deref(), Some("I'll get that for you."));
        let tool_calls = message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "shell");
    }

    #[test]
    fn parse_native_response_converts_to_chat_response() {
        let message = NativeResponseMessage {
            content: Some("Here you go.".into()),
            reasoning_content: None,
            reasoning: None,
            tool_calls: Some(vec![NativeToolCall {
                id: Some("call_789".into()),
                kind: Some("function".into()),
                function: NativeFunctionCall {
                    name: "file_read".into(),
                    arguments: r#"{"path":"test.txt"}"#.into(),
                },
            }]),
        };

        let response = OpenRouterProvider::parse_native_response(message).unwrap();

        assert_eq!(response.text.as_deref(), Some("Here you go."));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_789");
        assert_eq!(response.tool_calls[0].name, "file_read");
    }

    #[test]
    fn convert_messages_parses_assistant_tool_call_payload() {
        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: r#"{"content":"Using tool","tool_calls":[{"id":"call_abc","name":"shell","arguments":"{\"command\":\"pwd\"}"}]}"#
                .into(),
        }];

        let converted = OpenRouterProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "assistant");
        assert_eq!(
            converted[0]
                .content
                .as_ref()
                .and_then(|content| match content {
                    MessageContent::Text(value) => Some(value.as_str()),
                    MessageContent::Parts(_) => None,
                }),
            Some("Using tool")
        );

        let tool_calls = converted[0].tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id.as_deref(), Some("call_abc"));
        assert_eq!(tool_calls[0].function.name, "shell");
        assert_eq!(tool_calls[0].function.arguments, r#"{"command":"pwd"}"#);
    }

    #[test]
    fn convert_messages_parses_tool_result_payload() {
        let messages = vec![ChatMessage {
            role: "tool".into(),
            content: r#"{"tool_call_id":"call_xyz","content":"done"}"#.into(),
        }];

        let converted = OpenRouterProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "tool");
        assert_eq!(converted[0].tool_call_id.as_deref(), Some("call_xyz"));
        assert_eq!(
            converted[0]
                .content
                .as_ref()
                .and_then(|content| match content {
                    MessageContent::Text(value) => Some(value.as_str()),
                    MessageContent::Parts(_) => None,
                }),
            Some("done")
        );
        assert!(converted[0].tool_calls.is_none());
    }

    #[test]
    fn convert_messages_merges_system_blocks_to_one_front_message_for_openrouter() {
        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: "base policy".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "hi".into(),
            },
            ChatMessage {
                role: "system".into(),
                content: "late context".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "hello".into(),
            },
        ];

        let converted = OpenRouterProvider::convert_messages(&messages);
        let roles = converted
            .iter()
            .map(|message| message.role.as_str())
            .collect::<Vec<_>>();

        assert_eq!(roles, vec!["system", "user", "assistant"]);
        assert_eq!(
            converted[0]
                .content
                .as_ref()
                .and_then(|content| match content {
                    MessageContent::Text(value) => Some(value.as_str()),
                    MessageContent::Parts(_) => None,
                }),
            Some("base policy\n\nlate context")
        );
    }

    #[test]
    fn to_message_content_converts_image_markers_to_openai_parts() {
        let content = "Describe this\n\n[IMAGE:data:image/png;base64,abcd]";
        let value =
            serde_json::to_value(OpenRouterProvider::to_message_content("user", content)).unwrap();
        let parts = value
            .as_array()
            .expect("multimodal content should be an array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "Describe this");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,abcd");
    }

    #[test]
    fn native_response_parses_usage() {
        let json = r#"{
            "choices": [{"message": {"content": "Hello"}}],
            "usage": {"prompt_tokens": 42, "completion_tokens": 15}
        }"#;
        let resp: NativeChatResponse = serde_json::from_str(json).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(42));
        assert_eq!(usage.completion_tokens, Some(15));
    }

    #[test]
    fn native_response_parses_without_usage() {
        let json = r#"{"choices": [{"message": {"content": "Hello"}}]}"#;
        let resp: NativeChatResponse = serde_json::from_str(json).unwrap();
        assert!(resp.usage.is_none());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // reasoning_content pass-through tests
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn parse_native_response_captures_reasoning_content() {
        let message = NativeResponseMessage {
            content: Some("answer".into()),
            reasoning_content: Some("thinking step".into()),
            reasoning: None,
            tool_calls: Some(vec![NativeToolCall {
                id: Some("call_1".into()),
                kind: Some("function".into()),
                function: NativeFunctionCall {
                    name: "shell".into(),
                    arguments: "{}".into(),
                },
            }]),
        };
        let parsed = OpenRouterProvider::parse_native_response(message).unwrap();
        assert_eq!(parsed.reasoning_content.as_deref(), Some("thinking step"));
        assert_eq!(parsed.tool_calls.len(), 1);
    }

    #[test]
    fn parse_native_response_captures_openrouter_reasoning_field() {
        let message = NativeResponseMessage {
            content: Some("answer".into()),
            reasoning_content: None,
            reasoning: Some("normalized thinking".into()),
            tool_calls: None,
        };

        let parsed = OpenRouterProvider::parse_native_response(message).unwrap();

        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("normalized thinking")
        );
    }

    #[test]
    fn parse_native_response_none_reasoning_content_for_normal_model() {
        let message = NativeResponseMessage {
            content: Some("hello".into()),
            reasoning_content: None,
            reasoning: None,
            tool_calls: None,
        };
        let parsed = OpenRouterProvider::parse_native_response(message).unwrap();
        assert!(parsed.reasoning_content.is_none());
    }

    #[test]
    fn native_response_deserializes_reasoning_content() {
        let json = r#"{
            "choices":[{
                "message":{
                    "content":"answer",
                    "reasoning_content":"deep thought",
                    "tool_calls":[
                        {"id":"call_r1","type":"function","function":{"name":"shell","arguments":"{}"}}
                    ]
                }
            }]
        }"#;
        let resp: NativeChatResponse = serde_json::from_str(json).unwrap();
        let message = &resp.choices[0].message;
        assert_eq!(message.reasoning_content.as_deref(), Some("deep thought"));
    }

    #[test]
    fn convert_messages_round_trips_reasoning_content() {
        let history_json = serde_json::json!({
            "content": "I will check",
            "tool_calls": [{
                "id": "tc_1",
                "name": "shell",
                "arguments": "{}"
            }],
            "reasoning_content": "Let me think..."
        });

        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: history_json.to_string(),
        }];
        let native = OpenRouterProvider::convert_messages(&messages);
        assert_eq!(native.len(), 1);
        assert_eq!(
            native[0].reasoning_content.as_deref(),
            Some("Let me think...")
        );
    }

    #[test]
    fn convert_messages_no_reasoning_content_when_absent() {
        let history_json = serde_json::json!({
            "content": "I will check",
            "tool_calls": [{
                "id": "tc_1",
                "name": "shell",
                "arguments": "{}"
            }]
        });

        let messages = vec![ChatMessage {
            role: "assistant".into(),
            content: history_json.to_string(),
        }];
        let native = OpenRouterProvider::convert_messages(&messages);
        assert_eq!(native.len(), 1);
        assert!(native[0].reasoning_content.is_none());
    }

    #[test]
    fn native_message_omits_reasoning_content_when_none() {
        let msg = NativeMessage {
            role: "assistant".to_string(),
            content: Some(MessageContent::Text("hi".into())),
            tool_call_id: None,
            tool_calls: None,
            reasoning_content: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("reasoning_content"));
    }

    #[test]
    fn native_message_includes_reasoning_content_when_some() {
        let msg = NativeMessage {
            role: "assistant".to_string(),
            content: Some(MessageContent::Text("hi".into())),
            tool_call_id: None,
            tool_calls: None,
            reasoning_content: Some("thinking...".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("reasoning_content"));
        assert!(json.contains("thinking..."));
    }
}
