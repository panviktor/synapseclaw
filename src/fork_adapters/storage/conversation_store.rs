//! Adapter: wraps existing `ChatDb` behind `ConversationStorePort`.
//!
//! This is NOT a rewrite — it delegates to the existing SQLite backend
//! and maps between fork_core domain types and ChatDb row types.

use crate::fork_adapters::gateway::chat_db::{ChatDb, ChatMessageRow, ChatSessionRow};
use crate::fork_core::domain::conversation::{
    ConversationEvent, ConversationKind, ConversationSession, EventType,
};
use crate::fork_core::ports::conversation_store::ConversationStorePort;
use async_trait::async_trait;
use std::sync::Arc;

/// Wraps `ChatDb` to implement `ConversationStorePort`.
pub struct ChatDbConversationStore {
    db: Arc<ChatDb>,
}

impl ChatDbConversationStore {
    pub fn new(db: Arc<ChatDb>) -> Self {
        Self { db }
    }
}

// ── Mapping helpers ─────────────────────────────────────────────

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn session_from_row(row: &ChatSessionRow) -> ConversationSession {
    let kind = if row.key.starts_with("web:") {
        ConversationKind::Web
    } else if row.key.starts_with("ipc:") {
        ConversationKind::Ipc
    } else {
        ConversationKind::Channel
    };
    ConversationSession {
        key: row.key.clone(),
        kind,
        label: row.label.clone(),
        summary: row.session_summary.clone(),
        current_goal: row.current_goal.clone(),
        created_at: row.created_at as u64,
        last_active: row.last_active as u64,
        message_count: row.message_count as u32,
        input_tokens: row.input_tokens as u64,
        output_tokens: row.output_tokens as u64,
    }
}

fn session_to_row(session: &ConversationSession) -> ChatSessionRow {
    ChatSessionRow {
        key: session.key.clone(),
        label: session.label.clone(),
        current_goal: session.current_goal.clone(),
        session_summary: session.summary.clone(),
        #[allow(clippy::cast_possible_wrap)]
        created_at: session.created_at as i64,
        #[allow(clippy::cast_possible_wrap)]
        last_active: session.last_active as i64,
        message_count: i64::from(session.message_count),
        #[allow(clippy::cast_possible_wrap)]
        input_tokens: session.input_tokens as i64,
        #[allow(clippy::cast_possible_wrap)]
        output_tokens: session.output_tokens as i64,
    }
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn event_from_row(row: &ChatMessageRow) -> ConversationEvent {
    let event_type = match row.kind.as_str() {
        "user" => EventType::User,
        "assistant" => EventType::Assistant,
        "tool_call" => EventType::ToolCall,
        "tool_result" => EventType::ToolResult,
        "error" => EventType::Error,
        "interrupted" => EventType::Interrupted,
        _ => EventType::System,
    };
    ConversationEvent {
        event_type,
        actor: row.role.clone().unwrap_or_else(|| row.kind.clone()),
        content: row.content.clone(),
        tool_name: row.tool_name.clone(),
        run_id: row.run_id.clone(),
        #[allow(clippy::cast_sign_loss)]
        input_tokens: row.input_tokens.map(|t| t as u64),
        #[allow(clippy::cast_sign_loss)]
        output_tokens: row.output_tokens.map(|t| t as u64),
        timestamp: row.timestamp as u64,
    }
}

fn event_to_row(session_key: &str, event: &ConversationEvent) -> ChatMessageRow {
    ChatMessageRow {
        id: 0, // auto-increment
        session_key: session_key.to_string(),
        kind: event.event_type.to_string(),
        role: Some(event.actor.clone()),
        content: event.content.clone(),
        tool_name: event.tool_name.clone(),
        run_id: event.run_id.clone(),
        #[allow(clippy::cast_possible_wrap)]
        input_tokens: event.input_tokens.map(|t| t as i64),
        #[allow(clippy::cast_possible_wrap)]
        output_tokens: event.output_tokens.map(|t| t as i64),
        #[allow(clippy::cast_possible_wrap)]
        timestamp: event.timestamp as i64,
    }
}

// ── Port implementation ─────────────────────────────────────────

#[async_trait]
impl ConversationStorePort for ChatDbConversationStore {
    async fn get_session(&self, key: &str) -> Option<ConversationSession> {
        self.db
            .get_session(key)
            .ok()
            .flatten()
            .map(|r| session_from_row(&r))
    }

    async fn list_sessions(&self, prefix: Option<&str>) -> Vec<ConversationSession> {
        let prefix = prefix.unwrap_or("");
        self.db
            .list_sessions(prefix)
            .unwrap_or_default()
            .iter()
            .map(session_from_row)
            .collect()
    }

    async fn upsert_session(&self, session: &ConversationSession) -> anyhow::Result<()> {
        let row = session_to_row(session);
        self.db.upsert_session(&row)
    }

    async fn delete_session(&self, key: &str) -> anyhow::Result<bool> {
        let existed = self.db.get_session(key)?.is_some();
        self.db.delete_session(key)?;
        Ok(existed)
    }

    async fn touch_session(&self, key: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.db.touch_session(key, now)
    }

    async fn append_event(
        &self,
        session_key: &str,
        event: &ConversationEvent,
    ) -> anyhow::Result<()> {
        let row = event_to_row(session_key, event);
        self.db.append_message(&row)?;
        Ok(())
    }

    async fn get_events(&self, session_key: &str, limit: usize) -> Vec<ConversationEvent> {
        #[allow(clippy::cast_possible_wrap)]
        self.db
            .get_messages(session_key, limit as i64)
            .unwrap_or_default()
            .iter()
            .map(event_from_row)
            .collect()
    }

    async fn clear_events(&self, session_key: &str) -> anyhow::Result<()> {
        self.db.clear_messages(session_key)
    }

    async fn update_label(&self, key: &str, label: &str) -> anyhow::Result<()> {
        self.db.update_session_label(key, label)
    }

    async fn update_goal(&self, key: &str, goal: &str) -> anyhow::Result<()> {
        self.db.update_session_goal(key, goal)
    }

    async fn increment_message_count(&self, key: &str) -> anyhow::Result<()> {
        self.db.increment_message_count(key)
    }

    async fn add_token_usage(&self, key: &str, input: i64, output: i64) -> anyhow::Result<()> {
        self.db.add_token_usage(key, input, output)
    }

    async fn get_summary(&self, key: &str) -> Option<String> {
        self.db
            .get_session(key)
            .ok()
            .flatten()
            .and_then(|r| r.session_summary)
    }

    async fn set_summary(&self, key: &str, summary: &str) -> anyhow::Result<()> {
        self.db.update_session_summary(key, summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (TempDir, ChatDbConversationStore) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test_chat.db");
        let db = Arc::new(ChatDb::open(&db_path).unwrap());
        (tmp, ChatDbConversationStore::new(db))
    }

    #[tokio::test]
    async fn upsert_and_get_session() {
        let (_tmp, store) = make_store();
        let session = ConversationSession {
            key: "web:test:1".into(),
            kind: ConversationKind::Web,
            label: Some("Test".into()),
            summary: None,
            current_goal: None,
            created_at: 1000,
            last_active: 2000,
            message_count: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        store.upsert_session(&session).await.unwrap();
        let loaded = store.get_session("web:test:1").await.unwrap();
        assert_eq!(loaded.key, "web:test:1");
        assert_eq!(loaded.kind, ConversationKind::Web);
        assert_eq!(loaded.label, Some("Test".into()));
    }

    #[tokio::test]
    async fn append_and_get_events() {
        let (_tmp, store) = make_store();
        // Create session first
        let session = ConversationSession {
            key: "web:test:2".into(),
            kind: ConversationKind::Web,
            label: None,
            summary: None,
            current_goal: None,
            created_at: 1000,
            last_active: 2000,
            message_count: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        store.upsert_session(&session).await.unwrap();

        let event = ConversationEvent {
            event_type: EventType::User,
            actor: "user123".into(),
            content: "hello".into(),
            tool_name: None,
            run_id: None,
            input_tokens: None,
            output_tokens: None,
            timestamp: 1000,
        };
        store.append_event("web:test:2", &event).await.unwrap();

        let events = store.get_events("web:test:2", 10).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::User);
        assert_eq!(events[0].content, "hello");
    }

    #[tokio::test]
    async fn list_sessions_with_prefix() {
        let (_tmp, store) = make_store();
        for i in 0..3 {
            store
                .upsert_session(&ConversationSession {
                    key: format!("web:a:{i}"),
                    kind: ConversationKind::Web,
                    label: None,
                    summary: None,
                    current_goal: None,
                    created_at: 1000,
                    #[allow(clippy::cast_sign_loss)]
                    last_active: 2000 + i as u64,
                    message_count: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                })
                .await
                .unwrap();
        }
        store
            .upsert_session(&ConversationSession {
                key: "channel:b:0".into(),
                kind: ConversationKind::Channel,
                label: None,
                summary: None,
                current_goal: None,
                created_at: 1000,
                last_active: 3000,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            })
            .await
            .unwrap();

        let web = store.list_sessions(Some("web:")).await;
        assert_eq!(web.len(), 3);

        let all = store.list_sessions(None).await;
        assert_eq!(all.len(), 4);
    }

    #[tokio::test]
    async fn delete_session_cascades() {
        let (_tmp, store) = make_store();
        let session = ConversationSession {
            key: "web:del:1".into(),
            kind: ConversationKind::Web,
            label: None,
            summary: None,
            current_goal: None,
            created_at: 1000,
            last_active: 2000,
            message_count: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        store.upsert_session(&session).await.unwrap();
        store
            .append_event(
                "web:del:1",
                &ConversationEvent {
                    event_type: EventType::User,
                    actor: "u".into(),
                    content: "msg".into(),
                    tool_name: None,
                    run_id: None,
                    input_tokens: None,
                    output_tokens: None,
                    timestamp: 1000,
                },
            )
            .await
            .unwrap();

        store.delete_session("web:del:1").await.unwrap();
        assert!(store.get_session("web:del:1").await.is_none());
        assert!(store.get_events("web:del:1", 10).await.is_empty());
    }

    #[tokio::test]
    async fn update_label_and_goal() {
        let (_tmp, store) = make_store();
        store
            .upsert_session(&ConversationSession {
                key: "web:lbl:1".into(),
                kind: ConversationKind::Web,
                label: None,
                summary: None,
                current_goal: None,
                created_at: 1000,
                last_active: 2000,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            })
            .await
            .unwrap();

        store.update_label("web:lbl:1", "My Chat").await.unwrap();
        store.update_goal("web:lbl:1", "Fix the bug").await.unwrap();

        let loaded = store.get_session("web:lbl:1").await.unwrap();
        assert_eq!(loaded.label, Some("My Chat".into()));
        assert_eq!(loaded.current_goal, Some("Fix the bug".into()));
    }

    #[tokio::test]
    async fn increment_count_and_add_tokens() {
        let (_tmp, store) = make_store();
        store
            .upsert_session(&ConversationSession {
                key: "web:tok:1".into(),
                kind: ConversationKind::Web,
                label: None,
                summary: None,
                current_goal: None,
                created_at: 1000,
                last_active: 2000,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            })
            .await
            .unwrap();

        store.increment_message_count("web:tok:1").await.unwrap();
        store.increment_message_count("web:tok:1").await.unwrap();
        store.add_token_usage("web:tok:1", 100, 50).await.unwrap();
        store.add_token_usage("web:tok:1", 200, 75).await.unwrap();

        let loaded = store.get_session("web:tok:1").await.unwrap();
        assert_eq!(loaded.message_count, 2);
        assert_eq!(loaded.input_tokens, 300);
        assert_eq!(loaded.output_tokens, 125);
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_false() {
        let (_tmp, store) = make_store();
        let result = store.delete_session("web:nope:1").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn summary_round_trip() {
        let (_tmp, store) = make_store();
        store
            .upsert_session(&ConversationSession {
                key: "web:sum:1".into(),
                kind: ConversationKind::Web,
                label: None,
                summary: None,
                current_goal: None,
                created_at: 1000,
                last_active: 2000,
                message_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            })
            .await
            .unwrap();

        assert!(store.get_summary("web:sum:1").await.is_none());
        store
            .set_summary("web:sum:1", "This is a test summary")
            .await
            .unwrap();
        assert_eq!(
            store.get_summary("web:sum:1").await.unwrap(),
            "This is a test summary"
        );
    }
}
