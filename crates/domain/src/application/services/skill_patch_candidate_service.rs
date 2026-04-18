//! Skill patch candidate generation from repeated repair evidence.
//!
//! This service is intentionally deterministic. It turns repeated, resolved
//! repair traces into reviewable `SkillPatchCandidate`s instead of editing an
//! active skill in place.

use crate::application::services::skill_governance_service::{
    SkillEvidenceKind, SkillEvidenceRef, SkillPatchApplyRecord, SkillPatchCandidate,
    SkillPatchProcedureClaim, SkillPatchRollbackRecord,
};
use crate::domain::memory::{MemoryCategory, MemoryEntry, Skill, SkillStatus};
use crate::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, tool_repair_outcome_name, ToolRepairOutcome,
    ToolRepairTrace,
};
use std::collections::BTreeMap;

pub const SKILL_PATCH_CANDIDATE_MEMORY_CATEGORY: &str = "skill_patch_candidate";
pub const SKILL_PATCH_APPLY_MEMORY_CATEGORY: &str = "skill_patch_apply_record";
pub const SKILL_PATCH_ROLLBACK_MEMORY_CATEGORY: &str = "skill_patch_rollback_record";

pub fn skill_patch_candidate_memory_category() -> MemoryCategory {
    MemoryCategory::Custom(SKILL_PATCH_CANDIDATE_MEMORY_CATEGORY.to_string())
}

pub fn skill_patch_apply_memory_category() -> MemoryCategory {
    MemoryCategory::Custom(SKILL_PATCH_APPLY_MEMORY_CATEGORY.to_string())
}

pub fn skill_patch_rollback_memory_category() -> MemoryCategory {
    MemoryCategory::Custom(SKILL_PATCH_ROLLBACK_MEMORY_CATEGORY.to_string())
}

pub fn skill_patch_candidate_memory_key(candidate: &SkillPatchCandidate) -> String {
    format!("skill_patch_candidate:{}", candidate.id)
}

pub fn skill_patch_apply_memory_key(record: &SkillPatchApplyRecord) -> String {
    format!("skill_patch_apply:{}", record.id)
}

pub fn skill_patch_rollback_memory_key(record: &SkillPatchRollbackRecord) -> String {
    format!("skill_patch_rollback:{}", record.id)
}

pub fn skill_patch_candidate_to_memory_entry(
    candidate: &SkillPatchCandidate,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> Result<MemoryEntry, serde_json::Error> {
    Ok(MemoryEntry {
        id: String::new(),
        key: skill_patch_candidate_memory_key(candidate),
        content: serde_json::to_string(candidate)?,
        category: skill_patch_candidate_memory_category(),
        timestamp: observed_at.to_rfc3339(),
        session_id: None,
        score: None,
    })
}

pub fn parse_skill_patch_candidate_entry(entry: &MemoryEntry) -> Option<SkillPatchCandidate> {
    if entry.category != skill_patch_candidate_memory_category() {
        return None;
    }
    serde_json::from_str::<SkillPatchCandidate>(&entry.content).ok()
}

pub fn skill_patch_apply_to_memory_entry(
    record: &SkillPatchApplyRecord,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> Result<MemoryEntry, serde_json::Error> {
    Ok(MemoryEntry {
        id: String::new(),
        key: skill_patch_apply_memory_key(record),
        content: serde_json::to_string(record)?,
        category: skill_patch_apply_memory_category(),
        timestamp: observed_at.to_rfc3339(),
        session_id: None,
        score: None,
    })
}

pub fn parse_skill_patch_apply_entry(entry: &MemoryEntry) -> Option<SkillPatchApplyRecord> {
    if entry.category != skill_patch_apply_memory_category() {
        return None;
    }
    serde_json::from_str::<SkillPatchApplyRecord>(&entry.content).ok()
}

pub fn skill_patch_rollback_to_memory_entry(
    record: &SkillPatchRollbackRecord,
    observed_at: chrono::DateTime<chrono::Utc>,
) -> Result<MemoryEntry, serde_json::Error> {
    Ok(MemoryEntry {
        id: String::new(),
        key: skill_patch_rollback_memory_key(record),
        content: serde_json::to_string(record)?,
        category: skill_patch_rollback_memory_category(),
        timestamp: observed_at.to_rfc3339(),
        session_id: None,
        score: None,
    })
}

pub fn parse_skill_patch_rollback_entry(entry: &MemoryEntry) -> Option<SkillPatchRollbackRecord> {
    if entry.category != skill_patch_rollback_memory_category() {
        return None;
    }
    serde_json::from_str::<SkillPatchRollbackRecord>(&entry.content).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPatchCandidatePolicy {
    pub min_matching_repairs: usize,
    pub require_resolved_repair: bool,
    pub max_detail_chars: usize,
}

impl Default for SkillPatchCandidatePolicy {
    fn default() -> Self {
        Self {
            min_matching_repairs: 2,
            require_resolved_repair: true,
            max_detail_chars: 240,
        }
    }
}

pub fn build_skill_patch_candidates_from_repairs(
    skill: &Skill,
    traces: &[ToolRepairTrace],
    policy: &SkillPatchCandidatePolicy,
) -> Vec<SkillPatchCandidate> {
    let mut groups: BTreeMap<String, Vec<&ToolRepairTrace>> = BTreeMap::new();
    for trace in traces
        .iter()
        .filter(|trace| !trace.tool_name.trim().is_empty())
    {
        groups
            .entry(repair_group_key(trace))
            .or_default()
            .push(trace);
    }

    let mut candidates = Vec::new();
    for (group_key, group) in groups {
        if group.len() < policy.min_matching_repairs.max(1) {
            continue;
        }
        let resolved_count = group
            .iter()
            .filter(|trace| trace.repair_outcome == ToolRepairOutcome::Resolved)
            .count();
        if policy.require_resolved_repair && resolved_count == 0 {
            continue;
        }
        let Some(first) = group.first().copied() else {
            continue;
        };

        let evidence = group
            .iter()
            .enumerate()
            .map(|(idx, trace)| SkillEvidenceRef {
                kind: SkillEvidenceKind::RepairTrace,
                id: repair_trace_id(skill, trace, idx),
                summary: Some(repair_trace_summary(trace, policy.max_detail_chars)),
                metadata: repair_trace_metadata(trace),
            })
            .collect::<Vec<_>>();

        let replay_criteria = vec![
            format!(
                "Replay a task using tool `{}` that previously hit `{}` and verify the repaired path succeeds.",
                first.tool_name,
                tool_failure_kind_name(first.failure_kind)
            ),
            "Compare with-skill and without-skill behavior before promotion.".to_string(),
            "Reject if a newer repair trace contradicts the proposed procedure.".to_string(),
        ];

        let repair_note = format_repair_note(skill, first, group.len(), resolved_count, policy);
        candidates.push(SkillPatchCandidate {
            id: format!("patch:{}:{}", stable_id_part(&skill.id), group_key),
            target_skill_id: skill.id.clone(),
            target_version: skill.version,
            diff_summary: format!(
                "Add repair guidance for `{}` `{}` after {} matching traces ({} resolved).",
                first.tool_name,
                tool_failure_kind_name(first.failure_kind),
                group.len(),
                resolved_count
            ),
            proposed_body: append_repair_note(&skill.content, &repair_note),
            procedure_claims: repair_procedure_claims(&group),
            provenance: evidence,
            replay_criteria,
            eval_results: Vec::new(),
            status: SkillStatus::Candidate,
        });
    }

    candidates
}

fn repair_group_key(trace: &ToolRepairTrace) -> String {
    format!(
        "{}-{}-{}",
        stable_id_part(&trace.tool_name),
        tool_failure_kind_name(trace.failure_kind),
        tool_repair_action_name(trace.suggested_action)
    )
}

fn repair_trace_id(skill: &Skill, trace: &ToolRepairTrace, idx: usize) -> String {
    format!(
        "repair:{}:{}:{}:{}",
        stable_id_part(&skill.id),
        stable_id_part(&trace.tool_name),
        trace.observed_at_unix.max(0),
        idx
    )
}

fn repair_trace_summary(trace: &ToolRepairTrace, max_detail_chars: usize) -> String {
    let mut parts = vec![
        format!("tool={}", trace.tool_name),
        format!("failure={}", tool_failure_kind_name(trace.failure_kind)),
        format!("action={}", tool_repair_action_name(trace.suggested_action)),
        format!("outcome={}", tool_repair_outcome_name(trace.repair_outcome)),
    ];
    if let Some(detail) = trace
        .detail
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("detail={}", bounded(detail, max_detail_chars)));
    }
    parts.join("; ")
}

fn repair_trace_metadata(trace: &ToolRepairTrace) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "tool_name": trace.tool_name,
        "failure_kind": tool_failure_kind_name(trace.failure_kind),
        "suggested_action": tool_repair_action_name(trace.suggested_action),
        "repair_outcome": tool_repair_outcome_name(trace.repair_outcome),
        "observed_at_unix": trace.observed_at_unix,
        "has_detail": trace.detail.as_deref().is_some_and(|value| !value.trim().is_empty()),
    });
    if let Some(replay_args) = &trace.replay_args {
        metadata["replay_args"] = replay_args.clone();
    }
    metadata
}

fn repair_procedure_claims(group: &[&ToolRepairTrace]) -> Vec<SkillPatchProcedureClaim> {
    let mut claims = Vec::new();
    for trace in group {
        let claim = SkillPatchProcedureClaim {
            tool_name: trace.tool_name.trim().to_string(),
            failure_kind: tool_failure_kind_name(trace.failure_kind).to_string(),
            suggested_action: tool_repair_action_name(trace.suggested_action).to_string(),
        };
        if !claim.tool_name.is_empty() && !claims.iter().any(|existing| existing == &claim) {
            claims.push(claim);
        }
    }
    claims
}

fn format_repair_note(
    skill: &Skill,
    trace: &ToolRepairTrace,
    trace_count: usize,
    resolved_count: usize,
    policy: &SkillPatchCandidatePolicy,
) -> String {
    let detail = trace
        .detail
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| bounded(value, policy.max_detail_chars))
        .unwrap_or_else(|| "no detail captured".to_string());
    let task_family = skill
        .task_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unspecified");

    format!(
        "## Candidate Repair Guidance\n\n- Task family: {task_family}\n- Tool: `{}`\n- Failure kind: `{}`\n- Suggested action: `{}`\n- Evidence: {trace_count} matching repair traces, {resolved_count} resolved\n- Operator note: {detail}\n\nKeep this candidate inactive until replay/eval confirms the repaired procedure improves behavior.",
        trace.tool_name,
        tool_failure_kind_name(trace.failure_kind),
        tool_repair_action_name(trace.suggested_action)
    )
}

fn append_repair_note(content: &str, repair_note: &str) -> String {
    let trimmed = content.trim_end();
    if trimmed.is_empty() {
        return repair_note.to_string();
    }
    format!("{trimmed}\n\n{repair_note}")
}

fn bounded(value: &str, max_chars: usize) -> String {
    let limit = max_chars.max(1);
    let mut out = value.trim().chars().take(limit).collect::<String>();
    if value.trim().chars().count() > limit {
        out.push_str("...");
    }
    out
}

fn stable_id_part(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::memory::{SkillOrigin, SkillStatus};
    use crate::domain::tool_repair::{
        ToolFailureKind, ToolRepairAction, ToolRepairOutcome, ToolRepairTrace,
    };
    use chrono::Utc;

    fn test_skill() -> Skill {
        Skill {
            id: "skill-matrix-upgrade".into(),
            name: "Matrix Upgrade".into(),
            description: "Upgrade self-hosted Matrix safely".into(),
            content: "# Matrix Upgrade\n\nCheck current version before changing anything.".into(),
            task_family: Some("matrix-upgrade".into()),
            tool_pattern: vec!["shell".into(), "web".into()],
            lineage_task_families: Vec::new(),
            tags: vec!["ops".into()],
            success_count: 3,
            fail_count: 0,
            version: 4,
            origin: SkillOrigin::Learned,
            status: SkillStatus::Active,
            created_by: "agent".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn repair_trace(outcome: ToolRepairOutcome, observed_at_unix: i64) -> ToolRepairTrace {
        ToolRepairTrace {
            observed_at_unix,
            tool_name: "shell".into(),
            failure_kind: ToolFailureKind::MissingResource,
            suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
            repair_outcome: outcome,
            detail: Some("repository path moved under /opt/matrix before upgrade".into()),
            ..ToolRepairTrace::default()
        }
    }

    #[test]
    fn patch_candidate_round_trips_through_memory_entry() {
        let skill = test_skill();
        let traces = vec![
            repair_trace(ToolRepairOutcome::Resolved, 100),
            repair_trace(ToolRepairOutcome::Resolved, 200),
        ];
        let candidate = build_skill_patch_candidates_from_repairs(
            &skill,
            &traces,
            &SkillPatchCandidatePolicy::default(),
        )
        .pop()
        .expect("candidate");

        let entry = skill_patch_candidate_to_memory_entry(&candidate, Utc::now()).unwrap();
        assert_eq!(entry.category, skill_patch_candidate_memory_category());
        assert_eq!(entry.key, skill_patch_candidate_memory_key(&candidate));
        assert_eq!(parse_skill_patch_candidate_entry(&entry), Some(candidate));
    }

    #[test]
    fn repeated_resolved_repairs_create_candidate_patch() {
        let skill = test_skill();
        let traces = vec![
            repair_trace(ToolRepairOutcome::Resolved, 100),
            repair_trace(ToolRepairOutcome::Resolved, 200),
        ];

        let candidates = build_skill_patch_candidates_from_repairs(
            &skill,
            &traces,
            &SkillPatchCandidatePolicy::default(),
        );

        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.status, SkillStatus::Candidate);
        assert_eq!(candidate.target_skill_id, skill.id);
        assert_eq!(candidate.target_version, 4);
        assert!(candidate.diff_summary.contains("2 matching traces"));
        assert!(candidate
            .proposed_body
            .contains("Candidate Repair Guidance"));
        assert_eq!(candidate.procedure_claims.len(), 1);
        assert_eq!(candidate.procedure_claims[0].tool_name, "shell");
        assert_eq!(
            candidate.procedure_claims[0].failure_kind,
            "missing_resource"
        );
        assert_eq!(
            candidate.procedure_claims[0].suggested_action,
            "adjust_arguments_or_target"
        );
        assert!(candidate
            .provenance
            .iter()
            .all(|e| e.kind == SkillEvidenceKind::RepairTrace));
        assert!(candidate
            .replay_criteria
            .iter()
            .any(|criterion| criterion.contains("with-skill and without-skill")));
    }

    #[test]
    fn single_repair_trace_is_not_enough() {
        let candidates = build_skill_patch_candidates_from_repairs(
            &test_skill(),
            &[repair_trace(ToolRepairOutcome::Resolved, 100)],
            &SkillPatchCandidatePolicy::default(),
        );
        assert!(candidates.is_empty());
    }

    #[test]
    fn unresolved_cluster_is_not_promoted_to_patch_when_resolution_required() {
        let traces = vec![
            repair_trace(ToolRepairOutcome::Failed, 100),
            repair_trace(ToolRepairOutcome::Failed, 200),
        ];
        let candidates = build_skill_patch_candidates_from_repairs(
            &test_skill(),
            &traces,
            &SkillPatchCandidatePolicy::default(),
        );
        assert!(candidates.is_empty());
    }
}
