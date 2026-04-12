//! Dynamic user profile — durable user facts scoped by runtime user key.
//!
//! The profile stores arbitrary facts. Runtime code may agree on well-known
//! keys, but the domain model must not freeze them as Rust fields.

use crate::domain::conversation_target::ConversationDeliveryTarget;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    #[serde(default)]
    pub facts: BTreeMap<String, Value>,
}

impl UserProfile {
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }

    pub fn has_fact(&self, key: &str) -> bool {
        normalize_fact_key(key).is_some_and(|key| self.facts.contains_key(&key))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        let key = normalize_fact_key(key)?;
        self.facts.get(&key)
    }

    pub fn get_text(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::String(value) => Some(value.clone()),
            value if value.is_null() => None,
            value => Some(value.to_string()),
        }
    }

    pub fn get_string_list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(|value| match value {
                    Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
                    _ => None,
                })
                .collect(),
            Some(Value::String(value)) if !value.trim().is_empty() => vec![value.trim().into()],
            _ => Vec::new(),
        }
    }

    pub fn get_delivery_target(&self, key: &str) -> Option<ConversationDeliveryTarget> {
        serde_json::from_value(self.get(key)?.clone()).ok()
    }

    pub fn set(&mut self, key: impl AsRef<str>, value: Value) -> bool {
        let Some(key) = normalize_fact_key(key.as_ref()) else {
            return false;
        };
        let Some(value) = normalize_fact_value(value) else {
            self.facts.remove(&key);
            return false;
        };
        self.facts.insert(key, value);
        true
    }

    pub fn clear(&mut self, key: &str) -> bool {
        normalize_fact_key(key)
            .and_then(|key| self.facts.remove(&key))
            .is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.facts.iter()
    }
}

pub fn normalize_fact_key(key: &str) -> Option<String> {
    let normalized = key.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    (!normalized.is_empty()).then_some(normalized)
}

pub fn normalize_fact_value(value: Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| Value::String(trimmed.to_string()))
        }
        Value::Array(values) => {
            let mut normalized = Vec::new();
            for value in values {
                if let Some(value) = normalize_fact_value(value) {
                    if !normalized.iter().any(|existing| existing == &value) {
                        normalized.push(value);
                    }
                }
            }
            (!normalized.is_empty()).then_some(Value::Array(normalized))
        }
        Value::Object(values) => (!values.is_empty()).then_some(Value::Object(values)),
        value => Some(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_profile_detected() {
        assert!(UserProfile::default().is_empty());
    }

    #[test]
    fn populated_profile_is_not_empty() {
        let mut profile = UserProfile::default();
        profile.set("language_preference", json!("ru"));
        assert!(!profile.is_empty());
    }

    #[test]
    fn facts_are_dynamic_and_normalized() {
        let mut profile = UserProfile::default();
        profile.set("Weather City", json!(" Berlin "));
        profile.set("deployment-environments", json!(["prod", "Prod", "", null]));

        assert_eq!(profile.get_text("weather_city").as_deref(), Some("Berlin"));
        assert_eq!(
            profile.get_string_list("deployment_environments"),
            vec!["prod", "Prod"]
        );
    }

    #[test]
    fn delivery_target_fact_roundtrips() {
        let mut profile = UserProfile::default();
        profile.set(
            "delivery_target_preference",
            serde_json::to_value(ConversationDeliveryTarget::CurrentConversation).unwrap(),
        );

        assert!(matches!(
            profile.get_delivery_target("delivery_target_preference"),
            Some(ConversationDeliveryTarget::CurrentConversation)
        ));
    }
}
