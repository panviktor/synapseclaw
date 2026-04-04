//! Side question policy — ephemeral tangent handling.
//!
//! Detects when a user message is a side question (tangent) and
//! provides a lighter processing policy: no todo/standing-order mutation,
//! minimal memory writeback, does not derail main task state.

/// Whether a message is a side question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideQuestionStatus {
    /// Regular message — full processing.
    MainFlow,
    /// Side question — lighter processing.
    SideQuestion,
}

/// Detect if a user message is a side question.
///
/// Heuristic-based: checks for explicit markers and common tangent patterns.
pub fn detect_side_question(message: &str) -> SideQuestionStatus {
    let lower = message.to_lowercase();
    let trimmed = lower.trim();

    // Explicit markers
    let explicit_markers = [
        "btw ",
        "btw,",
        "by the way",
        "quick question",
        "unrelated:",
        "unrelated,",
        "/aside",
        "off topic",
        "off-topic",
        "side note",
        "random question",
        "not related but",
        "кстати",
        "кстати,",
        "между прочим",
    ];

    for marker in &explicit_markers {
        if trimmed.starts_with(marker) || trimmed.contains(marker) {
            return SideQuestionStatus::SideQuestion;
        }
    }

    SideQuestionStatus::MainFlow
}

/// Policy: what to suppress for side questions.
pub struct SideQuestionPolicy {
    /// Skip todo list mutations.
    pub skip_todo_updates: bool,
    /// Skip standing order creation.
    pub skip_standing_orders: bool,
    /// Reduce memory writeback (no consolidation, only if explicit signal).
    pub reduce_memory_writeback: bool,
    /// Don't update dialogue state focus entities.
    pub preserve_focus: bool,
}

impl SideQuestionPolicy {
    pub fn for_side_question() -> Self {
        Self {
            skip_todo_updates: true,
            skip_standing_orders: true,
            reduce_memory_writeback: true,
            preserve_focus: true,
        }
    }

    pub fn for_main_flow() -> Self {
        Self {
            skip_todo_updates: false,
            skip_standing_orders: false,
            reduce_memory_writeback: false,
            preserve_focus: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_btw_detected() {
        assert_eq!(detect_side_question("btw what time is it?"), SideQuestionStatus::SideQuestion);
        assert_eq!(detect_side_question("BTW, unrelated"), SideQuestionStatus::SideQuestion);
    }

    #[test]
    fn by_the_way_detected() {
        assert_eq!(
            detect_side_question("by the way, what's your version?"),
            SideQuestionStatus::SideQuestion
        );
    }

    #[test]
    fn aside_command_detected() {
        assert_eq!(detect_side_question("/aside quick thought"), SideQuestionStatus::SideQuestion);
    }

    #[test]
    fn russian_detected() {
        assert_eq!(detect_side_question("кстати, а который час?"), SideQuestionStatus::SideQuestion);
    }

    #[test]
    fn regular_message_is_main_flow() {
        assert_eq!(detect_side_question("deploy the service"), SideQuestionStatus::MainFlow);
        assert_eq!(detect_side_question("what's the weather?"), SideQuestionStatus::MainFlow);
    }

    #[test]
    fn policy_for_side_question() {
        let p = SideQuestionPolicy::for_side_question();
        assert!(p.skip_todo_updates);
        assert!(p.preserve_focus);
    }
}
