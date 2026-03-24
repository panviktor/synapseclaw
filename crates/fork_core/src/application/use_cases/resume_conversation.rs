//! Use case: ResumeConversation — restore session state from durable store.
//!
//! Phase 4.0: provides a clean entry point for web chat, channel, and IPC
//! session resumption.
//!
//! Orchestrates:
//! 1. Load session record from ConversationStorePort
//! 2. Fetch recent transcript events
//! 3. Rebuild agent history (user/assistant/system turns only)
//! 4. Restore metadata (tokens, summary, goal)

use crate::domain::conversation::{ConversationSession, EventType};
use crate::domain::message::ChatMessage;
use crate::ports::conversation_store::ConversationStorePort;
use anyhow::Result;

/// Restored session state.
#[derive(Debug, Clone)]
pub struct ResumedSession {
    /// Session metadata (label, summary, goal, counts).
    pub session: ConversationSession,
    /// Rebuilt agent history (user/assistant/system turns only).
    /// Tool calls, results, and errors are excluded — they're UI-only.
    pub transcript: Vec<ChatMessage>,
}

/// Maximum number of transcript events to load on resume.
const MAX_RESUME_EVENTS: usize = 200;

/// Resume a conversation from the durable store.
///
/// Returns the full session state needed to reconstruct an agent:
/// - Session metadata (label, summary, goal, token counts)
/// - Agent-compatible transcript (filtered to user/assistant/system)
pub async fn execute(
    store: &dyn ConversationStorePort,
    session_key: &str,
) -> Result<ResumedSession> {
    // Load session record
    let session = store
        .get_session(session_key)
        .await
        .ok_or_else(|| anyhow::anyhow!("Session '{session_key}' not found"))?;

    // Touch last_active
    let _ = store.touch_session(session_key).await;

    // Fetch recent transcript events
    let events = store.get_events(session_key, MAX_RESUME_EVENTS).await;

    // Rebuild agent history: only user/assistant/system turns
    let transcript = events
        .iter()
        .filter_map(|event| match event.event_type {
            EventType::User => Some(ChatMessage::user(&event.content)),
            EventType::Assistant => Some(ChatMessage::assistant(&event.content)),
            EventType::System => Some(ChatMessage::system(&event.content)),
            _ => None, // ToolCall, ToolResult, Error, Interrupted — UI-only
        })
        .collect();

    Ok(ResumedSession {
        session,
        transcript,
    })
}

/// Resume and optionally update session metadata.
pub async fn execute_with_updates(
    store: &dyn ConversationStorePort,
    session_key: &str,
    new_goal: Option<&str>,
    new_label: Option<&str>,
) -> Result<ResumedSession> {
    let mut result = execute(store, session_key).await?;

    if let Some(goal) = new_goal {
        store.update_goal(session_key, goal).await?;
        result.session.current_goal = Some(goal.to_string());
    }
    if let Some(label) = new_label {
        store.update_label(session_key, label).await?;
        result.session.label = Some(label.to_string());
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::{ConversationEvent, ConversationKind, ConversationSession, EventType};
    use async_trait::async_trait;

    struct MockStore {
        session: Option<ConversationSession>,
        events: Vec<ConversationEvent>,
    }

    impl MockStore {
        fn with_session(session: ConversationSession, events: Vec<ConversationEvent>) -> Self {
            Self {
                session: Some(session),
                events,
            }
        }

        fn empty() -> Self {
            Self {
                session: None,
                events: vec![],
            }
        }
    }

    #[async_trait]
    impl ConversationStorePort for MockStore {
        async fn get_session(&self, _key: &str) -> Option<ConversationSession> {
            self.session.clone()
        }
        async fn upsert_session(&self, _session: &ConversationSession) -> Result<()> { Ok(()) }
        async fn delete_session(&self, _key: &str) -> Result<bool> { Ok(true) }
        async fn list_sessions(&self, _prefix: Option<&str>) -> Vec<ConversationSession> { vec![] }
        async fn touch_session(&self, _key: &str) -> Result<()> { Ok(()) }
        async fn append_event(&self, _key: &str, _event: &ConversationEvent) -> Result<()> { Ok(()) }
        async fn get_events(&self, _key: &str, _limit: usize) -> Vec<ConversationEvent> {
            self.events.clone()
        }
        async fn clear_events(&self, _key: &str) -> Result<()> { Ok(()) }
        async fn update_label(&self, _key: &str, _label: &str) -> Result<()> { Ok(()) }
        async fn update_goal(&self, _key: &str, _goal: &str) -> Result<()> { Ok(()) }
        async fn increment_message_count(&self, _key: &str) -> Result<()> { Ok(()) }
        async fn add_token_usage(&self, _key: &str, _input: i64, _output: i64) -> Result<()> { Ok(()) }
        async fn set_summary(&self, _key: &str, _summary: &str) -> Result<()> { Ok(()) }
        async fn get_summary(&self, _key: &str) -> Option<String> { None }
    }

    fn test_session() -> ConversationSession {
        ConversationSession {
            key: "web:abc:123".into(),
            kind: ConversationKind::Web,
            label: Some("Test session".into()),
            summary: Some("Discussed auth".into()),
            current_goal: Some("Refactor auth".into()),
            created_at: 1000,
            last_active: 2000,
            message_count: 4,
            input_tokens: 1500,
            output_tokens: 800,
        }
    }

    fn test_events() -> Vec<ConversationEvent> {
        vec![
            ConversationEvent {
                event_type: EventType::User,
                actor: "user".into(),
                content: "Analyze auth system".into(),
                tool_name: None,
                run_id: None,
                input_tokens: None,
                output_tokens: None,
                timestamp: 1000,
            },
            ConversationEvent {
                event_type: EventType::Assistant,
                actor: "assistant".into(),
                content: "The auth uses RBAC...".into(),
                tool_name: None,
                run_id: None,
                input_tokens: Some(500),
                output_tokens: Some(200),
                timestamp: 1001,
            },
            ConversationEvent {
                event_type: EventType::ToolCall,
                actor: "assistant".into(),
                content: "shell: grep -r auth".into(),
                tool_name: Some("shell".into()),
                run_id: None,
                input_tokens: None,
                output_tokens: None,
                timestamp: 1002,
            },
            ConversationEvent {
                event_type: EventType::User,
                actor: "user".into(),
                content: "Good, refactor it".into(),
                tool_name: None,
                run_id: None,
                input_tokens: None,
                output_tokens: None,
                timestamp: 1003,
            },
        ]
    }

    #[tokio::test]
    async fn resume_loads_session_and_filters_transcript() {
        let store = MockStore::with_session(test_session(), test_events());

        let result = execute(&store, "web:abc:123").await.unwrap();

        assert_eq!(result.session.key, "web:abc:123");
        assert_eq!(result.session.summary, Some("Discussed auth".into()));

        // Tool call should be filtered out
        assert_eq!(result.transcript.len(), 3);
        assert_eq!(result.transcript[0].role, "user");
        assert_eq!(result.transcript[1].role, "assistant");
        assert_eq!(result.transcript[2].role, "user");
    }

    #[tokio::test]
    async fn resume_unknown_session_fails() {
        let store = MockStore::empty();
        let err = execute(&store, "nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn resume_with_updates_changes_goal() {
        let store = MockStore::with_session(test_session(), test_events());

        let result = execute_with_updates(&store, "web:abc:123", Some("New goal"), None)
            .await
            .unwrap();

        assert_eq!(result.session.current_goal, Some("New goal".into()));
    }

    #[tokio::test]
    async fn resume_empty_transcript() {
        let store = MockStore::with_session(test_session(), vec![]);

        let result = execute(&store, "web:abc:123").await.unwrap();
        assert!(result.transcript.is_empty());
        assert_eq!(result.session.message_count, 4); // metadata preserved
    }
}
