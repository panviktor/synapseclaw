//! Turn interpretation — bounded typed interpretation for a single turn.
//!
//! This layer is intentionally narrow. It combines structured runtime facts
//! without turning into a phrase-engine.

use crate::application::services::turn_model_routing::infer_turn_capability_requirement;
use crate::application::services::user_profile_service::format_profile_projection;
use crate::domain::conversation_target::{ConversationDeliveryTarget, CurrentConversationContext};
use crate::domain::dialogue_state::{
    DialogueState, ReferenceAnchor, ReferenceAnchorSelector, ReferenceOrdinal, ResourceReference,
    ScheduleJobReference, SearchReference, WorkspaceReference,
};
use crate::domain::tool_fact::{ResourceKind, SearchDomain, WorkspaceAction};
use crate::domain::user_profile::UserProfile;
use crate::ports::memory::UnifiedMemoryPort;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnInterpretation {
    pub user_profile: Option<UserProfile>,
    pub current_conversation: Option<CurrentConversationSnapshot>,
    pub dialogue_state: Option<DialogueStateSnapshot>,
    pub configured_delivery_target: Option<ConversationDeliveryTarget>,
    pub reference_candidates: Vec<ReferenceCandidate>,
    pub clarification_candidates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentConversationSnapshot {
    pub adapter: String,
    pub has_thread: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueStateSnapshot {
    pub focus_entities: Vec<(String, String)>,
    pub comparison_set: Vec<(String, String)>,
    pub reference_anchors: Vec<ReferenceAnchor>,
    pub last_tool_subjects: Vec<String>,
    pub recent_delivery_target: Option<ConversationDeliveryTarget>,
    pub recent_schedule_job: Option<ScheduleJobReference>,
    pub recent_resource: Option<ResourceReference>,
    pub recent_search: Option<SearchReference>,
    pub recent_workspace: Option<WorkspaceReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceSource {
    ConfiguredRuntime,
    DialogueState,
    UserProfile,
    CurrentConversation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceCandidateKind {
    Entity {
        entity_kind: String,
    },
    Anchor {
        selector: ReferenceAnchorSelector,
        entity_kind: Option<String>,
    },
    Profile {
        key: String,
    },
    DeliveryTarget,
    ScheduleJob,
    ResourceLocator {
        resource_kind: ResourceKind,
    },
    SearchQuery {
        domain: SearchDomain,
    },
    SearchResult {
        domain: SearchDomain,
    },
    WorkspaceName {
        action: WorkspaceAction,
    },
    RecentSubject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceCandidate {
    pub kind: ReferenceCandidateKind,
    pub value: String,
    pub source: ReferenceSource,
}

pub async fn build_turn_interpretation(
    _memory: Option<&dyn UnifiedMemoryPort>,
    _user_message: &str,
    profile: Option<UserProfile>,
    current_conversation: Option<&CurrentConversationContext>,
    dialogue_state: Option<&DialogueState>,
    configured_delivery_target: Option<ConversationDeliveryTarget>,
) -> Option<TurnInterpretation> {
    let user_profile = profile.filter(|profile| !profile.is_empty());
    let current_conversation = current_conversation.map(|ctx| CurrentConversationSnapshot {
        adapter: ctx.source_adapter.clone(),
        has_thread: ctx.thread_ref.is_some(),
    });
    let dialogue_state = dialogue_state.and_then(snapshot_dialogue_state);

    let reference_candidates = collect_reference_candidates(
        user_profile.as_ref(),
        current_conversation.as_ref(),
        dialogue_state.as_ref(),
        configured_delivery_target.as_ref(),
    );
    let clarification_candidates =
        collect_clarification_candidates(dialogue_state.as_ref(), user_profile.as_ref());

    let interpretation = TurnInterpretation {
        user_profile,
        current_conversation,
        dialogue_state,
        configured_delivery_target,
        reference_candidates,
        clarification_candidates,
    };

    if interpretation.user_profile.is_none()
        && interpretation.current_conversation.is_none()
        && interpretation.dialogue_state.is_none()
        && interpretation.reference_candidates.is_empty()
        && interpretation.clarification_candidates.is_empty()
    {
        None
    } else {
        Some(interpretation)
    }
}

pub fn format_turn_interpretation(interpretation: &TurnInterpretation) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(profile) = &interpretation.user_profile {
        lines.push("[user-profile]".to_string());
        lines.extend(
            format_profile_projection(profile)
                .lines()
                .map(str::to_string),
        );
    }

    if let Some(conversation) = &interpretation.current_conversation {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[current-conversation]".to_string());
        lines.push(format!("- adapter: {}", conversation.adapter));
        lines.push("- reply_here_available: true".to_string());
        lines.push(format!("- threaded_reply: {}", conversation.has_thread));
    }

    if let Some(target) = &interpretation.configured_delivery_target {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[configured-runtime]".to_string());
        lines.push(format!(
            "- configured_delivery_target: {}",
            format_delivery_target(target)
        ));
    }

    if let Some(state) = &interpretation.dialogue_state {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[working-state]".to_string());
        if !state.focus_entities.is_empty() {
            lines.push(format!(
                "- focus_entities: {}",
                format_pairs(&state.focus_entities)
            ));
        }
        if !state.comparison_set.is_empty() {
            lines.push(format!(
                "- comparison_set: {}",
                format_pairs(&state.comparison_set)
            ));
        }
        if !state.reference_anchors.is_empty() {
            lines.push(format!(
                "- reference_anchors: {}",
                state
                    .reference_anchors
                    .iter()
                    .map(format_reference_anchor)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !state.last_tool_subjects.is_empty() {
            lines.push(format!(
                "- last_tool_subjects: {}",
                state.last_tool_subjects.join(", ")
            ));
        }
        if let Some(target) = &state.recent_delivery_target {
            lines.push(format!(
                "- recent_delivery_target: {}",
                format_delivery_target(target)
            ));
        }
        if let Some(schedule_job) = &state.recent_schedule_job {
            lines.push(format!(
                "- recent_schedule_job: {}",
                format_schedule_job(schedule_job)
            ));
        }
        if let Some(resource) = &state.recent_resource {
            lines.push(format!(
                "- recent_resource: {}",
                format_resource_reference(resource)
            ));
        }
        if let Some(search) = &state.recent_search {
            lines.push(format!(
                "- recent_search: {}",
                format_search_reference(search)
            ));
        }
        if let Some(workspace) = &state.recent_workspace {
            lines.push(format!(
                "- recent_workspace: {}",
                format_workspace_reference(workspace)
            ));
        }
    }

    if !interpretation.reference_candidates.is_empty()
        || !interpretation.clarification_candidates.is_empty()
    {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("[bounded-interpretation]".to_string());
        if !interpretation.reference_candidates.is_empty() {
            lines.push(format!(
                "- reference_candidates: {}",
                interpretation
                    .reference_candidates
                    .iter()
                    .map(format_reference_candidate)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        if !interpretation.clarification_candidates.is_empty() {
            lines.push(format!(
                "- clarification_candidates: {}",
                interpretation.clarification_candidates.join(" | ")
            ));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("[runtime-interpretation]\n{}\n", lines.join("\n")))
    }
}

pub fn format_turn_interpretation_for_turn(
    user_message: &str,
    interpretation: &TurnInterpretation,
) -> Option<String> {
    if infer_turn_capability_requirement(user_message).is_some() {
        return format_media_turn_interpretation(interpretation);
    }

    format_turn_interpretation(interpretation)
}

fn format_media_turn_interpretation(interpretation: &TurnInterpretation) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(profile) = &interpretation.user_profile {
        lines.push("[user-profile]".to_string());
        lines.extend(
            format_profile_projection(profile)
                .lines()
                .take(4)
                .map(str::to_string),
        );
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("[runtime-interpretation]\n{}\n", lines.join("\n")))
    }
}

fn snapshot_dialogue_state(state: &DialogueState) -> Option<DialogueStateSnapshot> {
    let focus_entities = state
        .focus_entities
        .iter()
        .map(|entity| (entity.kind.clone(), entity.name.clone()))
        .collect::<Vec<_>>();
    let comparison_set = state
        .comparison_set
        .iter()
        .map(|entity| (entity.kind.clone(), entity.name.clone()))
        .collect::<Vec<_>>();
    let reference_anchors = state.reference_anchors.clone();
    let last_tool_subjects = state.last_tool_subjects.clone();
    let recent_delivery_target = state.recent_delivery_target.clone();
    let recent_schedule_job = state.recent_schedule_job.clone();
    let recent_resource = state.recent_resource.clone();
    let recent_search = state.recent_search.clone();
    let recent_workspace = state.recent_workspace.clone();

    if focus_entities.is_empty()
        && comparison_set.is_empty()
        && reference_anchors.is_empty()
        && last_tool_subjects.is_empty()
        && recent_delivery_target.is_none()
        && recent_schedule_job.is_none()
        && recent_resource.is_none()
        && recent_search.is_none()
        && recent_workspace.is_none()
    {
        None
    } else {
        Some(DialogueStateSnapshot {
            focus_entities,
            comparison_set,
            reference_anchors,
            last_tool_subjects,
            recent_delivery_target,
            recent_schedule_job,
            recent_resource,
            recent_search,
            recent_workspace,
        })
    }
}

fn collect_reference_candidates(
    profile: Option<&UserProfile>,
    current_conversation: Option<&CurrentConversationSnapshot>,
    dialogue_state: Option<&DialogueStateSnapshot>,
    configured_delivery_target: Option<&ConversationDeliveryTarget>,
) -> Vec<ReferenceCandidate> {
    let mut candidates = Vec::new();

    if let Some(target) = configured_delivery_target {
        push_reference_candidate(
            &mut candidates,
            ReferenceCandidateKind::DeliveryTarget,
            &format_delivery_target(target),
            ReferenceSource::ConfiguredRuntime,
        );
    }

    if let Some(state) = dialogue_state {
        for (entity_kind, value) in state
            .focus_entities
            .iter()
            .chain(state.comparison_set.iter())
        {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Entity {
                    entity_kind: entity_kind.clone(),
                },
                value,
                ReferenceSource::DialogueState,
            );
        }
        for anchor in &state.reference_anchors {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Anchor {
                    selector: anchor.selector.clone(),
                    entity_kind: anchor.entity_kind.clone(),
                },
                &anchor.value,
                ReferenceSource::DialogueState,
            );
        }
        for subject in &state.last_tool_subjects {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::RecentSubject,
                subject,
                ReferenceSource::DialogueState,
            );
        }
        if let Some(target) = &state.recent_delivery_target {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::DeliveryTarget,
                &format_delivery_target(target),
                ReferenceSource::DialogueState,
            );
        }
        if let Some(schedule_job) = &state.recent_schedule_job {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::ScheduleJob,
                &schedule_job.job_id,
                ReferenceSource::DialogueState,
            );
            if let Some(session_target) = schedule_job.session_target.as_deref() {
                push_reference_candidate(
                    &mut candidates,
                    ReferenceCandidateKind::Entity {
                        entity_kind: "session_target".into(),
                    },
                    session_target,
                    ReferenceSource::DialogueState,
                );
            }
        }
        if let Some(resource) = &state.recent_resource {
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::ResourceLocator {
                    resource_kind: resource.kind.clone(),
                },
                &resource.locator,
                ReferenceSource::DialogueState,
            );
            if let Some(host) = resource.host.as_deref() {
                push_reference_candidate(
                    &mut candidates,
                    ReferenceCandidateKind::ResourceLocator {
                        resource_kind: resource.kind.clone(),
                    },
                    host,
                    ReferenceSource::DialogueState,
                );
            }
        }
        if let Some(search) = &state.recent_search {
            if let Some(query) = search.query.as_deref() {
                push_reference_candidate(
                    &mut candidates,
                    ReferenceCandidateKind::SearchQuery {
                        domain: search.domain.clone(),
                    },
                    query,
                    ReferenceSource::DialogueState,
                );
            }
            if let Some(locator) = search.primary_locator.as_deref() {
                push_reference_candidate(
                    &mut candidates,
                    ReferenceCandidateKind::SearchResult {
                        domain: search.domain.clone(),
                    },
                    locator,
                    ReferenceSource::DialogueState,
                );
            }
        }
        if let Some(workspace) = &state.recent_workspace {
            if let Some(name) = workspace.name.as_deref() {
                push_reference_candidate(
                    &mut candidates,
                    ReferenceCandidateKind::WorkspaceName {
                        action: workspace.action.clone(),
                    },
                    name,
                    ReferenceSource::DialogueState,
                );
            }
        }
    }

    if let Some(profile) = profile {
        for (key, value) in profile.iter() {
            let candidate_value = if let Some(target) = profile.get_delivery_target(key) {
                format_delivery_target(&target)
            } else {
                profile.get_text(key).unwrap_or_else(|| value.to_string())
            };
            push_reference_candidate(
                &mut candidates,
                ReferenceCandidateKind::Profile { key: key.clone() },
                &candidate_value,
                ReferenceSource::UserProfile,
            );
        }
    }

    if current_conversation.is_some() {
        push_reference_candidate(
            &mut candidates,
            ReferenceCandidateKind::DeliveryTarget,
            "current_conversation",
            ReferenceSource::CurrentConversation,
        );
    }

    candidates
}

fn collect_clarification_candidates(
    dialogue_state: Option<&DialogueStateSnapshot>,
    profile: Option<&UserProfile>,
) -> Vec<String> {
    let mut values = Vec::new();

    if let Some(state) = dialogue_state {
        let source = if !state.comparison_set.is_empty() {
            &state.comparison_set
        } else {
            &state.focus_entities
        };
        for (_, value) in source {
            push_unique_string(&mut values, value);
        }
    }

    if values.is_empty() {
        if let Some(profile) = profile {
            let profile_values = profile
                .iter()
                .filter_map(|(key, _)| profile.get_text(key))
                .collect::<Vec<_>>();
            if (2..=6).contains(&profile_values.len()) {
                for value in &profile_values {
                    push_unique_string(&mut values, value);
                }
            }
        }
    }

    values
}

fn push_reference_candidate(
    values: &mut Vec<ReferenceCandidate>,
    kind: ReferenceCandidateKind,
    value: &str,
    source: ReferenceSource,
) {
    if value.trim().is_empty() {
        return;
    }
    if !values.iter().any(|candidate| {
        candidate.kind == kind && candidate.value == value && candidate.source == source
    }) {
        values.push(ReferenceCandidate {
            kind,
            value: value.to_string(),
            source,
        });
    }
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !value.trim().is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn format_pairs(values: &[(String, String)]) -> String {
    values
        .iter()
        .map(|(left, right)| format!("{left}={right}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_reference_candidate(candidate: &ReferenceCandidate) -> String {
    format!(
        "{}:{}={}",
        reference_source_name(candidate.source),
        reference_candidate_kind_name(&candidate.kind),
        candidate.value
    )
}

fn format_reference_anchor(anchor: &ReferenceAnchor) -> String {
    let selector = match &anchor.selector {
        ReferenceAnchorSelector::Current => "current".to_string(),
        ReferenceAnchorSelector::Latest => "latest".to_string(),
        ReferenceAnchorSelector::Ordinal(ordinal) => ordinal_name(ordinal).to_string(),
    };
    match anchor.entity_kind.as_deref() {
        Some(entity_kind) => format!("{selector}<{entity_kind}>={}", anchor.value),
        None => format!("{selector}={}", anchor.value),
    }
}

fn format_delivery_target(target: &ConversationDeliveryTarget) -> String {
    match target {
        ConversationDeliveryTarget::CurrentConversation => "current_conversation".into(),
        ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } => {
            if thread_ref.is_some() {
                format!("explicit:{channel}:{recipient}#thread")
            } else {
                format!("explicit:{channel}:{recipient}")
            }
        }
    }
}

fn format_schedule_job(job: &ScheduleJobReference) -> String {
    let mut parts = vec![job.job_id.clone()];
    if let Some(session_target) = &job.session_target {
        parts.push(format!("session={session_target}"));
    }
    if let Some(timezone) = &job.timezone {
        parts.push(format!("tz={timezone}"));
    }
    parts.join(" ")
}

fn format_resource_reference(resource: &ResourceReference) -> String {
    let mut parts = vec![format!(
        "{} {}",
        resource_operation_name(&resource.operation),
        resource.locator
    )];
    parts.push(format!("kind={}", resource_kind_name(&resource.kind)));
    if let Some(host) = &resource.host {
        parts.push(format!("host={host}"));
    }
    parts.join(" ")
}

fn format_search_reference(search: &SearchReference) -> String {
    let mut parts = vec![format!("domain={}", search_domain_name(&search.domain))];
    if let Some(query) = &search.query {
        parts.push(format!("query={query}"));
    }
    if let Some(locator) = &search.primary_locator {
        parts.push(format!("primary={locator}"));
    }
    if let Some(result_count) = search.result_count {
        parts.push(format!("results={result_count}"));
    }
    parts.join(" ")
}

fn format_workspace_reference(workspace: &WorkspaceReference) -> String {
    let mut parts = vec![format!(
        "action={}",
        workspace_action_name(&workspace.action)
    )];
    if let Some(name) = &workspace.name {
        parts.push(format!("name={name}"));
    }
    if let Some(item_count) = workspace.item_count {
        parts.push(format!("items={item_count}"));
    }
    parts.join(" ")
}

fn reference_source_name(source: ReferenceSource) -> &'static str {
    match source {
        ReferenceSource::ConfiguredRuntime => "configured_runtime",
        ReferenceSource::DialogueState => "dialogue_state",
        ReferenceSource::UserProfile => "user_profile",
        ReferenceSource::CurrentConversation => "current_conversation",
    }
}

fn reference_candidate_kind_name(kind: &ReferenceCandidateKind) -> String {
    match kind {
        ReferenceCandidateKind::Entity { entity_kind } => format!("entity<{entity_kind}>"),
        ReferenceCandidateKind::Anchor {
            selector,
            entity_kind,
        } => match entity_kind.as_deref() {
            Some(entity_kind) => format!("anchor<{}:{}>", selector_name(selector), entity_kind),
            None => format!("anchor<{}>", selector_name(selector)),
        },
        ReferenceCandidateKind::Profile { key } => format!("profile<{key}>"),
        ReferenceCandidateKind::DeliveryTarget => "delivery_target".into(),
        ReferenceCandidateKind::ScheduleJob => "schedule_job".into(),
        ReferenceCandidateKind::ResourceLocator { resource_kind } => {
            format!("resource<{}>", resource_kind_name(resource_kind))
        }
        ReferenceCandidateKind::SearchQuery { domain } => {
            format!("search_query<{}>", search_domain_name(domain))
        }
        ReferenceCandidateKind::SearchResult { domain } => {
            format!("search_result<{}>", search_domain_name(domain))
        }
        ReferenceCandidateKind::WorkspaceName { action } => {
            format!("workspace<{}>", workspace_action_name(action))
        }
        ReferenceCandidateKind::RecentSubject => "recent_subject".into(),
    }
}

fn selector_name(selector: &ReferenceAnchorSelector) -> &'static str {
    match selector {
        ReferenceAnchorSelector::Current => "current",
        ReferenceAnchorSelector::Latest => "latest",
        ReferenceAnchorSelector::Ordinal(ordinal) => ordinal_name(ordinal),
    }
}

fn ordinal_name(ordinal: &ReferenceOrdinal) -> &'static str {
    match ordinal {
        ReferenceOrdinal::First => "first",
        ReferenceOrdinal::Second => "second",
        ReferenceOrdinal::Third => "third",
        ReferenceOrdinal::Fourth => "fourth",
    }
}

fn resource_kind_name(kind: &ResourceKind) -> &'static str {
    match kind {
        ResourceKind::File => "file",
        ResourceKind::Directory => "directory",
        ResourceKind::Image => "image",
        ResourceKind::Pdf => "pdf",
        ResourceKind::WebPage => "web_page",
        ResourceKind::WebResource => "web_resource",
        ResourceKind::NetworkEndpoint => "network_endpoint",
        ResourceKind::BrowserPage => "browser_page",
        ResourceKind::BrowserSelector => "browser_selector",
        ResourceKind::GitRepository => "git_repository",
        ResourceKind::GitBranch => "git_branch",
        ResourceKind::BackupSnapshot => "backup_snapshot",
        ResourceKind::ConfigFile => "config_file",
    }
}

fn resource_operation_name(
    operation: &crate::domain::tool_fact::ResourceOperation,
) -> &'static str {
    match operation {
        crate::domain::tool_fact::ResourceOperation::Read => "read",
        crate::domain::tool_fact::ResourceOperation::Write => "write",
        crate::domain::tool_fact::ResourceOperation::Edit => "edit",
        crate::domain::tool_fact::ResourceOperation::Search => "search",
        crate::domain::tool_fact::ResourceOperation::Fetch => "fetch",
        crate::domain::tool_fact::ResourceOperation::Open => "open",
        crate::domain::tool_fact::ResourceOperation::Click => "click",
        crate::domain::tool_fact::ResourceOperation::Type => "type",
        crate::domain::tool_fact::ResourceOperation::Inspect => "inspect",
        crate::domain::tool_fact::ResourceOperation::Snapshot => "snapshot",
        crate::domain::tool_fact::ResourceOperation::Verify => "verify",
        crate::domain::tool_fact::ResourceOperation::Restore => "restore",
        crate::domain::tool_fact::ResourceOperation::Configure => "configure",
    }
}

fn search_domain_name(domain: &SearchDomain) -> &'static str {
    match domain {
        SearchDomain::Web => "web",
        SearchDomain::Workspace => "workspace",
        SearchDomain::Memory => "memory",
        SearchDomain::Session => "session",
        SearchDomain::Precedent => "precedent",
        SearchDomain::Knowledge => "knowledge",
    }
}

fn workspace_action_name(action: &WorkspaceAction) -> &'static str {
    match action {
        WorkspaceAction::List => "list",
        WorkspaceAction::Switch => "switch",
        WorkspaceAction::Create => "create",
        WorkspaceAction::Export => "export",
        WorkspaceAction::Info => "info",
        WorkspaceAction::Backup => "backup",
        WorkspaceAction::Retention => "retention",
        WorkspaceAction::Purge => "purge",
        WorkspaceAction::Stats => "stats",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::dialogue_state::{
        FocusEntity, ResourceReference, SearchReference, WorkspaceReference,
    };
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingProfile, Entity,
        HybridSearchResult, MemoryCategory, MemoryEntry, MemoryError, MemoryId, MemoryQuery,
        Reflection, SearchResult, SessionId, Skill, SkillUpdate, TemporalFact, Visibility,
    };
    use crate::domain::tool_fact::{
        ResourceKind, ResourceOperation, ScheduleAction, ScheduleJobType, ScheduleKind,
        SearchDomain, WorkspaceAction,
    };
    use crate::domain::user_profile::DELIVERY_TARGET_PREFERENCE_KEY;
    use crate::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        WorkingMemoryPort,
    };
    use async_trait::async_trait;

    #[derive(Default)]
    struct StubMemory;

    #[async_trait]
    impl WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
            Ok(vec![])
        }
        async fn update_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: String,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn append_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SemanticMemoryPort for StubMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
            Ok(None)
        }
        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
            Ok(vec![])
        }
        async fn traverse(
            &self,
            _: &MemoryId,
            _: usize,
        ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
            Ok(vec![])
        }
        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SkillMemoryPort for StubMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(vec![])
        }
        async fn update_skill(
            &self,
            _: &MemoryId,
            _: SkillUpdate,
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl ReflectionPort for StubMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_relevant_reflections(
            &self,
            _: &MemoryQuery,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
        async fn get_failure_patterns(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
            Ok(ConsolidationReport::default())
        }
        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
            Ok(0)
        }
        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(HybridSearchResult::default())
        }
        async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(test_embedding(text))
        }
        async fn store(
            &self,
            _: &str,
            _: &str,
            _: &MemoryCategory,
            _: Option<&str>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn recall(
            &self,
            _: &str,
            _: usize,
            _: Option<&str>,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> {
            Ok(false)
        }
        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> {
            Ok(None)
        }
        async fn list(
            &self,
            _: Option<&MemoryCategory>,
            _: Option<&str>,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn count(&self) -> Result<usize, MemoryError> {
            Ok(0)
        }
        fn name(&self) -> &str {
            "stub"
        }
        async fn health_check(&self) -> bool {
            true
        }
        async fn promote_visibility(
            &self,
            _: &MemoryId,
            _: &Visibility,
            _: &[AgentId],
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        fn embedding_profile(&self) -> EmbeddingProfile {
            EmbeddingProfile {
                profile_id: "test:multilingual:8".into(),
                provider_family: "test".into(),
                model_id: "multilingual".into(),
                dimensions: 8,
                supports_multilingual: true,
                ..EmbeddingProfile::default()
            }
        }
    }

    fn test_embedding(_text: &str) -> Vec<f32> {
        vec![0.0; 8]
    }

    #[tokio::test]
    async fn returns_none_for_empty_inputs() {
        assert!(build_turn_interpretation(None, "", None, None, None, None)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn builds_profile_and_followup_candidates_without_phrase_router() {
        let memory = StubMemory;
        let mut profile = UserProfile::default();
        profile.set("response_locale", serde_json::json!("ru"));
        profile.set("workspace_anchor", serde_json::json!("Borealis"));
        let state = DialogueState {
            comparison_set: vec![
                crate::domain::dialogue_state::FocusEntity {
                    kind: "city".into(),
                    name: "Berlin".into(),
                    metadata: None,
                },
                crate::domain::dialogue_state::FocusEntity {
                    kind: "city".into(),
                    name: "Tbilisi".into(),
                    metadata: None,
                },
            ],
            ..Default::default()
        };

        let interpretation = build_turn_interpretation(
            Some(&memory),
            "translate it into my language and what about the second one",
            Some(profile),
            None,
            Some(&state),
            None,
        )
        .await
        .unwrap();

        assert!(!interpretation.reference_candidates.is_empty());
        assert_eq!(
            interpretation.clarification_candidates,
            vec!["Berlin", "Tbilisi"]
        );
    }

    #[tokio::test]
    async fn formats_profile_and_structured_interpretation() {
        let memory = StubMemory;
        let mut profile = UserProfile::default();
        profile.set("response_locale", serde_json::json!("ru"));
        profile.set("project_alias", serde_json::json!("Borealis"));
        let state = DialogueState {
            focus_entities: vec![crate::domain::dialogue_state::FocusEntity {
                kind: "city".into(),
                name: "Berlin".into(),
                metadata: None,
            }],
            last_tool_subjects: vec!["weather_lookup".into()],
            ..Default::default()
        };
        let current = CurrentConversationContext {
            source_adapter: "matrix".into(),
            conversation_id: "matrix_room".into(),
            reply_ref: "!room:example.com".into(),
            thread_ref: Some("$thread".into()),
            actor_id: "alice".into(),
        };

        let interpretation = build_turn_interpretation(
            Some(&memory),
            "send it here and translate it into my language",
            Some(profile),
            Some(&current),
            Some(&state),
            None,
        )
        .await
        .unwrap();
        let block = format_turn_interpretation(&interpretation).unwrap();

        assert!(block.contains("[runtime-interpretation]"));
        assert!(block.contains("response_locale: ru"));
        assert!(block.contains("adapter: matrix"));
        assert!(block.contains("focus_entities: city=Berlin"));
        assert!(block.contains("last_tool_subjects: weather_lookup"));
    }

    #[tokio::test]
    async fn dialogue_state_slots_surface_reference_candidates() {
        let interpretation = build_turn_interpretation(
            None,
            "the second one",
            None,
            None,
            Some(&DialogueState {
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
                reference_anchors: vec![
                    crate::domain::dialogue_state::ReferenceAnchor {
                        selector: crate::domain::dialogue_state::ReferenceAnchorSelector::Ordinal(
                            crate::domain::dialogue_state::ReferenceOrdinal::First,
                        ),
                        entity_kind: Some("city".into()),
                        value: "Berlin".into(),
                    },
                    crate::domain::dialogue_state::ReferenceAnchor {
                        selector: crate::domain::dialogue_state::ReferenceAnchorSelector::Ordinal(
                            crate::domain::dialogue_state::ReferenceOrdinal::Second,
                        ),
                        entity_kind: Some("city".into()),
                        value: "Tbilisi".into(),
                    },
                ],
                last_tool_subjects: vec!["Berlin".into(), "Tbilisi".into()],
                ..Default::default()
            }),
            None,
        )
        .await
        .unwrap();

        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::Anchor {
                    selector: ReferenceAnchorSelector::Ordinal(ReferenceOrdinal::Second),
                    entity_kind,
                } if entity_kind.as_deref() == Some("city")
            ) && candidate.value == "Tbilisi"
        }));
    }

    #[tokio::test]
    async fn typed_delivery_and_schedule_state_surface_reference_candidates() {
        let interpretation = build_turn_interpretation(
            None,
            "send the report there and rerun that job",
            None,
            None,
            Some(&DialogueState {
                recent_delivery_target: Some(ConversationDeliveryTarget::Explicit {
                    channel: "telegram".into(),
                    recipient: "@synapseclaw".into(),
                    thread_ref: None,
                }),
                recent_schedule_job: Some(ScheduleJobReference {
                    job_id: "job_123".into(),
                    action: ScheduleAction::Run,
                    job_type: Some(ScheduleJobType::Agent),
                    schedule_kind: Some(ScheduleKind::Cron),
                    session_target: Some("main".into()),
                    timezone: Some("Europe/Berlin".into()),
                }),
                ..Default::default()
            }),
            None,
        )
        .await
        .unwrap();

        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(candidate.kind, ReferenceCandidateKind::DeliveryTarget)
                && candidate.value == "explicit:telegram:@synapseclaw"
        }));
        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(candidate.kind, ReferenceCandidateKind::ScheduleJob)
                && candidate.value == "job_123"
        }));
        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::Entity { entity_kind } if entity_kind == "session_target"
            ) && candidate.value == "main"
        }));
    }

    #[tokio::test]
    async fn typed_resource_and_search_state_surface_reference_candidates() {
        let interpretation = build_turn_interpretation(
            None,
            "open that file and reuse that search",
            None,
            None,
            Some(&DialogueState {
                recent_resource: Some(ResourceReference {
                    kind: ResourceKind::File,
                    operation: ResourceOperation::Read,
                    locator: "/workspace/README.md".into(),
                    host: None,
                }),
                recent_search: Some(SearchReference {
                    domain: SearchDomain::Session,
                    query: Some("what did we discuss".into()),
                    primary_locator: Some("web:session-123".into()),
                    result_count: Some(3),
                }),
                ..Default::default()
            }),
            None,
        )
        .await
        .unwrap();

        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::ResourceLocator { resource_kind }
                    if resource_kind == &ResourceKind::File
            ) && candidate.value == "/workspace/README.md"
        }));
        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::SearchQuery { domain }
                    if domain == &SearchDomain::Session
            ) && candidate.value == "what did we discuss"
        }));
        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::SearchResult { domain }
                    if domain == &SearchDomain::Session
            ) && candidate.value == "web:session-123"
        }));
    }

    #[tokio::test]
    async fn typed_workspace_state_surfaces_reference_candidates() {
        let interpretation = build_turn_interpretation(
            None,
            "switch back there",
            None,
            None,
            Some(&DialogueState {
                recent_workspace: Some(WorkspaceReference {
                    action: WorkspaceAction::Switch,
                    name: Some("research-lab".into()),
                    item_count: Some(12),
                }),
                ..Default::default()
            }),
            None,
        )
        .await
        .unwrap();

        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::WorkspaceName { action }
                    if action == &WorkspaceAction::Switch
            ) && candidate.value == "research-lab"
        }));
    }

    #[tokio::test]
    async fn profile_delivery_target_surfaces_actual_target_value() {
        let interpretation = build_turn_interpretation(
            None,
            "send it to my default place",
            Some({
                let mut profile = UserProfile::default();
                profile.set(
                    DELIVERY_TARGET_PREFERENCE_KEY,
                    serde_json::json!(ConversationDeliveryTarget::Explicit {
                        channel: "telegram".into(),
                        recipient: "@synapseclaw".into(),
                        thread_ref: None,
                    }),
                );
                profile
            }),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(interpretation.reference_candidates.iter().any(|candidate| {
            matches!(
                &candidate.kind,
                ReferenceCandidateKind::Profile { key } if key == DELIVERY_TARGET_PREFERENCE_KEY
            ) && candidate.value == "explicit:telegram:@synapseclaw"
        }));
    }

    #[tokio::test]
    async fn configured_delivery_target_is_formatted_without_profile_or_dialogue_state() {
        let interpretation = build_turn_interpretation(
            None,
            "send the restart report to matrix",
            None,
            None,
            None,
            Some(ConversationDeliveryTarget::Explicit {
                channel: "matrix".into(),
                recipient: "!ops:example.org".into(),
                thread_ref: None,
            }),
        )
        .await
        .unwrap();

        let block = format_turn_interpretation(&interpretation).unwrap();
        assert!(block.contains("[configured-runtime]"));
        assert!(block.contains("configured_delivery_target: explicit:matrix:!ops:example.org"));
    }

    #[test]
    fn media_turn_interpretation_uses_compact_profile_only_block() {
        let interpretation = TurnInterpretation {
            user_profile: Some({
                let mut profile = UserProfile::default();
                profile.set("response_locale", serde_json::json!("ru"));
                profile.set("project_alias", serde_json::json!("Borealis"));
                profile.set("workspace_anchor", serde_json::json!("Borealis"));
                profile.set("tone_hint", serde_json::json!("direct"));
                profile.set("release_tracks", serde_json::json!(["linux"]));
                profile.set(
                    DELIVERY_TARGET_PREFERENCE_KEY,
                    serde_json::json!(ConversationDeliveryTarget::CurrentConversation),
                );
                profile
            }),
            current_conversation: Some(CurrentConversationSnapshot {
                adapter: "matrix".into(),
                has_thread: true,
            }),
            dialogue_state: Some(DialogueStateSnapshot {
                focus_entities: vec![("project".into(), "Borealis".into())],
                comparison_set: Vec::new(),
                reference_anchors: Vec::new(),
                last_tool_subjects: vec!["workspace_search".into()],
                recent_delivery_target: None,
                recent_schedule_job: None,
                recent_resource: Some(ResourceReference {
                    kind: ResourceKind::File,
                    operation: ResourceOperation::Read,
                    locator: "docs/fork/ipc-phase4_10-plan.md".into(),
                    host: None,
                }),
                recent_search: None,
                recent_workspace: None,
            }),
            configured_delivery_target: Some(ConversationDeliveryTarget::CurrentConversation),
            reference_candidates: Vec::new(),
            clarification_candidates: vec!["Borealis".into()],
        };

        let block = format_turn_interpretation_for_turn(
            "Describe this [IMAGE:/tmp/smoke.png]",
            &interpretation,
        )
        .expect("compact media interpretation block");

        assert!(block.contains("[runtime-interpretation]"));
        assert!(block.contains("[user-profile]"));
        assert!(block.contains("response_locale: ru"));
        assert!(!block.contains("tone_hint"));
        assert!(!block.contains("[current-conversation]"));
        assert!(!block.contains("[configured-runtime]"));
        assert!(!block.contains("[working-state]"));
        assert!(!block.contains("[bounded-interpretation]"));
        assert!(block.contains("project_alias: Borealis"));
        assert!(!block.contains("workspace_anchor"));
    }
}
