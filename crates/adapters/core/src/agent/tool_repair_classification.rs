use crate::runtime::runtime_error_classification::{classify_runtime_error, RuntimeErrorClassKind};
use synapse_domain::application::services::tool_repair::{
    build_tool_repair_trace, build_tool_repair_trace_for_capability,
    build_tool_repair_trace_with_action,
};
use synapse_domain::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairTrace};

pub(crate) fn classify_tool_execution_error(
    tool_name: &str,
    error: &anyhow::Error,
) -> ToolRepairTrace {
    let class = classify_runtime_error(error);
    if let Some(capability_error) = class.capability_error {
        return build_tool_repair_trace_for_capability(
            tool_name,
            &capability_error.capability,
            Some(&class.detail),
        );
    }

    match class.kind {
        RuntimeErrorClassKind::CapabilityMismatch => build_tool_repair_trace(
            tool_name,
            ToolFailureKind::CapabilityMismatch,
            Some(&class.detail),
        ),
        RuntimeErrorClassKind::AuthFailure => build_tool_repair_trace_with_action(
            tool_name,
            ToolFailureKind::AuthFailure,
            ToolRepairAction::AuthenticateOrConfigureCredentials,
            Some(&class.detail),
        ),
        RuntimeErrorClassKind::PolicyBlocked => build_tool_repair_trace(
            tool_name,
            ToolFailureKind::PolicyBlocked,
            Some(&class.detail),
        ),
        RuntimeErrorClassKind::MissingResource => build_tool_repair_trace(
            tool_name,
            ToolFailureKind::MissingResource,
            Some(&class.detail),
        ),
        RuntimeErrorClassKind::Timeout => {
            build_tool_repair_trace(tool_name, ToolFailureKind::Timeout, Some(&class.detail))
        }
        RuntimeErrorClassKind::SchemaMismatch => build_tool_repair_trace(
            tool_name,
            ToolFailureKind::SchemaMismatch,
            Some(&class.detail),
        ),
        RuntimeErrorClassKind::ContextLimitExceeded => build_tool_repair_trace_with_action(
            tool_name,
            ToolFailureKind::ContextLimitExceeded,
            ToolRepairAction::CompactSessionOrStartFreshHandoff,
            Some(&class.detail),
        ),
        RuntimeErrorClassKind::RuntimeFailure => build_tool_repair_trace(
            tool_name,
            ToolFailureKind::RuntimeError,
            Some(&class.detail),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::ports::provider::ProviderCapabilityRequirement;
    use synapse_providers::ProviderCapabilityError;

    #[test]
    fn capability_error_maps_to_lane_switch() {
        let error = anyhow::Error::new(ProviderCapabilityError {
            provider: "openrouter".into(),
            capability: ProviderCapabilityRequirement::VisionInput,
            message: "provider does not support vision".into(),
        });

        let trace = classify_tool_execution_error("image_info", &error);

        assert_eq!(trace.failure_kind, ToolFailureKind::CapabilityMismatch);
        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::SwitchRouteLane(
                synapse_domain::config::schema::CapabilityLane::MultimodalUnderstanding
            )
        );
    }

    #[test]
    fn generic_runtime_error_falls_back_to_runtime_failure() {
        let error = anyhow::anyhow!("process crashed");

        let trace = classify_tool_execution_error("shell", &error);

        assert_eq!(trace.failure_kind, ToolFailureKind::RuntimeError);
        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::InspectRuntimeFailure
        );
    }

    #[test]
    fn io_not_found_maps_to_missing_resource() {
        let error = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing file",
        ));

        let trace = classify_tool_execution_error("file_read", &error);

        assert_eq!(trace.failure_kind, ToolFailureKind::MissingResource);
        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::AdjustArgumentsOrTarget
        );
    }

    #[test]
    fn io_timeout_maps_to_timeout_failure() {
        let error = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "socket timed out",
        ));

        let trace = classify_tool_execution_error("web_fetch", &error);

        assert_eq!(trace.failure_kind, ToolFailureKind::Timeout);
        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::RetryWithSimplerRequest
        );
    }

    #[test]
    fn serde_error_maps_to_schema_mismatch() {
        let err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let error = anyhow::Error::new(err);

        let trace = classify_tool_execution_error("message_send", &error);

        assert_eq!(trace.failure_kind, ToolFailureKind::SchemaMismatch);
        assert_eq!(
            trace.suggested_action,
            ToolRepairAction::AdjustArgumentsOrTarget
        );
    }
}
