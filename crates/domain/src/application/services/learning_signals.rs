//! Learning signal classification — hot-path vs background learning.
//!
//! Detects when a user message contains an explicit learning signal
//! (remember, correct, prefer, instruct) that should bypass background
//! consolidation and go straight into high-confidence memory mutation.
//!
//! Design: cheap heuristic-based classification — no LLM call on hot path.

// ── Signal types ─────────────────────────────────────────────────

/// Classification of learning intent in a user message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearningSignal {
    /// No explicit learning signal detected. Background consolidation only.
    BackgroundOnly,
    /// User explicitly asked to remember something.
    /// e.g. "remember that I prefer Rust", "note: my timezone is UTC+3"
    ExplicitMemory,
    /// User is correcting previous information.
    /// e.g. "actually, I use Python not Java", "that's wrong, I meant..."
    ExplicitCorrection,
    /// User is stating a preference or instruction.
    /// e.g. "I prefer short answers", "always use type hints"
    ExplicitInstruction,
}

impl LearningSignal {
    /// Whether this signal should trigger immediate high-confidence capture.
    pub fn is_explicit(&self) -> bool {
        !matches!(self, Self::BackgroundOnly)
    }

    /// Suggested confidence for a mutation candidate from this signal.
    pub fn confidence(&self) -> f32 {
        match self {
            Self::BackgroundOnly => 0.5,
            Self::ExplicitMemory => 0.95,
            Self::ExplicitCorrection => 0.9,
            Self::ExplicitInstruction => 0.85,
        }
    }
}

// ── Classifier ───────────────────────────────────────────────────

/// Classify a user message for explicit learning signals.
///
/// Uses cheap heuristics (no LLM call). Checks in priority order:
/// correction > memory > instruction > background.
pub fn classify_signal(message: &str) -> LearningSignal {
    let lower = message.to_lowercase();
    let trimmed = lower.trim();

    // Skip very short messages — unlikely to be explicit signals
    if trimmed.len() < 10 {
        return LearningSignal::BackgroundOnly;
    }

    // ── Corrections (highest priority) ──
    if is_correction(trimmed) {
        return LearningSignal::ExplicitCorrection;
    }

    // ── Explicit memory requests ──
    if is_memory_request(trimmed) {
        return LearningSignal::ExplicitMemory;
    }

    // ── Preferences / instructions ──
    if is_instruction(trimmed) {
        return LearningSignal::ExplicitInstruction;
    }

    LearningSignal::BackgroundOnly
}

// ── Pattern matchers ─────────────────────────────────────────────

fn is_correction(s: &str) -> bool {
    // Starts with correction markers
    let correction_starts = [
        "actually,",
        "actually ",
        "correction:",
        "correction ",
        "that's wrong",
        "that's not right",
        "that's incorrect",
        "not quite,",
        "no, ",
        "wrong,",
        "wrong.",
        "let me correct",
        "i was wrong",
        "i meant ",
        "i misspoke",
        "you misunderstood",
    ];
    for pat in &correction_starts {
        if s.starts_with(pat) {
            return true;
        }
    }
    // Contains strong correction markers (not at start)
    let correction_contains = [
        "that's wrong",
        "that's not right",
        "that's incorrect",
        "let me correct",
    ];
    for pat in &correction_contains {
        if s.contains(pat) {
            return true;
        }
    }
    false
}

fn is_memory_request(s: &str) -> bool {
    let memory_starts = [
        "remember ",
        "remember:",
        "remember,",
        "store:",
        "save this",
        "save that",
        "note:",
        "note that",
        "take note",
        "jot down",
        "keep in mind",
        "don't forget",
        "запомни",
        "запомни,",
    ];
    for pat in &memory_starts {
        if s.starts_with(pat) {
            return true;
        }
    }
    // "important:" at start signals high-priority info
    if s.starts_with("important:") || s.starts_with("important,") {
        return true;
    }
    false
}

fn is_instruction(s: &str) -> bool {
    let instruction_starts = [
        "i prefer ",
        "i like ",
        "i don't like ",
        "i dislike ",
        "i hate ",
        "i want ",
        "i need ",
        "i always ",
        "i never ",
        "always ",
        "never ",
        "from now on",
        "going forward",
        "in the future",
        "my preference is",
        "my style is",
        "я предпочитаю",
        "мне нравится",
    ];
    for pat in &instruction_starts {
        if s.starts_with(pat) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BackgroundOnly ──

    #[test]
    fn short_messages_are_background() {
        assert_eq!(classify_signal("hello"), LearningSignal::BackgroundOnly);
        assert_eq!(classify_signal("hi"), LearningSignal::BackgroundOnly);
    }

    #[test]
    fn regular_questions_are_background() {
        assert_eq!(
            classify_signal("What is the capital of France?"),
            LearningSignal::BackgroundOnly
        );
        assert_eq!(
            classify_signal("Can you help me write a function?"),
            LearningSignal::BackgroundOnly
        );
    }

    // ── ExplicitCorrection ──

    #[test]
    fn correction_actually() {
        assert_eq!(
            classify_signal("Actually, I use Python not Java"),
            LearningSignal::ExplicitCorrection
        );
    }

    #[test]
    fn correction_thats_wrong() {
        assert_eq!(
            classify_signal("That's wrong, I never said that"),
            LearningSignal::ExplicitCorrection
        );
    }

    #[test]
    fn correction_not_at_start() {
        assert_eq!(
            classify_signal("No wait, that's not right — I use vim"),
            LearningSignal::ExplicitCorrection
        );
    }

    #[test]
    fn correction_i_meant() {
        assert_eq!(
            classify_signal("I meant Python, not JavaScript"),
            LearningSignal::ExplicitCorrection
        );
    }

    // ── ExplicitMemory ──

    #[test]
    fn remember_explicit() {
        assert_eq!(
            classify_signal("Remember that I prefer Rust over Go"),
            LearningSignal::ExplicitMemory
        );
    }

    #[test]
    fn note_explicit() {
        assert_eq!(
            classify_signal("Note: my timezone is UTC+3"),
            LearningSignal::ExplicitMemory
        );
    }

    #[test]
    fn important_explicit() {
        assert_eq!(
            classify_signal("Important: always run tests before committing"),
            LearningSignal::ExplicitMemory
        );
    }

    #[test]
    fn russian_remember() {
        assert_eq!(
            classify_signal("Запомни, что я работаю на macOS"),
            LearningSignal::ExplicitMemory
        );
    }

    // ── ExplicitInstruction ──

    #[test]
    fn preference_i_prefer() {
        assert_eq!(
            classify_signal("I prefer short concise answers"),
            LearningSignal::ExplicitInstruction
        );
    }

    #[test]
    fn instruction_always() {
        assert_eq!(
            classify_signal("Always use type hints in Python code"),
            LearningSignal::ExplicitInstruction
        );
    }

    #[test]
    fn instruction_from_now_on() {
        assert_eq!(
            classify_signal("From now on, write comments in English"),
            LearningSignal::ExplicitInstruction
        );
    }

    #[test]
    fn instruction_never() {
        assert_eq!(
            classify_signal("Never use var in JavaScript"),
            LearningSignal::ExplicitInstruction
        );
    }

    // ── Signal properties ──

    #[test]
    fn explicit_signals_are_explicit() {
        assert!(LearningSignal::ExplicitMemory.is_explicit());
        assert!(LearningSignal::ExplicitCorrection.is_explicit());
        assert!(LearningSignal::ExplicitInstruction.is_explicit());
        assert!(!LearningSignal::BackgroundOnly.is_explicit());
    }

    #[test]
    fn confidence_ordering() {
        assert!(LearningSignal::ExplicitMemory.confidence() > LearningSignal::ExplicitCorrection.confidence());
        assert!(LearningSignal::ExplicitCorrection.confidence() > LearningSignal::ExplicitInstruction.confidence());
        assert!(LearningSignal::ExplicitInstruction.confidence() > LearningSignal::BackgroundOnly.confidence());
    }

    // ── Priority: correction beats memory ──

    #[test]
    fn correction_has_priority_over_memory() {
        // "Actually, remember..." — correction signal wins
        assert_eq!(
            classify_signal("Actually, remember that I use vim not emacs"),
            LearningSignal::ExplicitCorrection
        );
    }
}
