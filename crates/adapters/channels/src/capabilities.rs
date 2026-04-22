//! Declared channel support matrix.
//!
//! `ChannelCapability` means the adapter has an implemented runtime path for
//! that behavior. Planned support is exposed separately so UI/diagnostics can
//! show roadmap intent without allowing execution to rely on it.

use synapse_domain::domain::channel::ChannelCapability;
use synapse_domain::ports::channel_registry::ChannelCapabilityProfile;

const DECLARED_CHANNELS: &[&str] = &[
    "telegram",
    "discord",
    "slack",
    "matrix",
    "mattermost",
    "signal",
    "whatsapp",
    "wati",
    "clawdtalk",
    "web",
];

pub fn declared_channel_capabilities(channel_name: &str) -> Vec<ChannelCapability> {
    match channel_name {
        "telegram" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Attachments,
            ChannelCapability::AudioAttachments,
            ChannelCapability::NativeVoiceNotes,
            ChannelCapability::RichFormatting,
            ChannelCapability::EditMessage,
            ChannelCapability::RuntimeCommands,
            ChannelCapability::InterruptOnNewMessage,
        ],
        "discord" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Threads,
            ChannelCapability::Attachments,
            ChannelCapability::AudioAttachments,
            ChannelCapability::Reactions,
            ChannelCapability::RichFormatting,
            ChannelCapability::EditMessage,
            ChannelCapability::RuntimeCommands,
            ChannelCapability::ToolContextDisplay,
        ],
        "slack" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Threads,
            ChannelCapability::Attachments,
            ChannelCapability::AudioAttachments,
            ChannelCapability::Reactions,
            ChannelCapability::RichFormatting,
            ChannelCapability::InterruptOnNewMessage,
            ChannelCapability::ToolContextDisplay,
        ],
        #[cfg(feature = "channel-matrix")]
        "matrix" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Threads,
            ChannelCapability::Attachments,
            ChannelCapability::AudioAttachments,
            ChannelCapability::NativeVoiceNotes,
            ChannelCapability::NativeVoiceMetadata,
            ChannelCapability::Reactions,
            ChannelCapability::RichFormatting,
            ChannelCapability::RealtimeAudioCall,
            ChannelCapability::RuntimeCommands,
            ChannelCapability::ToolContextDisplay,
        ],
        "mattermost" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Threads,
            ChannelCapability::Reactions,
            ChannelCapability::RichFormatting,
            ChannelCapability::ToolContextDisplay,
        ],
        "signal" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Attachments,
            ChannelCapability::AudioAttachments,
            ChannelCapability::Reactions,
        ],
        "whatsapp" | "whatsapp-web" | "wati" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::Attachments,
            ChannelCapability::AudioAttachments,
            ChannelCapability::NativeVoiceNotes,
            ChannelCapability::OggOpusVoiceNotes,
        ],
        "clawdtalk" => vec![
            ChannelCapability::SendText,
            ChannelCapability::ReceiveText,
            ChannelCapability::RealtimeAudioCall,
        ],
        "web" => synapse_domain::domain::channel::web_channel_capabilities(),
        _ => vec![],
    }
}

pub fn planned_channel_capabilities(channel_name: &str) -> Vec<ChannelCapability> {
    match channel_name {
        "matrix" => vec![ChannelCapability::RealtimeVideoCall],
        "telegram" | "signal" => vec![ChannelCapability::RealtimeAudioCall],
        "clawdtalk" => vec![ChannelCapability::RealtimeVideoCall],
        _ => Vec::new(),
    }
}

pub fn declared_channel_capability_profile(channel_name: &str) -> ChannelCapabilityProfile {
    ChannelCapabilityProfile::new(
        channel_name.trim().to_ascii_lowercase(),
        declared_channel_capabilities(channel_name),
    )
    .with_planned_capabilities(planned_channel_capabilities(channel_name))
}

pub fn declared_channel_capability_profiles() -> Vec<ChannelCapabilityProfile> {
    DECLARED_CHANNELS
        .iter()
        .map(|channel| declared_channel_capability_profile(channel))
        .filter(|profile| {
            !profile.capabilities.is_empty() || !profile.planned_capabilities.is_empty()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_realtime_capabilities_are_not_declared_for_planned_channel_calls() {
        let matrix = declared_channel_capability_profile("matrix");
        assert!(matrix.has(ChannelCapability::RealtimeAudioCall));
        assert!(!matrix.has(ChannelCapability::RealtimeVideoCall));
        assert!(matrix.plans(ChannelCapability::RealtimeVideoCall));

        let clawdtalk = declared_channel_capability_profile("clawdtalk");
        assert!(clawdtalk.has(ChannelCapability::RealtimeAudioCall));
        assert!(!clawdtalk.has(ChannelCapability::RealtimeVideoCall));
        assert!(clawdtalk.plans(ChannelCapability::RealtimeVideoCall));
    }
}
