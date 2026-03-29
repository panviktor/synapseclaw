//! Memory domain types — explicit three-tier memory model.
//!
//! Phase 4.0 Slice 6: makes memory tiers first-class domain objects.
//!
//! Tier 1: Working memory — in-run transient context (RunContext)
//! Tier 2: Session memory — conversation-scoped durable state
//! Tier 3: Long-term memory — cross-session, cross-agent knowledge

use std::fmt;

/// Memory category — determines storage tier and retrieval scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCategory {
    /// Core facts — persisted long-term, high-value.
    Core,
    /// Daily journal — timestamped summaries, medium retention.
    Daily,
    /// Conversation-scoped — tied to a session, lower retention.
    Conversation,
    /// Custom user-defined category.
    Custom(String),
}

impl fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

impl serde::Serialize for MemoryCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for MemoryCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "core" => Self::Core,
            "daily" => Self::Daily,
            "conversation" => Self::Conversation,
            _ => Self::Custom(s),
        })
    }
}

/// A recalled memory entry.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub key: String,
    pub content: String,
    pub category: MemoryCategory,
    pub timestamp: String,
    pub session_id: Option<String>,
    pub score: Option<f64>,
}

impl fmt::Debug for MemoryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryEntry")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("content", &self.content)
            .field("category", &self.category)
            .field("timestamp", &self.timestamp)
            .field("score", &self.score)
            .finish_non_exhaustive()
    }
}

/// Session memory — conversation-scoped durable context.
///
/// Stored via ConversationStorePort (summary, goal) and MemoryTiersPort
/// (session-scoped recall entries).
#[derive(Debug, Clone, Default)]
pub struct SessionMemory {
    pub conversation_key: String,
    pub goal: Option<String>,
    pub summary: Option<String>,
}

/// Memory recall configuration.
#[derive(Debug, Clone)]
pub struct RecallConfig {
    pub max_entries: usize,
    pub entry_max_chars: usize,
    pub total_max_chars: usize,
    pub min_relevance_score: f64,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            max_entries: 4,
            entry_max_chars: 800,
            total_max_chars: 4_000,
            min_relevance_score: 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_display() {
        assert_eq!(MemoryCategory::Core.to_string(), "core");
        assert_eq!(MemoryCategory::Daily.to_string(), "daily");
        assert_eq!(MemoryCategory::Conversation.to_string(), "conversation");
        assert_eq!(
            MemoryCategory::Custom("project".into()).to_string(),
            "project"
        );
    }

    #[test]
    fn recall_config_defaults() {
        let cfg = RecallConfig::default();
        assert_eq!(cfg.max_entries, 4);
        assert_eq!(cfg.min_relevance_score, 0.5);
    }

    #[test]
    fn session_memory_default() {
        let sm = SessionMemory::default();
        assert!(sm.goal.is_none());
        assert!(sm.summary.is_none());
    }
}
