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

#[async_trait]
trait VoiceSynthesizer: Send + Sync {
    async fn synthesize(&self, text: &str, config: &TtsConfig) -> Result<Vec<u8>>;
}

struct ConfiguredVoiceSynthesizer;

#[async_trait]
impl VoiceSynthesizer for ConfiguredVoiceSynthesizer {
    async fn synthesize(&self, text: &str, config: &TtsConfig) -> Result<Vec<u8>> {
        let manager = synapse_channels::TtsManager::new(config)?;
        manager.synthesize(text).await
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

    fn output_extension(config: &TtsConfig) -> &'static str {
        match config.default_format.trim().to_ascii_lowercase().as_str() {
            "ogg" | "opus" => "ogg",
            "wav" => "wav",
            "m4a" | "mp4" | "aac" => "m4a",
            _ => "mp3",
        }
    }

    fn output_mime(extension: &str) -> &'static str {
        match extension {
            "ogg" => "audio/ogg",
            "wav" => "audio/wav",
            "m4a" => "audio/mp4",
            _ => "audio/mpeg",
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

        let tts_config = match crate::channels::lane_selected_tts_config(&self.root_config) {
            Ok(config) if config.enabled => config,
            Ok(_) => return Ok(Self::failure_execution("Voice synthesis is not enabled")),
            Err(error) => {
                return Ok(Self::failure_execution(format!(
                    "Voice synthesis is not ready: {error}"
                )))
            }
        };

        let audio = match self.synthesizer.synthesize(&content, &tts_config).await {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                return Ok(Self::failure_execution(
                    "Voice synthesis returned empty audio",
                ))
            }
            Err(error) => {
                return Ok(Self::failure_execution(format!(
                    "Voice synthesis failed: {error}"
                )))
            }
        };

        let extension = Self::output_extension(&tts_config);
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
        artifact.mime_type = Some(Self::output_mime(extension).to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use synapse_domain::domain::conversation_target::CurrentConversationContext;
    use synapse_domain::domain::turn_defaults::{ResolvedDeliveryTarget, ResolvedTurnDefaults};
    use synapse_domain::ports::turn_defaults_context::InMemoryTurnDefaultsContext;

    struct TestSynthesizer;

    #[async_trait]
    impl VoiceSynthesizer for TestSynthesizer {
        async fn synthesize(&self, _text: &str, _config: &TtsConfig) -> Result<Vec<u8>> {
            Ok(vec![1, 2, 3, 4])
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
            Arc::new(TestSynthesizer),
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
            Arc::new(TestSynthesizer),
        );

        let execution = tool
            .execute_with_facts(json!({"content": "hello"}))
            .await
            .unwrap();

        assert!(execution.result.success);
        assert_eq!(registry.delivered.lock()[0].target_channel, "telegram");
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
            Arc::new(TestSynthesizer),
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
}
