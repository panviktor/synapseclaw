//! Clarification policy — structured guidance for narrow disambiguation.
//!
//! This uses typed state only. No phrase tables or locale-specific triggers.

use crate::application::services::resolution_router::{ClarificationReason, ResolutionPlan};
use crate::application::services::turn_interpretation::TurnInterpretation;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClarificationGuidance {
    pub candidate_set: Vec<String>,
    pub required: bool,
    pub avoid_generic_questions: bool,
    pub reason: Option<String>,
}

pub fn build_clarification_guidance(
    plan: Option<&ResolutionPlan>,
    interpretation: Option<&TurnInterpretation>,
) -> Option<ClarificationGuidance> {
    let interpretation = interpretation?;
    let reason = plan.and_then(|plan| plan.clarification_reason);

    let mut guidance = ClarificationGuidance {
        avoid_generic_questions: true,
        required: reason.is_some(),
        ..Default::default()
    };

    if !interpretation.clarification_candidates.is_empty() {
        guidance.candidate_set = interpretation.clarification_candidates.clone();
    }

    guidance.reason = reason.map(reason_name).map(str::to_string);

    if guidance.candidate_set.is_empty() && !guidance.required {
        None
    } else {
        Some(guidance)
    }
}

pub fn format_clarification_guidance(guidance: &ClarificationGuidance) -> Option<String> {
    if guidance.candidate_set.is_empty() && !guidance.required && guidance.reason.is_none() {
        return None;
    }

    let mut lines = vec!["[clarification-policy]".to_string()];
    if guidance.avoid_generic_questions {
        lines.push("- avoid_generic_questions: true".to_string());
    }
    if guidance.required {
        lines.push("- clarification_required: true".to_string());
    }
    if !guidance.candidate_set.is_empty() {
        lines.push(format!(
            "- if_disambiguation_is_needed: {}",
            guidance.candidate_set.join(" | ")
        ));
    }
    if let Some(reason) = guidance.reason.as_deref() {
        lines.push(format!("- reason: {reason}"));
    }

    Some(format!("{}\n", lines.join("\n")))
}

fn reason_name(reason: ClarificationReason) -> &'static str {
    match reason {
        ClarificationReason::ResolverExhausted => "resolver_exhausted",
        ClarificationReason::LowConfidence => "low_confidence",
        ClarificationReason::AmbiguousCandidates => "ambiguous_candidates",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::{
        DialogueStateSnapshot, TurnInterpretation,
    };
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn builds_guidance_from_candidates() {
        let interpretation = TurnInterpretation {
            user_profile: Some({
                let mut profile = UserProfile::default();
                profile.set("response_locale", serde_json::json!("ru"));
                profile.set("project_alias", serde_json::json!("Borealis"));
                profile.set("workspace_anchor", serde_json::json!("Borealis"));
                profile
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![],
                comparison_set: vec![
                    ("city".into(), "Berlin".into()),
                    ("city".into(), "Tbilisi".into()),
                ],
                reference_anchors: vec![],
                last_tool_subjects: vec![],
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            clarification_candidates: vec!["Berlin".into(), "Tbilisi".into()],
            reference_candidates: vec![],
            configured_delivery_target: None,
            current_conversation: None,
        };

        let guidance = build_clarification_guidance(
            Some(&ResolutionPlan {
                source_order: vec![],
                confidence:
                    crate::application::services::resolution_router::ResolutionConfidence::Low,
                clarify_after_exhaustion: true,
                clarification_reason: Some(ClarificationReason::AmbiguousCandidates),
            }),
            Some(&interpretation),
        )
        .unwrap();
        assert_eq!(guidance.candidate_set, vec!["Berlin", "Tbilisi"]);
        assert!(guidance.required);
        assert_eq!(guidance.reason.as_deref(), Some("ambiguous_candidates"));
    }

    #[test]
    fn formats_policy_block() {
        let block = format_clarification_guidance(&ClarificationGuidance {
            candidate_set: vec!["Berlin".into(), "Tbilisi".into()],
            required: true,
            avoid_generic_questions: true,
            reason: Some("low_confidence".into()),
        })
        .unwrap();

        assert!(block.contains("[clarification-policy]"));
        assert!(block.contains("clarification_required: true"));
        assert!(block.contains("Berlin | Tbilisi"));
        assert!(block.contains("reason: low_confidence"));
    }

    #[test]
    fn formats_required_reason_without_candidate_set() {
        let block = format_clarification_guidance(&ClarificationGuidance {
            candidate_set: vec![],
            required: true,
            avoid_generic_questions: true,
            reason: Some("resolver_exhausted".into()),
        })
        .unwrap();

        assert!(block.contains("[clarification-policy]"));
        assert!(block.contains("clarification_required: true"));
        assert!(block.contains("reason: resolver_exhausted"));
    }
}
