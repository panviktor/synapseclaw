//! Dynamic user profile updates and projections.
//!
//! Profiles are arbitrary facts keyed by normalized strings. Runtime consumers
//! may use runtime conventions, but the service does
//! not expose fixed Rust fields.

use crate::domain::user_profile::{normalize_fact_key, normalize_fact_value, UserProfile};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "op", content = "value", rename_all = "snake_case")]
pub enum ProfileFactPatch {
    Set(Value),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct UserProfilePatch {
    #[serde(default)]
    pub facts: BTreeMap<String, ProfileFactPatch>,
}

impl UserProfilePatch {
    pub fn is_noop(&self) -> bool {
        self.facts.is_empty()
    }

    pub fn set(&mut self, key: impl AsRef<str>, value: Value) -> bool {
        let Some(key) = normalize_fact_key(key.as_ref()) else {
            return false;
        };
        let Some(value) = normalize_fact_value(value) else {
            self.facts.insert(key, ProfileFactPatch::Clear);
            return false;
        };
        self.facts.insert(key, ProfileFactPatch::Set(value));
        true
    }

    pub fn clear(&mut self, key: impl AsRef<str>) -> bool {
        let Some(key) = normalize_fact_key(key.as_ref()) else {
            return false;
        };
        self.facts.insert(key, ProfileFactPatch::Clear);
        true
    }
}

pub fn apply_patch(current: Option<UserProfile>, patch: &UserProfilePatch) -> Option<UserProfile> {
    let mut profile = current.unwrap_or_default();

    for (key, patch) in &patch.facts {
        match patch {
            ProfileFactPatch::Set(value) => {
                profile.set(key, value.clone());
            }
            ProfileFactPatch::Clear => {
                profile.clear(key);
            }
        }
    }

    if profile.is_empty() {
        None
    } else {
        Some(profile)
    }
}

pub fn format_profile_projection(profile: &UserProfile) -> String {
    let lines = profile
        .iter()
        .map(|(key, value)| format!("- {key}: {}", format_fact_value(value)))
        .collect::<Vec<_>>();

    if lines.is_empty() {
        "(empty)".into()
    } else {
        lines.join("\n")
    }
}

fn format_fact_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(format_fact_value)
            .collect::<Vec<_>>()
            .join(", "),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patch_sets_and_clears_dynamic_facts() {
        let mut current = UserProfile::default();
        current.set("response_locale", json!("en"));
        current.set("project_alias", json!("Atlas"));

        let mut patch = UserProfilePatch::default();
        patch.set("response locale", json!(" ru "));
        patch.clear("project_alias");
        patch.set("workspace anchor", json!("Borealis"));

        let updated = apply_patch(Some(current), &patch).unwrap();

        assert_eq!(updated.get_text("response_locale").as_deref(), Some("ru"));
        assert!(updated.get("project_alias").is_none());
        assert_eq!(
            updated.get_text("workspace_anchor").as_deref(),
            Some("Borealis")
        );
    }

    #[test]
    fn patch_normalizes_lists_and_drops_empty_profile() {
        let mut patch = UserProfilePatch::default();
        patch.set("release_tracks", json!(["prod", "Prod", " staging ", ""]));

        let updated = apply_patch(None, &patch).unwrap();
        assert_eq!(
            updated.get_string_list("release_tracks"),
            vec!["prod", "Prod", "staging"]
        );

        let mut clear = UserProfilePatch::default();
        clear.clear("release_tracks");
        assert!(apply_patch(Some(updated), &clear).is_none());
    }

    #[test]
    fn projection_formats_human_readable_block() {
        let mut profile = UserProfile::default();
        profile.set("response_locale", json!("ru"));
        profile.set("workspace_anchor", json!("Borealis"));

        let projection = format_profile_projection(&profile);

        assert!(projection.contains("response_locale: ru"));
        assert!(projection.contains("workspace_anchor: Borealis"));
    }
}
