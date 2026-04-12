use anyhow::Error;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextLimitObservation {
    pub observed_context_window_tokens: Option<usize>,
    pub requested_context_tokens: Option<usize>,
}

// Provider error payloads are not structured consistently; keep textual markers
// at the adapter boundary instead of leaking them into routing/domain services.
const CONTEXT_LIMIT_MESSAGE_MARKERS: &[&str] = &[
    "exceeds the context window",
    "context window of this model",
    "maximum context length",
    "context length exceeded",
    "too many tokens",
    "token limit exceeded",
    "prompt is too long",
    "input is too long",
];

pub fn classify_context_limit_error(err: &Error) -> Option<ContextLimitObservation> {
    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        if reqwest_err
            .status()
            .is_some_and(|status| status.as_u16() == 413)
        {
            return Some(ContextLimitObservation::default());
        }
    }

    let lower = err.to_string().to_lowercase();
    CONTEXT_LIMIT_MESSAGE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
        .then_some(ContextLimitObservation::default())
}

pub fn is_context_window_exceeded(err: &Error) -> bool {
    classify_context_limit_error(err).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_provider_context_limit_messages() {
        let err = anyhow::anyhow!(
            "OpenAI Codex stream error: Your input exceeds the context window of this model."
        );

        assert_eq!(
            classify_context_limit_error(&err),
            Some(ContextLimitObservation::default())
        );
        assert!(is_context_window_exceeded(&err));
    }

    #[test]
    fn rejects_unrelated_provider_messages() {
        let err = anyhow::anyhow!("upstream model overloaded, try again later");

        assert_eq!(classify_context_limit_error(&err), None);
        assert!(!is_context_window_exceeded(&err));
    }
}
