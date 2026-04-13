use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairTrace};
use crate::ports::provider::ProviderCapabilityRequirement;
use chrono::Utc;

pub const TOOL_REPAIR_TRACE_TTL_SECS: i64 = 48 * 60 * 60;
pub const MAX_TOOL_REPAIR_HISTORY: usize = 8;

pub fn build_tool_repair_trace(
    tool_name: &str,
    failure_kind: ToolFailureKind,
    detail: Option<&str>,
) -> ToolRepairTrace {
    build_tool_repair_trace_with_action(
        tool_name,
        failure_kind,
        suggested_action_for_failure(failure_kind),
        detail,
    )
}

pub fn build_tool_repair_trace_with_action(
    tool_name: &str,
    failure_kind: ToolFailureKind,
    suggested_action: ToolRepairAction,
    detail: Option<&str>,
) -> ToolRepairTrace {
    ToolRepairTrace {
        observed_at_unix: Utc::now().timestamp(),
        tool_name: tool_name.to_string(),
        failure_kind,
        suggested_action,
        detail: detail.map(ToString::to_string),
    }
}

pub fn build_tool_repair_trace_for_capability(
    tool_name: &str,
    capability: &ProviderCapabilityRequirement,
    detail: Option<&str>,
) -> ToolRepairTrace {
    let suggested_action = capability
        .repair_lane()
        .map(ToolRepairAction::SwitchRouteLane)
        .unwrap_or(ToolRepairAction::InspectRuntimeFailure);
    build_tool_repair_trace_with_action(
        tool_name,
        ToolFailureKind::CapabilityMismatch,
        suggested_action,
        detail,
    )
}

pub fn append_tool_repair_trace(
    history: &[ToolRepairTrace],
    next: Option<ToolRepairTrace>,
    now_unix: i64,
) -> Vec<ToolRepairTrace> {
    let mut bounded = retained_tool_repair_history(history, now_unix);

    if let Some(next) = next {
        append_single_tool_repair_trace(&mut bounded, next);
    }

    trim_tool_repair_history(&mut bounded);
    bounded
}

pub fn append_tool_repair_traces(
    history: &[ToolRepairTrace],
    next: &[ToolRepairTrace],
    now_unix: i64,
) -> Vec<ToolRepairTrace> {
    let mut bounded = retained_tool_repair_history(history, now_unix);

    for trace in next.iter().cloned() {
        append_single_tool_repair_trace(&mut bounded, trace);
    }

    trim_tool_repair_history(&mut bounded);
    bounded
}

fn suggested_action_for_failure(failure_kind: ToolFailureKind) -> ToolRepairAction {
    match failure_kind {
        ToolFailureKind::UnknownTool => ToolRepairAction::UseKnownTool,
        ToolFailureKind::PolicyBlocked => ToolRepairAction::RequestPermissionOrApproval,
        ToolFailureKind::DuplicateInvocation => ToolRepairAction::AvoidDuplicateRetry,
        ToolFailureKind::AuthFailure => ToolRepairAction::AuthenticateOrConfigureCredentials,
        ToolFailureKind::CapabilityMismatch => ToolRepairAction::InspectRuntimeFailure,
        ToolFailureKind::MissingResource => ToolRepairAction::AdjustArgumentsOrTarget,
        ToolFailureKind::Timeout => ToolRepairAction::RetryWithSimplerRequest,
        ToolFailureKind::SchemaMismatch => ToolRepairAction::AdjustArgumentsOrTarget,
        ToolFailureKind::ContextLimitExceeded => {
            ToolRepairAction::CompactSessionOrStartFreshHandoff
        }
        ToolFailureKind::RuntimeError => ToolRepairAction::InspectRuntimeFailure,
        ToolFailureKind::ReportedFailure => ToolRepairAction::AdjustArgumentsOrTarget,
    }
}

fn same_repair_signature(left: &ToolRepairTrace, right: &ToolRepairTrace) -> bool {
    left.tool_name == right.tool_name
        && left.failure_kind == right.failure_kind
        && left.suggested_action == right.suggested_action
        && left.detail == right.detail
}

fn retained_tool_repair_history(
    history: &[ToolRepairTrace],
    now_unix: i64,
) -> Vec<ToolRepairTrace> {
    history
        .iter()
        .filter(|trace| trace.observed_at_unix >= now_unix - TOOL_REPAIR_TRACE_TTL_SECS)
        .cloned()
        .collect::<Vec<_>>()
}

fn append_single_tool_repair_trace(history: &mut Vec<ToolRepairTrace>, next: ToolRepairTrace) {
    if let Some(last) = history.last_mut() {
        if same_repair_signature(last, &next) {
            last.observed_at_unix = next.observed_at_unix;
        } else {
            history.push(next);
        }
    } else {
        history.push(next);
    }
}

fn trim_tool_repair_history(history: &mut Vec<ToolRepairTrace>) {
    if history.len() > MAX_TOOL_REPAIR_HISTORY {
        let overflow = history.len() - MAX_TOOL_REPAIR_HISTORY;
        history.drain(0..overflow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::CapabilityLane;

    #[test]
    fn maps_duplicate_invocation_to_duplicate_retry_guard() {
        let trace = build_tool_repair_trace(
            "message_send",
            ToolFailureKind::DuplicateInvocation,
            Some("duplicate"),
        );

        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::AvoidDuplicateRetry
        );
        assert_eq!(trace.detail.as_deref(), Some("duplicate"));
        assert!(trace.observed_at_unix > 0);
    }

    #[test]
    fn maps_runtime_error_to_runtime_inspection() {
        let trace =
            build_tool_repair_trace("shell", ToolFailureKind::RuntimeError, Some("crashed"));

        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::InspectRuntimeFailure
        );
    }

    #[test]
    fn maps_auth_failure_to_credential_repair() {
        let trace = build_tool_repair_trace(
            "web_fetch",
            ToolFailureKind::AuthFailure,
            Some("401 Unauthorized"),
        );

        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::AuthenticateOrConfigureCredentials
        );
    }

    #[test]
    fn capability_trace_prefers_switch_route_lane_when_lane_is_known() {
        let trace = build_tool_repair_trace_for_capability(
            "image_info",
            &ProviderCapabilityRequirement::VisionInput,
            Some("provider does not support vision"),
        );

        assert_eq!(trace.failure_kind, ToolFailureKind::CapabilityMismatch);
        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::SwitchRouteLane(CapabilityLane::MultimodalUnderstanding)
        );
    }

    #[test]
    fn maps_missing_resource_to_argument_adjustment() {
        let trace = build_tool_repair_trace(
            "file_read",
            ToolFailureKind::MissingResource,
            Some("No such file or directory"),
        );

        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::AdjustArgumentsOrTarget
        );
    }

    #[test]
    fn maps_timeout_to_simpler_retry() {
        let trace =
            build_tool_repair_trace("web_fetch", ToolFailureKind::Timeout, Some("timed out"));

        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::RetryWithSimplerRequest
        );
    }

    #[test]
    fn maps_schema_mismatch_to_argument_adjustment() {
        let trace = build_tool_repair_trace(
            "message_send",
            ToolFailureKind::SchemaMismatch,
            Some("missing field `content`"),
        );

        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::AdjustArgumentsOrTarget
        );
    }

    #[test]
    fn bounded_history_deduplicates_latest_matching_trace() {
        let history = vec![ToolRepairTrace {
            observed_at_unix: 100,
            tool_name: "message_send".into(),
            failure_kind: ToolFailureKind::ReportedFailure,
            suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
            detail: Some("missing delivery target".into()),
        }];
        let updated = append_tool_repair_trace(
            &history,
            Some(ToolRepairTrace {
                observed_at_unix: 200,
                tool_name: "message_send".into(),
                failure_kind: ToolFailureKind::ReportedFailure,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                detail: Some("missing delivery target".into()),
            }),
            200,
        );

        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].observed_at_unix, 200);
    }

    #[test]
    fn bounded_history_drops_expired_entries() {
        let updated = append_tool_repair_trace(
            &[ToolRepairTrace {
                observed_at_unix: 10,
                tool_name: "shell".into(),
                failure_kind: ToolFailureKind::RuntimeError,
                suggested_action: ToolRepairAction::InspectRuntimeFailure,
                detail: None,
            }],
            None,
            10 + TOOL_REPAIR_TRACE_TTL_SECS + 1,
        );

        assert!(updated.is_empty());
    }

    #[test]
    fn bounded_history_appends_multiple_distinct_traces() {
        let updated = append_tool_repair_traces(
            &[],
            &[
                ToolRepairTrace {
                    observed_at_unix: 100,
                    tool_name: "message_send".into(),
                    failure_kind: ToolFailureKind::ReportedFailure,
                    suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                    detail: Some("missing target".into()),
                },
                ToolRepairTrace {
                    observed_at_unix: 101,
                    tool_name: "shell".into(),
                    failure_kind: ToolFailureKind::RuntimeError,
                    suggested_action: ToolRepairAction::InspectRuntimeFailure,
                    detail: Some("exit 127".into()),
                },
            ],
            101,
        );

        assert_eq!(updated.len(), 2);
        assert_eq!(updated[0].tool_name, "message_send");
        assert_eq!(updated[1].tool_name, "shell");
    }
}
