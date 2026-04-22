use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use synapse_domain::application::services::skill_candidate_eval_service::{
    run_skill_patch_replay_harness, SkillCandidateEvalPolicy, SkillReplayCase, SkillReplayCaseKind,
    SkillReplayHarnessPort, SkillReplayHarnessReport,
};
use synapse_domain::application::services::skill_governance_service::{
    SkillPatchCandidate, SkillPatchProcedureClaim, SkillReplayEvalResult, SkillReplayEvalStatus,
};
use synapse_domain::application::services::skill_patch_candidate_service;
use synapse_domain::application::services::skill_trace_service::{
    build_skill_use_trace_from_patch_replay, skill_use_trace_to_memory_entry,
};
use synapse_domain::application::services::tool_repair::sanitized_tool_replay_args_with_contract;
use synapse_domain::domain::memory::{MemoryEntry, Skill as MemorySkill};
use synapse_memory::UnifiedMemoryPort;

use crate::tools::{Tool, ToolResult};

pub struct RuntimeSkillReplayHarness<'a> {
    tools: &'a [Box<dyn Tool>],
    target_skill: Option<&'a MemorySkill>,
}

impl<'a> RuntimeSkillReplayHarness<'a> {
    pub fn new(tools: &'a [Box<dyn Tool>], target_skill: Option<&'a MemorySkill>) -> Self {
        Self {
            tools,
            target_skill,
        }
    }

    fn tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .map(|tool| tool.as_ref())
            .find(|tool| tool.name() == name)
    }
}

#[async_trait]
impl SkillReplayHarnessPort for RuntimeSkillReplayHarness<'_> {
    async fn run_replay_case(
        &self,
        candidate: &SkillPatchCandidate,
        case: &SkillReplayCase,
    ) -> Result<SkillReplayEvalResult, String> {
        Ok(match case.kind {
            SkillReplayCaseKind::RepairTraceReplay => run_tool_replay_case(self, case).await,
            SkillReplayCaseKind::WithWithoutComparison => {
                run_static_patch_comparison(candidate, self.target_skill, case)
            }
            SkillReplayCaseKind::ContradictionScan => {
                run_candidate_contradiction_scan(candidate, case)
            }
            SkillReplayCaseKind::OperatorReview => eval_result(
                case,
                SkillReplayEvalStatus::Missing,
                "operator review is required before this replay criterion can pass",
            ),
        })
    }
}

async fn run_tool_replay_case(
    harness: &RuntimeSkillReplayHarness<'_>,
    case: &SkillReplayCase,
) -> SkillReplayEvalResult {
    let Some(tool_name) = case.required_tool.as_deref() else {
        return eval_result(
            case,
            SkillReplayEvalStatus::Missing,
            "replay case has no required tool metadata",
        );
    };
    let Some(tool) = harness.tool(tool_name) else {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!("required tool `{tool_name}` is not registered in the runtime tool registry"),
        );
    };
    let Some(args) = case.tool_args.clone() else {
        return eval_result(
            case,
            SkillReplayEvalStatus::Missing,
            format!(
                "required tool `{tool_name}` is registered, but the candidate has no sanitized replay_args payload"
            ),
        );
    };

    let tool_spec = tool.spec();
    let contract = tool.tool_contract();
    let Some(args) =
        sanitized_tool_replay_args_with_contract(tool_name, &args, &tool_spec, &contract)
    else {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!(
                "required tool `{tool_name}` is registered, but its replay_args are not allowed by the typed tool contract"
            ),
        );
    };

    match tool.execute(args).await {
        Ok(ToolResult {
            success: true,
            output,
            ..
        }) => eval_result(
            case,
            SkillReplayEvalStatus::Passed,
            format!(
                "tool `{tool_name}` replay succeeded: {}",
                bounded(&output, 240)
            ),
        ),
        Ok(ToolResult {
            success: false,
            output,
            error,
        }) => eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!(
                "tool `{tool_name}` replay failed: {}{}",
                error.unwrap_or_default(),
                if output.trim().is_empty() {
                    String::new()
                } else {
                    format!(" output={}", bounded(&output, 240))
                }
            ),
        ),
        Err(error) => eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!("tool `{tool_name}` replay errored: {error}"),
        ),
    }
}

fn run_static_patch_comparison(
    candidate: &SkillPatchCandidate,
    target_skill: Option<&MemorySkill>,
    case: &SkillReplayCase,
) -> SkillReplayEvalResult {
    let Some(target_skill) = target_skill else {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            "target skill is not available for with-skill/without-skill comparison",
        );
    };
    if target_skill.id != candidate.target_skill_id {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!(
                "target skill mismatch: candidate targets `{}`, loaded `{}`",
                candidate.target_skill_id, target_skill.id
            ),
        );
    }
    if target_skill.version != candidate.target_version {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!(
                "stale patch target: candidate targets v{}, current skill is v{}",
                candidate.target_version, target_skill.version
            ),
        );
    }
    if candidate.proposed_body.trim() == target_skill.content.trim() {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            "proposed body is identical to the current target skill",
        );
    }
    if !candidate.provenance.iter().any(is_resolved_repair_evidence) {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            "candidate has no resolved repair provenance for with-skill/without-skill comparison",
        );
    }
    let claims = valid_candidate_procedure_claims(candidate);
    if claims.is_empty() {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            "candidate has no typed repair procedure claims for comparison",
        );
    }
    let provenance_claims = repair_procedure_claims_from_provenance(candidate);
    let unsupported_claims = claims
        .iter()
        .filter(|claim| !provenance_claims.contains(claim))
        .map(|claim| format_procedure_claim(claim))
        .collect::<Vec<_>>();
    if !unsupported_claims.is_empty() {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            format!(
                "typed repair procedure claims are not backed by resolved repair provenance: {}",
                unsupported_claims.join(", ")
            ),
        );
    }
    eval_result(
        case,
        SkillReplayEvalStatus::Passed,
        format!(
            "procedural with/without comparison passed: candidate adds typed repair procedure claims {}",
            claims
                .iter()
                .map(|claim| format_procedure_claim(claim))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

fn valid_candidate_procedure_claims(
    candidate: &SkillPatchCandidate,
) -> Vec<SkillPatchProcedureClaim> {
    candidate
        .procedure_claims
        .iter()
        .filter(|claim| valid_procedure_claim(claim))
        .cloned()
        .collect()
}

fn repair_procedure_claims_from_provenance(
    candidate: &SkillPatchCandidate,
) -> Vec<SkillPatchProcedureClaim> {
    let mut claims = Vec::new();
    for evidence in &candidate.provenance {
        if !is_resolved_repair_evidence(evidence) {
            continue;
        }
        let claim = SkillPatchProcedureClaim {
            tool_name: metadata_string(evidence, "tool_name"),
            failure_kind: metadata_string(evidence, "failure_kind"),
            suggested_action: metadata_string(evidence, "suggested_action"),
        };
        if valid_procedure_claim(&claim) && !claims.iter().any(|existing| existing == &claim) {
            claims.push(claim);
        }
    }
    claims
}

fn metadata_string(
    evidence: &synapse_domain::application::services::skill_governance_service::SkillEvidenceRef,
    key: &str,
) -> String {
    evidence
        .metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn valid_procedure_claim(claim: &SkillPatchProcedureClaim) -> bool {
    !claim.tool_name.trim().is_empty()
        && !claim.failure_kind.trim().is_empty()
        && !claim.suggested_action.trim().is_empty()
}

fn format_procedure_claim(claim: &SkillPatchProcedureClaim) -> String {
    format!(
        "tool={} failure={} action={}",
        claim.tool_name, claim.failure_kind, claim.suggested_action
    )
}

fn is_resolved_repair_evidence(
    evidence: &synapse_domain::application::services::skill_governance_service::SkillEvidenceRef,
) -> bool {
    evidence
        .metadata
        .get("repair_outcome")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value == "resolved")
}

fn run_candidate_contradiction_scan(
    candidate: &SkillPatchCandidate,
    case: &SkillReplayCase,
) -> SkillReplayEvalResult {
    let contradictory = candidate.provenance.iter().any(|evidence| {
        evidence
            .metadata
            .get("repair_outcome")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value != "resolved")
    });
    if contradictory {
        return eval_result(
            case,
            SkillReplayEvalStatus::Failed,
            "candidate provenance contains non-resolved repair evidence",
        );
    }
    eval_result(
        case,
        SkillReplayEvalStatus::Passed,
        "candidate provenance has no contradictory repair outcome metadata",
    )
}

fn eval_result(
    case: &SkillReplayCase,
    status: SkillReplayEvalStatus,
    evidence: impl Into<String>,
) -> SkillReplayEvalResult {
    SkillReplayEvalResult {
        criterion: case.criterion.clone(),
        status,
        evidence: Some(evidence.into()),
        observed_at_unix: chrono::Utc::now().timestamp(),
    }
}

fn bounded(value: &str, max_chars: usize) -> String {
    let mut out = value.trim().chars().take(max_chars).collect::<String>();
    if value.trim().chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

pub async fn run_and_store_skill_patch_replay(
    memory: &dyn UnifiedMemoryPort,
    agent_id: &str,
    candidate_ref: &str,
    tools: &[Box<dyn Tool>],
    limit: usize,
) -> Result<SkillReplayHarnessReport> {
    let candidate = resolve_skill_patch_candidate_ref(memory, candidate_ref, limit).await?;
    let target_skill = memory
        .list_skills(&agent_id.to_string(), 512)
        .await
        .context("failed to list learned skills for patch replay")?
        .into_iter()
        .find(|skill| skill.id == candidate.target_skill_id);
    let harness = RuntimeSkillReplayHarness::new(tools, target_skill.as_ref());
    let report =
        run_skill_patch_replay_harness(&candidate, &harness, &SkillCandidateEvalPolicy::default())
            .await;

    let mut updated = candidate;
    updated.eval_results = report.results.clone();
    replace_skill_patch_candidate(memory, agent_id, &updated).await?;
    if let Some(target_skill) = target_skill.as_ref() {
        let observed_at = chrono::Utc::now();
        let trace = build_skill_use_trace_from_patch_replay(
            &updated,
            target_skill,
            &report.promotion_report,
            observed_at.timestamp(),
        );
        let entry = skill_use_trace_to_memory_entry(agent_id, &trace, observed_at, None)?;
        if let Err(error) = memory.store_episode(entry).await {
            tracing::warn!(%error, "skill replay use trace write failed");
        }
    }

    Ok(report)
}

async fn resolve_skill_patch_candidate_ref(
    memory: &dyn UnifiedMemoryPort,
    candidate_ref: &str,
    limit: usize,
) -> Result<SkillPatchCandidate> {
    let needle = candidate_ref.trim();
    if needle.is_empty() {
        bail!("skill patch candidate id must not be empty");
    }
    let candidates = list_skill_patch_candidates(memory, limit.max(1)).await?;
    candidates
        .into_iter()
        .find(|candidate| {
            candidate.id == needle
                || skill_patch_candidate_service::skill_patch_candidate_memory_key(candidate)
                    == needle
        })
        .ok_or_else(|| anyhow::anyhow!("No skill patch candidate found: {needle}"))
}

async fn list_skill_patch_candidates(
    memory: &dyn UnifiedMemoryPort,
    limit: usize,
) -> Result<Vec<SkillPatchCandidate>> {
    let category = skill_patch_candidate_service::skill_patch_candidate_memory_category();
    let entries = memory.list(Some(&category), None, limit).await?;
    Ok(entries
        .iter()
        .filter_map(skill_patch_candidate_service::parse_skill_patch_candidate_entry)
        .collect())
}

async fn replace_skill_patch_candidate(
    memory: &dyn UnifiedMemoryPort,
    agent_id: &str,
    candidate: &SkillPatchCandidate,
) -> Result<()> {
    let key = skill_patch_candidate_service::skill_patch_candidate_memory_key(candidate);
    let _ = memory.forget(&key, &agent_id.to_string()).await?;
    let entry: MemoryEntry = skill_patch_candidate_service::skill_patch_candidate_to_memory_entry(
        candidate,
        chrono::Utc::now(),
    )?;
    memory.store_episode(entry).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tool;
    use async_trait::async_trait;
    use synapse_domain::application::services::skill_candidate_eval_service::SkillReplayCase;
    use synapse_domain::application::services::skill_governance_service::{
        SkillEvidenceRef, SkillUseOutcome,
    };
    use synapse_domain::application::services::skill_trace_service::{
        parse_skill_use_trace_entry, skill_use_trace_memory_category,
    };
    use synapse_domain::domain::memory::{SkillOrigin, SkillStatus};
    use synapse_domain::ports::tool::{
        ToolArgumentPolicy, ToolContract, ToolNonReplayableReason, ToolRuntimeRole,
    };
    use synapse_memory::{EpisodicMemoryPort, SkillMemoryPort, UnifiedMemoryPort};

    struct ProbeTool;

    #[async_trait]
    impl Tool for ProbeTool {
        fn name(&self) -> &str {
            "probe"
        }

        fn description(&self) -> &str {
            "test probe"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" }
                },
                "required": ["ok"]
            })
        }

        fn runtime_role(&self) -> Option<ToolRuntimeRole> {
            Some(ToolRuntimeRole::WorkspaceDiscovery)
        }

        fn tool_contract(&self) -> ToolContract {
            ToolContract::replayable(self.runtime_role())
                .with_arguments(vec![ToolArgumentPolicy::replayable("ok")])
        }

        async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
            Ok(ToolResult {
                success: args.get("ok").and_then(serde_json::Value::as_bool) == Some(true),
                output: "probe output".into(),
                error: None,
            })
        }
    }

    struct NonReplayableProbeTool;

    #[async_trait]
    impl Tool for NonReplayableProbeTool {
        fn name(&self) -> &str {
            "probe"
        }

        fn description(&self) -> &str {
            "test probe"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" }
                },
                "required": ["ok"]
            })
        }

        fn runtime_role(&self) -> Option<ToolRuntimeRole> {
            Some(ToolRuntimeRole::WorkspaceDiscovery)
        }

        fn tool_contract(&self) -> ToolContract {
            ToolContract::non_replayable(
                self.runtime_role(),
                ToolNonReplayableReason::Other("test_tool".into()),
            )
        }

        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
            panic!("non-replayable tool must not execute from replay harness")
        }
    }

    struct PrivateReplayProbeTool;

    #[async_trait]
    impl Tool for PrivateReplayProbeTool {
        fn name(&self) -> &str {
            "probe"
        }

        fn description(&self) -> &str {
            "test private probe"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            })
        }

        fn runtime_role(&self) -> Option<ToolRuntimeRole> {
            Some(ToolRuntimeRole::HistoricalLookup)
        }

        fn tool_contract(&self) -> ToolContract {
            ToolContract::replayable(self.runtime_role())
                .with_arguments(vec![ToolArgumentPolicy::replayable("query").user_private()])
        }

        async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
            panic!("private replay args must not execute from replay harness")
        }
    }

    fn case(tool_args: Option<serde_json::Value>) -> SkillReplayCase {
        SkillReplayCase {
            candidate_id: "patch-a".into(),
            candidate_kind: "patch",
            criterion: "replay probe".into(),
            kind: SkillReplayCaseKind::RepairTraceReplay,
            target_skill_id: Some("skill-a".into()),
            target_version: Some(1),
            required_tool: Some("probe".into()),
            tool_args,
            provenance_ids: vec!["repair-a".into()],
        }
    }

    fn candidate() -> SkillPatchCandidate {
        SkillPatchCandidate {
            id: "patch-a".into(),
            target_skill_id: "skill-a".into(),
            target_version: 1,
            diff_summary: "add guidance".into(),
            proposed_body:
                "# Skill\n\n## Candidate Repair Guidance\nUse `probe` after `schema_mismatch`; apply `adjust_arguments_or_target`."
                    .into(),
            procedure_claims: vec![SkillPatchProcedureClaim {
                tool_name: "probe".into(),
                failure_kind: "schema_mismatch".into(),
                suggested_action: "adjust_arguments_or_target".into(),
            }],
            provenance: vec![SkillEvidenceRef {
                kind: synapse_domain::application::services::skill_governance_service::SkillEvidenceKind::RepairTrace,
                id: "repair-a".into(),
                summary: None,
                metadata: serde_json::json!({
                    "repair_outcome": "resolved",
                    "tool_name": "probe",
                    "failure_kind": "schema_mismatch",
                    "suggested_action": "adjust_arguments_or_target",
                }),
            }],
            replay_criteria: vec!["replay probe".into()],
            eval_results: Vec::new(),
            status: SkillStatus::Candidate,
        }
    }

    fn target_skill() -> MemorySkill {
        MemorySkill {
            id: "skill-a".into(),
            name: "Skill A".into(),
            description: "test".into(),
            content: "# Skill".into(),
            task_family: None,
            tool_pattern: vec!["probe".into()],
            lineage_task_families: Vec::new(),
            tags: Vec::new(),
            success_count: 1,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Learned,
            status: SkillStatus::Active,
            created_by: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn tool_replay_requires_executable_args() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(ProbeTool)];
        let harness = RuntimeSkillReplayHarness::new(&tools, None);

        let result = harness
            .run_replay_case(&candidate(), &case(None))
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Missing);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("replay_args")));
    }

    #[tokio::test]
    async fn tool_replay_executes_registered_tool_with_args() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(ProbeTool)];
        let harness = RuntimeSkillReplayHarness::new(&tools, None);

        let result = harness
            .run_replay_case(&candidate(), &case(Some(serde_json::json!({"ok": true}))))
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Passed);
    }

    #[tokio::test]
    async fn tool_replay_rejects_unreplayable_tool_contract() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(NonReplayableProbeTool)];
        let harness = RuntimeSkillReplayHarness::new(&tools, None);

        let result = harness
            .run_replay_case(&candidate(), &case(Some(serde_json::json!({"ok": true}))))
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Failed);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("typed tool contract")));
    }

    #[tokio::test]
    async fn tool_replay_rejects_private_replay_args_before_execution() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(PrivateReplayProbeTool)];
        let harness = RuntimeSkillReplayHarness::new(&tools, None);

        let result = harness
            .run_replay_case(
                &candidate(),
                &case(Some(serde_json::json!({"query": "private session text"}))),
            )
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Failed);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("typed tool contract")));
    }

    #[tokio::test]
    async fn static_comparison_fails_stale_target_version() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        let mut target = target_skill();
        target.version = 2;
        let harness = RuntimeSkillReplayHarness::new(&tools, Some(&target));
        let compare_case = SkillReplayCase {
            kind: SkillReplayCaseKind::WithWithoutComparison,
            criterion: "compare".into(),
            required_tool: None,
            tool_args: None,
            ..case(None)
        };

        let result = harness
            .run_replay_case(&candidate(), &compare_case)
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Failed);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("stale patch target")));
    }

    #[tokio::test]
    async fn static_comparison_passes_when_patch_adds_typed_repair_claims() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        let target = target_skill();
        let harness = RuntimeSkillReplayHarness::new(&tools, Some(&target));
        let compare_case = SkillReplayCase {
            kind: SkillReplayCaseKind::WithWithoutComparison,
            criterion: "compare".into(),
            required_tool: None,
            tool_args: None,
            ..case(None)
        };

        let result = harness
            .run_replay_case(&candidate(), &compare_case)
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Passed);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("typed repair procedure claims")));
    }

    #[tokio::test]
    async fn static_comparison_rejects_candidate_without_typed_repair_claims() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        let target = target_skill();
        let harness = RuntimeSkillReplayHarness::new(&tools, Some(&target));
        let mut candidate = candidate();
        candidate.procedure_claims = Vec::new();
        let compare_case = SkillReplayCase {
            kind: SkillReplayCaseKind::WithWithoutComparison,
            criterion: "compare".into(),
            required_tool: None,
            tool_args: None,
            ..case(None)
        };

        let result = harness
            .run_replay_case(&candidate, &compare_case)
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Failed);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("no typed repair procedure claims")));
    }

    #[tokio::test]
    async fn static_comparison_rejects_unbacked_typed_repair_claims() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        let target = target_skill();
        let harness = RuntimeSkillReplayHarness::new(&tools, Some(&target));
        let mut candidate = candidate();
        candidate.procedure_claims[0].failure_kind = "permission_denied".into();
        let compare_case = SkillReplayCase {
            kind: SkillReplayCaseKind::WithWithoutComparison,
            criterion: "compare".into(),
            required_tool: None,
            tool_args: None,
            ..case(None)
        };

        let result = harness
            .run_replay_case(&candidate, &compare_case)
            .await
            .unwrap();

        assert_eq!(result.status, SkillReplayEvalStatus::Failed);
        assert!(result
            .evidence
            .as_deref()
            .is_some_and(|value| value.contains("not backed by resolved repair provenance")));
    }

    #[tokio::test]
    async fn run_and_store_replay_records_skill_use_trace() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        memory.store_skill(target_skill()).await.unwrap();
        let stored_target = memory
            .list_skills(&"test".to_string(), 10)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("stored target skill");
        let mut patch = candidate();
        patch.target_skill_id = stored_target.id.clone();
        let entry = skill_patch_candidate_service::skill_patch_candidate_to_memory_entry(
            &patch,
            chrono::Utc::now(),
        )
        .unwrap();
        memory.store_episode(entry).await.unwrap();
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(ProbeTool)];

        let report = run_and_store_skill_patch_replay(&memory, "test", "patch-a", &tools, 10)
            .await
            .unwrap();

        assert_eq!(
            report.promotion_report.reason,
            "missing_executable_replay_result"
        );
        let entries = memory
            .list(Some(&skill_use_trace_memory_category()), None, 10)
            .await
            .unwrap();
        let trace = entries
            .iter()
            .find_map(parse_skill_use_trace_entry)
            .expect("skill use trace should be stored");
        assert_eq!(trace.skill_id, stored_target.id);
        assert_eq!(trace.outcome, SkillUseOutcome::Failed);
        assert!(trace
            .verification
            .as_deref()
            .is_some_and(|value| value.contains("missing_executable_replay_result")));
    }
}
