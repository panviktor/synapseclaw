//! Port: provider value objects — domain-owned types for LLM interaction.
//!
//! These types model the contract between the application core and LLM
//! providers. The `Provider` trait itself stays in `synapse_adapters`
//! because its streaming methods depend on `reqwest` and `futures_util`,
//! but all data types it operates on are domain-owned.

use crate::config::schema::CapabilityLane;
use crate::domain::message::ChatMessage;
use crate::ports::tool::ToolSpec;
use serde::{Deserialize, Serialize};

// ── Value Objects ────────────────────────────────────────────────────

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Raw token counts from a single LLM API response.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Tokens served from the provider's prompt cache (Anthropic `cache_read_input_tokens`,
    /// OpenAI `prompt_tokens_details.cached_tokens`).
    pub cached_input_tokens: Option<u64>,
}

/// Generated media artifact kind exposed through the unified provider contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaArtifactKind {
    Image,
    Audio,
    Video,
    Music,
}

impl MediaArtifactKind {
    pub fn marker_label(self) -> &'static str {
        match self {
            Self::Image => "IMAGE",
            Self::Audio => "AUDIO",
            Self::Video => "VIDEO",
            Self::Music => "MUSIC",
        }
    }
}

/// Generated provider artifact with provider-specific extraction kept outside
/// the domain core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaArtifact {
    pub kind: MediaArtifactKind,
    #[serde(flatten)]
    pub locator: MediaArtifactLocator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "locator", rename_all = "snake_case")]
pub enum MediaArtifactLocator {
    Uri { uri: String },
    ProviderFile { file: ProviderFileRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFileRef {
    pub provider: String,
    pub file_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default)]
    pub downloadable: bool,
}

impl MediaArtifact {
    pub fn new(kind: MediaArtifactKind, uri: impl Into<String>) -> Self {
        Self::from_uri(kind, uri)
    }

    pub fn from_uri(kind: MediaArtifactKind, uri: impl Into<String>) -> Self {
        Self {
            kind,
            locator: MediaArtifactLocator::Uri { uri: uri.into() },
            mime_type: None,
            label: None,
        }
    }

    pub fn from_provider_file(kind: MediaArtifactKind, file: ProviderFileRef) -> Self {
        Self {
            kind,
            locator: MediaArtifactLocator::ProviderFile { file },
            mime_type: None,
            label: None,
        }
    }

    pub fn uri(&self) -> Option<&str> {
        match &self.locator {
            MediaArtifactLocator::Uri { uri } => Some(uri.as_str()),
            MediaArtifactLocator::ProviderFile { file } => file.uri.as_deref(),
        }
    }

    pub fn locator_key(&self) -> String {
        match &self.locator {
            MediaArtifactLocator::Uri { uri } => format!("uri:{uri}"),
            MediaArtifactLocator::ProviderFile { file } => {
                format!("provider_file:{}:{}", file.provider, file.file_id)
            }
        }
    }

    pub fn marker(&self) -> Option<String> {
        match &self.locator {
            MediaArtifactLocator::Uri { uri } => {
                let uri = uri.trim();
                (!uri.is_empty()).then(|| format!("[{}:{}]", self.kind.marker_label(), uri))
            }
            MediaArtifactLocator::ProviderFile { file } => {
                if let Some(uri) = file
                    .uri
                    .as_deref()
                    .map(str::trim)
                    .filter(|uri| !uri.is_empty())
                {
                    return Some(format!("[{}:{}]", self.kind.marker_label(), uri));
                }
                let provider = file.provider.trim();
                let file_id = file.file_id.trim();
                (!provider.is_empty() && !file_id.is_empty()).then(|| {
                    format!(
                        "[{}:provider_file:{provider}:{file_id}]",
                        self.kind.marker_label()
                    )
                })
            }
        }
    }
}

/// An LLM response that may contain text, tool calls, or both.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// Text content of the response (may be empty if only tool calls).
    pub text: Option<String>,
    /// Tool calls requested by the LLM.
    pub tool_calls: Vec<ToolCall>,
    /// Generated media artifacts, if the provider returned structured output.
    ///
    /// Adapters own provider-specific extraction. The domain only carries the
    /// normalized artifact contract and keeps text-marker compatibility via
    /// `MediaArtifact::marker`.
    pub media_artifacts: Vec<MediaArtifact>,
    /// Token usage reported by the provider, if available.
    pub usage: Option<TokenUsage>,
    /// Raw reasoning/thinking content from thinking models (e.g. DeepSeek-R1,
    /// Kimi K2.5, GLM-4.7). Preserved as an opaque pass-through so it can be
    /// sent back in subsequent API requests — some providers reject tool-call
    /// history that omits this field.
    pub reasoning_content: Option<String>,
}

impl ChatResponse {
    /// True when the LLM wants to invoke at least one tool.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    pub fn has_media_artifacts(&self) -> bool {
        !self.media_artifacts.is_empty()
    }

    /// Convenience: return text content or empty string.
    pub fn text_or_empty(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

/// Request payload for provider chat calls.
#[derive(Debug, Clone, Copy)]
pub struct ChatRequest<'a> {
    pub messages: &'a [ChatMessage],
    pub tools: Option<&'a [ToolSpec]>,
}

/// A tool result to feed back to the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: String,
}

/// A message in a multi-turn conversation, including tool interactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ConversationMessage {
    /// Regular chat message (system, user, assistant).
    Chat(ChatMessage),
    /// Tool calls from the assistant (stored for history fidelity).
    AssistantToolCalls {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
        /// Raw reasoning content from thinking models, preserved for round-trip
        /// fidelity with provider APIs that require it.
        reasoning_content: Option<String>,
        /// Structured generated media returned on the same assistant step.
        #[serde(default)]
        media_artifacts: Vec<MediaArtifact>,
    },
    /// Results of tool executions, fed back to the LLM.
    ToolResults(Vec<ToolResultMessage>),
}

/// Provider capabilities declaration.
///
/// Describes what features a provider supports, enabling intelligent
/// adaptation of tool calling modes and request formatting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Whether the provider supports native tool calling via API primitives.
    pub native_tool_calling: bool,
    /// Whether the provider supports vision / image inputs.
    pub vision: bool,
    /// Whether the provider supports prompt caching.
    pub prompt_caching: bool,
}

/// Structured error returned when a requested capability is not supported.
#[derive(Debug, Clone)]
pub struct ProviderCapabilityError {
    pub provider: String,
    pub capability: ProviderCapabilityRequirement,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCapabilityRequirement {
    NativeToolCalling,
    VisionInput,
    Lane(CapabilityLane),
}

impl ProviderCapabilityRequirement {
    pub fn repair_lane(&self) -> Option<CapabilityLane> {
        match self {
            Self::VisionInput => Some(CapabilityLane::MultimodalUnderstanding),
            Self::Lane(lane) => Some(*lane),
            Self::NativeToolCalling => None,
        }
    }
}

impl std::fmt::Display for ProviderCapabilityRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeToolCalling => write!(f, "native_tool_calling"),
            Self::VisionInput => write!(f, "vision_input"),
            Self::Lane(lane) => write!(f, "{}", lane.as_str()),
        }
    }
}

impl std::fmt::Display for ProviderCapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "provider_capability_error provider={} capability={} message={}",
            self.provider, self.capability, self.message
        )
    }
}

impl std::error::Error for ProviderCapabilityError {}
