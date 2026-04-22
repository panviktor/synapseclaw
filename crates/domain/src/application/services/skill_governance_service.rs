//! Runtime skill governance and on-demand activation policy.
//!
//! This service is deliberately pure domain logic: adapters provide candidate
//! metadata and concrete activation/read side effects, while the resolver owns
//! state, blocking, shadowing, and prompt projection policy.

use crate::domain::memory::{Skill, SkillOrigin, SkillStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Manual,
    Bundled,
    Imported,
    External,
    #[default]
    Learned,
    GeneratedPatch,
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manual => write!(f, "manual"),
            Self::Bundled => write!(f, "bundled"),
            Self::Imported => write!(f, "imported"),
            Self::External => write!(f, "external"),
            Self::Learned => write!(f, "learned"),
            Self::GeneratedPatch => write!(f, "generated_patch"),
        }
    }
}

impl From<&SkillOrigin> for SkillSource {
    fn from(origin: &SkillOrigin) -> Self {
        match origin {
            SkillOrigin::Manual => Self::Manual,
            SkillOrigin::Imported => Self::Imported,
            SkillOrigin::Learned => Self::Learned,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillTrustLevel {
    Builtin,
    Trusted,
    Community,
    #[default]
    AgentCreated,
}

impl fmt::Display for SkillTrustLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Trusted => write!(f, "trusted"),
            Self::Community => write!(f, "community"),
            Self::AgentCreated => write!(f, "agent_created"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillRuntimeState {
    #[default]
    Active,
    Candidate,
    Shadowed,
    Disabled,
    Incompatible,
    BlockedMissingCapability,
    NeedsSetup,
    Deprecated,
}

impl fmt::Display for SkillRuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Candidate => write!(f, "candidate"),
            Self::Shadowed => write!(f, "shadowed"),
            Self::Disabled => write!(f, "disabled"),
            Self::Incompatible => write!(f, "incompatible"),
            Self::BlockedMissingCapability => write!(f, "blocked_missing_capability"),
            Self::NeedsSetup => write!(f, "needs_setup"),
            Self::Deprecated => write!(f, "deprecated"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillActivationMode {
    Preload,
    #[default]
    CatalogOnly,
    Blocked,
    AlreadyLoaded,
    OperatorReviewRequired,
}

impl fmt::Display for SkillActivationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preload => write!(f, "preload"),
            Self::CatalogOnly => write!(f, "catalog_only"),
            Self::Blocked => write!(f, "blocked"),
            Self::AlreadyLoaded => write!(f, "already_loaded"),
            Self::OperatorReviewRequired => write!(f, "operator_review_required"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSetupRequirement {
    pub key: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCapabilityRequirement {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRuntimeCandidate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub trust_level: SkillTrustLevel,
    pub status: SkillStatus,
    pub disabled: bool,
    pub review_required: bool,
    pub task_family: Option<String>,
    pub lineage_task_families: Vec<String>,
    pub tool_pattern: Vec<String>,
    pub tags: Vec<String>,
    pub category: Option<String>,
    pub agents: Vec<String>,
    pub channels: Vec<String>,
    pub platforms: Vec<String>,
    pub required_tools: Vec<String>,
    pub required_tool_roles: Vec<String>,
    pub required_model_lanes: Vec<String>,
    pub required_modalities: Vec<String>,
    pub required_setup: Vec<SkillSetupRequirement>,
    pub source_ref: Option<String>,
    pub content_chars: usize,
    pub relevance_score: f32,
}

impl SkillRuntimeCandidate {
    pub fn from_memory_skill(skill: &Skill) -> Self {
        Self {
            id: skill_key(&skill.id, &skill.name),
            name: skill.name.clone(),
            description: skill.description.clone(),
            source: SkillSource::from(&skill.origin),
            trust_level: match skill.origin {
                SkillOrigin::Manual => SkillTrustLevel::Trusted,
                SkillOrigin::Imported => SkillTrustLevel::Community,
                SkillOrigin::Learned => SkillTrustLevel::AgentCreated,
            },
            status: skill.status.clone(),
            disabled: false,
            review_required: skill.status == SkillStatus::Candidate,
            task_family: skill.task_family.clone(),
            lineage_task_families: skill.lineage_task_families.clone(),
            tool_pattern: skill.tool_pattern.clone(),
            tags: skill.tags.clone(),
            category: None,
            agents: vec![skill.created_by.clone()],
            channels: Vec::new(),
            platforms: Vec::new(),
            required_tools: Vec::new(),
            required_tool_roles: Vec::new(),
            required_model_lanes: Vec::new(),
            required_modalities: Vec::new(),
            required_setup: Vec::new(),
            source_ref: None,
            content_chars: skill.content.chars().count(),
            relevance_score: 0.0,
        }
    }

    pub fn activation_id(&self) -> String {
        skill_key(&self.id, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPromptBudget {
    pub max_catalog_entries: usize,
    pub max_preloaded_skills: usize,
    pub max_skill_chars: usize,
}

impl Default for SkillPromptBudget {
    fn default() -> Self {
        Self {
            max_catalog_entries: 8,
            max_preloaded_skills: 1,
            max_skill_chars: 2_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillLoadRequest {
    pub agent_id: String,
    pub session_id: Option<String>,
    pub channel: Option<String>,
    pub platform: Option<String>,
    pub task_text: String,
    pub task_family: Option<String>,
    pub category: Option<String>,
    pub explicit_skill: Option<String>,
    pub available_tools: Vec<String>,
    pub available_tool_roles: Vec<String>,
    pub available_model_lanes: Vec<String>,
    pub available_modalities: Vec<String>,
    pub configured_setup_keys: Vec<String>,
    pub already_activated_skill_ids: Vec<String>,
    pub embedding_available: bool,
    pub prompt_budget: SkillPromptBudget,
}

impl Default for SkillLoadRequest {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            session_id: None,
            channel: None,
            platform: Some(std::env::consts::OS.to_string()),
            task_text: String::new(),
            task_family: None,
            category: None,
            explicit_skill: None,
            available_tools: Vec::new(),
            available_tool_roles: Vec::new(),
            available_model_lanes: Vec::new(),
            available_modalities: Vec::new(),
            configured_setup_keys: Vec::new(),
            already_activated_skill_ids: Vec::new(),
            embedding_available: false,
            prompt_budget: SkillPromptBudget::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPromptProjection {
    pub id: String,
    pub name: String,
    pub description: String,
    pub state: SkillRuntimeState,
    pub source: SkillSource,
    pub activation_id: String,
    pub source_ref: Option<String>,
    pub capability_hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRuntimeDecision {
    pub id: String,
    pub name: String,
    pub state: SkillRuntimeState,
    pub activation_mode: SkillActivationMode,
    pub reason_code: String,
    pub source: SkillSource,
    pub trust_level: SkillTrustLevel,
    pub shadowed_by: Option<String>,
    pub missing_capabilities: Vec<SkillCapabilityRequirement>,
    pub setup_requirements: Vec<SkillSetupRequirement>,
    pub source_ref: Option<String>,
    pub prompt_projection: Option<SkillPromptProjection>,
    pub relevance_score: f32,
}

impl SkillRuntimeDecision {
    pub fn loadable(&self) -> bool {
        self.state == SkillRuntimeState::Active
            && matches!(
                self.activation_mode,
                SkillActivationMode::Preload
                    | SkillActivationMode::CatalogOnly
                    | SkillActivationMode::AlreadyLoaded
            )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SkillResolutionReport {
    pub decisions: Vec<SkillRuntimeDecision>,
}

impl SkillResolutionReport {
    pub fn provider_catalog(&self) -> Vec<&SkillRuntimeDecision> {
        self.decisions
            .iter()
            .filter(|decision| decision.loadable() && decision.prompt_projection.is_some())
            .collect()
    }

    pub fn runtime_preloads(&self) -> Vec<&SkillRuntimeDecision> {
        self.decisions
            .iter()
            .filter(|decision| {
                decision.state == SkillRuntimeState::Active
                    && decision.activation_mode == SkillActivationMode::Preload
            })
            .collect()
    }

    pub fn diagnostics(&self) -> Vec<&SkillRuntimeDecision> {
        self.decisions.iter().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillEvidenceKind {
    RunRecipe,
    ToolTrace,
    RepairTrace,
    UserCorrection,
    OperatorFeedback,
    EvalResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEvidenceRef {
    pub kind: SkillEvidenceKind,
    pub id: String,
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReplayEvalStatus {
    Passed,
    Failed,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReplayEvalResult {
    pub criterion: String,
    pub status: SkillReplayEvalStatus,
    pub evidence: Option<String>,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDraft {
    pub id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub task_family: Option<String>,
    pub category: Option<String>,
    pub provenance: Vec<SkillEvidenceRef>,
    pub replay_criteria: Vec<String>,
    #[serde(default)]
    pub eval_results: Vec<SkillReplayEvalResult>,
    pub status: SkillStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPatchProcedureClaim {
    pub tool_name: String,
    pub failure_kind: String,
    pub suggested_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPatchCandidate {
    pub id: String,
    pub target_skill_id: String,
    pub target_version: u32,
    pub diff_summary: String,
    pub proposed_body: String,
    #[serde(default)]
    pub procedure_claims: Vec<SkillPatchProcedureClaim>,
    pub provenance: Vec<SkillEvidenceRef>,
    pub replay_criteria: Vec<String>,
    #[serde(default)]
    pub eval_results: Vec<SkillReplayEvalResult>,
    pub status: SkillStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPatchApplyRecord {
    pub id: String,
    pub candidate_id: String,
    pub target_skill_id: String,
    pub agent_id: String,
    pub previous_version: u32,
    pub new_version: u32,
    pub rollback_skill_id: String,
    pub diff_summary: String,
    pub procedure_claims: Vec<SkillPatchProcedureClaim>,
    pub provenance: Vec<SkillEvidenceRef>,
    pub eval_reason: String,
    pub applied_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPatchRollbackRecord {
    pub id: String,
    pub apply_record_id: String,
    pub candidate_id: String,
    pub target_skill_id: String,
    pub agent_id: String,
    pub from_version: u32,
    pub restored_from_version: u32,
    pub new_version: u32,
    pub rollback_skill_id: String,
    pub reason: String,
    pub rolled_back_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBlockedTraceReason {
    pub skill_id: String,
    pub state: SkillRuntimeState,
    pub reason_code: String,
    pub shadowed_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillActivationTrace {
    pub selected_skill_ids: Vec<String>,
    #[serde(default)]
    pub loaded_skill_ids: Vec<String>,
    pub blocked_skill_ids: Vec<String>,
    #[serde(default)]
    pub blocked_reasons: Vec<SkillBlockedTraceReason>,
    pub budget_catalog_entries: usize,
    pub budget_preloaded_skills: usize,
    pub route_model: Option<String>,
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseOutcome {
    Succeeded,
    Failed,
    Repaired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillUseTrace {
    pub id: String,
    pub skill_id: String,
    pub task_family: Option<String>,
    pub route_model: Option<String>,
    pub tool_pattern: Vec<String>,
    pub outcome: SkillUseOutcome,
    pub verification: Option<String>,
    pub repair_evidence: Vec<SkillEvidenceRef>,
    pub observed_at_unix: i64,
}

pub fn resolve_skill_states(
    request: &SkillLoadRequest,
    candidates: Vec<SkillRuntimeCandidate>,
) -> SkillResolutionReport {
    let normalized_available_tools = normalized_set(&request.available_tools);
    let normalized_available_roles = normalized_set(&request.available_tool_roles);
    let normalized_available_lanes = normalized_set(&request.available_model_lanes);
    let normalized_available_modalities = normalized_set(&request.available_modalities);
    let normalized_setup = normalized_set(&request.configured_setup_keys);
    let normalized_activated = normalized_set(&request.already_activated_skill_ids);
    let explicit = request
        .explicit_skill
        .as_ref()
        .map(|value| normalize(value));

    let mut paired = candidates
        .iter()
        .map(|candidate| {
            let mut decision = base_decision(
                request,
                candidate,
                &normalized_available_tools,
                &normalized_available_roles,
                &normalized_available_lanes,
                &normalized_available_modalities,
                &normalized_setup,
                explicit.as_deref(),
                &normalized_activated,
            );
            decision.prompt_projection = build_prompt_projection(candidate, decision.state);
            (candidate, decision)
        })
        .collect::<Vec<_>>();

    let active_candidates = paired
        .iter()
        .filter(|(_, decision)| decision.state == SkillRuntimeState::Active)
        .map(|(candidate, _)| *candidate)
        .collect::<Vec<_>>();

    for (candidate, decision) in &mut paired {
        if matches!(
            decision.state,
            SkillRuntimeState::Active | SkillRuntimeState::Candidate
        ) {
            if let Some(shadowing) = active_candidates.iter().find(|other| {
                source_priority(other.source) > source_priority(candidate.source)
                    && skills_overlap(candidate, other)
            }) {
                decision.state = SkillRuntimeState::Shadowed;
                decision.activation_mode = SkillActivationMode::Blocked;
                decision.reason_code = "shadowed_by_higher_priority_active_skill".into();
                decision.shadowed_by = Some(shadowing.name.clone());
                decision.prompt_projection = None;
            }
        }
    }

    paired.sort_by(
        |(left_candidate, left_decision), (right_candidate, right_decision)| {
            state_rank(right_decision.state)
                .cmp(&state_rank(left_decision.state))
                .then_with(|| {
                    source_priority(right_candidate.source)
                        .cmp(&source_priority(left_candidate.source))
                })
                .then_with(|| {
                    right_candidate
                        .relevance_score
                        .partial_cmp(&left_candidate.relevance_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left_candidate.name.cmp(&right_candidate.name))
        },
    );

    enforce_activation_budget(request, &mut paired);

    SkillResolutionReport {
        decisions: paired.into_iter().map(|(_, decision)| decision).collect(),
    }
}

fn base_decision(
    request: &SkillLoadRequest,
    candidate: &SkillRuntimeCandidate,
    available_tools: &HashSet<String>,
    available_roles: &HashSet<String>,
    available_lanes: &HashSet<String>,
    available_modalities: &HashSet<String>,
    configured_setup: &HashSet<String>,
    explicit: Option<&str>,
    already_activated: &HashSet<String>,
) -> SkillRuntimeDecision {
    let mut decision = SkillRuntimeDecision {
        id: candidate.activation_id(),
        name: candidate.name.clone(),
        state: SkillRuntimeState::Active,
        activation_mode: SkillActivationMode::CatalogOnly,
        reason_code: "active".into(),
        source: candidate.source,
        trust_level: candidate.trust_level,
        shadowed_by: None,
        missing_capabilities: Vec::new(),
        setup_requirements: Vec::new(),
        source_ref: candidate.source_ref.clone(),
        prompt_projection: None,
        relevance_score: candidate.relevance_score,
    };

    if candidate.status == SkillStatus::Deprecated {
        decision.state = SkillRuntimeState::Deprecated;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "deprecated".into();
        return decision;
    }
    if candidate.disabled {
        decision.state = SkillRuntimeState::Disabled;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "disabled_by_policy".into();
        return decision;
    }
    if !candidate.agents.is_empty() && !matches_any(&candidate.agents, &request.agent_id) {
        decision.state = SkillRuntimeState::Incompatible;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "agent_incompatible".into();
        return decision;
    }
    if !candidate.channels.is_empty()
        && !request
            .channel
            .as_ref()
            .is_some_and(|channel| matches_any(&candidate.channels, channel))
    {
        decision.state = SkillRuntimeState::Incompatible;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "channel_incompatible".into();
        return decision;
    }
    if !candidate.platforms.is_empty()
        && !request
            .platform
            .as_ref()
            .is_some_and(|platform| matches_any(&candidate.platforms, platform))
    {
        decision.state = SkillRuntimeState::Incompatible;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "platform_incompatible".into();
        return decision;
    }

    if let (Some(candidate_category), Some(request_category)) =
        (candidate.category.as_ref(), request.category.as_ref())
    {
        if normalize(candidate_category) != normalize(request_category) {
            decision.state = SkillRuntimeState::Incompatible;
            decision.activation_mode = SkillActivationMode::Blocked;
            decision.reason_code = "category_incompatible".into();
            return decision;
        }
    }

    decision.missing_capabilities = missing_capabilities(
        candidate,
        available_tools,
        available_roles,
        available_lanes,
        available_modalities,
    );
    if !decision.missing_capabilities.is_empty() {
        decision.state = SkillRuntimeState::BlockedMissingCapability;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "missing_capability".into();
        return decision;
    }

    decision.setup_requirements = candidate
        .required_setup
        .iter()
        .filter(|requirement| !configured_setup.contains(&normalize(&requirement.key)))
        .cloned()
        .collect();
    if !decision.setup_requirements.is_empty() {
        decision.state = SkillRuntimeState::NeedsSetup;
        decision.activation_mode = SkillActivationMode::Blocked;
        decision.reason_code = "setup_required".into();
        return decision;
    }

    if candidate.status == SkillStatus::Candidate || candidate.review_required {
        decision.state = SkillRuntimeState::Candidate;
        decision.activation_mode = SkillActivationMode::OperatorReviewRequired;
        decision.reason_code = "operator_review_required".into();
        return decision;
    }

    if already_activated.contains(&normalize(&candidate.activation_id())) {
        decision.activation_mode = SkillActivationMode::AlreadyLoaded;
        decision.reason_code = "already_loaded".into();
    } else if explicit.is_some_and(|value| explicit_matches(candidate, value))
        || high_confidence_match(request, candidate)
    {
        decision.activation_mode = SkillActivationMode::Preload;
        decision.reason_code = "high_confidence_activation".into();
    }

    decision
}

fn missing_capabilities(
    candidate: &SkillRuntimeCandidate,
    available_tools: &HashSet<String>,
    available_roles: &HashSet<String>,
    available_lanes: &HashSet<String>,
    available_modalities: &HashSet<String>,
) -> Vec<SkillCapabilityRequirement> {
    let mut out = Vec::new();
    collect_missing(&candidate.required_tools, available_tools, "tool", &mut out);
    collect_missing(
        &candidate.required_tool_roles,
        available_roles,
        "tool_role",
        &mut out,
    );
    collect_missing(
        &candidate.required_model_lanes,
        available_lanes,
        "model_lane",
        &mut out,
    );
    collect_missing(
        &candidate.required_modalities,
        available_modalities,
        "modality",
        &mut out,
    );
    out
}

fn collect_missing(
    required: &[String],
    available: &HashSet<String>,
    kind: &str,
    out: &mut Vec<SkillCapabilityRequirement>,
) {
    for value in required {
        if !available.contains(&normalize(value)) {
            out.push(SkillCapabilityRequirement {
                kind: kind.to_string(),
                name: value.clone(),
            });
        }
    }
}

fn build_prompt_projection(
    candidate: &SkillRuntimeCandidate,
    state: SkillRuntimeState,
) -> Option<SkillPromptProjection> {
    if state != SkillRuntimeState::Active {
        return None;
    }
    Some(SkillPromptProjection {
        id: candidate.activation_id(),
        name: candidate.name.clone(),
        description: candidate.description.clone(),
        state,
        source: candidate.source,
        activation_id: candidate.activation_id(),
        source_ref: candidate.source_ref.clone(),
        capability_hints: capability_hints(candidate),
    })
}

fn enforce_activation_budget(
    request: &SkillLoadRequest,
    paired: &mut [(&SkillRuntimeCandidate, SkillRuntimeDecision)],
) {
    let mut preloads = 0usize;
    let mut catalog_entries = 0usize;
    for (_, decision) in paired.iter_mut() {
        if decision.state != SkillRuntimeState::Active {
            continue;
        }
        if decision.activation_mode == SkillActivationMode::Preload {
            preloads += 1;
            if preloads > request.prompt_budget.max_preloaded_skills {
                decision.activation_mode = SkillActivationMode::CatalogOnly;
                decision.reason_code = "preload_budget_exceeded".into();
            }
        }
        if decision.prompt_projection.is_some() {
            catalog_entries += 1;
            if catalog_entries > request.prompt_budget.max_catalog_entries {
                decision.prompt_projection = None;
                if decision.activation_mode == SkillActivationMode::CatalogOnly {
                    decision.reason_code = "catalog_budget_exceeded".into();
                }
            }
        }
    }
}

fn high_confidence_match(request: &SkillLoadRequest, candidate: &SkillRuntimeCandidate) -> bool {
    let Some(task_family) = request.task_family.as_ref() else {
        return false;
    };
    candidate_family_values(candidate)
        .iter()
        .any(|family| normalize(family) == normalize(task_family))
}

fn explicit_matches(candidate: &SkillRuntimeCandidate, explicit: &str) -> bool {
    normalize(&candidate.name) == explicit || normalize(&candidate.activation_id()) == explicit
}

fn skills_overlap(left: &SkillRuntimeCandidate, right: &SkillRuntimeCandidate) -> bool {
    normalize(&left.name) == normalize(&right.name)
        || families_overlap(
            &candidate_family_values(left),
            &candidate_family_values(right),
        )
        || tool_pattern_overlap(&left.tool_pattern, &right.tool_pattern) >= 0.75
}

fn families_overlap(left: &[String], right: &[String]) -> bool {
    let right = normalized_set(right);
    left.iter().any(|value| right.contains(&normalize(value)))
}

fn candidate_family_values(candidate: &SkillRuntimeCandidate) -> Vec<String> {
    let mut families = Vec::new();
    if let Some(task_family) = &candidate.task_family {
        if !task_family.trim().is_empty() {
            families.push(task_family.clone());
        }
    }
    for value in &candidate.lineage_task_families {
        if !value.trim().is_empty() && !families.iter().any(|family| family == value) {
            families.push(value.clone());
        }
    }
    families
}

fn tool_pattern_overlap(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let left = normalized_set(left);
    let right = normalized_set(right);
    let overlap = left.intersection(&right).count();
    overlap as f64 / left.len().max(right.len()) as f64
}

fn source_priority(source: SkillSource) -> u8 {
    match source {
        SkillSource::Manual | SkillSource::Bundled => 5,
        SkillSource::Imported | SkillSource::External => 4,
        SkillSource::Learned => 2,
        SkillSource::GeneratedPatch => 1,
    }
}

fn state_rank(state: SkillRuntimeState) -> u8 {
    match state {
        SkillRuntimeState::Active => 8,
        SkillRuntimeState::Candidate => 7,
        SkillRuntimeState::NeedsSetup => 6,
        SkillRuntimeState::BlockedMissingCapability => 5,
        SkillRuntimeState::Shadowed => 4,
        SkillRuntimeState::Incompatible => 3,
        SkillRuntimeState::Disabled => 2,
        SkillRuntimeState::Deprecated => 1,
    }
}

fn capability_hints(candidate: &SkillRuntimeCandidate) -> Vec<String> {
    let mut hints = Vec::new();
    hints.extend(prefixed("tool", &candidate.required_tools));
    hints.extend(prefixed("tool_role", &candidate.required_tool_roles));
    hints.extend(prefixed("model_lane", &candidate.required_model_lanes));
    hints.extend(prefixed("modality", &candidate.required_modalities));
    hints
}

fn prefixed(prefix: &str, values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("{prefix}:{value}"))
        .collect()
}

fn matches_any(values: &[String], target: &str) -> bool {
    let normalized_target = normalize(target);
    values
        .iter()
        .any(|value| normalize(value) == normalized_target || normalize(value) == "*")
}

fn normalized_set(values: &[String]) -> HashSet<String> {
    values.iter().map(|value| normalize(value)).collect()
}

fn skill_key(id: &str, name: &str) -> String {
    if id.trim().is_empty() {
        normalize(name)
    } else {
        normalize(id)
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str, source: SkillSource) -> SkillRuntimeCandidate {
        SkillRuntimeCandidate {
            id: name.to_string(),
            name: name.to_string(),
            description: format!("{name} skill"),
            source,
            trust_level: SkillTrustLevel::Trusted,
            status: SkillStatus::Active,
            disabled: false,
            review_required: false,
            task_family: None,
            lineage_task_families: Vec::new(),
            tool_pattern: Vec::new(),
            tags: Vec::new(),
            category: None,
            agents: Vec::new(),
            channels: Vec::new(),
            platforms: Vec::new(),
            required_tools: Vec::new(),
            required_tool_roles: Vec::new(),
            required_model_lanes: Vec::new(),
            required_modalities: Vec::new(),
            required_setup: Vec::new(),
            source_ref: None,
            content_chars: 100,
            relevance_score: 0.0,
        }
    }

    fn request() -> SkillLoadRequest {
        SkillLoadRequest {
            agent_id: "agent".into(),
            channel: Some("web".into()),
            platform: Some("linux".into()),
            prompt_budget: SkillPromptBudget {
                max_catalog_entries: 8,
                max_preloaded_skills: 2,
                max_skill_chars: 2_000,
            },
            ..SkillLoadRequest::default()
        }
    }

    fn decision<'a>(report: &'a SkillResolutionReport, name: &str) -> &'a SkillRuntimeDecision {
        report.decisions.iter().find(|d| d.name == name).unwrap()
    }

    #[test]
    fn manual_active_skill_shadows_learned_same_task_family() {
        let mut manual = candidate("manual-deploy", SkillSource::Manual);
        manual.task_family = Some("deploy".into());
        let mut learned = candidate("learned-deploy", SkillSource::Learned);
        learned.task_family = Some("deploy".into());

        let report = resolve_skill_states(&request(), vec![learned, manual]);

        assert_eq!(
            decision(&report, "manual-deploy").state,
            SkillRuntimeState::Active
        );
        let learned = decision(&report, "learned-deploy");
        assert_eq!(learned.state, SkillRuntimeState::Shadowed);
        assert_eq!(learned.shadowed_by.as_deref(), Some("manual-deploy"));
    }

    #[test]
    fn imported_skill_shadows_learned_by_name_and_task_family() {
        let mut imported = candidate("matrix-upgrade", SkillSource::Imported);
        imported.task_family = Some("matrix-upgrade".into());
        let mut learned = candidate("matrix-upgrade", SkillSource::Learned);
        learned.task_family = Some("matrix-upgrade".into());

        let report = resolve_skill_states(&request(), vec![learned, imported]);

        assert_eq!(
            decision(&report, "matrix-upgrade").source,
            SkillSource::Imported
        );
        let shadowed = report
            .decisions
            .iter()
            .find(|decision| decision.source == SkillSource::Learned)
            .unwrap();
        assert_eq!(shadowed.state, SkillRuntimeState::Shadowed);
        assert_eq!(shadowed.shadowed_by.as_deref(), Some("matrix-upgrade"));
    }

    #[test]
    fn disabled_skill_is_not_active_for_web_or_channel() {
        let mut skill = candidate("disabled", SkillSource::Manual);
        skill.disabled = true;
        skill.channels = vec!["web".into(), "matrix".into()];

        let report = resolve_skill_states(&request(), vec![skill]);

        let decision = decision(&report, "disabled");
        assert_eq!(decision.state, SkillRuntimeState::Disabled);
        assert_eq!(decision.activation_mode, SkillActivationMode::Blocked);
    }

    #[test]
    fn missing_tool_capability_blocks_with_clear_reason() {
        let mut skill = candidate("browser-flow", SkillSource::Manual);
        skill.required_tools = vec!["browser".into()];

        let report = resolve_skill_states(&request(), vec![skill]);

        let decision = decision(&report, "browser-flow");
        assert_eq!(decision.state, SkillRuntimeState::BlockedMissingCapability);
        assert_eq!(decision.reason_code, "missing_capability");
        assert_eq!(decision.missing_capabilities[0].name, "browser");
    }

    #[test]
    fn missing_tool_role_blocks_with_clear_reason() {
        let mut skill = candidate("workspace-probe", SkillSource::Manual);
        skill.required_tool_roles = vec!["workspace_discovery".into()];

        let report = resolve_skill_states(&request(), vec![skill]);

        let decision = decision(&report, "workspace-probe");
        assert_eq!(decision.state, SkillRuntimeState::BlockedMissingCapability);
        assert_eq!(decision.reason_code, "missing_capability");
        assert_eq!(decision.missing_capabilities[0].kind, "tool_role");
        assert_eq!(decision.missing_capabilities[0].name, "workspace_discovery");
    }

    #[test]
    fn candidate_skill_visible_in_diagnostics_not_catalog() {
        let mut skill = candidate("candidate", SkillSource::Learned);
        skill.status = SkillStatus::Candidate;

        let report = resolve_skill_states(&request(), vec![skill]);

        let decision = decision(&report, "candidate");
        assert_eq!(decision.state, SkillRuntimeState::Candidate);
        assert_eq!(
            decision.activation_mode,
            SkillActivationMode::OperatorReviewRequired
        );
        assert!(report.provider_catalog().is_empty());
        assert_eq!(report.diagnostics().len(), 1);
    }

    #[test]
    fn explicit_activation_cannot_bypass_missing_setup() {
        let mut skill = candidate("needs-key", SkillSource::Manual);
        skill.required_setup = vec![SkillSetupRequirement {
            key: "API_KEY".into(),
            description: None,
        }];
        let mut request = request();
        request.explicit_skill = Some("needs-key".into());

        let report = resolve_skill_states(&request, vec![skill]);

        let decision = decision(&report, "needs-key");
        assert_eq!(decision.state, SkillRuntimeState::NeedsSetup);
        assert_eq!(decision.activation_mode, SkillActivationMode::Blocked);
    }

    #[test]
    fn exact_task_family_preloads_active_skill() {
        let mut skill = candidate("release-audit", SkillSource::Manual);
        skill.task_family = Some("release_audit".into());
        let mut request = request();
        request.task_family = Some("release_audit".into());

        let report = resolve_skill_states(&request, vec![skill]);

        assert_eq!(
            decision(&report, "release-audit").activation_mode,
            SkillActivationMode::Preload
        );
        assert_eq!(report.runtime_preloads().len(), 1);
    }

    #[test]
    fn already_loaded_skill_is_not_injected_again() {
        let skill = candidate("matrix-audit", SkillSource::Manual);
        let mut request = request();
        request.already_activated_skill_ids = vec!["matrix-audit".into()];

        let report = resolve_skill_states(&request, vec![skill]);

        assert_eq!(
            decision(&report, "matrix-audit").activation_mode,
            SkillActivationMode::AlreadyLoaded
        );
    }

    #[test]
    fn platform_mismatch_is_incompatible() {
        let mut skill = candidate("macos-only", SkillSource::Manual);
        skill.platforms = vec!["macos".into()];
        let mut request = request();
        request.platform = Some("linux".into());

        let report = resolve_skill_states(&request, vec![skill]);

        assert_eq!(
            decision(&report, "macos-only").state,
            SkillRuntimeState::Incompatible
        );
    }

    #[test]
    fn category_mismatch_is_incompatible() {
        let mut skill = candidate("calendar-skill", SkillSource::Manual);
        skill.category = Some("calendar".into());
        let mut request = request();
        request.category = Some("coding".into());

        let report = resolve_skill_states(&request, vec![skill]);
        let decision = decision(&report, "calendar-skill");

        assert_eq!(decision.state, SkillRuntimeState::Incompatible);
        assert_eq!(decision.reason_code, "category_incompatible");
    }
}
