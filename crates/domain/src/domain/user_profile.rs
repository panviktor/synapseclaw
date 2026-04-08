//! Structured user profile — durable user defaults and preferences.
//!
//! This is explicit runtime data, not a free-text memory block. It exists so
//! stable defaults can be resolved deterministically across channels and web.

use crate::domain::conversation_target::ConversationDeliveryTarget;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    pub preferred_language: Option<String>,
    pub timezone: Option<String>,
    pub default_city: Option<String>,
    pub communication_style: Option<String>,
    #[serde(default)]
    pub known_environments: Vec<String>,
    pub default_delivery_target: Option<ConversationDeliveryTarget>,
}

impl UserProfile {
    pub fn is_empty(&self) -> bool {
        self.preferred_language.is_none()
            && self.timezone.is_none()
            && self.default_city.is_none()
            && self.communication_style.is_none()
            && self.known_environments.is_empty()
            && self.default_delivery_target.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_profile_detected() {
        assert!(UserProfile::default().is_empty());
    }

    #[test]
    fn populated_profile_is_not_empty() {
        let profile = UserProfile {
            preferred_language: Some("ru".into()),
            ..Default::default()
        };
        assert!(!profile.is_empty());
    }
}
