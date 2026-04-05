//! Dialogue state service — session-scoped working memory store.
//!
//! This service is intentionally conservative. It does not infer
//! cities/languages/timezones from free text. Typed state is updated from
//! structured runtime facts such as tool-call arguments.

use crate::domain::dialogue_state::DialogueState;
use crate::ports::agent_runtime::AgentToolFact;
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
/// This only refreshes timestamps and stores structured subjects when
/// available. It deliberately avoids lexical extraction from user text.
pub fn update_state_from_turn(
    state: &mut DialogueState,
    _user_message: &str,
    tool_facts: &[AgentToolFact],
    _assistant_response: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.updated_at = now;

    if tool_facts.is_empty() {
        return;
    }

    let focus_entities = collect_focus_entities(tool_facts);
    if !focus_entities.is_empty() {
        state.focus_entities = focus_entities.clone();
        state.comparison_set = if focus_entities.len() > 1
            && focus_entities
                .iter()
                .all(|entity| entity.kind == focus_entities[0].kind)
        {
            focus_entities
        } else {
            Vec::new()
        };
    }

    for slot in collect_slots(tool_facts) {
        if let Some(existing) = state
            .slots
            .iter_mut()
            .find(|existing| existing.name == slot.name)
        {
            *existing = slot;
        } else {
            state.slots.push(slot);
        }
    }

    let subjects = collect_subjects(tool_facts);
    if !subjects.is_empty() {
        state.last_tool_subjects = subjects;
    }
}

pub fn should_materialize_state(
    existing: Option<&DialogueState>,
    tool_facts: &[AgentToolFact],
) -> bool {
    existing.is_some() || !tool_facts.is_empty()
}

fn collect_focus_entities(
    tool_facts: &[AgentToolFact],
) -> Vec<crate::domain::dialogue_state::FocusEntity> {
    let mut entities = Vec::new();
    for fact in tool_facts {
        for entity in &fact.focus_entities {
            if !entities
                .iter()
                .any(|existing: &crate::domain::dialogue_state::FocusEntity| {
                    existing.kind == entity.kind && existing.name == entity.name
                })
            {
                entities.push(entity.clone());
            }
        }
    }
    entities
}

fn collect_slots(tool_facts: &[AgentToolFact]) -> Vec<crate::domain::dialogue_state::DialogueSlot> {
    let mut slots = Vec::new();
    for fact in tool_facts {
        for slot in &fact.slots {
            if let Some(existing_idx) =
                slots
                    .iter()
                    .position(|existing: &crate::domain::dialogue_state::DialogueSlot| {
                        existing.name == slot.name
                    })
            {
                slots[existing_idx] = slot.clone();
            } else {
                slots.push(slot.clone());
            }
        }
    }
    slots
}

fn collect_subjects(tool_facts: &[AgentToolFact]) -> Vec<String> {
    let mut subjects = Vec::new();

    for fact in tool_facts {
        for entity in &fact.focus_entities {
            if !subjects.iter().any(|existing| existing == &entity.name) {
                subjects.push(entity.name.clone());
            }
        }
        for slot in &fact.slots {
            if !subjects.iter().any(|existing| existing == &slot.value) {
                subjects.push(slot.value.clone());
            }
        }
        if fact.focus_entities.is_empty()
            && fact.slots.is_empty()
            && !subjects.iter().any(|existing| existing == &fact.tool_name)
        {
            subjects.push(fact.tool_name.clone());
        }
    }

    subjects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dialogue_state::FocusEntity;
    use crate::ports::agent_runtime::AgentToolFact;

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
        update_state_from_turn(
            &mut state,
            "",
            &[AgentToolFact {
                tool_name: "weather_lookup".into(),
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
                slots: vec![],
            }],
            "",
        );
        assert_eq!(state.last_tool_subjects, vec!["Berlin", "Tbilisi"]);
        assert_eq!(state.focus_entities.len(), 2);
        assert_eq!(state.comparison_set.len(), 2);
    }

    #[test]
    fn materialize_only_when_existing_or_tools_present() {
        assert!(!should_materialize_state(None, &[]));
        assert!(should_materialize_state(
            None,
            &[AgentToolFact {
                tool_name: "shell".into(),
                ..Default::default()
            }]
        ));
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
