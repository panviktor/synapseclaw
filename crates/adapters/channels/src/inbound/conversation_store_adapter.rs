//! Adapter: wraps any `SessionBackend` as `ConversationStorePort`.
//!
//! This gives channel sessions the same durable session/search surface used by
//! web chat, without introducing a second historical store abstraction.

use crate::session_backend::SessionBackend;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::domain::conversation::{
    ConversationEvent, ConversationKind, ConversationSession, EventType,
};
use synapse_domain::domain::message::ChatMessage;
use synapse_domain::ports::conversation_store::ConversationStorePort;

pub struct SessionBackendConversationStore {
    store: Arc<dyn SessionBackend>,
}

impl SessionBackendConversationStore {
    pub fn new(store: Arc<dyn SessionBackend>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ConversationStorePort for SessionBackendConversationStore {
    async fn get_session(&self, key: &str) -> Option<ConversationSession> {
        let metadata = self
            .store
            .list_sessions_with_metadata()
            .await
            .into_iter()
            .find(|session| session.key == key)?;
        let summary = self.store.load_summary(key).await.map(|s| s.summary);
        Some(session_from_metadata(metadata, summary))
    }

    async fn list_sessions(&self, prefix: Option<&str>) -> Vec<ConversationSession> {
        let prefix = prefix.unwrap_or("");
        let metadata = self.store.list_sessions_with_metadata().await;
        let mut sessions = Vec::new();
        for session in metadata {
            if !prefix.is_empty() && !session.key.starts_with(prefix) {
                continue;
            }
            let summary = self
                .store
                .load_summary(&session.key)
                .await
                .map(|s| s.summary);
            sessions.push(session_from_metadata(session, summary));
        }
        sessions
    }

    async fn upsert_session(&self, session: &ConversationSession) -> anyhow::Result<()> {
        if let Some(summary) = session.summary.as_deref() {
            let summary = crate::session_backend::ChannelSummary {
                summary: summary.to_string(),
                message_count_at_summary: session.message_count as usize,
                updated_at: chrono::Utc::now(),
            };
            self.store.save_summary(&session.key, &summary).await?;
        }
        Ok(())
    }

    async fn delete_session(&self, key: &str) -> anyhow::Result<bool> {
        Ok(self.store.delete(key).await?)
    }

    async fn touch_session(&self, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn append_event(
        &self,
        session_key: &str,
        event: &ConversationEvent,
    ) -> anyhow::Result<()> {
        let message = match event.event_type {
            EventType::User => ChatMessage::user(&event.content),
            EventType::Assistant => ChatMessage::assistant(&event.content),
            EventType::ToolCall | EventType::ToolResult => ChatMessage::tool(&event.content),
            EventType::Error | EventType::Interrupted | EventType::System => {
                ChatMessage::system(&event.content)
            }
        };
        self.store.append(session_key, &message).await?;
        Ok(())
    }

    async fn get_events(&self, session_key: &str, limit: usize) -> Vec<ConversationEvent> {
        let messages = self.store.load(session_key).await;
        let start = messages.len().saturating_sub(limit);
        messages[start..]
            .iter()
            .enumerate()
            .map(|(idx, message)| ConversationEvent {
                event_type: event_type_from_role(&message.role),
                actor: message.role.clone(),
                content: message.content.clone(),
                tool_name: None,
                run_id: None,
                input_tokens: None,
                output_tokens: None,
                timestamp: idx as u64,
            })
            .collect()
    }

    async fn clear_events(&self, session_key: &str) -> anyhow::Result<()> {
        let _ = self.store.delete(session_key).await?;
        Ok(())
    }

    async fn update_label(&self, _key: &str, _label: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn update_goal(&self, _key: &str, _goal: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn increment_message_count(&self, _key: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn add_token_usage(&self, _key: &str, _input: i64, _output: i64) -> anyhow::Result<()> {
        Ok(())
    }

    async fn get_summary(&self, key: &str) -> Option<String> {
        self.store.load_summary(key).await.map(|s| s.summary)
    }

    async fn set_summary(&self, key: &str, summary: &str) -> anyhow::Result<()> {
        let summary = crate::session_backend::ChannelSummary {
            summary: summary.to_string(),
            message_count_at_summary: self.store.load(key).await.len(),
            updated_at: chrono::Utc::now(),
        };
        self.store.save_summary(key, &summary).await?;
        Ok(())
    }
}

fn session_from_metadata(
    metadata: crate::session_backend::SessionMetadata,
    summary: Option<String>,
) -> ConversationSession {
    ConversationSession {
        key: metadata.key,
        kind: ConversationKind::Channel,
        label: None,
        summary,
        current_goal: None,
        created_at: metadata.created_at.timestamp().max(0) as u64,
        last_active: metadata.last_activity.timestamp().max(0) as u64,
        message_count: metadata.message_count as u32,
        input_tokens: 0,
        output_tokens: 0,
    }
}

fn event_type_from_role(role: &str) -> EventType {
    match role {
        "user" => EventType::User,
        "assistant" => EventType::Assistant,
        "tool" => EventType::ToolResult,
        "system" => EventType::System,
        _ => EventType::System,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::SessionStore;
    use tempfile::TempDir;

    #[tokio::test]
    async fn exposes_session_store_as_conversation_store() {
        let tmp = TempDir::new().unwrap();
        let backend = Arc::new(SessionStore::new(tmp.path()).unwrap()) as Arc<dyn SessionBackend>;
        backend
            .append("matrix_room_alice", &ChatMessage::user("weather in Berlin"))
            .await
            .unwrap();
        backend
            .append(
                "matrix_room_alice",
                &ChatMessage::assistant("12C and cloudy"),
            )
            .await
            .unwrap();

        let store = SessionBackendConversationStore::new(Arc::clone(&backend));
        let sessions = store.list_sessions(None).await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].key, "matrix_room_alice");

        let events = store.get_events("matrix_room_alice", 10).await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::User);
        assert_eq!(events[1].event_type, EventType::Assistant);
    }
}
