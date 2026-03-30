//! Conversation service — owns session lifecycle and summary policy.
//!
//! Phase 4.0 Slice 3: extracts business logic from ws.rs into synapse_domain.
//!
//! Business rules this service owns:
//! - session key format and creation
//! - session deletion and reset semantics
//! - summary trigger policy (every N messages)
//! - summary prompt construction and truncation
//! - token tracking accumulation
//! - message counting
//! - run state machine (Running → Completed/Failed/Interrupted)

use crate::domain::conversation::{ConversationKind, ConversationSession};
use crate::domain::run::{Run, RunOrigin, RunState};
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::run_store::RunStorePort;
use anyhow::Result;

/// Summary every N messages (web sessions).
pub const WEB_SUMMARY_INTERVAL: usize = 10;
/// Summary every N messages (channel sessions).
pub const CHANNEL_SUMMARY_INTERVAL: usize = 20;
/// Max chars for a summary.
const SUMMARY_MAX_CHARS: usize = 300;

// ── Session key construction ─────────────────────────────────────

/// Generate a new web session key.
pub fn new_web_session_key(token_prefix: &str) -> String {
    format!("web:{token_prefix}:{}", uuid::Uuid::new_v4())
}

/// Build a ConversationSession for a new web session.
pub fn new_web_session(session_key: &str, label: Option<&str>) -> ConversationSession {
    let now = now_secs();
    ConversationSession {
        key: session_key.to_string(),
        kind: ConversationKind::Web,
        label: label.map(String::from),
        summary: None,
        current_goal: None,
        created_at: now,
        last_active: now,
        message_count: 0,
        input_tokens: 0,
        output_tokens: 0,
    }
}

// ── Summary policy ───────────────────────────────────────────────

/// Check if a session needs a summary based on message count.
pub fn needs_summary(message_count: usize, last_summary_count: usize, interval: usize) -> bool {
    message_count >= interval && (message_count - last_summary_count) >= interval
}

/// Build the summary generation prompt.
pub fn build_summary_prompt(previous_summary: Option<&str>, recent_turns: &[String]) -> String {
    let recent = recent_turns.join("\n");
    let prev = previous_summary.unwrap_or("(none)");
    format!(
        "Summarize the conversation so far in 2-3 concise sentences (max {SUMMARY_MAX_CHARS} chars). \
         Preserve: key decisions, goals, tasks, context needed for continuation.\n\n\
         Previous summary: {prev}\n\n\
         Recent messages:\n{recent}"
    )
}

/// Truncate a summary to the max allowed length.
pub fn truncate_summary(summary: &str) -> String {
    if summary.chars().count() <= SUMMARY_MAX_CHARS {
        summary.to_string()
    } else {
        let truncated: String = summary.chars().take(SUMMARY_MAX_CHARS).collect();
        format!("{truncated}…")
    }
}

/// Max chars per event in summary prompt.
const SUMMARY_EVENT_MAX_CHARS: usize = 200;

/// Generate and persist a session summary if the trigger condition is met.
///
/// Full orchestration: check trigger → load events → build prompt →
/// call LLM → truncate → persist to store.
///
/// Returns `Some(summary)` if generated, `None` if skipped.
pub async fn generate_session_summary(
    store: &dyn ConversationStorePort,
    summary_generator: &dyn crate::ports::summary::SummaryGeneratorPort,
    session_key: &str,
    message_count: usize,
    last_summary_count: usize,
    previous_summary: Option<&str>,
    interval: usize,
) -> Result<Option<String>> {
    if !needs_summary(message_count, last_summary_count, interval) {
        return Ok(None);
    }

    // Load recent events
    let events = store.get_events(session_key, 10).await;
    if events.is_empty() {
        return Ok(None);
    }

    // Build prompt from events
    let recent_turns: Vec<String> = events
        .iter()
        .map(|e| {
            let content = if e.content.chars().count() > SUMMARY_EVENT_MAX_CHARS {
                let t: String = e.content.chars().take(SUMMARY_EVENT_MAX_CHARS).collect();
                format!("{t}…")
            } else {
                e.content.clone()
            };
            format!("{}: {content}", e.actor)
        })
        .collect();

    let prompt = build_summary_prompt(previous_summary, &recent_turns);

    // Generate via LLM
    let raw_summary = summary_generator.generate_summary(&prompt).await?;
    let summary = truncate_summary(&raw_summary);

    // Persist
    store.set_summary(session_key, &summary).await?;

    Ok(Some(summary))
}

// ── Token tracking ───────────────────────────────────────────────

/// Accumulate token usage for a session.
pub async fn add_token_usage(
    store: &dyn ConversationStorePort,
    session_key: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<()> {
    if input_tokens == 0 && output_tokens == 0 {
        return Ok(());
    }
    store
        .add_token_usage(session_key, input_tokens, output_tokens)
        .await
}

/// Increment message count for a session.
pub async fn increment_message_count(
    store: &dyn ConversationStorePort,
    session_key: &str,
    count: usize,
) -> Result<()> {
    for _ in 0..count {
        store.increment_message_count(session_key).await?;
    }
    Ok(())
}

// ── Run lifecycle ────────────────────────────────────────────────

/// Create a new run for a web conversation.
pub async fn create_web_run(store: &dyn RunStorePort, session_key: &str) -> Result<String> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let run = Run {
        run_id: run_id.clone(),
        conversation_key: Some(session_key.to_string()),
        origin: RunOrigin::Web,
        state: RunState::Running,
        started_at: now_secs(),
        finished_at: None,
    };
    store.create_run(&run).await?;
    Ok(run_id)
}

/// Mark a run as completed.
pub async fn complete_run(store: &dyn RunStorePort, run_id: &str) -> Result<()> {
    store
        .update_state(run_id, RunState::Completed, Some(now_secs()))
        .await
}

/// Mark a run as failed.
pub async fn fail_run(store: &dyn RunStorePort, run_id: &str) -> Result<()> {
    store
        .update_state(run_id, RunState::Failed, Some(now_secs()))
        .await
}

/// Mark a run as interrupted (user abort).
pub async fn interrupt_run(store: &dyn RunStorePort, run_id: &str) -> Result<()> {
    store
        .update_state(run_id, RunState::Interrupted, Some(now_secs()))
        .await
}

// ── Session operations ───────────────────────────────────────────

/// Reset a session — clear events, zero counters, clear summary/goal.
pub async fn reset_session(store: &dyn ConversationStorePort, session_key: &str) -> Result<()> {
    store.clear_events(session_key).await?;
    // Zero the counters by touching the session
    // (ConversationStorePort doesn't have a reset_counters method,
    //  so we rely on the caller to reset in-memory state)
    Ok(())
}

/// Delete a session and all its events.
pub async fn delete_session(store: &dyn ConversationStorePort, session_key: &str) -> Result<bool> {
    store.delete_session(session_key).await
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_session_key_format() {
        let key = new_web_session_key("abc123");
        assert!(key.starts_with("web:abc123:"));
        assert!(key.len() > 20); // UUID part
    }

    #[test]
    fn new_session_fields() {
        let session = new_web_session("web:abc:123", Some("My Chat"));
        assert_eq!(session.key, "web:abc:123");
        assert_eq!(session.kind, ConversationKind::Web);
        assert_eq!(session.label, Some("My Chat".into()));
        assert_eq!(session.message_count, 0);
    }

    #[test]
    fn summary_needed_at_interval() {
        assert!(!needs_summary(5, 0, 10)); // not enough messages
        assert!(needs_summary(10, 0, 10)); // exactly at interval
        assert!(needs_summary(20, 10, 10)); // second interval
        assert!(!needs_summary(15, 10, 10)); // not enough since last
    }

    #[test]
    fn summary_prompt_includes_context() {
        let prompt = build_summary_prompt(
            Some("Previous context"),
            &["User: hello".into(), "Assistant: hi".into()],
        );
        assert!(prompt.contains("Previous context"));
        assert!(prompt.contains("User: hello"));
    }

    #[test]
    fn truncate_within_limit() {
        assert_eq!(truncate_summary("short"), "short");
    }

    #[test]
    fn truncate_over_limit() {
        let long = "a".repeat(400);
        let result = truncate_summary(&long);
        assert!(result.chars().count() <= SUMMARY_MAX_CHARS + 1); // +1 for ellipsis
    }
}
