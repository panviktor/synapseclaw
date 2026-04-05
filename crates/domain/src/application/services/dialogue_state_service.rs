//! Dialogue state service — session-scoped working memory store.
//!
//! This service is intentionally conservative. It does not infer
//! cities/languages/timezones from free text. Typed state is updated from
//! structured runtime facts such as tool-call arguments.

use crate::domain::conversation_target::CurrentConversationContext;
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
    current_conversation: Option<&CurrentConversationContext>,
    _assistant_response: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.updated_at = now;

    if tool_facts.is_empty() && current_conversation.is_none() {
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
        upsert_slot(state, slot);
    }

    state
        .slots
        .retain(|slot| !is_derived_reference_slot(&slot.name));
    for slot in derive_reference_slots(&state.focus_entities, &state.comparison_set) {
        upsert_slot(state, slot);
    }

    for slot in current_target_slots(current_conversation) {
        upsert_slot(state, slot);
    }

    let subjects = collect_subjects(tool_facts);
    if !subjects.is_empty() {
        state.last_tool_subjects = subjects;
    }
}

pub fn should_materialize_state(
    existing: Option<&DialogueState>,
    tool_facts: &[AgentToolFact],
    current_conversation: Option<&CurrentConversationContext>,
) -> bool {
    existing.is_some() || !tool_facts.is_empty() || current_conversation.is_some()
}

fn upsert_slot(state: &mut DialogueState, slot: crate::domain::dialogue_state::DialogueSlot) {
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

fn current_target_slots(
    current_conversation: Option<&CurrentConversationContext>,
) -> Vec<crate::domain::dialogue_state::DialogueSlot> {
    let Some(ctx) = current_conversation else {
        return Vec::new();
    };

    vec![
        crate::domain::dialogue_state::DialogueSlot {
            name: "current_delivery_target".into(),
            value: "current_conversation".into(),
        },
        crate::domain::dialogue_state::DialogueSlot {
            name: "current_adapter".into(),
            value: ctx.source_adapter.clone(),
        },
        crate::domain::dialogue_state::DialogueSlot {
            name: "current_reply_mode".into(),
            value: if ctx.thread_ref.is_some() {
                "thread".into()
            } else {
                "conversation".into()
            },
        },
    ]
}

fn derive_reference_slots(
    focus_entities: &[crate::domain::dialogue_state::FocusEntity],
    comparison_set: &[crate::domain::dialogue_state::FocusEntity],
) -> Vec<crate::domain::dialogue_state::DialogueSlot> {
    let source = if !comparison_set.is_empty() {
        comparison_set
    } else {
        focus_entities
    };
    if source.is_empty() {
        return Vec::new();
    }

    let mut slots = Vec::new();
    let single_kind = source
        .first()
        .map(|first| source.iter().all(|entity| entity.kind == first.kind))
        .unwrap_or(false);

    if source.len() == 1 {
        let entity = &source[0];
        slots.push(crate::domain::dialogue_state::DialogueSlot {
            name: "current_item".into(),
            value: entity.name.clone(),
        });
        slots.push(crate::domain::dialogue_state::DialogueSlot {
            name: format!("current_{}", entity.kind),
            value: entity.name.clone(),
        });
    } else {
        slots.push(crate::domain::dialogue_state::DialogueSlot {
            name: "comparison_count".into(),
            value: source.len().to_string(),
        });
        if single_kind {
            slots.push(crate::domain::dialogue_state::DialogueSlot {
                name: "comparison_kind".into(),
                value: source[0].kind.clone(),
            });
        }

        for (idx, entity) in source.iter().enumerate().take(4) {
            let Some(label) = ordinal_label(idx) else {
                continue;
            };
            slots.push(crate::domain::dialogue_state::DialogueSlot {
                name: format!("{label}_item"),
                value: entity.name.clone(),
            });
            slots.push(crate::domain::dialogue_state::DialogueSlot {
                name: format!("{label}_{}", entity.kind),
                value: entity.name.clone(),
            });
        }
    }

    if let Some(last) = source.last() {
        slots.push(crate::domain::dialogue_state::DialogueSlot {
            name: "latest_item".into(),
            value: last.name.clone(),
        });
        slots.push(crate::domain::dialogue_state::DialogueSlot {
            name: format!("latest_{}", last.kind),
            value: last.name.clone(),
        });
    }

    slots
}

fn ordinal_label(index: usize) -> Option<&'static str> {
    match index {
        0 => Some("first"),
        1 => Some("second"),
        2 => Some("third"),
        3 => Some("fourth"),
        _ => None,
    }
}

fn is_derived_reference_slot(name: &str) -> bool {
    matches!(
        name,
        "current_item"
            | "latest_item"
            | "comparison_count"
            | "comparison_kind"
            | "first_item"
            | "second_item"
            | "third_item"
            | "fourth_item"
    ) || name.starts_with("current_")
        || name.starts_with("latest_")
        || name.starts_with("first_")
        || name.starts_with("second_")
        || name.starts_with("third_")
        || name.starts_with("fourth_")
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
            None,
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
            None,
            "",
        );
        assert_eq!(state.last_tool_subjects, vec!["Berlin", "Tbilisi"]);
        assert_eq!(state.focus_entities.len(), 2);
        assert_eq!(state.comparison_set.len(), 2);
        assert!(state
            .slots
            .iter()
            .any(|slot| slot.name == "first_city" && slot.value == "Berlin"));
        assert!(state
            .slots
            .iter()
            .any(|slot| slot.name == "second_city" && slot.value == "Tbilisi"));
        assert!(state
            .slots
            .iter()
            .any(|slot| slot.name == "latest_item" && slot.value == "Tbilisi"));
    }

    #[test]
    fn derives_current_focus_slots_for_single_entity() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[AgentToolFact {
                tool_name: "service_status".into(),
                focus_entities: vec![FocusEntity {
                    kind: "service".into(),
                    name: "synapseclaw.service".into(),
                    metadata: None,
                }],
                slots: vec![],
            }],
            None,
            "",
        );

        assert!(state
            .slots
            .iter()
            .any(|slot| slot.name == "current_service" && slot.value == "synapseclaw.service"));
        assert!(state
            .slots
            .iter()
            .any(|slot| slot.name == "current_item" && slot.value == "synapseclaw.service"));
    }

    #[test]
    fn refreshes_derived_slots_when_focus_shape_changes() {
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
            None,
            "",
        );
        assert!(state.slots.iter().any(|slot| slot.name == "second_city"));

        update_state_from_turn(
            &mut state,
            "",
            &[AgentToolFact {
                tool_name: "service_status".into(),
                focus_entities: vec![FocusEntity {
                    kind: "service".into(),
                    name: "synapseclaw.service".into(),
                    metadata: None,
                }],
                slots: vec![],
            }],
            None,
            "",
        );

        assert!(!state.slots.iter().any(|slot| slot.name == "second_city"));
        assert!(state
            .slots
            .iter()
            .any(|slot| slot.name == "current_service" && slot.value == "synapseclaw.service"));
    }

    #[test]
    fn materialize_only_when_existing_or_tools_present() {
        assert!(!should_materialize_state(None, &[], None));
        assert!(should_materialize_state(
            None,
            &[AgentToolFact {
                tool_name: "shell".into(),
                ..Default::default()
            }],
            None
        ));
        assert!(should_materialize_state(
            Some(&DialogueState::default()),
            &[],
            None
        ));
        assert!(should_materialize_state(
            None,
            &[],
            Some(&CurrentConversationContext {
                source_adapter: "matrix".into(),
                conversation_ref: "matrix_room".into(),
                reply_ref: "!room:example.com".into(),
                thread_ref: Some("$thread".into()),
                actor_id: "alice".into(),
            })
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

    #[test]
    fn adds_current_target_slots_without_tool_facts() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[],
            Some(&CurrentConversationContext {
                source_adapter: "matrix".into(),
                conversation_ref: "matrix_room".into(),
                reply_ref: "!room:example.com".into(),
                thread_ref: Some("$thread".into()),
                actor_id: "alice".into(),
            }),
            "",
        );

        assert_eq!(
            state.slot("current_delivery_target"),
            Some("current_conversation")
        );
        assert_eq!(state.slot("current_adapter"), Some("matrix"));
        assert_eq!(state.slot("current_reply_mode"), Some("thread"));
    }
}
