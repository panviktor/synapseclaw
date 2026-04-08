//! Dialogue state service — session-scoped working memory store.
//!
//! This service is intentionally conservative. It does not infer
//! cities/languages/timezones from free text. Typed state is updated from
//! structured runtime facts such as tool-call arguments and results.

use crate::domain::dialogue_state::FocusEntity;
use crate::domain::dialogue_state::{
    DialogueState, ReferenceAnchor, ReferenceAnchorSelector, ReferenceOrdinal, ResourceReference,
    ScheduleJobReference, SearchReference, WorkspaceReference,
};
use crate::domain::tool_fact::{ToolFactPayload, TypedToolFact};
use parking_lot::RwLock;
use std::collections::HashMap;

/// TTL for dialogue state entries (30 minutes).
const STATE_TTL_SECS: u64 = 1800;

/// In-memory store for dialogue state, keyed by conversation_ref.
pub struct DialogueStateStore {
    states: RwLock<HashMap<String, DialogueState>>,
}

impl DialogueStateStore {
    pub fn new() -> Self {
        Self {
            states: RwLock::new(HashMap::new()),
        }
    }

    /// Get current state for a conversation (None if absent or stale).
    pub fn get(&self, conversation_ref: &str) -> Option<DialogueState> {
        let states = self.states.read();
        states.get(conversation_ref).and_then(|s| {
            if s.is_stale(STATE_TTL_SECS) {
                None
            } else {
                Some(s.clone())
            }
        })
    }

    /// Update state for a conversation.
    pub fn set(&self, conversation_ref: &str, state: DialogueState) {
        let mut states = self.states.write();
        states.insert(conversation_ref.to_string(), state);
    }

    /// Evict stale entries (call periodically).
    pub fn evict_stale(&self) {
        let mut states = self.states.write();
        states.retain(|_, s| !s.is_stale(STATE_TTL_SECS));
    }
}

impl Default for DialogueStateStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Update dialogue state after a user turn.
///
/// This only refreshes timestamps and stores structured subjects when
/// available. It deliberately avoids lexical extraction from user text.
pub fn update_state_from_turn(
    state: &mut DialogueState,
    _user_message: &str,
    tool_facts: &[TypedToolFact],
    _assistant_response: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.updated_at = now;

    if tool_facts.is_empty() {
        return;
    }

    clear_turn_context(state);

    let focus_entities = collect_focus_entities(tool_facts);
    if !focus_entities.is_empty() {
        state.focus_entities = focus_entities.clone();
        state.comparison_set = if focus_entities.len() > 1
            && focus_entities
                .iter()
                .all(|entity| entity.kind == focus_entities[0].kind)
        {
            focus_entities
        } else {
            Vec::new()
        };
    }

    state.reference_anchors =
        derive_reference_anchors(&state.focus_entities, &state.comparison_set);

    let subjects = collect_subjects(tool_facts);
    state.last_tool_subjects = subjects;

    if let Some(target) = collect_recent_delivery_target(tool_facts) {
        state.recent_delivery_target = Some(target);
    }

    if let Some(schedule_job) = collect_recent_schedule_job(tool_facts) {
        state.recent_schedule_job = Some(schedule_job);
    }

    if let Some(resource) = collect_recent_resource(tool_facts) {
        state.recent_resource = Some(resource);
    }

    if let Some(search) = collect_recent_search(tool_facts) {
        state.recent_search = Some(search);
    }

    if let Some(workspace) = collect_recent_workspace(tool_facts) {
        state.recent_workspace = Some(workspace);
    }
}

pub fn should_materialize_state(
    existing: Option<&DialogueState>,
    tool_facts: &[TypedToolFact],
) -> bool {
    existing.is_some() || !tool_facts.is_empty()
}

fn collect_focus_entities(tool_facts: &[TypedToolFact]) -> Vec<FocusEntity> {
    let mut entities = Vec::new();
    for fact in tool_facts {
        for entity in fact.projected_focus_entities() {
            if !entities.iter().any(|existing: &FocusEntity| {
                existing.kind == entity.kind && existing.name == entity.name
            }) {
                entities.push(entity);
            }
        }
    }
    entities
}

fn clear_turn_context(state: &mut DialogueState) {
    state.focus_entities.clear();
    state.comparison_set.clear();
    state.reference_anchors.clear();
    state.last_tool_subjects.clear();
    state.recent_delivery_target = None;
    state.recent_schedule_job = None;
    state.recent_resource = None;
    state.recent_search = None;
    state.recent_workspace = None;
}

fn collect_subjects(tool_facts: &[TypedToolFact]) -> Vec<String> {
    let mut subjects = Vec::new();

    for fact in tool_facts {
        for subject in fact.projected_subjects() {
            if !subjects.iter().any(|existing| existing == &subject) {
                subjects.push(subject);
            }
        }
    }

    subjects
}

fn collect_recent_delivery_target(
    tool_facts: &[TypedToolFact],
) -> Option<crate::domain::conversation_target::ConversationDeliveryTarget> {
    tool_facts
        .iter()
        .rev()
        .find_map(|fact| {
            match &fact.payload {
        ToolFactPayload::Delivery(delivery) => Some(match &delivery.target {
            crate::domain::tool_fact::DeliveryTargetKind::CurrentConversation => {
                crate::domain::conversation_target::ConversationDeliveryTarget::CurrentConversation
            }
            crate::domain::tool_fact::DeliveryTargetKind::Explicit(target)
            | crate::domain::tool_fact::DeliveryTargetKind::ProfileDefault(target) => {
                target.clone()
            }
        }),
        ToolFactPayload::Schedule(schedule) => schedule
            .target
            .as_ref()
            .and_then(|target| target.delivery.clone()),
        _ => None,
    }
        })
}

fn collect_recent_schedule_job(tool_facts: &[TypedToolFact]) -> Option<ScheduleJobReference> {
    tool_facts
        .iter()
        .rev()
        .find_map(|fact| match &fact.payload {
            ToolFactPayload::Schedule(schedule) => {
                schedule.job_id.as_ref().map(|job_id| ScheduleJobReference {
                    job_id: job_id.clone(),
                    action: schedule.action.clone(),
                    job_type: schedule.job_type.clone(),
                    schedule_kind: schedule.schedule_kind.clone(),
                    session_target: schedule
                        .target
                        .as_ref()
                        .and_then(|target| target.session.clone()),
                    timezone: schedule.timezone.clone(),
                })
            }
            _ => None,
        })
}

fn collect_recent_resource(tool_facts: &[TypedToolFact]) -> Option<ResourceReference> {
    tool_facts
        .iter()
        .rev()
        .find_map(|fact| match &fact.payload {
            ToolFactPayload::Resource(resource) => Some(ResourceReference {
                kind: resource.kind.clone(),
                operation: resource.operation.clone(),
                locator: resource.locator.clone(),
                host: resource.host.clone(),
            }),
            _ => None,
        })
}

fn collect_recent_search(tool_facts: &[TypedToolFact]) -> Option<SearchReference> {
    tool_facts
        .iter()
        .rev()
        .find_map(|fact| match &fact.payload {
            ToolFactPayload::Search(search) => Some(SearchReference {
                domain: search.domain.clone(),
                query: search.query.clone(),
                primary_locator: search.primary_locator.clone(),
                result_count: search.result_count,
            }),
            _ => None,
        })
}

fn collect_recent_workspace(tool_facts: &[TypedToolFact]) -> Option<WorkspaceReference> {
    tool_facts
        .iter()
        .rev()
        .find_map(|fact| match &fact.payload {
            ToolFactPayload::Workspace(workspace) => Some(WorkspaceReference {
                action: workspace.action.clone(),
                name: workspace.name.clone(),
                item_count: workspace.item_count,
            }),
            _ => None,
        })
}

fn derive_reference_anchors(
    focus_entities: &[crate::domain::dialogue_state::FocusEntity],
    comparison_set: &[crate::domain::dialogue_state::FocusEntity],
) -> Vec<ReferenceAnchor> {
    let source = if !comparison_set.is_empty() {
        comparison_set
    } else {
        focus_entities
    };
    if source.is_empty() {
        return Vec::new();
    }

    let mut anchors = Vec::new();
    let single_kind = source
        .first()
        .map(|first| source.iter().all(|entity| entity.kind == first.kind))
        .unwrap_or(false);

    if source.len() == 1 {
        let entity = &source[0];
        anchors.push(ReferenceAnchor {
            selector: ReferenceAnchorSelector::Current,
            entity_kind: Some(entity.kind.clone()),
            value: entity.name.clone(),
        });
    } else {
        for (idx, entity) in source.iter().enumerate().take(4) {
            let Some(ordinal) = ordinal_selector(idx) else {
                continue;
            };
            anchors.push(ReferenceAnchor {
                selector: ReferenceAnchorSelector::Ordinal(ordinal),
                entity_kind: if single_kind {
                    Some(entity.kind.clone())
                } else {
                    None
                },
                value: entity.name.clone(),
            });
        }
    }

    if let Some(last) = source.last() {
        anchors.push(ReferenceAnchor {
            selector: ReferenceAnchorSelector::Latest,
            entity_kind: if single_kind || source.len() == 1 {
                Some(last.kind.clone())
            } else {
                None
            },
            value: last.name.clone(),
        });
    }

    anchors
}

fn ordinal_selector(index: usize) -> Option<ReferenceOrdinal> {
    match index {
        0 => Some(ReferenceOrdinal::First),
        1 => Some(ReferenceOrdinal::Second),
        2 => Some(ReferenceOrdinal::Third),
        3 => Some(ReferenceOrdinal::Fourth),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::dialogue_state::{FocusEntity, ReferenceAnchorSelector, ReferenceOrdinal};
    use crate::domain::tool_fact::{
        DeliveryFact, DeliveryTargetKind, ResourceFact, ResourceKind, ResourceMetadata,
        ResourceOperation, ScheduleAction, ScheduleFact, ScheduleJobType, ScheduleKind,
        ScheduleTarget, SearchDomain, SearchFact, ToolFactPayload, TypedToolFact, WorkspaceAction,
        WorkspaceFact,
    };

    #[test]
    fn update_state_keeps_existing_focus_without_lexical_extraction() {
        let mut state = DialogueState::default();
        state.focus_entities.push(FocusEntity {
            kind: "service".into(),
            name: "synapseclaw".into(),
            metadata: None,
        });
        update_state_from_turn(
            &mut state,
            "compare weather in Berlin and Tbilisi",
            &[],
            "Weather in Berlin: 12C. Weather in Tbilisi: 25C.",
        );
        assert_eq!(state.focus_entities.len(), 1);
        assert_eq!(state.focus_entities[0].name, "synapseclaw");
        assert!(state.comparison_set.is_empty());
        assert!(state.reference_anchors.is_empty());
    }

    #[test]
    fn captures_tool_subjects_when_present() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact::focus(
                "weather_lookup",
                vec![
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
                vec!["Berlin".into(), "Tbilisi".into()],
            )],
            "",
        );
        assert_eq!(state.last_tool_subjects, vec!["Berlin", "Tbilisi"]);
        assert_eq!(state.focus_entities.len(), 2);
        assert_eq!(state.comparison_set.len(), 2);
        assert!(state.reference_anchors.iter().any(|anchor| anchor.selector
            == ReferenceAnchorSelector::Ordinal(ReferenceOrdinal::First)
            && anchor.entity_kind.as_deref() == Some("city")
            && anchor.value == "Berlin"));
        assert!(state.reference_anchors.iter().any(|anchor| anchor.selector
            == ReferenceAnchorSelector::Ordinal(ReferenceOrdinal::Second)
            && anchor.entity_kind.as_deref() == Some("city")
            && anchor.value == "Tbilisi"));
        assert!(state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.selector == ReferenceAnchorSelector::Latest
                && anchor.value == "Tbilisi"));
    }

    #[test]
    fn derives_current_focus_slots_for_single_entity() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact::focus(
                "service_status",
                vec![FocusEntity {
                    kind: "service".into(),
                    name: "synapseclaw.service".into(),
                    metadata: None,
                }],
                vec!["synapseclaw.service".into()],
            )],
            "",
        );

        assert!(state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.selector == ReferenceAnchorSelector::Current
                && anchor.entity_kind.as_deref() == Some("service")
                && anchor.value == "synapseclaw.service"));
    }

    #[test]
    fn refreshes_derived_slots_when_focus_shape_changes() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact::focus(
                "weather_lookup",
                vec![
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
                vec!["Berlin".into(), "Tbilisi".into()],
            )],
            "",
        );
        assert!(state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.selector
                == ReferenceAnchorSelector::Ordinal(ReferenceOrdinal::Second)));

        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact::focus(
                "service_status",
                vec![FocusEntity {
                    kind: "service".into(),
                    name: "synapseclaw.service".into(),
                    metadata: None,
                }],
                vec!["synapseclaw.service".into()],
            )],
            "",
        );

        assert!(!state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.selector
                == ReferenceAnchorSelector::Ordinal(ReferenceOrdinal::Second)));
        assert!(state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.selector == ReferenceAnchorSelector::Current
                && anchor.entity_kind.as_deref() == Some("service")
                && anchor.value == "synapseclaw.service"));
    }

    #[test]
    fn materialize_only_when_existing_or_tools_present() {
        assert!(!should_materialize_state(None, &[]));
        assert!(should_materialize_state(
            None,
            &[TypedToolFact::focus("shell", Vec::new(), Vec::new())],
        ));
        assert!(should_materialize_state(
            Some(&DialogueState::default()),
            &[]
        ));
    }

    #[test]
    fn store_get_set() {
        let store = DialogueStateStore::new();
        let mut state = DialogueState::default();
        state.focus_entities.push(FocusEntity {
            kind: "city".into(),
            name: "Moscow".into(),
            metadata: None,
        });
        state.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        store.set("conv1", state);
        let loaded = store.get("conv1");
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().focus_entities[0].name, "Moscow");
    }

    #[test]
    fn captures_recent_delivery_and_schedule_context_from_typed_facts() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[
                TypedToolFact {
                    tool_id: "telegram_post".into(),
                    payload: ToolFactPayload::Delivery(DeliveryFact {
                        target: DeliveryTargetKind::Explicit(
                            ConversationDeliveryTarget::Explicit {
                                channel: "telegram".into(),
                                recipient: "@synapseclaw".into(),
                                thread_ref: None,
                            },
                        ),
                        content_bytes: Some(4),
                    }),
                },
                TypedToolFact {
                    tool_id: "cron_add".into(),
                    payload: ToolFactPayload::Schedule(ScheduleFact {
                        action: ScheduleAction::Create,
                        job_type: Some(ScheduleJobType::Agent),
                        schedule_kind: Some(ScheduleKind::Cron),
                        job_id: Some("job_123".into()),
                        annotation: None,
                        timezone: Some("Europe/Berlin".into()),
                        target: Some(ScheduleTarget {
                            session: Some("main".into()),
                            delivery: None,
                        }),
                        run_count: None,
                        last_status: None,
                        last_duration_ms: None,
                    }),
                },
            ],
            "",
        );

        assert!(matches!(
            state.recent_delivery_target,
            Some(ConversationDeliveryTarget::Explicit { .. })
        ));
        assert_eq!(
            state
                .recent_schedule_job
                .as_ref()
                .map(|job| job.job_id.as_str()),
            Some("job_123")
        );
        assert_eq!(
            state
                .recent_schedule_job
                .as_ref()
                .and_then(|job| job.session_target.as_deref()),
            Some("main")
        );
    }

    #[test]
    fn captures_recent_resource_and_search_context_from_typed_facts() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[
                TypedToolFact {
                    tool_id: "file_read".into(),
                    payload: ToolFactPayload::Resource(ResourceFact {
                        kind: ResourceKind::File,
                        operation: ResourceOperation::Read,
                        locator: "/workspace/README.md".into(),
                        host: None,
                        metadata: ResourceMetadata::default(),
                    }),
                },
                TypedToolFact {
                    tool_id: "session_search".into(),
                    payload: ToolFactPayload::Search(SearchFact {
                        domain: SearchDomain::Session,
                        query: Some("what did we discuss".into()),
                        result_count: Some(3),
                        primary_locator: Some("web:session-123".into()),
                    }),
                },
            ],
            "",
        );

        let resource = state.recent_resource.expect("resource context");
        assert_eq!(resource.kind, ResourceKind::File);
        assert_eq!(resource.operation, ResourceOperation::Read);
        assert_eq!(resource.locator, "/workspace/README.md");

        let search = state.recent_search.expect("search context");
        assert_eq!(search.domain, SearchDomain::Session);
        assert_eq!(search.query.as_deref(), Some("what did we discuss"));
        assert_eq!(search.primary_locator.as_deref(), Some("web:session-123"));
        assert_eq!(search.result_count, Some(3));
    }

    #[test]
    fn captures_recent_workspace_context_from_typed_facts() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact {
                tool_id: "workspace".into(),
                payload: ToolFactPayload::Workspace(WorkspaceFact {
                    action: WorkspaceAction::Switch,
                    name: Some("research-lab".into()),
                    item_count: Some(12),
                }),
            }],
            "",
        );

        let workspace = state.recent_workspace.expect("workspace context");
        assert_eq!(workspace.action, WorkspaceAction::Switch);
        assert_eq!(workspace.name.as_deref(), Some("research-lab"));
        assert_eq!(workspace.item_count, Some(12));
    }

    #[test]
    fn clears_stale_focus_when_new_turn_has_non_focus_typed_context() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact::focus(
                "weather_lookup",
                vec![
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
                vec!["Berlin".into(), "Tbilisi".into()],
            )],
            "",
        );
        assert_eq!(state.focus_entities.len(), 2);
        assert!(!state.reference_anchors.is_empty());

        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact {
                tool_id: "session_search".into(),
                payload: ToolFactPayload::Search(SearchFact {
                    domain: SearchDomain::Session,
                    query: Some("what did we discuss".into()),
                    result_count: Some(3),
                    primary_locator: Some("web:session-123".into()),
                }),
            }],
            "",
        );

        assert!(!state
            .focus_entities
            .iter()
            .any(|entity| entity.name == "Berlin"));
        assert!(!state
            .focus_entities
            .iter()
            .any(|entity| entity.name == "Tbilisi"));
        assert!(state
            .focus_entities
            .iter()
            .any(|entity| entity.name == "web:session-123"));
        assert!(state.comparison_set.is_empty());
        assert!(!state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.value == "Berlin"));
        assert!(!state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.value == "Tbilisi"));
        assert!(state
            .reference_anchors
            .iter()
            .any(|anchor| anchor.value == "web:session-123"));
        assert!(state.recent_search.is_some());
    }

    #[test]
    fn clears_stale_recent_context_when_new_turn_has_other_typed_facts() {
        let mut state = DialogueState::default();
        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact {
                tool_id: "session_search".into(),
                payload: ToolFactPayload::Search(SearchFact {
                    domain: SearchDomain::Session,
                    query: Some("what did we discuss".into()),
                    result_count: Some(3),
                    primary_locator: Some("web:session-123".into()),
                }),
            }],
            "",
        );
        assert!(state.recent_search.is_some());
        assert!(state
            .last_tool_subjects
            .contains(&"what did we discuss".to_string()));
        assert!(state
            .last_tool_subjects
            .contains(&"web:session-123".to_string()));

        update_state_from_turn(
            &mut state,
            "",
            &[TypedToolFact {
                tool_id: "workspace".into(),
                payload: ToolFactPayload::Workspace(WorkspaceFact {
                    action: WorkspaceAction::Switch,
                    name: Some("research-lab".into()),
                    item_count: Some(12),
                }),
            }],
            "",
        );

        assert!(state.recent_search.is_none());
        assert!(state.recent_workspace.is_some());
        assert_eq!(state.last_tool_subjects, vec!["research-lab"]);
    }
}
