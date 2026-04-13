use synapse_domain::ports::agent_runtime::{AgentRuntimeError, AgentRuntimeErrorKind};
use synapse_providers::error_classification::classify_context_limit_error;
use synapse_providers::ProviderCapabilityError;
use synapse_security::scrub_credentials;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeErrorClassKind {
    CapabilityMismatch,
    AuthFailure,
    PolicyBlocked,
    MissingResource,
    Timeout,
    SchemaMismatch,
    ContextLimitExceeded,
    RuntimeFailure,
}

pub(crate) struct RuntimeErrorClass<'a> {
    pub kind: RuntimeErrorClassKind,
    pub capability_error: Option<&'a ProviderCapabilityError>,
    pub detail: String,
}

pub(crate) fn classify_runtime_error(error: &anyhow::Error) -> RuntimeErrorClass<'_> {
    let detail = scrub_credentials(&error.to_string());

    if let Some(capability_error) = error.downcast_ref::<ProviderCapabilityError>() {
        return RuntimeErrorClass {
            kind: RuntimeErrorClassKind::CapabilityMismatch,
            capability_error: Some(capability_error),
            detail,
        };
    }

    if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
        let kind = match io_error.kind() {
            std::io::ErrorKind::PermissionDenied => RuntimeErrorClassKind::PolicyBlocked,
            std::io::ErrorKind::NotFound => RuntimeErrorClassKind::MissingResource,
            std::io::ErrorKind::TimedOut => RuntimeErrorClassKind::Timeout,
            _ => RuntimeErrorClassKind::RuntimeFailure,
        };
        return RuntimeErrorClass {
            kind,
            capability_error: None,
            detail,
        };
    }

    if error.downcast_ref::<serde_json::Error>().is_some() {
        return RuntimeErrorClass {
            kind: RuntimeErrorClassKind::SchemaMismatch,
            capability_error: None,
            detail,
        };
    }

    if let Some(reqwest_error) = error.downcast_ref::<reqwest::Error>() {
        if reqwest_error.is_timeout() {
            return RuntimeErrorClass {
                kind: RuntimeErrorClassKind::Timeout,
                capability_error: None,
                detail,
            };
        }
        if let Some(status) = reqwest_error.status() {
            let kind = match status.as_u16() {
                401 | 403 => RuntimeErrorClassKind::AuthFailure,
                404 => RuntimeErrorClassKind::MissingResource,
                413 => RuntimeErrorClassKind::ContextLimitExceeded,
                _ => RuntimeErrorClassKind::RuntimeFailure,
            };
            return RuntimeErrorClass {
                kind,
                capability_error: None,
                detail,
            };
        }
    }

    if classify_context_limit_error(error).is_some() {
        return RuntimeErrorClass {
            kind: RuntimeErrorClassKind::ContextLimitExceeded,
            capability_error: None,
            detail,
        };
    }

    RuntimeErrorClass {
        kind: RuntimeErrorClassKind::RuntimeFailure,
        capability_error: None,
        detail,
    }
}

pub(crate) fn classify_agent_runtime_error(err: anyhow::Error) -> AgentRuntimeError {
    let class = classify_runtime_error(&err);
    AgentRuntimeError::new(agent_runtime_error_kind(class.kind), class.detail)
}

fn agent_runtime_error_kind(kind: RuntimeErrorClassKind) -> AgentRuntimeErrorKind {
    match kind {
        RuntimeErrorClassKind::CapabilityMismatch => AgentRuntimeErrorKind::CapabilityMismatch,
        RuntimeErrorClassKind::AuthFailure => AgentRuntimeErrorKind::AuthFailure,
        RuntimeErrorClassKind::PolicyBlocked => AgentRuntimeErrorKind::PolicyBlocked,
        RuntimeErrorClassKind::MissingResource => AgentRuntimeErrorKind::MissingResource,
        RuntimeErrorClassKind::Timeout => AgentRuntimeErrorKind::Timeout,
        RuntimeErrorClassKind::SchemaMismatch => AgentRuntimeErrorKind::SchemaMismatch,
        RuntimeErrorClassKind::ContextLimitExceeded => AgentRuntimeErrorKind::ContextLimitExceeded,
        RuntimeErrorClassKind::RuntimeFailure => AgentRuntimeErrorKind::RuntimeFailure,
    }
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

    #[test]
    fn common_classifier_preserves_context_limit_kind() {
        let error = anyhow::anyhow!("provider error: prompt is too long");
        let classified = classify_runtime_error(&error);

        assert_eq!(classified.kind, RuntimeErrorClassKind::ContextLimitExceeded);
    }
}
