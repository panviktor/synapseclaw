//! Resolution router — typed, score-based source ranking for a turn.
//!
//! This is intentionally not a phrase-engine. It consumes typed interpretation
//! and retrieval evidence, then decides which sources should be trusted first.

use crate::application::services::turn_interpretation::{
    ReferenceCandidateKind, ReferenceSource, TurnInterpretation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    ConfiguredRuntime,
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

const MIN_INCLUDED_SCORE: f64 = 0.35;
const HIGH_CONFIDENCE_SCORE: f64 = 0.86;
const HIGH_CONFIDENCE_GAP: f64 = 0.16;
const MEDIUM_CONFIDENCE_SCORE: f64 = 0.68;

#[derive(Debug, Clone, Copy, PartialEq)]
struct RankedSource {
    source: ResolutionSource,
    score: f64,
}

pub fn build_resolution_plan(evidence: ResolutionEvidence<'_>) -> ResolutionPlan {
    let ranked = rank_sources(evidence);
    let source_order = ranked
        .iter()
        .map(|ranked| ranked.source)
        .collect::<Vec<_>>();
    let confidence = compute_confidence(&ranked);
    let clarification_reason = compute_clarification_reason(&ranked, confidence, evidence);

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

pub fn source_priority(plan: Option<&ResolutionPlan>, source: ResolutionSource) -> usize {
    plan.and_then(|plan| plan.source_order.iter().position(|item| *item == source))
        .unwrap_or(usize::MAX)
}

fn source_name(source: ResolutionSource) -> &'static str {
    match source {
        ResolutionSource::ConfiguredRuntime => "configured_runtime",
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

fn rank_sources(evidence: ResolutionEvidence<'_>) -> Vec<RankedSource> {
    let mut ranked = Vec::new();

    push_ranked(
        &mut ranked,
        ResolutionSource::ConfiguredRuntime,
        score_configured_runtime(evidence.interpretation),
    );
    push_ranked(
        &mut ranked,
        ResolutionSource::DialogueState,
        score_dialogue_state(evidence.interpretation),
    );
    push_ranked(
        &mut ranked,
        ResolutionSource::UserProfile,
        score_user_profile(evidence.interpretation),
    );
    push_ranked(
        &mut ranked,
        ResolutionSource::CurrentConversation,
        score_current_conversation(evidence.interpretation),
    );
    push_ranked(
        &mut ranked,
        ResolutionSource::SessionHistory,
        score_session_history(evidence),
    );
    push_ranked(
        &mut ranked,
        ResolutionSource::RunRecipe,
        score_run_recipe(evidence),
    );
    push_ranked(
        &mut ranked,
        ResolutionSource::LongTermMemory,
        score_long_term_memory(evidence),
    );

    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| source_tie_breaker(left.source).cmp(&source_tie_breaker(right.source)))
    });

    ranked
}

fn push_ranked(ranked: &mut Vec<RankedSource>, source: ResolutionSource, score: Option<f64>) {
    let Some(score) = score else {
        return;
    };
    if score < MIN_INCLUDED_SCORE {
        return;
    }
    ranked.push(RankedSource { source, score });
}

fn source_tie_breaker(source: ResolutionSource) -> usize {
    match source {
        ResolutionSource::ConfiguredRuntime => 0,
        ResolutionSource::DialogueState => 1,
        ResolutionSource::UserProfile => 2,
        ResolutionSource::CurrentConversation => 3,
        ResolutionSource::RunRecipe => 4,
        ResolutionSource::SessionHistory => 5,
        ResolutionSource::LongTermMemory => 6,
    }
}

fn score_configured_runtime(interpretation: Option<&TurnInterpretation>) -> Option<f64> {
    let interpretation = interpretation?;
    let target = interpretation.configured_delivery_target.as_ref()?;
    let reference_count =
        count_reference_candidates(interpretation, ReferenceSource::ConfiguredRuntime) as f64;
    let explicit_bonus = if matches!(
        target,
        crate::domain::conversation_target::ConversationDeliveryTarget::Explicit { .. }
    ) {
        0.08
    } else {
        0.0
    };

    Some((0.74 + reference_count.min(2.0) * 0.06 + explicit_bonus).min(0.92))
}

fn score_dialogue_state(interpretation: Option<&TurnInterpretation>) -> Option<f64> {
    let interpretation = interpretation?;
    let state = interpretation.dialogue_state.as_ref()?;
    let reference_count =
        count_reference_candidates(interpretation, ReferenceSource::DialogueState) as f64;
    let direct_reference_count = count_direct_dialogue_state_references(interpretation) as f64;
    let anchor_count = state.reference_anchors.len().min(4) as f64;
    let focus_count = state.focus_entities.len().min(3) as f64;
    let comparison_bonus = if state.comparison_set.len() >= 2 {
        0.12
    } else {
        0.0
    };
    let subject_bonus = if state.last_tool_subjects.is_empty() {
        0.0
    } else {
        0.05
    };
    let delivery_bonus = if state.recent_delivery_target.is_some() {
        0.04
    } else {
        0.0
    };
    let schedule_bonus = if state.recent_schedule_job.is_some() {
        0.04
    } else {
        0.0
    };
    let resource_bonus = if state.recent_resource.is_some() {
        0.03
    } else {
        0.0
    };
    let search_bonus = if state.recent_search.is_some() {
        0.03
    } else {
        0.0
    };
    let workspace_bonus = if state.recent_workspace.is_some() {
        0.03
    } else {
        0.0
    };

    Some(
        (0.42
            + reference_count.min(4.0) * 0.08
            + direct_reference_count.min(3.0) * 0.05
            + anchor_count * 0.05
            + focus_count * 0.04
            + comparison_bonus
            + subject_bonus
            + delivery_bonus
            + schedule_bonus
            + resource_bonus
            + search_bonus
            + workspace_bonus)
            .min(0.96),
    )
}

fn score_user_profile(interpretation: Option<&TurnInterpretation>) -> Option<f64> {
    let interpretation = interpretation?;
    let profile = interpretation.user_profile.as_ref()?;
    let reference_count = count_reference_candidates(interpretation, ReferenceSource::UserProfile);
    let dialogue_reference_count =
        count_reference_candidates(interpretation, ReferenceSource::DialogueState);
    let current_conversation_reference_count =
        count_reference_candidates(interpretation, ReferenceSource::CurrentConversation);
    let direct_dialogue_reference_count = count_direct_dialogue_state_references(interpretation);
    let field_count = profile.fact_count();

    let competing_context_penalty = if direct_dialogue_reference_count > 0 {
        0.16
    } else if dialogue_reference_count > 0 || current_conversation_reference_count > 0 {
        0.08
    } else {
        0.0
    };

    Some(
        (0.66 + (reference_count.min(4) as f64) * 0.05 + (field_count.min(4) as f64) * 0.02
            - competing_context_penalty)
            .clamp(0.0, 0.92),
    )
}

fn score_current_conversation(interpretation: Option<&TurnInterpretation>) -> Option<f64> {
    let interpretation = interpretation?;
    let conversation = interpretation.current_conversation.as_ref()?;
    let reference_count =
        count_reference_candidates(interpretation, ReferenceSource::CurrentConversation);
    Some(
        (0.62_f64
            + if conversation.has_thread {
                0.04_f64
            } else {
                0.0_f64
            }
            + if reference_count > 0 {
                0.08_f64
            } else {
                0.0_f64
            })
        .min(0.82_f64),
    )
}

fn score_session_history(evidence: ResolutionEvidence<'_>) -> Option<f64> {
    let top = evidence.top_session_score?;
    if top <= 0.0 {
        return None;
    }
    let gap = score_gap_f64(top, evidence.second_session_score);
    Some(((top / 3.0).min(0.82) + (gap / 0.8).min(0.14)).min(0.96))
}

fn score_run_recipe(evidence: ResolutionEvidence<'_>) -> Option<f64> {
    let top = evidence.top_recipe_score?;
    if top <= 0 {
        return None;
    }
    let gap = score_gap_i64(top, evidence.second_recipe_score) as f64;
    Some(
        (((top as f64) / 260.0).min(0.82)
            + (gap / 80.0).min(0.14)
            + if top >= 200 { 0.03 } else { 0.0 })
        .min(0.96),
    )
}

fn score_long_term_memory(evidence: ResolutionEvidence<'_>) -> Option<f64> {
    let total_hits = evidence.recall_hits + evidence.skill_hits + evidence.entity_hits;
    if total_hits == 0 {
        return None;
    }

    let top = evidence
        .top_memory_score
        .unwrap_or_default()
        .clamp(0.0, 1.0);
    let gap = score_gap_f64(top, evidence.second_memory_score);
    let density_bonus = ((total_hits.min(5) as f64) / 5.0) * 0.14;
    let structured_bonus = match (evidence.skill_hits > 0, evidence.entity_hits > 0) {
        (true, true) => 0.06,
        (true, false) | (false, true) => 0.03,
        (false, false) => 0.0,
    };

    Some((0.24 + top * 0.42 + density_bonus + structured_bonus + (gap / 0.2).min(0.10)).min(0.94))
}

fn count_reference_candidates(
    interpretation: &TurnInterpretation,
    source: ReferenceSource,
) -> usize {
    interpretation
        .reference_candidates
        .iter()
        .filter(|candidate| candidate.source == source)
        .count()
}

fn count_direct_dialogue_state_references(interpretation: &TurnInterpretation) -> usize {
    interpretation
        .reference_candidates
        .iter()
        .filter(|candidate| {
            candidate.source == ReferenceSource::DialogueState
                && matches!(
                    candidate.kind,
                    ReferenceCandidateKind::DeliveryTarget
                        | ReferenceCandidateKind::ScheduleJob
                        | ReferenceCandidateKind::ResourceLocator { .. }
                        | ReferenceCandidateKind::SearchQuery { .. }
                        | ReferenceCandidateKind::SearchResult { .. }
                        | ReferenceCandidateKind::WorkspaceName { .. }
                )
        })
        .count()
}

fn compute_confidence(ranked: &[RankedSource]) -> ResolutionConfidence {
    let Some(primary) = ranked.first() else {
        return ResolutionConfidence::Low;
    };

    let gap = score_gap_f64(primary.score, ranked.get(1).map(|item| item.score));

    if primary.score >= HIGH_CONFIDENCE_SCORE && gap >= HIGH_CONFIDENCE_GAP {
        ResolutionConfidence::High
    } else if primary.score >= MEDIUM_CONFIDENCE_SCORE {
        ResolutionConfidence::Medium
    } else {
        ResolutionConfidence::Low
    }
}

fn compute_clarification_reason(
    ranked: &[RankedSource],
    confidence: ResolutionConfidence,
    evidence: ResolutionEvidence<'_>,
) -> Option<ClarificationReason> {
    let interpretation = evidence.interpretation?;
    let primary = ranked.first().map(|item| item.source);
    let direct_dialogue_refs = count_direct_dialogue_state_references(interpretation);
    let ambiguity_gap = score_gap_f64(
        ranked.first().map(|item| item.score).unwrap_or_default(),
        ranked.get(1).map(|item| item.score),
    );

    if interpretation.clarification_candidates.len() > 1
        && (matches!(primary, Some(ResolutionSource::DialogueState)) || ambiguity_gap < 0.16)
    {
        return Some(ClarificationReason::AmbiguousCandidates);
    }

    if ranked.is_empty() {
        return Some(ClarificationReason::ResolverExhausted);
    }

    if direct_dialogue_refs > 0
        && interpretation.clarification_candidates.is_empty()
        && matches!(primary, Some(ResolutionSource::DialogueState))
    {
        return None;
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
        CurrentConversationSnapshot, DialogueStateSnapshot, ReferenceCandidate,
        ReferenceCandidateKind, TurnInterpretation,
    };
    use crate::domain::user_profile::{UserProfile, DELIVERY_TARGET_PREFERENCE_KEY};

    #[test]
    fn ranks_sources_from_typed_evidence_without_fixed_order_branches() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[(
                "response_locale",
                serde_json::json!("ru"),
            )])),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: true,
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![("city".into(), "Berlin".into())],
                comparison_set: vec![],
                reference_anchors: vec![],
                last_tool_subjects: vec![],
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            reference_candidates: vec![],
            clarification_candidates: vec![],
            configured_delivery_target: None,
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
            plan.source_order.first(),
            Some(&ResolutionSource::RunRecipe)
        );
        assert!(plan.source_order.contains(&ResolutionSource::DialogueState));
        assert!(plan.source_order.contains(&ResolutionSource::UserProfile));
        assert!(plan
            .source_order
            .contains(&ResolutionSource::CurrentConversation));
        assert!(plan
            .source_order
            .contains(&ResolutionSource::SessionHistory));
        assert!(plan
            .source_order
            .contains(&ResolutionSource::LongTermMemory));
        assert!(
            source_priority(Some(&plan), ResolutionSource::SessionHistory)
                < source_priority(Some(&plan), ResolutionSource::LongTermMemory)
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

    #[test]
    fn direct_dialogue_references_suppress_generic_low_confidence_clarify() {
        let interpretation = TurnInterpretation {
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![],
                comparison_set: vec![],
                reference_anchors: vec![],
                last_tool_subjects: vec!["job_123".into()],
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            reference_candidates: vec![ReferenceCandidate {
                kind: ReferenceCandidateKind::ScheduleJob,
                value: "job_123".into(),
                source: ReferenceSource::DialogueState,
            }],
            clarification_candidates: vec![],
            ..Default::default()
        };

        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: Some(&interpretation),
            top_session_score: None,
            second_session_score: None,
            top_recipe_score: None,
            second_recipe_score: None,
            top_memory_score: Some(0.41),
            second_memory_score: Some(0.40),
            recall_hits: 0,
            skill_hits: 0,
            entity_hits: 0,
        });

        assert_eq!(
            plan.source_order.first(),
            Some(&ResolutionSource::DialogueState)
        );
        assert_ne!(
            plan.clarification_reason,
            Some(ClarificationReason::LowConfidence)
        );
    }

    #[test]
    fn current_conversation_outranks_profile_when_both_are_present() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[(
                "workspace_anchor",
                serde_json::json!("Borealis"),
            )])),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: false,
            }),
            reference_candidates: vec![
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::Profile {
                        key: "workspace_anchor".into(),
                    },
                    value: "Borealis".into(),
                    source: ReferenceSource::UserProfile,
                },
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::DeliveryTarget,
                    value: "current_conversation".into(),
                    source: ReferenceSource::CurrentConversation,
                },
            ],
            ..Default::default()
        };

        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: Some(&interpretation),
            ..Default::default()
        });

        assert_eq!(
            plan.source_order.first(),
            Some(&ResolutionSource::CurrentConversation)
        );
    }

    #[test]
    fn direct_dialogue_state_outranks_profile_facts() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[(
                "workspace_anchor",
                serde_json::json!("Borealis"),
            )])),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![],
                comparison_set: vec![],
                reference_anchors: vec![],
                last_tool_subjects: vec![],
                recent_delivery_target: None,
                recent_schedule_job: Some(crate::domain::dialogue_state::ScheduleJobReference {
                    job_id: "job_123".into(),
                    action: crate::domain::tool_fact::ScheduleAction::Run,
                    job_type: Some(crate::domain::tool_fact::ScheduleJobType::Agent),
                    schedule_kind: Some(crate::domain::tool_fact::ScheduleKind::Cron),
                    session_target: None,
                    timezone: None,
                }),
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            reference_candidates: vec![
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::Profile {
                        key: "workspace_anchor".into(),
                    },
                    value: "Borealis".into(),
                    source: ReferenceSource::UserProfile,
                },
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::ScheduleJob,
                    value: "job_123".into(),
                    source: ReferenceSource::DialogueState,
                },
            ],
            ..Default::default()
        };

        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: Some(&interpretation),
            ..Default::default()
        });

        assert_eq!(
            plan.source_order.first(),
            Some(&ResolutionSource::DialogueState)
        );
    }

    #[test]
    fn configured_runtime_target_outranks_profile_and_conversation_defaults() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[(
                DELIVERY_TARGET_PREFERENCE_KEY,
                serde_json::to_value(
                    crate::domain::conversation_target::ConversationDeliveryTarget::Explicit {
                        channel: "slack".into(),
                        recipient: "C123".into(),
                        thread_ref: None,
                    },
                )
                .unwrap(),
            )])),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: false,
            }),
            configured_delivery_target: Some(
                crate::domain::conversation_target::ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!ops:example.org".into(),
                    thread_ref: None,
                },
            ),
            reference_candidates: vec![
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::Profile {
                        key: DELIVERY_TARGET_PREFERENCE_KEY.into(),
                    },
                    value: "explicit:slack:C123".into(),
                    source: ReferenceSource::UserProfile,
                },
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::DeliveryTarget,
                    value: "explicit:matrix:!ops:example.org".into(),
                    source: ReferenceSource::ConfiguredRuntime,
                },
                ReferenceCandidate {
                    kind: ReferenceCandidateKind::DeliveryTarget,
                    value: "current_conversation".into(),
                    source: ReferenceSource::CurrentConversation,
                },
            ],
            ..Default::default()
        };

        let plan = build_resolution_plan(ResolutionEvidence {
            interpretation: Some(&interpretation),
            ..Default::default()
        });

        assert_eq!(
            plan.source_order.first(),
            Some(&ResolutionSource::ConfiguredRuntime)
        );
    }

    fn profile_with_facts(facts: &[(&str, serde_json::Value)]) -> UserProfile {
        let mut profile = UserProfile::default();
        for (key, value) in facts {
            profile.set(*key, value.clone());
        }
        profile
    }
}
