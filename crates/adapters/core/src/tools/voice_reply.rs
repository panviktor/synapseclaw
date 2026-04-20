use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use synapse_domain::config::schema::{Config, TtsConfig};
use synapse_domain::domain::channel::{
    ChannelCapability, DegradationPolicy, OutboundIntent, RenderableContent,
};
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::domain::tool_fact::{
    DeliveryFact, DeliveryTargetKind, ToolFactPayload, TypedToolFact,
};
use synapse_domain::domain::turn_defaults::TurnDefaultSource;
use synapse_domain::ports::channel_registry::ChannelRegistryPort;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::provider::{MediaArtifact, MediaArtifactKind};
use synapse_domain::ports::tool::{
    Tool, ToolArgumentPolicy, ToolContract, ToolExecution, ToolNonReplayableReason, ToolResult,
    ToolRuntimeRole,
};
use synapse_domain::ports::turn_defaults_context::TurnDefaultsContextPort;
use synapse_providers::reliable::classify_provider_error;

#[async_trait]
trait VoiceSynthesizer: Send + Sync {
    async fn synthesize(&self, text: &str, config: &TtsConfig) -> Result<Vec<u8>>;
    fn supported_voices(&self, config: &TtsConfig) -> Result<(String, Vec<String>)>;
}

struct ConfiguredVoiceSynthesizer;

#[async_trait]
impl VoiceSynthesizer for ConfiguredVoiceSynthesizer {
    async fn synthesize(&self, text: &str, config: &TtsConfig) -> Result<Vec<u8>> {
        let manager = synapse_channels::TtsManager::new(config)?;
        manager.synthesize(text).await
    }

    fn supported_voices(&self, config: &TtsConfig) -> Result<(String, Vec<String>)> {
        let manager = synapse_channels::TtsManager::new(config)?;
        let provider = manager.default_provider().to_string();
        let voices = manager.supported_voices(&provider)?;
        Ok((provider, voices))
    }
}

pub struct VoiceReplyTool {
    root_config: Arc<Config>,
    workspace_dir: PathBuf,
    context: Arc<dyn ConversationContextPort>,
    turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
    channel_registry: Arc<dyn ChannelRegistryPort>,
    synthesizer: Arc<dyn VoiceSynthesizer>,
}

impl VoiceReplyTool {
    pub fn new(
        root_config: Arc<Config>,
        workspace_dir: PathBuf,
        context: Arc<dyn ConversationContextPort>,
        turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
        channel_registry: Arc<dyn ChannelRegistryPort>,
    ) -> Self {
        Self {
            root_config,
            workspace_dir,
            context,
            turn_defaults_context,
            channel_registry,
            synthesizer: Arc::new(ConfiguredVoiceSynthesizer),
        }
    }

    #[cfg(test)]
    fn new_with_synthesizer(
        root_config: Arc<Config>,
        workspace_dir: PathBuf,
        context: Arc<dyn ConversationContextPort>,
        turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
        channel_registry: Arc<dyn ChannelRegistryPort>,
        synthesizer: Arc<dyn VoiceSynthesizer>,
    ) -> Self {
        Self {
            root_config,
            workspace_dir,
            context,
            turn_defaults_context,
            channel_registry,
            synthesizer,
        }
    }

    fn parse_explicit_target_object(
        obj: &serde_json::Value,
    ) -> Result<(ConversationDeliveryTarget, DeliveryTargetKind), String> {
        let channel = obj.get("channel").and_then(|v| v.as_str()).unwrap_or("");
        let recipient = obj.get("recipient").and_then(|v| v.as_str()).unwrap_or("");
        let thread_ref = obj
            .get("thread_ref")
            .and_then(|v| v.as_str())
            .map(String::from);

        if channel.trim().is_empty() || recipient.trim().is_empty() {
            Err("Explicit target requires both 'channel' and 'recipient'".to_string())
        } else {
            let target = ConversationDeliveryTarget::Explicit {
                channel: channel.trim().to_string(),
                recipient: recipient.trim().to_string(),
                thread_ref,
            };
            Ok((target.clone(), DeliveryTargetKind::Explicit(target)))
        }
    }

    fn resolve_target(
        &self,
        args: &serde_json::Value,
    ) -> Result<(ConversationDeliveryTarget, DeliveryTargetKind), String> {
        match args.get("target") {
            Some(serde_json::Value::String(s)) if s == "current_conversation" => self
                .context
                .get_current()
                .map(|ctx| (ctx.to_explicit_target(), DeliveryTargetKind::CurrentConversation))
                .ok_or_else(|| {
                    "No current conversation context available. Use an explicit target with channel and recipient."
                        .to_string()
                }),
            Some(serde_json::Value::String(_)) => Err(
                "Invalid target string. The only string target is 'current_conversation'. For an explicit destination, pass target as an object, not a JSON-encoded string: {\"target\":{\"channel\":\"matrix\",\"recipient\":\"...\"}}."
                    .into(),
            ),
            Some(obj) if obj.is_object() => Self::parse_explicit_target_object(obj),
            None => self
                .turn_defaults_context
                .get_current()
                .and_then(|defaults| defaults.delivery_target)
                .map(|resolved| {
                    let kind = match resolved.source {
                        TurnDefaultSource::DialogueState => {
                            DeliveryTargetKind::Explicit(resolved.target.clone())
                        }
                        TurnDefaultSource::UserProfile => {
                            DeliveryTargetKind::UserProfile(resolved.target.clone())
                        }
                        TurnDefaultSource::ConfiguredChannel => {
                            DeliveryTargetKind::ConfiguredDefault(resolved.target.clone())
                        }
                    };
                    (resolved.target, kind)
                })
                .ok_or_else(|| {
                    "No explicit target provided and no resolved delivery default is available."
                        .to_string()
                }),
            _ => Err(
                "Invalid target. Use 'current_conversation', omit target for a resolved default, or pass an explicit target object: {\"target\":{\"channel\":\"matrix\",\"recipient\":\"...\"}}."
                    .into(),
            ),
        }
    }

    fn provider_output_format(config: &TtsConfig) -> String {
        match config.default_provider.trim().to_ascii_lowercase().as_str() {
            "openai" => "opus".to_string(),
            "groq" => config
                .groq
                .as_ref()
                .map(|cfg| cfg.response_format.as_str())
                .unwrap_or(config.default_format.as_str())
                .to_string(),
            "elevenlabs" | "edge" | "google" | "minimax" => "mp3".to_string(),
            "mistral" => config
                .mistral
                .as_ref()
                .map(|cfg| cfg.response_format.as_str())
                .unwrap_or(config.default_format.as_str())
                .to_string(),
            "xai" => config
                .xai
                .as_ref()
                .map(|cfg| cfg.codec.as_str())
                .unwrap_or(config.default_format.as_str())
                .to_string(),
            _ => config.default_format.clone(),
        }
    }

    fn output_extension(format: &str) -> &'static str {
        match format.trim().to_ascii_lowercase().as_str() {
            "ogg" | "opus" => "ogg",
            "wav" | "wave" => "wav",
            "m4a" | "mp4" => "m4a",
            "aac" => "aac",
            "flac" => "flac",
            "pcm" => "pcm",
            _ => "mp3",
        }
    }

    fn output_mime(format: &str) -> &'static str {
        match format.trim().to_ascii_lowercase().as_str() {
            "ogg" | "opus" => "audio/ogg",
            "wav" | "wave" => "audio/wav",
            "m4a" | "mp4" => "audio/mp4",
            "aac" => "audio/aac",
            "flac" => "audio/flac",
            "pcm" => "audio/L16",
            _ => "audio/mpeg",
        }
    }

    fn resolve_voice_override(
        args: &serde_json::Value,
        provider: &str,
        supported_voices: &[String],
    ) -> Result<Option<String>, String> {
        let Some(raw_voice) = args.get("voice") else {
            return Ok(None);
        };
        let Some(voice) = raw_voice
            .as_str()
            .map(str::trim)
            .filter(|voice| !voice.is_empty())
        else {
            return Err("Voice override must be a non-empty string".to_string());
        };

        let Some(canonical_voice) = supported_voices
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(voice))
            .cloned()
        else {
            return Err(format!(
                "Voice `{voice}` is not supported for TTS provider `{provider}`. Use `voice_list` to inspect available voices."
            ));
        };

        Ok(Some(canonical_voice))
    }

    fn resolved_tts_configs(&self) -> Result<Vec<TtsConfig>, String> {
        match crate::channels::lane_selected_tts_candidate_configs(&self.root_config) {
            Ok(configs) if configs.iter().any(|(_, config)| config.enabled) => {
                Ok(configs.into_iter().map(|(_, config)| config).collect())
            }
            Ok(_) => Err("Voice synthesis is not enabled".to_string()),
            Err(error) => Err(format!("Voice synthesis is not ready: {error}")),
        }
    }

    async fn persist_voice_bytes(
        workspace_dir: &Path,
        extension: &str,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        let dir = workspace_dir.join("voice_out");
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join(format!("voice_{}.{}", uuid::Uuid::new_v4(), extension));
        tokio::fs::write(&path, bytes)
            .await
            .with_context(|| format!("failed to write voice reply {}", path.display()))?;
        Ok(path)
    }

    fn success_execution(
        target: DeliveryTargetKind,
        content_bytes: usize,
        output: impl Into<String>,
    ) -> ToolExecution {
        ToolExecution {
            result: ToolResult {
                success: true,
                output: output.into(),
                error: None,
            },
            facts: vec![TypedToolFact {
                tool_id: "voice_reply".into(),
                payload: ToolFactPayload::Delivery(DeliveryFact {
                    target,
                    content_bytes: Some(content_bytes),
                }),
            }],
        }
    }

    fn failure_execution(output: impl Into<String>) -> ToolExecution {
        ToolExecution {
            result: ToolResult {
                success: false,
                output: output.into(),
                error: None,
            },
            facts: Vec::new(),
        }
    }
}

#[async_trait]
impl Tool for VoiceReplyTool {
    fn name(&self) -> &str {
        "voice_reply"
    }

    fn description(&self) -> &str {
        "Synthesize text into a spoken voice note and send it to the current or configured conversation. Use this when the user asks for a voice/audio reply or when replying in kind to a transcribed voice note. The content is the spoken message itself; do not include delivery-status claims inside the audio. Do not claim a normal text response is a voice message."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Natural-language text to synthesize into the voice note. This should be the spoken reply itself, not a statement that the message was delivered."
                },
                "voice": {
                    "type": "string",
                    "description": "Optional one-message voice override. Use voice_list first when the user asks what voices are available; do not invent provider voice IDs."
                },
                "target": {
                    "description": "Where to send the voice note. Use 'current_conversation' when replying here, omit target only when a resolved runtime default exists, or provide explicit channel and recipient as an object. Do not JSON-encode the object into a string.",
                    "oneOf": [
                        {
                            "type": "string",
                            "enum": ["current_conversation"],
                            "description": "Send to the current conversation"
                        },
                        {
                            "type": "object",
                            "properties": {
                                "channel": { "type": "string", "description": "Channel adapter name (telegram, matrix, discord, etc.)" },
                                "recipient": { "type": "string", "description": "Chat ID, room ID, or channel ID" },
                                "thread_ref": { "type": "string", "description": "Optional thread ID" }
                            },
                            "required": ["channel", "recipient"]
                        }
                    ]
                }
            },
            "required": ["content"]
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::DirectDelivery)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(
            self.runtime_role(),
            ToolNonReplayableReason::ExternalSideEffect,
        )
        .with_arguments(vec![
            ToolArgumentPolicy::sensitive("content").user_private(),
            ToolArgumentPolicy::sensitive("voice").user_private(),
            ToolArgumentPolicy::sensitive("target").user_private(),
        ])
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        Ok(self.execute_with_facts(args).await?.result)
    }

    async fn execute_with_facts(&self, args: serde_json::Value) -> Result<ToolExecution> {
        let content = args
            .get("content")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            return Ok(Self::failure_execution(
                "Voice reply content cannot be empty",
            ));
        }

        let (target, fact_target) = match self.resolve_target(&args) {
            Ok(target) => target,
            Err(output) => return Ok(Self::failure_execution(output)),
        };
        let ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } = target
        else {
            return Ok(Self::failure_execution("Unexpected target state"));
        };

        let caps = self.channel_registry.capabilities(&channel);
        if !caps.contains(&ChannelCapability::Attachments) {
            return Ok(Self::failure_execution(format!(
                "Channel `{channel}` does not support voice attachment delivery"
            )));
        }

        let tts_configs = match self.resolved_tts_configs() {
            Ok(configs) => configs,
            Err(output) => return Ok(Self::failure_execution(output)),
        };

        let mut failures = Vec::new();
        let mut synthesized: Option<(TtsConfig, Vec<u8>)> = None;
        for (index, mut tts_config) in tts_configs.into_iter().enumerate() {
            if args.get("voice").is_some() {
                let (provider, voices) = match self.synthesizer.supported_voices(&tts_config) {
                    Ok(voices) => voices,
                    Err(error) => {
                        failures.push(format!(
                            "candidate={index} provider={} voice_catalog_error={error}",
                            tts_config.default_provider
                        ));
                        continue;
                    }
                };
                match Self::resolve_voice_override(&args, &provider, &voices) {
                    Ok(Some(voice)) => tts_config.default_voice = voice,
                    Ok(None) => {}
                    Err(output) => {
                        failures.push(format!(
                            "candidate={index} provider={} unsupported_voice={output}",
                            tts_config.default_provider
                        ));
                        continue;
                    }
                }
            }

            match self.synthesizer.synthesize(&content, &tts_config).await {
                Ok(bytes) if !bytes.is_empty() => {
                    synthesized = Some((tts_config, bytes));
                    break;
                }
                Ok(_) => {
                    failures.push(format!(
                        "candidate={index} provider={} error=empty_audio",
                        tts_config.default_provider
                    ));
                    continue;
                }
                Err(error) => {
                    let class = classify_provider_error(&error);
                    failures.push(format!(
                        "candidate={index} provider={} kind={} error={}",
                        tts_config.default_provider,
                        class.kind.as_str(),
                        class.detail
                    ));
                    tracing::warn!(
                        %error,
                        failure_kind = class.kind.as_str(),
                        failover_candidate = class.failover_candidate,
                        provider = tts_config.default_provider.as_str(),
                        "Voice synthesis candidate failed"
                    );
                    if class.failover_candidate {
                        continue;
                    }
                    return Ok(Self::failure_execution(format!(
                        "Voice synthesis failed: {error}"
                    )));
                }
            }
        }
        let Some((tts_config, audio)) = synthesized else {
            return Ok(Self::failure_execution(format!(
                "Voice synthesis failed for all candidates: {}",
                failures.join(" | ")
            )));
        };

        let format = Self::provider_output_format(&tts_config);
        let extension = Self::output_extension(&format);
        let path = match Self::persist_voice_bytes(&self.workspace_dir, extension, &audio).await {
            Ok(path) => path,
            Err(error) => return Ok(Self::failure_execution(error.to_string())),
        };
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("voice_reply")
            .to_string();
        let mut artifact = MediaArtifact::new(MediaArtifactKind::Voice, path.display().to_string());
        artifact.mime_type = Some(Self::output_mime(&format).to_string());
        artifact.label = Some(label);

        let mut intent = OutboundIntent::notify_in_thread(
            channel.clone(),
            recipient.clone(),
            thread_ref,
            String::new(),
        )
        .with_media_artifacts(vec![artifact]);
        intent.content = RenderableContent::Text(String::new());
        intent.required_capabilities = vec![ChannelCapability::Attachments];
        intent.degradation_policy = DegradationPolicy::Drop;

        match self.channel_registry.deliver(&intent).await {
            Ok(()) => Ok(Self::success_execution(
                fact_target,
                audio.len(),
                format!("Voice reply sent to {channel}:{recipient}"),
            )),
            Err(error) => Ok(Self::failure_execution(format!(
                "Voice reply delivery failed: {error}"
            ))),
        }
    }
}

pub struct VoiceListTool {
    root_config: Arc<Config>,
    synthesizer: Arc<dyn VoiceSynthesizer>,
}

impl VoiceListTool {
    pub fn new(root_config: Arc<Config>) -> Self {
        Self {
            root_config,
            synthesizer: Arc::new(ConfiguredVoiceSynthesizer),
        }
    }

    #[cfg(test)]
    fn new_with_synthesizer(
        root_config: Arc<Config>,
        synthesizer: Arc<dyn VoiceSynthesizer>,
    ) -> Self {
        Self {
            root_config,
            synthesizer,
        }
    }
}

#[async_trait]
impl Tool for VoiceListTool {
    fn name(&self) -> &str {
        "voice_list"
    }

    fn description(&self) -> &str {
        "List the configured TTS provider and the voice IDs that can be used with voice_reply.voice."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::RuntimeStateInspection)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::replayable(self.runtime_role())
    }

    async fn execute(&self, _args: serde_json::Value) -> Result<ToolResult> {
        let tts_config = match crate::channels::lane_selected_tts_config(&self.root_config) {
            Ok(config) if config.enabled => config,
            Ok(_) => {
                return Ok(ToolResult {
                    success: false,
                    output: "Voice synthesis is not enabled".into(),
                    error: None,
                })
            }
            Err(error) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Voice synthesis is not ready: {error}"),
                    error: None,
                })
            }
        };
        match self.synthesizer.supported_voices(&tts_config) {
            Ok((provider, voices)) => Ok(ToolResult {
                success: true,
                output: json!({
                    "provider": provider,
                    "default_voice": tts_config.default_voice,
                    "voices": voices
                })
                .to_string(),
                error: None,
            }),
            Err(error) => Ok(ToolResult {
                success: false,
                output: format!("Voice catalog is not available: {error}"),
                error: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use synapse_domain::domain::conversation_target::CurrentConversationContext;
    use synapse_domain::domain::turn_defaults::{ResolvedDeliveryTarget, ResolvedTurnDefaults};
    use synapse_domain::ports::turn_defaults_context::InMemoryTurnDefaultsContext;

    #[derive(Default)]
    struct TestSynthesizer {
        voices: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl VoiceSynthesizer for TestSynthesizer {
        async fn synthesize(&self, _text: &str, config: &TtsConfig) -> Result<Vec<u8>> {
            self.voices.lock().push(config.default_voice.clone());
            Ok(vec![1, 2, 3, 4])
        }

        fn supported_voices(&self, config: &TtsConfig) -> Result<(String, Vec<String>)> {
            Ok((
                config.default_provider.clone(),
                vec!["troy".into(), "hannah".into(), "diana".into()],
            ))
        }
    }

    #[derive(Default)]
    struct FailoverSynthesizer {
        attempts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl VoiceSynthesizer for FailoverSynthesizer {
        async fn synthesize(&self, _text: &str, config: &TtsConfig) -> Result<Vec<u8>> {
            self.attempts.lock().push(format!(
                "{}:{}",
                config.default_provider, config.default_voice
            ));
            if config.default_provider == "groq" {
                anyhow::bail!("Groq TTS API error (429): insufficient quota");
            }
            Ok(vec![9, 8, 7])
        }

        fn supported_voices(&self, config: &TtsConfig) -> Result<(String, Vec<String>)> {
            Ok((
                config.default_provider.clone(),
                synapse_channels::TtsManager::new(config)?
                    .supported_voices(&config.default_provider)?,
            ))
        }
    }

    #[derive(Default)]
    struct TestConversationContext {
        current: Mutex<Option<CurrentConversationContext>>,
    }

    impl TestConversationContext {
        fn set(&self, current: CurrentConversationContext) {
            *self.current.lock() = Some(current);
        }
    }

    impl ConversationContextPort for TestConversationContext {
        fn get_current(&self) -> Option<CurrentConversationContext> {
            self.current.lock().clone()
        }

        fn set_current(&self, context: Option<CurrentConversationContext>) {
            *self.current.lock() = context;
        }
    }

    #[derive(Default)]
    struct TestRegistry {
        delivered: Mutex<Vec<OutboundIntent>>,
    }

    #[async_trait]
    impl ChannelRegistryPort for TestRegistry {
        fn has_channel(&self, channel_name: &str) -> bool {
            matches!(channel_name, "matrix" | "telegram")
        }

        fn capabilities(&self, channel_name: &str) -> Vec<ChannelCapability> {
            if self.has_channel(channel_name) {
                vec![ChannelCapability::SendText, ChannelCapability::Attachments]
            } else {
                Vec::new()
            }
        }

        async fn deliver(&self, intent: &OutboundIntent) -> Result<()> {
            self.delivered.lock().push(intent.clone());
            Ok(())
        }
    }

    fn enabled_config(workspace: &Path) -> Config {
        let mut config = Config {
            workspace_dir: workspace.to_path_buf(),
            ..Config::default()
        };
        config.tts.enabled = true;
        config.tts.edge = Some(synapse_domain::config::schema::EdgeTtsConfig {
            binary_path: "edge-tts".into(),
        });
        config
            .model_lanes
            .push(synapse_domain::config::schema::ModelLaneConfig {
                lane: synapse_domain::config::schema::CapabilityLane::SpeechSynthesis,
                candidates: vec![synapse_domain::config::schema::ModelLaneCandidateConfig {
                    provider: "edge".into(),
                    model: "edge-tts".into(),
                    api_key: None,
                    api_key_env: None,
                    dimensions: None,
                    profile: Default::default(),
                }],
            });
        config
    }

    fn enabled_groq_config(workspace: &Path) -> Config {
        let mut config = Config {
            workspace_dir: workspace.to_path_buf(),
            ..Config::default()
        };
        config.tts.enabled = true;
        config.tts.default_provider = "groq".into();
        config.tts.default_voice = "troy".into();
        config
            .model_lanes
            .push(synapse_domain::config::schema::ModelLaneConfig {
                lane: synapse_domain::config::schema::CapabilityLane::SpeechSynthesis,
                candidates: vec![synapse_domain::config::schema::ModelLaneCandidateConfig {
                    provider: "groq".into(),
                    model: "canopylabs/orpheus-v1-english".into(),
                    api_key: Some("test-groq-key".into()),
                    api_key_env: None,
                    dimensions: None,
                    profile: Default::default(),
                }],
            });
        config
    }

    fn enabled_groq_then_openai_config(workspace: &Path) -> Config {
        let mut config = Config {
            workspace_dir: workspace.to_path_buf(),
            ..Config::default()
        };
        config.tts.enabled = true;
        config.tts.default_voice = "hannah".into();
        config
            .model_lanes
            .push(synapse_domain::config::schema::ModelLaneConfig {
                lane: synapse_domain::config::schema::CapabilityLane::SpeechSynthesis,
                candidates: vec![
                    synapse_domain::config::schema::ModelLaneCandidateConfig {
                        provider: "groq".into(),
                        model: "canopylabs/orpheus-v1-english".into(),
                        api_key: Some("test-groq-key".into()),
                        api_key_env: None,
                        dimensions: None,
                        profile: Default::default(),
                    },
                    synapse_domain::config::schema::ModelLaneCandidateConfig {
                        provider: "openai".into(),
                        model: "tts-1".into(),
                        api_key: Some("test-openai-key".into()),
                        api_key_env: None,
                        dimensions: None,
                        profile: Default::default(),
                    },
                ],
            });
        config
    }

    #[tokio::test]
    async fn voice_reply_sends_voice_artifact_to_current_conversation() {
        let tmp = tempfile::tempdir().unwrap();
        let context = Arc::new(TestConversationContext::default());
        context.set(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "conv".into(),
            reply_ref: "!room:example".into(),
            thread_ref: Some("$thread".into()),
            actor_id: "@user:example".into(),
        });
        let defaults = Arc::new(InMemoryTurnDefaultsContext::default());
        let registry = Arc::new(TestRegistry::default());
        let tool = VoiceReplyTool::new_with_synthesizer(
            Arc::new(enabled_config(tmp.path())),
            tmp.path().join("workspace"),
            context,
            defaults,
            registry.clone(),
            Arc::new(TestSynthesizer::default()),
        );

        let execution = tool
            .execute_with_facts(json!({
                "content": "hello from voice",
                "target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(execution.result.success);
        let delivered = registry.delivered.lock();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].target_channel, "matrix");
        assert_eq!(delivered[0].target_recipient, "!room:example");
        assert!(delivered[0].content.as_text().is_empty());
        assert_eq!(delivered[0].media_artifacts.len(), 1);
        assert_eq!(
            delivered[0].media_artifacts[0].kind,
            MediaArtifactKind::Voice
        );
        assert_eq!(
            delivered[0].media_artifacts[0].mime_type.as_deref(),
            Some("audio/mpeg")
        );
        assert!(delivered[0].media_artifacts[0]
            .label
            .as_deref()
            .is_some_and(|label| label.ends_with(".mp3")));
        assert!(matches!(
            execution.facts[0].payload,
            ToolFactPayload::Delivery(DeliveryFact {
                target: DeliveryTargetKind::CurrentConversation,
                content_bytes: Some(4),
            })
        ));
    }

    #[tokio::test]
    async fn voice_reply_uses_resolved_default_when_target_is_omitted() {
        let tmp = tempfile::tempdir().unwrap();
        let context = Arc::new(TestConversationContext::default());
        let defaults = Arc::new(InMemoryTurnDefaultsContext::default());
        defaults.set_current(Some(ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "telegram".into(),
                    recipient: "123".into(),
                    thread_ref: None,
                },
                source: TurnDefaultSource::ConfiguredChannel,
            }),
            ..ResolvedTurnDefaults::default()
        }));
        let registry = Arc::new(TestRegistry::default());
        let tool = VoiceReplyTool::new_with_synthesizer(
            Arc::new(enabled_config(tmp.path())),
            tmp.path().join("workspace"),
            context,
            defaults,
            registry.clone(),
            Arc::new(TestSynthesizer::default()),
        );

        let execution = tool
            .execute_with_facts(json!({"content": "hello"}))
            .await
            .unwrap();

        assert!(execution.result.success);
        assert_eq!(registry.delivered.lock()[0].target_channel, "telegram");
    }

    #[tokio::test]
    async fn voice_reply_fails_over_to_next_tts_candidate_on_quota_error() {
        let tmp = tempfile::tempdir().unwrap();
        let context = Arc::new(TestConversationContext::default());
        context.set(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "conv".into(),
            reply_ref: "!room:example".into(),
            thread_ref: None,
            actor_id: "@user:example".into(),
        });
        let registry = Arc::new(TestRegistry::default());
        let synth = Arc::new(FailoverSynthesizer::default());
        let tool = VoiceReplyTool::new_with_synthesizer(
            Arc::new(enabled_groq_then_openai_config(tmp.path())),
            tmp.path().join("workspace"),
            context,
            Arc::new(InMemoryTurnDefaultsContext::default()),
            registry.clone(),
            synth.clone(),
        );

        let execution = tool
            .execute_with_facts(json!({
                "content": "hello",
                "target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(execution.result.success);
        assert_eq!(registry.delivered.lock().len(), 1);
        assert_eq!(
            synth.attempts.lock().as_slice(),
            ["groq:hannah", "openai:alloy"]
        );
        let delivered = registry.delivered.lock();
        assert_eq!(
            delivered[0].media_artifacts[0].mime_type.as_deref(),
            Some("audio/ogg")
        );
        assert!(delivered[0].media_artifacts[0]
            .label
            .as_deref()
            .is_some_and(|label| label.ends_with(".ogg")));
    }

    #[test]
    fn voice_reply_uses_provider_native_output_format() {
        let mut config = TtsConfig {
            enabled: true,
            default_provider: "openai".into(),
            default_format: "wav".into(),
            ..TtsConfig::default()
        };
        assert_eq!(VoiceReplyTool::provider_output_format(&config), "opus");
        assert_eq!(VoiceReplyTool::output_extension("opus"), "ogg");
        assert_eq!(VoiceReplyTool::output_mime("opus"), "audio/ogg");

        config.default_provider = "groq".into();
        config.groq = Some(synapse_domain::config::schema::GroqTtsConfig {
            api_key: Some("test".into()),
            model: "canopylabs/orpheus-v1-english".into(),
            response_format: "wav".into(),
        });
        assert_eq!(VoiceReplyTool::provider_output_format(&config), "wav");
        assert_eq!(VoiceReplyTool::output_mime("wav"), "audio/wav");
    }

    #[tokio::test]
    async fn voice_reply_fails_when_tts_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let context = Arc::new(TestConversationContext::default());
        context.set(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "conv".into(),
            reply_ref: "!room:example".into(),
            thread_ref: None,
            actor_id: "@user:example".into(),
        });
        let tool = VoiceReplyTool::new_with_synthesizer(
            Arc::new(Config::default()),
            tmp.path().join("workspace"),
            context,
            Arc::new(InMemoryTurnDefaultsContext::default()),
            Arc::new(TestRegistry::default()),
            Arc::new(TestSynthesizer::default()),
        );

        let execution = tool
            .execute_with_facts(json!({
                "content": "hello",
                "target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(!execution.result.success);
        assert!(execution.result.output.contains("not enabled"));
    }

    #[tokio::test]
    async fn voice_reply_accepts_voice_override_from_provider_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let context = Arc::new(TestConversationContext::default());
        context.set(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "conv".into(),
            reply_ref: "!room:example".into(),
            thread_ref: None,
            actor_id: "@user:example".into(),
        });
        let synth = Arc::new(TestSynthesizer::default());
        let tool = VoiceReplyTool::new_with_synthesizer(
            Arc::new(enabled_groq_config(tmp.path())),
            tmp.path().join("workspace"),
            context,
            Arc::new(InMemoryTurnDefaultsContext::default()),
            Arc::new(TestRegistry::default()),
            synth.clone(),
        );

        let execution = tool
            .execute_with_facts(json!({
                "content": "hello",
                "voice": "Hannah",
                "target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(execution.result.success);
        assert_eq!(synth.voices.lock().as_slice(), ["hannah"]);
    }

    #[tokio::test]
    async fn voice_reply_rejects_voice_override_not_in_provider_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let context = Arc::new(TestConversationContext::default());
        context.set(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "conv".into(),
            reply_ref: "!room:example".into(),
            thread_ref: None,
            actor_id: "@user:example".into(),
        });
        let registry = Arc::new(TestRegistry::default());
        let tool = VoiceReplyTool::new_with_synthesizer(
            Arc::new(enabled_groq_config(tmp.path())),
            tmp.path().join("workspace"),
            context,
            Arc::new(InMemoryTurnDefaultsContext::default()),
            registry.clone(),
            Arc::new(TestSynthesizer::default()),
        );

        let execution = tool
            .execute_with_facts(json!({
                "content": "hello",
                "voice": "unknown_voice",
                "target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(!execution.result.success);
        assert!(execution.result.output.contains("Use `voice_list`"));
        assert!(registry.delivered.lock().is_empty());
    }

    #[tokio::test]
    async fn voice_list_reports_runtime_provider_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = VoiceListTool::new_with_synthesizer(
            Arc::new(enabled_groq_config(tmp.path())),
            Arc::new(TestSynthesizer::default()),
        );

        let result = tool.execute(json!({})).await.unwrap();

        assert!(result.success);
        let payload: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(payload["provider"], "groq");
        assert_eq!(payload["default_voice"], "troy");
        assert!(payload["voices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|voice| voice == "hannah"));
    }
}
