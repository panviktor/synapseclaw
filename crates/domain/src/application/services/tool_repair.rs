use crate::domain::tool_repair::{
    ToolArgumentShape, ToolArgumentShapeKind, ToolFailureKind, ToolRepairAction,
    ToolRepairAdmissionState, ToolRepairAttemptReason, ToolRepairOutcome, ToolRepairRoute,
    ToolRepairSuppressionKey, ToolRepairTrace,
};
use crate::domain::turn_admission::{
    candidate_admission_reason_label, context_pressure_state_name, turn_admission_action_name,
};
use crate::ports::provider::ProviderCapabilityRequirement;
use crate::ports::tool::{
    ToolArgumentPolicy, ToolArgumentTransform, ToolContract, ToolRuntimeRole, ToolSpec,
};
use chrono::Utc;
use serde_json::{Map, Value};
use url::Url;

pub const TOOL_REPAIR_TRACE_TTL_SECS: i64 = 48 * 60 * 60;
pub const MAX_TOOL_REPAIR_HISTORY: usize = 8;
const MAX_ARGUMENT_KEYS: usize = 12;
const MAX_ARGUMENT_KEY_CHARS: usize = 48;
const MAX_REPLAY_ARGS_BYTES: usize = 2_048;
const MAX_REPLAY_STRING_CHARS: usize = 240;
const MAX_REPLAY_ARRAY_ITEMS: usize = 8;
const MAX_REPLAY_OBJECT_KEYS: usize = 16;
const MAX_REPLAY_DEPTH: usize = 4;

#[derive(Debug, Clone, Default)]
pub struct ToolRepairTraceContext<'a> {
    pub tool_role: Option<ToolRuntimeRole>,
    pub route: Option<ToolRepairRoute>,
    pub attempt_reason: Option<ToolRepairAttemptReason>,
    pub arguments: Option<&'a serde_json::Value>,
    pub tool_spec: Option<&'a ToolSpec>,
    pub admission:
        Option<&'a crate::application::services::turn_admission::CandidateAdmissionDecision>,
}

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
    build_tool_repair_trace_with_context(
        tool_name,
        failure_kind,
        suggested_action,
        detail,
        ToolRepairTraceContext::default(),
    )
}

pub fn build_tool_repair_trace_with_context(
    tool_name: &str,
    failure_kind: ToolFailureKind,
    suggested_action: ToolRepairAction,
    detail: Option<&str>,
    context: ToolRepairTraceContext<'_>,
) -> ToolRepairTrace {
    let observed_at_unix = Utc::now().timestamp();
    let tool_role = context
        .tool_role
        .or_else(|| context.tool_spec.and_then(|spec| spec.runtime_role));
    ToolRepairTrace {
        observed_at_unix,
        tool_name: tool_name.to_string(),
        tool_role,
        failure_kind,
        suggested_action,
        route: context.route,
        attempt_reason: context.attempt_reason.unwrap_or_default(),
        argument_shape: context
            .arguments
            .map(|arguments| tool_argument_shape(arguments, context.tool_spec)),
        replay_args: None,
        admission_state: context.admission.map(tool_repair_admission_state),
        repair_outcome: ToolRepairOutcome::Failed,
        expires_at_unix: observed_at_unix + TOOL_REPAIR_TRACE_TTL_SECS,
        repeat_count: 1,
        suppression_key: tool_repair_suppression_key(tool_name, tool_role),
        detail: detail.map(ToString::to_string),
    }
}

pub fn enrich_tool_repair_trace(
    mut trace: ToolRepairTrace,
    context: ToolRepairTraceContext<'_>,
) -> ToolRepairTrace {
    let tool_role = context
        .tool_role
        .or(trace.tool_role)
        .or_else(|| context.tool_spec.and_then(|spec| spec.runtime_role));
    trace.tool_role = tool_role;
    if trace.route.is_none() {
        trace.route = context.route;
    }
    if let Some(attempt_reason) = context.attempt_reason {
        trace.attempt_reason = attempt_reason;
    }
    if trace.argument_shape.is_none() {
        trace.argument_shape = context
            .arguments
            .map(|arguments| tool_argument_shape(arguments, context.tool_spec));
    }
    if trace.admission_state.is_none() {
        trace.admission_state = context.admission.map(tool_repair_admission_state);
    }
    if trace.expires_at_unix == 0 {
        trace.expires_at_unix = trace.observed_at_unix + TOOL_REPAIR_TRACE_TTL_SECS;
    }
    if trace.repeat_count == 0 {
        trace.repeat_count = 1;
    }
    if trace.suppression_key.is_none() {
        trace.suppression_key = tool_repair_suppression_key(&trace.tool_name, trace.tool_role);
    }
    trace
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
        && left.tool_role == right.tool_role
        && left.failure_kind == right.failure_kind
        && left.suggested_action == right.suggested_action
        && left.argument_shape == right.argument_shape
        && left.suppression_key == right.suppression_key
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
            last.expires_at_unix = next.expires_at_unix;
            last.repeat_count = last.repeat_count.saturating_add(next.repeat_count.max(1));
            last.repair_outcome = next.repair_outcome;
            if last.replay_args.is_none() {
                last.replay_args = next.replay_args;
            }
            if last.detail.is_none() {
                last.detail = next.detail;
            }
        } else {
            history.push(next);
        }
    } else {
        history.push(next);
    }
}

pub fn apply_successful_tool_repair_observation(
    history: &[ToolRepairTrace],
    tool_name: &str,
    tool_role: Option<ToolRuntimeRole>,
    observed_at_unix: i64,
) -> Vec<ToolRepairTrace> {
    apply_successful_tool_repair_observation_with_args(
        history,
        tool_name,
        tool_role,
        observed_at_unix,
        None,
    )
}

pub fn apply_successful_tool_repair_observation_with_args(
    history: &[ToolRepairTrace],
    tool_name: &str,
    tool_role: Option<ToolRuntimeRole>,
    observed_at_unix: i64,
    replay_args: Option<&Value>,
) -> Vec<ToolRepairTrace> {
    let mut bounded = retained_tool_repair_history(history, observed_at_unix);
    for trace in bounded.iter_mut() {
        let same_tool = trace.tool_name == tool_name;
        let same_role = tool_role.is_some() && trace.tool_role == tool_role;
        if same_tool {
            trace.repair_outcome = ToolRepairOutcome::Resolved;
            trace.observed_at_unix = observed_at_unix;
            trace.expires_at_unix = observed_at_unix + TOOL_REPAIR_TRACE_TTL_SECS;
            if trace.replay_args.is_none() {
                trace.replay_args = replay_args.cloned();
            }
        } else if same_role {
            trace.repair_outcome = ToolRepairOutcome::Downgraded;
            trace.observed_at_unix = observed_at_unix;
            trace.expires_at_unix = observed_at_unix + TOOL_REPAIR_TRACE_TTL_SECS;
        }
    }
    sort_tool_repair_history(&mut bounded);
    trim_tool_repair_history(&mut bounded);
    bounded
}

pub fn sanitized_tool_replay_args(
    tool_name: &str,
    arguments: &Value,
    tool_spec: Option<&ToolSpec>,
) -> Option<Value> {
    let tool_spec = tool_spec?;
    let contract = ToolContract::from_schema(tool_spec.runtime_role, &tool_spec.parameters);
    sanitized_tool_replay_args_with_contract(tool_name, arguments, tool_spec, &contract)
}

pub fn sanitized_tool_replay_args_with_contract(
    tool_name: &str,
    arguments: &Value,
    tool_spec: &ToolSpec,
    contract: &ToolContract,
) -> Option<Value> {
    if tool_name == "shell"
        || !is_replayable_tool_role(contract.runtime_role)
        || !contract.replayable
    {
        return None;
    }
    let sanitized = sanitize_replay_value(
        arguments,
        Some(&tool_spec.parameters),
        Some(contract),
        None,
        0,
    )?;
    if sanitized.to_string().len() > MAX_REPLAY_ARGS_BYTES {
        return None;
    }
    if sanitized
        .as_object()
        .is_some_and(|object| object.is_empty())
    {
        return None;
    }
    let shape = tool_argument_shape(&sanitized, Some(tool_spec));
    if !shape.missing_required_keys.is_empty() {
        return None;
    }
    Some(sanitized)
}

pub fn latest_tool_repair_trace(history: &[ToolRepairTrace]) -> Option<ToolRepairTrace> {
    history
        .iter()
        .max_by_key(|trace| {
            (
                trace.observed_at_unix,
                tool_repair_outcome_sort_key(trace.repair_outcome),
            )
        })
        .cloned()
}

pub fn tool_argument_shape(
    arguments: &serde_json::Value,
    tool_spec: Option<&ToolSpec>,
) -> ToolArgumentShape {
    let mut top_level_keys = match arguments {
        serde_json::Value::Object(map) => map
            .keys()
            .map(|key| bounded_key(key))
            .collect::<Vec<String>>(),
        _ => Vec::new(),
    };
    top_level_keys.sort();
    top_level_keys.dedup();
    top_level_keys.truncate(MAX_ARGUMENT_KEYS);

    let mut missing_required_keys = tool_spec
        .and_then(|spec| spec.parameters.get("required"))
        .and_then(serde_json::Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|key| {
                    !arguments
                        .as_object()
                        .is_some_and(|object| object.contains_key(*key))
                })
                .map(bounded_key)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    missing_required_keys.sort();
    missing_required_keys.dedup();
    missing_required_keys.truncate(MAX_ARGUMENT_KEYS);

    ToolArgumentShape {
        root_kind: argument_shape_kind(arguments),
        top_level_keys,
        missing_required_keys,
        approximate_chars: arguments.to_string().chars().count(),
    }
}

fn sanitize_replay_value(
    value: &Value,
    schema: Option<&Value>,
    contract: Option<&ToolContract>,
    policy: Option<&ToolArgumentPolicy>,
    depth: usize,
) -> Option<Value> {
    if depth > MAX_REPLAY_DEPTH {
        return None;
    }
    if !schema_type_allows_value(schema, value) {
        return None;
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(value.clone()),
        Value::String(value) => {
            if !argument_policy_allows_replay_value(policy, value) {
                return None;
            }
            Some(Value::String(sanitize_replay_string(value, policy)?))
        }
        Value::Array(values) => {
            let item_schema = schema
                .and_then(|schema| schema.get("items"))
                .filter(|schema| schema.is_object());
            let sanitized = values
                .iter()
                .take(MAX_REPLAY_ARRAY_ITEMS)
                .filter_map(|value| {
                    sanitize_replay_value(value, item_schema, None, None, depth + 1)
                })
                .collect::<Vec<_>>();
            Some(Value::Array(sanitized))
        }
        Value::Object(values) => {
            let property_schemas = schema
                .and_then(|schema| schema.get("properties"))
                .and_then(Value::as_object);
            if schema.is_some() && property_schemas.is_none() {
                return None;
            }
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut out = Map::new();
            for key in keys.into_iter().take(MAX_REPLAY_OBJECT_KEYS) {
                let child_schema = property_schemas.and_then(|schemas| schemas.get(key));
                if property_schemas.is_some() && child_schema.is_none() {
                    continue;
                }
                let child_policy = match contract {
                    Some(contract) => match contract.argument(key) {
                        Some(policy) => Some(policy),
                        None => continue,
                    },
                    None => None,
                };
                if child_policy.is_some_and(|policy| !policy.replayable || policy.sensitive) {
                    continue;
                }
                if let Some(value) = values.get(key).and_then(|value| {
                    sanitize_replay_value(value, child_schema, None, child_policy, depth + 1)
                }) {
                    out.insert(bounded_key(key), value);
                }
            }
            Some(Value::Object(out))
        }
    }
}

fn is_replayable_tool_role(role: Option<ToolRuntimeRole>) -> bool {
    matches!(
        role,
        Some(
            ToolRuntimeRole::HistoricalLookup
                | ToolRuntimeRole::WorkspaceDiscovery
                | ToolRuntimeRole::RuntimeStateInspection
                | ToolRuntimeRole::ExternalLookup
        )
    )
}

fn argument_policy_allows_replay_value(policy: Option<&ToolArgumentPolicy>, value: &str) -> bool {
    let Some(policy) = policy else {
        return true;
    };
    if policy.replayable_values.is_empty() {
        return true;
    }
    policy
        .replayable_values
        .iter()
        .any(|allowed| allowed == value)
}

fn schema_type_allows_value(schema: Option<&Value>, value: &Value) -> bool {
    let Some(schema_type) = schema.and_then(|schema| schema.get("type")) else {
        return true;
    };
    match schema_type {
        Value::String(kind) => schema_type_name_allows_value(kind, value),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .any(|kind| schema_type_name_allows_value(kind, value)),
        _ => true,
    }
}

fn schema_type_name_allows_value(kind: &str, value: &Value) -> bool {
    match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn bounded_replay_string(value: &str) -> String {
    value.trim().chars().take(MAX_REPLAY_STRING_CHARS).collect()
}

fn sanitize_replay_string(value: &str, policy: Option<&ToolArgumentPolicy>) -> Option<String> {
    match policy.and_then(|policy| policy.transform) {
        Some(ToolArgumentTransform::UrlOriginPath) => sanitize_replay_url_origin_path(value),
        None => Some(bounded_replay_string(value)),
    }
}

fn sanitize_replay_url_origin_path(value: &str) -> Option<String> {
    let mut parsed = Url::parse(value.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(bounded_replay_string(parsed.as_str()))
}

fn tool_repair_admission_state(
    decision: &crate::application::services::turn_admission::CandidateAdmissionDecision,
) -> ToolRepairAdmissionState {
    ToolRepairAdmissionState {
        action: turn_admission_action_name(decision.snapshot.action).to_string(),
        pressure_state: context_pressure_state_name(decision.snapshot.pressure_state).to_string(),
        reasons: decision
            .reasons
            .iter()
            .map(candidate_admission_reason_label)
            .collect(),
    }
}

fn tool_repair_suppression_key(
    tool_name: &str,
    _tool_role: Option<ToolRuntimeRole>,
) -> Option<ToolRepairSuppressionKey> {
    if tool_name.trim().is_empty() {
        None
    } else {
        Some(ToolRepairSuppressionKey::Tool {
            tool_name: tool_name.to_string(),
        })
    }
}

fn argument_shape_kind(value: &serde_json::Value) -> ToolArgumentShapeKind {
    match value {
        serde_json::Value::Null => ToolArgumentShapeKind::Null,
        serde_json::Value::Bool(_) => ToolArgumentShapeKind::Bool,
        serde_json::Value::Number(_) => ToolArgumentShapeKind::Number,
        serde_json::Value::String(_) => ToolArgumentShapeKind::String,
        serde_json::Value::Array(_) => ToolArgumentShapeKind::Array,
        serde_json::Value::Object(_) => ToolArgumentShapeKind::Object,
    }
}

fn bounded_key(key: &str) -> String {
    key.trim().chars().take(MAX_ARGUMENT_KEY_CHARS).collect()
}

fn trim_tool_repair_history(history: &mut Vec<ToolRepairTrace>) {
    if history.len() > MAX_TOOL_REPAIR_HISTORY {
        let overflow = history.len() - MAX_TOOL_REPAIR_HISTORY;
        history.drain(0..overflow);
    }
}

fn sort_tool_repair_history(history: &mut [ToolRepairTrace]) {
    history.sort_by_key(|trace| {
        (
            trace.observed_at_unix,
            tool_repair_outcome_sort_key(trace.repair_outcome),
        )
    });
}

fn tool_repair_outcome_sort_key(outcome: ToolRepairOutcome) -> u8 {
    match outcome {
        ToolRepairOutcome::Failed => 0,
        ToolRepairOutcome::Downgraded => 1,
        ToolRepairOutcome::Resolved => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::CapabilityLane;
    use crate::ports::tool::{ToolArgumentPolicy, ToolContract, ToolRuntimeRole, ToolSpec};

    fn external_lookup_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("{name} desc"),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["url", "token"],
                "properties": {
                    "url": {"type": "string"},
                    "token": {"type": "string"},
                    "api_key": {"type": "string"}
                }
            }),
            runtime_role: Some(ToolRuntimeRole::ExternalLookup),
        }
    }

    fn replayable_lookup_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("{name} desc"),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {"type": "string"},
                    "api_key": {"type": "string"}
                }
            }),
            runtime_role: Some(ToolRuntimeRole::ExternalLookup),
        }
    }

    fn web_fetch_contract() -> ToolContract {
        ToolContract::replayable(Some(ToolRuntimeRole::ExternalLookup)).with_arguments(vec![
            ToolArgumentPolicy::replayable("url"),
            ToolArgumentPolicy::sensitive("api_key"),
        ])
    }

    fn web_fetch_url_policy_contract() -> ToolContract {
        ToolContract::replayable(Some(ToolRuntimeRole::ExternalLookup))
            .with_arguments(vec![ToolArgumentPolicy::replayable("url")
                .with_transform(ToolArgumentTransform::UrlOriginPath)])
    }

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
    fn context_builder_records_safe_argument_shape_without_raw_values() {
        let spec = external_lookup_spec("web_fetch");
        let args = serde_json::json!({
            "url": "https://example.invalid/private",
            "api_key": "sk-secret-value"
        });

        let trace = build_tool_repair_trace_with_context(
            "web_fetch",
            ToolFailureKind::SchemaMismatch,
            ToolRepairAction::AdjustArgumentsOrTarget,
            Some("missing token"),
            ToolRepairTraceContext {
                route: Some(ToolRepairRoute {
                    provider: "openrouter".into(),
                    model: "qwen/qwen3.6-plus".into(),
                    lane: Some(CapabilityLane::Reasoning),
                    candidate_index: Some(1),
                }),
                arguments: Some(&args),
                tool_spec: Some(&spec),
                ..Default::default()
            },
        );

        assert_eq!(trace.tool_role, Some(ToolRuntimeRole::ExternalLookup));
        assert_eq!(trace.route.as_ref().unwrap().provider, "openrouter");
        let shape = trace.argument_shape.as_ref().expect("argument shape");
        assert_eq!(shape.root_kind, ToolArgumentShapeKind::Object);
        assert_eq!(
            shape.top_level_keys,
            vec!["api_key".to_string(), "url".to_string()]
        );
        assert_eq!(shape.missing_required_keys, vec!["token".to_string()]);
        let debug_shape = format!("{shape:?}");
        assert!(!debug_shape.contains("sk-secret-value"));
        assert!(!debug_shape.contains("example.invalid/private"));
        assert_eq!(
            trace.suppression_key,
            Some(ToolRepairSuppressionKey::Tool {
                tool_name: "web_fetch".into()
            })
        );
        assert_eq!(trace.repeat_count, 1);
        assert!(trace.expires_at_unix > trace.observed_at_unix);
    }

    #[test]
    fn sanitized_replay_args_reject_unstructured_shell_command() {
        let spec = ToolSpec {
            name: "shell".into(),
            description: "shell".into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {"type": "string"},
                    "approved": {"type": "boolean"}
                }
            }),
            runtime_role: Some(ToolRuntimeRole::WorkspaceDiscovery),
        };
        let args = serde_json::json!({ "command": "git status --short" });
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::WorkspaceDiscovery))
            .with_arguments(vec![
                ToolArgumentPolicy::replayable("command"),
                ToolArgumentPolicy::replayable("approved"),
            ]);

        assert!(
            sanitized_tool_replay_args_with_contract("shell", &args, &spec, &contract).is_none()
        );
    }

    #[test]
    fn sanitized_replay_args_keep_structured_non_secret_payload() {
        let spec = replayable_lookup_spec("web_fetch");
        let args = serde_json::json!({
            "url": "https://example.invalid/status",
            "api_key": "sk-secret-value"
        });

        let contract = web_fetch_contract();
        let sanitized =
            sanitized_tool_replay_args_with_contract("web_fetch", &args, &spec, &contract)
                .expect("replay args");

        assert_eq!(
            sanitized,
            serde_json::json!({ "url": "https://example.invalid/status" })
        );
        assert!(!sanitized.to_string().contains("sk-secret-value"));
    }

    #[test]
    fn sanitized_replay_args_apply_url_origin_path_policy() {
        let spec = replayable_lookup_spec("web_fetch");
        let contract = web_fetch_url_policy_contract();
        let args = serde_json::json!({
            "url": "https://user:pass@example.invalid/status?token=secret#frag"
        });

        let sanitized =
            sanitized_tool_replay_args_with_contract("web_fetch", &args, &spec, &contract)
                .expect("replay args");

        assert_eq!(
            sanitized,
            serde_json::json!({ "url": "https://example.invalid/status" })
        );
        assert!(!sanitized.to_string().contains("secret"));
        assert!(!sanitized.to_string().contains("user:pass"));
    }

    #[test]
    fn sanitized_replay_args_require_typed_contract() {
        let spec = replayable_lookup_spec("web_fetch");
        let args = serde_json::json!({ "url": "https://example.invalid/status" });

        assert!(sanitized_tool_replay_args("web_fetch", &args, Some(&spec)).is_none());
    }

    #[test]
    fn sanitized_replay_args_honor_typed_tool_contract() {
        let spec = ToolSpec {
            name: "git_operations".into(),
            description: "git".into(),
            parameters: serde_json::json!({
                "type": "object",
                "required": ["operation"],
                "properties": {
                    "operation": {"type": "string"},
                    "message": {"type": "string"}
                }
            }),
            runtime_role: Some(ToolRuntimeRole::WorkspaceDiscovery),
        };
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::WorkspaceDiscovery))
            .with_arguments(vec![
                ToolArgumentPolicy::replayable("operation").with_values(["status"]),
                ToolArgumentPolicy::blocked("message"),
            ]);

        assert_eq!(
            sanitized_tool_replay_args_with_contract(
                "git_operations",
                &serde_json::json!({ "operation": "status", "message": "ignore me" }),
                &spec,
                &contract,
            ),
            Some(serde_json::json!({ "operation": "status" }))
        );
        assert!(sanitized_tool_replay_args_with_contract(
            "git_operations",
            &serde_json::json!({ "operation": "commit", "message": "ship it" }),
            &spec,
            &contract,
        )
        .is_none());
    }

    #[test]
    fn sanitized_replay_args_reject_schema_type_mismatch() {
        let spec = replayable_lookup_spec("web_fetch");
        let contract = web_fetch_contract();

        assert!(sanitized_tool_replay_args_with_contract(
            "web_fetch",
            &serde_json::json!({ "url": 42 }),
            &spec,
            &contract,
        )
        .is_none());
    }

    #[test]
    fn sanitized_replay_args_reject_contract_with_schema_without_properties() {
        let spec = ToolSpec {
            name: "opaque_lookup".into(),
            description: "opaque".into(),
            parameters: serde_json::json!({
                "type": "object"
            }),
            runtime_role: Some(ToolRuntimeRole::ExternalLookup),
        };
        let contract = ToolContract::replayable(Some(ToolRuntimeRole::ExternalLookup))
            .with_arguments(vec![ToolArgumentPolicy::replayable("anything")]);

        assert!(sanitized_tool_replay_args_with_contract(
            "opaque_lookup",
            &serde_json::json!({ "anything": "goes" }),
            &spec,
            &contract,
        )
        .is_none());
    }

    #[test]
    fn enrich_preserves_attempt_reason_without_context_override() {
        let trace = build_tool_repair_trace_with_context(
            "web_fetch",
            ToolFailureKind::PolicyBlocked,
            ToolRepairAction::InspectRuntimeFailure,
            Some("hook cancelled"),
            ToolRepairTraceContext {
                attempt_reason: Some(ToolRepairAttemptReason::HookPolicy),
                ..Default::default()
            },
        );

        let enriched = enrich_tool_repair_trace(trace, ToolRepairTraceContext::default());

        assert_eq!(enriched.attempt_reason, ToolRepairAttemptReason::HookPolicy);
    }

    #[test]
    fn bounded_history_deduplicates_latest_matching_trace() {
        let history = vec![build_tool_repair_trace(
            "message_send",
            ToolFailureKind::ReportedFailure,
            Some("missing delivery target"),
        )];
        let updated = append_tool_repair_trace(
            &history,
            Some(build_tool_repair_trace(
                "message_send",
                ToolFailureKind::ReportedFailure,
                Some("missing delivery target"),
            )),
            200,
        );

        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].repeat_count, 2);
    }

    #[test]
    fn successful_tool_observation_resolves_same_tool_and_downgrades_same_role() {
        let updated = apply_successful_tool_repair_observation(
            &[
                ToolRepairTrace {
                    observed_at_unix: 100,
                    tool_name: "web_fetch".into(),
                    tool_role: Some(ToolRuntimeRole::ExternalLookup),
                    failure_kind: ToolFailureKind::Timeout,
                    suggested_action: ToolRepairAction::RetryWithSimplerRequest,
                    repair_outcome: ToolRepairOutcome::Failed,
                    expires_at_unix: 100 + TOOL_REPAIR_TRACE_TTL_SECS,
                    repeat_count: 1,
                    ..ToolRepairTrace::default()
                },
                ToolRepairTrace {
                    observed_at_unix: 101,
                    tool_name: "image_info".into(),
                    tool_role: Some(ToolRuntimeRole::ExternalLookup),
                    failure_kind: ToolFailureKind::CapabilityMismatch,
                    suggested_action: ToolRepairAction::InspectRuntimeFailure,
                    repair_outcome: ToolRepairOutcome::Failed,
                    expires_at_unix: 101 + TOOL_REPAIR_TRACE_TTL_SECS,
                    repeat_count: 1,
                    ..ToolRepairTrace::default()
                },
            ],
            "web_fetch",
            Some(ToolRuntimeRole::ExternalLookup),
            200,
        );

        assert_eq!(updated.len(), 2);
        assert_eq!(updated[0].repair_outcome, ToolRepairOutcome::Downgraded);
        assert_eq!(updated[1].repair_outcome, ToolRepairOutcome::Resolved);
        assert_eq!(
            latest_tool_repair_trace(&updated)
                .expect("latest repair")
                .tool_name,
            "web_fetch"
        );
    }

    #[test]
    fn successful_tool_observation_attaches_sanitized_replay_args_to_same_tool_only() {
        let replay_args = serde_json::json!({ "url": "https://example.invalid/status" });

        let updated = apply_successful_tool_repair_observation_with_args(
            &[
                ToolRepairTrace {
                    observed_at_unix: 100,
                    tool_name: "web_fetch".into(),
                    tool_role: Some(ToolRuntimeRole::ExternalLookup),
                    failure_kind: ToolFailureKind::Timeout,
                    suggested_action: ToolRepairAction::RetryWithSimplerRequest,
                    repair_outcome: ToolRepairOutcome::Failed,
                    expires_at_unix: 100 + TOOL_REPAIR_TRACE_TTL_SECS,
                    repeat_count: 1,
                    ..ToolRepairTrace::default()
                },
                ToolRepairTrace {
                    observed_at_unix: 101,
                    tool_name: "image_info".into(),
                    tool_role: Some(ToolRuntimeRole::ExternalLookup),
                    failure_kind: ToolFailureKind::CapabilityMismatch,
                    suggested_action: ToolRepairAction::InspectRuntimeFailure,
                    repair_outcome: ToolRepairOutcome::Failed,
                    expires_at_unix: 101 + TOOL_REPAIR_TRACE_TTL_SECS,
                    repeat_count: 1,
                    ..ToolRepairTrace::default()
                },
            ],
            "web_fetch",
            Some(ToolRuntimeRole::ExternalLookup),
            200,
            Some(&replay_args),
        );

        let web_fetch = updated
            .iter()
            .find(|trace| trace.tool_name == "web_fetch")
            .expect("web fetch trace");
        let image_info = updated
            .iter()
            .find(|trace| trace.tool_name == "image_info")
            .expect("image info trace");
        assert_eq!(web_fetch.replay_args.as_ref(), Some(&replay_args));
        assert!(image_info.replay_args.is_none());
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
                ..ToolRepairTrace::default()
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
                    ..ToolRepairTrace::default()
                },
                ToolRepairTrace {
                    observed_at_unix: 101,
                    tool_name: "shell".into(),
                    failure_kind: ToolFailureKind::RuntimeError,
                    suggested_action: ToolRepairAction::InspectRuntimeFailure,
                    detail: Some("exit 127".into()),
                    ..ToolRepairTrace::default()
                },
            ],
            101,
        );

        assert_eq!(updated.len(), 2);
        assert_eq!(updated[0].tool_name, "message_send");
        assert_eq!(updated[1].tool_name, "shell");
    }
}
