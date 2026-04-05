//! Structured user profile updates and projections.
//!
//! This is the typed source of truth for durable user defaults. It avoids
//! parsing arbitrary free text into profile fields.

use crate::domain::conversation_target::ConversationDeliveryTarget;
use crate::domain::user_profile::UserProfile;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProfileFieldPatch<T> {
    #[default]
    Keep,
    Set(T),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserProfilePatch {
    pub preferred_language: ProfileFieldPatch<String>,
    pub timezone: ProfileFieldPatch<String>,
    pub default_city: ProfileFieldPatch<String>,
    pub communication_style: ProfileFieldPatch<String>,
    pub known_environments: ProfileFieldPatch<Vec<String>>,
    pub default_delivery_target: ProfileFieldPatch<ConversationDeliveryTarget>,
}

impl UserProfilePatch {
    pub fn is_noop(&self) -> bool {
        matches!(self.preferred_language, ProfileFieldPatch::Keep)
            && matches!(self.timezone, ProfileFieldPatch::Keep)
            && matches!(self.default_city, ProfileFieldPatch::Keep)
            && matches!(self.communication_style, ProfileFieldPatch::Keep)
            && matches!(self.known_environments, ProfileFieldPatch::Keep)
            && matches!(self.default_delivery_target, ProfileFieldPatch::Keep)
    }
}

pub fn apply_patch(current: Option<UserProfile>, patch: &UserProfilePatch) -> Option<UserProfile> {
    let mut profile = current.unwrap_or_default();

    apply_string_patch(&mut profile.preferred_language, &patch.preferred_language);
    apply_string_patch(&mut profile.timezone, &patch.timezone);
    apply_string_patch(&mut profile.default_city, &patch.default_city);
    apply_string_patch(&mut profile.communication_style, &patch.communication_style);

    match &patch.known_environments {
        ProfileFieldPatch::Keep => {}
        ProfileFieldPatch::Set(values) => {
            profile.known_environments = normalize_list(values);
        }
        ProfileFieldPatch::Clear => {
            profile.known_environments.clear();
        }
    }

    match &patch.default_delivery_target {
        ProfileFieldPatch::Keep => {}
        ProfileFieldPatch::Set(target) => {
            profile.default_delivery_target = Some(target.clone());
        }
        ProfileFieldPatch::Clear => {
            profile.default_delivery_target = None;
        }
    }

    if profile.is_empty() {
        None
    } else {
        Some(profile)
    }
}

pub fn format_profile_projection(profile: &UserProfile) -> String {
    let mut lines = Vec::new();
    if let Some(value) = profile.preferred_language.as_deref() {
        lines.push(format!("- preferred_language: {value}"));
    }
    if let Some(value) = profile.timezone.as_deref() {
        lines.push(format!("- timezone: {value}"));
    }
    if let Some(value) = profile.default_city.as_deref() {
        lines.push(format!("- default_city: {value}"));
    }
    if let Some(value) = profile.communication_style.as_deref() {
        lines.push(format!("- communication_style: {value}"));
    }
    if !profile.known_environments.is_empty() {
        lines.push(format!(
            "- known_environments: {}",
            profile.known_environments.join(", ")
        ));
    }
    if let Some(target) = profile.default_delivery_target.as_ref() {
        lines.push(format!(
            "- default_delivery_target: {}",
            format_delivery_target(target)
        ));
    }

    if lines.is_empty() {
        "(empty)".into()
    } else {
        lines.join("\n")
    }
}

fn apply_string_patch(target: &mut Option<String>, patch: &ProfileFieldPatch<String>) {
    match patch {
        ProfileFieldPatch::Keep => {}
        ProfileFieldPatch::Set(value) => {
            *target = normalize_text(value);
        }
        ProfileFieldPatch::Clear => {
            *target = None;
        }
    }
}

fn normalize_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_list(values: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            normalized.push(trimmed.to_string());
        }
    }
    normalized
}

fn format_delivery_target(target: &ConversationDeliveryTarget) -> String {
    match target {
        ConversationDeliveryTarget::CurrentConversation => "current_conversation".into(),
        ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } => {
            if thread_ref.is_some() {
                format!("explicit:{channel}:{recipient}#thread")
            } else {
                format!("explicit:{channel}:{recipient}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_sets_and_clears_fields() {
        let updated = apply_patch(
            Some(UserProfile {
                preferred_language: Some("en".into()),
                timezone: Some("UTC".into()),
                ..Default::default()
            }),
            &UserProfilePatch {
                preferred_language: ProfileFieldPatch::Set(" ru ".into()),
                timezone: ProfileFieldPatch::Clear,
                default_city: ProfileFieldPatch::Set("Berlin".into()),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(updated.preferred_language.as_deref(), Some("ru"));
        assert!(updated.timezone.is_none());
        assert_eq!(updated.default_city.as_deref(), Some("Berlin"));
    }

    #[test]
    fn patch_normalizes_environments_and_drops_empty_profile() {
        let updated = apply_patch(
            None,
            &UserProfilePatch {
                known_environments: ProfileFieldPatch::Set(vec![
                    "prod".into(),
                    "Prod".into(),
                    " staging ".into(),
                    "".into(),
                ]),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(updated.known_environments, vec!["prod", "staging"]);

        let cleared = apply_patch(
            Some(updated),
            &UserProfilePatch {
                known_environments: ProfileFieldPatch::Clear,
                ..Default::default()
            },
        );
        assert!(cleared.is_none());
    }

    #[test]
    fn projection_formats_human_readable_block() {
        let projection = format_profile_projection(&UserProfile {
            preferred_language: Some("ru".into()),
            default_city: Some("Berlin".into()),
            ..Default::default()
        });

        assert!(projection.contains("preferred_language: ru"));
        assert!(projection.contains("default_city: Berlin"));
    }
}
