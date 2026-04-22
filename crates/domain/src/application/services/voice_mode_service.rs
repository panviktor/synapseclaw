use crate::domain::user_profile::UserProfile;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const VOICE_MODE_PROFILE_KEY: &str = "voice_mode:cli";
pub const VOICE_MODE_SETTINGS_FACT_KEY: &str = "voice_mode_settings";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceModeSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_playback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl Default for VoiceModeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_playback: false,
            session_id: None,
        }
    }
}

impl VoiceModeSettings {
    pub fn normalized(mut self) -> Self {
        self.session_id = normalize_optional(self.session_id);
        self
    }

    pub fn is_empty(&self) -> bool {
        let normalized = self.clone().normalized();
        !normalized.enabled && !normalized.auto_playback && normalized.session_id.is_none()
    }
}

pub fn read_voice_mode_settings(profile: Option<UserProfile>) -> VoiceModeSettings {
    profile
        .and_then(|profile| profile.get(VOICE_MODE_SETTINGS_FACT_KEY).cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn write_voice_mode_settings(settings: VoiceModeSettings) -> Option<UserProfile> {
    let settings = settings.normalized();
    if settings.is_empty() {
        return None;
    }

    let mut profile = UserProfile::default();
    profile.set(VOICE_MODE_SETTINGS_FACT_KEY, json!(settings));
    Some(profile)
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_roundtrip_as_empty() {
        assert!(write_voice_mode_settings(VoiceModeSettings::default()).is_none());
        assert_eq!(read_voice_mode_settings(None), VoiceModeSettings::default());
    }

    #[test]
    fn settings_roundtrip_with_normalized_session() {
        let profile = write_voice_mode_settings(VoiceModeSettings {
            enabled: true,
            auto_playback: true,
            session_id: Some("  voice-session  ".into()),
        })
        .expect("profile");

        assert_eq!(
            read_voice_mode_settings(Some(profile)),
            VoiceModeSettings {
                enabled: true,
                auto_playback: true,
                session_id: Some("voice-session".into()),
            }
        );
    }
}
