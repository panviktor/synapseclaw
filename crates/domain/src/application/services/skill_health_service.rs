//! Deterministic health report for the procedural skill catalog.
//!
//! This service is intentionally read-only. It turns existing skill metadata,
//! live use traces, and review decisions into a compact operator report so
//! cleanup can be audited before any lifecycle mutation is applied elsewhere.

use crate::application::services::skill_governance_service::{
    SkillActivationTrace, SkillPatchRollbackRecord, SkillUseOutcome, SkillUseTrace,
};
use crate::application::services::skill_review_service::{SkillReviewAction, SkillReviewDecision};
use crate::domain::memory::{AgentId, MemoryId, Skill, SkillOrigin, SkillStatus};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const STALE_CANDIDATE_SECS: i64 = 60 * 60 * 24 * 30;
const OVERSIZED_SKILL_BODY_CHARS: usize = 12_000;
const FAILURE_DOMINANT_TRACE_THRESHOLD: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillHealthSeverity {
    Healthy,
    Watch,
    Review,
    Deprecated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillHealthSignal {
    Deprecated,
    PendingReviewDecision,
    UnusedActiveSkill,
    StaleCandidate,
    FailureDominantCounters,
    FailureDominantRecentTraces,
    MissingTaskFamily,
    MissingToolPattern,
    OversizedBody,
}

impl SkillHealthSignal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deprecated => "deprecated",
            Self::PendingReviewDecision => "pending_review_decision",
            Self::UnusedActiveSkill => "unused_active_skill",
            Self::StaleCandidate => "stale_candidate",
            Self::FailureDominantCounters => "failure_dominant_counters",
            Self::FailureDominantRecentTraces => "failure_dominant_recent_traces",
            Self::MissingTaskFamily => "missing_task_family",
            Self::MissingToolPattern => "missing_tool_pattern",
            Self::OversizedBody => "oversized_body",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillHealthRecommendation {
    None,
    ImproveMetadata,
    Review,
    Promote,
    Demote,
    Deprecate,
}

impl SkillHealthRecommendation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ImproveMetadata => "improve_metadata",
            Self::Review => "review",
            Self::Promote => "promote",
            Self::Demote => "demote",
            Self::Deprecate => "deprecate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SkillUsageStats {
    pub trace_total: u32,
    pub trace_succeeded: u32,
    pub trace_failed: u32,
    pub trace_repaired: u32,
    pub last_used_at_unix: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SkillUtilityStats {
    pub selected_count: u32,
    pub read_count: u32,
    pub blocked_count: u32,
    pub helped_count: u32,
    pub failed_count: u32,
    pub repaired_count: u32,
    pub rollback_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHealthItem {
    pub skill_id: MemoryId,
    pub name: String,
    pub origin: SkillOrigin,
    pub status: SkillStatus,
    pub version: u32,
    pub success_count: u32,
    pub fail_count: u32,
    pub usage: SkillUsageStats,
    pub utility: SkillUtilityStats,
    pub updated_at_unix: i64,
    pub severity: SkillHealthSeverity,
    pub recommendation: SkillHealthRecommendation,
    pub signals: Vec<SkillHealthSignal>,
    pub review_action: Option<SkillReviewAction>,
    pub review_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHealthSummary {
    pub total: usize,
    pub healthy: usize,
    pub watch: usize,
    pub review: usize,
    pub deprecated: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHealthReport {
    pub agent_id: AgentId,
    pub inspected_traces: usize,
    pub inspected_activation_traces: usize,
    pub inspected_rollbacks: usize,
    pub summary: SkillHealthSummary,
    pub items: Vec<SkillHealthItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHealthCleanupDecision {
    pub skill_id: MemoryId,
    pub skill_name: String,
    pub current_status: SkillStatus,
    pub target_status: SkillStatus,
    pub reason: String,
}

pub fn build_skill_health_report(
    agent_id: impl Into<AgentId>,
    skills: &[Skill],
    traces: &[SkillUseTrace],
    activation_traces: &[SkillActivationTrace],
    rollback_records: &[SkillPatchRollbackRecord],
    review_decisions: &[SkillReviewDecision],
    now_unix: i64,
    limit: usize,
) -> SkillHealthReport {
    let agent_id = agent_id.into();
    let usage_by_skill = usage_stats_by_skill(traces);
    let utility_by_skill =
        utility_stats_by_skill(&agent_id, traces, activation_traces, rollback_records);
    let review_by_skill = review_decisions
        .iter()
        .map(|decision| (decision.skill_id.clone(), decision))
        .collect::<HashMap<_, _>>();

    let mut items = skills
        .iter()
        .map(|skill| {
            build_health_item(
                skill,
                usage_by_skill.get(&skill.id).cloned().unwrap_or_default(),
                utility_by_skill.get(&skill.id).cloned().unwrap_or_default(),
                review_by_skill.get(&skill.id).copied(),
                now_unix,
            )
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        severity_rank(right.severity)
            .cmp(&severity_rank(left.severity))
            .then_with(|| {
                right
                    .usage
                    .last_used_at_unix
                    .unwrap_or(right.updated_at_unix)
                    .cmp(&left.usage.last_used_at_unix.unwrap_or(left.updated_at_unix))
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    items.truncate(limit.max(1));

    let summary = SkillHealthSummary {
        total: items.len(),
        healthy: items
            .iter()
            .filter(|item| item.severity == SkillHealthSeverity::Healthy)
            .count(),
        watch: items
            .iter()
            .filter(|item| item.severity == SkillHealthSeverity::Watch)
            .count(),
        review: items
            .iter()
            .filter(|item| item.severity == SkillHealthSeverity::Review)
            .count(),
        deprecated: items
            .iter()
            .filter(|item| item.severity == SkillHealthSeverity::Deprecated)
            .count(),
    };

    SkillHealthReport {
        agent_id,
        inspected_traces: traces.len(),
        inspected_activation_traces: activation_traces.len(),
        inspected_rollbacks: rollback_records.len(),
        summary,
        items,
    }
}

pub fn skill_health_cleanup_decisions(
    report: &SkillHealthReport,
) -> Vec<SkillHealthCleanupDecision> {
    report
        .items
        .iter()
        .filter_map(cleanup_decision_for_item)
        .collect()
}

fn cleanup_decision_for_item(item: &SkillHealthItem) -> Option<SkillHealthCleanupDecision> {
    if item.origin != SkillOrigin::Learned || item.status == SkillStatus::Deprecated {
        return None;
    }

    let (target_status, reason) = if let Some(action) = &item.review_action {
        (
            status_from_review_action(action),
            item.review_reason
                .clone()
                .unwrap_or_else(|| "pending_review_decision".to_string()),
        )
    } else if has_signal(item, SkillHealthSignal::FailureDominantCounters)
        || has_signal(item, SkillHealthSignal::FailureDominantRecentTraces)
    {
        let target = match item.status {
            SkillStatus::Active => SkillStatus::Candidate,
            SkillStatus::Candidate => SkillStatus::Deprecated,
            SkillStatus::Deprecated => return None,
        };
        (target, "failure_dominant_health_signals".to_string())
    } else if item.status == SkillStatus::Candidate
        && has_signal(item, SkillHealthSignal::StaleCandidate)
    {
        (SkillStatus::Deprecated, "stale_candidate".to_string())
    } else {
        return None;
    };

    if target_status == item.status {
        return None;
    }

    Some(SkillHealthCleanupDecision {
        skill_id: item.skill_id.clone(),
        skill_name: item.name.clone(),
        current_status: item.status.clone(),
        target_status,
        reason,
    })
}

fn status_from_review_action(action: &SkillReviewAction) -> SkillStatus {
    match action {
        SkillReviewAction::PromoteToActive => SkillStatus::Active,
        SkillReviewAction::DowngradeToCandidate => SkillStatus::Candidate,
        SkillReviewAction::Deprecate => SkillStatus::Deprecated,
    }
}

fn has_signal(item: &SkillHealthItem, signal: SkillHealthSignal) -> bool {
    item.signals.iter().any(|candidate| *candidate == signal)
}

fn build_health_item(
    skill: &Skill,
    usage: SkillUsageStats,
    utility: SkillUtilityStats,
    review_decision: Option<&SkillReviewDecision>,
    now_unix: i64,
) -> SkillHealthItem {
    let mut signals = Vec::new();

    if skill.status == SkillStatus::Deprecated {
        signals.push(SkillHealthSignal::Deprecated);
    }
    if review_decision.is_some() {
        signals.push(SkillHealthSignal::PendingReviewDecision);
    }
    if skill.status == SkillStatus::Active
        && skill.origin != SkillOrigin::Imported
        && skill.success_count == 0
        && usage.trace_total == 0
        && utility.selected_count == 0
        && utility.read_count == 0
    {
        signals.push(SkillHealthSignal::UnusedActiveSkill);
    }
    if skill.status == SkillStatus::Candidate
        && usage.trace_total == 0
        && now_unix.saturating_sub(skill.updated_at.timestamp()) >= STALE_CANDIDATE_SECS
    {
        signals.push(SkillHealthSignal::StaleCandidate);
    }
    if skill.fail_count >= FAILURE_DOMINANT_TRACE_THRESHOLD
        && skill.fail_count > skill.success_count
    {
        signals.push(SkillHealthSignal::FailureDominantCounters);
    }
    if usage.trace_failed >= FAILURE_DOMINANT_TRACE_THRESHOLD
        && usage.trace_failed > usage.trace_succeeded.saturating_add(usage.trace_repaired)
    {
        signals.push(SkillHealthSignal::FailureDominantRecentTraces);
    }
    if skill.task_family.as_deref().unwrap_or("").trim().is_empty() {
        signals.push(SkillHealthSignal::MissingTaskFamily);
    }
    if skill.tool_pattern.is_empty() {
        signals.push(SkillHealthSignal::MissingToolPattern);
    }
    if skill.content.chars().count() > OVERSIZED_SKILL_BODY_CHARS {
        signals.push(SkillHealthSignal::OversizedBody);
    }

    let recommendation = recommendation_for(skill, &signals, review_decision);
    let severity = severity_for(skill, &signals, review_decision);

    SkillHealthItem {
        skill_id: skill.id.clone(),
        name: skill.name.clone(),
        origin: skill.origin.clone(),
        status: skill.status.clone(),
        version: skill.version,
        success_count: skill.success_count,
        fail_count: skill.fail_count,
        usage,
        utility,
        updated_at_unix: skill.updated_at.timestamp(),
        severity,
        recommendation,
        signals,
        review_action: review_decision.map(|decision| decision.action.clone()),
        review_reason: review_decision.map(|decision| decision.reason.to_string()),
    }
}

fn recommendation_for(
    skill: &Skill,
    signals: &[SkillHealthSignal],
    review_decision: Option<&SkillReviewDecision>,
) -> SkillHealthRecommendation {
    if let Some(decision) = review_decision {
        return match decision.action {
            SkillReviewAction::PromoteToActive => SkillHealthRecommendation::Promote,
            SkillReviewAction::DowngradeToCandidate => SkillHealthRecommendation::Demote,
            SkillReviewAction::Deprecate => SkillHealthRecommendation::Deprecate,
        };
    }
    if skill.status == SkillStatus::Deprecated {
        return SkillHealthRecommendation::None;
    }
    if signals.iter().any(|signal| {
        matches!(
            signal,
            SkillHealthSignal::FailureDominantCounters
                | SkillHealthSignal::FailureDominantRecentTraces
                | SkillHealthSignal::StaleCandidate
        )
    }) {
        return SkillHealthRecommendation::Review;
    }
    if signals.iter().any(|signal| {
        matches!(
            signal,
            SkillHealthSignal::MissingTaskFamily
                | SkillHealthSignal::MissingToolPattern
                | SkillHealthSignal::OversizedBody
                | SkillHealthSignal::UnusedActiveSkill
        )
    }) {
        return SkillHealthRecommendation::ImproveMetadata;
    }
    SkillHealthRecommendation::None
}

fn severity_for(
    skill: &Skill,
    signals: &[SkillHealthSignal],
    review_decision: Option<&SkillReviewDecision>,
) -> SkillHealthSeverity {
    if skill.status == SkillStatus::Deprecated {
        return SkillHealthSeverity::Deprecated;
    }
    if review_decision.is_some()
        || signals.iter().any(|signal| {
            matches!(
                signal,
                SkillHealthSignal::FailureDominantCounters
                    | SkillHealthSignal::FailureDominantRecentTraces
                    | SkillHealthSignal::StaleCandidate
            )
        })
    {
        return SkillHealthSeverity::Review;
    }
    if signals.is_empty() {
        SkillHealthSeverity::Healthy
    } else {
        SkillHealthSeverity::Watch
    }
}

fn usage_stats_by_skill(traces: &[SkillUseTrace]) -> HashMap<MemoryId, SkillUsageStats> {
    let mut usage_by_skill = HashMap::<MemoryId, SkillUsageStats>::new();
    for trace in traces {
        let stats = usage_by_skill.entry(trace.skill_id.clone()).or_default();
        stats.trace_total = stats.trace_total.saturating_add(1);
        stats.last_used_at_unix = Some(
            stats
                .last_used_at_unix
                .unwrap_or(trace.observed_at_unix)
                .max(trace.observed_at_unix),
        );
        match trace.outcome {
            SkillUseOutcome::Succeeded => {
                stats.trace_succeeded = stats.trace_succeeded.saturating_add(1);
            }
            SkillUseOutcome::Failed => {
                stats.trace_failed = stats.trace_failed.saturating_add(1);
            }
            SkillUseOutcome::Repaired => {
                stats.trace_repaired = stats.trace_repaired.saturating_add(1);
            }
        }
    }
    usage_by_skill
}

fn utility_stats_by_skill(
    agent_id: &str,
    traces: &[SkillUseTrace],
    activation_traces: &[SkillActivationTrace],
    rollback_records: &[SkillPatchRollbackRecord],
) -> HashMap<MemoryId, SkillUtilityStats> {
    let mut utility_by_skill = HashMap::<MemoryId, SkillUtilityStats>::new();

    for trace in traces {
        let stats = utility_by_skill.entry(trace.skill_id.clone()).or_default();
        match trace.outcome {
            SkillUseOutcome::Succeeded => {
                stats.helped_count = stats.helped_count.saturating_add(1);
            }
            SkillUseOutcome::Failed => {
                stats.failed_count = stats.failed_count.saturating_add(1);
            }
            SkillUseOutcome::Repaired => {
                stats.repaired_count = stats.repaired_count.saturating_add(1);
            }
        }
    }

    for trace in activation_traces {
        for skill_id in unique_skill_ids(&trace.selected_skill_ids) {
            let stats = utility_by_skill.entry(skill_id).or_default();
            stats.selected_count = stats.selected_count.saturating_add(1);
        }
        for skill_id in unique_skill_ids(&trace.loaded_skill_ids) {
            let stats = utility_by_skill.entry(skill_id).or_default();
            stats.read_count = stats.read_count.saturating_add(1);
        }
        for skill_id in unique_skill_ids(&trace.blocked_skill_ids) {
            let stats = utility_by_skill.entry(skill_id).or_default();
            stats.blocked_count = stats.blocked_count.saturating_add(1);
        }
    }

    for record in rollback_records
        .iter()
        .filter(|record| record.agent_id == agent_id)
    {
        let stats = utility_by_skill
            .entry(record.target_skill_id.clone())
            .or_default();
        stats.rollback_count = stats.rollback_count.saturating_add(1);
    }

    utility_by_skill
}

fn unique_skill_ids(ids: &[String]) -> Vec<MemoryId> {
    let mut seen = HashSet::<&str>::new();
    ids.iter()
        .filter_map(|id| {
            let trimmed = id.trim();
            if trimmed.is_empty() || !seen.insert(trimmed) {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

fn severity_rank(severity: SkillHealthSeverity) -> u8 {
    match severity {
        SkillHealthSeverity::Deprecated => 0,
        SkillHealthSeverity::Healthy => 1,
        SkillHealthSeverity::Watch => 2,
        SkillHealthSeverity::Review => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::skill_governance_service::SkillEvidenceRef;
    use chrono::{TimeZone, Utc};

    fn sample_skill(id: &str, status: SkillStatus) -> Skill {
        Skill {
            id: id.into(),
            name: id.into(),
            description: "desc".into(),
            content: "Use structured tools.".into(),
            task_family: Some("repo_audit".into()),
            tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
            lineage_task_families: vec!["repo_audit".into()],
            tags: vec![],
            success_count: 0,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Learned,
            status,
            created_by: "agent".into(),
            created_at: Utc.timestamp_opt(100, 0).single().unwrap(),
            updated_at: Utc.timestamp_opt(100, 0).single().unwrap(),
        }
    }

    fn trace(skill_id: &str, outcome: SkillUseOutcome, observed_at_unix: i64) -> SkillUseTrace {
        SkillUseTrace {
            id: format!("{skill_id}-{observed_at_unix}"),
            skill_id: skill_id.into(),
            task_family: Some("repo_audit".into()),
            route_model: None,
            tool_pattern: vec!["repo_discovery".into()],
            outcome,
            verification: None,
            repair_evidence: Vec::<SkillEvidenceRef>::new(),
            observed_at_unix,
        }
    }

    #[test]
    fn flags_failure_dominant_recent_traces_for_review() {
        let report = build_skill_health_report(
            "agent",
            &[sample_skill("skill-a", SkillStatus::Active)],
            &[
                trace("skill-a", SkillUseOutcome::Failed, 200),
                trace("skill-a", SkillUseOutcome::Failed, 210),
                trace("skill-a", SkillUseOutcome::Succeeded, 220),
            ],
            &[],
            &[],
            &[],
            1_000,
            10,
        );

        assert_eq!(report.summary.review, 1);
        assert_eq!(report.items[0].severity, SkillHealthSeverity::Review);
        assert!(report.items[0]
            .signals
            .contains(&SkillHealthSignal::FailureDominantRecentTraces));
        assert_eq!(report.items[0].usage.last_used_at_unix, Some(220));
    }

    #[test]
    fn surfaces_pending_review_decision_recommendation() {
        let skill = sample_skill("skill-a", SkillStatus::Candidate);
        let decision = SkillReviewDecision {
            skill_id: "skill-a".into(),
            skill_name: "skill-a".into(),
            lineage_task_families: vec!["repo_audit".into()],
            action: SkillReviewAction::PromoteToActive,
            target_status: SkillStatus::Active,
            reason: "repeated_successes",
        };

        let report =
            build_skill_health_report("agent", &[skill], &[], &[], &[], &[decision], 1_000, 10);

        assert_eq!(report.items[0].severity, SkillHealthSeverity::Review);
        assert_eq!(
            report.items[0].recommendation,
            SkillHealthRecommendation::Promote
        );
        assert!(report.items[0]
            .signals
            .contains(&SkillHealthSignal::PendingReviewDecision));
    }

    #[test]
    fn metadata_gaps_are_watch_not_mutation_recommendations() {
        let mut skill = sample_skill("manual-a", SkillStatus::Active);
        skill.origin = SkillOrigin::Manual;
        skill.task_family = None;
        skill.tool_pattern = vec![];

        let report = build_skill_health_report("agent", &[skill], &[], &[], &[], &[], 1_000, 10);

        assert_eq!(report.items[0].severity, SkillHealthSeverity::Watch);
        assert_eq!(
            report.items[0].recommendation,
            SkillHealthRecommendation::ImproveMetadata
        );
        assert!(report.items[0]
            .signals
            .contains(&SkillHealthSignal::MissingTaskFamily));
        assert!(report.items[0]
            .signals
            .contains(&SkillHealthSignal::MissingToolPattern));
    }

    #[test]
    fn cleanup_decisions_apply_only_to_learned_lifecycle_changes() {
        let mut failing = sample_skill("failing-active", SkillStatus::Active);
        failing.fail_count = 3;
        failing.success_count = 1;

        let mut manual_gap = sample_skill("manual-gap", SkillStatus::Active);
        manual_gap.origin = SkillOrigin::Manual;
        manual_gap.task_family = None;
        manual_gap.tool_pattern = vec![];

        let report = build_skill_health_report(
            "agent",
            &[failing, manual_gap],
            &[],
            &[],
            &[],
            &[],
            1_000,
            10,
        );
        let decisions = skill_health_cleanup_decisions(&report);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].skill_id, "failing-active");
        assert_eq!(decisions[0].current_status, SkillStatus::Active);
        assert_eq!(decisions[0].target_status, SkillStatus::Candidate);
        assert_eq!(decisions[0].reason, "failure_dominant_health_signals");
    }

    #[test]
    fn cleanup_decisions_reuse_pending_review_action() {
        let skill = sample_skill("skill-a", SkillStatus::Candidate);
        let decision = SkillReviewDecision {
            skill_id: "skill-a".into(),
            skill_name: "skill-a".into(),
            lineage_task_families: vec!["repo_audit".into()],
            action: SkillReviewAction::PromoteToActive,
            target_status: SkillStatus::Active,
            reason: "repeated_successes",
        };

        let report =
            build_skill_health_report("agent", &[skill], &[], &[], &[], &[decision], 1_000, 10);
        let decisions = skill_health_cleanup_decisions(&report);

        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].target_status, SkillStatus::Active);
        assert_eq!(decisions[0].reason, "repeated_successes");
    }

    #[test]
    fn folds_typed_utility_traces_without_prompt_material() {
        let skill = sample_skill("skill-a", SkillStatus::Active);
        let activation = SkillActivationTrace {
            selected_skill_ids: vec!["skill-a".into(), "skill-a".into()],
            loaded_skill_ids: vec!["skill-a".into()],
            blocked_skill_ids: vec!["shadowed-skill".into()],
            blocked_reasons: vec![],
            budget_catalog_entries: 3,
            budget_preloaded_skills: 0,
            route_model: Some("deepseek".into()),
            outcome: Some("loaded".into()),
        };
        let rollback = SkillPatchRollbackRecord {
            id: "rollback-1".into(),
            apply_record_id: "apply-1".into(),
            candidate_id: "candidate-1".into(),
            target_skill_id: "skill-a".into(),
            agent_id: "agent".into(),
            from_version: 3,
            restored_from_version: 2,
            new_version: 4,
            rollback_skill_id: "rollback-snapshot".into(),
            reason: "operator_rollback".into(),
            rolled_back_at_unix: 500,
        };

        let report = build_skill_health_report(
            "agent",
            &[skill],
            &[
                trace("skill-a", SkillUseOutcome::Succeeded, 200),
                trace("skill-a", SkillUseOutcome::Repaired, 210),
            ],
            &[activation],
            &[rollback],
            &[],
            1_000,
            10,
        );

        assert_eq!(report.inspected_activation_traces, 1);
        assert_eq!(report.inspected_rollbacks, 1);
        assert_eq!(report.items[0].utility.selected_count, 1);
        assert_eq!(report.items[0].utility.read_count, 1);
        assert_eq!(report.items[0].utility.helped_count, 1);
        assert_eq!(report.items[0].utility.repaired_count, 1);
        assert_eq!(report.items[0].utility.rollback_count, 1);
    }
}
