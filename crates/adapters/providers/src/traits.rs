use async_trait::async_trait;
use futures_util::{stream, StreamExt};

// ── Re-exports from domain ──────────────────────────────────────────
//
// All LLM value objects are domain-owned. Adapters re-export them so
// existing `crate::*` import paths continue to work.

pub use synapse_domain::domain::message::ChatMessage;
pub use synapse_domain::ports::provider::{
    ChatRequest, ChatResponse, ConversationMessage, MediaArtifact, MediaArtifactKind,
    MediaArtifactLocator, ProviderCapabilities, ProviderCapabilityError, ProviderFileRef,
    TokenUsage, ToolCall, ToolResultMessage,
};
pub use synapse_domain::ports::tool::ToolSpec;

// ── Streaming types (infra-owned — depend on reqwest/futures) ───────

/// A chunk of content from a streaming response.
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// Text delta for this chunk.
    pub delta: String,
    /// Whether this is the final chunk.
    pub is_final: bool,
    /// Approximate token count for this chunk (estimated).
    pub token_count: usize,
}

impl StreamChunk {
    /// Create a new non-final chunk.
    pub fn delta(text: impl Into<String>) -> Self {
        Self {
            delta: text.into(),
            is_final: false,
            token_count: 0,
        }
    }

    /// Create a final chunk.
    pub fn final_chunk() -> Self {
        Self {
            delta: String::new(),
            is_final: true,
            token_count: 0,
        }
    }

    /// Create an error chunk.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            delta: message.into(),
            is_final: true,
            token_count: 0,
        }
    }

    /// Estimate tokens (rough approximation: ~4 chars per token).
    pub fn with_token_estimate(mut self) -> Self {
        self.token_count = self.delta.len().div_ceil(4);
        self
    }
}

/// Options for streaming chat requests.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamOptions {
    /// Whether to enable streaming (default: true).
    pub enabled: bool,
    /// Whether to include token counts in chunks.
    pub count_tokens: bool,
}

impl StreamOptions {
    /// Create new streaming options with enabled flag.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            count_tokens: false,
        }
    }

    /// Enable token counting.
    pub fn with_token_count(mut self) -> Self {
        self.count_tokens = true;
        self
    }
}

/// Result type for streaming operations.
pub type StreamResult<T> = std::result::Result<T, StreamError>;

/// Errors that can occur during streaming.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(serde_json::Error),

    #[error("Invalid SSE format: {0}")]
    InvalidSse(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Provider-specific tool payload ──────────────────────────────────

/// Provider-specific tool payload formats.
///
/// Different LLM providers require different formats for tool definitions.
/// This enum encapsulates those variations, enabling providers to convert
/// from the unified `ToolSpec` format to their native API requirements.
#[derive(Debug, Clone)]
pub enum ToolsPayload {
    /// Gemini API format (functionDeclarations).
    Gemini {
        function_declarations: Vec<serde_json::Value>,
    },
    /// Anthropic Messages API format (tools with input_schema).
    Anthropic { tools: Vec<serde_json::Value> },
    /// OpenAI Chat Completions API format (tools with function).
    OpenAI { tools: Vec<serde_json::Value> },
}

// ── Provider trait ──────────────────────────────────────────────────

#[async_trait]
pub trait Provider: Send + Sync {
    /// Query provider capabilities.
    ///
    /// Default implementation returns minimal capabilities (no native tool calling).
    /// Providers should override this to declare their actual capabilities.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Convert tool specifications to provider-native format.
    ///
    /// Default implementation returns OpenAI-compatible native tool JSON.
    /// Providers with different native schemas should override this.
    fn convert_tools(&self, tools: &[ToolSpec]) -> ToolsPayload {
        ToolsPayload::OpenAI {
            tools: tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect(),
        }
    }

    /// Simple one-shot chat (single user message, no explicit system prompt).
    ///
    /// This is the preferred API for non-agentic direct interactions.
    async fn simple_chat(
        &self,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        self.chat_with_system(None, message, model, temperature)
            .await
    }

    /// One-shot chat with optional system prompt.
    ///
    /// Kept for compatibility and advanced one-shot prompting.
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String>;

    /// Multi-turn conversation. Default implementation extracts the last user
    /// message and delegates to `chat_with_system`.
    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let system = messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.as_str());
        let last_user = messages
            .iter()
            .rfind(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        self.chat_with_system(system, last_user, model, temperature)
            .await
    }

    /// Structured chat API for agent loop callers.
    async fn chat(
        &self,
        request: ChatRequest<'_>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        if let Some(tools) = request.tools {
            if !tools.is_empty() {
                anyhow::bail!(
                    "Provider does not implement structured chat with native tools; refusing prompt-guided tool fallback"
                );
            }
        }

        let text = self
            .chat_with_history(request.messages, model, temperature)
            .await?;
        Ok(ChatResponse {
            text: Some(text),
            tool_calls: Vec::new(),
            usage: None,
            reasoning_content: None,
            media_artifacts: Vec::new(),
        })
    }

    /// Whether provider supports native tool calls over API.
    fn supports_native_tools(&self) -> bool {
        self.capabilities().native_tool_calling
    }

    /// Whether provider supports multimodal vision input.
    fn supports_vision(&self) -> bool {
        self.capabilities().vision
    }

    /// Warm up the HTTP connection pool (TLS handshake, DNS, HTTP/2 setup).
    /// Default implementation is a no-op; providers with HTTP clients should override.
    async fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Chat with tool definitions for native function calling support.
    /// Providers must override this when they support direct native tool calls.
    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        if !tools.is_empty() {
            anyhow::bail!(
                "Provider does not implement chat_with_tools; refusing prompt-guided tool fallback"
            );
        }

        let text = self.chat_with_history(messages, model, temperature).await?;
        Ok(ChatResponse {
            text: Some(text),
            tool_calls: Vec::new(),
            usage: None,
            reasoning_content: None,
            media_artifacts: Vec::new(),
        })
    }

    /// Whether provider supports streaming responses.
    /// Default implementation returns false.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Streaming chat with optional system prompt.
    /// Returns an async stream of text chunks.
    /// Default implementation falls back to non-streaming chat.
    fn stream_chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
        _options: StreamOptions,
    ) -> stream::BoxStream<'static, StreamResult<StreamChunk>> {
        // Default: return an empty stream (not supported)
        stream::empty().boxed()
    }

    /// Streaming chat with history.
    /// Default implementation falls back to stream_chat_with_system with last user message.
    fn stream_chat_with_history(
        &self,
        _messages: &[ChatMessage],
        _model: &str,
        _temperature: f64,
        _options: StreamOptions,
    ) -> stream::BoxStream<'static, StreamResult<StreamChunk>> {
        // For default implementation, we need to convert to owned strings
        // This is a limitation of the default implementation
        let provider_name = "unknown".to_string();

        // Create a single empty chunk to indicate not supported
        let chunk = StreamChunk::error(format!("{} does not support streaming", provider_name));
        stream::once(async move { Ok(chunk) }).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CapabilityMockProvider;

    #[async_trait]
    impl Provider for CapabilityMockProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                native_tool_calling: true,
                vision: true,
                prompt_caching: false,
            }
        }

        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok("ok".into())
        }
    }

    #[test]
    fn chat_message_constructors() {
        let sys = ChatMessage::system("Be helpful");
        assert_eq!(sys.role, "system");
        assert_eq!(sys.content, "Be helpful");

        let user = ChatMessage::user("Hello");
        assert_eq!(user.role, "user");

        let asst = ChatMessage::assistant("Hi there");
        assert_eq!(asst.role, "assistant");

        let tool = ChatMessage::tool("{}");
        assert_eq!(tool.role, "tool");
    }

    #[test]
    fn chat_response_helpers() {
        let empty = ChatResponse {
            text: None,
            tool_calls: vec![],
            usage: None,
            reasoning_content: None,
            media_artifacts: Vec::new(),
        };
        assert!(!empty.has_tool_calls());
        assert_eq!(empty.text_or_empty(), "");

        let with_tools = ChatResponse {
            text: Some("Let me check".into()),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "shell".into(),
                arguments: "{}".into(),
            }],
            usage: None,
            reasoning_content: None,
            media_artifacts: Vec::new(),
        };
        assert!(with_tools.has_tool_calls());
        assert_eq!(with_tools.text_or_empty(), "Let me check");
    }

    #[test]
    fn token_usage_default_is_none() {
        let usage = TokenUsage::default();
        assert!(usage.input_tokens.is_none());
        assert!(usage.output_tokens.is_none());
    }

    #[test]
    fn chat_response_with_usage() {
        let resp = ChatResponse {
            text: Some("Hello".into()),
            tool_calls: vec![],
            usage: Some(TokenUsage {
                input_tokens: Some(100),
                output_tokens: Some(50),
                cached_input_tokens: None,
            }),
            reasoning_content: None,
            media_artifacts: Vec::new(),
        };
        assert_eq!(resp.usage.as_ref().unwrap().input_tokens, Some(100));
        assert_eq!(resp.usage.as_ref().unwrap().output_tokens, Some(50));
    }

    #[test]
    fn provider_file_media_artifact_marker_is_normalized() {
        let artifact = MediaArtifact::from_provider_file(
            MediaArtifactKind::Audio,
            ProviderFileRef {
                provider: "minimax".into(),
                file_id: "file_123".into(),
                purpose: Some("t2a_async_input".into()),
                uri: None,
                downloadable: false,
            },
        );

        assert_eq!(
            artifact.marker().as_deref(),
            Some("[AUDIO:provider_file:minimax:file_123]")
        );
    }

    #[test]
    fn tool_call_serialization() {
        let tc = ToolCall {
            id: "call_123".into(),
            name: "file_read".into(),
            arguments: r#"{"path":"test.txt"}"#.into(),
        };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(json.contains("call_123"));
        assert!(json.contains("file_read"));
    }

    #[test]
    fn conversation_message_variants() {
        let chat = ConversationMessage::Chat(ChatMessage::user("hi"));
        let json = serde_json::to_string(&chat).unwrap();
        assert!(json.contains("\"type\":\"Chat\""));

        let tool_result = ConversationMessage::ToolResults(vec![ToolResultMessage {
            tool_call_id: "1".into(),
            content: "done".into(),
        }]);
        let json = serde_json::to_string(&tool_result).unwrap();
        assert!(json.contains("\"type\":\"ToolResults\""));
    }

    #[test]
    fn provider_capabilities_default() {
        let caps = ProviderCapabilities::default();
        assert!(!caps.native_tool_calling);
        assert!(!caps.vision);
    }

    #[test]
    fn provider_capabilities_equality() {
        let caps1 = ProviderCapabilities {
            native_tool_calling: true,
            vision: false,
            prompt_caching: false,
        };
        let caps2 = ProviderCapabilities {
            native_tool_calling: true,
            vision: false,
            prompt_caching: false,
        };
        let caps3 = ProviderCapabilities {
            native_tool_calling: false,
            vision: false,
            prompt_caching: false,
        };

        assert_eq!(caps1, caps2);
        assert_ne!(caps1, caps3);
    }

    #[test]
    fn supports_native_tools_reflects_capabilities_default_mapping() {
        let provider = CapabilityMockProvider;
        assert!(provider.supports_native_tools());
    }

    #[test]
    fn supports_vision_reflects_capabilities_default_mapping() {
        let provider = CapabilityMockProvider;
        assert!(provider.supports_vision());
    }

    #[test]
    fn tools_payload_variants() {
        // Test Gemini variant
        let gemini = ToolsPayload::Gemini {
            function_declarations: vec![serde_json::json!({"name": "test"})],
        };
        assert!(matches!(gemini, ToolsPayload::Gemini { .. }));

        // Test Anthropic variant
        let anthropic = ToolsPayload::Anthropic {
            tools: vec![serde_json::json!({"name": "test"})],
        };
        assert!(matches!(anthropic, ToolsPayload::Anthropic { .. }));

        // Test OpenAI variant
        let openai = ToolsPayload::OpenAI {
            tools: vec![serde_json::json!({"type": "function"})],
        };
        assert!(matches!(openai, ToolsPayload::OpenAI { .. }));
    }

    // Mock provider for testing.
    struct MockProvider {
        supports_native: bool,
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn supports_native_tools(&self) -> bool {
            self.supports_native
        }

        async fn chat_with_system(
            &self,
            _system: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            Ok("response".to_string())
        }
    }

    #[test]
    fn provider_convert_tools_default() {
        let provider = MockProvider {
            supports_native: false,
        };

        let tools = vec![ToolSpec {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            runtime_role: None,
        }];

        let payload = provider.convert_tools(&tools);

        let ToolsPayload::OpenAI { tools } = payload else {
            panic!("default payload should be OpenAI-compatible");
        };
        assert_eq!(tools[0]["function"]["name"], "test_tool");
        assert_eq!(tools[0]["function"]["description"], "A test tool");
    }

    #[tokio::test]
    async fn provider_chat_rejects_tools_without_structured_implementation() {
        let provider = MockProvider {
            supports_native: false,
        };

        let tools = vec![ToolSpec {
            name: "shell".to_string(),
            description: "Run commands".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            runtime_role: None,
        }];

        let request = ChatRequest {
            messages: &[ChatMessage::user("Hello")],
            tools: Some(&tools),
        };

        let err = provider.chat(request, "model", 0.7).await.unwrap_err();
        assert!(err.to_string().contains("refusing prompt-guided"));
    }

    #[tokio::test]
    async fn provider_chat_with_tools_default_rejects_non_empty_tools() {
        let provider = MockProvider {
            supports_native: true,
        };

        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "shell", "parameters": {"type": "object"}}
        })];

        let err = provider
            .chat_with_tools(&[ChatMessage::user("Hello")], &tools, "model", 0.7)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refusing prompt-guided"));
    }

    #[tokio::test]
    async fn provider_chat_with_tools_default_allows_empty_tools() {
        let provider = MockProvider {
            supports_native: true,
        };

        let response = provider
            .chat_with_tools(&[ChatMessage::user("Hello")], &[], "model", 0.7)
            .await
            .unwrap();
        assert!(response.text.is_some());
    }

    #[tokio::test]
    async fn provider_chat_without_tools() {
        let provider = MockProvider {
            supports_native: true,
        };

        let request = ChatRequest {
            messages: &[ChatMessage::user("Hello")],
            tools: None,
        };

        let response = provider.chat(request, "model", 0.7).await.unwrap();

        // Should work normally without tools.
        assert!(response.text.is_some());
    }
}
