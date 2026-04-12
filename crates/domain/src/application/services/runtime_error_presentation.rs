pub fn format_context_limit_recovery_response(compacted: bool) -> &'static str {
    if compacted {
        "Context window exceeded. I compacted history and preserved a bounded summary. Please try again."
    } else {
        "Context window exceeded. Start a fresh session or switch to a larger-context route before retrying."
    }
}

pub fn format_timeout_recovery_response() -> &'static str {
    "Request timed out. Try a simpler question or start a fresh session."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limit_response_distinguishes_compaction_outcome() {
        assert!(format_context_limit_recovery_response(true).contains("compacted"));
        assert!(format_context_limit_recovery_response(false).contains("fresh session"));
    }

    #[test]
    fn timeout_response_is_operator_facing() {
        assert!(format_timeout_recovery_response().contains("timed out"));
    }
}
