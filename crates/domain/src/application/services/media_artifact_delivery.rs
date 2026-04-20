use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::schema::TtsConfig;
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

    pub fn is_telegram_voice_compatible(self) -> bool {
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
    pub artifact_kind: MediaArtifactKind,
    pub mime_type: Option<&'a str>,
    pub file_name: Option<&'a str>,
    pub provider_format: Option<&'a str>,
    pub normalizer_available: bool,
}

pub fn media_delivery_decision(input: MediaDeliveryPolicyInput<'_>) -> MediaDeliveryDecision {
    let profile = ChannelMediaProfile::from_channel(input.channel);
    let audio_container =
        AudioContainer::from_media(input.mime_type, input.file_name, input.provider_format);

    if input.artifact_kind != MediaArtifactKind::Voice {
        return non_voice_decision(input, profile, audio_container);
    }

    match profile {
        ChannelMediaProfile::Matrix => matrix_voice_decision(input, audio_container),
        ChannelMediaProfile::Telegram => telegram_voice_decision(input, audio_container),
        ChannelMediaProfile::Whatsapp => {
            ogg_opus_voice_decision(input, audio_container, "whatsapp_ptt_requires_ogg_opus")
        }
        ChannelMediaProfile::Signal => MediaDeliveryDecision {
            channel: profile.label().into(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::AudioAttachment,
            audio_container: Some(audio_container),
            native_voice: false,
            recommended_kind: MediaArtifactKind::Audio,
            required_provider_format: None,
            compatibility_notes: vec!["signal_cli_sends_voice_as_attachment".into()],
            reason: "signal_delivers_voice_as_audio_attachment".into(),
        },
        ChannelMediaProfile::Discord => MediaDeliveryDecision {
            channel: profile.label().into(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::AudioAttachment,
            audio_container: Some(audio_container),
            native_voice: false,
            recommended_kind: MediaArtifactKind::Audio,
            required_provider_format: None,
            compatibility_notes: vec!["discord_message_files_do_not_have_voice_bubble".into()],
            reason: "discord_delivers_voice_as_audio_file".into(),
        },
        ChannelMediaProfile::Slack => MediaDeliveryDecision {
            channel: profile.label().into(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::FileAttachment,
            audio_container: Some(audio_container),
            native_voice: false,
            recommended_kind: MediaArtifactKind::Audio,
            required_provider_format: None,
            compatibility_notes: vec!["slack_files_do_not_have_native_voice_note_semantics".into()],
            reason: "slack_delivers_voice_as_file_upload".into(),
        },
        ChannelMediaProfile::Generic => MediaDeliveryDecision {
            channel: input.channel.trim().to_ascii_lowercase(),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::AudioAttachment,
            audio_container: Some(audio_container),
            native_voice: false,
            recommended_kind: MediaArtifactKind::Audio,
            required_provider_format: None,
            compatibility_notes: Vec::new(),
            reason: "generic_channel_delivers_voice_as_audio_attachment".into(),
        },
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

pub fn whatsapp_ptt_mime(format: &str) -> Option<&'static str> {
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

pub fn voice_delivery_channel_profiles() -> Vec<VoiceDeliveryChannelProfile> {
    vec![
        VoiceDeliveryChannelProfile {
            channel: "matrix".into(),
            native_voice_formats: vec!["provider_native".into()],
            fallback_mode: MediaDeliveryMode::NativeVoice,
            notes: vec![
                "uses_matrix_msc3245_voice_event".into(),
                "mobile_clients_may_prefer_ogg_opus".into(),
            ],
        },
        VoiceDeliveryChannelProfile {
            channel: "telegram".into(),
            native_voice_formats: vec!["ogg_opus".into(), "ogg".into(), "mp3".into(), "m4a".into()],
            fallback_mode: MediaDeliveryMode::AudioAttachment,
            notes: vec![
                "send_voice_accepts_multiple_audio_containers".into(),
                "ogg_opus_is_safest_for_consistent_voice_bubble".into(),
            ],
        },
        VoiceDeliveryChannelProfile {
            channel: "whatsapp".into(),
            native_voice_formats: vec!["ogg_opus".into()],
            fallback_mode: MediaDeliveryMode::AudioAttachment,
            notes: vec![
                "ptt_true_requires_ogg_opus".into(),
                "mp3_wav_m4a_are_sent_as_normal_audio".into(),
            ],
        },
        VoiceDeliveryChannelProfile {
            channel: "signal".into(),
            native_voice_formats: Vec::new(),
            fallback_mode: MediaDeliveryMode::AudioAttachment,
            notes: vec!["signal_cli_sends_voice_as_attachment".into()],
        },
        VoiceDeliveryChannelProfile {
            channel: "discord".into(),
            native_voice_formats: Vec::new(),
            fallback_mode: MediaDeliveryMode::AudioAttachment,
            notes: vec!["message_file_upload_without_voice_bubble".into()],
        },
        VoiceDeliveryChannelProfile {
            channel: "slack".into(),
            native_voice_formats: Vec::new(),
            fallback_mode: MediaDeliveryMode::FileAttachment,
            notes: vec!["file_upload_without_native_voice_note_semantics".into()],
        },
    ]
}

fn non_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    profile: ChannelMediaProfile,
    audio_container: AudioContainer,
) -> MediaDeliveryDecision {
    let is_audio = matches!(
        input.artifact_kind,
        MediaArtifactKind::Audio | MediaArtifactKind::Music
    );
    MediaDeliveryDecision {
        channel: profile.output_channel(input.channel),
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

fn matrix_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    audio_container: AudioContainer,
) -> MediaDeliveryDecision {
    let mut compatibility_notes = vec!["matrix_msc3245_native_voice_event".into()];
    if !audio_container.is_ogg_opus_compatible() {
        compatibility_notes.push("strict_mobile_clients_may_require_ogg_opus_payload".into());
    }

    MediaDeliveryDecision {
        channel: ChannelMediaProfile::Matrix.label().into(),
        artifact_kind: input.artifact_kind,
        mode: MediaDeliveryMode::NativeVoice,
        audio_container: Some(audio_container),
        native_voice: true,
        recommended_kind: MediaArtifactKind::Voice,
        required_provider_format: None,
        compatibility_notes,
        reason: "matrix_supports_native_voice_event_with_provider_native_payload".into(),
    }
}

fn telegram_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    audio_container: AudioContainer,
) -> MediaDeliveryDecision {
    if audio_container.is_telegram_voice_compatible() {
        let mut compatibility_notes = Vec::new();
        if !audio_container.is_ogg_opus_compatible() {
            compatibility_notes
                .push("telegram_non_opus_voice_is_not_guaranteed_voice_bubble".into());
        }

        return MediaDeliveryDecision {
            channel: ChannelMediaProfile::Telegram.output_channel(input.channel),
            artifact_kind: input.artifact_kind,
            mode: MediaDeliveryMode::NativeVoice,
            audio_container: Some(audio_container),
            native_voice: true,
            recommended_kind: MediaArtifactKind::Voice,
            required_provider_format: (!audio_container.is_ogg_opus_compatible())
                .then(|| "opus".into()),
            compatibility_notes,
            reason: "telegram_send_voice_accepts_ogg_mp3_m4a_opus".into(),
        };
    }

    MediaDeliveryDecision {
        channel: ChannelMediaProfile::Telegram.output_channel(input.channel),
        artifact_kind: input.artifact_kind,
        mode: MediaDeliveryMode::AudioAttachment,
        audio_container: Some(audio_container),
        native_voice: false,
        recommended_kind: MediaArtifactKind::Audio,
        required_provider_format: Some("opus".into()),
        compatibility_notes: vec!["telegram_native_voice_degraded_to_audio_attachment".into()],
        reason: "telegram_voice_payload_format_not_supported_by_send_voice_profile".into(),
    }
}

fn ogg_opus_voice_decision(
    input: MediaDeliveryPolicyInput<'_>,
    audio_container: AudioContainer,
    incompatible_reason: &str,
) -> MediaDeliveryDecision {
    if audio_container.is_ogg_opus_compatible() {
        return MediaDeliveryDecision {
            channel: ChannelMediaProfile::from_channel(input.channel).output_channel(input.channel),
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
            channel: ChannelMediaProfile::from_channel(input.channel).output_channel(input.channel),
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
        channel: ChannelMediaProfile::from_channel(input.channel).output_channel(input.channel),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelMediaProfile {
    Matrix,
    Telegram,
    Whatsapp,
    Signal,
    Discord,
    Slack,
    Generic,
}

impl ChannelMediaProfile {
    fn from_channel(channel: &str) -> Self {
        match channel.trim().to_ascii_lowercase().as_str() {
            "matrix" => Self::Matrix,
            "telegram" => Self::Telegram,
            "whatsapp" | "whatsapp-web" | "wati" => Self::Whatsapp,
            "signal" => Self::Signal,
            "discord" => Self::Discord,
            "slack" => Self::Slack,
            _ => Self::Generic,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Matrix => "matrix",
            Self::Telegram => "telegram",
            Self::Whatsapp => "whatsapp",
            Self::Signal => "signal",
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Generic => "generic",
        }
    }

    fn output_channel(self, requested: &str) -> String {
        if self == Self::Generic {
            requested.trim().to_ascii_lowercase()
        } else {
            self.label().into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice_decision(
        channel: &str,
        mime_type: Option<&str>,
        file_name: Option<&str>,
        provider_format: Option<&str>,
    ) -> MediaDeliveryDecision {
        media_delivery_decision(MediaDeliveryPolicyInput {
            channel,
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
            .any(|note| note == "telegram_non_opus_voice_is_not_guaranteed_voice_bubble"));
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
        assert_eq!(whatsapp_ptt_mime("opus"), Some("audio/ogg; codecs=opus"));
    }

    #[test]
    fn signal_delivers_voice_as_audio_attachment() {
        let decision = voice_decision("signal", Some("audio/ogg"), Some("voice.ogg"), Some("opus"));

        assert_eq!(decision.mode, MediaDeliveryMode::AudioAttachment);
        assert!(!decision.native_voice);
        assert_eq!(decision.recommended_kind, MediaArtifactKind::Audio);
        assert!(decision
            .compatibility_notes
            .iter()
            .any(|note| note == "signal_cli_sends_voice_as_attachment"));
    }

    #[test]
    fn normalizer_available_preserves_native_voice_requirement() {
        let decision = media_delivery_decision(MediaDeliveryPolicyInput {
            channel: "whatsapp",
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
        let profiles = voice_delivery_channel_profiles();

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

        let signal = profiles
            .iter()
            .find(|profile| profile.channel == "signal")
            .expect("signal profile");
        assert_eq!(signal.fallback_mode, MediaDeliveryMode::AudioAttachment);
    }
}
