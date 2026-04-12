use anyhow::Error;

pub use synapse_domain::ports::model_profile_catalog::ContextLimitProfileObservation as ContextLimitObservation;

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

    let message = err.to_string();
    let lower = message.to_lowercase();
    CONTEXT_LIMIT_MESSAGE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
        .then(|| parse_context_limit_observation(&lower))
}

pub fn is_context_window_exceeded(err: &Error) -> bool {
    classify_context_limit_error(err).is_some()
}

fn parse_context_limit_observation(lower: &str) -> ContextLimitObservation {
    ContextLimitObservation {
        observed_context_window_tokens: extract_observed_context_window_tokens(lower),
        requested_context_tokens: extract_requested_context_tokens(lower),
    }
}

fn extract_observed_context_window_tokens(lower: &str) -> Option<usize> {
    const MAX_CONTEXT_MARKERS: &[&str] = &[
        "maximum context length is",
        "max context length is",
        "maximum context length:",
        "max context length:",
        "context window is",
        "context window of",
        "context length limit is",
        "context limit is",
        "supports up to",
    ];

    MAX_CONTEXT_MARKERS
        .iter()
        .find_map(|marker| first_number_after(lower, marker))
        .or_else(|| number_after_last_greater_than_before_marker(lower, "maximum"))
}

fn extract_requested_context_tokens(lower: &str) -> Option<usize> {
    const REQUESTED_CONTEXT_MARKERS: &[&str] = &[
        "you requested",
        "requested",
        "but got",
        "got",
        "resulted in",
    ];

    REQUESTED_CONTEXT_MARKERS
        .iter()
        .find_map(|marker| first_number_after(lower, marker))
}

fn first_number_after(text: &str, marker: &str) -> Option<usize> {
    let start = text.find(marker)? + marker.len();
    first_ascii_number(&text[start..])
}

fn number_after_last_greater_than_before_marker(text: &str, marker: &str) -> Option<usize> {
    let marker_index = text.find(marker)?;
    let before_marker = &text[..marker_index];
    let greater_than_index = before_marker.rfind('>')?;
    first_ascii_number(&before_marker[greater_than_index + 1..])
}

fn first_ascii_number(text: &str) -> Option<usize> {
    let mut digits = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            started = true;
            continue;
        }
        if started && matches!(ch, ',' | '_') {
            continue;
        }
        if started {
            break;
        }
    }
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
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

    #[test]
    fn extracts_openai_style_context_limit_tokens() {
        let err = anyhow::anyhow!(
            "OpenAI API error (400 Bad Request): This model's maximum context length is 128,000 tokens. However, you requested 140,512 tokens."
        );

        assert_eq!(
            classify_context_limit_error(&err),
            Some(ContextLimitObservation {
                observed_context_window_tokens: Some(128_000),
                requested_context_tokens: Some(140_512),
            })
        );
    }

    #[test]
    fn extracts_anthropic_style_context_limit_tokens() {
        let err = anyhow::anyhow!(
            "Anthropic API error: prompt is too long: 213747 tokens > 200000 maximum"
        );

        assert_eq!(
            classify_context_limit_error(&err),
            Some(ContextLimitObservation {
                observed_context_window_tokens: Some(200_000),
                requested_context_tokens: None,
            })
        );
    }
}
