//! Replay/eval gates for generated skill candidates.
//!
//! Evaluation never promotes a skill directly. It only reports whether a
//! `SkillDraft` or `SkillPatchCandidate` has enough deterministic replay
//! evidence to be eligible for operator or policy promotion.

use crate::application::services::skill_governance_service::{
    SkillDraft, SkillPatchCandidate, SkillReplayEvalResult, SkillReplayEvalStatus, SkillUseOutcome,
    SkillUseTrace,
};
use crate::domain::memory::{Skill, SkillOrigin, SkillStatus};
use async_trait::async_trait;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCandidateEvalPolicy {
    pub min_passed_criteria: usize,
    pub allow_missing_criteria: bool,
    pub block_on_failed_criteria: bool,
    pub require_executable_replay: bool,
    pub require_with_without_comparison: bool,
}

impl Default for SkillCandidateEvalPolicy {
    fn default() -> Self {
        Self {
            min_passed_criteria: 1,
            allow_missing_criteria: false,
            block_on_failed_criteria: true,
            require_executable_replay: true,
            require_with_without_comparison: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillCandidateEvalReport {
    pub candidate_id: String,
    pub candidate_kind: &'static str,
    pub promotion_allowed: bool,
    pub reason: &'static str,
    pub passed_count: usize,
    pub failed_count: usize,
    pub missing_count: usize,
    pub criteria: Vec<SkillReplayEvalResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReplayCaseKind {
    RepairTraceReplay,
    WithWithoutComparison,
    ContradictionScan,
    OperatorReview,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkillReplayCase {
    pub candidate_id: String,
    pub candidate_kind: &'static str,
    pub criterion: String,
    pub kind: SkillReplayCaseKind,
    pub target_skill_id: Option<String>,
    pub target_version: Option<u32>,
    pub required_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_args: Option<serde_json::Value>,
    pub provenance_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkillReplayHarnessReport {
    pub candidate_id: String,
    pub candidate_kind: &'static str,
    pub cases: Vec<SkillReplayCase>,
    pub results: Vec<SkillReplayEvalResult>,
    pub promotion_report: SkillCandidateEvalReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillPatchApplyReport {
    pub candidate_id: String,
    pub target_skill_id: String,
    pub target_version: u32,
    pub current_version: u32,
    pub apply_allowed: bool,
    pub reason: &'static str,
    pub promotion_report: SkillCandidateEvalReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillPatchAutoPromotionPolicy {
    pub enabled: bool,
    pub min_successful_live_traces: usize,
    pub trace_window_limit: usize,
    pub max_recent_blocking_traces: usize,
}

impl Default for SkillPatchAutoPromotionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            min_successful_live_traces: 2,
            trace_window_limit: 12,
            max_recent_blocking_traces: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillPatchAutoPromotionReason {
    Allowed,
    AutoPromotionDisabled,
    ApplyGateBlocked,
    MissingLiveTrace,
    InsufficientSuccessfulLiveTraces,
    RecentBlockingLiveTraces,
}

impl SkillPatchAutoPromotionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::AutoPromotionDisabled => "auto_promotion_disabled",
            Self::ApplyGateBlocked => "apply_gate_blocked",
            Self::MissingLiveTrace => "missing_live_trace",
            Self::InsufficientSuccessfulLiveTraces => "insufficient_successful_live_traces",
            Self::RecentBlockingLiveTraces => "recent_blocking_live_traces",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillPatchAutoPromotionReport {
    pub candidate_id: String,
    pub target_skill_id: String,
    pub auto_promotion_allowed: bool,
    pub reason: SkillPatchAutoPromotionReason,
    pub successful_trace_count: usize,
    pub blocking_trace_count: usize,
    pub considered_trace_count: usize,
    pub required_successful_trace_count: usize,
    pub max_allowed_blocking_trace_count: usize,
    pub apply_report: SkillPatchApplyReport,
}

#[async_trait]
pub trait SkillReplayHarnessPort: Send + Sync {
    async fn run_replay_case(
        &self,
        candidate: &SkillPatchCandidate,
        case: &SkillReplayCase,
    ) -> Result<SkillReplayEvalResult, String>;
}

pub fn evaluate_skill_draft_for_promotion(
    draft: &SkillDraft,
    policy: &SkillCandidateEvalPolicy,
) -> SkillCandidateEvalReport {
    evaluate_candidate(
        &draft.id,
        "draft",
        &draft.replay_criteria,
        &draft.eval_results,
        policy,
    )
}

pub fn evaluate_skill_patch_for_promotion(
    patch: &SkillPatchCandidate,
    policy: &SkillCandidateEvalPolicy,
) -> SkillCandidateEvalReport {
    let cases = build_skill_patch_replay_cases(patch);
    evaluate_patch_candidate(
        &patch.id,
        &patch.replay_criteria,
        &patch.eval_results,
        &cases,
        policy,
    )
}

pub fn evaluate_skill_patch_for_apply(
    patch: &SkillPatchCandidate,
    target: &Skill,
    policy: &SkillCandidateEvalPolicy,
) -> SkillPatchApplyReport {
    let promotion_report = evaluate_skill_patch_for_promotion(patch, policy);
    let reason = skill_patch_apply_block_reason(patch, target, &promotion_report);

    SkillPatchApplyReport {
        candidate_id: patch.id.clone(),
        target_skill_id: patch.target_skill_id.clone(),
        target_version: patch.target_version,
        current_version: target.version,
        apply_allowed: reason == "apply_allowed",
        reason,
        promotion_report,
    }
}

pub fn evaluate_skill_patch_for_auto_promotion(
    patch: &SkillPatchCandidate,
    target: &Skill,
    traces: &[SkillUseTrace],
    auto_policy: &SkillPatchAutoPromotionPolicy,
    eval_policy: &SkillCandidateEvalPolicy,
) -> SkillPatchAutoPromotionReport {
    let apply_report = evaluate_skill_patch_for_apply(patch, target, eval_policy);
    let considered = recent_traces_for_skill(
        traces,
        &patch.target_skill_id,
        auto_policy.trace_window_limit.max(1),
    );
    let successful_trace_count = considered
        .iter()
        .filter(|trace| trace.outcome == SkillUseOutcome::Succeeded)
        .count();
    let blocking_trace_count = considered
        .iter()
        .filter(|trace| {
            matches!(
                trace.outcome,
                SkillUseOutcome::Failed | SkillUseOutcome::Repaired
            )
        })
        .count();

    let reason = if !auto_policy.enabled {
        SkillPatchAutoPromotionReason::AutoPromotionDisabled
    } else if !apply_report.apply_allowed {
        SkillPatchAutoPromotionReason::ApplyGateBlocked
    } else if considered.is_empty() {
        SkillPatchAutoPromotionReason::MissingLiveTrace
    } else if blocking_trace_count > auto_policy.max_recent_blocking_traces {
        SkillPatchAutoPromotionReason::RecentBlockingLiveTraces
    } else if successful_trace_count < auto_policy.min_successful_live_traces.max(1) {
        SkillPatchAutoPromotionReason::InsufficientSuccessfulLiveTraces
    } else {
        SkillPatchAutoPromotionReason::Allowed
    };

    SkillPatchAutoPromotionReport {
        candidate_id: patch.id.clone(),
        target_skill_id: patch.target_skill_id.clone(),
        auto_promotion_allowed: reason == SkillPatchAutoPromotionReason::Allowed,
        reason,
        successful_trace_count,
        blocking_trace_count,
        considered_trace_count: considered.len(),
        required_successful_trace_count: auto_policy.min_successful_live_traces.max(1),
        max_allowed_blocking_trace_count: auto_policy.max_recent_blocking_traces,
        apply_report,
    }
}

fn recent_traces_for_skill<'a>(
    traces: &'a [SkillUseTrace],
    skill_id: &str,
    limit: usize,
) -> Vec<&'a SkillUseTrace> {
    let mut matching = traces
        .iter()
        .filter(|trace| trace.skill_id == skill_id)
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        right
            .observed_at_unix
            .cmp(&left.observed_at_unix)
            .then_with(|| right.id.cmp(&left.id))
    });
    matching.truncate(limit);
    matching
}

pub fn build_skill_patch_replay_cases(patch: &SkillPatchCandidate) -> Vec<SkillReplayCase> {
    let repair_evidence = patch.provenance.iter().find(|evidence| {
        evidence
            .metadata
            .get("tool_name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    });
    let repair_tool = repair_evidence.and_then(|evidence| {
        evidence
            .metadata
            .get("tool_name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    });
    let replay_args = repair_evidence
        .and_then(|evidence| evidence.metadata.get("replay_args"))
        .cloned();
    let provenance_ids = patch
        .provenance
        .iter()
        .map(|evidence| evidence.id.clone())
        .collect::<Vec<_>>();

    patch
        .replay_criteria
        .iter()
        .enumerate()
        .filter(|(_, criterion)| !criterion.trim().is_empty())
        .map(|(idx, criterion)| {
            let kind = match idx {
                0 if repair_tool.is_some() => SkillReplayCaseKind::RepairTraceReplay,
                1 => SkillReplayCaseKind::WithWithoutComparison,
                2 => SkillReplayCaseKind::ContradictionScan,
                _ => SkillReplayCaseKind::OperatorReview,
            };
            SkillReplayCase {
                candidate_id: patch.id.clone(),
                candidate_kind: "patch",
                criterion: criterion.clone(),
                kind,
                target_skill_id: Some(patch.target_skill_id.clone()),
                target_version: Some(patch.target_version),
                required_tool: if kind == SkillReplayCaseKind::RepairTraceReplay {
                    repair_tool.clone()
                } else {
                    None
                },
                tool_args: if kind == SkillReplayCaseKind::RepairTraceReplay {
                    replay_args.clone()
                } else {
                    None
                },
                provenance_ids: provenance_ids.clone(),
            }
        })
        .collect()
}

pub async fn run_skill_patch_replay_harness(
    patch: &SkillPatchCandidate,
    harness: &dyn SkillReplayHarnessPort,
    policy: &SkillCandidateEvalPolicy,
) -> SkillReplayHarnessReport {
    let cases = build_skill_patch_replay_cases(patch);
    let mut results = Vec::with_capacity(cases.len());
    for case in &cases {
        match harness.run_replay_case(patch, case).await {
            Ok(result) => results.push(result),
            Err(error) => results.push(SkillReplayEvalResult {
                criterion: case.criterion.clone(),
                status: SkillReplayEvalStatus::Failed,
                evidence: Some(format!("replay_harness_error: {error}")),
                observed_at_unix: chrono::Utc::now().timestamp(),
            }),
        }
    }
    let merged_results = merge_skill_replay_eval_results(&patch.eval_results, &results);
    let promotion_report = evaluate_patch_candidate(
        &patch.id,
        &patch.replay_criteria,
        &merged_results,
        &cases,
        policy,
    );

    SkillReplayHarnessReport {
        candidate_id: patch.id.clone(),
        candidate_kind: "patch",
        cases,
        results: merged_results,
        promotion_report,
    }
}

pub fn merge_skill_replay_eval_results(
    existing: &[SkillReplayEvalResult],
    fresh: &[SkillReplayEvalResult],
) -> Vec<SkillReplayEvalResult> {
    let mut merged = existing.to_vec();
    for result in fresh {
        if let Some(slot) = merged
            .iter_mut()
            .find(|existing| same_criterion(&existing.criterion, &result.criterion))
        {
            *slot = result.clone();
        } else {
            merged.push(result.clone());
        }
    }
    merged
}

fn evaluate_patch_candidate(
    candidate_id: &str,
    replay_criteria: &[String],
    eval_results: &[SkillReplayEvalResult],
    cases: &[SkillReplayCase],
    policy: &SkillCandidateEvalPolicy,
) -> SkillCandidateEvalReport {
    let report = evaluate_candidate(candidate_id, "patch", replay_criteria, eval_results, policy);
    enforce_patch_replay_requirements(report, cases, policy)
}

fn enforce_patch_replay_requirements(
    report: SkillCandidateEvalReport,
    cases: &[SkillReplayCase],
    policy: &SkillCandidateEvalPolicy,
) -> SkillCandidateEvalReport {
    if report.reason == "missing_replay_criteria"
        || (policy.block_on_failed_criteria && report.failed_count > 0)
    {
        return report;
    }

    if policy.require_executable_replay {
        match required_case_status(
            cases,
            &report.criteria,
            SkillReplayCaseKind::RepairTraceReplay,
        ) {
            RequiredCaseStatus::Passed => {}
            RequiredCaseStatus::Failed => return report,
            RequiredCaseStatus::MissingCase => {
                return report_with_reason(report, "missing_executable_replay_case");
            }
            RequiredCaseStatus::MissingResult => {
                return report_with_reason(report, "missing_executable_replay_result");
            }
        }
    }

    if policy.require_with_without_comparison {
        match required_case_status(
            cases,
            &report.criteria,
            SkillReplayCaseKind::WithWithoutComparison,
        ) {
            RequiredCaseStatus::Passed => {}
            RequiredCaseStatus::Failed => return report,
            RequiredCaseStatus::MissingCase => {
                return report_with_reason(report, "missing_with_without_comparison_case");
            }
            RequiredCaseStatus::MissingResult => {
                return report_with_reason(report, "missing_with_without_comparison_result");
            }
        }
    }

    report
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequiredCaseStatus {
    Passed,
    Failed,
    MissingResult,
    MissingCase,
}

fn required_case_status(
    cases: &[SkillReplayCase],
    eval_results: &[SkillReplayEvalResult],
    kind: SkillReplayCaseKind,
) -> RequiredCaseStatus {
    let matching_cases = cases
        .iter()
        .filter(|case| case.kind == kind)
        .collect::<Vec<_>>();
    if matching_cases.is_empty() {
        return RequiredCaseStatus::MissingCase;
    }

    let mut saw_missing = false;
    for case in matching_cases {
        match eval_results
            .iter()
            .find(|result| same_criterion(&result.criterion, &case.criterion))
            .map(|result| result.status)
        {
            Some(SkillReplayEvalStatus::Passed) => return RequiredCaseStatus::Passed,
            Some(SkillReplayEvalStatus::Failed) => return RequiredCaseStatus::Failed,
            Some(SkillReplayEvalStatus::Missing) | None => saw_missing = true,
        }
    }

    if saw_missing {
        RequiredCaseStatus::MissingResult
    } else {
        RequiredCaseStatus::MissingCase
    }
}

fn report_with_reason(
    mut report: SkillCandidateEvalReport,
    reason: &'static str,
) -> SkillCandidateEvalReport {
    report.reason = reason;
    report.promotion_allowed = false;
    report
}

fn evaluate_candidate(
    candidate_id: &str,
    candidate_kind: &'static str,
    replay_criteria: &[String],
    eval_results: &[SkillReplayEvalResult],
    policy: &SkillCandidateEvalPolicy,
) -> SkillCandidateEvalReport {
    let criteria = normalized_eval_results(replay_criteria, eval_results);
    let passed_count = criteria
        .iter()
        .filter(|result| result.status == SkillReplayEvalStatus::Passed)
        .count();
    let failed_count = criteria
        .iter()
        .filter(|result| result.status == SkillReplayEvalStatus::Failed)
        .count();
    let missing_count = criteria
        .iter()
        .filter(|result| result.status == SkillReplayEvalStatus::Missing)
        .count();

    let reason = if replay_criteria.is_empty() {
        "missing_replay_criteria"
    } else if policy.block_on_failed_criteria && failed_count > 0 {
        "failed_replay_criterion"
    } else if !policy.allow_missing_criteria && missing_count > 0 {
        "missing_replay_result"
    } else if passed_count < policy.min_passed_criteria.max(1) {
        "insufficient_passed_replay_results"
    } else {
        "passed_replay_eval_gate"
    };

    SkillCandidateEvalReport {
        candidate_id: candidate_id.to_string(),
        candidate_kind,
        promotion_allowed: reason == "passed_replay_eval_gate",
        reason,
        passed_count,
        failed_count,
        missing_count,
        criteria,
    }
}

fn normalized_eval_results(
    replay_criteria: &[String],
    eval_results: &[SkillReplayEvalResult],
) -> Vec<SkillReplayEvalResult> {
    replay_criteria
        .iter()
        .filter(|criterion| !criterion.trim().is_empty())
        .map(|criterion| {
            eval_results
                .iter()
                .find(|result| same_criterion(&result.criterion, criterion))
                .cloned()
                .unwrap_or_else(|| SkillReplayEvalResult {
                    criterion: criterion.clone(),
                    status: SkillReplayEvalStatus::Missing,
                    evidence: None,
                    observed_at_unix: 0,
                })
        })
        .collect()
}

fn same_criterion(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

fn skill_patch_apply_block_reason(
    patch: &SkillPatchCandidate,
    target: &Skill,
    promotion_report: &SkillCandidateEvalReport,
) -> &'static str {
    if patch.status != SkillStatus::Candidate {
        return "candidate_not_pending";
    }
    if target.id != patch.target_skill_id {
        return "target_skill_mismatch";
    }
    if target.origin != SkillOrigin::Learned {
        return "target_not_learned";
    }
    if target.status == SkillStatus::Deprecated {
        return "target_deprecated";
    }
    if target.version != patch.target_version {
        return "target_version_mismatch";
    }
    if patch.proposed_body.trim().is_empty() {
        return "empty_proposed_body";
    }
    if patch.proposed_body.trim() == target.content.trim() {
        return "proposed_body_unchanged";
    }
    if patch.procedure_claims.is_empty() {
        return "missing_procedure_claims";
    }
    if !procedure_claims_backed_by_resolved_provenance(patch) {
        return "unbacked_procedure_claims";
    }
    if !promotion_report.promotion_allowed {
        return promotion_report.reason;
    }
    "apply_allowed"
}

fn procedure_claims_backed_by_resolved_provenance(patch: &SkillPatchCandidate) -> bool {
    patch.procedure_claims.iter().all(|claim| {
        patch.provenance.iter().any(|evidence| {
            let metadata = &evidence.metadata;
            metadata
                .get("repair_outcome")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value == "resolved")
                && metadata
                    .get("tool_name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| value == claim.tool_name)
                && metadata
                    .get("failure_kind")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| value == claim.failure_kind)
                && metadata
                    .get("suggested_action")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| value == claim.suggested_action)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::skill_governance_service::{
        SkillEvidenceKind, SkillEvidenceRef, SkillPatchCandidate,
    };
    use crate::domain::memory::{Skill, SkillOrigin, SkillStatus};

    fn eval(criterion: &str, status: SkillReplayEvalStatus) -> SkillReplayEvalResult {
        SkillReplayEvalResult {
            criterion: criterion.into(),
            status,
            evidence: Some("deterministic replay".into()),
            observed_at_unix: 100,
        }
    }

    fn patch(eval_results: Vec<SkillReplayEvalResult>) -> SkillPatchCandidate {
        SkillPatchCandidate {
            id: "patch-a".into(),
            target_skill_id: "skill-a".into(),
            target_version: 2,
            diff_summary: "add repair note".into(),
            proposed_body: "# Skill\n\n## Candidate Repair Guidance".into(),
            procedure_claims: vec![
                crate::application::services::skill_governance_service::SkillPatchProcedureClaim {
                    tool_name: "shell".into(),
                    failure_kind: "missing_resource".into(),
                    suggested_action: "adjust_arguments_or_target".into(),
                },
            ],
            provenance: vec![SkillEvidenceRef {
                kind: SkillEvidenceKind::RepairTrace,
                id: "repair-a".into(),
                summary: None,
                metadata: serde_json::json!({
                    "repair_outcome": "resolved",
                    "tool_name": "shell",
                    "failure_kind": "missing_resource",
                    "suggested_action": "adjust_arguments_or_target",
                }),
            }],
            replay_criteria: vec!["replay fixed path".into(), "compare with-skill".into()],
            eval_results,
            status: SkillStatus::Candidate,
        }
    }

    fn target_skill() -> Skill {
        Skill {
            id: "skill-a".into(),
            name: "Skill A".into(),
            description: "test skill".into(),
            content: "# Skill\n\nOriginal body".into(),
            task_family: Some("test".into()),
            tool_pattern: vec!["shell".into()],
            lineage_task_families: Vec::new(),
            tags: Vec::new(),
            success_count: 4,
            fail_count: 1,
            version: 2,
            origin: SkillOrigin::Learned,
            status: SkillStatus::Active,
            created_by: "test-agent".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn live_trace(id: &str, outcome: SkillUseOutcome, observed_at_unix: i64) -> SkillUseTrace {
        SkillUseTrace {
            id: id.into(),
            skill_id: "skill-a".into(),
            task_family: Some("test".into()),
            route_model: Some("test-model".into()),
            tool_pattern: vec!["shell".into()],
            outcome,
            verification: Some("live execution observed".into()),
            repair_evidence: Vec::new(),
            observed_at_unix,
        }
    }

    #[test]
    fn all_replay_criteria_pass_allows_promotion_gate() {
        let report = evaluate_skill_patch_for_promotion(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Passed),
            ]),
            &SkillCandidateEvalPolicy::default(),
        );
        assert!(report.promotion_allowed);
        assert_eq!(report.reason, "passed_replay_eval_gate");
        assert_eq!(report.passed_count, 2);
    }

    #[test]
    fn apply_gate_allows_checked_candidate_for_matching_learned_skill() {
        let report = evaluate_skill_patch_for_apply(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Passed),
            ]),
            &target_skill(),
            &SkillCandidateEvalPolicy::default(),
        );

        assert!(report.apply_allowed);
        assert_eq!(report.reason, "apply_allowed");
    }

    #[test]
    fn auto_promotion_can_be_disabled_even_when_apply_gate_passes() {
        let policy = SkillPatchAutoPromotionPolicy {
            enabled: false,
            ..SkillPatchAutoPromotionPolicy::default()
        };
        let report = evaluate_skill_patch_for_auto_promotion(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Passed),
            ]),
            &target_skill(),
            &[
                live_trace("trace-a", SkillUseOutcome::Succeeded, 200),
                live_trace("trace-b", SkillUseOutcome::Succeeded, 201),
            ],
            &policy,
            &SkillCandidateEvalPolicy::default(),
        );

        assert!(!report.auto_promotion_allowed);
        assert_eq!(
            report.reason,
            SkillPatchAutoPromotionReason::AutoPromotionDisabled
        );
        assert!(report.apply_report.apply_allowed);
    }

    #[test]
    fn auto_promotion_allows_passed_patch_with_clean_live_successes() {
        let policy = SkillPatchAutoPromotionPolicy {
            enabled: true,
            min_successful_live_traces: 2,
            trace_window_limit: 5,
            max_recent_blocking_traces: 0,
        };
        let report = evaluate_skill_patch_for_auto_promotion(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Passed),
            ]),
            &target_skill(),
            &[
                live_trace("trace-a", SkillUseOutcome::Succeeded, 200),
                live_trace("trace-b", SkillUseOutcome::Succeeded, 201),
            ],
            &policy,
            &SkillCandidateEvalPolicy::default(),
        );

        assert!(report.auto_promotion_allowed);
        assert_eq!(report.reason, SkillPatchAutoPromotionReason::Allowed);
        assert_eq!(report.successful_trace_count, 2);
        assert_eq!(report.blocking_trace_count, 0);
    }

    #[test]
    fn auto_promotion_blocks_recent_repair_or_failure_traces() {
        let policy = SkillPatchAutoPromotionPolicy {
            enabled: true,
            min_successful_live_traces: 2,
            trace_window_limit: 5,
            max_recent_blocking_traces: 0,
        };
        let report = evaluate_skill_patch_for_auto_promotion(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Passed),
            ]),
            &target_skill(),
            &[
                live_trace("trace-a", SkillUseOutcome::Succeeded, 200),
                live_trace("trace-b", SkillUseOutcome::Succeeded, 201),
                live_trace("trace-c", SkillUseOutcome::Repaired, 202),
            ],
            &policy,
            &SkillCandidateEvalPolicy::default(),
        );

        assert!(!report.auto_promotion_allowed);
        assert_eq!(
            report.reason,
            SkillPatchAutoPromotionReason::RecentBlockingLiveTraces
        );
        assert_eq!(report.blocking_trace_count, 1);
    }

    #[test]
    fn apply_gate_rejects_version_mismatch_before_write() {
        let mut skill = target_skill();
        skill.version = 3;

        let report = evaluate_skill_patch_for_apply(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Passed),
            ]),
            &skill,
            &SkillCandidateEvalPolicy::default(),
        );

        assert!(!report.apply_allowed);
        assert_eq!(report.reason, "target_version_mismatch");
    }

    #[test]
    fn apply_gate_rejects_unbacked_procedure_claims() {
        let mut patch = patch(vec![
            eval("replay fixed path", SkillReplayEvalStatus::Passed),
            eval("compare with-skill", SkillReplayEvalStatus::Passed),
        ]);
        patch.provenance[0]
            .metadata
            .as_object_mut()
            .expect("metadata object")
            .remove("suggested_action");

        let report = evaluate_skill_patch_for_apply(
            &patch,
            &target_skill(),
            &SkillCandidateEvalPolicy::default(),
        );

        assert!(!report.apply_allowed);
        assert_eq!(report.reason, "unbacked_procedure_claims");
    }

    #[test]
    fn missing_replay_result_blocks_promotion_by_default() {
        let report = evaluate_skill_patch_for_promotion(
            &patch(vec![eval(
                "replay fixed path",
                SkillReplayEvalStatus::Passed,
            )]),
            &SkillCandidateEvalPolicy::default(),
        );
        assert!(!report.promotion_allowed);
        assert_eq!(report.reason, "missing_with_without_comparison_result");
        assert_eq!(report.missing_count, 1);
    }

    #[test]
    fn patch_promotion_requires_executable_replay_case() {
        let mut patch = patch(vec![
            eval("replay fixed path", SkillReplayEvalStatus::Passed),
            eval("compare with-skill", SkillReplayEvalStatus::Passed),
        ]);
        patch.provenance[0]
            .metadata
            .as_object_mut()
            .expect("metadata object")
            .remove("tool_name");

        let report =
            evaluate_skill_patch_for_promotion(&patch, &SkillCandidateEvalPolicy::default());

        assert!(!report.promotion_allowed);
        assert_eq!(report.reason, "missing_executable_replay_case");
    }

    #[test]
    fn failed_replay_result_blocks_promotion() {
        let report = evaluate_skill_patch_for_promotion(
            &patch(vec![
                eval("replay fixed path", SkillReplayEvalStatus::Passed),
                eval("compare with-skill", SkillReplayEvalStatus::Failed),
            ]),
            &SkillCandidateEvalPolicy::default(),
        );
        assert!(!report.promotion_allowed);
        assert_eq!(report.reason, "failed_replay_criterion");
        assert_eq!(report.failed_count, 1);
    }

    #[test]
    fn builds_patch_replay_cases_from_structured_metadata() {
        let cases = build_skill_patch_replay_cases(&patch(Vec::new()));

        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].kind, SkillReplayCaseKind::RepairTraceReplay);
        assert_eq!(cases[0].required_tool.as_deref(), Some("shell"));
        assert!(cases[0].tool_args.is_none());
        assert_eq!(cases[0].provenance_ids, vec!["repair-a".to_string()]);
        assert_eq!(cases[1].kind, SkillReplayCaseKind::WithWithoutComparison);
        assert!(cases[1].required_tool.is_none());
    }

    #[test]
    fn replay_case_carries_structured_tool_args_when_evidence_has_them() {
        let mut patch = patch(Vec::new());
        patch.provenance[0].metadata["replay_args"] = serde_json::json!({
            "command": "true"
        });

        let cases = build_skill_patch_replay_cases(&patch);

        assert_eq!(
            cases[0].tool_args,
            Some(serde_json::json!({ "command": "true" }))
        );
    }

    struct PassingHarness;

    #[async_trait::async_trait]
    impl SkillReplayHarnessPort for PassingHarness {
        async fn run_replay_case(
            &self,
            _candidate: &SkillPatchCandidate,
            case: &SkillReplayCase,
        ) -> Result<SkillReplayEvalResult, String> {
            Ok(SkillReplayEvalResult {
                criterion: case.criterion.clone(),
                status: SkillReplayEvalStatus::Passed,
                evidence: Some(format!("passed {:?}", case.kind)),
                observed_at_unix: 200,
            })
        }
    }

    #[tokio::test]
    async fn replay_harness_results_feed_promotion_gate() {
        let report = run_skill_patch_replay_harness(
            &patch(Vec::new()),
            &PassingHarness,
            &SkillCandidateEvalPolicy::default(),
        )
        .await;

        assert_eq!(report.results.len(), 2);
        assert!(report.promotion_report.promotion_allowed);
        assert_eq!(report.promotion_report.reason, "passed_replay_eval_gate");
    }

    struct FailingHarness;

    #[async_trait::async_trait]
    impl SkillReplayHarnessPort for FailingHarness {
        async fn run_replay_case(
            &self,
            _candidate: &SkillPatchCandidate,
            case: &SkillReplayCase,
        ) -> Result<SkillReplayEvalResult, String> {
            if case.kind == SkillReplayCaseKind::RepairTraceReplay {
                return Err("tool sandbox rejected replay".into());
            }
            Ok(SkillReplayEvalResult {
                criterion: case.criterion.clone(),
                status: SkillReplayEvalStatus::Passed,
                evidence: Some("passed comparison".into()),
                observed_at_unix: 200,
            })
        }
    }

    #[tokio::test]
    async fn replay_harness_error_becomes_failed_eval_result() {
        let report = run_skill_patch_replay_harness(
            &patch(Vec::new()),
            &FailingHarness,
            &SkillCandidateEvalPolicy::default(),
        )
        .await;

        assert_eq!(report.promotion_report.reason, "failed_replay_criterion");
        assert_eq!(report.promotion_report.failed_count, 1);
        assert!(report.results.iter().any(|result| {
            result.status == SkillReplayEvalStatus::Failed
                && result
                    .evidence
                    .as_deref()
                    .is_some_and(|value| value.contains("sandbox rejected"))
        }));
    }
}
