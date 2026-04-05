//! Dialogue state service — session-scoped working memory store.
//!
//! This service is intentionally conservative. It does not try to infer
//! cities/languages/timezones from free text via phrase tables. Typed
//! state should be populated by future structured interpreters or explicit
//! tool/runtime events.

use crate::domain::dialogue_state::DialogueState;
use parking_lot::RwLock;
use std::collections::HashMap;

/// TTL for dialogue state entries (30 minutes).
const STATE_TTL_SECS: u64 = 1800;

/// In-memory store for dialogue state, keyed by conversation_ref.
pub struct DialogueStateStore {
    states: RwLock<HashMap<String, DialogueState>>,
}

impl DialogueStateStore {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
        }
    }

    /// Get current state for a conversation (None if absent or stale).
    pub fn get(&self, conversation_ref: &str) -> Option<DialogueState> {
        let states = self.states.read();
        states.get(conversation_ref).and_then(|s| {
            if s.is_stale(STATE_TTL_SECS) {
                None
            } else {
                Some(s.clone())
            }
        })
    }

    /// Update state for a conversation.
    pub fn set(&self, conversation_ref: &str, state: DialogueState) {
        let mut states = self.states.write();
        states.insert(conversation_ref.to_string(), state);
    }

    /// Evict stale entries (call periodically).
    pub fn evict_stale(&self) {
        let mut states = self.states.write();
        states.retain(|_, s| !s.is_stale(STATE_TTL_SECS));
    }
}

impl Default for DialogueStateStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Update dialogue state after a user turn.
///
/// This only refreshes timestamps and stores structured tool subjects when
/// available. It deliberately avoids lexical extraction from user text.
pub fn update_state_from_turn(
    state: &mut DialogueState,
    _user_message: &str,
    tool_names: &[String],
    _assistant_response: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.updated_at = now;

    // Extract entities from tool results (most reliable source)
    if !tool_names.is_empty() {
        state.last_tool_subjects = tool_names.to_vec();
    }
}

pub fn should_materialize_state(existing: Option<&DialogueState>, tool_names: &[String]) -> bool {
    existing.is_some() || !tool_names.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dialogue_state::FocusEntity;

    #[test]
    fn update_state_keeps_existing_focus_without_lexical_extraction() {
        let mut state = DialogueState::default();
        state.focus_entities.push(FocusEntity {
            kind: "service".into(),
            name: "synapseclaw".into(),
            metadata: None,
        });
        update_state_from_turn(
            &mut state,
            "compare weather in Berlin and Tbilisi",
            &[],
            "Weather in Berlin: 12C. Weather in Tbilisi: 25C.",
        );
        assert_eq!(state.focus_entities.len(), 1);
        assert_eq!(state.focus_entities[0].name, "synapseclaw");
        assert!(state.comparison_set.is_empty());
        assert!(state.slots.is_empty());
    }

    #[test]
    fn captures_tool_subjects_when_present() {
        let mut state = DialogueState::default();
        update_state_from_turn(&mut state, "", &["shell".into(), "web_fetch".into()], "");
        assert_eq!(state.last_tool_subjects, vec!["shell", "web_fetch"]);
    }

    #[test]
    fn materialize_only_when_existing_or_tools_present() {
        assert!(!should_materialize_state(None, &[]));
        assert!(should_materialize_state(None, &["shell".into()]));
        assert!(should_materialize_state(
            Some(&DialogueState::default()),
            &[]
        ));
    }

    #[test]
    fn store_get_set() {
        let store = DialogueStateStore::new();
        let mut state = DialogueState::default();
        state.focus_entities.push(FocusEntity {
            kind: "city".into(),
            name: "Moscow".into(),
            metadata: None,
        });
        state.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        store.set("conv1", state);
        let loaded = store.get("conv1");
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().focus_entities[0].name, "Moscow");
    }
}
