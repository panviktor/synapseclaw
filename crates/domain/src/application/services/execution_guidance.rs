//! Execution guidance — typed runtime policy for direct-resolution turns.
//!
//! This is not a phrase engine. It turns structured interpretation and
//! resolution output into narrow execution policy so the runtime can avoid
//! unnecessary archaeology when a direct structured runtime fact is already enough.

use crate::application::services::resolution_router::{ResolutionPlan, ResolutionSource};
use crate::application::services::turn_interpretation::{
    ReferenceCandidateKind, ReferenceSource, TurnInterpretation,
};
use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairTrace};
use crate::domain::turn_admission::{
    admission_repair_hint_label, candidate_admission_reason_label, AdmissionRepairHint,
    CandidateAdmissionReason,
};

const MAX_EXECUTION_FAILURE_HINTS: usize = 3;
const MAX_EXECUTION_ADMISSION_REASONS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionCapability {
    ConversationReply,
    Delivery,
    ProfileFacts,
    DirectReferenceAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFailureHint {
    pub tool_name: String,
    pub failure_kind: ToolFailureKind,
    pub suggested_action: ToolRepairAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAdmissionHint {
    pub reasons: Vec<CandidateAdmissionReason>,
    pub recommended_action: Option<AdmissionRepairHint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionGuidance {
    pub resolved_from: Option<ResolutionSource>,
    pub direct_resolution_ready: bool,
    pub preferred_capabilities: Vec<ExecutionCapability>,
    pub recent_failure_hints: Vec<ExecutionFailureHint>,
    pub recent_admission_hint: Option<ExecutionAdmissionHint>,
    pub prefer_answer_from_resolved_state: bool,
    pub avoid_session_history_lookup: bool,
    pub avoid_run_recipe_lookup: bool,
    pub avoid_workspace_discovery: bool,
    pub avoid_bootstrap_doc_reads: bool,
}

pub fn build_execution_guidance(
    plan: Option<&ResolutionPlan>,
    interpretation: Option<&TurnInterpretation>,
    recent_tool_repairs: &[ToolRepairTrace],
    recent_admission_reasons: &[CandidateAdmissionReason],
    recent_admission_repair: Option<AdmissionRepairHint>,
) -> Option<ExecutionGuidance> {
    let interpretation = interpretation?;
    let resolved_from = plan.and_then(|plan| plan.source_order.first().copied());

    let current_conversation_ready =
        matches!(resolved_from, Some(ResolutionSource::CurrentConversation))
            && interpretation.current_conversation.is_some();

    let delivery_ready = match resolved_from {
        Some(ResolutionSource::DialogueState) => {
            interpretation.reference_candidates.iter().any(|candidate| {
                candidate.source == ReferenceSource::DialogueState
                    && matches!(candidate.kind, ReferenceCandidateKind::DeliveryTarget)
            })
        }
        Some(ResolutionSource::UserProfile) => {
            interpretation.reference_candidates.iter().any(|candidate| {
                candidate.source == ReferenceSource::UserProfile
                    && matches!(
                        &candidate.kind,
                        ReferenceCandidateKind::Profile { key } if key == "delivery_target_preference"
                    )
            })
        }
        _ => false,
    };

    let profile_facts_ready = matches!(resolved_from, Some(ResolutionSource::UserProfile))
        && interpretation.reference_candidates.iter().any(|candidate| {
            candidate.source == ReferenceSource::UserProfile
                && matches!(candidate.kind, ReferenceCandidateKind::Profile { .. })
        });

    let direct_reference_ready = matches!(resolved_from, Some(ResolutionSource::DialogueState))
        && interpretation.reference_candidates.iter().any(|candidate| {
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
        });

    let direct_resolution_ready = matches!(
        resolved_from,
        Some(
            ResolutionSource::ConfiguredRuntime
                | ResolutionSource::DialogueState
                | ResolutionSource::UserProfile
                | ResolutionSource::CurrentConversation
        )
    ) && plan.is_none_or(|plan| plan.clarification_reason.is_none());

    let mut preferred_capabilities = Vec::new();
    push_capability(
        &mut preferred_capabilities,
        current_conversation_ready,
        ExecutionCapability::ConversationReply,
    );
    push_capability(
        &mut preferred_capabilities,
        delivery_ready,
        ExecutionCapability::Delivery,
    );
    push_capability(
        &mut preferred_capabilities,
        profile_facts_ready,
        ExecutionCapability::ProfileFacts,
    );
    push_capability(
        &mut preferred_capabilities,
        direct_reference_ready,
        ExecutionCapability::DirectReferenceAction,
    );
    let recent_failure_hints = recent_failure_hints(recent_tool_repairs);
    let recent_admission_hint =
        recent_admission_hint(recent_admission_reasons, recent_admission_repair);

    let prefer_answer_from_resolved_state = direct_resolution_ready
        && (preferred_capabilities.is_empty()
            || preferred_capabilities == vec![ExecutionCapability::ConversationReply])
        && matches!(
            resolved_from,
            Some(ResolutionSource::DialogueState | ResolutionSource::UserProfile)
        );

    let avoid_historical_lookup = prefer_answer_from_resolved_state
        || (direct_resolution_ready
            && (delivery_ready || profile_facts_ready || direct_reference_ready));

    let guidance = ExecutionGuidance {
        resolved_from,
        direct_resolution_ready,
        preferred_capabilities,
        recent_failure_hints,
        recent_admission_hint,
        prefer_answer_from_resolved_state,
        avoid_session_history_lookup: avoid_historical_lookup,
        avoid_run_recipe_lookup: avoid_historical_lookup,
        avoid_workspace_discovery: avoid_historical_lookup,
        avoid_bootstrap_doc_reads: avoid_historical_lookup,
    };

    if guidance.preferred_capabilities.is_empty() && !guidance.direct_resolution_ready {
        None
    } else {
        Some(guidance)
    }
}

pub fn format_execution_guidance(guidance: &ExecutionGuidance) -> Option<String> {
    if guidance.preferred_capabilities.is_empty()
        && !guidance.direct_resolution_ready
        && guidance.resolved_from.is_none()
    {
        return None;
    }

    let mut lines = vec!["[execution-guidance]".to_string()];

    if let Some(source) = guidance.resolved_from {
        lines.push(format!(
            "- resolved_from: {}",
            resolution_source_name(source)
        ));
    }
    if guidance.direct_resolution_ready {
        lines.push("- direct_resolution_ready: true".to_string());
    }
    if !guidance.preferred_capabilities.is_empty() {
        lines.push(format!(
            "- preferred_capabilities: {}",
            guidance
                .preferred_capabilities
                .iter()
                .map(|capability| capability_name(*capability))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !guidance.recent_failure_hints.is_empty() {
        lines.push("- avoid_immediate_retry_of_recent_failures: true".to_string());
        lines.push(format!(
            "- recent_failure_hints: {}",
            guidance
                .recent_failure_hints
                .iter()
                .map(format_failure_hint)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if let Some(admission_hint) = guidance.recent_admission_hint.as_ref() {
        lines.push("- respect_recent_route_admission_hint: true".to_string());
        if !admission_hint.reasons.is_empty() {
            lines.push(format!(
                "- recent_admission_reasons: {}",
                admission_hint
                    .reasons
                    .iter()
                    .map(candidate_admission_reason_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(repair) = admission_hint.recommended_action {
            lines.push(format!(
                "- recent_admission_recommended_action: {}",
                admission_repair_hint_label(repair)
            ));
        }
    }
    if guidance.prefer_answer_from_resolved_state {
        lines.push("- prefer_answer_from_resolved_state: true".to_string());
        lines.push(
            "- mutate_state_only_when_user_explicitly_requests_memory_or_profile_change: true"
                .to_string(),
        );
    }
    if guidance.direct_resolution_ready
        && guidance
            .preferred_capabilities
            .contains(&ExecutionCapability::Delivery)
    {
        lines.push("- prefer_local_delivery_tool: message_send".to_string());
        lines
            .push("- message_send_requires_content_only_when_target_is_resolved: true".to_string());
        lines.push("- avoid_delegate_delivery_when_target_is_resolved: true".to_string());
    }
    if guidance.direct_resolution_ready
        && guidance
            .preferred_capabilities
            .contains(&ExecutionCapability::ProfileFacts)
    {
        lines.push("- apply_profile_facts_before_lookup: true".to_string());
    }
    if guidance.avoid_session_history_lookup {
        lines.push("- avoid_session_history_lookup: true".to_string());
    }
    if guidance.avoid_run_recipe_lookup {
        lines.push("- avoid_run_recipe_lookup: true".to_string());
    }
    if guidance.avoid_workspace_discovery {
        lines.push("- avoid_workspace_discovery: true".to_string());
    }
    if guidance.avoid_bootstrap_doc_reads {
        lines.push("- avoid_bootstrap_doc_reads: true".to_string());
    }

    Some(format!("{}\n", lines.join("\n")))
}

fn push_capability(
    capabilities: &mut Vec<ExecutionCapability>,
    enabled: bool,
    capability: ExecutionCapability,
) {
    if enabled && !capabilities.contains(&capability) {
        capabilities.push(capability);
    }
}

fn resolution_source_name(source: ResolutionSource) -> &'static str {
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

fn capability_name(capability: ExecutionCapability) -> &'static str {
    match capability {
        ExecutionCapability::ConversationReply => "conversation_reply",
        ExecutionCapability::Delivery => "delivery",
        ExecutionCapability::ProfileFacts => "profile_facts",
        ExecutionCapability::DirectReferenceAction => "direct_reference_action",
    }
}

fn recent_failure_hints(repairs: &[ToolRepairTrace]) -> Vec<ExecutionFailureHint> {
    let mut hints = Vec::new();
    for repair in repairs.iter().rev() {
        if matches!(repair.failure_kind, ToolFailureKind::RuntimeError) {
            continue;
        }
        let hint = ExecutionFailureHint {
            tool_name: repair.tool_name.clone(),
            failure_kind: repair.failure_kind,
            suggested_action: repair.suggested_action,
        };
        if !hints.iter().any(|existing: &ExecutionFailureHint| {
            existing.tool_name == hint.tool_name
                && existing.failure_kind == hint.failure_kind
                && existing.suggested_action == hint.suggested_action
        }) {
            hints.push(hint);
        }
        if hints.len() >= MAX_EXECUTION_FAILURE_HINTS {
            break;
        }
    }
    hints
}

fn recent_admission_hint(
    reasons: &[CandidateAdmissionReason],
    recommended_action: Option<AdmissionRepairHint>,
) -> Option<ExecutionAdmissionHint> {
    let reasons = bounded_admission_reasons(reasons);

    if reasons.is_empty() && recommended_action.is_none() {
        None
    } else {
        Some(ExecutionAdmissionHint {
            reasons,
            recommended_action,
        })
    }
}

fn bounded_admission_reasons(
    reasons: &[CandidateAdmissionReason],
) -> Vec<CandidateAdmissionReason> {
    let mut bounded = Vec::new();
    for reason in reasons {
        if !bounded.contains(reason) {
            bounded.push(reason.clone());
        }
        if bounded.len() >= MAX_EXECUTION_ADMISSION_REASONS {
            break;
        }
    }
    bounded
}

fn format_failure_hint(hint: &ExecutionFailureHint) -> String {
    format!(
        "{}:{}->{}",
        hint.tool_name,
        failure_kind_name(hint.failure_kind),
        repair_action_name(hint.suggested_action)
    )
}

fn failure_kind_name(kind: ToolFailureKind) -> &'static str {
    match kind {
        ToolFailureKind::UnknownTool => "unknown_tool",
        ToolFailureKind::PolicyBlocked => "policy_blocked",
        ToolFailureKind::DuplicateInvocation => "duplicate_invocation",
        ToolFailureKind::AuthFailure => "auth_failure",
        ToolFailureKind::CapabilityMismatch => "capability_mismatch",
        ToolFailureKind::MissingResource => "missing_resource",
        ToolFailureKind::Timeout => "timeout",
        ToolFailureKind::SchemaMismatch => "schema_mismatch",
        ToolFailureKind::ContextLimitExceeded => "context_limit_exceeded",
        ToolFailureKind::RuntimeError => "runtime_error",
        ToolFailureKind::ReportedFailure => "reported_failure",
    }
}

fn repair_action_name(action: ToolRepairAction) -> &'static str {
    match action {
        ToolRepairAction::UseKnownTool => "use_known_tool",
        ToolRepairAction::RequestPermissionOrApproval => "request_permission_or_approval",
        ToolRepairAction::AvoidDuplicateRetry => "avoid_duplicate_retry",
        ToolRepairAction::AuthenticateOrConfigureCredentials => {
            "authenticate_or_configure_credentials"
        }
        ToolRepairAction::RetryWithSimplerRequest => "retry_with_simpler_request",
        ToolRepairAction::AdjustArgumentsOrTarget => "adjust_arguments_or_target",
        ToolRepairAction::CompactSessionOrStartFreshHandoff => {
            "compact_session_or_start_fresh_handoff"
        }
        ToolRepairAction::InspectRuntimeFailure => "inspect_runtime_failure",
        ToolRepairAction::SwitchRouteLane(_) => "switch_route_lane",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::resolution_router::{
        ClarificationReason, ResolutionConfidence,
    };
    use crate::application::services::turn_interpretation::{
        CurrentConversationSnapshot, DialogueStateSnapshot, TurnInterpretation,
    };
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn configured_runtime_target_is_available_but_does_not_force_delivery_narrowing() {
        let interpretation = TurnInterpretation {
            configured_delivery_target: Some(ConversationDeliveryTarget::Explicit {
                channel: "matrix".into(),
                recipient: "!ops:example.org".into(),
                thread_ref: None,
            }),
            ..TurnInterpretation::default()
        };
        let plan = ResolutionPlan {
            source_order: vec![ResolutionSource::ConfiguredRuntime],
            confidence: ResolutionConfidence::High,
            clarify_after_exhaustion: true,
            clarification_reason: None,
        };

        let guidance =
            build_execution_guidance(Some(&plan), Some(&interpretation), &[], &[], None).unwrap();
        assert!(guidance.direct_resolution_ready);
        assert!(guidance.preferred_capabilities.is_empty());
        assert!(!guidance.prefer_answer_from_resolved_state);
        assert!(!guidance.avoid_session_history_lookup);
        assert!(!guidance.avoid_workspace_discovery);
    }

    #[test]
    fn profile_fact_turn_avoids_historical_lookup_without_string_rules() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[
                ("weather_city", serde_json::json!("Tokyo")),
                ("local_timezone", serde_json::json!("Asia/Tokyo")),
                ("language_preference", serde_json::json!("ja")),
            ])),
            reference_candidates: vec![
                crate::application::services::turn_interpretation::ReferenceCandidate {
                    kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::Profile {
                        key: "weather_city".into(),
                    },
                    value: "Tokyo".into(),
                    source: ReferenceSource::UserProfile,
                },
                crate::application::services::turn_interpretation::ReferenceCandidate {
                    kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::Profile {
                        key: "local_timezone".into(),
                    },
                    value: "Asia/Tokyo".into(),
                    source: ReferenceSource::UserProfile,
                },
                crate::application::services::turn_interpretation::ReferenceCandidate {
                    kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::Profile {
                        key: "language_preference".into(),
                    },
                    value: "ja".into(),
                    source: ReferenceSource::UserProfile,
                },
            ],
            ..TurnInterpretation::default()
        };
        let plan = ResolutionPlan {
            source_order: vec![ResolutionSource::UserProfile],
            confidence: ResolutionConfidence::High,
            clarify_after_exhaustion: true,
            clarification_reason: None,
        };

        let guidance =
            build_execution_guidance(Some(&plan), Some(&interpretation), &[], &[], None).unwrap();
        assert!(guidance.direct_resolution_ready);
        assert!(guidance
            .preferred_capabilities
            .contains(&ExecutionCapability::ProfileFacts));
        assert!(guidance.avoid_run_recipe_lookup);
    }

    #[test]
    fn ambiguous_turn_does_not_claim_direct_resolution_ready() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[(
                "weather_city",
                serde_json::json!("Berlin"),
            )])),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: false,
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![],
                comparison_set: vec![],
                reference_anchors: vec![],
                last_tool_subjects: vec![],
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            ..TurnInterpretation::default()
        };
        let plan = ResolutionPlan {
            source_order: vec![ResolutionSource::UserProfile],
            confidence: ResolutionConfidence::Low,
            clarify_after_exhaustion: true,
            clarification_reason: Some(ClarificationReason::LowConfidence),
        };

        let guidance = build_execution_guidance(Some(&plan), Some(&interpretation), &[], &[], None);
        assert!(guidance.is_none());
    }

    #[test]
    fn current_conversation_does_not_trigger_profile_fact_narrowing_by_itself() {
        let interpretation = TurnInterpretation {
            user_profile: Some(profile_with_facts(&[
                ("weather_city", serde_json::json!("Tokyo")),
                ("local_timezone", serde_json::json!("Asia/Tokyo")),
                ("language_preference", serde_json::json!("ja")),
            ])),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "web".into(),
                has_thread: false,
            }),
            reference_candidates: vec![
                crate::application::services::turn_interpretation::ReferenceCandidate {
                    kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::Profile {
                        key: "weather_city".into(),
                    },
                    value: "Tokyo".into(),
                    source: ReferenceSource::UserProfile,
                },
                crate::application::services::turn_interpretation::ReferenceCandidate {
                    kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::Profile {
                        key: "local_timezone".into(),
                    },
                    value: "Asia/Tokyo".into(),
                    source: ReferenceSource::UserProfile,
                },
            ],
            ..TurnInterpretation::default()
        };
        let plan = ResolutionPlan {
            source_order: vec![
                ResolutionSource::CurrentConversation,
                ResolutionSource::UserProfile,
            ],
            confidence: ResolutionConfidence::High,
            clarify_after_exhaustion: true,
            clarification_reason: None,
        };

        let guidance =
            build_execution_guidance(Some(&plan), Some(&interpretation), &[], &[], None).unwrap();
        assert!(guidance.direct_resolution_ready);
        assert_eq!(
            guidance.preferred_capabilities,
            vec![ExecutionCapability::ConversationReply]
        );
        assert!(!guidance.prefer_answer_from_resolved_state);
        assert!(!guidance.avoid_session_history_lookup);
        assert!(!guidance.avoid_workspace_discovery);
    }

    #[test]
    fn dialogue_state_reply_prefers_answer_from_resolved_state() {
        let interpretation = TurnInterpretation {
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![("service".into(), "synapseclaw.service".into())],
                comparison_set: vec![],
                reference_anchors: vec![],
                last_tool_subjects: vec![],
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: None,
                recent_search: None,
                recent_workspace: None,
            }),
            ..TurnInterpretation::default()
        };
        let plan = ResolutionPlan {
            source_order: vec![ResolutionSource::DialogueState],
            confidence: ResolutionConfidence::High,
            clarify_after_exhaustion: true,
            clarification_reason: None,
        };

        let guidance =
            build_execution_guidance(Some(&plan), Some(&interpretation), &[], &[], None).unwrap();
        assert!(guidance.direct_resolution_ready);
        assert!(guidance.preferred_capabilities.is_empty());
        assert!(guidance.prefer_answer_from_resolved_state);
        assert!(guidance.avoid_session_history_lookup);
        assert!(guidance.avoid_workspace_discovery);
    }

    #[test]
    fn formats_guidance_block() {
        let block = format_execution_guidance(&ExecutionGuidance {
            resolved_from: Some(ResolutionSource::ConfiguredRuntime),
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            recent_admission_hint: None,
            prefer_answer_from_resolved_state: false,
            avoid_session_history_lookup: true,
            avoid_run_recipe_lookup: true,
            avoid_workspace_discovery: true,
            avoid_bootstrap_doc_reads: true,
        })
        .unwrap();

        assert!(block.contains("[execution-guidance]"));
        assert!(block.contains("resolved_from: configured_runtime"));
        assert!(block.contains("preferred_capabilities: delivery"));
        assert!(block.contains("prefer_local_delivery_tool: message_send"));
        assert!(block.contains("message_send_requires_content_only_when_target_is_resolved: true"));
        assert!(block.contains("avoid_delegate_delivery_when_target_is_resolved: true"));
        assert!(block.contains("avoid_workspace_discovery: true"));
    }

    fn profile_with_facts(facts: &[(&str, serde_json::Value)]) -> UserProfile {
        let mut profile = UserProfile::default();
        for (key, value) in facts {
            profile.set(*key, value.clone());
        }
        profile
    }

    #[test]
    fn formats_recent_failure_hints_when_present() {
        let block = format_execution_guidance(&ExecutionGuidance {
            resolved_from: Some(ResolutionSource::DialogueState),
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: vec![ExecutionFailureHint {
                tool_name: "message_send".into(),
                failure_kind: ToolFailureKind::SchemaMismatch,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
            }],
            recent_admission_hint: None,
            prefer_answer_from_resolved_state: false,
            avoid_session_history_lookup: true,
            avoid_run_recipe_lookup: true,
            avoid_workspace_discovery: true,
            avoid_bootstrap_doc_reads: true,
        })
        .unwrap();

        assert!(block.contains("avoid_immediate_retry_of_recent_failures: true"));
        assert!(block.contains(
            "recent_failure_hints: message_send:schema_mismatch->adjust_arguments_or_target"
        ));
    }

    #[test]
    fn formats_recent_admission_hints_when_present() {
        let block = format_execution_guidance(&ExecutionGuidance {
            resolved_from: Some(ResolutionSource::ConfiguredRuntime),
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            recent_admission_hint: Some(ExecutionAdmissionHint {
                reasons: vec![
                    CandidateAdmissionReason::RequiresLane(
                        crate::config::schema::CapabilityLane::ImageGeneration,
                    ),
                    CandidateAdmissionReason::CapabilityMetadataStale(
                        crate::config::schema::CapabilityLane::ImageGeneration,
                    ),
                ],
                recommended_action: Some(AdmissionRepairHint::RefreshCapabilityMetadata(
                    crate::config::schema::CapabilityLane::ImageGeneration,
                )),
            }),
            prefer_answer_from_resolved_state: false,
            avoid_session_history_lookup: true,
            avoid_run_recipe_lookup: true,
            avoid_workspace_discovery: true,
            avoid_bootstrap_doc_reads: true,
        })
        .unwrap();

        assert!(block.contains("respect_recent_route_admission_hint: true"));
        assert!(block.contains(
            "recent_admission_reasons: requires_image_generation, metadata_stale_image_generation"
        ));
        assert!(block.contains(
            "recent_admission_recommended_action: refresh_capability_metadata:image_generation"
        ));
    }

    #[test]
    fn bounds_recent_admission_hints_to_distinct_compact_subset() {
        let guidance = build_execution_guidance(
            Some(&ResolutionPlan {
                source_order: vec![ResolutionSource::ConfiguredRuntime],
                confidence: ResolutionConfidence::High,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            Some(&TurnInterpretation::default()),
            &[],
            &[
                CandidateAdmissionReason::RequiresLane(
                    crate::config::schema::CapabilityLane::ImageGeneration,
                ),
                CandidateAdmissionReason::RequiresLane(
                    crate::config::schema::CapabilityLane::ImageGeneration,
                ),
                CandidateAdmissionReason::CapabilityMetadataStale(
                    crate::config::schema::CapabilityLane::ImageGeneration,
                ),
                CandidateAdmissionReason::CandidateWindowNearLimit,
                CandidateAdmissionReason::ProviderContextWarning,
            ],
            Some(AdmissionRepairHint::RefreshCapabilityMetadata(
                crate::config::schema::CapabilityLane::ImageGeneration,
            )),
        )
        .unwrap();

        let admission_hint = guidance
            .recent_admission_hint
            .expect("bounded admission hint");
        assert_eq!(
            admission_hint.reasons,
            vec![
                CandidateAdmissionReason::RequiresLane(
                    crate::config::schema::CapabilityLane::ImageGeneration,
                ),
                CandidateAdmissionReason::CapabilityMetadataStale(
                    crate::config::schema::CapabilityLane::ImageGeneration,
                ),
                CandidateAdmissionReason::CandidateWindowNearLimit,
            ]
        );
    }
}
