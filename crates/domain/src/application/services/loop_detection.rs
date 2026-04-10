//! No-progress loop detection — prevents tool flailing.
//!
//! Detects when the agent keeps calling the same tool with the same args
//! or alternates between no-progress pairs. Triggers early termination
//! or a forced clarification.

/// A single tool invocation record for loop detection.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub args_hash: u64,
    pub progress_hash: Option<u64>,
    pub success: bool,
}

/// Loop detection state for a single agent turn.
#[derive(Debug, Default)]
pub struct LoopDetector {
    history: Vec<ToolInvocation>,
    /// Max consecutive identical calls before triggering.
    pub max_repeats: usize,
    /// Max total tool calls before hard stop.
    pub max_total: usize,
}

/// What the detector recommends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopAction {
    /// Continue normally.
    Continue,
    /// Suggest the agent use clarify instead.
    SuggestClarify,
    /// Force stop — too many calls.
    ForceStop,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            max_repeats: 3,
            max_total: 30,
        }
    }

    /// Record a tool invocation and check for loops.
    pub fn record(&mut self, invocation: ToolInvocation) -> LoopAction {
        self.history.push(invocation);

        // Hard limit
        if self.history.len() >= self.max_total {
            return LoopAction::ForceStop;
        }

        // Check for repeated identical calls
        if self.history.len() >= self.max_repeats {
            let recent = &self.history[self.history.len() - self.max_repeats..];
            let all_same = recent
                .windows(2)
                .all(|w| w[0].tool_name == w[1].tool_name && w[0].args_hash == w[1].args_hash);
            if all_same {
                return LoopAction::SuggestClarify;
            }

            let same_progress = recent.windows(2).all(|w| {
                w[0].tool_name == w[1].tool_name
                    && w[0].success == w[1].success
                    && w[0].progress_hash.is_some()
                    && w[0].progress_hash == w[1].progress_hash
            });
            if same_progress {
                return LoopAction::SuggestClarify;
            }
        }

        // Check for alternating pair (A→B→A→B)
        if self.history.len() >= 4 {
            let h = &self.history;
            let n = h.len();
            if h[n - 1].tool_name == h[n - 3].tool_name
                && h[n - 2].tool_name == h[n - 4].tool_name
                && h[n - 1].args_hash == h[n - 3].args_hash
                && h[n - 2].args_hash == h[n - 4].args_hash
                && !h[n - 1].success
                && !h[n - 3].success
            {
                return LoopAction::SuggestClarify;
            }
        }

        LoopAction::Continue
    }

    /// Reset for a new turn.
    pub fn reset(&mut self) {
        self.history.clear();
    }
}

/// Simple hash for tool args (for comparison, not crypto).
pub fn hash_args(args: &serde_json::Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let s = args.to_string();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

pub fn hash_strings(values: &[String]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for value in values {
        value.hash(&mut hasher);
    }
    Some(hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_loop_initially() {
        let mut d = LoopDetector::new();
        let action = d.record(ToolInvocation {
            tool_name: "shell".into(),
            args_hash: 123,
            progress_hash: None,
            success: true,
        });
        assert_eq!(action, LoopAction::Continue);
    }

    #[test]
    fn repeated_calls_trigger_clarify() {
        let mut d = LoopDetector::new();
        for _ in 0..3 {
            d.record(ToolInvocation {
                tool_name: "shell".into(),
                args_hash: 42,
                progress_hash: None,
                success: false,
            });
        }
        let action = d.record(ToolInvocation {
            tool_name: "shell".into(),
            args_hash: 42,
            progress_hash: None,
            success: false,
        });
        // 4th call, last 3 are identical → SuggestClarify
        assert_eq!(action, LoopAction::SuggestClarify);
    }

    #[test]
    fn max_total_forces_stop() {
        let mut d = LoopDetector::new();
        d.max_total = 5;
        for i in 0..5 {
            d.record(ToolInvocation {
                tool_name: format!("tool_{i}"),
                args_hash: i as u64,
                progress_hash: None,
                success: true,
            });
        }
        assert_eq!(d.history.len(), 5);
    }

    #[test]
    fn hash_args_deterministic() {
        let args = serde_json::json!({"command": "ls -la"});
        let h1 = hash_args(&args);
        let h2 = hash_args(&args);
        assert_eq!(h1, h2);
    }

    #[test]
    fn repeated_progress_signature_triggers_clarify_even_when_args_drift() {
        let mut d = LoopDetector::new();
        let progress_hash = hash_strings(&["anchor-a".to_string(), "anchor-b".to_string()]);
        for idx in 0..3 {
            d.record(ToolInvocation {
                tool_name: "memory_recall".into(),
                args_hash: 100 + idx,
                progress_hash,
                success: true,
            });
        }
        let action = d.record(ToolInvocation {
            tool_name: "memory_recall".into(),
            args_hash: 404,
            progress_hash,
            success: true,
        });

        assert_eq!(action, LoopAction::SuggestClarify);
    }
}
