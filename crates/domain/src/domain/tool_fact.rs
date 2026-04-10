//! Typed tool-fact scaffold for Phase 4.8.
//!
//! This module is intentionally introduced before the full migration so the
//! runtime has an explicit target architecture that does not depend on
//! `slot.name == "..."` string contracts.
//!
//! Current active paths may still use transitional string slots. The long-term
//! goal is:
//!
//! - tools emit typed fact payloads
//! - embeddings retrieve candidate context
//! - a bounded interpreter updates working state from typed payloads
//! - projection/UI layers render strings only at the edge

use crate::domain::conversation_target::ConversationDeliveryTarget;
use crate::domain::dialogue_state::FocusEntity;
use serde::{Deserialize, Serialize};

/// Future canonical structured fact emitted by a tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypedToolFact {
    /// Stable tool id (external capability id, not semantic state).
    pub tool_id: String,
    /// Tool-owned typed payload.
    pub payload: ToolFactPayload,
}

/// High-level typed fact families.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolFactPayload {
    Focus(FocusFact),
    Outcome(OutcomeFact),
    Delivery(DeliveryFact),
    Resource(ResourceFact),
    Schedule(ScheduleFact),
    UserProfile(UserProfileFact),
    Search(SearchFact),
    Workspace(WorkspaceFact),
    Knowledge(KnowledgeFact),
    Project(ProjectFact),
    Security(SecurityFact),
    Routing(RoutingFact),
    Notification(NotificationFact),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FocusFact {
    pub entities: Vec<FocusEntity>,
    pub subjects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutcomeFact {
    pub status: OutcomeStatus,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutcomeStatus {
    Succeeded,
    ReportedFailure,
    RuntimeError,
    Blocked,
    UnknownTool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryFact {
    pub target: DeliveryTargetKind,
    pub content_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryTargetKind {
    CurrentConversation,
    Explicit(ConversationDeliveryTarget),
    ProfileDefault(ConversationDeliveryTarget),
    ConfiguredDefault(ConversationDeliveryTarget),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceFact {
    pub kind: ResourceKind,
    pub operation: ResourceOperation,
    pub locator: String,
    pub host: Option<String>,
    pub metadata: ResourceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceKind {
    File,
    Directory,
    Image,
    Pdf,
    WebPage,
    WebResource,
    NetworkEndpoint,
    BrowserPage,
    BrowserSelector,
    GitRepository,
    GitBranch,
    BackupSnapshot,
    ConfigFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceOperation {
    Read,
    Write,
    Edit,
    Search,
    Fetch,
    Open,
    Click,
    Type,
    Inspect,
    Snapshot,
    Verify,
    Restore,
    Configure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ResourceMetadata {
    pub byte_count: Option<usize>,
    pub item_count: Option<usize>,
    pub include_base64: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleFact {
    pub action: ScheduleAction,
    pub job_type: Option<ScheduleJobType>,
    pub schedule_kind: Option<ScheduleKind>,
    pub job_id: Option<String>,
    pub annotation: Option<String>,
    pub timezone: Option<String>,
    pub target: Option<ScheduleTarget>,
    pub run_count: Option<usize>,
    pub last_status: Option<String>,
    pub last_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScheduleAction {
    Create,
    Update,
    Remove,
    Inspect,
    Run,
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScheduleJobType {
    Agent,
    Shell,
    Delivery,
    Heartbeat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScheduleKind {
    Cron,
    At,
    Every,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleTarget {
    pub session: Option<String>,
    pub delivery: Option<ConversationDeliveryTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserProfileFact {
    pub field: UserProfileField,
    pub operation: ProfileOperation,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UserProfileField {
    PreferredLanguage,
    Timezone,
    DefaultCity,
    CommunicationStyle,
    KnownEnvironments,
    DefaultDeliveryTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProfileOperation {
    Set,
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchFact {
    pub domain: SearchDomain,
    pub query: Option<String>,
    pub result_count: Option<usize>,
    pub primary_locator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchDomain {
    Web,
    Workspace,
    Memory,
    Session,
    Precedent,
    Knowledge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceFact {
    pub action: WorkspaceAction,
    pub name: Option<String>,
    pub item_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkspaceAction {
    List,
    Switch,
    Create,
    Export,
    Info,
    Backup,
    Retention,
    Purge,
    Stats,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeFact {
    pub action: KnowledgeAction,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub entity_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KnowledgeAction {
    Search,
    AddEntity,
    AddFact,
    GetFacts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectFact {
    pub action: ProjectAction,
    pub project_name: Option<String>,
    pub language: Option<String>,
    pub period: Option<String>,
    pub audience: Option<String>,
    pub tone: Option<String>,
    pub task_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProjectAction {
    StatusReport,
    RiskScan,
    DraftUpdate,
    SprintSummary,
    EffortEstimate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityFact {
    pub action: SecurityAction,
    pub severity: Option<String>,
    pub subject: Option<String>,
    pub step: Option<u64>,
    pub client_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecurityAction {
    TriageAlert,
    RunPlaybook,
    ParseVulnerability,
    GenerateReport,
    AlertStats,
    ListPlaybooks,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutingFact {
    pub action: RoutingAction,
    pub preset: Option<String>,
    pub hint: Option<String>,
    pub agent_name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub matcher_count: Option<usize>,
    pub allowed_tool_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RoutingAction {
    Get,
    ListHints,
    ListPresets,
    SetPreset,
    SetDefault,
    UpsertScenario,
    RemoveScenario,
    UpsertAgent,
    RemoveAgent,
    ConfigureProxy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationFact {
    pub channel: NotificationChannel,
    pub message_bytes: usize,
    pub title_bytes: Option<usize>,
    pub priority: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationChannel {
    Telegram,
    Pushover,
    Matrix,
    Slack,
    Other(String),
}

impl TypedToolFact {
    pub fn focus(
        tool_id: impl Into<String>,
        entities: Vec<FocusEntity>,
        subjects: Vec<String>,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            payload: ToolFactPayload::Focus(FocusFact { entities, subjects }),
        }
    }

    pub fn push_focus_entity(&mut self, entity: FocusEntity) {
        if let ToolFactPayload::Focus(focus) = &mut self.payload {
            if !focus
                .entities
                .iter()
                .any(|existing| existing.kind == entity.kind && existing.name == entity.name)
            {
                focus.entities.push(entity);
            }
        }
    }

    pub fn push_subject(&mut self, subject: impl Into<String>) {
        if let ToolFactPayload::Focus(focus) = &mut self.payload {
            let subject = subject.into();
            if !focus.subjects.iter().any(|existing| existing == &subject) {
                focus.subjects.push(subject);
            }
        }
    }

    pub fn focus_entities(&self) -> &[FocusEntity] {
        match &self.payload {
            ToolFactPayload::Focus(focus) => &focus.entities,
            _ => &[],
        }
    }

    pub fn subjects(&self) -> &[String] {
        match &self.payload {
            ToolFactPayload::Focus(focus) => &focus.subjects,
            _ => &[],
        }
    }

    pub fn outcome(
        tool_id: impl Into<String>,
        status: OutcomeStatus,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            payload: ToolFactPayload::Outcome(OutcomeFact {
                status,
                duration_ms,
            }),
        }
    }

    pub fn projected_focus_entities(&self) -> Vec<FocusEntity> {
        match &self.payload {
            ToolFactPayload::Focus(focus) => focus.entities.clone(),
            ToolFactPayload::Outcome(_) => Vec::new(),
            ToolFactPayload::Delivery(delivery) => vec![project_delivery_focus(delivery)],
            ToolFactPayload::Resource(resource) => vec![project_resource_focus(resource)],
            ToolFactPayload::Schedule(schedule) => project_schedule_focus(schedule),
            ToolFactPayload::UserProfile(profile) => {
                project_profile_focus(profile).into_iter().collect()
            }
            ToolFactPayload::Search(search) => project_search_focus(search).into_iter().collect(),
            ToolFactPayload::Workspace(workspace) => {
                project_workspace_focus(workspace).into_iter().collect()
            }
            ToolFactPayload::Knowledge(knowledge) => project_knowledge_focus(knowledge),
            ToolFactPayload::Project(project) => {
                project_project_focus(project).into_iter().collect()
            }
            ToolFactPayload::Security(security) => {
                project_security_focus(security).into_iter().collect()
            }
            ToolFactPayload::Routing(routing) => {
                project_routing_focus(routing).into_iter().collect()
            }
            ToolFactPayload::Notification(notification) => {
                vec![project_notification_focus(notification)]
            }
        }
    }

    pub fn projected_subjects(&self) -> Vec<String> {
        let mut subjects = Vec::new();
        for entity in self.projected_focus_entities() {
            if !subjects.iter().any(|existing| existing == &entity.name) {
                subjects.push(entity.name);
            }
        }

        match &self.payload {
            ToolFactPayload::Focus(focus) => {
                for subject in &focus.subjects {
                    if !subjects.iter().any(|existing| existing == subject) {
                        subjects.push(subject.clone());
                    }
                }
            }
            ToolFactPayload::Outcome(_) => {}
            ToolFactPayload::Delivery(delivery) => match &delivery.target {
                DeliveryTargetKind::CurrentConversation
                | DeliveryTargetKind::Explicit(ConversationDeliveryTarget::CurrentConversation)
                | DeliveryTargetKind::ProfileDefault(
                    ConversationDeliveryTarget::CurrentConversation,
                )
                | DeliveryTargetKind::ConfiguredDefault(
                    ConversationDeliveryTarget::CurrentConversation,
                ) => push_unique(&mut subjects, "current_conversation".to_string()),
                DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                    channel,
                    recipient,
                    ..
                })
                | DeliveryTargetKind::ProfileDefault(ConversationDeliveryTarget::Explicit {
                    channel,
                    recipient,
                    ..
                })
                | DeliveryTargetKind::ConfiguredDefault(ConversationDeliveryTarget::Explicit {
                    channel,
                    recipient,
                    ..
                }) => {
                    push_unique(&mut subjects, channel.clone());
                    push_unique(&mut subjects, recipient.clone());
                }
            },
            ToolFactPayload::Resource(resource) => {
                push_unique(&mut subjects, resource.locator.clone());
                if let Some(host) = &resource.host {
                    push_unique(&mut subjects, host.clone());
                }
            }
            ToolFactPayload::Schedule(schedule) => {
                if let Some(job_id) = &schedule.job_id {
                    push_unique(&mut subjects, job_id.clone());
                }
                if let Some(annotation) = &schedule.annotation {
                    push_unique(&mut subjects, annotation.clone());
                }
                if let Some(timezone) = &schedule.timezone {
                    push_unique(&mut subjects, timezone.clone());
                }
                if let Some(status) = &schedule.last_status {
                    push_unique(&mut subjects, status.clone());
                }
                if let Some(target) = &schedule.target {
                    if let Some(session) = &target.session {
                        push_unique(&mut subjects, session.clone());
                    }
                    if let Some(ConversationDeliveryTarget::Explicit {
                        channel, recipient, ..
                    }) = &target.delivery
                    {
                        push_unique(&mut subjects, channel.clone());
                        push_unique(&mut subjects, recipient.clone());
                    }
                }
            }
            ToolFactPayload::UserProfile(profile) => {
                if let Some(value) = &profile.value {
                    push_unique(&mut subjects, value.clone());
                }
            }
            ToolFactPayload::Search(search) => {
                if let Some(query) = &search.query {
                    push_unique(&mut subjects, query.clone());
                }
                if let Some(locator) = &search.primary_locator {
                    push_unique(&mut subjects, locator.clone());
                }
            }
            ToolFactPayload::Workspace(workspace) => {
                if let Some(name) = &workspace.name {
                    push_unique(&mut subjects, name.clone());
                }
            }
            ToolFactPayload::Knowledge(knowledge) => {
                if let Some(subject) = &knowledge.subject {
                    push_unique(&mut subjects, subject.clone());
                }
                if let Some(object) = &knowledge.object {
                    push_unique(&mut subjects, object.clone());
                }
            }
            ToolFactPayload::Project(project) => {
                if let Some(name) = &project.project_name {
                    push_unique(&mut subjects, name.clone());
                }
            }
            ToolFactPayload::Security(security) => {
                if let Some(subject) = &security.subject {
                    push_unique(&mut subjects, subject.clone());
                }
            }
            ToolFactPayload::Routing(routing) => {
                if let Some(agent_name) = &routing.agent_name {
                    push_unique(&mut subjects, agent_name.clone());
                }
                if let Some(hint) = &routing.hint {
                    push_unique(&mut subjects, hint.clone());
                }
            }
            ToolFactPayload::Notification(notification) => {
                push_unique(
                    &mut subjects,
                    notification_channel_name(&notification.channel),
                );
            }
        }

        subjects
    }
}

impl OutcomeStatus {
    pub fn is_failure(&self) -> bool {
        !matches!(self, Self::Succeeded)
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn project_delivery_focus(delivery: &DeliveryFact) -> FocusEntity {
    match &delivery.target {
        DeliveryTargetKind::CurrentConversation
        | DeliveryTargetKind::Explicit(ConversationDeliveryTarget::CurrentConversation)
        | DeliveryTargetKind::ProfileDefault(ConversationDeliveryTarget::CurrentConversation)
        | DeliveryTargetKind::ConfiguredDefault(ConversationDeliveryTarget::CurrentConversation) => {
            FocusEntity {
                kind: "delivery_target".into(),
                name: "current_conversation".into(),
                metadata: Some("current".into()),
            }
        }
        DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            ..
        }) => FocusEntity {
            kind: "delivery_target".into(),
            name: recipient.clone(),
            metadata: Some(channel.clone()),
        },
        DeliveryTargetKind::ProfileDefault(ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            ..
        }) => FocusEntity {
            kind: "delivery_target".into(),
            name: recipient.clone(),
            metadata: Some(format!("profile_default:{channel}")),
        },
        DeliveryTargetKind::ConfiguredDefault(ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            ..
        }) => FocusEntity {
            kind: "delivery_target".into(),
            name: recipient.clone(),
            metadata: Some(format!("configured_default:{channel}")),
        },
    }
}

fn project_resource_focus(resource: &ResourceFact) -> FocusEntity {
    FocusEntity {
        kind: resource_focus_kind(&resource.kind, &resource.operation).into(),
        name: resource.locator.clone(),
        metadata: resource
            .host
            .clone()
            .or_else(|| Some(resource_operation_name(&resource.operation).to_string())),
    }
}

fn project_schedule_focus(schedule: &ScheduleFact) -> Vec<FocusEntity> {
    let mut entities = Vec::new();
    if let Some(job_id) = &schedule.job_id {
        let metadata = schedule
            .annotation
            .clone()
            .or_else(|| {
                schedule
                    .schedule_kind
                    .as_ref()
                    .map(schedule_kind_name)
                    .map(str::to_string)
            })
            .or_else(|| Some(schedule_action_name(&schedule.action).to_string()));
        entities.push(FocusEntity {
            kind: "scheduled_job".into(),
            name: job_id.clone(),
            metadata,
        });
    }
    if let Some(run_count) = schedule.run_count {
        entities.push(FocusEntity {
            kind: "run_history".into(),
            name: run_count.to_string(),
            metadata: schedule
                .last_status
                .clone()
                .or_else(|| schedule.last_duration_ms.map(|value| value.to_string())),
        });
    }
    if let Some(target) = &schedule.target {
        if let Some(session) = &target.session {
            entities.push(FocusEntity {
                kind: "session_target".into(),
                name: session.clone(),
                metadata: Some(schedule_action_name(&schedule.action).into()),
            });
        }
        if let Some(ConversationDeliveryTarget::Explicit {
            channel, recipient, ..
        }) = &target.delivery
        {
            entities.push(FocusEntity {
                kind: "delivery_target".into(),
                name: recipient.clone(),
                metadata: Some(channel.clone()),
            });
        }
    }
    entities
}

fn project_profile_focus(profile: &UserProfileFact) -> Option<FocusEntity> {
    let value = profile.value.clone()?;
    Some(FocusEntity {
        kind: user_profile_kind(&profile.field).into(),
        name: value,
        metadata: Some(profile_operation_name(&profile.operation).into()),
    })
}

fn project_search_focus(search: &SearchFact) -> Option<FocusEntity> {
    let locator = search.primary_locator.clone()?;
    Some(FocusEntity {
        kind: search_domain_kind(&search.domain).into(),
        name: locator,
        metadata: search
            .query
            .clone()
            .or_else(|| search.result_count.map(|count| format!("{count}_results"))),
    })
}

fn project_workspace_focus(workspace: &WorkspaceFact) -> Option<FocusEntity> {
    let name = workspace.name.clone()?;
    Some(FocusEntity {
        kind: "workspace".into(),
        name,
        metadata: Some(workspace_action_name(&workspace.action).into()),
    })
}

fn project_knowledge_focus(knowledge: &KnowledgeFact) -> Vec<FocusEntity> {
    let mut entities = Vec::new();
    if let Some(subject) = &knowledge.subject {
        entities.push(FocusEntity {
            kind: knowledge
                .entity_type
                .clone()
                .unwrap_or_else(|| "knowledge_entity".into()),
            name: subject.clone(),
            metadata: knowledge.predicate.clone(),
        });
    }
    if let Some(object) = &knowledge.object {
        entities.push(FocusEntity {
            kind: "knowledge_object".into(),
            name: object.clone(),
            metadata: knowledge.predicate.clone(),
        });
    }
    entities
}

fn project_project_focus(project: &ProjectFact) -> Option<FocusEntity> {
    let name = project.project_name.clone()?;
    Some(FocusEntity {
        kind: "project".into(),
        name,
        metadata: Some(project_action_name(&project.action).into()),
    })
}

fn project_security_focus(security: &SecurityFact) -> Option<FocusEntity> {
    let subject = security.subject.clone()?;
    Some(FocusEntity {
        kind: "security_subject".into(),
        name: subject,
        metadata: Some(security_action_name(&security.action).into()),
    })
}

fn project_routing_focus(routing: &RoutingFact) -> Option<FocusEntity> {
    if let Some(agent_name) = &routing.agent_name {
        return Some(FocusEntity {
            kind: "routing_agent".into(),
            name: agent_name.clone(),
            metadata: Some(routing_action_name(&routing.action).into()),
        });
    }
    routing.hint.as_ref().map(|hint| FocusEntity {
        kind: "routing_hint".into(),
        name: hint.clone(),
        metadata: Some(routing_action_name(&routing.action).into()),
    })
}

fn project_notification_focus(notification: &NotificationFact) -> FocusEntity {
    FocusEntity {
        kind: "notification_channel".into(),
        name: notification_channel_name(&notification.channel),
        metadata: notification.priority.map(|priority| priority.to_string()),
    }
}

fn resource_focus_kind(kind: &ResourceKind, operation: &ResourceOperation) -> &'static str {
    match (kind, operation) {
        (ResourceKind::File, ResourceOperation::Edit | ResourceOperation::Search) => {
            "workspace_file"
        }
        (ResourceKind::File, _) => "file_resource",
        (ResourceKind::Directory, _) => "workspace_directory",
        (ResourceKind::Image, _) => "image_file",
        (ResourceKind::Pdf, _) => "pdf_document",
        (ResourceKind::WebPage, _) => "browser_page",
        (ResourceKind::WebResource, _) => "web_resource",
        (ResourceKind::NetworkEndpoint, _) => "network_resource",
        (ResourceKind::BrowserPage, _) => "browser_page",
        (ResourceKind::BrowserSelector, _) => "browser_selector",
        (ResourceKind::GitRepository, _) => "git_repository",
        (ResourceKind::GitBranch, _) => "git_branch",
        (ResourceKind::BackupSnapshot, _) => "backup_snapshot",
        (ResourceKind::ConfigFile, _) => "config_file",
    }
}

fn resource_operation_name(operation: &ResourceOperation) -> &'static str {
    match operation {
        ResourceOperation::Read => "read",
        ResourceOperation::Write => "write",
        ResourceOperation::Edit => "edit",
        ResourceOperation::Search => "search",
        ResourceOperation::Fetch => "fetch",
        ResourceOperation::Open => "open",
        ResourceOperation::Click => "click",
        ResourceOperation::Type => "type",
        ResourceOperation::Inspect => "inspect",
        ResourceOperation::Snapshot => "snapshot",
        ResourceOperation::Verify => "verify",
        ResourceOperation::Restore => "restore",
        ResourceOperation::Configure => "configure",
    }
}

fn schedule_action_name(action: &ScheduleAction) -> &'static str {
    match action {
        ScheduleAction::Create => "create",
        ScheduleAction::Update => "update",
        ScheduleAction::Remove => "remove",
        ScheduleAction::Inspect => "inspect",
        ScheduleAction::Run => "run",
        ScheduleAction::List => "list",
    }
}

fn schedule_kind_name(kind: &ScheduleKind) -> &'static str {
    match kind {
        ScheduleKind::Cron => "cron",
        ScheduleKind::At => "at",
        ScheduleKind::Every => "every",
    }
}

fn user_profile_kind(field: &UserProfileField) -> &'static str {
    match field {
        UserProfileField::PreferredLanguage => "preferred_language",
        UserProfileField::Timezone => "timezone",
        UserProfileField::DefaultCity => "default_city",
        UserProfileField::CommunicationStyle => "communication_style",
        UserProfileField::KnownEnvironments => "known_environment",
        UserProfileField::DefaultDeliveryTarget => "default_delivery_target",
    }
}

fn profile_operation_name(operation: &ProfileOperation) -> &'static str {
    match operation {
        ProfileOperation::Set => "set",
        ProfileOperation::Clear => "clear",
    }
}

fn search_domain_kind(domain: &SearchDomain) -> &'static str {
    match domain {
        SearchDomain::Web => "search_result",
        SearchDomain::Workspace => "workspace_file",
        SearchDomain::Memory => "memory_entry",
        SearchDomain::Session => "session",
        SearchDomain::Precedent => "run_recipe",
        SearchDomain::Knowledge => "knowledge_entity",
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

fn project_action_name(action: &ProjectAction) -> &'static str {
    match action {
        ProjectAction::StatusReport => "status_report",
        ProjectAction::RiskScan => "risk_scan",
        ProjectAction::DraftUpdate => "draft_update",
        ProjectAction::SprintSummary => "sprint_summary",
        ProjectAction::EffortEstimate => "effort_estimate",
    }
}

fn security_action_name(action: &SecurityAction) -> &'static str {
    match action {
        SecurityAction::TriageAlert => "triage_alert",
        SecurityAction::RunPlaybook => "run_playbook",
        SecurityAction::ParseVulnerability => "parse_vulnerability",
        SecurityAction::GenerateReport => "generate_report",
        SecurityAction::AlertStats => "alert_stats",
        SecurityAction::ListPlaybooks => "list_playbooks",
    }
}

fn routing_action_name(action: &RoutingAction) -> &'static str {
    match action {
        RoutingAction::Get => "get",
        RoutingAction::ListHints => "list_hints",
        RoutingAction::ListPresets => "list_presets",
        RoutingAction::SetPreset => "set_preset",
        RoutingAction::SetDefault => "set_default",
        RoutingAction::UpsertScenario => "upsert_scenario",
        RoutingAction::RemoveScenario => "remove_scenario",
        RoutingAction::UpsertAgent => "upsert_agent",
        RoutingAction::RemoveAgent => "remove_agent",
        RoutingAction::ConfigureProxy => "configure_proxy",
    }
}

fn notification_channel_name(channel: &NotificationChannel) -> String {
    match channel {
        NotificationChannel::Telegram => "telegram".into(),
        NotificationChannel::Pushover => "pushover".into(),
        NotificationChannel::Matrix => "matrix".into(),
        NotificationChannel::Slack => "slack".into(),
        NotificationChannel::Other(value) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_payloads_are_serializable() {
        let fact = TypedToolFact {
            tool_id: "message_send".into(),
            payload: ToolFactPayload::Delivery(DeliveryFact {
                target: DeliveryTargetKind::CurrentConversation,
                content_bytes: Some(42),
            }),
        };

        let json = serde_json::to_string(&fact).unwrap();
        assert!(json.contains("message_send"));
        assert!(json.contains("CurrentConversation"));
    }

    #[test]
    fn delivery_fact_projects_target_and_subjects() {
        let fact = TypedToolFact {
            tool_id: "telegram_post".into(),
            payload: ToolFactPayload::Delivery(DeliveryFact {
                target: DeliveryTargetKind::Explicit(ConversationDeliveryTarget::Explicit {
                    channel: "telegram".into(),
                    recipient: "@synapseclaw".into(),
                    thread_ref: None,
                }),
                content_bytes: Some(5),
            }),
        };

        let projected = fact.projected_focus_entities();
        assert_eq!(projected[0].kind, "delivery_target");
        assert_eq!(projected[0].name, "@synapseclaw");
        assert_eq!(projected[0].metadata.as_deref(), Some("telegram"));
        let subjects = fact.projected_subjects();
        assert!(subjects.iter().any(|subject| subject == "telegram"));
        assert!(subjects.iter().any(|subject| subject == "@synapseclaw"));
    }

    #[test]
    fn resource_fact_projects_locator_and_host() {
        let fact = TypedToolFact {
            tool_id: "web_fetch".into(),
            payload: ToolFactPayload::Resource(ResourceFact {
                kind: ResourceKind::WebResource,
                operation: ResourceOperation::Fetch,
                locator: "https://example.com/docs".into(),
                host: Some("example.com".into()),
                metadata: ResourceMetadata::default(),
            }),
        };

        let projected = fact.projected_focus_entities();
        assert_eq!(projected[0].kind, "web_resource");
        assert_eq!(projected[0].name, "https://example.com/docs");
        assert_eq!(projected[0].metadata.as_deref(), Some("example.com"));
        let subjects = fact.projected_subjects();
        assert!(subjects
            .iter()
            .any(|subject| subject == "https://example.com/docs"));
        assert!(subjects.iter().any(|subject| subject == "example.com"));
    }

    #[test]
    fn schedule_fact_projects_job_and_session_targets() {
        let fact = TypedToolFact {
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
                    delivery: Some(ConversationDeliveryTarget::Explicit {
                        channel: "matrix".into(),
                        recipient: "!room:example.org".into(),
                        thread_ref: Some("$thread".into()),
                    }),
                }),
                run_count: Some(4),
                last_status: Some("ok".into()),
                last_duration_ms: Some(250),
            }),
        };

        let projected = fact.projected_focus_entities();
        assert!(projected
            .iter()
            .any(|entity| entity.kind == "scheduled_job" && entity.name == "job_123"));
        assert!(projected
            .iter()
            .any(|entity| entity.kind == "session_target" && entity.name == "main"));
        assert!(projected
            .iter()
            .any(|entity| entity.kind == "delivery_target" && entity.name == "!room:example.org"));
        assert!(projected
            .iter()
            .any(|entity| entity.kind == "run_history" && entity.name == "4"));
        let subjects = fact.projected_subjects();
        assert!(subjects.iter().any(|subject| subject == "job_123"));
        assert!(subjects.iter().any(|subject| subject == "Europe/Berlin"));
        assert!(subjects.iter().any(|subject| subject == "main"));
        assert!(subjects.iter().any(|subject| subject == "matrix"));
        assert!(subjects
            .iter()
            .any(|subject| subject == "!room:example.org"));
        assert!(subjects.iter().any(|subject| subject == "ok"));
    }

    #[test]
    fn routing_and_security_facts_project_stable_focus() {
        let routing = TypedToolFact {
            tool_id: "model_routing_config".into(),
            payload: ToolFactPayload::Routing(RoutingFact {
                action: RoutingAction::UpsertAgent,
                preset: None,
                hint: Some("deploy".into()),
                agent_name: Some("publisher".into()),
                provider: Some("openrouter".into()),
                model: Some("qwen".into()),
                matcher_count: Some(3),
                allowed_tool_count: Some(5),
            }),
        };
        let security = TypedToolFact {
            tool_id: "security_ops".into(),
            payload: ToolFactPayload::Security(SecurityFact {
                action: SecurityAction::RunPlaybook,
                severity: Some("high".into()),
                subject: Some("credential_stuffing".into()),
                step: Some(2),
                client_name: None,
            }),
        };

        let routing_projected = routing.projected_focus_entities();
        assert_eq!(routing_projected[0].kind, "routing_agent");
        assert_eq!(routing_projected[0].name, "publisher");
        let security_projected = security.projected_focus_entities();
        assert_eq!(security_projected[0].kind, "security_subject");
        assert_eq!(security_projected[0].name, "credential_stuffing");
        assert_eq!(
            security_projected[0].metadata.as_deref(),
            Some("run_playbook")
        );
    }
}
