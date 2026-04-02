//! Memory domain types — Phase 4.3: full agent memory architecture.
//!
//! Five memory subsystems:
//! - **Working memory** (core blocks): always-in-prompt biographical/preference data (MemGPT pattern)
//! - **Episodic memory**: raw interaction history, session-scoped
//! - **Semantic memory**: knowledge graph — entities + bitemporal facts
//! - **Procedural memory**: skills learned from pipeline reflections
//! - **Reflective memory**: self-improvement records
//!
//! Previous three-tier model (Phase 4.0) is subsumed:
//! - Tier 1 (working) → WorkingMemory (core blocks)
//! - Tier 2 (session) → EpisodicMemory (session-scoped episodes)
//! - Tier 3 (long-term) → SemanticMemory + SkillMemory

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Identifiers ──────────────────────────────────────────────────

pub type AgentId = String;
pub type MemoryId = String;
pub type SessionId = String;

// ── Memory Category (backward-compatible + extended) ─────────────

/// Memory category — determines storage tier and retrieval scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCategory {
    /// Core facts — persisted long-term, high-value.
    Core,
    /// Daily journal — timestamped summaries, medium retention.
    Daily,
    /// Conversation-scoped — tied to a session, lower retention.
    Conversation,
    /// Entity in the knowledge graph.
    Entity,
    /// Skill / procedure.
    Skill,
    /// Reflection / lesson learned.
    Reflection,
    /// Custom user-defined category.
    Custom(String),
}

impl fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core => write!(f, "core"),
            Self::Daily => write!(f, "daily"),
            Self::Conversation => write!(f, "conversation"),
            Self::Entity => write!(f, "entity"),
            Self::Skill => write!(f, "skill"),
            Self::Reflection => write!(f, "reflection"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

impl MemoryCategory {
    /// Parse from string (case-insensitive).
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "core" => Self::Core,
            "daily" => Self::Daily,
            "conversation" => Self::Conversation,
            "entity" => Self::Entity,
            "skill" => Self::Skill,
            "reflection" => Self::Reflection,
            other => Self::Custom(other.to_string()),
        }
    }
}

impl Serialize for MemoryCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MemoryCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str_lossy(&s))
    }
}

// ── Memory Entry (backward-compatible + extended) ────────────────

/// A recalled memory entry — the universal return type for all memory queries.
#[derive(Clone, Serialize, Deserialize)]
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

// ── Session Memory (backward-compatible) ─────────────────────────

/// Session memory — conversation-scoped durable context.
#[derive(Debug, Clone, Default)]
pub struct SessionMemory {
    pub conversation_key: String,
    pub goal: Option<String>,
    pub summary: Option<String>,
}

// ── Recall Config (backward-compatible) ──────────────────────────

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

// ── Visibility ───────────────────────────────────────────────────

/// Controls who can read a memory entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Only the owning agent.
    #[default]
    Private,
    /// Specific agents by ID.
    SharedWith(Vec<AgentId>),
    /// All agents in the fleet.
    Global,
}

// ── Core Memory Block (MemGPT pattern) ───────────────────────────

/// A named block of text always present in the agent's system prompt.
///
/// Labels: `"persona"`, `"user_knowledge"`, `"task_state"`, `"domain"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreMemoryBlock {
    pub agent_id: AgentId,
    pub label: String,
    pub content: String,
    pub max_tokens: usize,
    #[serde(default = "default_timestamp")]
    pub updated_at: DateTime<Utc>,
}

fn default_timestamp() -> DateTime<Utc> {
    Utc::now()
}

// ── Knowledge Graph: Entity ──────────────────────────────────────

/// A node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: MemoryId,
    pub name: String,
    pub entity_type: String,
    pub properties: serde_json::Value,
    pub summary: Option<String>,
    pub created_by: AgentId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── Knowledge Graph: Temporal Fact ───────────────────────────────

/// An edge in the knowledge graph with bitemporal semantics.
///
/// `valid_to = None` means the fact is current.
/// When a contradicting fact arrives, the old one is invalidated
/// (valid_to set to now).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalFact {
    pub id: MemoryId,
    pub subject: MemoryId,
    pub predicate: String,
    pub object: MemoryId,
    pub confidence: f32,
    /// When the fact became true in the real world.
    pub valid_from: DateTime<Utc>,
    /// When the fact stopped being true (`None` = still current).
    pub valid_to: Option<DateTime<Utc>>,
    /// When we recorded this fact.
    pub recorded_at: DateTime<Utc>,
    /// Episode that sourced this fact (provenance).
    pub source_episode: Option<MemoryId>,
    pub created_by: AgentId,
    /// Pre-computed embedding vector. If provided, add_fact() reuses it
    /// instead of generating a new one (avoids redundant API calls).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
}

// ── Skill (procedural memory) ────────────────────────────────────

/// A learned procedure — created from successful pipeline runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: MemoryId,
    pub name: String,
    pub description: String,
    /// Markdown step-by-step procedure.
    pub content: String,
    pub tags: Vec<String>,
    pub success_count: u32,
    pub fail_count: u32,
    pub version: u32,
    pub created_by: AgentId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Update payload for an existing skill.
#[derive(Debug, Clone)]
pub struct SkillUpdate {
    pub increment_success: bool,
    pub increment_fail: bool,
    pub new_content: Option<String>,
}

// ── Reflection ───────────────────────────────────────────────────

/// A self-improvement record from a pipeline run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub id: MemoryId,
    pub agent_id: AgentId,
    pub pipeline_run: Option<String>,
    pub task_summary: String,
    pub outcome: ReflectionOutcome,
    pub what_worked: String,
    pub what_failed: String,
    pub lesson: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionOutcome {
    Success,
    Partial,
    Failure,
}

impl fmt::Display for ReflectionOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Partial => write!(f, "partial"),
            Self::Failure => write!(f, "failure"),
        }
    }
}

// ── Memory Query ─────────────────────────────────────────────────

/// Unified query for cross-tier memory search.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub text: String,
    pub embedding: Option<Vec<f32>>,
    pub agent_id: AgentId,
    pub include_shared: bool,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub limit: usize,
}

/// A search result with provenance.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub entry: MemoryEntry,
    pub score: f32,
    pub source: SearchSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchSource {
    Vector,
    BM25,
    Graph,
    Hybrid,
}

// ── Hybrid Search Result ─────────────────────────────────────────

/// Combined results from a cross-tier search.
#[derive(Debug, Clone, Default)]
pub struct HybridSearchResult {
    pub episodes: Vec<SearchResult>,
    pub entities: Vec<Entity>,
    pub facts: Vec<TemporalFact>,
    pub skills: Vec<Skill>,
    pub reflections: Vec<Reflection>,
}

// ── Consolidation Report ─────────────────────────────────────────

/// Summary of a consolidation cycle.
#[derive(Debug, Clone, Default)]
pub struct ConsolidationReport {
    pub episodes_processed: u32,
    pub entities_extracted: u32,
    pub facts_created: u32,
    pub facts_invalidated: u32,
    pub skills_generated: u32,
    pub entries_garbage_collected: u32,
}

// ── Memory Event (for IPC broadcast) ─────────────────────────

/// Event broadcast to fleet when memory state changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub event_type: MemoryEventType,
    pub source_agent: AgentId,
    pub entry_id: MemoryId,
    /// Human-readable summary for other agents.
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEventType {
    /// New entity discovered in knowledge graph.
    EntityDiscovered,
    /// New fact established between entities.
    FactEstablished,
    /// Existing fact invalidated (contradicted).
    FactInvalidated,
    /// New skill learned from pipeline reflection.
    SkillLearned,
    /// Existing skill updated (new version).
    SkillUpdated,
    /// New insight generated from reflection.
    InsightGenerated,
}

impl fmt::Display for MemoryEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntityDiscovered => write!(f, "entity_discovered"),
            Self::FactEstablished => write!(f, "fact_established"),
            Self::FactInvalidated => write!(f, "fact_invalidated"),
            Self::SkillLearned => write!(f, "skill_learned"),
            Self::SkillUpdated => write!(f, "skill_updated"),
            Self::InsightGenerated => write!(f, "insight_generated"),
        }
    }
}

// ── Memory Error ─────────────────────────────────────────────────

/// Typed errors for memory operations.
#[derive(Debug)]
pub enum MemoryError {
    Storage(String),
    AccessDenied {
        agent: AgentId,
        action: String,
        resource: String,
    },
    NotFound(String),
    Embedding(String),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(msg) => write!(f, "Storage error: {msg}"),
            Self::AccessDenied {
                agent,
                action,
                resource,
            } => write!(
                f,
                "Access denied: agent {agent} cannot {action} on {resource}"
            ),
            Self::NotFound(msg) => write!(f, "Not found: {msg}"),
            Self::Embedding(msg) => write!(f, "Embedding error: {msg}"),
        }
    }
}

impl std::error::Error for MemoryError {}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_display() {
        assert_eq!(MemoryCategory::Core.to_string(), "core");
        assert_eq!(MemoryCategory::Daily.to_string(), "daily");
        assert_eq!(MemoryCategory::Conversation.to_string(), "conversation");
        assert_eq!(MemoryCategory::Entity.to_string(), "entity");
        assert_eq!(MemoryCategory::Skill.to_string(), "skill");
        assert_eq!(MemoryCategory::Reflection.to_string(), "reflection");
        assert_eq!(
            MemoryCategory::Custom("project".into()).to_string(),
            "project"
        );
    }

    #[test]
    fn category_from_str_lossy() {
        assert_eq!(MemoryCategory::from_str_lossy("Core"), MemoryCategory::Core);
        assert_eq!(
            MemoryCategory::from_str_lossy("SKILL"),
            MemoryCategory::Skill
        );
        assert_eq!(
            MemoryCategory::from_str_lossy("unknown"),
            MemoryCategory::Custom("unknown".into())
        );
    }

    #[test]
    fn category_serde_roundtrip() {
        let categories = vec![
            MemoryCategory::Core,
            MemoryCategory::Entity,
            MemoryCategory::Skill,
            MemoryCategory::Custom("x".into()),
        ];
        for cat in categories {
            let json = serde_json::to_string(&cat).unwrap();
            let back: MemoryCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(cat, back);
        }
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

    #[test]
    fn visibility_default() {
        assert_eq!(Visibility::default(), Visibility::Private);
    }

    #[test]
    fn reflection_outcome_display() {
        assert_eq!(ReflectionOutcome::Success.to_string(), "success");
        assert_eq!(ReflectionOutcome::Failure.to_string(), "failure");
    }

    #[test]
    fn memory_error_display() {
        let e = MemoryError::Storage("disk full".into());
        assert!(e.to_string().contains("disk full"));

        let e = MemoryError::AccessDenied {
            agent: "agent-1".into(),
            action: "read".into(),
            resource: "entity:42".into(),
        };
        assert!(e.to_string().contains("agent-1"));
    }

    #[test]
    fn hybrid_search_result_default() {
        let r = HybridSearchResult::default();
        assert!(r.episodes.is_empty());
        assert!(r.entities.is_empty());
        assert!(r.skills.is_empty());
    }
}
