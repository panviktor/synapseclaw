//! Implicit memory recall — bounded pre-tool retrieval prior.
//!
//! This service turns durable memory into a compact runtime hint before broad
//! discovery. It does not dump raw memory into the prompt. It produces:
//! - a bounded guidance block for the live turn when useful
//! - redacted trace decisions/notes for diagnostics
//! - verification-first policy for stale or mutable facts

use crate::application::services::runtime_decision_trace::{
    RuntimeTraceMemoryDecision, RuntimeTraceNote,
};
use crate::application::services::turn_interpretation::{
    ReferenceCandidateKind, ReferenceSource, TurnInterpretation,
};
use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryQuery, SearchResult};
use crate::ports::memory::UnifiedMemoryPort;
use chrono::{DateTime, Duration, Utc};

const DEFAULT_RECALL_LIMIT: usize = 8;
const DEFAULT_ACCEPT_LIMIT: usize = 2;
const MAX_GUIDANCE_HINTS: usize = 2;
const STALE_INFRA_DAYS: i64 = 30;
const STALE_PROJECT_DAYS: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplicitMemoryScope {
    Core,
    Project,
    LocalInfra,
    Procedural,
    RecentSuccess,
}

impl ImplicitMemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Project => "project",
            Self::LocalInfra => "local_infra",
            Self::Procedural => "procedural",
            Self::RecentSuccess => "recent_success",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplicitMemoryVerificationPolicy {
    None,
    MinimalLiveVerification,
    VerificationFirst,
}

impl ImplicitMemoryVerificationPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::MinimalLiveVerification => "minimal_live_verification",
            Self::VerificationFirst => "verification_first",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImplicitMemoryRecallInput<'a> {
    pub agent_id: &'a str,
    pub user_message: &'a str,
    pub conversation_key: Option<&'a str>,
    pub interpretation: Option<&'a TurnInterpretation>,
    pub min_relevance_score: f64,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplicitMemoryRecallAcceptedAnchor {
    pub entry_id: String,
    pub key: String,
    pub scope: ImplicitMemoryScope,
    pub verification_policy: ImplicitMemoryVerificationPolicy,
    pub stale: bool,
    pub tool_prior_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplicitMemoryRecallRejectedCandidate {
    pub entry_id: String,
    pub key: String,
    pub scope: ImplicitMemoryScope,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplicitMemoryRecallPlan {
    pub query: String,
    pub scopes: Vec<ImplicitMemoryScope>,
    pub verification_policy: ImplicitMemoryVerificationPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplicitMemoryRecallOutcome {
    pub attempted: bool,
    pub plan: ImplicitMemoryRecallPlan,
    pub accepted: Vec<ImplicitMemoryRecallAcceptedAnchor>,
    pub rejected: Vec<ImplicitMemoryRecallRejectedCandidate>,
    pub guidance_block: Option<String>,
    pub runtime_memory_decisions: Vec<RuntimeTraceMemoryDecision>,
    pub runtime_notes: Vec<RuntimeTraceNote>,
}

pub async fn execute_implicit_memory_recall(
    memory: Option<&dyn UnifiedMemoryPort>,
    input: ImplicitMemoryRecallInput<'_>,
) -> ImplicitMemoryRecallOutcome {
    let scopes = infer_scopes(input.interpretation);
    let plan = ImplicitMemoryRecallPlan {
        query: build_query(input.user_message, input.interpretation),
        scopes: scopes.clone(),
        verification_policy: plan_verification_policy(&scopes),
    };

    let mut outcome = ImplicitMemoryRecallOutcome {
        attempted: memory.is_some(),
        plan,
        accepted: Vec::new(),
        rejected: Vec::new(),
        guidance_block: None,
        runtime_memory_decisions: Vec::new(),
        runtime_notes: Vec::new(),
    };

    let Some(memory) = memory else {
        outcome.runtime_notes.push(RuntimeTraceNote {
            observed_at_unix: input.now.timestamp(),
            kind: "implicit_memory_recall".into(),
            detail: "attempted=false reason=no_memory_backend".into(),
        });
        return outcome;
    };

    let query = MemoryQuery {
        text: outcome.plan.query.clone(),
        embedding: None,
        agent_id: input.agent_id.to_string(),
        categories: plan_categories(&scopes),
        include_shared: false,
        time_range: None,
        limit: DEFAULT_RECALL_LIMIT,
    };

    let search = memory.hybrid_search(&query).await;
    let Ok(search) = search else {
        outcome.runtime_notes.push(RuntimeTraceNote {
            observed_at_unix: input.now.timestamp(),
            kind: "implicit_memory_recall".into(),
            detail: "attempted=true outcome=memory_error".into(),
        });
        return outcome;
    };

    let mut candidates = search
        .episodes
        .into_iter()
        .filter_map(|candidate| classify_candidate(candidate, &input))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));

    let mut seen_keys = std::collections::BTreeMap::<String, String>::new();
    for candidate in candidates {
        let verification_policy = determine_verification_policy(&candidate.entry, candidate.scope, &input);
        let tool_prior_hint = build_tool_prior_hint(&candidate.entry, candidate.scope, verification_policy);
        let conflict = seen_keys
            .get(candidate.entry.key.as_str())
            .is_some_and(|existing| existing != &candidate.entry.content);
        let accept = !conflict
            && candidate.score >= input.min_relevance_score
            && !is_rejectable_noise(&candidate.entry, candidate.scope)
            && outcome.accepted.len() < DEFAULT_ACCEPT_LIMIT;

        if accept {
            seen_keys.insert(candidate.entry.key.clone(), candidate.entry.content.clone());
            outcome.runtime_memory_decisions.push(RuntimeTraceMemoryDecision {
                observed_at_unix: input.now.timestamp(),
                source: "implicit_memory_recall".into(),
                category: candidate.scope.as_str().into(),
                write_class: None,
                action: "recall_accept".into(),
                applied: true,
                entry_id_present: true,
                reason: format!(
                    "accepted key={} verification_policy={}",
                    redact_token(&candidate.entry.key),
                    verification_policy.as_str()
                ),
                similarity_basis_points: Some(similarity_basis_points(candidate.score)),
                failure: None,
            });
            outcome.accepted.push(ImplicitMemoryRecallAcceptedAnchor {
                entry_id: candidate.entry.id.clone(),
                key: candidate.entry.key.clone(),
                scope: candidate.scope,
                verification_policy,
                stale: is_stale(&candidate.entry, candidate.scope, input.now),
                tool_prior_hint,
            });
        } else {
            let reason = if conflict {
                "conflicting_candidate".to_string()
            } else if candidate.score < input.min_relevance_score {
                "low_confidence".to_string()
            } else {
                "rejected_noise".to_string()
            };
            outcome.runtime_memory_decisions.push(RuntimeTraceMemoryDecision {
                observed_at_unix: input.now.timestamp(),
                source: "implicit_memory_recall".into(),
                category: candidate.scope.as_str().into(),
                write_class: None,
                action: "recall_reject".into(),
                applied: false,
                entry_id_present: true,
                reason: format!(
                    "{} key={}",
                    reason,
                    redact_token(&candidate.entry.key)
                ),
                similarity_basis_points: Some(similarity_basis_points(candidate.score)),
                failure: None,
            });
            outcome.rejected.push(ImplicitMemoryRecallRejectedCandidate {
                entry_id: candidate.entry.id.clone(),
                key: candidate.entry.key.clone(),
                scope: candidate.scope,
                reason,
            });
        }
    }

    outcome.guidance_block = format_guidance_block(&outcome);
    outcome.runtime_notes.push(RuntimeTraceNote {
        observed_at_unix: input.now.timestamp(),
        kind: "implicit_memory_recall".into(),
        detail: format!(
            "attempted=true query={} scopes={} accepted={} rejected={} verification_policy={}",
            redact_trace_query(&outcome.plan.query),
            outcome
                .plan
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(","),
            outcome.accepted.len(),
            outcome.rejected.len(),
            outcome.plan.verification_policy.as_str()
        ),
    });
    outcome.runtime_notes.push(RuntimeTraceNote {
        observed_at_unix: input.now.timestamp(),
        kind: "implicit_memory_context".into(),
        detail: format!(
            "guidance_chars={} accepted_anchors={}",
            outcome
                .guidance_block
                .as_ref()
                .map(|block| block.chars().count())
                .unwrap_or(0),
            outcome.accepted.len()
        ),
    });
    outcome
}

#[derive(Debug, Clone)]
struct ClassifiedCandidate {
    entry: MemoryEntry,
    scope: ImplicitMemoryScope,
    score: f64,
}

fn infer_scopes(interpretation: Option<&TurnInterpretation>) -> Vec<ImplicitMemoryScope> {
    let mut scopes = vec![ImplicitMemoryScope::Core];
    let Some(interpretation) = interpretation else {
        scopes.push(ImplicitMemoryScope::Project);
        scopes.push(ImplicitMemoryScope::LocalInfra);
        scopes.push(ImplicitMemoryScope::Procedural);
        scopes.push(ImplicitMemoryScope::RecentSuccess);
        return dedupe_scopes(scopes);
    };

    if interpretation.user_profile.is_some() {
        scopes.push(ImplicitMemoryScope::Project);
    }
    if interpretation.dialogue_state.as_ref().is_some_and(|state| state.recent_workspace.is_some())
    {
        scopes.push(ImplicitMemoryScope::Project);
    }
    if interpretation.dialogue_state.as_ref().is_some_and(|state| {
        state.recent_resource.is_some() || !state.last_tool_subjects.is_empty()
    }) {
        scopes.push(ImplicitMemoryScope::LocalInfra);
    }
    if interpretation.reference_candidates.iter().any(|candidate| {
        matches!(
            candidate.kind,
            ReferenceCandidateKind::WorkspaceName { .. }
                | ReferenceCandidateKind::ResourceLocator { .. }
                | ReferenceCandidateKind::RecentSubject
                | ReferenceCandidateKind::SearchQuery { .. }
                | ReferenceCandidateKind::SearchResult { .. }
        ) && matches!(
            candidate.source,
            ReferenceSource::DialogueState | ReferenceSource::CurrentConversation
        )
    }) {
        scopes.push(ImplicitMemoryScope::LocalInfra);
        scopes.push(ImplicitMemoryScope::Project);
    }
    dedupe_scopes(scopes)
}

fn build_query(user_message: &str, interpretation: Option<&TurnInterpretation>) -> String {
    let mut terms = vec![user_message.trim().to_string()];
    if let Some(interpretation) = interpretation {
        for candidate in interpretation.reference_candidates.iter().take(6) {
            if !candidate.value.trim().is_empty() {
                terms.push(candidate.value.trim().to_string());
            }
        }
        if let Some(state) = interpretation.dialogue_state.as_ref() {
            for subject in state.last_tool_subjects.iter().take(4) {
                if !subject.trim().is_empty() {
                    terms.push(subject.trim().to_string());
                }
            }
        }
    }
    terms.join(" | ")
}

fn plan_categories(scopes: &[ImplicitMemoryScope]) -> Vec<MemoryCategory> {
    let mut categories = vec![MemoryCategory::Core];
    for scope in scopes {
        match scope {
            ImplicitMemoryScope::Core => {}
            ImplicitMemoryScope::Project => categories.push(MemoryCategory::Custom("project".into())),
            ImplicitMemoryScope::LocalInfra => {
                categories.push(MemoryCategory::Custom("local_infra".into()))
            }
            ImplicitMemoryScope::Procedural => {
                categories.push(MemoryCategory::Custom("procedural".into()));
                categories.push(MemoryCategory::Skill);
            }
            ImplicitMemoryScope::RecentSuccess => {
                categories.push(MemoryCategory::Custom("recent_success".into()))
            }
        }
    }
    categories
}

fn plan_verification_policy(scopes: &[ImplicitMemoryScope]) -> ImplicitMemoryVerificationPolicy {
    if scopes.contains(&ImplicitMemoryScope::LocalInfra) || scopes.contains(&ImplicitMemoryScope::Project) {
        return ImplicitMemoryVerificationPolicy::MinimalLiveVerification;
    }
    ImplicitMemoryVerificationPolicy::None
}

fn classify_candidate(
    candidate: SearchResult,
    _input: &ImplicitMemoryRecallInput<'_>,
) -> Option<ClassifiedCandidate> {
    let score = candidate.entry.score.unwrap_or(candidate.score as f64);
    let scope = scope_for_entry(&candidate.entry)?;
    Some(ClassifiedCandidate {
        entry: candidate.entry,
        scope,
        score,
    })
}

fn scope_for_entry(entry: &MemoryEntry) -> Option<ImplicitMemoryScope> {
    let lower_key = entry.key.to_lowercase();
    let lower_category = entry.category.to_string().to_lowercase();
    let lower_content = entry.content.to_lowercase();

    if lower_category == "core" {
        return Some(ImplicitMemoryScope::Core);
    }
    if lower_category == "skill" || lower_category == "procedural" || lower_key.contains("procedure") {
        return Some(ImplicitMemoryScope::Procedural);
    }
    if lower_category == "recent_success" || lower_key.contains("success") {
        return Some(ImplicitMemoryScope::RecentSuccess);
    }
    if lower_category == "project" || contains_any(&lower_key, &["project", "branch", "staging"]) {
        return Some(ImplicitMemoryScope::Project);
    }
    if lower_category == "local_infra"
        || contains_any(&lower_key, &["matrix", "homeserver", "server", "service", "package"])
        || contains_any(&lower_content, &[".service", "/etc/", "systemctl", "package "])
    {
        return Some(ImplicitMemoryScope::LocalInfra);
    }
    None
}

fn determine_verification_policy(
    entry: &MemoryEntry,
    scope: ImplicitMemoryScope,
    input: &ImplicitMemoryRecallInput<'_>,
) -> ImplicitMemoryVerificationPolicy {
    let lower = entry.content.to_lowercase();
    if contains_any(
        &lower,
        &["version", "latest", "current", "installed", "running", "status", "release", "url", "path"],
    ) {
        return ImplicitMemoryVerificationPolicy::VerificationFirst;
    }
    if is_stale(entry, scope, input.now) {
        return ImplicitMemoryVerificationPolicy::VerificationFirst;
    }
    match scope {
        ImplicitMemoryScope::LocalInfra | ImplicitMemoryScope::Project => {
            ImplicitMemoryVerificationPolicy::MinimalLiveVerification
        }
        _ => ImplicitMemoryVerificationPolicy::None,
    }
}

fn is_stale(entry: &MemoryEntry, scope: ImplicitMemoryScope, now: DateTime<Utc>) -> bool {
    let Ok(parsed) = DateTime::parse_from_rfc3339(&entry.timestamp) else {
        return false;
    };
    let parsed = parsed.with_timezone(&Utc);
    let age = now.signed_duration_since(parsed);
    match scope {
        ImplicitMemoryScope::LocalInfra => age > Duration::days(STALE_INFRA_DAYS),
        ImplicitMemoryScope::Project => age > Duration::days(STALE_PROJECT_DAYS),
        _ => false,
    }
}

fn is_rejectable_noise(entry: &MemoryEntry, scope: ImplicitMemoryScope) -> bool {
    matches!(entry.category, MemoryCategory::Conversation)
        && !matches!(scope, ImplicitMemoryScope::Core)
        && entry.content.split_whitespace().count() < 4
}

fn build_tool_prior_hint(
    entry: &MemoryEntry,
    scope: ImplicitMemoryScope,
    verification_policy: ImplicitMemoryVerificationPolicy,
) -> Option<String> {
    let narrow_target = extract_narrow_target(entry);
    match scope {
        ImplicitMemoryScope::LocalInfra => Some(match (narrow_target, verification_policy) {
            (Some(target), ImplicitMemoryVerificationPolicy::VerificationFirst) => format!(
                "Verify remembered local infrastructure target `{}` first; do not trust mutable details without live check and avoid broad host inventory.",
                target
            ),
            (Some(target), _) => format!(
                "Verify remembered local infrastructure target `{}` first and avoid broad host inventory.",
                target
            ),
            (None, _) => {
                "Use remembered local infrastructure anchor for narrow verification before wide discovery.".into()
            }
        }),
        ImplicitMemoryScope::Project => Some(
            "Use remembered project/branch/staging anchor for narrow verification before broad discovery."
                .into(),
        ),
        ImplicitMemoryScope::Procedural | ImplicitMemoryScope::RecentSuccess => Some(
            "Use the recalled procedural anchor as a starting prior, not as final truth.".into(),
        ),
        ImplicitMemoryScope::Core => None,
    }
}

fn extract_narrow_target(entry: &MemoryEntry) -> Option<String> {
    let content_tokens = tokenize(&entry.content);
    content_tokens
        .iter()
        .find(|token| token.ends_with(".service") || token.starts_with('/') || token.contains("://"))
        .cloned()
        .or_else(|| {
            tokenize(&entry.key)
                .into_iter()
                .find(|token| token.ends_with(".service"))
        })
}

fn format_guidance_block(outcome: &ImplicitMemoryRecallOutcome) -> Option<String> {
    if outcome.accepted.is_empty() {
        return None;
    }
    let mut lines = vec![
        "[implicit-memory-recall]".to_string(),
        format!("- accepted_anchors: {}", outcome.accepted.len()),
        format!(
            "- verification_policy: {}",
            outcome.plan.verification_policy.as_str()
        ),
        "- keep_memory_payload_bounded: true".to_string(),
    ];
    for hint in outcome
        .accepted
        .iter()
        .filter_map(|anchor| anchor.tool_prior_hint.as_deref())
        .take(MAX_GUIDANCE_HINTS)
    {
        lines.push(format!("- tool_prior: {hint}"));
    }
    Some(format!("{}\n", lines.join("\n")))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn dedupe_scopes(scopes: Vec<ImplicitMemoryScope>) -> Vec<ImplicitMemoryScope> {
    let mut out = Vec::new();
    for scope in scopes {
        if !out.contains(&scope) {
            out.push(scope);
        }
    }
    out
}

fn redact_trace_query(query: &str) -> String {
    query.split('|')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn redact_token(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.len() <= 48 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..48])
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch: char| !ch.is_alphanumeric() && !matches!(ch, '.' | '/' | ':' | '_' | '-'))
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

fn similarity_basis_points(score: f64) -> u32 {
    (score.clamp(0.0, 1.0) * 10_000.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingProfile, Entity, HybridSearchResult,
        MemoryError, MemoryId, Reflection, SearchSource, SessionId, Skill, SkillUpdate, TemporalFact,
        Visibility,
    };
    use async_trait::async_trait;

    #[derive(Default)]
    struct StubMemory {
        hybrid: HybridSearchResult,
    }

    #[async_trait]
    impl crate::ports::memory::WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> { Ok(vec![]) }
        async fn update_core_block(&self, _: &AgentId, _: &str, _: String) -> Result<(), MemoryError> { Ok(()) }
        async fn append_core_block(&self, _: &AgentId, _: &str, _: &str) -> Result<(), MemoryError> { Ok(()) }
    }
    #[async_trait]
    impl crate::ports::memory::EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> { Ok(vec![]) }
    }
    #[async_trait]
    impl crate::ports::memory::SemanticMemoryPort for StubMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> { Ok(None) }
        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> { Ok(()) }
        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> { Ok(vec![]) }
        async fn traverse(&self, _: &MemoryId, _: usize) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> { Ok(vec![]) }
        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> { Ok(vec![]) }
    }
    #[async_trait]
    impl crate::ports::memory::SkillMemoryPort for StubMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> { Ok(vec![]) }
        async fn update_skill(&self, _: &MemoryId, _: SkillUpdate, _: &AgentId) -> Result<(), MemoryError> { Ok(()) }
        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> { Ok(None) }
    }
    #[async_trait]
    impl crate::ports::memory::ReflectionPort for StubMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> { Ok(String::new()) }
        async fn get_relevant_reflections(&self, _: &MemoryQuery) -> Result<Vec<Reflection>, MemoryError> { Ok(vec![]) }
        async fn get_failure_patterns(&self, _: &AgentId, _: usize) -> Result<Vec<Reflection>, MemoryError> { Ok(vec![]) }
    }
    #[async_trait]
    impl crate::ports::memory::ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> { Ok(ConsolidationReport::default()) }
        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> { Ok(0) }
        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> { Ok(0) }
    }
    #[async_trait]
    impl UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> { Ok(self.hybrid.clone()) }
        async fn embed(&self, _: &str) -> Result<Vec<f32>, MemoryError> { Ok(vec![0.1]) }
        async fn store(&self, _: &str, _: &str, _: &MemoryCategory, _: Option<&str>) -> Result<(), MemoryError> { Ok(()) }
        async fn recall(&self, _: &str, _: usize, _: Option<&str>) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> { Ok(()) }
        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> { Ok(false) }
        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> { Ok(None) }
        async fn list(&self, _: Option<&MemoryCategory>, _: Option<&str>, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> { Ok(vec![]) }
        async fn count(&self) -> Result<usize, MemoryError> { Ok(0) }
        fn name(&self) -> &str { "stub" }
        async fn health_check(&self) -> bool { true }
        fn embedding_profile(&self) -> EmbeddingProfile { EmbeddingProfile::default() }
        async fn promote_visibility(&self, _: &MemoryId, _: &Visibility, _: &[AgentId], _: &AgentId) -> Result<(), MemoryError> { Ok(()) }
    }

    fn entry(
        id: &str,
        key: &str,
        category: MemoryCategory,
        content: &str,
        timestamp: &str,
        score: f32,
    ) -> SearchResult {
        SearchResult {
            entry: MemoryEntry {
                id: id.into(),
                key: key.into(),
                content: content.into(),
                category,
                timestamp: timestamp.into(),
                session_id: None,
                score: Some(score as f64),
            },
            score,
            source: SearchSource::Hybrid,
        }
    }

    #[tokio::test]
    async fn recalls_local_infra_without_memory_phrase_and_builds_narrow_prior() {
        let memory = StubMemory {
            hybrid: HybridSearchResult {
                episodes: vec![entry(
                    "1",
                    "local_infra_matrix_homeserver",
                    MemoryCategory::Custom("local_infra".into()),
                    "Our self-hosted Matrix homeserver runs as tuwunel.service package tuwunel.",
                    "2026-04-20T00:00:00Z",
                    0.96,
                )],
                ..Default::default()
            },
        };

        let outcome = execute_implicit_memory_recall(
            Some(&memory),
            ImplicitMemoryRecallInput {
                agent_id: "agent",
                user_message: "What is our self-hosted Matrix server?",
                conversation_key: Some("room"),
                interpretation: None,
                min_relevance_score: 0.5,
                now: Utc::now(),
            },
        )
        .await;

        assert_eq!(outcome.accepted.len(), 1);
        assert!(outcome.guidance_block.as_deref().unwrap_or("").contains("tuwunel.service"));
        assert!(outcome.guidance_block.as_deref().unwrap_or("").contains("avoid broad host inventory"));
    }

    #[tokio::test]
    async fn stale_version_memory_requires_verification_first() {
        let memory = StubMemory {
            hybrid: HybridSearchResult {
                episodes: vec![entry(
                    "1",
                    "local_infra_matrix_version",
                    MemoryCategory::Custom("local_infra".into()),
                    "Matrix stack package version 1.5.0 installed on tuwunel.service",
                    "2025-12-01T00:00:00Z",
                    0.91,
                )],
                ..Default::default()
            },
        };

        let outcome = execute_implicit_memory_recall(
            Some(&memory),
            ImplicitMemoryRecallInput {
                agent_id: "agent",
                user_message: "Are we on the latest Matrix server version?",
                conversation_key: None,
                interpretation: None,
                min_relevance_score: 0.5,
                now: DateTime::parse_from_rfc3339("2026-04-22T00:00:00Z").unwrap().with_timezone(&Utc),
            },
        )
        .await;

        assert_eq!(
            outcome.accepted[0].verification_policy,
            ImplicitMemoryVerificationPolicy::VerificationFirst
        );
        assert!(outcome.guidance_block.as_deref().unwrap_or("").contains("do not trust mutable details"));
    }

    #[tokio::test]
    async fn conflicting_candidates_are_rejected_instead_of_confidently_accepted() {
        let memory = StubMemory {
            hybrid: HybridSearchResult {
                episodes: vec![
                    entry(
                        "1",
                        "local_infra_matrix_homeserver",
                        MemoryCategory::Custom("local_infra".into()),
                        "Homeserver runs as tuwunel.service",
                        "2026-04-20T00:00:00Z",
                        0.95,
                    ),
                    entry(
                        "2",
                        "local_infra_matrix_homeserver",
                        MemoryCategory::Custom("local_infra".into()),
                        "Homeserver runs as synapse.service",
                        "2026-04-20T00:00:00Z",
                        0.94,
                    ),
                ],
                ..Default::default()
            },
        };

        let outcome = execute_implicit_memory_recall(
            Some(&memory),
            ImplicitMemoryRecallInput {
                agent_id: "agent",
                user_message: "Which Matrix server do we run?",
                conversation_key: None,
                interpretation: None,
                min_relevance_score: 0.5,
                now: Utc::now(),
            },
        )
        .await;

        assert_eq!(outcome.accepted.len(), 1);
        assert!(outcome.rejected.iter().any(|candidate| candidate.reason == "conflicting_candidate"));
    }

    #[tokio::test]
    async fn empty_recall_attempt_records_trace_without_guidance() {
        let memory = StubMemory::default();
        let outcome = execute_implicit_memory_recall(
            Some(&memory),
            ImplicitMemoryRecallInput {
                agent_id: "agent",
                user_message: "What is our self-hosted Matrix server?",
                conversation_key: None,
                interpretation: None,
                min_relevance_score: 0.5,
                now: Utc::now(),
            },
        )
        .await;

        assert!(outcome.guidance_block.is_none());
        assert!(outcome.accepted.is_empty());
        assert!(outcome
            .runtime_notes
            .iter()
            .any(|note| note.kind == "implicit_memory_recall" && note.detail.contains("accepted=0")));
    }
}
