//! Learning signal classification — hot-path vs background learning.
//!
//! Detects when a user message contains an explicit learning signal
//! (remember, correct, prefer, instruct) that should bypass background
//! consolidation and go straight into high-confidence memory mutation.
//!
//! Patterns are loaded from DB (configurable via web UI).
//! Fallback to built-in defaults when DB patterns are empty.

// ── Signal types ─────────────────────────────────────────────────

/// Classification of learning intent in a user message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningSignal {
    /// No explicit learning signal detected. Background consolidation only.
    BackgroundOnly,
    /// User explicitly asked to remember something.
    ExplicitMemory,
    /// User is correcting previous information.
    ExplicitCorrection,
    /// User is stating a preference or instruction.
    ExplicitInstruction,
}

impl LearningSignal {
    pub fn is_explicit(&self) -> bool {
        !matches!(self, Self::BackgroundOnly)
    }

    pub fn confidence(&self) -> f32 {
        match self {
            Self::BackgroundOnly => 0.5,
            Self::ExplicitMemory => 0.95,
            Self::ExplicitCorrection => 0.9,
            Self::ExplicitInstruction => 0.85,
        }
    }

    pub fn from_type_str(s: &str) -> Self {
        match s {
            "correction" => Self::ExplicitCorrection,
            "memory" => Self::ExplicitMemory,
            "instruction" => Self::ExplicitInstruction,
            _ => Self::BackgroundOnly,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BackgroundOnly => "background",
            Self::ExplicitMemory => "memory",
            Self::ExplicitCorrection => "correction",
            Self::ExplicitInstruction => "instruction",
        }
    }
}

// ── Configurable pattern ─────────────────────────────────────────

/// How to match the pattern against the message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Pattern must appear at the start of the message.
    StartsWith,
    /// Pattern can appear anywhere in the message.
    Contains,
}

impl MatchMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "contains" => Self::Contains,
            _ => Self::StartsWith,
        }
    }
}

/// A configurable learning signal pattern, stored in DB.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SignalPattern {
    /// DB record ID.
    #[serde(default)]
    pub id: String,
    /// Which signal type this pattern detects.
    pub signal_type: String,
    /// The text pattern to match (lowercase).
    pub pattern: String,
    /// How to match: starts_with or contains.
    pub match_mode: String,
    /// Language hint (e.g. "en", "ru"). For display only.
    #[serde(default = "default_language")]
    pub language: String,
    /// Whether this pattern is active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_language() -> String {
    "en".into()
}

fn default_enabled() -> bool {
    true
}

// ── Classifier ───────────────────────────────────────────────────

/// Classify a user message using loaded patterns.
///
/// If `patterns` is empty, returns `BackgroundOnly` — no implicit fallback.
/// Caller must load patterns from DB or seed defaults before calling.
/// Priority: correction > memory > instruction (first match wins within group).
pub fn classify_signal_with_patterns(message: &str, patterns: &[SignalPattern]) -> LearningSignal {
    let lower = message.to_lowercase();
    let trimmed = lower.trim();

    if trimmed.len() < 10 || patterns.is_empty() {
        return LearningSignal::BackgroundOnly;
    }

    // Check in priority order: correction > memory > instruction
    for signal_type in &["correction", "memory", "instruction"] {
        for pat in patterns.iter().filter(|p| p.enabled && p.signal_type == *signal_type) {
            let matched = match MatchMode::parse(&pat.match_mode) {
                MatchMode::StartsWith => trimmed.starts_with(&pat.pattern),
                MatchMode::Contains => trimmed.contains(&pat.pattern),
            };
            if matched {
                return LearningSignal::from_type_str(signal_type);
            }
        }
    }

    LearningSignal::BackgroundOnly
}

/// Classify using built-in defaults (for tests and backward compat).
pub fn classify_signal(message: &str) -> LearningSignal {
    classify_signal_with_patterns(message, &default_patterns())
}

// ── Default patterns (seed data) ─────────────────────────────────

/// Get the default patterns for seeding the DB on first boot.
pub fn default_patterns() -> Vec<SignalPattern> {
    build_defaults()
}

fn build_defaults() -> Vec<SignalPattern> {
    let mut patterns = Vec::new();

    // Corrections
    for pat in &[
        "actually,", "actually ", "correction:", "correction ",
        "that's wrong", "that's not right", "that's incorrect",
        "not quite,", "no, ", "wrong,", "wrong.",
        "let me correct", "i was wrong", "i meant ", "i misspoke",
        "you misunderstood",
    ] {
        patterns.push(SignalPattern {
            id: String::new(),
            signal_type: "correction".into(),
            pattern: pat.to_string(),
            match_mode: "starts_with".into(),
            language: "en".into(),
            enabled: true,
        });
    }
    // Correction contains patterns
    for pat in &[
        "that's wrong", "that's not right", "that's incorrect", "let me correct",
    ] {
        patterns.push(SignalPattern {
            id: String::new(),
            signal_type: "correction".into(),
            pattern: pat.to_string(),
            match_mode: "contains".into(),
            language: "en".into(),
            enabled: true,
        });
    }

    // Memory requests
    for pat in &[
        "remember ", "remember:", "remember,", "store:", "save this",
        "save that", "note:", "note that", "take note", "jot down",
        "keep in mind", "don't forget", "important:", "important,",
    ] {
        patterns.push(SignalPattern {
            id: String::new(),
            signal_type: "memory".into(),
            pattern: pat.to_string(),
            match_mode: "starts_with".into(),
            language: "en".into(),
            enabled: true,
        });
    }
    // Russian memory
    for pat in &["запомни", "запомни,"] {
        patterns.push(SignalPattern {
            id: String::new(),
            signal_type: "memory".into(),
            pattern: pat.to_string(),
            match_mode: "starts_with".into(),
            language: "ru".into(),
            enabled: true,
        });
    }

    // Instructions
    for pat in &[
        "i prefer ", "i like ", "i don't like ", "i dislike ", "i hate ",
        "i want ", "i need ", "i always ", "i never ",
        "always ", "never ", "from now on", "going forward",
        "in the future", "my preference is", "my style is",
    ] {
        patterns.push(SignalPattern {
            id: String::new(),
            signal_type: "instruction".into(),
            pattern: pat.to_string(),
            match_mode: "starts_with".into(),
            language: "en".into(),
            enabled: true,
        });
    }
    // Russian instructions
    for pat in &["я предпочитаю", "мне нравится"] {
        patterns.push(SignalPattern {
            id: String::new(),
            signal_type: "instruction".into(),
            pattern: pat.to_string(),
            match_mode: "starts_with".into(),
            language: "ru".into(),
            enabled: true,
        });
    }

    patterns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_messages_are_background() {
        assert_eq!(classify_signal("hello"), LearningSignal::BackgroundOnly);
    }

    #[test]
    fn regular_questions_are_background() {
        assert_eq!(
            classify_signal("What is the capital of France?"),
            LearningSignal::BackgroundOnly
        );
    }

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
    fn remember_explicit() {
        assert_eq!(
            classify_signal("Remember that I prefer Rust over Go"),
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
    fn explicit_signals_are_explicit() {
        assert!(LearningSignal::ExplicitMemory.is_explicit());
        assert!(LearningSignal::ExplicitCorrection.is_explicit());
        assert!(LearningSignal::ExplicitInstruction.is_explicit());
        assert!(!LearningSignal::BackgroundOnly.is_explicit());
    }

    #[test]
    fn correction_has_priority_over_memory() {
        assert_eq!(
            classify_signal("Actually, remember that I use vim not emacs"),
            LearningSignal::ExplicitCorrection
        );
    }

    // ── Custom patterns ──

    #[test]
    fn custom_patterns_override_defaults() {
        let custom = vec![SignalPattern {
            id: String::new(),
            signal_type: "memory".into(),
            pattern: "hey bot, save".into(),
            match_mode: "starts_with".into(),
            language: "en".into(),
            enabled: true,
        }];
        assert_eq!(
            classify_signal_with_patterns("Hey bot, save this fact for later", &custom),
            LearningSignal::ExplicitMemory
        );
        // Default "remember" won't work because custom patterns replace defaults
        assert_eq!(
            classify_signal_with_patterns("Remember my timezone", &custom),
            LearningSignal::BackgroundOnly
        );
    }

    #[test]
    fn disabled_patterns_skipped() {
        let custom = vec![SignalPattern {
            id: String::new(),
            signal_type: "correction".into(),
            pattern: "actually".into(),
            match_mode: "starts_with".into(),
            language: "en".into(),
            enabled: false,
        }];
        assert_eq!(
            classify_signal_with_patterns("Actually this is wrong", &custom),
            LearningSignal::BackgroundOnly
        );
    }
}
