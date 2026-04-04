//! Task intent classification — what kind of request is this?
//!
//! Classifies user messages into high-level intents for planner policy.
//! Used to decide: should the agent act now, schedule, clarify first, etc.

/// High-level intent of a user request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskIntent {
    /// Direct question — answer from knowledge/tools.
    Answer,
    /// Immediate action — do something now.
    ActNow,
    /// Schedule something for later.
    Schedule,
    /// Subscribe to recurring updates.
    Subscribe,
    /// User is asking the agent to clarify something back.
    Clarify,
    /// Tangential side question (not the main task).
    SideQuestion,
}

/// Classify user message intent. Cheap heuristic, no LLM.
pub fn classify_intent(message: &str) -> TaskIntent {
    let lower = message.to_lowercase();
    let trimmed = lower.trim();

    // Schedule patterns
    if trimmed.starts_with("schedule ")
        || trimmed.starts_with("remind me ")
        || trimmed.starts_with("set a reminder")
        || trimmed.contains("every day")
        || trimmed.contains("every morning")
        || trimmed.contains("every hour")
        || trimmed.contains("at 9am")
        || trimmed.contains("tomorrow at")
        || trimmed.contains("in 30 minutes")
    {
        return TaskIntent::Schedule;
    }

    // Subscribe patterns
    if trimmed.starts_with("subscribe")
        || trimmed.starts_with("notify me when")
        || trimmed.starts_with("alert me")
        || trimmed.contains("after restart")
        || trimmed.contains("when it fails")
        || trimmed.contains("standing order")
    {
        return TaskIntent::Subscribe;
    }

    // Side question patterns
    if trimmed.starts_with("btw ")
        || trimmed.starts_with("btw,")
        || trimmed.starts_with("by the way")
        || trimmed.starts_with("quick question")
        || trimmed.starts_with("unrelated:")
        || trimmed.starts_with("/aside")
    {
        return TaskIntent::SideQuestion;
    }

    // ActNow patterns
    if trimmed.starts_with("restart ")
        || trimmed.starts_with("deploy ")
        || trimmed.starts_with("run ")
        || trimmed.starts_with("send ")
        || trimmed.starts_with("create ")
        || trimmed.starts_with("delete ")
        || trimmed.starts_with("update ")
        || trimmed.starts_with("stop ")
        || trimmed.starts_with("start ")
        || trimmed.starts_with("install ")
        || trimmed.starts_with("fix ")
        || trimmed.starts_with("do ")
    {
        return TaskIntent::ActNow;
    }

    // Default: Answer (question or conversation)
    TaskIntent::Answer
}

/// Suggested tool profile for messaging channels based on intent.
///
/// Returns list of preferred tools that should be tried before falling
/// back to low-level shell/file tools.
pub fn preferred_tools_for_intent(intent: &TaskIntent) -> Vec<&'static str> {
    match intent {
        TaskIntent::Answer => vec!["memory_recall", "session_search", "web_search"],
        TaskIntent::ActNow => vec!["shell", "message_send", "todo"],
        TaskIntent::Schedule => vec!["cron_add", "todo"],
        TaskIntent::Subscribe => vec!["standing_order", "cron_add"],
        TaskIntent::Clarify => vec!["clarify"],
        TaskIntent::SideQuestion => vec!["session_search", "memory_recall"],
    }
}

/// Channel-optimized tool profile: high-level tools first, low-level last.
pub const CHANNEL_PREFERRED_TOOLS: &[&str] = &[
    "clarify",
    "todo",
    "message_send",
    "session_search",
    "cron_add",
    "core_memory_update",
    "memory_recall",
    "memory_store",
    "web_search",
    "web_fetch",
    "shell",
    "file_read",
    "file_write",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_detected() {
        assert_eq!(classify_intent("remind me tomorrow at 9am"), TaskIntent::Schedule);
        assert_eq!(classify_intent("Schedule a daily check"), TaskIntent::Schedule);
    }

    #[test]
    fn subscribe_detected() {
        assert_eq!(classify_intent("notify me when it fails"), TaskIntent::Subscribe);
        assert_eq!(classify_intent("after restart, report here"), TaskIntent::Subscribe);
    }

    #[test]
    fn act_now_detected() {
        assert_eq!(classify_intent("restart synapseclaw.service"), TaskIntent::ActNow);
        assert_eq!(classify_intent("deploy the latest build"), TaskIntent::ActNow);
    }

    #[test]
    fn side_question_detected() {
        assert_eq!(classify_intent("btw what time is it?"), TaskIntent::SideQuestion);
        assert_eq!(classify_intent("by the way, unrelated question"), TaskIntent::SideQuestion);
    }

    #[test]
    fn default_is_answer() {
        assert_eq!(classify_intent("what's the weather?"), TaskIntent::Answer);
        assert_eq!(classify_intent("how does memory work?"), TaskIntent::Answer);
    }
}
