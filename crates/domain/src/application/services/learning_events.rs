//! Stable learning event contract — the API consumed by UI and gateway.
//!
//! Emits canonical events from memory operations so the frontend
//! does not need to infer "what the agent learned" from raw row diffs.
//!
//! Read-models aggregate events into UI-friendly summaries.

use crate::domain::memory::{AgentId, MemoryCategory, MemoryEventType, MemoryId};
use crate::domain::memory_mutation::MutationAction;

// ── Canonical learning events ────────────────────────────────────

/// A single learning event emitted from the application layer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LearningEvent {
    /// What happened.
    pub kind: LearningEventKind,
    /// Which agent produced this event.
    pub agent_id: AgentId,
    /// Affected memory entry (if any).
    pub entry_id: Option<MemoryId>,
    /// Human-readable summary.
    pub summary: String,
    /// Timestamp (RFC 3339).
    pub timestamp: String,
}

/// All canonical learning event types.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningEventKind {
    /// New fact/memory added via AUDN.
    MemoryAdded,
    /// Existing memory updated (superseded).
    MemoryUpdated,
    /// Memory deleted (contradiction or retraction).
    MemoryDeleted,
    /// Duplicate detected, no action taken.
    MemoryNoop,
    /// Skill created from reflection.
    SkillCreated,
    /// Skill updated (new version).
    SkillUpdated,
    /// Reflection stored (lesson learned).
    ReflectionStored,
    /// Core memory block updated.
    CoreBlocksUpdated,
    /// Entity discovered in knowledge graph.
    EntityDiscovered,
    /// Visibility changed (namespace promotion).
    VisibilityChanged,
    /// Consolidation cycle completed.
    ConsolidationCompleted,
    /// Prompt optimization applied.
    PromptOptimized,
}

impl LearningEvent {
    /// Create a learning event from a mutation action.
    pub fn from_mutation(
        action: &MutationAction,
        agent_id: &str,
        entry_id: Option<&str>,
        summary: &str,
    ) -> Self {
        let kind = match action {
            MutationAction::Add => LearningEventKind::MemoryAdded,
            MutationAction::Update { .. } => LearningEventKind::MemoryUpdated,
            MutationAction::Delete { .. } => LearningEventKind::MemoryDeleted,
            MutationAction::Noop => LearningEventKind::MemoryNoop,
        };
        Self {
            kind,
            agent_id: agent_id.to_string(),
            entry_id: entry_id.map(String::from),
            summary: summary.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Create a learning event from a memory event type.
    pub fn from_memory_event_type(
        event_type: &MemoryEventType,
        agent_id: &str,
        entry_id: &str,
        summary: &str,
    ) -> Self {
        let kind = match event_type {
            MemoryEventType::EntityDiscovered => LearningEventKind::EntityDiscovered,
            MemoryEventType::FactEstablished => LearningEventKind::MemoryAdded,
            MemoryEventType::FactInvalidated => LearningEventKind::MemoryDeleted,
            MemoryEventType::SkillLearned => LearningEventKind::SkillCreated,
            MemoryEventType::SkillUpdated => LearningEventKind::SkillUpdated,
            MemoryEventType::InsightGenerated => LearningEventKind::ReflectionStored,
            MemoryEventType::VisibilityChanged => LearningEventKind::VisibilityChanged,
        };
        Self {
            kind,
            agent_id: agent_id.to_string(),
            entry_id: Some(entry_id.to_string()),
            summary: summary.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

// ── Read-models ──────────────────────────────────────────────────

/// Per-turn learning summary (what happened during one agent turn).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TurnLearningReport {
    /// Events produced during this turn.
    pub events: Vec<LearningEvent>,
    /// Count of facts added.
    pub facts_added: u32,
    /// Count of facts updated.
    pub facts_updated: u32,
    /// Count of facts deleted (contradicted).
    pub facts_deleted: u32,
    /// Count of noop decisions (duplicates skipped).
    pub noops: u32,
    /// Skills learned or updated.
    pub skills_changed: u32,
    /// Entities discovered.
    pub entities_discovered: u32,
}

impl TurnLearningReport {
    /// Record a learning event and update counters.
    pub fn record(&mut self, event: LearningEvent) {
        match &event.kind {
            LearningEventKind::MemoryAdded => self.facts_added += 1,
            LearningEventKind::MemoryUpdated => self.facts_updated += 1,
            LearningEventKind::MemoryDeleted => self.facts_deleted += 1,
            LearningEventKind::MemoryNoop => self.noops += 1,
            LearningEventKind::SkillCreated | LearningEventKind::SkillUpdated => {
                self.skills_changed += 1;
            }
            LearningEventKind::EntityDiscovered => self.entities_discovered += 1,
            _ => {}
        }
        self.events.push(event);
    }

    /// Whether any learning happened this turn.
    pub fn has_learning(&self) -> bool {
        self.facts_added > 0
            || self.facts_updated > 0
            || self.facts_deleted > 0
            || self.skills_changed > 0
            || self.entities_discovered > 0
    }
}

/// Agent-level learning statistics (aggregated across turns).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AgentLearningStats {
    pub agent_id: AgentId,
    pub total_memories: u64,
    pub memories_by_category: Vec<(String, u64)>,
    pub total_entities: u64,
    pub total_skills: u64,
    pub total_reflections: u64,
}

/// Overview of memory state for dashboard display.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MemoryOverview {
    pub agent_id: AgentId,
    pub total_entries: u64,
    pub core_blocks: u32,
    pub episodic_entries: u64,
    pub daily_entries: u64,
    pub entities: u64,
    pub skills: u64,
    pub reflections: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_report_records_events() {
        let mut report = TurnLearningReport::default();

        report.record(LearningEvent::from_mutation(
            &MutationAction::Add,
            "test-agent",
            Some("entry1"),
            "learned user preference",
        ));
        report.record(LearningEvent::from_mutation(
            &MutationAction::Noop,
            "test-agent",
            None,
            "duplicate skipped",
        ));
        report.record(LearningEvent::from_mutation(
            &MutationAction::Delete {
                target_id: "old".into(),
            },
            "test-agent",
            Some("old"),
            "contradiction resolved",
        ));

        assert_eq!(report.facts_added, 1);
        assert_eq!(report.noops, 1);
        assert_eq!(report.facts_deleted, 1);
        assert_eq!(report.events.len(), 3);
        assert!(report.has_learning());
    }

    #[test]
    fn noop_only_report_has_no_learning() {
        let mut report = TurnLearningReport::default();
        report.record(LearningEvent::from_mutation(
            &MutationAction::Noop,
            "test",
            None,
            "dup",
        ));
        assert!(!report.has_learning());
    }

    #[test]
    fn from_memory_event_type_mapping() {
        let e = LearningEvent::from_memory_event_type(
            &MemoryEventType::SkillLearned,
            "agent-a",
            "skill-1",
            "learned deploy procedure",
        );
        assert_eq!(e.kind, LearningEventKind::SkillCreated);
        assert_eq!(e.agent_id, "agent-a");
    }
}
