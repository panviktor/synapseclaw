//! Dialogue state — ephemeral session-scoped working memory.
//!
//! Tracks active entities, comparison sets, and slots within a conversation
//! so short follow-ups ("and the second one?", "restart it") resolve without
//! relying on long-term memory alone.
//!
//! This is NOT long-term memory. It lives in-memory with TTL expiry and is
//! never promoted to the core_memory or episode tables.

use crate::domain::conversation_target::ConversationDeliveryTarget;
use crate::domain::tool_fact::{
    ResourceKind, ResourceOperation, ScheduleAction, ScheduleJobType, ScheduleKind, SearchDomain,
    WorkspaceAction,
};
use serde::{Deserialize, Serialize};

/// Session-scoped dialogue state — the "what are we talking about?" layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialogueState {
    /// Entities currently in focus (city, service, file, branch, etc.).
    pub focus_entities: Vec<FocusEntity>,
    /// When the user compared two things (Berlin vs Tbilisi, staging vs prod).
    pub comparison_set: Vec<FocusEntity>,
    /// Derived typed anchors for short follow-ups ("second", "latest", "current").
    pub reference_anchors: Vec<ReferenceAnchor>,
    /// Structured subjects from the last tool execution.
    pub last_tool_subjects: Vec<String>,
    /// Most recent typed delivery target surfaced by tools.
    pub recent_delivery_target: Option<ConversationDeliveryTarget>,
    /// Most recent typed schedule/job context surfaced by tools.
    pub recent_schedule_job: Option<ScheduleJobReference>,
    /// Most recent typed resource context surfaced by tools.
    pub recent_resource: Option<ResourceReference>,
    /// Most recent typed search context surfaced by tools.
    pub recent_search: Option<SearchReference>,
    /// Most recent typed workspace context surfaced by tools.
    pub recent_workspace: Option<WorkspaceReference>,
    /// Timestamp of last update (unix secs).
    pub updated_at: u64,
}

/// An entity currently in conversational focus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FocusEntity {
    /// What kind of thing: "city", "service", "file", "branch", "person", etc.
    pub kind: String,
    /// The name/value: "Berlin", "synapseclaw.service", "main", etc.
    pub name: String,
    /// Optional extra metadata.
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReferenceOrdinal {
    First,
    Second,
    Third,
    Fourth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReferenceAnchorSelector {
    Current,
    Latest,
    Ordinal(ReferenceOrdinal),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReferenceAnchor {
    /// Which follow-up selector this anchor satisfies.
    pub selector: ReferenceAnchorSelector,
    /// Optional entity kind this anchor belongs to ("city", "service", ...).
    pub entity_kind: Option<String>,
    /// Actual resolved value.
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleJobReference {
    pub job_id: String,
    pub action: ScheduleAction,
    pub job_type: Option<ScheduleJobType>,
    pub schedule_kind: Option<ScheduleKind>,
    pub session_target: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceReference {
    pub kind: ResourceKind,
    pub operation: ResourceOperation,
    pub locator: String,
    pub host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchReference {
    pub domain: SearchDomain,
    pub query: Option<String>,
    pub primary_locator: Option<String>,
    pub result_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceReference {
    pub action: WorkspaceAction,
    pub name: Option<String>,
    pub item_count: Option<usize>,
}

impl DialogueState {
    /// Check if there's a single dominant focus entity.
    pub fn single_focus(&self) -> Option<&FocusEntity> {
        if self.focus_entities.len() == 1 {
            self.focus_entities.first()
        } else {
            None
        }
    }

    /// Check if there's a comparison set (2+ entities of same kind).
    pub fn has_comparison(&self) -> bool {
        self.comparison_set.len() >= 2
    }

    /// Whether the state is stale (older than TTL seconds).
    pub fn is_stale(&self, ttl_secs: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(self.updated_at) > ttl_secs
    }

    /// Clear all state.
    pub fn clear(&mut self) {
        self.focus_entities.clear();
        self.comparison_set.clear();
        self.reference_anchors.clear();
        self.last_tool_subjects.clear();
        self.recent_delivery_target = None;
        self.recent_schedule_job = None;
        self.recent_resource = None;
        self.recent_search = None;
        self.recent_workspace = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_focus_when_one_entity() {
        let state = DialogueState {
            focus_entities: vec![FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            ..Default::default()
        };
        assert!(state.single_focus().is_some());
        assert_eq!(state.single_focus().unwrap().name, "Berlin");
    }

    #[test]
    fn no_single_focus_when_multiple() {
        let state = DialogueState {
            focus_entities: vec![
                FocusEntity {
                    kind: "city".into(),
                    name: "Berlin".into(),
                    metadata: None,
                },
                FocusEntity {
                    kind: "city".into(),
                    name: "Tbilisi".into(),
                    metadata: None,
                },
            ],
            ..Default::default()
        };
        assert!(state.single_focus().is_none());
    }

    #[test]
    fn comparison_set() {
        let state = DialogueState {
            comparison_set: vec![
                FocusEntity {
                    kind: "city".into(),
                    name: "Berlin".into(),
                    metadata: None,
                },
                FocusEntity {
                    kind: "city".into(),
                    name: "Tbilisi".into(),
                    metadata: None,
                },
            ],
            ..Default::default()
        };
        assert!(state.has_comparison());
    }
}
