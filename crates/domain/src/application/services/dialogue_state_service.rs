//! Dialogue state service — updates session-scoped working memory.
//!
//! Extracts focus entities, comparison sets, and slots from user messages
//! and tool results. Mostly deterministic (regex/pattern), no LLM calls.

use crate::domain::dialogue_state::{DialogueSlot, DialogueState, FocusEntity};
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
/// Extracts focus entities and comparison patterns from the user message
/// and tool output summary. Deterministic — no LLM call.
pub fn update_state_from_turn(
    state: &mut DialogueState,
    user_message: &str,
    tool_summary: &str,
    assistant_response: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.updated_at = now;

    // Extract entities from tool results (most reliable source)
    if !tool_summary.is_empty() {
        state.last_tool_subjects = extract_tool_subjects(tool_summary);
    }

    // Extract focus entities from user message + response
    let new_entities = extract_entities(user_message, assistant_response);
    if !new_entities.is_empty() {
        // Detect comparison pattern: if 2+ entities of same kind mentioned together
        let kinds: Vec<&str> = new_entities.iter().map(|e| e.kind.as_str()).collect();
        let has_comparison = kinds.len() >= 2
            && kinds.windows(2).any(|w| w[0] == w[1]);

        if has_comparison {
            state.comparison_set = new_entities.clone();
            state.focus_entities = new_entities;
        } else {
            // Replace focus (new topic supersedes old)
            state.focus_entities = new_entities;
            state.comparison_set.clear();
        }
    }

    // Extract slots from user message
    extract_slots(user_message, &mut state.slots);
}

/// Extract entity-like mentions from message text.
fn extract_entities(user_message: &str, assistant_response: &str) -> Vec<FocusEntity> {
    let mut entities = Vec::new();
    let combined = format!("{user_message} {assistant_response}").to_lowercase();

    // Service patterns: "X.service", "service X", "restart X"
    for word in combined.split_whitespace() {
        if word.ends_with(".service") {
            let name = word.trim_end_matches(".service");
            if !name.is_empty() {
                entities.push(FocusEntity {
                    kind: "service".into(),
                    name: name.to_string(),
                    metadata: None,
                });
            }
        }
    }

    // City/location detection via common weather/location patterns
    let location_triggers = ["weather in ", "weather for ", "temperature in ", "located in "];
    let stop_words = ["is", "are", "was", "or", "the", "today", "tomorrow", "now"];
    for trigger in &location_triggers {
        // Find ALL occurrences of this trigger
        let mut search_from = 0;
        while let Some(pos) = combined[search_from..].find(trigger) {
            let abs_pos = search_from + pos;
            let after = &combined[abs_pos + trigger.len()..];
            // Take words until a stop word or punctuation
            let words: Vec<&str> = after
                .split_whitespace()
                .take_while(|w| !stop_words.contains(w) && !w.contains(',') && !w.contains('.'))
                .take(3)
                .collect();
            // Split on "and" to handle "Berlin and Tbilisi"
            let raw = words.join(" ");
            for part in raw.split(" and ") {
                let city = part.trim().to_string();
                if !city.is_empty() && city.len() < 50 {
                    entities.push(FocusEntity {
                        kind: "city".into(),
                        name: city,
                        metadata: None,
                    });
                }
            }
            search_from = abs_pos + trigger.len() + 1;
            if search_from >= combined.len() { break; }
        }
    }

    // Environment patterns: "staging", "production", "prod", "dev"
    let envs = ["staging", "production", "prod", "dev", "development"];
    for env in &envs {
        if combined.contains(env) {
            entities.push(FocusEntity {
                kind: "environment".into(),
                name: env.to_string(),
                metadata: None,
            });
        }
    }

    entities
}

/// Extract named slots from user message.
fn extract_slots(message: &str, slots: &mut Vec<DialogueSlot>) {
    let lower = message.to_lowercase();

    // Timezone slot
    if lower.contains("utc") || lower.contains("timezone") {
        if let Some(tz) = extract_timezone(&lower) {
            upsert_slot(slots, "timezone", &tz);
        }
    }
}

fn extract_timezone(text: &str) -> Option<String> {
    // Match "UTC+3", "UTC-5", "UTC+03:00"
    if let Some(pos) = text.find("utc") {
        let after = &text[pos..];
        let tz: String = after
            .chars()
            .take(10)
            .take_while(|c| c.is_alphanumeric() || *c == '+' || *c == '-' || *c == ':')
            .collect();
        if tz.len() > 3 {
            return Some(tz.to_uppercase());
        }
    }
    None
}

fn upsert_slot(slots: &mut Vec<DialogueSlot>, name: &str, value: &str) {
    if let Some(slot) = slots.iter_mut().find(|s| s.name == name) {
        slot.value = value.to_string();
    } else {
        slots.push(DialogueSlot {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
}

fn extract_tool_subjects(summary: &str) -> Vec<String> {
    // Parse "[Used tools: X, Y]" format
    summary
        .trim_start_matches("[Used tools: ")
        .trim_end_matches(']')
        .split(", ")
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_service_entity() {
        let entities = extract_entities("restart synapseclaw.service", "");
        assert!(entities.iter().any(|e| e.kind == "service" && e.name == "synapseclaw"));
    }

    #[test]
    fn extract_city_from_weather() {
        let entities = extract_entities("", "The weather in Berlin is rainy");
        assert!(entities.iter().any(|e| e.kind == "city" && e.name == "berlin"));
    }

    #[test]
    fn detect_comparison_set() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "compare weather in Berlin and Tbilisi",
            "",
            "Weather in Berlin: 12C. Weather in Tbilisi: 25C.",
        );
        assert!(state.has_comparison());
        assert!(state.comparison_set.len() >= 2, "expected 2+ cities, got {}", state.comparison_set.len());
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
