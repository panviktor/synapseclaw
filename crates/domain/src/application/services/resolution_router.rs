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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResolutionConfidence {
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClarificationReason {
    ResolverExhausted,
    LowConfidence,
    AmbiguousCandidates,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolutionPlan {
    pub source_order: Vec<ResolutionSource>,
    pub confidence: ResolutionConfidence,
    pub clarify_after_exhaustion: bool,
    pub clarification_reason: Option<ClarificationReason>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResolutionEvidence<'a> {
    pub interpretation: Option<&'a TurnInterpretation>,
    pub top_session_score: Option<f64>,
    pub second_session_score: Option<f64>,
    pub top_recipe_score: Option<i64>,
    pub second_recipe_score: Option<i64>,
    pub top_memory_score: Option<f64>,
    pub second_memory_score: Option<f64>,
    pub recall_hits: usize,
    pub skill_hits: usize,
    pub entity_hits: usize,
}

const SESSION_PRIMARY_SCORE: f64 = 1.9;
const RECIPE_PRIMARY_SCORE: i64 = 180;
const SESSION_CONFIDENT_GAP: f64 = 0.25;
const RECIPE_CONFIDENT_GAP: i64 = 25;
const MEMORY_CONFIDENT_SCORE: f64 = 0.78;
const MEMORY_CONFIDENT_GAP: f64 = 0.08;

pub fn build_resolution_plan(evidence: ResolutionEvidence<'_>) -> ResolutionPlan {
    let mut source_order = Vec::new();
    let (prefer_defaults, prefer_followup) = if let Some(interpretation) = evidence.interpretation {
        (
            !interpretation.defaults_requested.is_empty(),
            !interpretation.reference_candidates.is_empty(),
        )
    } else {
        (false, false)
    };

    if let Some(interpretation) = evidence.interpretation {
        if interpretation.dialogue_state.is_some() {
            source_order.push(ResolutionSource::DialogueState);
        }
        if interpretation.user_profile.is_some() {
            source_order.push(ResolutionSource::UserProfile);
        }
        if interpretation.current_conversation.is_some() {
            source_order.push(ResolutionSource::CurrentConversation);
        }
        if prefer_followup && interpretation.dialogue_state.is_some() {
            source_order.retain(|source| *source != ResolutionSource::DialogueState);
            source_order.insert(0, ResolutionSource::DialogueState);
        }
        if prefer_defaults && interpretation.user_profile.is_some() {
            source_order.retain(|source| *source != ResolutionSource::UserProfile);
            source_order.insert(0, ResolutionSource::UserProfile);
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

    let confidence = compute_confidence(source_order.first().copied(), evidence);
    let clarification_reason =
        compute_clarification_reason(source_order.first().copied(), confidence, evidence);

    ResolutionPlan {
        source_order,
        confidence,
        clarify_after_exhaustion: true,
        clarification_reason,
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
    lines.push(format!(
        "- confidence: {}",
        match plan.confidence {
            ResolutionConfidence::High => "high",
            ResolutionConfidence::Medium => "medium",
            ResolutionConfidence::Low => "low",
        }
    ));
    if plan.clarify_after_exhaustion {
        lines.push("- clarify_only_after: source_exhaustion_or_low_confidence".to_string());
    }
    if let Some(reason) = plan.clarification_reason {
        lines.push(format!(
            "- clarification_reason: {}",
            clarification_reason_name(reason)
        ));
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

fn clarification_reason_name(reason: ClarificationReason) -> &'static str {
    match reason {
        ClarificationReason::ResolverExhausted => "resolver_exhausted",
        ClarificationReason::LowConfidence => "low_confidence",
        ClarificationReason::AmbiguousCandidates => "ambiguous_candidates",
    }
}

fn compute_confidence(
    primary: Option<ResolutionSource>,
    evidence: ResolutionEvidence<'_>,
) -> ResolutionConfidence {
    let Some(primary) = primary else {
        return ResolutionConfidence::Low;
    };

    match primary {
        ResolutionSource::CurrentConversation | ResolutionSource::UserProfile => {
            ResolutionConfidence::High
        }
        ResolutionSource::DialogueState => {
            if evidence
                .interpretation
                .is_some_and(|interpretation| !interpretation.reference_candidates.is_empty())
            {
                ResolutionConfidence::High
            } else {
                ResolutionConfidence::Medium
            }
        }
        ResolutionSource::SessionHistory => {
            let score = evidence.top_session_score.unwrap_or_default();
            let gap = score_gap_f64(score, evidence.second_session_score);
            match score {
                score if score >= SESSION_PRIMARY_SCORE && gap >= SESSION_CONFIDENT_GAP => {
                    ResolutionConfidence::High
                }
                score if score > 0.0 => ResolutionConfidence::Medium,
                _ => ResolutionConfidence::Low,
            }
        }
        ResolutionSource::RunRecipe => match evidence.top_recipe_score.unwrap_or_default() {
            score
                if score >= RECIPE_PRIMARY_SCORE
                    && score_gap_i64(score, evidence.second_recipe_score)
                        >= RECIPE_CONFIDENT_GAP =>
            {
                ResolutionConfidence::High
            }
            score if score > 0 => ResolutionConfidence::Medium,
            _ => ResolutionConfidence::Low,
        },
        ResolutionSource::LongTermMemory => {
            let total = evidence.recall_hits + evidence.skill_hits + evidence.entity_hits;
            let top_score = evidence.top_memory_score.unwrap_or_default();
            let gap = score_gap_f64(top_score, evidence.second_memory_score);
            if top_score >= MEMORY_CONFIDENT_SCORE && gap >= MEMORY_CONFIDENT_GAP {
                ResolutionConfidence::High
            } else if total >= 3 || (evidence.skill_hits > 0 && evidence.entity_hits > 0) {
                ResolutionConfidence::Medium
            } else {
                ResolutionConfidence::Low
            }
        }
    }
}

fn compute_clarification_reason(
    primary: Option<ResolutionSource>,
    confidence: ResolutionConfidence,
    evidence: ResolutionEvidence<'_>,
) -> Option<ClarificationReason> {
    let interpretation = evidence.interpretation?;

    if interpretation.clarification_candidates.len() > 1
        && matches!(primary, Some(ResolutionSource::DialogueState))
    {
        return Some(if confidence == ResolutionConfidence::High {
            ClarificationReason::LowConfidence
        } else {
            ClarificationReason::AmbiguousCandidates
        });
    }

    if primary.is_none() {
        return Some(ClarificationReason::ResolverExhausted);
    }

    if confidence == ResolutionConfidence::Low {
        return Some(ClarificationReason::LowConfidence);
    }

    None
}

fn score_gap_f64(top: f64, second: Option<f64>) -> f64 {
    (top - second.unwrap_or_default()).max(0.0)
}

fn score_gap_i64(top: i64, second: Option<i64>) -> i64 {
    (top - second.unwrap_or_default()).max(0)
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
                reference_anchors: vec![],
                last_tool_subjects: vec![],
            }),
            defaults_requested: vec![],
            temporal_scope: None,
            delivery_scope: None,
            reference_candidates: vec![],
            clarification_candidates: vec![],
        };

        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: Some(&interpretation),
            top_session_score: Some(2.2),
            second_session_score: Some(1.6),
            top_recipe_score: Some(245),
            second_recipe_score: Some(180),
            top_memory_score: Some(0.92),
            second_memory_score: Some(0.72),
            recall_hits: 1,
            skill_hits: 0,
            entity_hits: 0,
        });

        assert_eq!(
            plan.source_order,
            vec![
                ResolutionSource::DialogueState,
                ResolutionSource::UserProfile,
                ResolutionSource::CurrentConversation,
                ResolutionSource::RunRecipe,
                ResolutionSource::SessionHistory,
                ResolutionSource::LongTermMemory,
            ]
        );
        assert_eq!(plan.confidence, ResolutionConfidence::Medium);
        assert!(plan.clarification_reason.is_none());
    }

    #[test]
    fn formats_resolution_block() {
        let plan = ResolutionPlan {
            source_order: vec![
                ResolutionSource::DialogueState,
                ResolutionSource::LongTermMemory,
            ],
            confidence: ResolutionConfidence::Medium,
            clarify_after_exhaustion: true,
            clarification_reason: Some(ClarificationReason::LowConfidence),
        };
        let block = format_resolution_plan(&plan).unwrap();
        assert!(block.contains("[resolution-plan]"));
        assert!(block.contains("dialogue_state -> long_term_memory"));
        assert!(block.contains("confidence: medium"));
        assert!(block.contains("clarify_only_after"));
        assert!(block.contains("clarification_reason: low_confidence"));
    }

    #[test]
    fn low_gap_history_only_gets_medium_confidence() {
        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: None,
            top_session_score: Some(2.1),
            second_session_score: Some(1.98),
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: None,
            second_memory_score: None,
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 0,
        });

        assert_eq!(plan.source_order, vec![ResolutionSource::SessionHistory]);
        assert_eq!(plan.confidence, ResolutionConfidence::Medium);
    }
}
