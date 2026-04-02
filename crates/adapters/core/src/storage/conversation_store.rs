//! Adapter: wraps `ChatDb` behind `ConversationStorePort`.
//!
//! Phase 4.5: ChatDb methods are now async (SurrealDB backend).
//! Maps between synapse_domain domain types and ChatDb row types.

use crate::gateway::chat_db::{ChatDb, ChatMessageRow, ChatSessionRow};
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::domain::conversation::{
    ConversationEvent, ConversationKind, ConversationSession, EventType,
};
use synapse_domain::ports::conversation_store::ConversationStorePort;

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
        id: 0,
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
            .await
            .ok()
            .flatten()
            .map(|r| session_from_row(&r))
    }

    async fn list_sessions(&self, prefix: Option<&str>) -> Vec<ConversationSession> {
        let prefix = prefix.unwrap_or("");
        self.db
            .list_sessions(prefix)
            .await
            .unwrap_or_default()
            .iter()
            .map(session_from_row)
            .collect()
    }

    async fn upsert_session(&self, session: &ConversationSession) -> anyhow::Result<()> {
        let row = session_to_row(session);
        self.db.upsert_session(&row).await
    }

    async fn delete_session(&self, key: &str) -> anyhow::Result<bool> {
        let existed = self.db.get_session(key).await?.is_some();
        self.db.delete_session(key).await?;
        Ok(existed)
    }

    async fn touch_session(&self, key: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.db.touch_session(key, now).await
    }

    async fn append_event(
        &self,
        session_key: &str,
        event: &ConversationEvent,
    ) -> anyhow::Result<()> {
        let row = event_to_row(session_key, event);
        self.db.append_message(&row).await?;
        Ok(())
    }

    async fn get_events(&self, session_key: &str, limit: usize) -> Vec<ConversationEvent> {
        #[allow(clippy::cast_possible_wrap)]
        self.db
            .get_messages(session_key, limit as i64)
            .await
            .unwrap_or_default()
            .iter()
            .map(event_from_row)
            .collect()
    }

    async fn clear_events(&self, session_key: &str) -> anyhow::Result<()> {
        self.db.clear_messages(session_key).await
    }

    async fn update_label(&self, key: &str, label: &str) -> anyhow::Result<()> {
        self.db.update_session_label(key, label).await
    }

    async fn update_goal(&self, key: &str, goal: &str) -> anyhow::Result<()> {
        self.db.update_session_goal(key, goal).await
    }

    async fn increment_message_count(&self, key: &str) -> anyhow::Result<()> {
        self.db.increment_message_count(key).await
    }

    async fn add_token_usage(&self, key: &str, input: i64, output: i64) -> anyhow::Result<()> {
        self.db.add_token_usage(key, input, output).await
    }

    async fn get_summary(&self, key: &str) -> Option<String> {
        self.db
            .get_session(key)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.session_summary)
    }

    async fn set_summary(&self, key: &str, summary: &str) -> anyhow::Result<()> {
        self.db.update_session_summary(key, summary).await
    }
}
