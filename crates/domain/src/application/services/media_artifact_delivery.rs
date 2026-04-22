use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::schema::TtsConfig;
use crate::domain::channel::ChannelCapability;
use crate::ports::channel_registry::ChannelCapabilityProfile;
use crate::ports::provider::{MediaArtifact, MediaArtifactKind, MediaArtifactLocator};

pub fn artifact_delivery_uri<'a>(transport: &str, artifact: &'a MediaArtifact) -> Result<&'a str> {
    let target = match &artifact.locator {
        MediaArtifactLocator::Uri { uri } => uri.as_str(),
        MediaArtifactLocator::ProviderFile { file } => file.uri.as_deref().ok_or_else(|| {
            anyhow!(
                "{transport} cannot deliver provider file artifact without a URI: provider={} file_id={}",
                file.provider,
                file.file_id
            )
        })?,
    }
    .trim();

    if target.is_empty() {
        bail!("{transport} cannot deliver media artifact with an empty URI");
    }

    Ok(target)
}

pub fn strip_media_artifact_markers(content: &str, artifacts: &[MediaArtifact]) -> String {
    let mut cleaned = content.to_string();
    for marker in artifacts.iter().filter_map(MediaArtifact::marker) {
        cleaned = cleaned.replace(&marker, "");
    }
    cleaned
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaDeliveryMode {
    NativeVoice,
    NativeAudio,
    AudioAttachment,
    FileAttachment,
    RequiresCompatibleProviderFormat,
    RequiresNormalizer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioContainer {
    OggOpus,
    Ogg,
    Mp3,
    Mp4Audio,
    Wav,
    Flac,
    Aac,
    Pcm,
    Unknown,
}

impl AudioContainer {
    pub fn from_media(
        mime_type: Option<&str>,
        file_name: Option<&str>,
        provider_format: Option<&str>,
    ) -> Self {
        if let Some(container) = provider_format.and_then(Self::from_provider_format) {
            return container;
        }

        if let Some(container) = mime_type.and_then(Self::from_mime_type) {
            if container != Self::Ogg {
                return container;
            }
            if mime_type.is_some_and(mime_has_opus_codec) {
                return Self::OggOpus;
            }
            return Self::Ogg;
        }

        file_name
            .and_then(Self::from_file_name)
            .unwrap_or(Self::Unknown)
    }

    pub fn is_ogg_opus_compatible(self) -> bool {
        matches!(self, Self::OggOpus)
    }

    pub fn is_common_native_voice_compatible(self) -> bool {
        matches!(self, Self::OggOpus | Self::Ogg | Self::Mp3 | Self::Mp4Audio)
    }

    pub fn file_extension(self) -> Option<&'static str> {
        match self {
            Self::OggOpus | Self::Ogg => Some("ogg"),
            Self::Mp3 => Some("mp3"),
            Self::Mp4Audio => Some("m4a"),
            Self::Wav => Some("wav"),
            Self::Flac => Some("flac"),
            Self::Aac => Some("aac"),
            Self::Pcm => Some("pcm"),
            Self::Unknown => None,
        }
    }

    fn from_provider_format(format: &str) -> Option<Self> {
        match format.trim().to_ascii_lowercase().as_str() {
            "opus" => Some(Self::OggOpus),
            "ogg" | "oga" => Some(Self::Ogg),
            "mp3" | "mpeg" => Some(Self::Mp3),
            "m4a" | "mp4" => Some(Self::Mp4Audio),
            "wav" | "wave" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "aac" => Some(Self::Aac),
            "pcm" | "l16" => Some(Self::Pcm),
            _ => None,
        }
    }

    fn from_mime_type(raw: &str) -> Option<Self> {
        let parsed = raw.trim().parse::<mime::Mime>().ok()?;
        if parsed.type_() != mime::AUDIO {
            return None;
        }

        match parsed.subtype().as_str() {
            "ogg" | "oga" => Some(Self::Ogg),
            "opus" => Some(Self::OggOpus),
            "mpeg" | "mp3" => Some(Self::Mp3),
            "mp4" | "m4a" => Some(Self::Mp4Audio),
            "wav" | "wave" | "x-wav" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "aac" => Some(Self::Aac),
            "l16" | "pcm" => Some(Self::Pcm),
            _ => Some(Self::Unknown),
        }
    }

    fn from_file_name(file_name: &str) -> Option<Self> {
        let extension = std::path::Path::new(file_name)
            .extension()
            .and_then(|ext| ext.to_str())?
            .to_ascii_lowercase();
        match extension.as_str() {
            "opus" => Some(Self::OggOpus),
            "ogg" | "oga" => Some(Self::Ogg),
            "mp3" => Some(Self::Mp3),
            "m4a" | "mp4" => Some(Self::Mp4Audio),
            "wav" | "wave" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "aac" => Some(Self::Aac),
            "pcm" => Some(Self::Pcm),
            _ => Some(Self::Unknown),
        }
    }
}

pub fn audio_extension_for_media(
    mime_type: Option<&str>,
    file_name: Option<&str>,
    provider_format: Option<&str>,
) -> Option<&'static str> {
    AudioContainer::from_media(mime_type, file_name, provider_format).file_extension()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDeliveryDecision {
    pub channel: String,
    pub artifact_kind: MediaArtifactKind,
    pub mode: MediaDeliveryMode,
    pub audio_container: Option<AudioContainer>,
    pub native_voice: bool,
    pub recommended_kind: MediaArtifactKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_provider_format: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compatibility_notes: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Copy)]
pub struct MediaDeliveryPolicyInput<'a> {
    pub channel: &'a str,
    pub capabilities: &'a [ChannelCapability],
    pub artifact_kind: MediaArtifactKind,
    pub mime_type: Option<&'a str>,
    pub file_name: Option<&'a str>,
    pub provider_format: Option<&'a str>,
    pub normalizer_available: bool,
}

pub fn media_delivery_decision(input: MediaDeliveryPolicyInput<'_>) -> MediaDeliveryDecision {
    let profile = ChannelCapabilityProfile::new(input.channel, input.capabilities.to_vec());
    media_delivery_decision_for_profile(&profile, input)
}

pub fn media_delivery_decision_for_profile(
    profile: &ChannelCapabilityProfile,
    input: MediaDeliveryPolicyInput<'_>,
) -> MediaDeliveryDecision {
    let audio_container =
        AudioContainer::from_media(input.mime_type, input.file_name, input.provider_format);

    if input.artifact_kind != MediaArtifactKind::Voice {
        return non_voice_decision(input, profile, audio_container);
    }

    if profile.has(ChannelCapability::NativeVoiceMetadata) {
        return metadata_voice_decision(input, profile, audio_container);
    }
    if profile.has(ChannelCapability::OggOpusVoiceNotes) {
        return ogg_opus_voice_decision(
            input,
            profile,
            audio_container,
            "native_voice_requires_ogg_opus",
        );
    }
    if profile.has(ChannelCapability::NativeVoiceNotes) {
        return common_container_voice_decision(input, profile, audio_container);
    }
    if profile.has(ChannelCapability::AudioAttachments)
        || profile.has(ChannelCapability::Attachments)
    {
        return MediaDeliveryDecision {
            channel: profile.channel.clone(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::AudioAttachment,
            audio_container: Some(audio_container),
            native_voice: false,
            recommended_kind: MediaArtifactKind::Audio,
            required_provider_format: None,
            compatibility_notes: vec!["channel_delivers_voice_as_audio_attachment".into()],
            reason: "voice_note_not_declared_by_channel_capabilities".into(),
        };
    }

    MediaDeliveryDecision {
        channel: profile.channel.clone(),
        artifact_kind: input.artifact_kind,
        mode: MediaDeliveryMode::FileAttachment,
        audio_container: Some(audio_container),
        native_voice: false,
        recommended_kind: MediaArtifactKind::Audio,
        required_provider_format: None,
        compatibility_notes: vec!["channel_has_no_audio_attachment_capability".into()],
        reason: "voice_delivery_degraded_to_file_attachment".into(),
    }
}

pub fn tts_provider_output_format(config: &TtsConfig) -> String {
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

pub fn tts_output_extension(format: &str) -> &'static str {
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

pub fn tts_output_mime(format: &str) -> &'static str {
    match format.trim().to_ascii_lowercase().as_str() {
        "opus" => "audio/ogg; codecs=opus",
        "ogg" => "audio/ogg",
        "wav" | "wave" => "audio/wav",
        "m4a" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "pcm" => "audio/L16",
        _ => "audio/mpeg",
    }
}

pub fn ogg_opus_voice_mime(format: &str) -> Option<&'static str> {
    let container = AudioContainer::from_media(None, None, Some(format));
    container
        .is_ogg_opus_compatible()
        .then_some("audio/ogg; codecs=opus")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceSynthesisAttemptTrace {
    pub candidate_index: usize,
    pub provider: String,
    pub voice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub output_format: String,
    pub outcome: VoiceSynthesisAttemptOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<String>,
    pub failover_candidate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSynthesisAttemptOutcome {
    Success,
    EmptyAudio,
    VoiceCatalogError,
    UnsupportedVoice,
    ProviderError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceReplyDiagnostics {
    pub selected_provider: String,
    pub selected_voice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    pub selected_format: String,
    pub output_mime: String,
    pub output_extension: String,
    pub audio_bytes: usize,
    pub target_channel: String,
    pub delivery: MediaDeliveryDecision,
    pub synthesis_attempts: Vec<VoiceSynthesisAttemptTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceDeliveryChannelProfile {
    pub channel: String,
    pub native_voice_formats: Vec<String>,
    pub fallback_mode: MediaDeliveryMode,
    pub notes: Vec<String>,
}

pub fn voice_delivery_channel_profiles(
    profiles: impl IntoIterator<Item = ChannelCapabilityProfile>,
) -> Vec<VoiceDeliveryChannelProfile> {
    profiles
        .into_iter()
        .map(|profile| {
            let native_voice_formats = if profile.has(ChannelCapability::NativeVoiceMetadata) {
                vec!["provider_native".into()]
            } else if profile.has(ChannelCapability::OggOpusVoiceNotes) {
                vec!["ogg_opus".into()]
            } else if profile.has(ChannelCapability::NativeVoiceNotes) {
                vec!["ogg_opus".into(), "ogg".into(), "mp3".into(), "m4a".into()]
            } else {
                Vec::new()
            };

            let fallback_mode = if profile.has(ChannelCapability::AudioAttachments)
                || profile.has(ChannelCapability::Attachments)
            {
                MediaDeliveryMode::AudioAttachment
            } else {
                MediaDeliveryMode::FileAttachment
            };

            let mut notes = Vec::new();
            if profile.has(ChannelCapability::NativeVoiceMetadata) {
                notes.push("uses_native_voice_metadata".into());
            }
            if profile.has(ChannelCapability::OggOpusVoiceNotes) {
                notes.push("native_voice_requires_ogg_opus".into());
            } else if profile.has(ChannelCapability::NativeVoiceNotes)
                && !profile.has(ChannelCapability::NativeVoiceMetadata)
            {
                notes.push("native_voice_accepts_common_audio_containers".into());
            }
            if profile.has(ChannelCapability::AudioAttachments)
                || profile.has(ChannelCapability::Attachments)
            {
                notes.push("can_degrade_voice_to_audio_attachment".into());
            } else {
                notes.push("can_degrade_voice_to_file_attachment".into());
            }

            VoiceDeliveryChannelProfile {
                channel: profile.channel,
                native_voice_formats,
                fallback_mode,
                notes,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeCallSupport {
    Available,
    Planned,
    Unsupported,
}

impl RealtimeCallSupport {
    pub fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Planned => "planned",
            Self::Unsupported => "unsupported",
        }
    }
}

impl std::fmt::Display for RealtimeCallSupport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeCallChannelProfile {
    pub channel: String,
    pub audio_call: RealtimeCallSupport,
    pub video_call: RealtimeCallSupport,
    pub notes: Vec<String>,
}

pub fn realtime_call_channel_profiles(
    profiles: impl IntoIterator<Item = ChannelCapabilityProfile>,
) -> Vec<RealtimeCallChannelProfile> {
    profiles
        .into_iter()
        .map(|profile| {
            let audio_call = realtime_call_support(&profile, ChannelCapability::RealtimeAudioCall);
            let video_call = realtime_call_support(&profile, ChannelCapability::RealtimeVideoCall);
            RealtimeCallChannelProfile {
                channel: profile.channel.clone(),
                audio_call,
                video_call,
                notes: call_profile_notes(audio_call, video_call),
            }
        })
        .collect()
}

fn realtime_call_support(
    profile: &ChannelCapabilityProfile,
    capability: ChannelCapability,
) -> RealtimeCallSupport {
    if profile.has(capability) {
        RealtimeCallSupport::Available
    } else if profile.plans(capability) {
        RealtimeCallSupport::Planned
    } else {
        RealtimeCallSupport::Unsupported
    }
}

fn call_profile_notes(
    audio_call: RealtimeCallSupport,
    video_call: RealtimeCallSupport,
) -> Vec<String> {
    let mut notes = Vec::new();
    if audio_call.is_available() {
        notes.push("realtime_audio_call_runtime_declared".into());
    }
    if video_call.is_available() {
        notes.push("realtime_video_call_runtime_declared".into());
    }
    if audio_call == RealtimeCallSupport::Planned {
        notes.push("realtime_audio_call_runtime_planned_not_available".into());
    }
    if video_call == RealtimeCallSupport::Planned {
        notes.push("realtime_video_call_runtime_planned_not_available".into());
    }
    if audio_call == RealtimeCallSupport::Unsupported
        && video_call == RealtimeCallSupport::Unsupported
    {
        notes.push("voice_notes_or_audio_attachments_are_not_realtime_calls".into());
    }
    notes
}

fn non_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    profile: &ChannelCapabilityProfile,
    audio_container: AudioContainer,
) -> MediaDeliveryDecision {
    let is_audio = matches!(
        input.artifact_kind,
        MediaArtifactKind::Audio | MediaArtifactKind::Music
    );
    MediaDeliveryDecision {
        channel: profile.channel.clone(),
        artifact_kind: input.artifact_kind,
        mode: if is_audio {
            MediaDeliveryMode::NativeAudio
        } else {
            MediaDeliveryMode::FileAttachment
        },
        audio_container: is_audio.then_some(audio_container),
        native_voice: false,
        recommended_kind: input.artifact_kind,
        required_provider_format: None,
        compatibility_notes: Vec::new(),
        reason: if is_audio {
            "audio_artifact_uses_channel_audio_delivery".into()
        } else {
            "non_audio_artifact_uses_channel_media_delivery".into()
        },
    }
}

fn metadata_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    profile: &ChannelCapabilityProfile,
    audio_container: AudioContainer,
) -> MediaDeliveryDecision {
    let mut compatibility_notes = vec!["native_voice_metadata_event".into()];
    if !audio_container.is_ogg_opus_compatible() {
        compatibility_notes.push("strict_mobile_clients_may_require_ogg_opus_payload".into());
    }

    MediaDeliveryDecision {
        channel: profile.channel.clone(),
        artifact_kind: input.artifact_kind,
        mode: MediaDeliveryMode::NativeVoice,
        audio_container: Some(audio_container),
        native_voice: true,
        recommended_kind: MediaArtifactKind::Voice,
        required_provider_format: None,
        compatibility_notes,
        reason: "channel_supports_native_voice_metadata_with_provider_native_payload".into(),
    }
}

fn common_container_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    profile: &ChannelCapabilityProfile,
    audio_container: AudioContainer,
) -> MediaDeliveryDecision {
    if audio_container.is_common_native_voice_compatible() {
        let mut compatibility_notes = Vec::new();
        if !audio_container.is_ogg_opus_compatible() {
            compatibility_notes.push("native_voice_non_opus_may_not_render_as_voice_bubble".into());
        }

        return MediaDeliveryDecision {
            channel: profile.channel.clone(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::NativeVoice,
            audio_container: Some(audio_container),
            native_voice: true,
            recommended_kind: MediaArtifactKind::Voice,
            required_provider_format: (!audio_container.is_ogg_opus_compatible())
                .then(|| "opus".into()),
            compatibility_notes,
            reason: "channel_native_voice_accepts_declared_audio_containers".into(),
        };
    }

    MediaDeliveryDecision {
        channel: profile.channel.clone(),
        artifact_kind: input.artifact_kind,
        mode: MediaDeliveryMode::AudioAttachment,
        audio_container: Some(audio_container),
        native_voice: false,
        recommended_kind: MediaArtifactKind::Audio,
        required_provider_format: Some("opus".into()),
        compatibility_notes: vec!["native_voice_degraded_to_audio_attachment".into()],
        reason: "native_voice_payload_format_not_supported_by_declared_profile".into(),
    }
}

fn ogg_opus_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    profile: &ChannelCapabilityProfile,
    audio_container: AudioContainer,
    incompatible_reason: &str,
) -> MediaDeliveryDecision {
    if audio_container.is_ogg_opus_compatible() {
        return MediaDeliveryDecision {
            channel: profile.channel.clone(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::NativeVoice,
            audio_container: Some(audio_container),
            native_voice: true,
            recommended_kind: MediaArtifactKind::Voice,
            required_provider_format: None,
            compatibility_notes: Vec::new(),
            reason: "channel_accepts_ogg_opus_native_voice".into(),
        };
    }

    if input.normalizer_available {
        return MediaDeliveryDecision {
            channel: profile.channel.clone(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::RequiresNormalizer,
            audio_container: Some(audio_container),
            native_voice: false,
            recommended_kind: MediaArtifactKind::Voice,
            required_provider_format: Some("opus".into()),
            compatibility_notes: vec!["normalizer_needed_before_native_voice_delivery".into()],
            reason: incompatible_reason.into(),
        };
    }

    MediaDeliveryDecision {
        channel: profile.channel.clone(),
        artifact_kind: input.artifact_kind,
        mode: MediaDeliveryMode::AudioAttachment,
        audio_container: Some(audio_container),
        native_voice: false,
        recommended_kind: MediaArtifactKind::Audio,
        required_provider_format: Some("opus".into()),
        compatibility_notes: vec!["native_voice_degraded_to_audio_attachment".into()],
        reason: incompatible_reason.into(),
    }
}

fn mime_has_opus_codec(raw: &str) -> bool {
    raw.trim()
        .parse::<mime::Mime>()
        .ok()
        .and_then(|mime| {
            mime.params()
                .find(|(name, _)| name.as_str().eq_ignore_ascii_case("codecs"))
                .map(|(_, value)| value.as_str().to_ascii_lowercase())
        })
        .is_some_and(|value| value.split(',').any(|codec| codec.trim() == "opus"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_capabilities(channel: &str) -> Vec<ChannelCapability> {
        match channel {
            "matrix" => vec![
                ChannelCapability::Attachments,
                ChannelCapability::AudioAttachments,
                ChannelCapability::NativeVoiceNotes,
                ChannelCapability::NativeVoiceMetadata,
            ],
            "telegram" => vec![
                ChannelCapability::Attachments,
                ChannelCapability::AudioAttachments,
                ChannelCapability::NativeVoiceNotes,
            ],
            "whatsapp" => vec![
                ChannelCapability::Attachments,
                ChannelCapability::AudioAttachments,
                ChannelCapability::NativeVoiceNotes,
                ChannelCapability::OggOpusVoiceNotes,
            ],
            "signal" => vec![
                ChannelCapability::Attachments,
                ChannelCapability::AudioAttachments,
            ],
            _ => vec![ChannelCapability::Attachments],
        }
    }

    fn voice_decision(
        channel: &str,
        mime_type: Option<&str>,
        file_name: Option<&str>,
        provider_format: Option<&str>,
    ) -> MediaDeliveryDecision {
        let capabilities = test_capabilities(channel);
        media_delivery_decision(MediaDeliveryPolicyInput {
            channel,
            capabilities: capabilities.as_slice(),
            artifact_kind: MediaArtifactKind::Voice,
            mime_type,
            file_name,
            provider_format,
            normalizer_available: false,
        })
    }

    #[test]
    fn matrix_voice_keeps_native_event_and_warns_for_non_ogg_payload() {
        let decision = voice_decision("matrix", Some("audio/wav"), Some("voice.wav"), None);

        assert_eq!(decision.mode, MediaDeliveryMode::NativeVoice);
        assert!(decision.native_voice);
        assert_eq!(decision.recommended_kind, MediaArtifactKind::Voice);
        assert!(decision
            .compatibility_notes
            .iter()
            .any(|note| note == "strict_mobile_clients_may_require_ogg_opus_payload"));
    }

    #[test]
    fn telegram_degrades_incompatible_voice_payload_to_audio_attachment() {
        let decision = voice_decision("telegram", Some("audio/wav"), Some("voice.wav"), None);

        assert_eq!(decision.mode, MediaDeliveryMode::AudioAttachment);
        assert!(!decision.native_voice);
        assert_eq!(decision.recommended_kind, MediaArtifactKind::Audio);
        assert_eq!(decision.required_provider_format.as_deref(), Some("opus"));
    }

    #[test]
    fn telegram_accepts_mp3_voice_with_non_opus_compatibility_note() {
        let decision = voice_decision("telegram", Some("audio/mpeg"), Some("voice.mp3"), None);

        assert_eq!(decision.mode, MediaDeliveryMode::NativeVoice);
        assert!(decision.native_voice);
        assert_eq!(decision.recommended_kind, MediaArtifactKind::Voice);
        assert_eq!(decision.required_provider_format.as_deref(), Some("opus"));
        assert!(decision
            .compatibility_notes
            .iter()
            .any(|note| note == "native_voice_non_opus_may_not_render_as_voice_bubble"));
    }

    #[test]
    fn whatsapp_accepts_provider_opus_as_native_ptt() {
        let decision = voice_decision(
            "whatsapp",
            Some("audio/ogg"),
            Some("voice.ogg"),
            Some("opus"),
        );

        assert_eq!(decision.mode, MediaDeliveryMode::NativeVoice);
        assert!(decision.native_voice);
        assert_eq!(ogg_opus_voice_mime("opus"), Some("audio/ogg; codecs=opus"));
    }

    #[test]
    fn signal_delivers_voice_as_audio_attachment() {
        let decision = voice_decision("signal", Some("audio/ogg"), Some("voice.ogg"), Some("opus"));

        assert_eq!(decision.mode, MediaDeliveryMode::AudioAttachment);
        assert!(!decision.native_voice);
        assert_eq!(decision.recommended_kind, MediaArtifactKind::Audio);
        assert_eq!(
            decision.reason,
            "voice_note_not_declared_by_channel_capabilities"
        );
        assert!(decision
            .compatibility_notes
            .iter()
            .any(|note| note == "channel_delivers_voice_as_audio_attachment"));
    }

    #[test]
    fn normalizer_available_preserves_native_voice_requirement() {
        let capabilities = test_capabilities("whatsapp");
        let decision = media_delivery_decision(MediaDeliveryPolicyInput {
            channel: "whatsapp",
            capabilities: capabilities.as_slice(),
            artifact_kind: MediaArtifactKind::Voice,
            mime_type: Some("audio/mpeg"),
            file_name: Some("voice.mp3"),
            provider_format: None,
            normalizer_available: true,
        });

        assert_eq!(decision.mode, MediaDeliveryMode::RequiresNormalizer);
        assert_eq!(decision.recommended_kind, MediaArtifactKind::Voice);
    }

    #[test]
    fn voice_delivery_profiles_expose_channel_specific_requirements() {
        let profiles = voice_delivery_channel_profiles([
            ChannelCapabilityProfile::new("matrix", test_capabilities("matrix")),
            ChannelCapabilityProfile::new("telegram", test_capabilities("telegram")),
            ChannelCapabilityProfile::new("whatsapp", test_capabilities("whatsapp")),
            ChannelCapabilityProfile::new("signal", test_capabilities("signal")),
        ]);

        let whatsapp = profiles
            .iter()
            .find(|profile| profile.channel == "whatsapp")
            .expect("whatsapp profile");
        assert_eq!(whatsapp.native_voice_formats, vec!["ogg_opus"]);
        assert_eq!(whatsapp.fallback_mode, MediaDeliveryMode::AudioAttachment);

        let matrix = profiles
            .iter()
            .find(|profile| profile.channel == "matrix")
            .expect("matrix profile");
        assert_eq!(matrix.native_voice_formats, vec!["provider_native"]);
        assert!(matrix
            .notes
            .iter()
            .any(|note| note == "uses_native_voice_metadata"));

        let signal = profiles
            .iter()
            .find(|profile| profile.channel == "signal")
            .expect("signal profile");
        assert_eq!(signal.fallback_mode, MediaDeliveryMode::AudioAttachment);
    }

    #[test]
    fn realtime_call_profiles_do_not_confuse_voice_notes_with_calls() {
        let profiles = realtime_call_channel_profiles([
            ChannelCapabilityProfile::new("matrix", test_capabilities("matrix")),
            ChannelCapabilityProfile::new("clawdtalk", vec![ChannelCapability::RealtimeAudioCall]),
        ]);

        let matrix = profiles
            .iter()
            .find(|profile| profile.channel == "matrix")
            .expect("matrix profile");
        assert_eq!(matrix.audio_call, RealtimeCallSupport::Unsupported);
        assert_eq!(matrix.video_call, RealtimeCallSupport::Unsupported);

        let clawdtalk = profiles
            .iter()
            .find(|profile| profile.channel == "clawdtalk")
            .expect("clawdtalk profile");
        assert_eq!(clawdtalk.audio_call, RealtimeCallSupport::Available);
        assert_eq!(clawdtalk.video_call, RealtimeCallSupport::Unsupported);
    }

    #[test]
    fn realtime_call_profiles_report_planned_without_runtime_capability() {
        let profiles = realtime_call_channel_profiles([ChannelCapabilityProfile::new(
            "matrix",
            test_capabilities("matrix"),
        )
        .with_planned_capabilities(vec![
            ChannelCapability::RealtimeAudioCall,
            ChannelCapability::RealtimeVideoCall,
        ])]);

        let matrix = profiles
            .iter()
            .find(|profile| profile.channel == "matrix")
            .expect("matrix profile");
        assert_eq!(matrix.audio_call, RealtimeCallSupport::Planned);
        assert_eq!(matrix.video_call, RealtimeCallSupport::Planned);
        assert!(matrix
            .notes
            .iter()
            .any(|note| note == "realtime_audio_call_runtime_planned_not_available"));
    }
}
