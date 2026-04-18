use crate::skills::{
    load_file_backed_runtime_skills, loaded_skill_to_runtime_candidate,
    memory_skill_is_file_backed_index,
};
use crate::tools::traits::{Tool, ToolResult};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use synapse_domain::application::services::skill_governance_service::{
    resolve_skill_states, SkillLoadRequest, SkillPromptBudget, SkillResolutionReport,
    SkillRuntimeCandidate, SkillRuntimeDecision, SkillRuntimeState,
};
use synapse_domain::application::services::skill_trace_service::{
    build_skill_activation_trace, skill_activation_trace_to_memory_entry,
};
use synapse_domain::config::schema::Config;
use synapse_domain::ports::tool::{
    ToolArgumentPolicy, ToolContract, ToolNonReplayableReason, ToolRuntimeRole,
};
use synapse_memory::UnifiedMemoryPort;

const DEFAULT_MAX_SKILL_BODY_CHARS: usize = 8_000;
const MIN_SKILL_BODY_CHARS: usize = 500;
const MAX_SKILL_BODY_CHARS: usize = 20_000;
const MAX_MEMORY_SKILL_SCAN: usize = 256;

/// Runtime tool for progressive skill disclosure.
///
/// The provider sees a compact skill catalog first. This tool is the controlled
/// path for loading a full skill body when a catalog entry becomes relevant.
pub struct SkillReadTool {
    config: Arc<Config>,
    memory: Arc<dyn UnifiedMemoryPort>,
    available_tools: Vec<String>,
    available_tool_roles: Vec<String>,
    activated_skill_ids: Arc<Mutex<HashSet<String>>>,
}

impl SkillReadTool {
    pub fn new(
        config: Arc<Config>,
        memory: Arc<dyn UnifiedMemoryPort>,
        available_tools: Vec<String>,
        available_tool_roles: Vec<String>,
        activated_skill_ids: Arc<Mutex<HashSet<String>>>,
    ) -> Self {
        Self {
            config,
            memory,
            available_tools,
            available_tool_roles,
            activated_skill_ids,
        }
    }

    async fn activate(&self, args: serde_json::Value) -> Result<SkillActivationOutput> {
        let skill_ref = args
            .get("skill")
            .or_else(|| args.get("id"))
            .or_else(|| args.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("missing required 'skill' parameter"))?;
        let force = args
            .get("force")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let max_chars = args
            .get("max_chars")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(DEFAULT_MAX_SKILL_BODY_CHARS)
            .clamp(MIN_SKILL_BODY_CHARS, MAX_SKILL_BODY_CHARS);

        let entries = self.load_skill_entries().await?;
        if entries.is_empty() {
            bail!("no skills are available to read");
        }

        let matched_ids = entries
            .iter()
            .filter(|entry| entry.matches_ref(skill_ref))
            .map(|entry| entry.candidate.activation_id())
            .collect::<HashSet<_>>();
        if matched_ids.is_empty() {
            bail!("skill not found in governed registry: {skill_ref}");
        }

        let already_activated_skill_ids = if force {
            Vec::new()
        } else {
            self.activated_skill_ids
                .lock()
                .map_err(|_| anyhow!("activated skill set is poisoned"))?
                .iter()
                .cloned()
                .collect()
        };
        let request = SkillLoadRequest {
            agent_id: crate::agent::resolve_agent_id(&self.config),
            platform: Some(std::env::consts::OS.to_string()),
            explicit_skill: Some(skill_ref.to_string()),
            available_tools: self.available_tools.clone(),
            available_tool_roles: self.available_tool_roles.clone(),
            already_activated_skill_ids,
            prompt_budget: SkillPromptBudget {
                max_catalog_entries: entries.len().max(1),
                max_preloaded_skills: 1,
                max_skill_chars: max_chars,
            },
            ..SkillLoadRequest::default()
        };
        let report = resolve_skill_states(
            &request,
            entries
                .iter()
                .map(|entry| entry.candidate.clone())
                .collect(),
        );
        let decision = select_matching_decision(&report.decisions, &matched_ids)
            .ok_or_else(|| anyhow!("skill disappeared from governance report: {skill_ref}"))?;

        if decision.state != SkillRuntimeState::Active {
            self.record_activation_trace(
                &report,
                None,
                &format!("blocked:{}", decision.reason_code),
            )
            .await;
            bail!(
                "skill '{}' is not loadable: state={} reason={}",
                decision.name,
                decision.state,
                decision.reason_code
            );
        }

        if decision.reason_code == "already_loaded" && !force {
            self.record_activation_trace(&report, None, "already_loaded")
                .await;
            return Ok(SkillActivationOutput {
                id: decision.id.clone(),
                name: decision.name.clone(),
                source: decision.source.to_string(),
                reason_code: decision.reason_code.clone(),
                source_ref: decision.source_ref.clone(),
                already_loaded: true,
                truncated: false,
                body: String::new(),
            });
        }

        let entry = entries
            .iter()
            .find(|entry| {
                entry.candidate.activation_id() == decision.id && !entry.indexed_file_backed
            })
            .or_else(|| {
                entries.iter().find(|entry| {
                    !entry.indexed_file_backed
                        && normalize(&entry.candidate.name) == normalize(&decision.name)
                })
            })
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry.candidate.activation_id() == decision.id)
            })
            .ok_or_else(|| anyhow!("skill body missing for {}", decision.id))?;
        if entry.indexed_file_backed {
            bail!(
                "skill '{}' is an indexed file-backed package, but its source body is unavailable",
                decision.name
            );
        }
        let raw_body = entry.body.trim();
        if raw_body.is_empty() {
            bail!("skill '{}' has no readable body", decision.name);
        }
        let (body, truncated) = truncate_chars(raw_body, max_chars);
        self.activated_skill_ids
            .lock()
            .map_err(|_| anyhow!("activated skill set is poisoned"))?
            .insert(decision.id.clone());
        self.record_activation_trace(&report, Some(&decision.id), "loaded")
            .await;

        Ok(SkillActivationOutput {
            id: decision.id.clone(),
            name: decision.name.clone(),
            source: decision.source.to_string(),
            reason_code: decision.reason_code.clone(),
            source_ref: decision.source_ref.clone(),
            already_loaded: false,
            truncated,
            body,
        })
    }

    async fn load_skill_entries(&self) -> Result<Vec<SkillBodyEntry>> {
        let mut entries = Vec::new();
        for skill in load_file_backed_runtime_skills(&self.config.workspace_dir, &self.config) {
            let body = skill
                .prompts
                .iter()
                .map(|prompt| prompt.trim())
                .filter(|prompt| !prompt.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            entries.push(SkillBodyEntry {
                source_ref: loaded_skill_to_runtime_candidate(&skill, &self.config.workspace_dir)
                    .source_ref,
                candidate: loaded_skill_to_runtime_candidate(&skill, &self.config.workspace_dir),
                body,
                indexed_file_backed: false,
            });
        }

        let agent_id = crate::agent::resolve_agent_id(&self.config);
        let learned = self
            .memory
            .list_skills(&agent_id, MAX_MEMORY_SKILL_SCAN)
            .await
            .with_context(|| format!("failed to list learned skills for agent {agent_id}"))?;
        for skill in learned {
            let indexed_file_backed = memory_skill_is_file_backed_index(&skill);
            entries.push(SkillBodyEntry {
                source_ref: None,
                candidate: SkillRuntimeCandidate::from_memory_skill(&skill),
                body: skill.content,
                indexed_file_backed,
            });
        }

        Ok(entries)
    }

    async fn record_activation_trace(
        &self,
        report: &SkillResolutionReport,
        loaded_skill_id: Option<&str>,
        outcome: &str,
    ) {
        let agent_id = crate::agent::resolve_agent_id(&self.config);
        let trace = build_skill_activation_trace(
            report,
            loaded_skill_id
                .map(|id| vec![id.to_string()])
                .unwrap_or_default(),
            None,
            Some(outcome.to_string()),
        );
        let observed_at = chrono::Utc::now();
        match skill_activation_trace_to_memory_entry(&agent_id, &trace, observed_at, None) {
            Ok(entry) => {
                if let Err(error) = self.memory.store_episode(entry).await {
                    tracing::warn!(%error, "skill_read activation trace write failed");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "skill_read activation trace serialization failed");
            }
        }
    }
}

#[async_trait]
impl Tool for SkillReadTool {
    fn name(&self) -> &str {
        "skill_read"
    }

    fn description(&self) -> &str {
        "Load full instructions for one governed skill by id, name, or catalog location. Use this after the compact skill catalog indicates a relevant skill. Repeated activation is deduped unless force=true."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Skill id, name, or catalog location from Available Skills."
                },
                "force": {
                    "type": "boolean",
                    "description": "Return the body even if this skill was already activated in the current runtime. Default false."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum body characters to return, clamped between 500 and 20000. Default 8000."
                }
            },
            "required": ["skill"]
        })
    }

    fn runtime_role(&self) -> Option<synapse_domain::ports::tool::ToolRuntimeRole> {
        Some(ToolRuntimeRole::RuntimeStateInspection)
    }

    fn tool_contract(&self) -> ToolContract {
        ToolContract::non_replayable(
            self.runtime_role(),
            ToolNonReplayableReason::RuntimeActivation,
        )
        .with_arguments(vec![
            ToolArgumentPolicy::blocked("skill"),
            ToolArgumentPolicy::blocked("force"),
            ToolArgumentPolicy::blocked("max_chars"),
        ])
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        match self.activate(args).await {
            Ok(output) => Ok(ToolResult {
                success: true,
                output: output.render(),
                error: None,
            }),
            Err(error) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(error.to_string()),
            }),
        }
    }
}

struct SkillBodyEntry {
    candidate: SkillRuntimeCandidate,
    source_ref: Option<String>,
    body: String,
    indexed_file_backed: bool,
}

impl SkillBodyEntry {
    fn matches_ref(&self, skill_ref: &str) -> bool {
        let needle = normalize(skill_ref);
        if needle.is_empty() {
            return false;
        }
        let activation_id = normalize(&self.candidate.activation_id());
        if activation_id == needle || normalize(&self.candidate.name) == needle {
            return true;
        }
        self.source_ref.as_ref().is_some_and(|source_ref| {
            let normalized_ref = normalize(source_ref);
            normalized_ref == needle || normalized_ref.ends_with(&needle)
        })
    }
}

struct SkillActivationOutput {
    id: String,
    name: String,
    source: String,
    reason_code: String,
    source_ref: Option<String>,
    already_loaded: bool,
    truncated: bool,
    body: String,
}

impl SkillActivationOutput {
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("<activated_skill\n");
        write_xml_attr_line(&mut out, "id", &self.id);
        write_xml_attr_line(&mut out, "name", &self.name);
        write_xml_attr_line(&mut out, "source", &self.source);
        write_xml_attr_line(&mut out, "reason", &self.reason_code);
        write_xml_attr_line(&mut out, "already_loaded", bool_str(self.already_loaded));
        write_xml_attr_line(&mut out, "truncated", bool_str(self.truncated));
        if let Some(source_ref) = &self.source_ref {
            write_xml_attr_line(&mut out, "source_ref", source_ref);
        }
        out.push_str(">\n");
        if self.already_loaded {
            out.push_str("  <note>This skill is already activated in the current runtime; reuse the earlier instructions.</note>\n");
        } else {
            out.push_str("  <instructions>");
            append_xml_escaped(&mut out, &self.body);
            out.push_str("</instructions>\n");
        }
        out.push_str("</activated_skill>");
        out
    }
}

fn select_matching_decision<'a>(
    decisions: &'a [SkillRuntimeDecision],
    matched_ids: &HashSet<String>,
) -> Option<&'a SkillRuntimeDecision> {
    decisions
        .iter()
        .filter(|decision| matched_ids.contains(&decision.id))
        .find(|decision| decision.state == SkillRuntimeState::Active)
        .or_else(|| {
            decisions
                .iter()
                .find(|decision| matched_ids.contains(&decision.id))
        })
}

fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return (value.to_string(), false);
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    out.push_str("\n[truncated]");
    (out, true)
}

fn write_xml_attr_line(out: &mut String, name: &str, value: &str) {
    out.push_str("  ");
    out.push_str(name);
    out.push_str("=\"");
    append_xml_escaped(out, value);
    out.push_str("\"\n");
}

fn append_xml_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

fn bool_str(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        create_user_authored_skill, port_workspace_skill_packages_to_memory,
        skill_health_report_for_agent, sync_file_backed_skill_index_to_memory,
        UserAuthoredSkillCreateRequest,
    };
    use chrono::Utc;
    use std::sync::OnceLock;
    use synapse_domain::application::services::skill_health_service::SkillHealthSignal;
    use synapse_domain::application::services::skill_trace_service::{
        parse_skill_activation_trace_entry, skill_activation_trace_memory_category,
    };
    use synapse_domain::config::schema::{Config, MemoryConfig, SkillsConfig};
    use synapse_domain::domain::memory::{Skill, SkillOrigin, SkillStatus};
    use synapse_memory::SkillMemoryPort;

    fn open_skills_env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn test_config(workspace_dir: std::path::PathBuf) -> Config {
        let mut config = Config {
            workspace_dir,
            memory: MemoryConfig {
                backend: "none".into(),
                ..MemoryConfig::default()
            },
            skills: SkillsConfig::default(),
            ..Config::default()
        };
        config.agents_ipc.agent_id = Some("agent".into());
        config
    }

    fn learned_skill(name: &str, status: SkillStatus) -> Skill {
        Skill {
            id: format!("skill:{name}"),
            name: name.to_string(),
            description: format!("{name} learned procedure"),
            content:
                "Use the Matrix release_status recipe and preserve the local deployment notes."
                    .into(),
            task_family: Some("matrix-upgrade".into()),
            tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
            lineage_task_families: vec!["release-audit".into()],
            tags: vec!["matrix".into(), "ops".into()],
            success_count: 2,
            fail_count: 0,
            version: 1,
            origin: SkillOrigin::Learned,
            status,
            created_by: "agent".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    async fn memory_with_skill(
        workspace_dir: &std::path::Path,
        skill: Skill,
    ) -> Arc<synapse_memory::SurrealMemoryAdapter> {
        let memory_path = workspace_dir.join("skill-memory.surreal");
        let memory = Arc::new(
            synapse_memory::SurrealMemoryAdapter::new(
                &memory_path.to_string_lossy(),
                Arc::new(synapse_memory::embeddings::NoopEmbedding),
                "agent".into(),
            )
            .await
            .unwrap(),
        );
        memory.store_skill(skill).await.unwrap();
        memory
    }

    #[tokio::test]
    async fn ports_package_skill_to_memory_then_reads_once_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("release-audit");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: release-audit\ndescription: Release audit workflow\n---\n# Release Audit\nCheck tests and changelog.\n",
        )
        .unwrap();

        let config = test_config(dir.path().to_path_buf());
        assert!(config.skills.port_workspace_packages_on_start);
        let agent_id = crate::agent::resolve_agent_id(&config);
        let memory_path = dir.path().join("ported-skill-memory.surreal");
        let memory = Arc::new(
            synapse_memory::SurrealMemoryAdapter::new(
                &memory_path.to_string_lossy(),
                Arc::new(synapse_memory::embeddings::NoopEmbedding),
                agent_id.clone(),
            )
            .await
            .unwrap(),
        );
        let report =
            port_workspace_skill_packages_to_memory(memory.as_ref(), &agent_id, dir.path())
                .await
                .unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.moved, 1);
        assert!(!skill_dir.exists());
        assert!(dir
            .path()
            .join("skills")
            .join("ported")
            .join("release-audit")
            .join("SKILL.md")
            .exists());

        let config = Arc::new(config);
        let tool = SkillReadTool::new(
            config,
            memory,
            vec!["skill_read".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let first = tool
            .execute(json!({ "skill": "release-audit" }))
            .await
            .unwrap();
        assert!(first.success, "{:?}", first.error);
        assert!(first.output.contains("Check tests and changelog."));
        assert!(first.output.contains("already_loaded=\"false\""));

        let second = tool
            .execute(json!({ "skill": "release-audit" }))
            .await
            .unwrap();
        assert!(second.success, "{:?}", second.error);
        assert!(second.output.contains("already_loaded=\"true\""));
        assert!(!second.output.contains("Check tests and changelog."));
    }

    #[tokio::test]
    async fn reads_imported_file_backed_skill_when_workspace_porting_is_enabled() {
        let _env_lock = open_skills_env_lock().lock().unwrap();
        let enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        std::fs::create_dir_all(workspace_dir.join("skills")).unwrap();
        let open_skills_dir = dir.path().join("open-skills-local");
        std::fs::create_dir_all(open_skills_dir.join("skills/pdf")).unwrap();
        std::fs::write(
            open_skills_dir.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: Imported PDF workflow.\n---\n# PDF\nInspect PDF documents safely.\n",
        )
        .unwrap();

        let mut config = test_config(workspace_dir);
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());
        assert!(config.skills.port_workspace_packages_on_start);
        let config = Arc::new(config);
        let tool = SkillReadTool::new(
            config,
            Arc::new(synapse_memory::NoopUnifiedMemory),
            vec!["skill_read".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool.execute(json!({ "skill": "pdf" })).await.unwrap();

        drop(dir_guard);
        drop(enabled_guard);

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("source=\"imported\""));
        assert!(result.output.contains("Inspect PDF documents safely."));
    }

    #[tokio::test]
    async fn indexed_imported_skill_uses_file_body_instead_of_index_card() {
        let _env_lock = open_skills_env_lock().lock().unwrap();
        let enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        std::fs::create_dir_all(workspace_dir.join("skills")).unwrap();
        let open_skills_dir = dir.path().join("open-skills-local");
        std::fs::create_dir_all(open_skills_dir.join("skills/pdf")).unwrap();
        std::fs::write(
            open_skills_dir.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: Imported PDF workflow.\n---\n# PDF\nREAL FILE BODY.\n",
        )
        .unwrap();

        let mut config = test_config(workspace_dir.clone());
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());
        assert!(config.skills.port_workspace_packages_on_start);
        let memory_path = dir.path().join("indexed-skill-memory.surreal");
        let memory = Arc::new(
            synapse_memory::SurrealMemoryAdapter::new(
                &memory_path.to_string_lossy(),
                Arc::new(synapse_memory::embeddings::NoopEmbedding),
                "agent".into(),
            )
            .await
            .unwrap(),
        );
        let report = sync_file_backed_skill_index_to_memory(
            memory.as_ref(),
            "agent",
            &workspace_dir,
            &config,
        )
        .await
        .unwrap();
        assert_eq!(report.indexed, 1);

        let config = Arc::new(config);
        let tool = SkillReadTool::new(
            config,
            memory,
            vec!["skill_read".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool.execute(json!({ "skill": "pdf" })).await.unwrap();

        drop(dir_guard);
        drop(enabled_guard);

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("REAL FILE BODY."));
        assert!(!result.output.contains("[package-skill-index]"));
    }

    #[tokio::test]
    async fn unknown_skill_returns_tool_error_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(test_config(dir.path().to_path_buf()));
        let tool = SkillReadTool::new(
            config,
            Arc::new(synapse_memory::NoopUnifiedMemory),
            vec!["skill_read".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool.execute(json!({ "skill": "missing" })).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("no skills are available"));
    }

    #[tokio::test]
    async fn reads_active_learned_skill_from_memory() {
        let dir = tempfile::tempdir().unwrap();
        let memory = memory_with_skill(
            dir.path(),
            learned_skill("matrix-upgrade", SkillStatus::Active),
        )
        .await;
        let config = Arc::new(test_config(dir.path().to_path_buf()));
        let tool = SkillReadTool::new(
            config,
            memory,
            vec!["skill_read".into(), "git_operations".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool
            .execute(json!({ "skill": "matrix-upgrade" }))
            .await
            .unwrap();

        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("source=\"learned\""));
        assert!(result.output.contains("Matrix release_status recipe"));
    }

    #[tokio::test]
    async fn user_authored_skill_is_loaded_on_demand_and_counted_as_utility() {
        let dir = tempfile::tempdir().unwrap();
        let memory_path = dir.path().join("user-skill-memory.surreal");
        let memory = Arc::new(
            synapse_memory::SurrealMemoryAdapter::new(
                &memory_path.to_string_lossy(),
                Arc::new(synapse_memory::embeddings::NoopEmbedding),
                "agent".into(),
            )
            .await
            .unwrap(),
        );
        let created = create_user_authored_skill(
            memory.as_ref(),
            "agent",
            UserAuthoredSkillCreateRequest {
                name: "Matrix release audit".into(),
                description: Some("Check local Matrix checkout against upstream tags.".into()),
                body:
                    "# Matrix release audit\n\nFind the checkout, read remotes, and compare tags."
                        .into(),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                tags: vec!["matrix".into()],
                status: SkillStatus::Active,
            },
        )
        .await
        .unwrap();
        let config = Arc::new(test_config(dir.path().join("workspace")));
        let tool = SkillReadTool::new(
            config,
            memory.clone(),
            vec!["skill_read".into(), "repo_discovery".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool
            .execute(json!({ "skill": "Matrix release audit" }))
            .await
            .unwrap();
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("source=\"manual\""));
        assert!(result.output.contains("compare tags"));

        let health = skill_health_report_for_agent(memory.as_ref(), "agent", 10, 10)
            .await
            .unwrap();
        let item = health
            .items
            .iter()
            .find(|item| item.skill_id == created.skill_id)
            .expect("created skill should be in health report");
        assert_eq!(item.utility.selected_count, 1);
        assert_eq!(item.utility.read_count, 1);
        assert!(!item.signals.contains(&SkillHealthSignal::UnusedActiveSkill));
    }

    #[tokio::test]
    async fn refuses_candidate_learned_skill_body() {
        let dir = tempfile::tempdir().unwrap();
        let memory = memory_with_skill(
            dir.path(),
            learned_skill("matrix-candidate", SkillStatus::Candidate),
        )
        .await;
        let config = Arc::new(test_config(dir.path().to_path_buf()));
        let tool = SkillReadTool::new(
            config,
            memory,
            vec!["skill_read".into(), "git_operations".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool
            .execute(json!({ "skill": "matrix-candidate" }))
            .await
            .unwrap();

        assert!(!result.success);
        let error = result.error.unwrap();
        assert!(error.contains("state=candidate"), "{error}");
        assert!(error.contains("operator_review_required"), "{error}");
    }

    #[tokio::test]
    async fn records_activation_trace_without_skill_body() {
        let dir = tempfile::tempdir().unwrap();
        let memory = memory_with_skill(
            dir.path(),
            learned_skill("matrix-upgrade", SkillStatus::Active),
        )
        .await;
        let config = Arc::new(test_config(dir.path().to_path_buf()));
        let tool = SkillReadTool::new(
            config,
            memory.clone(),
            vec!["skill_read".into(), "git_operations".into()],
            Vec::new(),
            Arc::new(Mutex::new(HashSet::new())),
        );

        let result = tool
            .execute(json!({ "skill": "matrix-upgrade" }))
            .await
            .unwrap();
        assert!(result.success, "{:?}", result.error);

        let entries = memory
            .list(Some(&skill_activation_trace_memory_category()), None, 10)
            .await
            .unwrap();
        let trace = entries
            .iter()
            .find_map(parse_skill_activation_trace_entry)
            .expect("activation trace should be stored");

        assert_eq!(trace.loaded_skill_ids.len(), 1);
        assert_eq!(trace.outcome.as_deref(), Some("loaded"));
        assert!(!entries[0].content.contains("Matrix release_status recipe"));
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn unset(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
