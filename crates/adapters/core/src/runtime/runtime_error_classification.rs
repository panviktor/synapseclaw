use synapse_domain::ports::agent_runtime::{AgentRuntimeError, AgentRuntimeErrorKind};
use synapse_providers::error_classification::classify_context_limit_error;
use synapse_providers::ProviderCapabilityError;
use synapse_security::scrub_credentials;

pub(crate) fn classify_agent_runtime_error(err: anyhow::Error) -> AgentRuntimeError {
    if err.downcast_ref::<ProviderCapabilityError>().is_some() {
        return AgentRuntimeError::new(
            AgentRuntimeErrorKind::CapabilityMismatch,
            scrub_credentials(&err.to_string()),
        );
    }

    if let Some(io_error) = err.downcast_ref::<std::io::Error>() {
        let detail = scrub_credentials(&err.to_string());
        return match io_error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                AgentRuntimeError::new(AgentRuntimeErrorKind::PolicyBlocked, detail)
            }
            std::io::ErrorKind::NotFound => {
                AgentRuntimeError::new(AgentRuntimeErrorKind::MissingResource, detail)
            }
            std::io::ErrorKind::TimedOut => {
                AgentRuntimeError::new(AgentRuntimeErrorKind::Timeout, detail)
            }
            _ => AgentRuntimeError::new(AgentRuntimeErrorKind::RuntimeFailure, detail),
        };
    }

    if err.downcast_ref::<serde_json::Error>().is_some() {
        return AgentRuntimeError::new(
            AgentRuntimeErrorKind::SchemaMismatch,
            scrub_credentials(&err.to_string()),
        );
    }

    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        if reqwest_err.is_timeout() {
            return AgentRuntimeError::new(
                AgentRuntimeErrorKind::Timeout,
                scrub_credentials(&err.to_string()),
            );
        }
        if let Some(status) = reqwest_err.status() {
            return match status.as_u16() {
                401 | 403 => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::AuthFailure,
                    scrub_credentials(&err.to_string()),
                ),
                404 => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::MissingResource,
                    scrub_credentials(&err.to_string()),
                ),
                413 => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::ContextLimitExceeded,
                    scrub_credentials(&err.to_string()),
                ),
                _ => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::RuntimeFailure,
                    scrub_credentials(&err.to_string()),
                ),
            };
        }
    }

    let detail = scrub_credentials(&err.to_string());
    if classify_context_limit_error(&err).is_some() {
        return AgentRuntimeError::new(AgentRuntimeErrorKind::ContextLimitExceeded, detail);
    }

    AgentRuntimeError::new(AgentRuntimeErrorKind::RuntimeFailure, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_error_classifier_uses_typed_io_kinds() {
        let missing = classify_agent_runtime_error(anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing workspace file",
        )));
        assert_eq!(missing.kind, AgentRuntimeErrorKind::MissingResource);

        let denied = classify_agent_runtime_error(anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy denied",
        )));
        assert_eq!(denied.kind, AgentRuntimeErrorKind::PolicyBlocked);
    }

    #[test]
    fn runtime_error_classifier_maps_json_decode_to_schema_mismatch() {
        let error =
            serde_json::from_str::<serde_json::Value>("{").expect_err("malformed JSON should fail");

        let classified = classify_agent_runtime_error(anyhow::Error::new(error));

        assert_eq!(classified.kind, AgentRuntimeErrorKind::SchemaMismatch);
    }

    #[test]
    fn runtime_error_classifier_maps_provider_context_limit() {
        let classified = classify_agent_runtime_error(anyhow::anyhow!(
            "provider error: input exceeds the context window of this model"
        ));

        assert_eq!(classified.kind, AgentRuntimeErrorKind::ContextLimitExceeded);
    }
}
