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
pub struct DeliveryFact {
    pub target: DeliveryTargetKind,
    pub content_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryTargetKind {
    CurrentConversation,
    Explicit(ConversationDeliveryTarget),
    ProfileDefault(ConversationDeliveryTarget),
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
    pub timezone: Option<String>,
    pub target: Option<ScheduleTarget>,
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
}
