use crate::application::services::turn_interpretation::TurnInterpretation;
use crate::domain::conversation_target::ConversationDeliveryTarget;
use crate::domain::tool_repair::{ToolFailureKind, ToolRepairTrace};
use crate::domain::turn_admission::{
    admission_repair_hint_label, candidate_admission_reason_label, AdmissionRepairHint,
    CandidateAdmissionReason,
};
use crate::domain::user_profile::DELIVERY_TARGET_PREFERENCE_KEY;

const MAX_ASSUMPTIONS: usize = 8;
const MAX_VALUE_CHARS: usize = 160;
const CHALLENGED_CONFIDENCE_BASIS_POINTS: u16 = 3_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAssumptionKind {
    ActiveTask,
    CurrentConversation,
    DeliveryTarget,
    ProfileFact,
    RouteCapability,
    ContextWindow,
    WorkspaceAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAssumptionSource {
    ConfiguredRuntime,
    CurrentConversation,
    DialogueState,
    RouteAdmission,
    UserMessage,
    UserProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAssumptionFreshness {
    CurrentTurn,
    SessionRecent,
    Challenged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAssumptionInvalidation {
    ConversationChanged,
    ContextOverflow,
    DeliveryFailure,
    ProfileUpdate,
    RouteAdmissionFailure,
    UserContradiction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAssumptionReplacementPath {
    AskUserClarification,
    CompactSession,
    RefreshCapabilityMetadata,
    SwitchRoute,
    UpdateProfile,
    UseCurrentConversation,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAssumption {
    pub kind: RuntimeAssumptionKind,
    pub source: RuntimeAssumptionSource,
    pub freshness: RuntimeAssumptionFreshness,
    pub confidence_basis_points: u16,
    pub value: String,
    pub invalidation: RuntimeAssumptionInvalidation,
    pub replacement_path: RuntimeAssumptionReplacementPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAssumptionChallenge<'a> {
    pub kind: RuntimeAssumptionKind,
    pub value: &'a str,
    pub invalidation: RuntimeAssumptionInvalidation,
    pub replacement_path: RuntimeAssumptionReplacementPath,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeAssumptionInput<'a> {
    pub user_message: &'a str,
    pub interpretation: Option<&'a TurnInterpretation>,
    pub recent_admission_repair: Option<AdmissionRepairHint>,
    pub recent_admission_reasons: &'a [CandidateAdmissionReason],
}

pub fn build_runtime_assumptions(input: RuntimeAssumptionInput<'_>) -> Vec<RuntimeAssumption> {
    let mut assumptions = Vec::new();

    if let Some(task) = bounded_non_empty(input.user_message, MAX_VALUE_CHARS) {
        push_assumption(
            &mut assumptions,
            RuntimeAssumption {
                kind: RuntimeAssumptionKind::ActiveTask,
                source: RuntimeAssumptionSource::UserMessage,
                freshness: RuntimeAssumptionFreshness::CurrentTurn,
                confidence_basis_points: 9_000,
                value: task,
                invalidation: RuntimeAssumptionInvalidation::UserContradiction,
                replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
            },
        );
    }

    if let Some(interpretation) = input.interpretation {
        collect_interpretation_assumptions(&mut assumptions, interpretation);
    }

    collect_route_admission_assumptions(
        &mut assumptions,
        input.recent_admission_repair,
        input.recent_admission_reasons,
    );
    bound_runtime_assumptions(assumptions)
}

pub fn merge_runtime_assumption_ledger(
    existing: &[RuntimeAssumption],
    observed: &[RuntimeAssumption],
) -> Vec<RuntimeAssumption> {
    let mut ledger = existing.to_vec();
    for assumption in observed {
        upsert_assumption(&mut ledger, assumption.clone());
    }
    bound_runtime_assumptions(ledger)
}

pub fn challenge_runtime_assumption_ledger(
    existing: &[RuntimeAssumption],
    challenge: RuntimeAssumptionChallenge<'_>,
) -> Vec<RuntimeAssumption> {
    let mut ledger = existing.to_vec();
    let mut matched = false;
    for assumption in ledger
        .iter_mut()
        .filter(|assumption| assumption.kind == challenge.kind)
    {
        assumption.freshness = RuntimeAssumptionFreshness::Challenged;
        assumption.confidence_basis_points = assumption
            .confidence_basis_points
            .min(CHALLENGED_CONFIDENCE_BASIS_POINTS);
        assumption.invalidation = challenge.invalidation;
        assumption.replacement_path = challenge.replacement_path;
        matched = true;
    }
    if !matched {
        if let Some(value) = bounded_non_empty(challenge.value, MAX_VALUE_CHARS) {
            ledger.push(RuntimeAssumption {
                kind: challenge.kind,
                source: RuntimeAssumptionSource::RouteAdmission,
                freshness: RuntimeAssumptionFreshness::Challenged,
                confidence_basis_points: CHALLENGED_CONFIDENCE_BASIS_POINTS,
                value,
                invalidation: challenge.invalidation,
                replacement_path: challenge.replacement_path,
            });
        }
    }
    bound_runtime_assumptions(ledger)
}

pub fn apply_tool_repair_assumption_challenges(
    assumptions: &[RuntimeAssumption],
    tool_repairs: &[ToolRepairTrace],
) -> Vec<RuntimeAssumption> {
    let mut ledger = assumptions.to_vec();
    for repair in tool_repairs {
        if let Some(challenge) = tool_repair_assumption_challenge(repair) {
            ledger = challenge_runtime_assumption_ledger(&ledger, challenge);
        }
    }
    ledger
}

pub fn bound_runtime_assumptions(assumptions: Vec<RuntimeAssumption>) -> Vec<RuntimeAssumption> {
    let mut bounded = Vec::new();
    for mut assumption in assumptions {
        let Some(value) = bounded_non_empty(&assumption.value, MAX_VALUE_CHARS) else {
            continue;
        };
        assumption.value = value;
        assumption.confidence_basis_points = assumption.confidence_basis_points.min(10_000);
        push_assumption(&mut bounded, assumption);
        if bounded.len() >= MAX_ASSUMPTIONS {
            break;
        }
    }
    bounded
}

pub fn format_runtime_assumption(assumption: &RuntimeAssumption) -> String {
    format!(
        "kind={} source={} freshness={} confidence={} value={} invalidation={} replacement_path={}",
        runtime_assumption_kind_name(assumption.kind),
        runtime_assumption_source_name(assumption.source),
        runtime_assumption_freshness_name(assumption.freshness),
        assumption.confidence_basis_points,
        assumption.value,
        runtime_assumption_invalidation_name(assumption.invalidation),
        runtime_assumption_replacement_path_name(assumption.replacement_path),
    )
}

pub fn runtime_assumption_kind_name(kind: RuntimeAssumptionKind) -> &'static str {
    match kind {
        RuntimeAssumptionKind::ActiveTask => "active_task",
        RuntimeAssumptionKind::CurrentConversation => "current_conversation",
        RuntimeAssumptionKind::DeliveryTarget => "delivery_target",
        RuntimeAssumptionKind::ProfileFact => "profile_fact",
        RuntimeAssumptionKind::RouteCapability => "route_capability",
        RuntimeAssumptionKind::ContextWindow => "context_window",
        RuntimeAssumptionKind::WorkspaceAnchor => "workspace_anchor",
    }
}

pub fn runtime_assumption_source_name(source: RuntimeAssumptionSource) -> &'static str {
    match source {
        RuntimeAssumptionSource::ConfiguredRuntime => "configured_runtime",
        RuntimeAssumptionSource::CurrentConversation => "current_conversation",
        RuntimeAssumptionSource::DialogueState => "dialogue_state",
        RuntimeAssumptionSource::RouteAdmission => "route_admission",
        RuntimeAssumptionSource::UserMessage => "user_message",
        RuntimeAssumptionSource::UserProfile => "user_profile",
    }
}

pub fn runtime_assumption_freshness_name(freshness: RuntimeAssumptionFreshness) -> &'static str {
    match freshness {
        RuntimeAssumptionFreshness::CurrentTurn => "current_turn",
        RuntimeAssumptionFreshness::SessionRecent => "session_recent",
        RuntimeAssumptionFreshness::Challenged => "challenged",
    }
}

pub fn runtime_assumption_invalidation_name(
    invalidation: RuntimeAssumptionInvalidation,
) -> &'static str {
    match invalidation {
        RuntimeAssumptionInvalidation::ConversationChanged => "conversation_changed",
        RuntimeAssumptionInvalidation::ContextOverflow => "context_overflow",
        RuntimeAssumptionInvalidation::DeliveryFailure => "delivery_failure",
        RuntimeAssumptionInvalidation::ProfileUpdate => "profile_update",
        RuntimeAssumptionInvalidation::RouteAdmissionFailure => "route_admission_failure",
        RuntimeAssumptionInvalidation::UserContradiction => "user_contradiction",
    }
}

pub fn runtime_assumption_replacement_path_name(
    replacement_path: RuntimeAssumptionReplacementPath,
) -> &'static str {
    match replacement_path {
        RuntimeAssumptionReplacementPath::AskUserClarification => "ask_user_clarification",
        RuntimeAssumptionReplacementPath::CompactSession => "compact_session",
        RuntimeAssumptionReplacementPath::RefreshCapabilityMetadata => {
            "refresh_capability_metadata"
        }
        RuntimeAssumptionReplacementPath::SwitchRoute => "switch_route",
        RuntimeAssumptionReplacementPath::UpdateProfile => "update_profile",
        RuntimeAssumptionReplacementPath::UseCurrentConversation => "use_current_conversation",
    }
}

fn tool_repair_assumption_challenge(
    repair: &ToolRepairTrace,
) -> Option<RuntimeAssumptionChallenge<'static>> {
    match repair.failure_kind {
        ToolFailureKind::ContextLimitExceeded => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::ContextWindow,
            value: "tool_context_limit_exceeded",
            invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
            replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
        }),
        ToolFailureKind::AuthFailure => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::RouteCapability,
            value: "tool_auth_failure",
            invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        }),
        ToolFailureKind::CapabilityMismatch | ToolFailureKind::UnknownTool => {
            Some(RuntimeAssumptionChallenge {
                kind: RuntimeAssumptionKind::RouteCapability,
                value: "tool_capability_mismatch",
                invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
                replacement_path: RuntimeAssumptionReplacementPath::SwitchRoute,
            })
        }
        ToolFailureKind::MissingResource => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::WorkspaceAnchor,
            value: "tool_missing_resource",
            invalidation: RuntimeAssumptionInvalidation::UserContradiction,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        }),
        ToolFailureKind::ReportedFailure => Some(RuntimeAssumptionChallenge {
            kind: RuntimeAssumptionKind::DeliveryTarget,
            value: "tool_reported_failure",
            invalidation: RuntimeAssumptionInvalidation::DeliveryFailure,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        }),
        ToolFailureKind::PolicyBlocked
        | ToolFailureKind::DuplicateInvocation
        | ToolFailureKind::Timeout
        | ToolFailureKind::SchemaMismatch
        | ToolFailureKind::RuntimeError => None,
    }
}

fn collect_interpretation_assumptions(
    assumptions: &mut Vec<RuntimeAssumption>,
    interpretation: &TurnInterpretation,
) {
    if let Some(profile) = interpretation.user_profile.as_ref() {
        for (key, value) in profile.iter().take(3) {
            let value = profile.get_text(key).unwrap_or_else(|| value.to_string());
            push_assumption(
                assumptions,
                RuntimeAssumption {
                    kind: RuntimeAssumptionKind::ProfileFact,
                    source: RuntimeAssumptionSource::UserProfile,
                    freshness: RuntimeAssumptionFreshness::SessionRecent,
                    confidence_basis_points: 8_000,
                    value: format!("{key}={value}"),
                    invalidation: RuntimeAssumptionInvalidation::UserContradiction,
                    replacement_path: RuntimeAssumptionReplacementPath::UpdateProfile,
                },
            );
        }
        if let Some(target) = profile.get_delivery_target(DELIVERY_TARGET_PREFERENCE_KEY) {
            push_delivery_target_assumption(
                assumptions,
                RuntimeAssumptionSource::UserProfile,
                RuntimeAssumptionFreshness::SessionRecent,
                7_500,
                &target,
            );
        }
    }

    if let Some(target) = interpretation.configured_delivery_target.as_ref() {
        push_delivery_target_assumption(
            assumptions,
            RuntimeAssumptionSource::ConfiguredRuntime,
            RuntimeAssumptionFreshness::CurrentTurn,
            9_500,
            target,
        );
    }

    if let Some(conversation) = interpretation.current_conversation.as_ref() {
        push_assumption(
            assumptions,
            RuntimeAssumption {
                kind: RuntimeAssumptionKind::CurrentConversation,
                source: RuntimeAssumptionSource::CurrentConversation,
                freshness: RuntimeAssumptionFreshness::CurrentTurn,
                confidence_basis_points: 9_000,
                value: format!(
                    "adapter={},threaded={}",
                    conversation.adapter, conversation.has_thread
                ),
                invalidation: RuntimeAssumptionInvalidation::ConversationChanged,
                replacement_path: RuntimeAssumptionReplacementPath::UseCurrentConversation,
            },
        );
    }

    if let Some(state) = interpretation.dialogue_state.as_ref() {
        if let Some(target) = state.recent_delivery_target.as_ref() {
            push_delivery_target_assumption(
                assumptions,
                RuntimeAssumptionSource::DialogueState,
                RuntimeAssumptionFreshness::SessionRecent,
                7_000,
                target,
            );
        }
        if let Some(workspace_name) = state
            .recent_workspace
            .as_ref()
            .and_then(|workspace| workspace.name.as_deref())
        {
            push_assumption(
                assumptions,
                RuntimeAssumption {
                    kind: RuntimeAssumptionKind::WorkspaceAnchor,
                    source: RuntimeAssumptionSource::DialogueState,
                    freshness: RuntimeAssumptionFreshness::SessionRecent,
                    confidence_basis_points: 7_000,
                    value: workspace_name.to_string(),
                    invalidation: RuntimeAssumptionInvalidation::UserContradiction,
                    replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
                },
            );
        }
    }
}

fn collect_route_admission_assumptions(
    assumptions: &mut Vec<RuntimeAssumption>,
    repair: Option<AdmissionRepairHint>,
    reasons: &[CandidateAdmissionReason],
) {
    if let Some(repair) = repair {
        push_assumption(
            assumptions,
            RuntimeAssumption {
                kind: route_assumption_kind_for_repair(repair),
                source: RuntimeAssumptionSource::RouteAdmission,
                freshness: RuntimeAssumptionFreshness::Challenged,
                confidence_basis_points: 3_500,
                value: admission_repair_hint_label(repair),
                invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
                replacement_path: replacement_path_for_repair(repair),
            },
        );
    }

    for reason in reasons.iter().take(3) {
        push_assumption(
            assumptions,
            RuntimeAssumption {
                kind: route_assumption_kind_for_reason(reason),
                source: RuntimeAssumptionSource::RouteAdmission,
                freshness: RuntimeAssumptionFreshness::Challenged,
                confidence_basis_points: 3_000,
                value: candidate_admission_reason_label(reason),
                invalidation: invalidation_for_reason(reason),
                replacement_path: replacement_path_for_reason(reason),
            },
        );
    }
}

fn push_delivery_target_assumption(
    assumptions: &mut Vec<RuntimeAssumption>,
    source: RuntimeAssumptionSource,
    freshness: RuntimeAssumptionFreshness,
    confidence_basis_points: u16,
    target: &ConversationDeliveryTarget,
) {
    push_assumption(
        assumptions,
        RuntimeAssumption {
            kind: RuntimeAssumptionKind::DeliveryTarget,
            source,
            freshness,
            confidence_basis_points,
            value: format_delivery_target(target),
            invalidation: RuntimeAssumptionInvalidation::DeliveryFailure,
            replacement_path: RuntimeAssumptionReplacementPath::AskUserClarification,
        },
    );
}

fn route_assumption_kind_for_repair(repair: AdmissionRepairHint) -> RuntimeAssumptionKind {
    match repair {
        AdmissionRepairHint::CompactSession | AdmissionRepairHint::StartFreshHandoff => {
            RuntimeAssumptionKind::ContextWindow
        }
        AdmissionRepairHint::SwitchToLane(_)
        | AdmissionRepairHint::SwitchToToolCapableReasoning
        | AdmissionRepairHint::RefreshCapabilityMetadata(_) => {
            RuntimeAssumptionKind::RouteCapability
        }
    }
}

fn route_assumption_kind_for_reason(reason: &CandidateAdmissionReason) -> RuntimeAssumptionKind {
    match reason {
        CandidateAdmissionReason::CandidateWindowMetadataUnknown
        | CandidateAdmissionReason::CandidateWindowNearLimit
        | CandidateAdmissionReason::CandidateWindowExceeded
        | CandidateAdmissionReason::ProviderContextWarning
        | CandidateAdmissionReason::ProviderContextCritical
        | CandidateAdmissionReason::ProviderContextOverflowRisk => {
            RuntimeAssumptionKind::ContextWindow
        }
        CandidateAdmissionReason::RequiresLane(_)
        | CandidateAdmissionReason::MissingFeature(_)
        | CandidateAdmissionReason::CapabilityMetadataUnknown(_)
        | CandidateAdmissionReason::CapabilityMetadataStale(_)
        | CandidateAdmissionReason::CapabilityMetadataLowConfidence(_)
        | CandidateAdmissionReason::SpecializedLaneMismatch(_)
        | CandidateAdmissionReason::CalibrationSuppressedRoute => {
            RuntimeAssumptionKind::RouteCapability
        }
    }
}

fn invalidation_for_reason(reason: &CandidateAdmissionReason) -> RuntimeAssumptionInvalidation {
    match route_assumption_kind_for_reason(reason) {
        RuntimeAssumptionKind::ContextWindow => RuntimeAssumptionInvalidation::ContextOverflow,
        _ => RuntimeAssumptionInvalidation::RouteAdmissionFailure,
    }
}

fn replacement_path_for_repair(repair: AdmissionRepairHint) -> RuntimeAssumptionReplacementPath {
    match repair {
        AdmissionRepairHint::RefreshCapabilityMetadata(_) => {
            RuntimeAssumptionReplacementPath::RefreshCapabilityMetadata
        }
        AdmissionRepairHint::CompactSession => RuntimeAssumptionReplacementPath::CompactSession,
        AdmissionRepairHint::StartFreshHandoff
        | AdmissionRepairHint::SwitchToLane(_)
        | AdmissionRepairHint::SwitchToToolCapableReasoning => {
            RuntimeAssumptionReplacementPath::SwitchRoute
        }
    }
}

fn replacement_path_for_reason(
    reason: &CandidateAdmissionReason,
) -> RuntimeAssumptionReplacementPath {
    match reason {
        CandidateAdmissionReason::CapabilityMetadataUnknown(_)
        | CandidateAdmissionReason::CapabilityMetadataStale(_)
        | CandidateAdmissionReason::CapabilityMetadataLowConfidence(_) => {
            RuntimeAssumptionReplacementPath::RefreshCapabilityMetadata
        }
        CandidateAdmissionReason::CandidateWindowNearLimit
        | CandidateAdmissionReason::CandidateWindowExceeded
        | CandidateAdmissionReason::ProviderContextCritical
        | CandidateAdmissionReason::ProviderContextOverflowRisk => {
            RuntimeAssumptionReplacementPath::CompactSession
        }
        CandidateAdmissionReason::ProviderContextWarning
        | CandidateAdmissionReason::CandidateWindowMetadataUnknown => {
            RuntimeAssumptionReplacementPath::RefreshCapabilityMetadata
        }
        CandidateAdmissionReason::RequiresLane(_)
        | CandidateAdmissionReason::MissingFeature(_)
        | CandidateAdmissionReason::SpecializedLaneMismatch(_)
        | CandidateAdmissionReason::CalibrationSuppressedRoute => {
            RuntimeAssumptionReplacementPath::SwitchRoute
        }
    }
}

fn format_delivery_target(target: &ConversationDeliveryTarget) -> String {
    match target {
        ConversationDeliveryTarget::CurrentConversation => "current_conversation".to_string(),
        ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } => match thread_ref.as_deref() {
            Some(thread) => format!("{channel}:{recipient}#{thread}"),
            None => format!("{channel}:{recipient}"),
        },
    }
}

fn push_assumption(assumptions: &mut Vec<RuntimeAssumption>, assumption: RuntimeAssumption) {
    if assumptions.iter().any(|existing| {
        existing.kind == assumption.kind
            && existing.source == assumption.source
            && existing.value == assumption.value
    }) {
        return;
    }
    assumptions.push(assumption);
}

fn upsert_assumption(assumptions: &mut Vec<RuntimeAssumption>, assumption: RuntimeAssumption) {
    if let Some(existing) = assumptions.iter_mut().find(|existing| {
        existing.kind == assumption.kind
            && existing.source == assumption.source
            && existing.value == assumption.value
    }) {
        *existing = assumption;
    } else {
        assumptions.push(assumption);
    }
}

fn bounded_non_empty(value: &str, max_chars: usize) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else if collapsed.chars().count() <= max_chars {
        Some(collapsed)
    } else {
        Some(format!(
            "{}...",
            collapsed.chars().take(max_chars).collect::<String>()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::{
        CurrentConversationSnapshot, TurnInterpretation,
    };
    use crate::config::schema::CapabilityLane;
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn builds_bounded_runtime_assumptions_from_structured_turn_state() {
        let interpretation = TurnInterpretation {
            user_profile: Some({
                let mut profile = UserProfile::default();
                profile.set("weather_city", serde_json::json!("Berlin"));
                profile.set("local_timezone", serde_json::json!("Europe/Berlin"));
                profile
            }),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: true,
            }),
            ..Default::default()
        };

        let assumptions = build_runtime_assumptions(RuntimeAssumptionInput {
            user_message: "continue the current route-switch analysis",
            interpretation: Some(&interpretation),
            recent_admission_repair: Some(AdmissionRepairHint::RefreshCapabilityMetadata(
                CapabilityLane::Reasoning,
            )),
            recent_admission_reasons: &[CandidateAdmissionReason::CandidateWindowNearLimit],
        });

        assert!(assumptions
            .iter()
            .any(|assumption| assumption.kind == RuntimeAssumptionKind::ProfileFact));
        assert!(assumptions
            .iter()
            .any(|assumption| assumption.kind == RuntimeAssumptionKind::CurrentConversation));
        assert!(assumptions.iter().any(|assumption| {
            assumption.kind == RuntimeAssumptionKind::RouteCapability
                && assumption.freshness == RuntimeAssumptionFreshness::Challenged
        }));
        assert!(assumptions.iter().any(|assumption| {
            assumption.kind == RuntimeAssumptionKind::ContextWindow
                && assumption.replacement_path == RuntimeAssumptionReplacementPath::CompactSession
        }));
    }

    #[test]
    fn formats_runtime_assumption_without_prompt_prose() {
        let assumption = RuntimeAssumption {
            kind: RuntimeAssumptionKind::ProfileFact,
            source: RuntimeAssumptionSource::UserProfile,
            freshness: RuntimeAssumptionFreshness::SessionRecent,
            confidence_basis_points: 8_500,
            value: "local_timezone=Europe/Berlin".into(),
            invalidation: RuntimeAssumptionInvalidation::ProfileUpdate,
            replacement_path: RuntimeAssumptionReplacementPath::UpdateProfile,
        };

        let formatted = format_runtime_assumption(&assumption);

        assert!(formatted.contains("kind=profile_fact"));
        assert!(formatted.contains("source=user_profile"));
        assert!(formatted.contains("replacement_path=update_profile"));
    }

    #[test]
    fn ledger_challenges_matching_assumption_without_promoting_to_memory() {
        let existing = vec![RuntimeAssumption {
            kind: RuntimeAssumptionKind::ContextWindow,
            source: RuntimeAssumptionSource::RouteAdmission,
            freshness: RuntimeAssumptionFreshness::SessionRecent,
            confidence_basis_points: 8_000,
            value: "candidate_window_near_limit".into(),
            invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
            replacement_path: RuntimeAssumptionReplacementPath::RefreshCapabilityMetadata,
        }];

        let ledger = challenge_runtime_assumption_ledger(
            &existing,
            RuntimeAssumptionChallenge {
                kind: RuntimeAssumptionKind::ContextWindow,
                value: "context_limit_exceeded",
                invalidation: RuntimeAssumptionInvalidation::ContextOverflow,
                replacement_path: RuntimeAssumptionReplacementPath::CompactSession,
            },
        );

        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].freshness, RuntimeAssumptionFreshness::Challenged);
        assert_eq!(ledger[0].confidence_basis_points, 3_500);
        assert_eq!(
            ledger[0].replacement_path,
            RuntimeAssumptionReplacementPath::CompactSession
        );
    }

    #[test]
    fn ledger_adds_challenged_assumption_when_no_match_exists() {
        let ledger = challenge_runtime_assumption_ledger(
            &[],
            RuntimeAssumptionChallenge {
                kind: RuntimeAssumptionKind::RouteCapability,
                value: "capability_mismatch",
                invalidation: RuntimeAssumptionInvalidation::RouteAdmissionFailure,
                replacement_path: RuntimeAssumptionReplacementPath::SwitchRoute,
            },
        );

        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].kind, RuntimeAssumptionKind::RouteCapability);
        assert_eq!(ledger[0].freshness, RuntimeAssumptionFreshness::Challenged);
        assert_eq!(ledger[0].value, "capability_mismatch");
    }
}
