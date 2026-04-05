//! Clarification policy — structured guidance for narrow disambiguation.
//!
//! This uses typed state only. No phrase tables or locale-specific triggers.

use crate::application::services::turn_interpretation::TurnInterpretation;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClarificationGuidance {
    pub use_defaults_for: Vec<String>,
    pub candidate_set: Vec<String>,
    pub avoid_generic_questions: bool,
}

pub fn build_clarification_guidance(
    interpretation: Option<&TurnInterpretation>,
) -> Option<ClarificationGuidance> {
    let interpretation = interpretation?;

    let mut guidance = ClarificationGuidance {
        avoid_generic_questions: true,
        ..Default::default()
    };

    if let Some(profile) = interpretation.user_profile.as_ref() {
        if profile.default_city.is_some() {
            guidance.use_defaults_for.push("city".into());
        }
        if profile.preferred_language.is_some() {
            guidance.use_defaults_for.push("language".into());
        }
        if profile.timezone.is_some() {
            guidance.use_defaults_for.push("timezone".into());
        }
        if profile.default_delivery_target.is_some() {
            guidance.use_defaults_for.push("delivery_target".into());
        }
    }

    if let Some(state) = interpretation.dialogue_state.as_ref() {
        if state.comparison_set.len() >= 2 {
            guidance.candidate_set = state
                .comparison_set
                .iter()
                .map(|(_, name)| name.clone())
                .collect();
        }
    }

    if guidance.use_defaults_for.is_empty() && guidance.candidate_set.is_empty() {
        None
    } else {
        Some(guidance)
    }
}

pub fn format_clarification_guidance(guidance: &ClarificationGuidance) -> Option<String> {
    if guidance.use_defaults_for.is_empty() && guidance.candidate_set.is_empty() {
        return None;
    }

    let mut lines = vec!["[clarification-policy]".to_string()];
    if guidance.avoid_generic_questions {
        lines.push("- avoid_generic_questions: true".to_string());
    }
    if !guidance.use_defaults_for.is_empty() {
        lines.push(format!(
            "- use_known_defaults_for: {}",
            guidance.use_defaults_for.join(", ")
        ));
    }
    if !guidance.candidate_set.is_empty() {
        lines.push(format!(
            "- if_disambiguation_is_needed: {}",
            guidance.candidate_set.join(" | ")
        ));
    }

    Some(format!("{}\n", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::{
        DialogueStateSnapshot, TurnInterpretation,
    };
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn builds_guidance_from_defaults_and_candidates() {
        let interpretation = TurnInterpretation {
            user_profile: Some(UserProfile {
                preferred_language: Some("ru".into()),
                timezone: Some("Europe/Berlin".into()),
                default_city: Some("Berlin".into()),
                ..Default::default()
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![],
                comparison_set: vec![
                    ("city".into(), "Berlin".into()),
                    ("city".into(), "Tbilisi".into()),
                ],
                slots: vec![],
                last_tool_subjects: vec![],
            }),
            current_conversation: None,
        };

        let guidance = build_clarification_guidance(Some(&interpretation)).unwrap();
        assert_eq!(
            guidance.use_defaults_for,
            vec!["city", "language", "timezone"]
        );
        assert_eq!(guidance.candidate_set, vec!["Berlin", "Tbilisi"]);
    }

    #[test]
    fn formats_policy_block() {
        let block = format_clarification_guidance(&ClarificationGuidance {
            use_defaults_for: vec!["city".into(), "timezone".into()],
            candidate_set: vec!["Berlin".into(), "Tbilisi".into()],
            avoid_generic_questions: true,
        })
        .unwrap();

        assert!(block.contains("[clarification-policy]"));
        assert!(block.contains("use_known_defaults_for: city, timezone"));
        assert!(block.contains("Berlin | Tbilisi"));
    }
}
