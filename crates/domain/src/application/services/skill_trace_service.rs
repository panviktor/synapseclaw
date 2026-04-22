//! Compact skill activation/use trace records.
//!
//! These records are audit material, not provider prompt material. They keep
//! skill identity and governance outcomes available for later compaction,
//! diagnostics, and learning without replaying full skill bodies.

use crate::application::services::skill_candidate_eval_service::SkillCandidateEvalReport;
use crate::application::services::skill_governance_service::{
    SkillActivationTrace, SkillBlockedTraceReason, SkillEvidenceKind, SkillEvidenceRef,
    SkillPatchCandidate, SkillResolutionReport, SkillUseOutcome, SkillUseTrace,
};
use crate::domain::memory::{MemoryCategory, MemoryEntry, Skill};
use crate::domain::tool_fact::{ToolFactPayload, TypedToolFact};
use crate::domain::tool_repair::{
    tool_failure_kind_name, tool_repair_action_name, tool_repair_outcome_name, ToolRepairOutcome,
    ToolRepairTrace,
};
use std::collections::HashSet;

pub const SKILL_ACTIVATION_TRACE_MEMORY_CATEGORY: &str = "skill_activation_trace";
pub const SKILL_USE_TRACE_MEMORY_CATEGORY: &str = "skill_use_trace";

pub fn skill_activation_trace_memory_category() -> MemoryCategory {
    MemoryCategory::Custom(SKILL_ACTIVATION_TRACE_MEMORY_CATEGORY.to_string())
}

pub fn skill_use_trace_memory_category() -> MemoryCategory {
    MemoryCategory::Custom(SKILL_USE_TRACE_MEMORY_CATEGORY.to_string())
}

pub fn build_skill_activation_trace(
    report: &SkillResolutionReport,
    loaded_skill_ids: Vec<String>,
    route_model: Option<String>,
    outcome: Option<String>,
) -> SkillActivationTrace {
    let selected_skill_ids = report
        .decisions
        .iter()
        .filter(|decision| decision.loadable())
        .map(|decision| decision.id.clone())
        .collect::<Vec<_>>();
    let blocked_reasons = report
        .decisions
        .iter()
        .filter(|decision| !decision.loadable())
        .map(|decision| SkillBlockedTraceReason {
            skill_id: decision.id.clone(),
            state: decision.state,
            reason_code: decision.reason_code.clone(),
            shadowed_by: decision.shadowed_by.clone(),
        })
        .collect::<Vec<_>>();
    let blocked_skill_ids = blocked_reasons
        .iter()
        .map(|reason| reason.skill_id.clone())
        .collect::<Vec<_>>();

    SkillActivationTrace {
        selected_skill_ids,
        loaded_skill_ids,
        blocked_skill_ids,
        blocked_reasons,
        budget_catalog_entries: report.provider_catalog().len(),
        budget_preloaded_skills: report.runtime_preloads().len(),
        route_model,
        outcome,
    }
}

pub fn skill_activation_trace_memory_key(
    agent_id: &str,
    observed_at: chrono::DateTime<chrono::Utc>,
    trace: &SkillActivationTrace,
) -> String {
    let primary = trace
        .loaded_skill_ids
        .first()
        .or_else(|| trace.selected_skill_ids.first())
        .or_else(|| trace.blocked_skill_ids.first())
        .map(|id| stable_key_part(id))
        .unwrap_or_else(|| "none".to_string());
    format!(
        "{}:{}:{}",
        SKILL_ACTIVATION_TRACE_MEMORY_CATEGORY,
        stable_key_part(agent_id),
        format!("{}-{primary}", observed_at.timestamp_millis())
    )
}

pub fn skill_activation_trace_memory_key_prefix(agent_id: &str) -> String {
    format!(
        "{}:{}:",
        SKILL_ACTIVATION_TRACE_MEMORY_CATEGORY,
        stable_key_part(agent_id)
    )
}

pub fn skill_use_trace_memory_key(agent_id: &str, trace: &SkillUseTrace) -> String {
    format!(
        "{}:{}:{}",
        SKILL_USE_TRACE_MEMORY_CATEGORY,
        stable_key_part(agent_id),
        stable_key_part(&trace.id)
    )
}

pub fn skill_use_trace_memory_key_prefix(agent_id: &str) -> String {
    format!(
        "{}:{}:",
        SKILL_USE_TRACE_MEMORY_CATEGORY,
        stable_key_part(agent_id)
    )
}

pub fn build_skill_use_trace_from_patch_replay(
    patch: &SkillPatchCandidate,
    target_skill: &Skill,
    promotion_report: &SkillCandidateEvalReport,
    observed_at_unix: i64,
) -> SkillUseTrace {
    SkillUseTrace {
        id: format!(
            "skill_use:{}:{}",
            stable_key_part(&patch.id),
            observed_at_unix.max(0)
        ),
        skill_id: target_skill.id.clone(),
        task_family: target_skill.task_family.clone(),
        route_model: None,
        tool_pattern: target_skill.tool_pattern.clone(),
        outcome: if promotion_report.promotion_allowed {
            SkillUseOutcome::Repaired
        } else {
            SkillUseOutcome::Failed
        },
        verification: Some(format!(
            "{} passed={} failed={} missing={}",
            promotion_report.reason,
            promotion_report.passed_count,
            promotion_report.failed_count,
            promotion_report.missing_count
        )),
        repair_evidence: patch.provenance.clone(),
        observed_at_unix,
    }
}

pub fn build_skill_use_trace_from_live_turn(
    skill: &Skill,
    activation_trace: &SkillActivationTrace,
    tools_used: &[String],
    tool_facts: &[TypedToolFact],
    tool_repairs: &[ToolRepairTrace],
    observed_at_unix: i64,
) -> SkillUseTrace {
    let relevant_repairs = relevant_repairs_for_skill(skill, tool_repairs);
    let relevant_failure_outcomes = relevant_failure_outcome_count(skill, tool_facts);
    let outcome = live_turn_skill_outcome(&relevant_repairs, relevant_failure_outcomes);
    let repair_evidence = relevant_repairs
        .iter()
        .enumerate()
        .map(|(index, trace)| repair_trace_evidence_ref(skill, trace, observed_at_unix, index))
        .collect::<Vec<_>>();
    let tools_used_count = count_matching_tools(skill, tools_used);

    SkillUseTrace {
        id: format!(
            "skill_use:live_turn:{}:{}",
            stable_key_part(&skill.id),
            observed_at_unix.max(0)
        ),
        skill_id: skill.id.clone(),
        task_family: skill.task_family.clone(),
        route_model: activation_trace.route_model.clone(),
        tool_pattern: skill.tool_pattern.clone(),
        outcome,
        verification: Some(format!(
            "typed_turn_outcome matching_tools={} repairs_resolved={} repairs_failed={} repairs_downgraded={} failure_outcomes={} activation_outcome={}",
            tools_used_count,
            relevant_repairs
                .iter()
                .filter(|trace| trace.repair_outcome == ToolRepairOutcome::Resolved)
                .count(),
            relevant_repairs
                .iter()
                .filter(|trace| trace.repair_outcome == ToolRepairOutcome::Failed)
                .count(),
            relevant_repairs
                .iter()
                .filter(|trace| trace.repair_outcome == ToolRepairOutcome::Downgraded)
                .count(),
            relevant_failure_outcomes,
            activation_trace.outcome.as_deref().unwrap_or("unknown")
        )),
        repair_evidence,
        observed_at_unix,
    }
}

pub fn skill_activation_trace_to_memory_entry(
    agent_id: &str,
    trace: &SkillActivationTrace,
    observed_at: chrono::DateTime<chrono::Utc>,
    session_id: Option<&str>,
) -> Result<MemoryEntry, serde_json::Error> {
    Ok(MemoryEntry {
        id: String::new(),
        key: skill_activation_trace_memory_key(agent_id, observed_at, trace),
        content: serde_json::to_string(trace)?,
        category: skill_activation_trace_memory_category(),
        timestamp: observed_at.to_rfc3339(),
        session_id: session_id.map(ToOwned::to_owned),
        score: None,
    })
}

pub fn skill_use_trace_to_memory_entry(
    agent_id: &str,
    trace: &SkillUseTrace,
    observed_at: chrono::DateTime<chrono::Utc>,
    session_id: Option<&str>,
) -> Result<MemoryEntry, serde_json::Error> {
    Ok(MemoryEntry {
        id: String::new(),
        key: skill_use_trace_memory_key(agent_id, trace),
        content: serde_json::to_string(trace)?,
        category: skill_use_trace_memory_category(),
        timestamp: observed_at.to_rfc3339(),
        session_id: session_id.map(ToOwned::to_owned),
        score: None,
    })
}

pub fn parse_skill_activation_trace_entry(entry: &MemoryEntry) -> Option<SkillActivationTrace> {
    if entry.category != skill_activation_trace_memory_category() {
        return None;
    }
    serde_json::from_str::<SkillActivationTrace>(&entry.content).ok()
}

pub fn parse_skill_use_trace_entry(entry: &MemoryEntry) -> Option<SkillUseTrace> {
    if entry.category != skill_use_trace_memory_category() {
        return None;
    }
    serde_json::from_str::<SkillUseTrace>(&entry.content).ok()
}

fn stable_key_part(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "none".to_string()
    } else {
        trimmed.to_string()
    }
}

fn relevant_repairs_for_skill(
    skill: &Skill,
    tool_repairs: &[ToolRepairTrace],
) -> Vec<ToolRepairTrace> {
    if skill.tool_pattern.is_empty() {
        return Vec::new();
    }
    let skill_tools = normalized_skill_tool_set(skill);
    tool_repairs
        .iter()
        .filter(|trace| skill_tools.contains(&normalize_tool_name(&trace.tool_name)))
        .cloned()
        .collect()
}

fn count_matching_tools(skill: &Skill, tools_used: &[String]) -> usize {
    if skill.tool_pattern.is_empty() {
        return 0;
    }
    let skill_tools = normalized_skill_tool_set(skill);
    tools_used
        .iter()
        .filter(|tool| skill_tools.contains(&normalize_tool_name(tool)))
        .count()
}

fn relevant_failure_outcome_count(skill: &Skill, tool_facts: &[TypedToolFact]) -> usize {
    if skill.tool_pattern.is_empty() {
        return 0;
    }
    let skill_tools = normalized_skill_tool_set(skill);
    tool_facts
        .iter()
        .filter(|fact| skill_tools.contains(&normalize_tool_name(&fact.tool_id)))
        .filter(|fact| match &fact.payload {
            ToolFactPayload::Outcome(outcome) => outcome.status.is_failure(),
            _ => false,
        })
        .count()
}

fn normalized_skill_tool_set(skill: &Skill) -> HashSet<String> {
    skill
        .tool_pattern
        .iter()
        .map(|tool| normalize_tool_name(tool))
        .filter(|tool| !tool.is_empty())
        .collect()
}

fn normalize_tool_name(value: &str) -> String {
    value.trim().to_lowercase()
}

fn live_turn_skill_outcome(
    relevant_repairs: &[ToolRepairTrace],
    relevant_failure_outcomes: usize,
) -> SkillUseOutcome {
    if relevant_repairs
        .iter()
        .any(|trace| trace.repair_outcome == ToolRepairOutcome::Resolved)
    {
        SkillUseOutcome::Repaired
    } else if relevant_failure_outcomes > 0
        || relevant_repairs.iter().any(|trace| {
            matches!(
                trace.repair_outcome,
                ToolRepairOutcome::Failed | ToolRepairOutcome::Downgraded
            )
        })
    {
        SkillUseOutcome::Failed
    } else {
        SkillUseOutcome::Succeeded
    }
}

fn repair_trace_evidence_ref(
    skill: &Skill,
    trace: &ToolRepairTrace,
    observed_at_unix: i64,
    index: usize,
) -> SkillEvidenceRef {
    SkillEvidenceRef {
        kind: SkillEvidenceKind::RepairTrace,
        id: format!(
            "turn_repair:{}:{}:{}:{}",
            stable_key_part(&skill.id),
            stable_key_part(&trace.tool_name),
            observed_at_unix.max(0),
            index
        ),
        summary: Some(format!(
            "tool={} outcome={} action={}",
            trace.tool_name,
            tool_repair_outcome_name(trace.repair_outcome),
            tool_repair_action_name(trace.suggested_action)
        )),
        metadata: serde_json::json!({
            "tool_name": trace.tool_name,
            "failure_kind": tool_failure_kind_name(trace.failure_kind),
            "suggested_action": tool_repair_action_name(trace.suggested_action),
            "repair_outcome": tool_repair_outcome_name(trace.repair_outcome),
            "observed_at_unix": trace.observed_at_unix,
            "repeat_count": trace.repeat_count,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::skill_governance_service::{
        resolve_skill_states, SkillEvidenceKind, SkillEvidenceRef, SkillLoadRequest,
        SkillPatchCandidate, SkillReplayEvalResult, SkillReplayEvalStatus, SkillRuntimeCandidate,
        SkillRuntimeState, SkillSource, SkillTrustLevel,
    };
    use crate::domain::memory::{Skill, SkillOrigin, SkillStatus};
    use crate::domain::tool_fact::{OutcomeStatus, TypedToolFact};
    use crate::domain::tool_repair::{
        ToolFailureKind, ToolRepairAction, ToolRepairOutcome, ToolRepairTrace,
    };

    fn candidate(name: &str, source: SkillSource) -> SkillRuntimeCandidate {
        SkillRuntimeCandidate {
            id: name.into(),
            name: name.into(),
            description: "test skill".into(),
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

    fn learned_skill() -> Skill {
        Skill {
            id: "skill-a".into(),
            name: "Skill A".into(),
            description: "test".into(),
            content: "# Skill A".into(),
            task_family: Some("matrix-upgrade".into()),
            tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
            lineage_task_families: Vec::new(),
            tags: vec!["matrix".into()],
            success_count: 2,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Learned,
            status: SkillStatus::Active,
            created_by: "agent".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn patch_candidate() -> SkillPatchCandidate {
        SkillPatchCandidate {
            id: "patch-a".into(),
            target_skill_id: "skill-a".into(),
            target_version: 1,
            diff_summary: "add repair".into(),
            proposed_body: "# Skill A\n\n## Candidate Repair Guidance".into(),
            procedure_claims: Vec::new(),
            provenance: vec![SkillEvidenceRef {
                kind: SkillEvidenceKind::RepairTrace,
                id: "repair-a".into(),
                summary: Some("resolved repair".into()),
                metadata: serde_json::json!({"repair_outcome": "resolved"}),
            }],
            replay_criteria: vec!["compare with-skill and without-skill".into()],
            eval_results: Vec::new(),
            status: SkillStatus::Candidate,
        }
    }

    #[test]
    fn activation_trace_records_selected_loaded_and_blocked_without_body() {
        let active = candidate("matrix-active", SkillSource::Manual);
        let mut blocked = candidate("needs-browser", SkillSource::Manual);
        blocked.required_tools = vec!["browser".into()];
        let report = resolve_skill_states(&SkillLoadRequest::default(), vec![active, blocked]);

        let trace = build_skill_activation_trace(
            &report,
            vec!["matrix-active".into()],
            Some("deepseek".into()),
            Some("loaded".into()),
        );
        let entry = skill_activation_trace_to_memory_entry(
            "agent",
            &trace,
            chrono::Utc::now(),
            Some("session-a"),
        )
        .unwrap();
        let parsed = parse_skill_activation_trace_entry(&entry).unwrap();

        assert_eq!(parsed.loaded_skill_ids, vec!["matrix-active"]);
        assert!(parsed.selected_skill_ids.contains(&"matrix-active".into()));
        assert!(parsed.blocked_skill_ids.contains(&"needs-browser".into()));
        assert_eq!(
            parsed.blocked_reasons[0].state,
            SkillRuntimeState::BlockedMissingCapability
        );
        assert!(!entry.content.contains("full skill body"));
    }

    #[test]
    fn patch_replay_use_trace_records_eval_outcome_without_body() {
        let report = SkillCandidateEvalReport {
            candidate_id: "patch-a".into(),
            candidate_kind: "patch",
            promotion_allowed: true,
            reason: "passed_replay_eval_gate",
            passed_count: 2,
            failed_count: 0,
            missing_count: 0,
            criteria: vec![SkillReplayEvalResult {
                criterion: "compare with-skill and without-skill".into(),
                status: SkillReplayEvalStatus::Passed,
                evidence: Some("passed comparison".into()),
                observed_at_unix: 42,
            }],
        };
        let trace = build_skill_use_trace_from_patch_replay(
            &patch_candidate(),
            &learned_skill(),
            &report,
            123,
        );
        let entry =
            skill_use_trace_to_memory_entry("agent", &trace, chrono::Utc::now(), None).unwrap();
        let parsed = parse_skill_use_trace_entry(&entry).unwrap();

        assert_eq!(parsed.skill_id, "skill-a");
        assert_eq!(parsed.outcome, SkillUseOutcome::Repaired);
        assert_eq!(
            parsed.tool_pattern,
            vec!["repo_discovery", "git_operations"]
        );
        assert!(parsed
            .verification
            .as_deref()
            .is_some_and(|value| value.contains("passed_replay_eval_gate")));
        assert!(!entry.content.contains("# Skill A"));
    }

    #[test]
    fn live_turn_use_trace_uses_typed_repair_outcome_without_body() {
        let skill = learned_skill();
        let activation = SkillActivationTrace {
            selected_skill_ids: vec!["skill-a".into()],
            loaded_skill_ids: vec!["skill-a".into()],
            blocked_skill_ids: Vec::new(),
            blocked_reasons: Vec::new(),
            budget_catalog_entries: 1,
            budget_preloaded_skills: 0,
            route_model: Some("deepseek".into()),
            outcome: Some("loaded".into()),
        };
        let trace = build_skill_use_trace_from_live_turn(
            &skill,
            &activation,
            &["repo_discovery".into()],
            &[TypedToolFact::outcome(
                "repo_discovery",
                OutcomeStatus::Succeeded,
                Some(20),
            )],
            &[ToolRepairTrace {
                observed_at_unix: 120,
                tool_name: "repo_discovery".into(),
                failure_kind: ToolFailureKind::MissingResource,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                repair_outcome: ToolRepairOutcome::Resolved,
                ..ToolRepairTrace::default()
            }],
            123,
        );
        let entry =
            skill_use_trace_to_memory_entry("agent", &trace, chrono::Utc::now(), None).unwrap();
        let parsed = parse_skill_use_trace_entry(&entry).unwrap();

        assert_eq!(parsed.skill_id, "skill-a");
        assert_eq!(parsed.outcome, SkillUseOutcome::Repaired);
        assert_eq!(parsed.route_model.as_deref(), Some("deepseek"));
        assert_eq!(parsed.repair_evidence.len(), 1);
        assert!(parsed
            .verification
            .as_deref()
            .is_some_and(|value| value.contains("typed_turn_outcome")));
        assert!(!entry.content.contains("# Skill A"));
    }

    #[test]
    fn live_turn_use_trace_ignores_unrelated_tool_failure() {
        let skill = learned_skill();
        let activation = SkillActivationTrace {
            selected_skill_ids: vec!["skill-a".into()],
            loaded_skill_ids: vec!["skill-a".into()],
            blocked_skill_ids: Vec::new(),
            blocked_reasons: Vec::new(),
            budget_catalog_entries: 1,
            budget_preloaded_skills: 0,
            route_model: None,
            outcome: Some("loaded".into()),
        };
        let trace = build_skill_use_trace_from_live_turn(
            &skill,
            &activation,
            &["repo_discovery".into()],
            &[TypedToolFact::outcome(
                "message_send",
                OutcomeStatus::RuntimeError,
                Some(20),
            )],
            &[],
            123,
        );

        assert_eq!(trace.outcome, SkillUseOutcome::Succeeded);
        assert!(trace
            .verification
            .as_deref()
            .is_some_and(|value| value.contains("failure_outcomes=0")));
    }
}
