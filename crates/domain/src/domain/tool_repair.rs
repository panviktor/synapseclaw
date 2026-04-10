use crate::config::schema::CapabilityLane;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolFailureKind {
    UnknownTool,
    PolicyBlocked,
    DuplicateInvocation,
    AuthFailure,
    CapabilityMismatch,
    MissingResource,
    Timeout,
    SchemaMismatch,
    ContextLimitExceeded,
    RuntimeError,
    ReportedFailure,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolRepairAction {
    UseKnownTool,
    RequestPermissionOrApproval,
    AvoidDuplicateRetry,
    AuthenticateOrConfigureCredentials,
    RetryWithSimplerRequest,
    AdjustArgumentsOrTarget,
    CompactSessionOrStartFreshHandoff,
    InspectRuntimeFailure,
    SwitchRouteLane(CapabilityLane),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRepairTrace {
    pub observed_at_unix: i64,
    pub tool_name: String,
    pub failure_kind: ToolFailureKind,
    pub suggested_action: ToolRepairAction,
    pub detail: Option<String>,
}

pub fn tool_failure_kind_name(kind: ToolFailureKind) -> &'static str {
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

pub fn tool_repair_action_name(action: ToolRepairAction) -> &'static str {
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
