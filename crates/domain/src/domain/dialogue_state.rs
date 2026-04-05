//! Dialogue state — ephemeral session-scoped working memory.
//!
//! Tracks active entities, comparison sets, and slots within a conversation
//! so short follow-ups ("and the second one?", "restart it") resolve without
//! relying on long-term memory alone.
//!
//! This is NOT long-term memory. It lives in-memory with TTL expiry and is
//! never promoted to the core_memory or episode tables.

use serde::{Deserialize, Serialize};

/// Session-scoped dialogue state — the "what are we talking about?" layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialogueState {
    /// Entities currently in focus (city, service, file, branch, etc.).
    pub focus_entities: Vec<FocusEntity>,
    /// When the user compared two things (Berlin vs Tbilisi, staging vs prod).
    pub comparison_set: Vec<FocusEntity>,
    /// Structured slots filled during the conversation (location, timezone, etc.).
    pub slots: Vec<DialogueSlot>,
    /// Structured subjects from the last tool execution.
    pub last_tool_subjects: Vec<String>,
    /// Timestamp of last update (unix secs).
    pub updated_at: u64,
}

/// An entity currently in conversational focus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusEntity {
    /// What kind of thing: "city", "service", "file", "branch", "person", etc.
    pub kind: String,
    /// The name/value: "Berlin", "synapseclaw.service", "main", etc.
    pub name: String,
    /// Optional extra metadata.
    pub metadata: Option<String>,
}

/// A named slot filled during conversation (like Rasa slots).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogueSlot {
    /// Slot name: "location", "service_name", "environment", "time_range".
    pub name: String,
    /// Slot value.
    pub value: String,
}

impl DialogueState {
    /// Check if there's a single dominant focus entity.
    pub fn single_focus(&self) -> Option<&FocusEntity> {
        if self.focus_entities.len() == 1 {
            self.focus_entities.first()
        } else {
            None
        }
    }

    /// Check if there's a comparison set (2+ entities of same kind).
    pub fn has_comparison(&self) -> bool {
        self.comparison_set.len() >= 2
    }

    /// Get slot value by name.
    pub fn slot(&self, name: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.value.as_str())
    }

    /// Whether the state is stale (older than TTL seconds).
    pub fn is_stale(&self, ttl_secs: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(self.updated_at) > ttl_secs
    }

    /// Clear all state.
    pub fn clear(&mut self) {
        self.focus_entities.clear();
        self.comparison_set.clear();
        self.slots.clear();
        self.last_tool_subjects.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_focus_when_one_entity() {
        let state = DialogueState {
            focus_entities: vec![FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            ..Default::default()
        };
        assert!(state.single_focus().is_some());
        assert_eq!(state.single_focus().unwrap().name, "Berlin");
    }

    #[test]
    fn no_single_focus_when_multiple() {
        let state = DialogueState {
            focus_entities: vec![
                FocusEntity {
                    kind: "city".into(),
                    name: "Berlin".into(),
                    metadata: None,
                },
                FocusEntity {
                    kind: "city".into(),
                    name: "Tbilisi".into(),
                    metadata: None,
                },
            ],
            ..Default::default()
        };
        assert!(state.single_focus().is_none());
    }

    #[test]
    fn comparison_set() {
        let state = DialogueState {
            comparison_set: vec![
                FocusEntity {
                    kind: "city".into(),
                    name: "Berlin".into(),
                    metadata: None,
                },
                FocusEntity {
                    kind: "city".into(),
                    name: "Tbilisi".into(),
                    metadata: None,
                },
            ],
            ..Default::default()
        };
        assert!(state.has_comparison());
    }

    #[test]
    fn slot_lookup() {
        let state = DialogueState {
            slots: vec![DialogueSlot {
                name: "location".into(),
                value: "Moscow".into(),
            }],
            ..Default::default()
        };
        assert_eq!(state.slot("location"), Some("Moscow"));
        assert_eq!(state.slot("timezone"), None);
    }
}
