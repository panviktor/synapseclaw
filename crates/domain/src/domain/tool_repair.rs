use crate::config::schema::CapabilityLane;
use crate::ports::tool::ToolRuntimeRole;
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRepairOutcome {
    Failed,
    Resolved,
    Downgraded,
}

impl Default for ToolRepairOutcome {
    fn default() -> Self {
        Self::Failed
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRepairAttemptReason {
    ModelToolCall,
    HookPolicy,
    ApprovalGate,
    DuplicateGuard,
    ToolExecution,
}

impl Default for ToolRepairAttemptReason {
    fn default() -> Self {
        Self::ModelToolCall
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgumentShapeKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolArgumentShape {
    pub root_kind: ToolArgumentShapeKind,
    pub top_level_keys: Vec<String>,
    pub missing_required_keys: Vec<String>,
    pub approximate_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRepairRoute {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<CapabilityLane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRepairAdmissionState {
    pub action: String,
    pub pressure_state: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolRepairSuppressionKey {
    Tool { tool_name: String },
    ToolRole { role: ToolRuntimeRole },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRepairTrace {
    pub observed_at_unix: i64,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_role: Option<ToolRuntimeRole>,
    pub failure_kind: ToolFailureKind,
    pub suggested_action: ToolRepairAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<ToolRepairRoute>,
    #[serde(default)]
    pub attempt_reason: ToolRepairAttemptReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_shape: Option<ToolArgumentShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_state: Option<ToolRepairAdmissionState>,
    #[serde(default)]
    pub repair_outcome: ToolRepairOutcome,
    #[serde(default)]
    pub expires_at_unix: i64,
    #[serde(default)]
    pub repeat_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppression_key: Option<ToolRepairSuppressionKey>,
    pub detail: Option<String>,
}

impl Default for ToolRepairTrace {
    fn default() -> Self {
        Self {
            observed_at_unix: 0,
            tool_name: String::new(),
            tool_role: None,
            failure_kind: ToolFailureKind::RuntimeError,
            suggested_action: ToolRepairAction::InspectRuntimeFailure,
            route: None,
            attempt_reason: ToolRepairAttemptReason::ModelToolCall,
            argument_shape: None,
            replay_args: None,
            admission_state: None,
            repair_outcome: ToolRepairOutcome::Failed,
            expires_at_unix: 0,
            repeat_count: 0,
            suppression_key: None,
            detail: None,
        }
    }
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

pub fn tool_repair_outcome_name(outcome: ToolRepairOutcome) -> &'static str {
    match outcome {
        ToolRepairOutcome::Failed => "failed",
        ToolRepairOutcome::Resolved => "resolved",
        ToolRepairOutcome::Downgraded => "downgraded",
    }
}

pub fn tool_repair_attempt_reason_name(reason: ToolRepairAttemptReason) -> &'static str {
    match reason {
        ToolRepairAttemptReason::ModelToolCall => "model_tool_call",
        ToolRepairAttemptReason::HookPolicy => "hook_policy",
        ToolRepairAttemptReason::ApprovalGate => "approval_gate",
        ToolRepairAttemptReason::DuplicateGuard => "duplicate_guard",
        ToolRepairAttemptReason::ToolExecution => "tool_execution",
    }
}

pub fn tool_argument_shape_kind_name(kind: ToolArgumentShapeKind) -> &'static str {
    match kind {
        ToolArgumentShapeKind::Null => "null",
        ToolArgumentShapeKind::Bool => "bool",
        ToolArgumentShapeKind::Number => "number",
        ToolArgumentShapeKind::String => "string",
        ToolArgumentShapeKind::Array => "array",
        ToolArgumentShapeKind::Object => "object",
    }
}
