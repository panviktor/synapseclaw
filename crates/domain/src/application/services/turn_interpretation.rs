//! Turn interpretation — typed runtime context for a single turn.
//!
//! This is intentionally not a phrase-engine. It formats already structured
//! runtime facts so web and channels can surface the same deterministic
//! context to the model without ad-hoc heuristics.

use crate::domain::conversation_target::{ConversationDeliveryTarget, CurrentConversationContext};
use crate::domain::dialogue_state::DialogueState;
use crate::domain::user_profile::UserProfile;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnInterpretation {
    pub user_profile: Option<UserProfile>,
    pub current_conversation: Option<CurrentConversationSnapshot>,
    pub dialogue_state: Option<DialogueStateSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentConversationSnapshot {
    pub adapter: String,
    pub has_thread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueStateSnapshot {
    pub focus_entities: Vec<(String, String)>,
    pub comparison_set: Vec<(String, String)>,
    pub slots: Vec<(String, String)>,
    pub last_tool_subjects: Vec<String>,
}

pub fn build_turn_interpretation(
    profile: Option<UserProfile>,
    current_conversation: Option<&CurrentConversationContext>,
    dialogue_state: Option<&DialogueState>,
) -> Option<TurnInterpretation> {
    let user_profile = profile.filter(|profile| !profile.is_empty());
    let current_conversation = current_conversation.map(|ctx| CurrentConversationSnapshot {
        adapter: ctx.source_adapter.clone(),
        has_thread: ctx.thread_ref.is_some(),
    });
    let dialogue_state = dialogue_state.and_then(snapshot_dialogue_state);

    let interpretation = TurnInterpretation {
        user_profile,
        current_conversation,
        dialogue_state,
    };

    if interpretation.user_profile.is_none()
        && interpretation.current_conversation.is_none()
        && interpretation.dialogue_state.is_none()
    {
        None
    } else {
        Some(interpretation)
    }
}

pub fn format_turn_interpretation(interpretation: &TurnInterpretation) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(profile) = &interpretation.user_profile {
        lines.push("[user-profile]".to_string());
        if let Some(language) = &profile.preferred_language {
            lines.push(format!("- preferred_language: {language}"));
        }
        if let Some(timezone) = &profile.timezone {
            lines.push(format!("- timezone: {timezone}"));
        }
        if let Some(city) = &profile.default_city {
            lines.push(format!("- default_city: {city}"));
        }
        if let Some(style) = &profile.communication_style {
            lines.push(format!("- communication_style: {style}"));
        }
        if !profile.known_environments.is_empty() {
            lines.push(format!(
                "- known_environments: {}",
                profile.known_environments.join(", ")
            ));
        }
        if let Some(target) = &profile.default_delivery_target {
            lines.push(format!(
                "- default_delivery_target: {}",
                format_delivery_target(target)
            ));
        }
    }

    if let Some(conversation) = &interpretation.current_conversation {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[current-conversation]".to_string());
        lines.push(format!("- adapter: {}", conversation.adapter));
        lines.push("- reply_here_available: true".to_string());
        lines.push(format!("- threaded_reply: {}", conversation.has_thread));
    }

    if let Some(state) = &interpretation.dialogue_state {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[working-state]".to_string());
        if !state.focus_entities.is_empty() {
            lines.push(format!(
                "- focus_entities: {}",
                format_pairs(&state.focus_entities)
            ));
        }
        if !state.comparison_set.is_empty() {
            lines.push(format!(
                "- comparison_set: {}",
                format_pairs(&state.comparison_set)
            ));
        }
        if !state.slots.is_empty() {
            lines.push(format!("- slots: {}", format_pairs(&state.slots)));
        }
        if !state.last_tool_subjects.is_empty() {
            lines.push(format!(
                "- last_tool_subjects: {}",
                state.last_tool_subjects.join(", ")
            ));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("[runtime-interpretation]\n{}\n", lines.join("\n")))
    }
}

fn snapshot_dialogue_state(state: &DialogueState) -> Option<DialogueStateSnapshot> {
    let focus_entities = state
        .focus_entities
        .iter()
        .map(|entity| (entity.kind.clone(), entity.name.clone()))
        .collect::<Vec<_>>();
    let comparison_set = state
        .comparison_set
        .iter()
        .map(|entity| (entity.kind.clone(), entity.name.clone()))
        .collect::<Vec<_>>();
    let slots = state
        .slots
        .iter()
        .map(|slot| (slot.name.clone(), slot.value.clone()))
        .collect::<Vec<_>>();
    let last_tool_subjects = state.last_tool_subjects.clone();

    if focus_entities.is_empty()
        && comparison_set.is_empty()
        && slots.is_empty()
        && last_tool_subjects.is_empty()
    {
        None
    } else {
        Some(DialogueStateSnapshot {
            focus_entities,
            comparison_set,
            slots,
            last_tool_subjects,
        })
    }
}

fn format_pairs(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(left, right)| format!("{left}={right}"))
        .collect::<Vec<_>>()
        .join(", ")
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
    use crate::domain::conversation_target::CurrentConversationContext;
    use crate::domain::dialogue_state::{DialogueSlot, FocusEntity};

    #[test]
    fn returns_none_for_empty_inputs() {
        assert!(build_turn_interpretation(None, None, None).is_none());
    }

    #[test]
    fn formats_profile_and_state() {
        let profile = UserProfile {
            preferred_language: Some("ru".into()),
            timezone: Some("Europe/Berlin".into()),
            ..Default::default()
        };
        let state = DialogueState {
            focus_entities: vec![FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            slots: vec![DialogueSlot {
                name: "timezone".into(),
                value: "Europe/Berlin".into(),
            }],
            last_tool_subjects: vec!["weather_lookup".into()],
            ..Default::default()
        };
        let current = CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_ref: "matrix_room".into(),
            reply_ref: "!room:example.com".into(),
            thread_ref: Some("$thread".into()),
            actor_id: "alice".into(),
        };

        let interpretation =
            build_turn_interpretation(Some(profile), Some(&current), Some(&state)).unwrap();
        let block = format_turn_interpretation(&interpretation).unwrap();

        assert!(block.contains("[runtime-interpretation]"));
        assert!(block.contains("preferred_language: ru"));
        assert!(block.contains("adapter: matrix"));
        assert!(block.contains("focus_entities: city=Berlin"));
        assert!(block.contains("last_tool_subjects: weather_lookup"));
    }
}
