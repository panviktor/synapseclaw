//! Post-turn learning policy — unified gates for consolidation & reflection.
//!
//! Both web (ws.rs) and channel (handle_inbound_message.rs) paths should
//! call `decide_post_turn()` instead of duplicating gate logic.

use super::learning_signals::LearningSignal;

/// Decision output: what post-turn learning actions to perform.
#[derive(Debug)]
pub struct PostTurnDecision {
    /// Whether to run memory consolidation (daily journal + core facts + entities).
    pub should_consolidate: bool,
    /// Whether to run skill reflection (learn from tool usage / errors).
    pub should_reflect: bool,
    /// Tool names used during this turn (for reflection input).
    pub tools_used: Vec<String>,
    /// Detected learning signal from user message.
    pub signal: LearningSignal,
}

/// Minimum user message length (chars) for consolidation.
/// Aligned with `memory_service::AUTOSAVE_MIN_CHARS`.
const CONSOLIDATE_MIN_CHARS: usize = 20;

/// Minimum user message length (chars) for reflection.
const REFLECT_MIN_USER_CHARS: usize = 30;

/// Minimum response length (bytes) for reflection.
const REFLECT_MIN_RESPONSE_LEN: usize = 200;

/// Decide what post-turn learning actions to perform.
///
/// Unifies the duplicated gate logic from:
/// - `ws.rs:1043-1065` (web path)
/// - `handle_inbound_message.rs:592-633` (channel path)
pub fn decide_post_turn(
    auto_save_enabled: bool,
    user_message: &str,
    assistant_response: &str,
    tools_used: Vec<String>,
) -> PostTurnDecision {
    decide_post_turn_with_patterns(auto_save_enabled, user_message, assistant_response, tools_used, &[])
}

/// Like `decide_post_turn` but uses DB-loaded signal patterns.
pub fn decide_post_turn_with_patterns(
    auto_save_enabled: bool,
    user_message: &str,
    assistant_response: &str,
    tools_used: Vec<String>,
    signal_patterns: &[super::learning_signals::SignalPattern],
) -> PostTurnDecision {
    let signal = super::learning_signals::classify_signal_with_patterns(user_message, signal_patterns);
    let user_chars = user_message.chars().count();

    // Explicit signals use the direct AUDN hot-path — skip background consolidation
    // to avoid duplicate writes. Background consolidation only for non-explicit turns.
    let should_consolidate = if signal.is_explicit() {
        false
    } else {
        auto_save_enabled && user_chars >= CONSOLIDATE_MIN_CHARS
    };

    let resp_lower = assistant_response.to_lowercase();
    let has_errors = resp_lower.contains("error") || resp_lower.contains("failed");
    let should_reflect = assistant_response.len() > REFLECT_MIN_RESPONSE_LEN
        && user_chars >= REFLECT_MIN_USER_CHARS
        && (!tools_used.is_empty() || has_errors);

    let decision = PostTurnDecision {
        should_consolidate,
        should_reflect,
        tools_used,
        signal,
    };

    tracing::debug!(
        target: "post_turn",
        consolidate = decision.should_consolidate,
        reflect = decision.should_reflect,
        tools = decision.tools_used.len(),
        signal = ?decision.signal,
        "Post-turn decision"
    );

    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidation_requires_autosave_and_min_length() {
        let d = decide_post_turn(true, "A sufficiently long message", "", vec![]);
        assert!(d.should_consolidate);

        let d = decide_post_turn(false, "A sufficiently long message", "", vec![]);
        assert!(!d.should_consolidate);

        let d = decide_post_turn(true, "short", "", vec![]);
        assert!(!d.should_consolidate);
    }

    #[test]
    fn reflection_requires_tools_or_errors() {
        let long_response = "x".repeat(300);
        let long_msg = "a long enough user message for reflection to trigger";

        // Has tools → reflect
        let d = decide_post_turn(true, long_msg, &long_response, vec!["shell".into()]);
        assert!(d.should_reflect);
        assert_eq!(d.tools_used, vec!["shell"]);

        // No tools, no errors → no reflect
        let d = decide_post_turn(true, long_msg, &long_response, vec![]);
        assert!(!d.should_reflect);

        // No tools, has error → reflect
        let error_response = format!("{long_response} encountered an error during execution");
        let d = decide_post_turn(true, long_msg, &error_response, vec![]);
        assert!(d.should_reflect);
    }

    #[test]
    fn reflection_requires_min_lengths() {
        // Short response → no reflect
        let d = decide_post_turn(true, "long enough message for reflection", "short", vec!["shell".into()]);
        assert!(!d.should_reflect);

        // Short user message → no reflect
        let d = decide_post_turn(true, "short", &"x".repeat(300), vec!["shell".into()]);
        assert!(!d.should_reflect);
    }

    #[test]
    fn tools_passed_through() {
        let d = decide_post_turn(
            true,
            "test message",
            "response",
            vec!["shell".into(), "file_read".into()],
        );
        assert_eq!(d.tools_used, vec!["shell", "file_read"]);
    }

    /// Parity invariant: web and channels get identical decisions for identical inputs.
    /// Both paths call `decide_post_turn()` — this test documents that contract.
    #[test]
    fn web_and_channel_same_gates() {
        let msg = "A long enough user message for gates";
        let resp = &format!("{} encountered an error", "x".repeat(250));
        let tools = vec!["shell".into(), "browser".into()];

        // Simulate web path inputs
        let web = decide_post_turn(true, msg, resp, tools.clone());
        // Simulate channel path inputs (identical)
        let channel = decide_post_turn(true, msg, resp, tools);

        assert_eq!(web.should_consolidate, channel.should_consolidate);
        assert_eq!(web.should_reflect, channel.should_reflect);
        assert_eq!(web.tools_used, channel.tools_used);
    }

    /// Explicit signals skip consolidation (use direct AUDN hot-path instead).
    #[test]
    fn explicit_signal_skips_consolidation() {
        let pats = super::super::learning_signals::default_patterns();
        let d = decide_post_turn_with_patterns(true, "Remember that I use vim", "", vec![], &pats);
        assert!(!d.should_consolidate, "explicit signals use direct AUDN, not consolidation");
        assert!(d.signal.is_explicit());

        let d = decide_post_turn_with_patterns(true, "What editor?", "", vec![], &pats);
        assert!(!d.should_consolidate);
        assert!(!d.signal.is_explicit());
    }

    /// Signal is classified and attached to decision.
    #[test]
    fn signal_classification_attached() {
        let pats = super::super::learning_signals::default_patterns();
        let d = decide_post_turn_with_patterns(true, "Actually, I prefer Go not Rust", "", vec![], &pats);
        assert_eq!(d.signal, LearningSignal::ExplicitCorrection);

        let d = decide_post_turn_with_patterns(true, "I prefer short answers", "", vec![], &pats);
        assert_eq!(d.signal, LearningSignal::ExplicitInstruction);

        let d = decide_post_turn_with_patterns(true, "How does memory work?", "", vec![], &pats);
        assert_eq!(d.signal, LearningSignal::BackgroundOnly);
    }
}
