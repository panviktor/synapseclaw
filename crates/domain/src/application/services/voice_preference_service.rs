use crate::config::schema::TtsConfig;
use crate::domain::user_profile::UserProfile;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const VOICE_SETTINGS_FACT_KEY: &str = "voice_settings";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoicePreferenceScope {
    Global,
    Channel,
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoTtsPolicy {
    Inherit,
    Off,
    Always,
    InboundVoice,
    Tagged,
    ChannelDefault,
    ConversationDefault,
}

impl Default for AutoTtsPolicy {
    fn default() -> Self {
        Self::Inherit
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VoicePreference {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl VoicePreference {
    pub fn is_empty(&self) -> bool {
        self.provider.as_deref().is_none_or(str::is_empty)
            && self.model.as_deref().is_none_or(str::is_empty)
            && self.voice.as_deref().is_none_or(str::is_empty)
            && self.format.as_deref().is_none_or(str::is_empty)
    }

    pub fn normalized(mut self) -> Self {
        self.provider = normalize_optional_lower(self.provider);
        self.model = normalize_optional(self.model);
        self.voice = normalize_optional(self.voice);
        self.format = normalize_optional_lower(self.format);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preference: Option<VoicePreference>,
    #[serde(default)]
    pub auto_tts_policy: AutoTtsPolicy,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            preference: None,
            auto_tts_policy: AutoTtsPolicy::Inherit,
        }
    }
}

impl VoiceSettings {
    pub fn is_empty(&self) -> bool {
        self.preference
            .as_ref()
            .is_none_or(VoicePreference::is_empty)
            && self.auto_tts_policy == AutoTtsPolicy::Inherit
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoicePreferenceTarget {
    pub scope: VoicePreferenceScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
}

impl VoicePreferenceTarget {
    pub fn global() -> Self {
        Self {
            scope: VoicePreferenceScope::Global,
            channel: None,
            recipient: None,
        }
    }

    pub fn channel(channel: impl Into<String>) -> Self {
        Self {
            scope: VoicePreferenceScope::Channel,
            channel: Some(channel.into()),
            recipient: None,
        }
    }

    pub fn conversation(channel: impl Into<String>, recipient: impl Into<String>) -> Self {
        Self {
            scope: VoicePreferenceScope::Conversation,
            channel: Some(channel.into()),
            recipient: Some(recipient.into()),
        }
    }

    pub fn normalized(mut self) -> Result<Self, String> {
        self.channel = normalize_optional_lower(self.channel);
        self.recipient = normalize_optional(self.recipient);

        match self.scope {
            VoicePreferenceScope::Global => {
                self.channel = None;
                self.recipient = None;
            }
            VoicePreferenceScope::Channel => {
                if self.channel.is_none() {
                    return Err("channel scope requires channel".into());
                }
                self.recipient = None;
            }
            VoicePreferenceScope::Conversation => {
                if self.channel.is_none() || self.recipient.is_none() {
                    return Err("conversation scope requires channel and recipient".into());
                }
            }
        }
        Ok(self)
    }

    pub fn storage_key(&self) -> Result<String, String> {
        let target = self.clone().normalized()?;
        Ok(match target.scope {
            VoicePreferenceScope::Global => "voice:global".into(),
            VoicePreferenceScope::Channel => {
                format!(
                    "voice:channel:{}",
                    target.channel.expect("validated channel")
                )
            }
            VoicePreferenceScope::Conversation => format!(
                "voice:conversation:{}:{}",
                target.channel.expect("validated channel"),
                target.recipient.expect("validated recipient")
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedVoicePreference {
    pub source: VoicePreferenceScope,
    pub storage_key: String,
    pub preference: VoicePreference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAutoTtsPolicy {
    pub source: VoicePreferenceScope,
    pub storage_key: String,
    pub policy: AutoTtsPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoTtsTrigger {
    NormalReply,
    InboundVoice,
    ExplicitTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoTtsDecision {
    Skip,
    Synthesize,
    DeferToBroaderScope,
}

pub fn read_voice_settings(profile: Option<UserProfile>) -> VoiceSettings {
    profile
        .and_then(|profile| profile.get(VOICE_SETTINGS_FACT_KEY).cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn write_voice_settings(settings: VoiceSettings) -> Option<UserProfile> {
    if settings.is_empty() {
        return None;
    }

    let mut profile = UserProfile::default();
    profile.set(VOICE_SETTINGS_FACT_KEY, json!(settings));
    Some(profile)
}

pub fn resolve_voice_preference(
    global: Option<UserProfile>,
    channel: Option<UserProfile>,
    conversation: Option<UserProfile>,
    channel_name: &str,
    recipient: &str,
) -> Option<ResolvedVoicePreference> {
    let conversation_target =
        VoicePreferenceTarget::conversation(channel_name.to_string(), recipient.to_string());
    let channel_target = VoicePreferenceTarget::channel(channel_name.to_string());
    let global_target = VoicePreferenceTarget::global();

    [
        (
            VoicePreferenceScope::Conversation,
            conversation_target.storage_key().ok()?,
            read_voice_settings(conversation).preference,
        ),
        (
            VoicePreferenceScope::Channel,
            channel_target.storage_key().ok()?,
            read_voice_settings(channel).preference,
        ),
        (
            VoicePreferenceScope::Global,
            global_target.storage_key().ok()?,
            read_voice_settings(global).preference,
        ),
    ]
    .into_iter()
    .find_map(|(source, storage_key, preference)| {
        let preference = preference?.normalized();
        (!preference.is_empty()).then_some(ResolvedVoicePreference {
            source,
            storage_key,
            preference,
        })
    })
}

pub fn resolve_auto_tts_policy(
    global: Option<UserProfile>,
    channel: Option<UserProfile>,
    conversation: Option<UserProfile>,
    channel_name: &str,
    recipient: &str,
) -> Option<ResolvedAutoTtsPolicy> {
    let conversation_target =
        VoicePreferenceTarget::conversation(channel_name.to_string(), recipient.to_string());
    let channel_target = VoicePreferenceTarget::channel(channel_name.to_string());
    let global_target = VoicePreferenceTarget::global();

    [
        (
            VoicePreferenceScope::Conversation,
            conversation_target.storage_key().ok()?,
            read_voice_settings(conversation).auto_tts_policy,
        ),
        (
            VoicePreferenceScope::Channel,
            channel_target.storage_key().ok()?,
            read_voice_settings(channel).auto_tts_policy,
        ),
        (
            VoicePreferenceScope::Global,
            global_target.storage_key().ok()?,
            read_voice_settings(global).auto_tts_policy,
        ),
    ]
    .into_iter()
    .find_map(|(source, storage_key, policy)| {
        (policy != AutoTtsPolicy::Inherit).then_some(ResolvedAutoTtsPolicy {
            source,
            storage_key,
            policy,
        })
    })
}

pub fn candidate_matches_preference(
    config: &TtsConfig,
    selected_model: Option<&str>,
    preference: &VoicePreference,
) -> bool {
    if let Some(provider) = preference.provider.as_deref() {
        if !config.default_provider.eq_ignore_ascii_case(provider) {
            return false;
        }
    }
    if let Some(model) = preference.model.as_deref() {
        if !selected_model.is_some_and(|candidate| candidate.eq_ignore_ascii_case(model)) {
            return false;
        }
    }
    if let Some(format) = preference.format.as_deref() {
        if !config.default_format.eq_ignore_ascii_case(format)
            && !crate::application::services::media_artifact_delivery::tts_provider_output_format(
                config,
            )
            .eq_ignore_ascii_case(format)
        {
            return false;
        }
    }
    true
}

pub fn auto_tts_decision(
    policy: AutoTtsPolicy,
    trigger: AutoTtsTrigger,
    broader_scope_policy: Option<AutoTtsPolicy>,
) -> AutoTtsDecision {
    match policy {
        AutoTtsPolicy::Inherit => {
            if let Some(policy) = broader_scope_policy {
                auto_tts_decision(policy, trigger, None)
            } else {
                AutoTtsDecision::DeferToBroaderScope
            }
        }
        AutoTtsPolicy::Off => AutoTtsDecision::Skip,
        AutoTtsPolicy::Always => AutoTtsDecision::Synthesize,
        AutoTtsPolicy::InboundVoice => match trigger {
            AutoTtsTrigger::InboundVoice => AutoTtsDecision::Synthesize,
            AutoTtsTrigger::NormalReply | AutoTtsTrigger::ExplicitTag => AutoTtsDecision::Skip,
        },
        AutoTtsPolicy::Tagged => match trigger {
            AutoTtsTrigger::ExplicitTag => AutoTtsDecision::Synthesize,
            AutoTtsTrigger::NormalReply | AutoTtsTrigger::InboundVoice => AutoTtsDecision::Skip,
        },
        AutoTtsPolicy::ChannelDefault | AutoTtsPolicy::ConversationDefault => {
            if let Some(policy) = broader_scope_policy {
                auto_tts_decision(policy, trigger, None)
            } else {
                AutoTtsDecision::DeferToBroaderScope
            }
        }
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_optional_lower(value: Option<String>) -> Option<String> {
    normalize_optional(value).map(|value| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_keys_are_scope_specific() {
        assert_eq!(
            VoicePreferenceTarget::global().storage_key().unwrap(),
            "voice:global"
        );
        assert_eq!(
            VoicePreferenceTarget::channel("Matrix")
                .storage_key()
                .unwrap(),
            "voice:channel:matrix"
        );
        assert_eq!(
            VoicePreferenceTarget::conversation("Matrix", "!room:example")
                .storage_key()
                .unwrap(),
            "voice:conversation:matrix:!room:example"
        );
    }

    #[test]
    fn resolve_prefers_conversation_then_channel_then_global() {
        let mut global = UserProfile::default();
        global.set(
            VOICE_SETTINGS_FACT_KEY,
            json!(VoiceSettings {
                preference: Some(VoicePreference {
                    voice: Some("global".into()),
                    ..VoicePreference::default()
                }),
                auto_tts_policy: AutoTtsPolicy::Off,
            }),
        );
        let mut channel = UserProfile::default();
        channel.set(
            VOICE_SETTINGS_FACT_KEY,
            json!(VoiceSettings {
                preference: Some(VoicePreference {
                    voice: Some("channel".into()),
                    ..VoicePreference::default()
                }),
                auto_tts_policy: AutoTtsPolicy::Off,
            }),
        );
        let mut conversation = UserProfile::default();
        conversation.set(
            VOICE_SETTINGS_FACT_KEY,
            json!(VoiceSettings {
                preference: Some(VoicePreference {
                    voice: Some("conversation".into()),
                    ..VoicePreference::default()
                }),
                auto_tts_policy: AutoTtsPolicy::Off,
            }),
        );

        let resolved = resolve_voice_preference(
            Some(global),
            Some(channel),
            Some(conversation),
            "matrix",
            "!room",
        )
        .unwrap();
        assert_eq!(resolved.source, VoicePreferenceScope::Conversation);
        assert_eq!(resolved.preference.voice.as_deref(), Some("conversation"));
    }

    #[test]
    fn candidate_match_includes_provider_model_and_format() {
        let mut config = TtsConfig {
            enabled: true,
            default_provider: "groq".into(),
            default_format: "wav".into(),
            ..TtsConfig::default()
        };
        config.groq = Some(crate::config::schema::GroqTtsConfig {
            api_key: Some("test".into()),
            model: "canopylabs/orpheus-v1-english".into(),
            response_format: "wav".into(),
        });

        assert!(candidate_matches_preference(
            &config,
            Some("canopylabs/orpheus-v1-english"),
            &VoicePreference {
                provider: Some("groq".into()),
                model: Some("canopylabs/orpheus-v1-english".into()),
                format: Some("wav".into()),
                ..VoicePreference::default()
            }
        ));
        assert!(!candidate_matches_preference(
            &config,
            Some("other-model"),
            &VoicePreference {
                model: Some("canopylabs/orpheus-v1-english".into()),
                ..VoicePreference::default()
            }
        ));
    }

    #[test]
    fn auto_tts_policy_is_typed_and_scope_aware() {
        assert_eq!(
            auto_tts_decision(AutoTtsPolicy::Always, AutoTtsTrigger::NormalReply, None),
            AutoTtsDecision::Synthesize
        );
        assert_eq!(
            auto_tts_decision(
                AutoTtsPolicy::InboundVoice,
                AutoTtsTrigger::NormalReply,
                None
            ),
            AutoTtsDecision::Skip
        );
        assert_eq!(
            auto_tts_decision(
                AutoTtsPolicy::InboundVoice,
                AutoTtsTrigger::InboundVoice,
                None
            ),
            AutoTtsDecision::Synthesize
        );
        assert_eq!(
            auto_tts_decision(
                AutoTtsPolicy::ConversationDefault,
                AutoTtsTrigger::ExplicitTag,
                Some(AutoTtsPolicy::Tagged)
            ),
            AutoTtsDecision::Synthesize
        );
    }

    #[test]
    fn auto_tts_policy_resolution_skips_inherited_scopes() {
        let mut global = UserProfile::default();
        global.set(
            VOICE_SETTINGS_FACT_KEY,
            json!(VoiceSettings {
                preference: None,
                auto_tts_policy: AutoTtsPolicy::Always,
            }),
        );
        let mut conversation = UserProfile::default();
        conversation.set(
            VOICE_SETTINGS_FACT_KEY,
            json!(VoiceSettings {
                preference: Some(VoicePreference {
                    voice: Some("hannah".into()),
                    ..VoicePreference::default()
                }),
                auto_tts_policy: AutoTtsPolicy::Inherit,
            }),
        );

        let resolved =
            resolve_auto_tts_policy(Some(global), None, Some(conversation), "matrix", "!room")
                .unwrap();

        assert_eq!(resolved.source, VoicePreferenceScope::Global);
        assert_eq!(resolved.policy, AutoTtsPolicy::Always);
    }
}
