use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use synapse_domain::application::services::media_artifact_delivery::{
    media_delivery_decision, tts_output_extension, tts_output_mime, tts_provider_output_format,
    voice_delivery_channel_profiles, MediaDeliveryPolicyInput, VoiceReplyDiagnostics,
    VoiceSynthesisAttemptOutcome, VoiceSynthesisAttemptTrace,
};
use synapse_domain::application::services::voice_preference_service::{
    candidate_matches_preference, read_voice_settings, resolve_voice_preference,
    write_voice_settings, AutoTtsPolicy, VoicePreference, VoicePreferenceScope,
    VoicePreferenceTarget, VoiceSettings,
};
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
use synapse_domain::ports::user_profile_store::UserProfileStorePort;
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
    user_profile_store: Arc<dyn UserProfileStorePort>,
    synthesizer: Arc<dyn VoiceSynthesizer>,
}

impl VoiceReplyTool {
    pub fn new(
        root_config: Arc<Config>,
        workspace_dir: PathBuf,
        context: Arc<dyn ConversationContextPort>,
        turn_defaults_context: Arc<dyn TurnDefaultsContextPort>,
        channel_registry: Arc<dyn ChannelRegistryPort>,
        user_profile_store: Arc<dyn UserProfileStorePort>,
    ) -> Self {
        Self {
            root_config,
            workspace_dir,
            context,
            turn_defaults_context,
            channel_registry,
            user_profile_store,
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
        user_profile_store: Arc<dyn UserProfileStorePort>,
        synthesizer: Arc<dyn VoiceSynthesizer>,
    ) -> Self {
        Self {
            root_config,
            workspace_dir,
            context,
            turn_defaults_context,
            channel_registry,
            user_profile_store,
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

    fn selected_tts_model(config: &TtsConfig) -> Option<String> {
        match config.default_provider.trim().to_ascii_lowercase().as_str() {
            "openai" => config.openai.as_ref().map(|cfg| cfg.model.clone()),
            "groq" => config.groq.as_ref().map(|cfg| cfg.model.clone()),
            "elevenlabs" => config.elevenlabs.as_ref().map(|cfg| cfg.model_id.clone()),
            "google" => Some("cloud-text-to-speech".into()),
            "edge" => Some("edge-tts".into()),
            "minimax" => config.minimax.as_ref().map(|cfg| cfg.model.clone()),
            "mistral" => config.mistral.as_ref().map(|cfg| cfg.model.clone()),
            "xai" => Some("tts".into()),
            _ => None,
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

    fn scoped_preference_for_target(
        &self,
        channel: &str,
        recipient: &str,
    ) -> Option<
        synapse_domain::application::services::voice_preference_service::ResolvedVoicePreference,
    > {
        let global_key = VoicePreferenceTarget::global().storage_key().ok()?;
        let channel_key = VoicePreferenceTarget::channel(channel.to_string())
            .storage_key()
            .ok()?;
        let conversation_key =
            VoicePreferenceTarget::conversation(channel.to_string(), recipient.to_string())
                .storage_key()
                .ok()?;
        resolve_voice_preference(
            self.user_profile_store.load(&global_key),
            self.user_profile_store.load(&channel_key),
            self.user_profile_store.load(&conversation_key),
            channel,
            recipient,
        )
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

        let scoped_preference = if args.get("voice").is_none() {
            self.scoped_preference_for_target(&channel, &recipient)
        } else {
            None
        };

        let tts_configs = match self.resolved_tts_configs() {
            Ok(configs) => configs,
            Err(output) => return Ok(Self::failure_execution(output)),
        };

        let mut failures = Vec::new();
        let mut attempts = Vec::new();
        let mut synthesized: Option<(TtsConfig, Vec<u8>)> = None;
        for (index, mut tts_config) in tts_configs.into_iter().enumerate() {
            let preference = scoped_preference
                .as_ref()
                .map(|resolved| &resolved.preference);
            if let Some(preference) = preference {
                if !candidate_matches_preference(
                    &tts_config,
                    Self::selected_tts_model(&tts_config).as_deref(),
                    preference,
                ) {
                    attempts.push(VoiceSynthesisAttemptTrace {
                        candidate_index: index,
                        provider: tts_config.default_provider.clone(),
                        voice: tts_config.default_voice.clone(),
                        model: Self::selected_tts_model(&tts_config),
                        output_format: tts_provider_output_format(&tts_config),
                        outcome: VoiceSynthesisAttemptOutcome::ProviderError,
                        failure_kind: Some("voice_preference_candidate_mismatch".into()),
                        failure_detail: Some(format!(
                            "candidate does not match scoped voice preference from {}",
                            scoped_preference
                                .as_ref()
                                .map(|resolved| resolved.storage_key.as_str())
                                .unwrap_or("unknown")
                        )),
                        failover_candidate: true,
                    });
                    continue;
                }
            }

            if args.get("voice").is_some()
                || preference.and_then(|pref| pref.voice.as_ref()).is_some()
            {
                let (provider, voices) = match self.synthesizer.supported_voices(&tts_config) {
                    Ok(voices) => voices,
                    Err(error) => {
                        attempts.push(VoiceSynthesisAttemptTrace {
                            candidate_index: index,
                            provider: tts_config.default_provider.clone(),
                            voice: tts_config.default_voice.clone(),
                            model: Self::selected_tts_model(&tts_config),
                            output_format: tts_provider_output_format(&tts_config),
                            outcome: VoiceSynthesisAttemptOutcome::VoiceCatalogError,
                            failure_kind: Some("voice_catalog_error".into()),
                            failure_detail: Some(error.to_string()),
                            failover_candidate: true,
                        });
                        failures.push(format!(
                            "candidate={index} provider={} voice_catalog_error={error}",
                            tts_config.default_provider
                        ));
                        continue;
                    }
                };
                let mut voice_args = args.clone();
                if voice_args.get("voice").is_none() {
                    if let Some(voice) = preference.and_then(|pref| pref.voice.as_ref()) {
                        voice_args["voice"] = serde_json::Value::String(voice.clone());
                    }
                }
                match Self::resolve_voice_override(&voice_args, &provider, &voices) {
                    Ok(Some(voice)) => tts_config.default_voice = voice,
                    Ok(None) => {}
                    Err(output) => {
                        attempts.push(VoiceSynthesisAttemptTrace {
                            candidate_index: index,
                            provider: tts_config.default_provider.clone(),
                            voice: tts_config.default_voice.clone(),
                            model: Self::selected_tts_model(&tts_config),
                            output_format: tts_provider_output_format(&tts_config),
                            outcome: VoiceSynthesisAttemptOutcome::UnsupportedVoice,
                            failure_kind: Some("unsupported_voice".into()),
                            failure_detail: Some(output.clone()),
                            failover_candidate: true,
                        });
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
                    attempts.push(VoiceSynthesisAttemptTrace {
                        candidate_index: index,
                        provider: tts_config.default_provider.clone(),
                        voice: tts_config.default_voice.clone(),
                        model: Self::selected_tts_model(&tts_config),
                        output_format: tts_provider_output_format(&tts_config),
                        outcome: VoiceSynthesisAttemptOutcome::Success,
                        failure_kind: None,
                        failure_detail: None,
                        failover_candidate: false,
                    });
                    synthesized = Some((tts_config, bytes));
                    break;
                }
                Ok(_) => {
                    attempts.push(VoiceSynthesisAttemptTrace {
                        candidate_index: index,
                        provider: tts_config.default_provider.clone(),
                        voice: tts_config.default_voice.clone(),
                        model: Self::selected_tts_model(&tts_config),
                        output_format: tts_provider_output_format(&tts_config),
                        outcome: VoiceSynthesisAttemptOutcome::EmptyAudio,
                        failure_kind: Some("empty_audio".into()),
                        failure_detail: None,
                        failover_candidate: true,
                    });
                    failures.push(format!(
                        "candidate={index} provider={} error=empty_audio",
                        tts_config.default_provider
                    ));
                    continue;
                }
                Err(error) => {
                    let class = classify_provider_error(&error);
                    attempts.push(VoiceSynthesisAttemptTrace {
                        candidate_index: index,
                        provider: tts_config.default_provider.clone(),
                        voice: tts_config.default_voice.clone(),
                        model: Self::selected_tts_model(&tts_config),
                        output_format: tts_provider_output_format(&tts_config),
                        outcome: VoiceSynthesisAttemptOutcome::ProviderError,
                        failure_kind: Some(class.kind.as_str().into()),
                        failure_detail: Some(class.detail.clone()),
                        failover_candidate: class.failover_candidate,
                    });
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

        let format = tts_provider_output_format(&tts_config);
        let extension = tts_output_extension(&format);
        let path = match Self::persist_voice_bytes(&self.workspace_dir, extension, &audio).await {
            Ok(path) => path,
            Err(error) => return Ok(Self::failure_execution(error.to_string())),
        };
        let label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("voice_reply")
            .to_string();
        let output_mime = tts_output_mime(&format).to_string();
        let delivery = media_delivery_decision(MediaDeliveryPolicyInput {
            channel: &channel,
            artifact_kind: MediaArtifactKind::Voice,
            mime_type: Some(output_mime.as_str()),
            file_name: Some(label.as_str()),
            provider_format: Some(format.as_str()),
            normalizer_available: false,
        });
        let mut artifact =
            MediaArtifact::new(delivery.recommended_kind, path.display().to_string());
        artifact.mime_type = Some(output_mime.clone());
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
            Ok(()) => {
                let diagnostics = VoiceReplyDiagnostics {
                    selected_provider: tts_config.default_provider.clone(),
                    selected_voice: tts_config.default_voice.clone(),
                    selected_model: Self::selected_tts_model(&tts_config),
                    selected_format: format,
                    output_mime,
                    output_extension: extension.into(),
                    audio_bytes: audio.len(),
                    target_channel: channel.clone(),
                    delivery,
                    synthesis_attempts: attempts,
                };
                Ok(Self::success_execution(
                    fact_target,
                    audio.len(),
                    json!({
                    "message": format!("Voice reply sent to {channel}:{recipient}"),
                        "preference": scoped_preference,
                        "diagnostics": diagnostics,
                    })
                    .to_string(),
                ))
            }
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

pub struct VoicePreferenceTool {
    root_config: Arc<Config>,
    context: Arc<dyn ConversationContextPort>,
    user_profile_store: Arc<dyn UserProfileStorePort>,
    synthesizer: Arc<dyn VoiceSynthesizer>,
}

impl VoicePreferenceTool {
    pub fn new(
        root_config: Arc<Config>,
        context: Arc<dyn ConversationContextPort>,
        user_profile_store: Arc<dyn UserProfileStorePort>,
    ) -> Self {
        Self {
            root_config,
            context,
            user_profile_store,
            synthesizer: Arc::new(ConfiguredVoiceSynthesizer),
        }
    }

    fn active_tts_configs(&self) -> Result<Vec<TtsConfig>, String> {
        match crate::channels::lane_selected_tts_candidate_configs(&self.root_config) {
            Ok(configs) => {
                let active = configs
                    .into_iter()
                    .map(|(_, config)| config)
                    .filter(|config| config.enabled)
                    .collect::<Vec<_>>();
                if active.is_empty() {
                    Err("Voice synthesis is not enabled".into())
                } else {
                    Ok(active)
                }
            }
            Err(error) => Err(format!("Voice synthesis is not ready: {error}")),
        }
    }

    fn resolve_target(
        &self,
        request: &VoicePreferenceRequest,
    ) -> Result<VoicePreferenceTarget, String> {
        let scope = request.scope.unwrap_or(VoicePreferenceScope::Global);
        let current = self.context.get_current();
        let channel = request
            .channel
            .clone()
            .or_else(|| current.as_ref().map(|ctx| ctx.source_adapter.clone()));
        let recipient = request
            .recipient
            .clone()
            .or_else(|| current.as_ref().map(|ctx| ctx.reply_ref.clone()));

        match scope {
            VoicePreferenceScope::Global => VoicePreferenceTarget::global().normalized(),
            VoicePreferenceScope::Channel => channel
                .map(VoicePreferenceTarget::channel)
                .ok_or_else(|| {
                    "channel scope requires channel or current conversation".to_string()
                })?
                .normalized(),
            VoicePreferenceScope::Conversation => match (channel, recipient) {
                (Some(channel), Some(recipient)) => {
                    VoicePreferenceTarget::conversation(channel, recipient).normalized()
                }
                _ => Err(
                    "conversation scope requires channel and recipient or current conversation"
                        .into(),
                ),
            },
        }
    }

    fn load_settings(&self, target: &VoicePreferenceTarget) -> Result<VoiceSettings, String> {
        let key = target.storage_key()?;
        Ok(read_voice_settings(self.user_profile_store.load(&key)))
    }

    fn save_settings(
        &self,
        target: &VoicePreferenceTarget,
        settings: VoiceSettings,
    ) -> Result<bool, String> {
        let key = target.storage_key()?;
        if let Some(profile) = write_voice_settings(settings) {
            self.user_profile_store
                .upsert(&key, profile)
                .map_err(|error| format!("voice preference update failed: {error}"))?;
            Ok(true)
        } else {
            self.user_profile_store
                .remove(&key)
                .map_err(|error| format!("voice preference clear failed: {error}"))
        }
    }

    fn validate_preference(&self, preference: &VoicePreference) -> Result<(), String> {
        if preference.is_empty() {
            return Err("set requires at least one of provider, model, voice, or format".into());
        }
        let configs = self.active_tts_configs()?;
        let matching = configs
            .into_iter()
            .filter(|config| {
                preference
                    .provider
                    .as_deref()
                    .is_none_or(|provider| config.default_provider.eq_ignore_ascii_case(provider))
            })
            .filter(|config| {
                preference.model.as_deref().is_none_or(|model| {
                    VoiceReplyTool::selected_tts_model(config)
                        .as_deref()
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(model))
                })
            })
            .filter(|config| {
                candidate_matches_preference(
                    config,
                    VoiceReplyTool::selected_tts_model(config).as_deref(),
                    preference,
                )
            })
            .collect::<Vec<_>>();

        if matching.is_empty() {
            return Err(
                "no active speech_synthesis lane candidate matches requested provider/model/format"
                    .into(),
            );
        }

        if let Some(voice) = preference.voice.as_deref() {
            for config in matching {
                let Ok((provider, voices)) = self.synthesizer.supported_voices(&config) else {
                    continue;
                };
                if voices
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(voice))
                {
                    return Ok(());
                }
                if preference.provider.is_some()
                    && provider.eq_ignore_ascii_case(&config.default_provider)
                {
                    return Err(format!(
                        "voice `{voice}` is not supported for provider `{provider}`"
                    ));
                }
            }
            return Err(format!(
                "voice `{voice}` is not supported by any matching speech_synthesis candidate"
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VoicePreferenceAction {
    Get,
    Set,
    Clear,
    List,
}

#[derive(Debug, Deserialize)]
struct VoicePreferenceRequest {
    action: VoicePreferenceAction,
    #[serde(default)]
    scope: Option<VoicePreferenceScope>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    recipient: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    auto_tts_policy: Option<AutoTtsPolicy>,
}

#[async_trait]
impl Tool for VoicePreferenceTool {
    fn name(&self) -> &str {
        "voice_preference"
    }

    fn description(&self) -> &str {
        "Get, set, clear, or list durable voice preferences and auto-TTS policy for global, channel, or conversation scope. Use this when the user asks to remember a voice or spoken-reply setting."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["get", "set", "clear", "list"] },
                "scope": { "type": "string", "enum": ["global", "channel", "conversation"] },
                "channel": { "type": "string" },
                "recipient": { "type": "string" },
                "provider": { "type": "string" },
                "model": { "type": "string" },
                "voice": { "type": "string" },
                "format": { "type": "string", "enum": ["opus", "ogg", "mp3", "m4a", "wav", "flac", "aac", "pcm"] },
                "auto_tts_policy": {
                    "type": "string",
                    "enum": ["inherit", "off", "always", "inbound_voice", "tagged", "channel_default", "conversation_default"]
                }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn runtime_role(&self) -> Option<ToolRuntimeRole> {
        Some(ToolRuntimeRole::ProfileMutation)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(self.runtime_role(), ToolNonReplayableReason::MutatesState)
            .with_arguments(vec![
                ToolArgumentPolicy::sensitive("channel").user_private(),
                ToolArgumentPolicy::sensitive("recipient").user_private(),
                ToolArgumentPolicy::sensitive("voice").user_private(),
            ])
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let request: VoicePreferenceRequest = match serde_json::from_value(args) {
            Ok(request) => request,
            Err(error) => {
                return Ok(ToolResult {
                    success: false,
                    output: format!("Invalid voice preference request: {error}"),
                    error: None,
                })
            }
        };

        if matches!(request.action, VoicePreferenceAction::List) {
            let settings = self
                .user_profile_store
                .list()
                .into_iter()
                .filter(|(key, _)| key.starts_with("voice:"))
                .map(|(key, profile)| {
                    json!({
                        "key": key,
                        "settings": read_voice_settings(Some(profile)),
                    })
                })
                .collect::<Vec<_>>();
            return Ok(ToolResult {
                success: true,
                output: json!({ "voice_preferences": settings }).to_string(),
                error: None,
            });
        }

        let target = match self.resolve_target(&request) {
            Ok(target) => target,
            Err(output) => {
                return Ok(ToolResult {
                    success: false,
                    output,
                    error: None,
                })
            }
        };
        let key = target
            .storage_key()
            .unwrap_or_else(|_| "voice:invalid".into());

        match request.action {
            VoicePreferenceAction::Get => {
                let settings = self.load_settings(&target).unwrap_or_default();
                Ok(ToolResult {
                    success: true,
                    output: json!({
                        "key": key,
                        "target": target,
                        "settings": settings,
                    })
                    .to_string(),
                    error: None,
                })
            }
            VoicePreferenceAction::Set => {
                let mut settings = self.load_settings(&target).unwrap_or_default();
                let preference = VoicePreference {
                    provider: request.provider,
                    model: request.model,
                    voice: request.voice,
                    format: request.format,
                }
                .normalized();
                if !preference.is_empty() {
                    if let Err(output) = self.validate_preference(&preference) {
                        return Ok(ToolResult {
                            success: false,
                            output,
                            error: None,
                        });
                    }
                    settings.preference = Some(preference);
                }
                if let Some(policy) = request.auto_tts_policy {
                    settings.auto_tts_policy = policy;
                }
                if settings.is_empty() {
                    return Ok(ToolResult {
                        success: false,
                        output: "set requires a voice preference field or auto_tts_policy change"
                            .into(),
                        error: None,
                    });
                }
                match self.save_settings(&target, settings.clone()) {
                    Ok(_) => Ok(ToolResult {
                        success: true,
                        output: json!({
                            "status": "ok",
                            "key": key,
                            "target": target,
                            "settings": settings,
                        })
                        .to_string(),
                        error: None,
                    }),
                    Err(output) => Ok(ToolResult {
                        success: false,
                        output,
                        error: None,
                    }),
                }
            }
            VoicePreferenceAction::Clear => {
                match self.save_settings(&target, VoiceSettings::default()) {
                    Ok(removed) => Ok(ToolResult {
                        success: true,
                        output: json!({
                            "status": "ok",
                            "key": key,
                            "removed": removed,
                        })
                        .to_string(),
                        error: None,
                    }),
                    Err(output) => Ok(ToolResult {
                        success: false,
                        output,
                        error: None,
                    }),
                }
            }
            VoicePreferenceAction::List => unreachable!("handled above"),
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
        let tts_configs =
            match crate::channels::lane_selected_tts_candidate_configs(&self.root_config) {
                Ok(configs) if configs.iter().any(|(_, config)| config.enabled) => configs,
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
        let candidates = tts_configs
            .into_iter()
            .filter(|(_, config)| config.enabled)
            .map(|(lane_candidate_index, tts_config)| {
                let format = tts_provider_output_format(&tts_config);
                match self.synthesizer.supported_voices(&tts_config) {
                    Ok((provider, voices)) => json!({
                        "lane_candidate_index": lane_candidate_index,
                        "provider": provider,
                        "model": VoiceReplyTool::selected_tts_model(&tts_config),
                        "default_voice": tts_config.default_voice,
                        "format": format,
                        "extension": tts_output_extension(&format),
                        "mime_type": tts_output_mime(&format),
                        "voices": voices,
                        "error": null,
                    }),
                    Err(error) => json!({
                        "lane_candidate_index": lane_candidate_index,
                        "provider": tts_config.default_provider,
                        "model": VoiceReplyTool::selected_tts_model(&tts_config),
                        "default_voice": tts_config.default_voice,
                        "format": format,
                        "extension": tts_output_extension(&format),
                        "mime_type": tts_output_mime(&format),
                        "voices": [],
                        "error": error.to_string(),
                    }),
                }
            })
            .collect::<Vec<_>>();
        Ok(ToolResult {
            success: true,
            output: json!({
                "candidates": candidates,
                "delivery_profiles": voice_delivery_channel_profiles()
            })
            .to_string(),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use synapse_domain::domain::conversation_target::CurrentConversationContext;
    use synapse_domain::domain::turn_defaults::{ResolvedDeliveryTarget, ResolvedTurnDefaults};
    use synapse_domain::ports::turn_defaults_context::InMemoryTurnDefaultsContext;
    use synapse_domain::ports::user_profile_store::InMemoryUserProfileStore;

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

    fn voice_store() -> Arc<InMemoryUserProfileStore> {
        Arc::new(InMemoryUserProfileStore::new())
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
            voice_store(),
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
        let output: serde_json::Value = serde_json::from_str(&execution.result.output).unwrap();
        assert_eq!(output["diagnostics"]["delivery"]["mode"], "native_voice");
        assert_eq!(
            output["diagnostics"]["delivery"]["compatibility_notes"][1],
            "strict_mobile_clients_may_require_ogg_opus_payload"
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
            voice_store(),
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
            voice_store(),
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
            Some("audio/ogg; codecs=opus")
        );
        assert!(delivered[0].media_artifacts[0]
            .label
            .as_deref()
            .is_some_and(|label| label.ends_with(".ogg")));
        let output: serde_json::Value = serde_json::from_str(&execution.result.output).unwrap();
        assert_eq!(output["diagnostics"]["selected_provider"], "openai");
        assert_eq!(
            output["diagnostics"]["synthesis_attempts"][0]["failure_kind"],
            "quota_exceeded"
        );
        assert_eq!(
            output["diagnostics"]["synthesis_attempts"][1]["outcome"],
            "success"
        );
    }

    #[test]
    fn voice_reply_uses_provider_native_output_format() {
        let mut config = TtsConfig {
            enabled: true,
            default_provider: "openai".into(),
            default_format: "wav".into(),
            ..TtsConfig::default()
        };
        assert_eq!(tts_provider_output_format(&config), "opus");
        assert_eq!(tts_output_extension("opus"), "ogg");
        assert_eq!(tts_output_mime("opus"), "audio/ogg; codecs=opus");

        config.default_provider = "groq".into();
        config.groq = Some(synapse_domain::config::schema::GroqTtsConfig {
            api_key: Some("test".into()),
            model: "canopylabs/orpheus-v1-english".into(),
            response_format: "wav".into(),
        });
        assert_eq!(tts_provider_output_format(&config), "wav");
        assert_eq!(tts_output_mime("wav"), "audio/wav");
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
            voice_store(),
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
            voice_store(),
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
            voice_store(),
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
    async fn voice_preference_tool_sets_global_voice_used_by_voice_reply() {
        let tmp = tempfile::tempdir().unwrap();
        let config = Arc::new(enabled_groq_config(tmp.path()));
        let context = Arc::new(TestConversationContext::default());
        context.set(CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "conv".into(),
            reply_ref: "!room:example".into(),
            thread_ref: None,
            actor_id: "@user:example".into(),
        });
        let store = voice_store();
        let preference_tool =
            VoicePreferenceTool::new(Arc::clone(&config), context.clone(), store.clone());

        let preference_result = preference_tool
            .execute(json!({
                "action": "set",
                "scope": "global",
                "voice": "hannah"
            }))
            .await
            .unwrap();

        assert!(preference_result.success, "{}", preference_result.output);
        let synth = Arc::new(TestSynthesizer::default());
        let reply_tool = VoiceReplyTool::new_with_synthesizer(
            config,
            tmp.path().join("workspace"),
            context,
            Arc::new(InMemoryTurnDefaultsContext::default()),
            Arc::new(TestRegistry::default()),
            store,
            synth.clone(),
        );

        let execution = reply_tool
            .execute_with_facts(json!({
                "content": "hello",
                "target": "current_conversation"
            }))
            .await
            .unwrap();

        assert!(execution.result.success, "{}", execution.result.output);
        assert_eq!(synth.voices.lock().as_slice(), ["hannah"]);
        let output: serde_json::Value = serde_json::from_str(&execution.result.output).unwrap();
        assert_eq!(output["preference"]["source"], "global");
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
        assert_eq!(payload["candidates"][0]["provider"], "groq");
        assert_eq!(payload["candidates"][0]["default_voice"], "troy");
        assert!(payload["candidates"][0]["voices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|voice| voice == "hannah"));
        assert!(payload["delivery_profiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|profile| profile["channel"] == "whatsapp"
                && profile["native_voice_formats"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|format| format == "ogg_opus")));
    }
}
