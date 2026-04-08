//! Resolve per-turn typed defaults from structured interpretation.

use crate::application::services::turn_interpretation::TurnInterpretation;
use crate::domain::turn_defaults::{
    ResolvedDeliveryTarget, ResolvedTurnDefaults, TurnDefaultSource,
};

pub fn resolve_turn_defaults(interpretation: Option<&TurnInterpretation>) -> ResolvedTurnDefaults {
    let Some(interpretation) = interpretation else {
        return ResolvedTurnDefaults::default();
    };

    if let Some(target) = interpretation
        .dialogue_state
        .as_ref()
        .and_then(|state| state.recent_delivery_target.clone())
    {
        return ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target,
                source: TurnDefaultSource::DialogueState,
            }),
        };
    }

    if let Some(target) = interpretation
        .user_profile
        .as_ref()
        .and_then(|profile| profile.default_delivery_target.clone())
    {
        return ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target,
                source: TurnDefaultSource::UserProfile,
            }),
        };
    }

    ResolvedTurnDefaults::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::{
        DialogueStateSnapshot, TurnInterpretation,
    };
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn prefers_dialogue_state_target_over_profile_default() {
        let interpretation = TurnInterpretation {
            user_profile: Some(UserProfile {
                default_delivery_target: Some(ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!profile:example.com".into(),
                    thread_ref: None,
                }),
                ..UserProfile::default()
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: Vec::new(),
                comparison_set: Vec::new(),
                reference_anchors: Vec::new(),
                last_tool_subjects: Vec::new(),
                recent_delivery_target: Some(ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!recent:example.com".into(),
                    thread_ref: None,
                }),
                recent_schedule_job: None,
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            ..TurnInterpretation::default()
        };

        let defaults = resolve_turn_defaults(Some(&interpretation));
        let delivery = defaults.delivery_target.expect("delivery default");
        assert_eq!(delivery.source, TurnDefaultSource::DialogueState);
        match delivery.target {
            ConversationDeliveryTarget::Explicit { recipient, .. } => {
                assert_eq!(recipient, "!recent:example.com");
            }
            ConversationDeliveryTarget::CurrentConversation => panic!("expected explicit target"),
        }
    }

    #[test]
    fn falls_back_to_profile_default_when_dialogue_state_missing() {
        let interpretation = TurnInterpretation {
            user_profile: Some(UserProfile {
                default_delivery_target: Some(ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!profile:example.com".into(),
                    thread_ref: None,
                }),
                ..UserProfile::default()
            }),
            ..TurnInterpretation::default()
        };

        let defaults = resolve_turn_defaults(Some(&interpretation));
        let delivery = defaults.delivery_target.expect("delivery default");
        assert_eq!(delivery.source, TurnDefaultSource::UserProfile);
    }
}
