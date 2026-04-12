use synapse_domain::application::services::tool_repair::{
    build_tool_repair_trace, build_tool_repair_trace_for_capability,
    build_tool_repair_trace_with_action,
};
use synapse_domain::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairTrace};
use synapse_providers::ProviderCapabilityError;
use synapse_security::scrub_credentials;

pub(crate) fn classify_tool_execution_error(
    tool_name: &str,
    error: &anyhow::Error,
) -> ToolRepairTrace {
    if let Some(capability_error) = error.downcast_ref::<ProviderCapabilityError>() {
        return build_tool_repair_trace_for_capability(
            tool_name,
            &capability_error.capability,
            Some(&scrub_credentials(&capability_error.message)),
        );
    }

    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        let detail = scrub_credentials(&error.to_string());
        return match io_error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                build_tool_repair_trace(tool_name, ToolFailureKind::PolicyBlocked, Some(&detail))
            }
            std::io::ErrorKind::NotFound => {
                build_tool_repair_trace(tool_name, ToolFailureKind::MissingResource, Some(&detail))
            }
            std::io::ErrorKind::TimedOut => {
                build_tool_repair_trace(tool_name, ToolFailureKind::Timeout, Some(&detail))
            }
            _ => build_tool_repair_trace(tool_name, ToolFailureKind::RuntimeError, Some(&detail)),
        };
    }

    if error.downcast_ref::<serde_json::Error>().is_some() {
        return build_tool_repair_trace(
            tool_name,
            ToolFailureKind::SchemaMismatch,
            Some(&scrub_credentials(&error.to_string())),
        );
    }

    if let Some(reqwest_error) = error.downcast_ref::<reqwest::Error>() {
        if reqwest_error.is_timeout() {
            return build_tool_repair_trace(
                tool_name,
                ToolFailureKind::Timeout,
                Some(&scrub_credentials(&error.to_string())),
            );
        }
        if let Some(status) = reqwest_error.status() {
            let detail = scrub_credentials(&error.to_string());
            match status.as_u16() {
                401 | 403 => {
                    return build_tool_repair_trace_with_action(
                        tool_name,
                        ToolFailureKind::AuthFailure,
                        ToolRepairAction::AuthenticateOrConfigureCredentials,
                        Some(&detail),
                    );
                }
                413 => {
                    return build_tool_repair_trace_with_action(
                        tool_name,
                        ToolFailureKind::ContextLimitExceeded,
                        ToolRepairAction::CompactSessionOrStartFreshHandoff,
                        Some(&detail),
                    );
                }
                _ => {}
            }
        }
    }

    let detail = scrub_credentials(&error.to_string());
    let lower = detail.to_ascii_lowercase();
    if lower.contains("missing field")
        || lower.contains("unknown variant")
        || lower.contains("invalid type")
        || lower.contains("expected ")
    {
        return build_tool_repair_trace(tool_name, ToolFailureKind::SchemaMismatch, Some(&detail));
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return build_tool_repair_trace(tool_name, ToolFailureKind::Timeout, Some(&detail));
    }

    build_tool_repair_trace(tool_name, ToolFailureKind::RuntimeError, Some(&detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::ports::provider::ProviderCapabilityRequirement;

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
