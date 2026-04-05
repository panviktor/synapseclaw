//! Resolution router — deterministic source ordering for a turn.
//!
//! This is intentionally not a phrase-engine. It consumes typed interpretation
//! and retrieval evidence, then decides which sources should be trusted first.

use crate::application::services::turn_interpretation::TurnInterpretation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    CurrentConversation,
    DialogueState,
    UserProfile,
    SessionHistory,
    RunRecipe,
    LongTermMemory,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolutionPlan {
    pub source_order: Vec<ResolutionSource>,
    pub clarify_after_exhaustion: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResolutionEvidence<'a> {
    pub interpretation: Option<&'a TurnInterpretation>,
    pub top_session_score: Option<f64>,
    pub top_recipe_score: Option<i64>,
    pub recall_hits: usize,
    pub skill_hits: usize,
    pub entity_hits: usize,
}

const SESSION_PRIMARY_SCORE: f64 = 1.9;
const RECIPE_PRIMARY_SCORE: i64 = 180;

pub fn build_resolution_plan(evidence: ResolutionEvidence<'_>) -> ResolutionPlan {
    let mut source_order = Vec::new();

    if let Some(interpretation) = evidence.interpretation {
        if interpretation.current_conversation.is_some() {
            source_order.push(ResolutionSource::CurrentConversation);
        }
        if interpretation.dialogue_state.is_some() {
            source_order.push(ResolutionSource::DialogueState);
        }
        if interpretation.user_profile.is_some() {
            source_order.push(ResolutionSource::UserProfile);
        }
    }

    let session_primary = evidence.top_session_score.unwrap_or_default() >= SESSION_PRIMARY_SCORE;
    let recipe_primary = evidence.top_recipe_score.unwrap_or_default() >= RECIPE_PRIMARY_SCORE;
    let long_term_available =
        evidence.recall_hits > 0 || evidence.skill_hits > 0 || evidence.entity_hits > 0;

    match (session_primary, recipe_primary) {
        (true, true) => {
            if evidence.top_recipe_score.unwrap_or_default() >= RECIPE_PRIMARY_SCORE + 40 {
                source_order.push(ResolutionSource::RunRecipe);
                source_order.push(ResolutionSource::SessionHistory);
            } else {
                source_order.push(ResolutionSource::SessionHistory);
                source_order.push(ResolutionSource::RunRecipe);
            }
        }
        (true, false) => source_order.push(ResolutionSource::SessionHistory),
        (false, true) => source_order.push(ResolutionSource::RunRecipe),
        (false, false) => {}
    }

    if long_term_available {
        source_order.push(ResolutionSource::LongTermMemory);
    }

    if !session_primary && evidence.top_session_score.unwrap_or_default() > 0.0 {
        source_order.push(ResolutionSource::SessionHistory);
    }
    if !recipe_primary && evidence.top_recipe_score.unwrap_or_default() > 0 {
        source_order.push(ResolutionSource::RunRecipe);
    }

    dedupe_source_order(&mut source_order);

    ResolutionPlan {
        source_order,
        clarify_after_exhaustion: true,
    }
}

pub fn format_resolution_plan(plan: &ResolutionPlan) -> Option<String> {
    if plan.source_order.is_empty() {
        return None;
    }

    let mut lines = vec!["[resolution-plan]".to_string()];
    lines.push(format!(
        "- source_order: {}",
        plan.source_order
            .iter()
            .map(|source| source_name(*source))
            .collect::<Vec<_>>()
            .join(" -> ")
    ));
    if plan.clarify_after_exhaustion {
        lines.push("- clarify_only_after: source_exhaustion_or_low_confidence".to_string());
    }

    Some(format!("{}\n", lines.join("\n")))
}

fn dedupe_source_order(source_order: &mut Vec<ResolutionSource>) {
    let mut deduped = Vec::new();
    for source in source_order.drain(..) {
        if !deduped.contains(&source) {
            deduped.push(source);
        }
    }
    *source_order = deduped;
}

pub fn source_priority(plan: Option<&ResolutionPlan>, source: ResolutionSource) -> usize {
    plan.and_then(|plan| plan.source_order.iter().position(|item| *item == source))
        .unwrap_or(usize::MAX)
}

fn source_name(source: ResolutionSource) -> &'static str {
    match source {
        ResolutionSource::CurrentConversation => "current_conversation",
        ResolutionSource::DialogueState => "dialogue_state",
        ResolutionSource::UserProfile => "user_profile",
        ResolutionSource::SessionHistory => "session_history",
        ResolutionSource::RunRecipe => "run_recipe",
        ResolutionSource::LongTermMemory => "long_term_memory",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::{
        CurrentConversationSnapshot, DialogueStateSnapshot, TurnInterpretation,
    };
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn prioritizes_structured_state_then_specialized_retrieval() {
        let interpretation = TurnInterpretation {
            user_profile: Some(UserProfile {
                preferred_language: Some("ru".into()),
                ..Default::default()
            }),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: true,
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![("city".into(), "Berlin".into())],
                comparison_set: vec![],
                slots: vec![],
                last_tool_subjects: vec![],
            }),
        };

        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: Some(&interpretation),
            top_session_score: Some(2.2),
            top_recipe_score: Some(245),
            recall_hits: 1,
            skill_hits: 0,
            entity_hits: 0,
        });

        assert_eq!(
            plan.source_order,
            vec![
                ResolutionSource::CurrentConversation,
                ResolutionSource::DialogueState,
                ResolutionSource::UserProfile,
                ResolutionSource::RunRecipe,
                ResolutionSource::SessionHistory,
                ResolutionSource::LongTermMemory,
            ]
        );
    }

    #[test]
    fn formats_resolution_block() {
        let plan = ResolutionPlan {
            source_order: vec![
                ResolutionSource::DialogueState,
                ResolutionSource::LongTermMemory,
            ],
            clarify_after_exhaustion: true,
        };
        let block = format_resolution_plan(&plan).unwrap();
        assert!(block.contains("[resolution-plan]"));
        assert!(block.contains("dialogue_state -> long_term_memory"));
        assert!(block.contains("clarify_only_after"));
    }
}
