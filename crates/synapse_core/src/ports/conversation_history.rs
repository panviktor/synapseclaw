//! Port: conversation history management.
//!
//! Owns the in-memory conversation turns for channel sessions.
//! The adapter wraps the existing `Mutex<HashMap<String, Vec<ChatMessage>>>`.

use crate::domain::message::ChatMessage;

/// Port for managing per-session conversation history (in-memory turns).
pub trait ConversationHistoryPort: Send + Sync {
    /// Check if a conversation key has any prior turns.
    fn has_history(&self, key: &str) -> bool;

    /// Get a clone of the current history for a conversation.
    fn get_history(&self, key: &str) -> Vec<ChatMessage>;

    /// Append a turn to the conversation history.
    /// Implementations should enforce a max history cap.
    fn append_turn(&self, key: &str, turn: ChatMessage);

    /// Clear all history for a conversation key.
    fn clear_history(&self, key: &str);

    /// Compact history — keep only the last N turns, truncate content.
    /// Returns true if compaction happened.
    fn compact_history(&self, key: &str, keep_turns: usize) -> bool;

    /// Remove the last turn if it matches the expected content (rollback).
    fn rollback_last_turn(&self, key: &str, expected_content: &str) -> bool;

    /// Insert a turn at position 0 (e.g. injecting a summary).
    fn prepend_turn(&self, key: &str, turn: ChatMessage);
}
