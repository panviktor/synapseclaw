use anyhow::{bail, Context, Result};
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use synapse_domain::application::services::inbound_message_service::{
    RuntimeSkillStatusView, RuntimeUserSkillCreateMetadata,
};
use synapse_domain::application::services::skill_candidate_eval_service::{
    evaluate_skill_patch_for_apply, evaluate_skill_patch_for_auto_promotion,
    SkillCandidateEvalPolicy, SkillPatchApplyReport, SkillPatchAutoPromotionPolicy,
    SkillPatchAutoPromotionReport, SkillReplayHarnessReport,
};
use synapse_domain::application::services::skill_governance_service::{
    resolve_skill_states, SkillActivationTrace, SkillLoadRequest, SkillPatchApplyRecord,
    SkillPatchCandidate, SkillPatchProcedureClaim, SkillPatchRollbackRecord, SkillPromptBudget,
    SkillReplayEvalStatus, SkillResolutionReport, SkillRuntimeCandidate, SkillRuntimeState,
    SkillSource, SkillTrustLevel, SkillUseOutcome, SkillUseTrace,
};
use synapse_domain::application::services::skill_health_service::{
    build_skill_health_report, skill_health_cleanup_decisions, SkillHealthCleanupDecision,
    SkillHealthItem, SkillHealthReport, SkillHealthSeverity,
};
use synapse_domain::application::services::skill_patch_candidate_service;
use synapse_domain::application::services::skill_review_service::{
    review_learned_skills, SkillReviewAction, SkillReviewDecision,
};
use synapse_domain::application::services::skill_trace_service::{
    parse_skill_activation_trace_entry, parse_skill_use_trace_entry,
    skill_activation_trace_memory_category, skill_activation_trace_memory_key_prefix,
    skill_use_trace_memory_category, skill_use_trace_memory_key_prefix,
};
use synapse_domain::application::services::skill_user_authoring_service::{
    build_user_authored_skill, UserAuthoredSkillInput, UserAuthoredSkillPolicy,
    UserAuthoredSkillValidationReport,
};
use synapse_domain::domain::memory::{Skill as MemorySkill, SkillOrigin, SkillStatus, SkillUpdate};

mod audit;
pub(crate) mod replay;

const OPEN_SKILLS_REPO_URL: &str = "https://github.com/besoeasy/open-skills";
const OPEN_SKILLS_SYNC_MARKER: &str = ".synapseclaw-open-skills-sync";
const OPEN_SKILLS_SYNC_INTERVAL_SECS: u64 = 60 * 60 * 24 * 7;
const PORTED_SKILLS_DIR_NAME: &str = "ported";
const FILE_BACKED_SKILL_INDEX_TAG: &str = "file-backed-skill-index";
const FILE_BACKED_SKILL_SOURCE_REF_TAG_PREFIX: &str = "source-ref:";
const FILE_BACKED_SKILL_CONTENT_HASH_TAG_PREFIX: &str = "content-hash:";
const FILE_BACKED_SKILL_INDEX_EXCERPT_CHARS: usize = 1_200;
const FILE_BACKED_SKILL_INDEX_SCAN_LIMIT: usize = 1_000;

/// A skill is a user-defined or community-built capability.
/// Skills live in `~/.synapseclaw/workspace/skills/<name>/SKILL.md`
/// and can include tool definitions, prompts, and automation scripts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub tools: Vec<SkillTool>,
    #[serde(default)]
    pub prompts: Vec<String>,
    #[serde(skip)]
    pub location: Option<PathBuf>,
}

pub fn infer_loaded_skill_origin(skill: &Skill) -> &'static str {
    if skill.tags.iter().any(|tag| tag == "open-skills")
        || skill.author.as_deref() == Some("besoeasy/open-skills")
    {
        "imported"
    } else {
        "manual"
    }
}

pub fn format_loaded_skill_projection(skill: &Skill, workspace_dir: &Path) -> String {
    let mut lines = vec![
        "[skill]".to_string(),
        format!("- name: {}", skill.name),
        format!("- origin: {}", infer_loaded_skill_origin(skill)),
        "- status: active".to_string(),
        format!("- version: {}", skill.version),
        format!(
            "- location: {}",
            render_skill_location(skill, workspace_dir, true)
        ),
    ];
    if let Some(author) = skill
        .author
        .as_deref()
        .filter(|author| !author.trim().is_empty())
    {
        lines.push(format!("- author: {author}"));
    }
    if !skill.description.trim().is_empty() {
        lines.push("- description:".to_string());
        lines.push(indent_multiline(skill.description.trim(), 2));
    }
    if !skill.prompts.is_empty() {
        lines.push(format!("- instruction_count: {}", skill.prompts.len()));
    }
    if !skill.tools.is_empty() {
        lines.push(format!("- tool_count: {}", skill.tools.len()));
    }
    format!("{}\n", lines.join("\n"))
}

/// A tool defined by a skill (shell command, HTTP call, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTool {
    pub name: String,
    pub description: String,
    /// "shell", "http", "script"
    pub kind: String,
    /// The command/URL/script to execute
    pub command: String,
    #[serde(default)]
    pub args: HashMap<String, String>,
}

/// Skill manifest parsed from SKILL.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillManifest {
    skill: SkillMeta,
    #[serde(default)]
    tools: Vec<SkillTool>,
    #[serde(default)]
    prompts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillMeta {
    name: String,
    description: String,
    #[serde(default = "default_version")]
    version: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SkillMarkdownMeta {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

fn indent_multiline(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Load all skills from the workspace skills directory
pub fn load_skills(workspace_dir: &Path) -> Vec<Skill> {
    load_skills_with_open_skills_config(workspace_dir, None, None)
}

/// Load skills using runtime config values (preferred at runtime).
pub fn load_skills_with_config(
    workspace_dir: &Path,
    config: &synapse_domain::config::schema::Config,
) -> Vec<Skill> {
    load_skills_with_open_skills_config(
        workspace_dir,
        Some(config.skills.open_skills_enabled),
        config.skills.open_skills_dir.as_deref(),
    )
}

/// Load file-backed skills that should remain active at runtime.
///
/// When workspace package porting is enabled, operator-authored workspace
/// `SKILL.*` files are imported into memory and moved under `skills/ported/`.
/// Imported/community file-backed skills remain file-backed, so they must stay
/// visible to the compact catalog and `skill_read`.
pub(crate) fn load_file_backed_runtime_skills(
    workspace_dir: &Path,
    config: &synapse_domain::config::schema::Config,
) -> Vec<Skill> {
    let skills = load_skills_with_config(workspace_dir, config);
    if !config.skills.port_workspace_packages_on_start {
        return skills;
    }

    skills
        .into_iter()
        .filter(|skill| infer_loaded_skill_origin(skill) != "manual")
        .collect()
}

fn load_skills_with_open_skills_config(
    workspace_dir: &Path,
    config_open_skills_enabled: Option<bool>,
    config_open_skills_dir: Option<&str>,
) -> Vec<Skill> {
    let mut skills = Vec::new();

    if let Some(open_skills_dir) =
        ensure_open_skills_repo(config_open_skills_enabled, config_open_skills_dir)
    {
        skills.extend(load_open_skills(&open_skills_dir));
    }

    skills.extend(load_workspace_skills(workspace_dir));
    skills
}

fn load_workspace_skills(workspace_dir: &Path) -> Vec<Skill> {
    let skills_dir = workspace_dir.join("skills");
    load_skills_from_directory(&skills_dir)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SkillPackagePortReport {
    pub scanned: usize,
    pub imported: usize,
    pub skipped_existing: usize,
    pub moved: usize,
    pub failed: usize,
}

pub async fn port_workspace_skill_packages_to_memory(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    workspace_dir: &Path,
) -> Result<SkillPackagePortReport> {
    let skills_dir = workspace_dir.join("skills");
    if !skills_dir.is_dir() {
        return Ok(SkillPackagePortReport::default());
    }

    let mut report = SkillPackagePortReport::default();
    let sources = discover_workspace_skill_sources(&skills_dir)
        .with_context(|| format!("failed to scan {}", skills_dir.display()))?;
    let ported_dir = skills_dir.join(PORTED_SKILLS_DIR_NAME);
    std::fs::create_dir_all(&ported_dir)
        .with_context(|| format!("failed to create {}", ported_dir.display()))?;

    for source in sources {
        let Some(skill) = load_workspace_skill_source(&source) else {
            continue;
        };
        report.scanned += 1;

        match memory.get_skill(&skill.name, &agent_id.to_string()).await {
            Ok(Some(_)) => {
                report.skipped_existing += 1;
            }
            Ok(None) => {
                let memory_skill = loaded_skill_to_memory_skill(&skill, agent_id, workspace_dir);
                memory.store_skill(memory_skill).await.with_context(|| {
                    format!("failed to import skill package {}", source.path().display())
                })?;
                report.imported += 1;
            }
            Err(error) => {
                report.failed += 1;
                tracing::warn!(
                    %error,
                    skill = skill.name.as_str(),
                    "failed to check existing skill before package port"
                );
                continue;
            }
        }

        match move_ported_skill_source(&source, &ported_dir) {
            Ok(()) => report.moved += 1,
            Err(error) => {
                report.failed += 1;
                tracing::warn!(
                    %error,
                    path = %source.path().display(),
                    "failed to move ported skill package"
                );
            }
        }
    }

    Ok(report)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FileBackedSkillIndexReport {
    pub scanned: usize,
    pub indexed: usize,
    pub updated: usize,
    pub skipped_existing: usize,
    pub deprecated_stale: usize,
    pub failed: usize,
}

pub async fn sync_file_backed_skill_index_to_memory(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    workspace_dir: &Path,
    config: &synapse_domain::config::schema::Config,
) -> Result<FileBackedSkillIndexReport> {
    let mut report = FileBackedSkillIndexReport::default();
    let file_backed_skills = load_file_backed_runtime_skills(workspace_dir, config)
        .into_iter()
        .filter(|skill| infer_loaded_skill_origin(skill) != "manual")
        .collect::<Vec<_>>();
    let current_names = file_backed_skills
        .iter()
        .map(|skill| normalized_skill_name(&skill.name))
        .collect::<HashSet<_>>();

    for skill in file_backed_skills {
        report.scanned += 1;
        let memory_skill = file_backed_skill_index_memory_skill(&skill, agent_id, workspace_dir);
        let content_hash = file_backed_skill_content_hash(&skill, workspace_dir);
        match memory.get_skill(&skill.name, &agent_id.to_string()).await {
            Ok(Some(existing)) if is_file_backed_skill_index(&existing) => {
                if memory_skill_tag_value(&existing.tags, FILE_BACKED_SKILL_CONTENT_HASH_TAG_PREFIX)
                    .is_some_and(|existing_hash| existing_hash == content_hash)
                    && existing.status == SkillStatus::Active
                {
                    report.skipped_existing += 1;
                    continue;
                }
                memory
                    .update_skill(
                        &existing.id,
                        SkillUpdate {
                            increment_success: false,
                            increment_fail: false,
                            new_description: Some(memory_skill.description.clone()),
                            new_content: Some(memory_skill.content.clone()),
                            new_task_family: Some(memory_skill.task_family.clone()),
                            new_tool_pattern: Some(memory_skill.tool_pattern.clone()),
                            new_lineage_task_families: Some(
                                memory_skill.lineage_task_families.clone(),
                            ),
                            new_tags: Some(memory_skill.tags.clone()),
                            new_status: Some(SkillStatus::Active),
                        },
                        &agent_id.to_string(),
                    )
                    .await
                    .with_context(|| format!("failed to update indexed skill {}", skill.name))?;
                report.updated += 1;
            }
            Ok(Some(_)) => {
                report.skipped_existing += 1;
            }
            Ok(None) => {
                memory
                    .store_skill(memory_skill)
                    .await
                    .with_context(|| format!("failed to index file-backed skill {}", skill.name))?;
                report.indexed += 1;
            }
            Err(error) => {
                report.failed += 1;
                tracing::warn!(
                    %error,
                    skill = skill.name.as_str(),
                    "failed to inspect existing indexed skill"
                );
            }
        }
    }

    let existing = memory
        .list_skills(&agent_id.to_string(), FILE_BACKED_SKILL_INDEX_SCAN_LIMIT)
        .await
        .unwrap_or_default();
    for skill in existing {
        if !is_file_backed_skill_index(&skill) {
            continue;
        }
        if current_names.contains(&normalized_skill_name(&skill.name)) {
            continue;
        }
        match memory
            .update_skill(
                &skill.id,
                SkillUpdate {
                    increment_success: false,
                    increment_fail: false,
                    new_description: None,
                    new_content: None,
                    new_task_family: None,
                    new_tool_pattern: None,
                    new_lineage_task_families: None,
                    new_tags: None,
                    new_status: Some(SkillStatus::Deprecated),
                },
                &agent_id.to_string(),
            )
            .await
        {
            Ok(()) => report.deprecated_stale += 1,
            Err(error) => {
                report.failed += 1;
                tracing::warn!(
                    %error,
                    skill = skill.name.as_str(),
                    "failed to deprecate stale file-backed skill index"
                );
            }
        }
    }

    Ok(report)
}

#[derive(Debug, Clone)]
enum WorkspaceSkillSource {
    Directory(PathBuf),
    File(PathBuf),
}

impl WorkspaceSkillSource {
    fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::File(path) => path,
        }
    }
}

fn discover_workspace_skill_sources(skills_dir: &Path) -> Result<Vec<WorkspaceSkillSource>> {
    let mut sources = Vec::new();
    collect_workspace_skill_sources(skills_dir, &mut sources)?;
    sources.sort_by(|left, right| left.path().cmp(right.path()));
    Ok(sources)
}

fn collect_workspace_skill_sources(
    dir: &Path,
    sources: &mut Vec<WorkspaceSkillSource>,
) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if is_ported_skill_path(&path) {
            continue;
        }
        if path.is_dir() {
            if path.join("SKILL.toml").is_file() || path.join("SKILL.md").is_file() {
                sources.push(WorkspaceSkillSource::Directory(path));
            } else {
                collect_workspace_skill_sources(&path, sources)?;
            }
            continue;
        }
        if is_loose_workspace_skill_file(&path) {
            sources.push(WorkspaceSkillSource::File(path));
        }
    }
    Ok(())
}

fn is_ported_skill_path(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(PORTED_SKILLS_DIR_NAME)
}

fn is_loose_workspace_skill_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if matches!(file_name, "SKILL.md" | "SKILL.toml") {
        return true;
    }
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md" | "toml")
    )
}

fn load_workspace_skill_source(source: &WorkspaceSkillSource) -> Option<Skill> {
    match source {
        WorkspaceSkillSource::Directory(package_dir) => load_workspace_skill_package(package_dir),
        WorkspaceSkillSource::File(path) => load_workspace_skill_file(path),
    }
}

fn load_workspace_skill_package(package_dir: &Path) -> Option<Skill> {
    match audit::audit_skill_directory(package_dir) {
        Ok(report) if report.is_clean() => {}
        Ok(report) => {
            tracing::warn!(
                "skipping insecure skill directory {}: {}",
                package_dir.display(),
                report.summary()
            );
            return None;
        }
        Err(error) => {
            tracing::warn!(
                "skipping unauditable skill directory {}: {error}",
                package_dir.display()
            );
            return None;
        }
    }

    let manifest_path = package_dir.join("SKILL.toml");
    let md_path = package_dir.join("SKILL.md");
    if manifest_path.exists() {
        load_skill_toml(&manifest_path).ok()
    } else if md_path.exists() {
        load_skill_md(&md_path, package_dir).ok()
    } else {
        None
    }
}

fn load_workspace_skill_file(path: &Path) -> Option<Skill> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if file_name.ends_with(".toml") {
        return match load_skill_toml(path) {
            Ok(skill) => Some(skill),
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "failed to load workspace skill toml"
                );
                None
            }
        };
    }
    if file_name.ends_with(".md") {
        let default_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.eq_ignore_ascii_case("skill"))
            .or_else(|| {
                path.parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
            })
            .unwrap_or("workspace-skill");
        return match load_skill_md_with_default_name(path, default_name) {
            Ok(skill) => Some(skill),
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "failed to load workspace skill markdown"
                );
                None
            }
        };
    }
    None
}

fn loaded_skill_to_memory_skill(
    skill: &Skill,
    agent_id: &str,
    workspace_dir: &Path,
) -> MemorySkill {
    let now = chrono::Utc::now();
    let mut tags = skill.tags.clone();
    for tag in [
        "ported-skill-package".to_string(),
        format!(
            "ported-from:{}",
            render_skill_location(skill, workspace_dir, true)
        ),
    ] {
        if !tags.iter().any(|existing| existing == &tag) {
            tags.push(tag);
        }
    }
    MemorySkill {
        id: String::new(),
        name: skill.name.clone(),
        description: skill.description.clone(),
        content: skill
            .prompts
            .iter()
            .map(|prompt| prompt.trim())
            .filter(|prompt| !prompt.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        task_family: None,
        tool_pattern: skill.tools.iter().map(|tool| tool.name.clone()).collect(),
        lineage_task_families: Vec::new(),
        tags,
        success_count: 0,
        fail_count: 0,
        version: skill.version.parse::<u32>().unwrap_or(1),
        origin: match infer_loaded_skill_origin(skill) {
            "imported" => SkillOrigin::Imported,
            _ => SkillOrigin::Manual,
        },
        status: SkillStatus::Active,
        created_by: agent_id.to_string(),
        created_at: now,
        updated_at: now,
    }
}

fn file_backed_skill_index_memory_skill(
    skill: &Skill,
    agent_id: &str,
    workspace_dir: &Path,
) -> MemorySkill {
    let now = chrono::Utc::now();
    let source_ref = render_skill_location(skill, workspace_dir, true);
    let content_hash = file_backed_skill_content_hash(skill, workspace_dir);
    let mut tags = skill.tags.clone();
    push_unique_tag(&mut tags, FILE_BACKED_SKILL_INDEX_TAG.to_string());
    push_unique_tag(
        &mut tags,
        format!("{FILE_BACKED_SKILL_SOURCE_REF_TAG_PREFIX}{source_ref}"),
    );
    push_unique_tag(
        &mut tags,
        format!("{FILE_BACKED_SKILL_CONTENT_HASH_TAG_PREFIX}{content_hash}"),
    );

    MemorySkill {
        id: format!("file-backed-{}", normalized_skill_name(&skill.name)),
        name: skill.name.clone(),
        description: skill.description.clone(),
        content: render_file_backed_skill_index_content(skill, &source_ref),
        task_family: None,
        tool_pattern: skill.tools.iter().map(|tool| tool.name.clone()).collect(),
        lineage_task_families: Vec::new(),
        tags,
        success_count: 0,
        fail_count: 0,
        version: skill.version.parse::<u32>().unwrap_or(1),
        origin: SkillOrigin::Imported,
        status: SkillStatus::Active,
        created_by: agent_id.to_string(),
        created_at: now,
        updated_at: now,
    }
}

fn render_file_backed_skill_index_content(skill: &Skill, source_ref: &str) -> String {
    let mut lines = vec![
        "[package-skill-index]".to_string(),
        format!("name: {}", skill.name.trim()),
        format!("source_ref: {source_ref}"),
        "activation: use skill_read with this skill name; full instructions remain file-backed."
            .to_string(),
    ];
    if !skill.description.trim().is_empty() {
        lines.push(format!("description: {}", skill.description.trim()));
    }
    if !skill.tags.is_empty() {
        lines.push(format!("tags: {}", skill.tags.join(", ")));
    }
    if !skill.tools.is_empty() {
        lines.push(format!(
            "tools: {}",
            skill
                .tools
                .iter()
                .map(|tool| tool.name.trim())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let excerpt = truncate_for_cli(
        &skill
            .prompts
            .iter()
            .map(|prompt| prompt.trim())
            .filter(|prompt| !prompt.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        FILE_BACKED_SKILL_INDEX_EXCERPT_CHARS,
    );
    if !excerpt.trim().is_empty() {
        lines.push("instruction_excerpt:".to_string());
        lines.push(indent_multiline(&excerpt, 2));
    }
    lines.join("\n")
}

fn file_backed_skill_content_hash(skill: &Skill, workspace_dir: &Path) -> String {
    let payload = serde_json::json!({
        "name": &skill.name,
        "description": &skill.description,
        "version": &skill.version,
        "author": &skill.author,
        "tags": &skill.tags,
        "tools": &skill.tools,
        "prompts": &skill.prompts,
        "location": render_skill_location(skill, workspace_dir, true),
    });
    hex::encode(Sha256::digest(payload.to_string().as_bytes()))
}

fn is_file_backed_skill_index(skill: &MemorySkill) -> bool {
    skill
        .tags
        .iter()
        .any(|tag| tag == FILE_BACKED_SKILL_INDEX_TAG)
}

pub(crate) fn memory_skill_is_file_backed_index(skill: &MemorySkill) -> bool {
    is_file_backed_skill_index(skill)
}

fn memory_skill_tag_value<'a>(tags: &'a [String], prefix: &str) -> Option<&'a str> {
    tags.iter()
        .find_map(|tag| tag.strip_prefix(prefix).map(str::trim))
        .filter(|value| !value.is_empty())
}

fn push_unique_tag(tags: &mut Vec<String>, tag: String) {
    if !tags.iter().any(|existing| existing == &tag) {
        tags.push(tag);
    }
}

fn normalized_skill_name(value: &str) -> String {
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

fn move_ported_skill_source(source: &WorkspaceSkillSource, ported_dir: &Path) -> Result<()> {
    let source_path = source.path();
    let package_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("skill");
    let mut destination = ported_dir.join(package_name);
    if destination.exists() {
        destination = ported_dir.join(format!(
            "{}-{}",
            package_name,
            chrono::Utc::now().timestamp()
        ));
    }
    std::fs::rename(source_path, &destination).with_context(|| {
        format!(
            "failed to move {} to {}",
            source_path.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn load_skills_from_directory(skills_dir: &Path) -> Vec<Skill> {
    if !skills_dir.exists() {
        return Vec::new();
    }

    let mut skills = Vec::new();

    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return skills;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some(PORTED_SKILLS_DIR_NAME) {
            continue;
        }

        match audit::audit_skill_directory(&path) {
            Ok(report) if report.is_clean() => {}
            Ok(report) => {
                tracing::warn!(
                    "skipping insecure skill directory {}: {}",
                    path.display(),
                    report.summary()
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    "skipping unauditable skill directory {}: {err}",
                    path.display()
                );
                continue;
            }
        }

        // Try SKILL.toml first, then SKILL.md
        let manifest_path = path.join("SKILL.toml");
        let md_path = path.join("SKILL.md");

        if manifest_path.exists() {
            if let Ok(skill) = load_skill_toml(&manifest_path) {
                skills.push(skill);
            }
        } else if md_path.exists() {
            if let Ok(skill) = load_skill_md(&md_path, &path) {
                skills.push(skill);
            }
        }
    }

    skills
}

fn finalize_open_skill(mut skill: Skill) -> Skill {
    if !skill.tags.iter().any(|tag| tag == "open-skills") {
        skill.tags.push("open-skills".to_string());
    }
    if skill.author.is_none() {
        skill.author = Some("besoeasy/open-skills".to_string());
    }
    skill
}

fn load_open_skills_from_directory(skills_dir: &Path) -> Vec<Skill> {
    if !skills_dir.exists() {
        return Vec::new();
    }

    let mut skills = Vec::new();

    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return skills;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        match audit::audit_skill_directory(&path) {
            Ok(report) if report.is_clean() => {}
            Ok(report) => {
                tracing::warn!(
                    "skipping insecure open-skill directory {}: {}",
                    path.display(),
                    report.summary()
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    "skipping unauditable open-skill directory {}: {err}",
                    path.display()
                );
                continue;
            }
        }

        let manifest_path = path.join("SKILL.toml");
        let md_path = path.join("SKILL.md");

        if manifest_path.exists() {
            if let Ok(skill) = load_skill_toml(&manifest_path) {
                skills.push(finalize_open_skill(skill));
            }
        } else if md_path.exists() {
            if let Ok(skill) = load_open_skill_md(&md_path) {
                skills.push(skill);
            }
        }
    }

    skills
}

fn load_open_skills(repo_dir: &Path) -> Vec<Skill> {
    // Modern open-skills layout stores skill packages in `skills/<name>/SKILL.md`.
    // Prefer that structure to avoid treating repository docs (e.g. CONTRIBUTING.md)
    // as executable skills.
    let nested_skills_dir = repo_dir.join("skills");
    if nested_skills_dir.is_dir() {
        return load_open_skills_from_directory(&nested_skills_dir);
    }

    let mut skills = Vec::new();

    let Ok(entries) = std::fs::read_dir(repo_dir) else {
        return skills;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let is_markdown = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if !is_markdown {
            continue;
        }

        let is_readme = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("README.md"));
        if is_readme {
            continue;
        }

        match audit::audit_open_skill_markdown(&path, repo_dir) {
            Ok(report) if report.is_clean() => {}
            Ok(report) => {
                tracing::warn!(
                    "skipping insecure open-skill file {}: {}",
                    path.display(),
                    report.summary()
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    "skipping unauditable open-skill file {}: {err}",
                    path.display()
                );
                continue;
            }
        }

        if let Ok(skill) = load_open_skill_md(&path) {
            skills.push(skill);
        }
    }

    skills
}

fn parse_open_skills_enabled(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn open_skills_enabled_from_sources(
    config_open_skills_enabled: Option<bool>,
    env_override: Option<&str>,
) -> bool {
    if let Some(raw) = env_override {
        if let Some(enabled) = parse_open_skills_enabled(raw) {
            return enabled;
        }
        if !raw.trim().is_empty() {
            tracing::warn!(
                "Ignoring invalid SYNAPSECLAW_OPEN_SKILLS_ENABLED (valid: 1|0|true|false|yes|no|on|off)"
            );
        }
    }

    config_open_skills_enabled.unwrap_or(false)
}

fn open_skills_enabled(config_open_skills_enabled: Option<bool>) -> bool {
    let env_override = std::env::var("SYNAPSECLAW_OPEN_SKILLS_ENABLED").ok();
    open_skills_enabled_from_sources(config_open_skills_enabled, env_override.as_deref())
}

fn resolve_open_skills_dir_from_sources(
    env_dir: Option<&str>,
    config_dir: Option<&str>,
    home_dir: Option<&Path>,
) -> Option<PathBuf> {
    let parse_dir = |raw: &str| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    };

    if let Some(env_dir) = env_dir.and_then(parse_dir) {
        return Some(env_dir);
    }
    if let Some(config_dir) = config_dir.and_then(parse_dir) {
        return Some(config_dir);
    }
    home_dir.map(|home| home.join("open-skills"))
}

fn resolve_open_skills_dir(config_open_skills_dir: Option<&str>) -> Option<PathBuf> {
    let env_dir = std::env::var("SYNAPSECLAW_OPEN_SKILLS_DIR").ok();
    let home_dir = UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    resolve_open_skills_dir_from_sources(
        env_dir.as_deref(),
        config_open_skills_dir,
        home_dir.as_deref(),
    )
}

fn ensure_open_skills_repo(
    config_open_skills_enabled: Option<bool>,
    config_open_skills_dir: Option<&str>,
) -> Option<PathBuf> {
    if !open_skills_enabled(config_open_skills_enabled) {
        return None;
    }

    let repo_dir = resolve_open_skills_dir(config_open_skills_dir)?;

    if !repo_dir.exists() {
        if !clone_open_skills_repo(&repo_dir) {
            return None;
        }
        let _ = mark_open_skills_synced(&repo_dir);
        return Some(repo_dir);
    }

    if should_sync_open_skills(&repo_dir) {
        if pull_open_skills_repo(&repo_dir) {
            let _ = mark_open_skills_synced(&repo_dir);
        } else {
            tracing::warn!(
                "open-skills update failed; using local copy from {}",
                repo_dir.display()
            );
        }
    }

    Some(repo_dir)
}

fn clone_open_skills_repo(repo_dir: &Path) -> bool {
    if let Some(parent) = repo_dir.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "failed to create open-skills parent directory {}: {err}",
                parent.display()
            );
            return false;
        }
    }

    let output = Command::new("git")
        .args(["clone", "--depth", "1", OPEN_SKILLS_REPO_URL])
        .arg(repo_dir)
        .output();

    match output {
        Ok(result) if result.status.success() => {
            tracing::info!("initialized open-skills at {}", repo_dir.display());
            true
        }
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            tracing::warn!("failed to clone open-skills: {stderr}");
            false
        }
        Err(err) => {
            tracing::warn!("failed to run git clone for open-skills: {err}");
            false
        }
    }
}

fn pull_open_skills_repo(repo_dir: &Path) -> bool {
    // If user points to a non-git directory via env var, keep using it without pulling.
    if !repo_dir.join(".git").exists() {
        return true;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["pull", "--ff-only"])
        .output();

    match output {
        Ok(result) if result.status.success() => true,
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            tracing::warn!("failed to pull open-skills updates: {stderr}");
            false
        }
        Err(err) => {
            tracing::warn!("failed to run git pull for open-skills: {err}");
            false
        }
    }
}

fn should_sync_open_skills(repo_dir: &Path) -> bool {
    let marker = repo_dir.join(OPEN_SKILLS_SYNC_MARKER);
    let Ok(metadata) = std::fs::metadata(marker) else {
        return true;
    };
    let Ok(modified_at) = metadata.modified() else {
        return true;
    };
    let Ok(age) = SystemTime::now().duration_since(modified_at) else {
        return true;
    };

    age >= Duration::from_secs(OPEN_SKILLS_SYNC_INTERVAL_SECS)
}

fn mark_open_skills_synced(repo_dir: &Path) -> Result<()> {
    std::fs::write(repo_dir.join(OPEN_SKILLS_SYNC_MARKER), b"synced")?;
    Ok(())
}

/// Load a skill from a SKILL.toml manifest
fn load_skill_toml(path: &Path) -> Result<Skill> {
    let content = std::fs::read_to_string(path)?;
    let manifest: SkillManifest = toml::from_str(&content)?;

    Ok(Skill {
        name: manifest.skill.name,
        description: manifest.skill.description,
        version: manifest.skill.version,
        author: manifest.skill.author,
        tags: manifest.skill.tags,
        tools: manifest.tools,
        prompts: manifest.prompts,
        location: Some(path.to_path_buf()),
    })
}

/// Load a skill from a SKILL.md file (simpler format)
fn load_skill_md(path: &Path, dir: &Path) -> Result<Skill> {
    let default_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    load_skill_md_with_default_name(path, default_name)
}

fn load_skill_md_with_default_name(path: &Path, default_name: &str) -> Result<Skill> {
    let content = std::fs::read_to_string(path)?;
    let parsed = parse_skill_markdown(&content);

    Ok(Skill {
        name: parsed.meta.name.unwrap_or_else(|| default_name.to_string()),
        description: parsed
            .meta
            .description
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| extract_description(&parsed.body)),
        version: parsed.meta.version.unwrap_or_else(default_version),
        author: parsed.meta.author,
        tags: parsed.meta.tags,
        tools: Vec::new(),
        prompts: vec![parsed.body],
        location: Some(path.to_path_buf()),
    })
}

fn load_open_skill_md(path: &Path) -> Result<Skill> {
    let content = std::fs::read_to_string(path)?;
    let parsed = parse_skill_markdown(&content);
    let file_stem = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("open-skill")
        .to_string();
    let name = if file_stem.eq_ignore_ascii_case("skill") {
        path.parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or(&file_stem)
            .to_string()
    } else {
        file_stem
    };
    Ok(finalize_open_skill(Skill {
        name: parsed.meta.name.unwrap_or(name),
        description: parsed
            .meta
            .description
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| extract_description(&parsed.body)),
        version: parsed
            .meta
            .version
            .unwrap_or_else(|| "open-skills".to_string()),
        author: parsed
            .meta
            .author
            .or_else(|| Some("besoeasy/open-skills".to_string())),
        tags: parsed.meta.tags,
        tools: Vec::new(),
        prompts: vec![parsed.body],
        location: Some(path.to_path_buf()),
    }))
}

struct ParsedSkillMarkdown {
    meta: SkillMarkdownMeta,
    body: String,
}

fn parse_skill_markdown(content: &str) -> ParsedSkillMarkdown {
    if let Some((frontmatter, body)) = split_skill_frontmatter(content) {
        if let Ok(meta) = serde_yaml::from_str::<SkillMarkdownMeta>(&frontmatter) {
            return ParsedSkillMarkdown { meta, body };
        }
    }

    ParsedSkillMarkdown {
        meta: SkillMarkdownMeta::default(),
        body: content.to_string(),
    }
}

fn split_skill_frontmatter(content: &str) -> Option<(String, String)> {
    let normalized = content.replace("\r\n", "\n");
    let rest = normalized.strip_prefix("---\n")?;
    if let Some(idx) = rest.find("\n---\n") {
        let frontmatter = rest[..idx].to_string();
        let body = rest[idx + 5..].to_string();
        return Some((frontmatter, body));
    }
    if let Some(frontmatter) = rest.strip_suffix("\n---") {
        return Some((frontmatter.to_string(), String::new()));
    }
    None
}

fn extract_description(content: &str) -> String {
    content
        .lines()
        .find(|line| !line.starts_with('#') && !line.trim().is_empty())
        .unwrap_or("No description")
        .trim()
        .to_string()
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

fn write_xml_text_element(out: &mut String, indent: usize, tag: &str, value: &str) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push('<');
    out.push_str(tag);
    out.push('>');
    append_xml_escaped(out, value);
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

fn resolve_skill_location(skill: &Skill, workspace_dir: &Path) -> PathBuf {
    skill.location.clone().unwrap_or_else(|| {
        workspace_dir
            .join("skills")
            .join(&skill.name)
            .join("SKILL.md")
    })
}

fn render_skill_location(skill: &Skill, workspace_dir: &Path, prefer_relative: bool) -> String {
    let location = resolve_skill_location(skill, workspace_dir);
    if prefer_relative {
        if let Ok(relative) = location.strip_prefix(workspace_dir) {
            return relative.display().to_string();
        }
    }
    location.display().to_string()
}

pub(crate) fn skill_activation_id(skill: &Skill) -> String {
    let mut normalized = String::new();
    for ch in skill.name.trim().chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            normalized.extend(ch.to_lowercase());
        } else {
            normalized.push('-');
        }
    }
    format!("skill:{normalized}")
}

/// Build the default compact "Available Skills" system prompt section.
pub fn skills_to_prompt(skills: &[Skill], workspace_dir: &Path) -> String {
    skills_to_prompt_with_mode(
        skills,
        workspace_dir,
        synapse_domain::config::schema::SkillsPromptInjectionMode::Compact,
    )
}

/// Build the "Available Skills" system prompt section with configurable verbosity.
pub fn skills_to_prompt_with_mode(
    skills: &[Skill],
    workspace_dir: &Path,
    mode: synapse_domain::config::schema::SkillsPromptInjectionMode,
) -> String {
    use std::fmt::Write;

    if skills.is_empty() {
        return String::new();
    }
    let selection = select_skills_for_prompt(skills, mode);
    if selection.skills.is_empty() {
        return String::new();
    }

    let mut prompt = match mode {
        synapse_domain::config::schema::SkillsPromptInjectionMode::Full => String::from(
            "## Available Skills\n\n\
             Skill instructions and tool metadata are preloaded below.\n\
             Follow these instructions directly; do not read skill files at runtime unless the user asks.\n\n\
             <available_skills>\n",
        ),
        synapse_domain::config::schema::SkillsPromptInjectionMode::Compact => String::from(
            "## Available Skills\n\n\
             Skill summaries are preloaded below to keep context compact.\n\
             Skill instructions are loaded on demand with `skill_read` using the skill id, name, or location.\n\
             Do not read skill files directly unless the user explicitly asks for raw file contents.\n\n\
             <available_skills>\n",
        ),
    };

    for skill in selection.skills {
        let _ = writeln!(prompt, "  <skill>");
        write_xml_text_element(&mut prompt, 4, "id", &skill_activation_id(skill));
        write_xml_text_element(&mut prompt, 4, "name", &skill.name);
        write_xml_text_element(&mut prompt, 4, "origin", infer_loaded_skill_origin(skill));
        write_xml_text_element(&mut prompt, 4, "status", "active");
        write_xml_text_element(&mut prompt, 4, "description", &skill.description);
        let location = render_skill_location(
            skill,
            workspace_dir,
            matches!(
                mode,
                synapse_domain::config::schema::SkillsPromptInjectionMode::Compact
            ),
        );
        write_xml_text_element(&mut prompt, 4, "location", &location);

        if matches!(
            mode,
            synapse_domain::config::schema::SkillsPromptInjectionMode::Full
        ) {
            if !skill.prompts.is_empty() {
                let _ = writeln!(prompt, "    <instructions>");
                for instruction in &skill.prompts {
                    write_xml_text_element(&mut prompt, 6, "instruction", instruction);
                }
                let _ = writeln!(prompt, "    </instructions>");
            }

            if !skill.tools.is_empty() {
                let _ = writeln!(prompt, "    <tools>");
                for tool in &skill.tools {
                    let _ = writeln!(prompt, "      <tool>");
                    write_xml_text_element(&mut prompt, 8, "name", &tool.name);
                    write_xml_text_element(&mut prompt, 8, "description", &tool.description);
                    write_xml_text_element(&mut prompt, 8, "kind", &tool.kind);
                    let _ = writeln!(prompt, "      </tool>");
                }
                let _ = writeln!(prompt, "    </tools>");
            }
        }

        let _ = writeln!(prompt, "  </skill>");
    }
    if selection.omitted > 0 {
        let _ = writeln!(
            prompt,
            "  <skills_omitted count=\"{}\" reason=\"catalog_budget_exceeded\" />",
            selection.omitted
        );
    }

    prompt.push_str("</available_skills>");
    prompt
}

struct PromptSkillSelection<'a> {
    skills: Vec<&'a Skill>,
    omitted: usize,
}

fn select_skills_for_prompt(
    skills: &[Skill],
    mode: synapse_domain::config::schema::SkillsPromptInjectionMode,
) -> PromptSkillSelection<'_> {
    let mut selected = Vec::<&Skill>::new();
    let mut positions = HashMap::<String, usize>::new();

    for skill in skills {
        let key = skill_activation_id(skill);
        if let Some(position) = positions.get(&key).copied() {
            if loaded_skill_prompt_priority(skill)
                > loaded_skill_prompt_priority(selected[position])
            {
                selected[position] = skill;
            }
            continue;
        }
        positions.insert(key, selected.len());
        selected.push(skill);
    }

    selected.sort_by(|left, right| {
        loaded_skill_prompt_priority(right).cmp(&loaded_skill_prompt_priority(left))
    });

    let cap = match mode {
        synapse_domain::config::schema::SkillsPromptInjectionMode::Compact => {
            SkillPromptBudget::default().max_catalog_entries
        }
        synapse_domain::config::schema::SkillsPromptInjectionMode::Full => usize::MAX,
    };
    let omitted = selected.len().saturating_sub(cap);
    selected.truncate(cap);
    PromptSkillSelection {
        skills: selected,
        omitted,
    }
}

fn loaded_skill_prompt_priority(skill: &Skill) -> u8 {
    match infer_loaded_skill_origin(skill) {
        "manual" => 5,
        "imported" => 4,
        _ => 1,
    }
}

/// Get the skills directory path
pub fn skills_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("skills")
}

/// Initialize the skills directory with a README
pub fn init_skills_dir(workspace_dir: &Path) -> Result<()> {
    let dir = skills_dir(workspace_dir);
    std::fs::create_dir_all(&dir)?;

    let readme = dir.join("README.md");
    if !readme.exists() {
        std::fs::write(
            &readme,
            "# SynapseClaw Skills\n\n\
             Each subdirectory is a skill. Create a `SKILL.toml` or `SKILL.md` file inside.\n\n\
             ## SKILL.toml format\n\n\
             ```toml\n\
             [skill]\n\
             name = \"my-skill\"\n\
             description = \"What this skill does\"\n\
             version = \"0.1.0\"\n\
             author = \"your-name\"\n\
             tags = [\"productivity\", \"automation\"]\n\n\
             [[tools]]\n\
             name = \"my_tool\"\n\
             description = \"What this tool does\"\n\
             kind = \"shell\"\n\
             command = \"echo hello\"\n\
             ```\n\n\
             ## SKILL.md format (simpler)\n\n\
             Just write a markdown file with instructions for the agent.\n\
             Optional YAML frontmatter is supported for `name`, `description`, `version`, `author`, and `tags`.\n\
             The agent will read it and follow the instructions.\n\n\
             ## Installing community skills\n\n\
             ```bash\n\
             synapseclaw skills install <source>\n\
             synapseclaw skills list\n\
             ```\n",
        )?;
    }

    Ok(())
}

fn is_git_source(source: &str) -> bool {
    is_git_scheme_source(source, "https://")
        || is_git_scheme_source(source, "http://")
        || is_git_scheme_source(source, "ssh://")
        || is_git_scheme_source(source, "git://")
        || is_git_scp_source(source)
}

fn is_git_scheme_source(source: &str, scheme: &str) -> bool {
    let Some(rest) = source.strip_prefix(scheme) else {
        return false;
    };
    if rest.is_empty() || rest.starts_with('/') {
        return false;
    }

    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
}

fn is_git_scp_source(source: &str) -> bool {
    // SCP-like syntax accepted by git, e.g. git@host:owner/repo.git
    // Keep this strict enough to avoid treating local paths as git remotes.
    let Some((user_host, remote_path)) = source.split_once(':') else {
        return false;
    };
    if remote_path.is_empty() {
        return false;
    }
    if source.contains("://") {
        return false;
    }

    let Some((user, host)) = user_host.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && !host.is_empty()
        && !user.contains('/')
        && !user.contains('\\')
        && !host.contains('/')
        && !host.contains('\\')
}

fn snapshot_skill_children(skills_path: &Path) -> Result<HashSet<PathBuf>> {
    let mut paths = HashSet::new();
    for entry in std::fs::read_dir(skills_path)? {
        let entry = entry?;
        paths.insert(entry.path());
    }
    Ok(paths)
}

fn detect_newly_installed_directory(
    skills_path: &Path,
    before: &HashSet<PathBuf>,
) -> Result<PathBuf> {
    let mut created = Vec::new();
    for entry in std::fs::read_dir(skills_path)? {
        let entry = entry?;
        let path = entry.path();
        if !before.contains(&path) && path.is_dir() {
            created.push(path);
        }
    }

    match created.len() {
        1 => Ok(created.remove(0)),
        0 => anyhow::bail!(
            "Unable to determine installed skill directory after clone (no new directory found)"
        ),
        _ => anyhow::bail!(
            "Unable to determine installed skill directory after clone (multiple new directories found)"
        ),
    }
}

fn enforce_skill_security_audit(skill_path: &Path) -> Result<audit::SkillAuditReport> {
    let report = audit::audit_skill_directory(skill_path)?;
    if report.is_clean() {
        return Ok(report);
    }

    anyhow::bail!("Skill security audit failed: {}", report.summary());
}

fn remove_git_metadata(skill_path: &Path) -> Result<()> {
    let git_dir = skill_path.join(".git");
    if git_dir.exists() {
        std::fs::remove_dir_all(&git_dir)
            .with_context(|| format!("failed to remove {}", git_dir.display()))?;
    }
    Ok(())
}

fn copy_dir_recursive_secure(src: &Path, dest: &Path) -> Result<()> {
    let src_meta = std::fs::symlink_metadata(src)
        .with_context(|| format!("failed to read metadata for {}", src.display()))?;
    if src_meta.file_type().is_symlink() {
        anyhow::bail!(
            "Refusing to copy symlinked skill source path: {}",
            src.display()
        );
    }
    if !src_meta.is_dir() {
        anyhow::bail!("Skill source must be a directory: {}", src.display());
    }

    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create destination {}", dest.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&src_path)
            .with_context(|| format!("failed to read metadata for {}", src_path.display()))?;

        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "Refusing to copy symlink within skill source: {}",
                src_path.display()
            );
        }

        if metadata.is_dir() {
            copy_dir_recursive_secure(&src_path, &dest_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&src_path, &dest_path).with_context(|| {
                format!(
                    "failed to copy skill file from {} to {}",
                    src_path.display(),
                    dest_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn install_local_skill_source(source: &str, skills_path: &Path) -> Result<(PathBuf, usize)> {
    let source_path = PathBuf::from(source);
    if !source_path.exists() {
        anyhow::bail!("Source path does not exist: {source}");
    }

    let source_path = source_path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize source path {source}"))?;
    let _ = enforce_skill_security_audit(&source_path)?;

    let name = source_path
        .file_name()
        .context("Source path must include a directory name")?;
    let dest = skills_path.join(name);
    if dest.exists() {
        anyhow::bail!("Destination skill already exists: {}", dest.display());
    }

    if let Err(err) = copy_dir_recursive_secure(&source_path, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(err);
    }

    match enforce_skill_security_audit(&dest) {
        Ok(report) => Ok((dest, report.files_scanned)),
        Err(err) => {
            let _ = std::fs::remove_dir_all(&dest);
            Err(err)
        }
    }
}

fn install_git_skill_source(source: &str, skills_path: &Path) -> Result<(PathBuf, usize)> {
    let before = snapshot_skill_children(skills_path)?;
    let output = std::process::Command::new("git")
        .args(["clone", "--depth", "1", source])
        .current_dir(skills_path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git clone failed: {stderr}");
    }

    let installed_dir = detect_newly_installed_directory(skills_path, &before)?;
    remove_git_metadata(&installed_dir)?;
    match enforce_skill_security_audit(&installed_dir) {
        Ok(report) => Ok((installed_dir, report.files_scanned)),
        Err(err) => {
            let _ = std::fs::remove_dir_all(&installed_dir);
            Err(err)
        }
    }
}

pub(crate) fn loaded_skill_to_runtime_candidate(
    skill: &Skill,
    workspace_dir: &Path,
) -> SkillRuntimeCandidate {
    let origin = infer_loaded_skill_origin(skill);
    let source = match origin {
        "manual" => SkillSource::Manual,
        "imported" => SkillSource::Imported,
        _ => SkillSource::External,
    };
    SkillRuntimeCandidate {
        id: skill_activation_id(skill),
        name: skill.name.clone(),
        description: skill.description.clone(),
        source,
        trust_level: if source == SkillSource::Manual {
            SkillTrustLevel::Trusted
        } else {
            SkillTrustLevel::Community
        },
        status: SkillStatus::Active,
        disabled: false,
        review_required: false,
        task_family: None,
        lineage_task_families: Vec::new(),
        tool_pattern: skill.tools.iter().map(|tool| tool.name.clone()).collect(),
        tags: skill.tags.clone(),
        category: None,
        agents: Vec::new(),
        channels: Vec::new(),
        platforms: Vec::new(),
        required_tools: Vec::new(),
        required_tool_roles: Vec::new(),
        required_model_lanes: Vec::new(),
        required_modalities: Vec::new(),
        required_setup: Vec::new(),
        source_ref: Some(render_skill_location(skill, workspace_dir, true)),
        content_chars: skill
            .prompts
            .iter()
            .map(|prompt| prompt.chars().count())
            .sum(),
        relevance_score: 0.0,
    }
}

fn resolve_loaded_skill_status(skills: &[Skill], workspace_dir: &Path) -> SkillResolutionReport {
    let request = SkillLoadRequest {
        agent_id: "local".into(),
        platform: Some(std::env::consts::OS.into()),
        prompt_budget: SkillPromptBudget {
            max_catalog_entries: skills.len().max(1),
            max_preloaded_skills: 0,
            max_skill_chars: usize::MAX,
        },
        ..SkillLoadRequest::default()
    };
    resolve_skill_states(
        &request,
        skills
            .iter()
            .map(|skill| loaded_skill_to_runtime_candidate(skill, workspace_dir))
            .collect(),
    )
}

fn print_skill_resolution_report(
    report: &SkillResolutionReport,
    include: impl Fn(SkillRuntimeState) -> bool,
) -> bool {
    print_skill_resolution_report_inner(report, include, true)
}

fn print_skill_resolution_report_if_nonempty(
    report: &SkillResolutionReport,
    include: impl Fn(SkillRuntimeState) -> bool,
) -> bool {
    print_skill_resolution_report_inner(report, include, false)
}

fn print_skill_resolution_report_inner(
    report: &SkillResolutionReport,
    include: impl Fn(SkillRuntimeState) -> bool,
    print_empty: bool,
) -> bool {
    let decisions = report
        .decisions
        .iter()
        .filter(|decision| include(decision.state))
        .collect::<Vec<_>>();
    if decisions.is_empty() {
        if print_empty {
            println!("No matching skills.");
        }
        return false;
    }

    print_skill_resolution_decisions(&decisions);
    true
}

fn print_skill_resolution_decisions(
    decisions: &[&synapse_domain::application::services::skill_governance_service::SkillRuntimeDecision],
) {
    if decisions.is_empty() {
        println!("No matching skills.");
        return;
    }

    println!("Skills runtime status ({}):", decisions.len());
    println!();
    for decision in decisions {
        println!(
            "  {} [{} / {}] — {}",
            console::style(&decision.name).white().bold(),
            decision.state,
            decision.activation_mode,
            decision.reason_code
        );
        if let Some(source_ref) = &decision.source_ref {
            println!("    location: {source_ref}");
        }
        if let Some(shadowed_by) = &decision.shadowed_by {
            println!("    shadowed_by: {shadowed_by}");
        }
        if !decision.missing_capabilities.is_empty() {
            let missing = decision
                .missing_capabilities
                .iter()
                .map(|cap| format!("{}:{}", cap.kind, cap.name))
                .collect::<Vec<_>>()
                .join(", ");
            println!("    missing: {missing}");
        }
    }
    println!();
}

pub fn format_skill_resolution_report_text(report: &SkillResolutionReport) -> String {
    format_skill_resolution_report_view_text(report, RuntimeSkillStatusView::All)
}

pub fn format_skill_resolution_report_view_text(
    report: &SkillResolutionReport,
    view: RuntimeSkillStatusView,
) -> String {
    let decisions = report.diagnostics();
    let filtered = decisions
        .iter()
        .copied()
        .filter(|decision| match view {
            RuntimeSkillStatusView::All => true,
            RuntimeSkillStatusView::Blocked => !matches!(
                decision.state,
                SkillRuntimeState::Active | SkillRuntimeState::Candidate
            ),
            RuntimeSkillStatusView::Candidates => decision.state == SkillRuntimeState::Candidate,
        })
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return match view {
            RuntimeSkillStatusView::All => "No matching skills.".to_string(),
            RuntimeSkillStatusView::Blocked => "No blocked skills.".to_string(),
            RuntimeSkillStatusView::Candidates => "No skill candidates.".to_string(),
        };
    }

    let active = decisions
        .iter()
        .filter(|decision| decision.state == SkillRuntimeState::Active)
        .count();
    let review = decisions
        .iter()
        .filter(|decision| decision.state == SkillRuntimeState::Candidate)
        .count();
    let blocked = decisions
        .len()
        .saturating_sub(active)
        .saturating_sub(review);

    let label = match view {
        RuntimeSkillStatusView::All => "Skills status",
        RuntimeSkillStatusView::Blocked => "Blocked skills",
        RuntimeSkillStatusView::Candidates => "Skill candidates",
    };
    let mut lines = vec![format!(
        "{label}: {} shown, {} total, {} active, {} review, {} blocked",
        filtered.len(),
        decisions.len(),
        active,
        review,
        blocked
    )];
    for decision in filtered.iter().take(12).copied() {
        lines.push(format!(
            "- {} [{} / {} / {}]: {}",
            decision.name,
            decision.source,
            decision.state,
            decision.activation_mode,
            decision.reason_code
        ));
        if let Some(source_ref) = &decision.source_ref {
            lines.push(format!("  location: {source_ref}"));
        }
        if let Some(shadowed_by) = &decision.shadowed_by {
            lines.push(format!("  shadowed_by: {shadowed_by}"));
        }
        if !decision.missing_capabilities.is_empty() {
            let missing = decision
                .missing_capabilities
                .iter()
                .map(|cap| format!("{}:{}", cap.kind, cap.name))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("  missing: {missing}"));
        }
    }
    if filtered.len() > 12 {
        lines.push(format!("... {} more skills omitted", filtered.len() - 12));
    }
    lines.join("\n")
}

fn patch_candidate_to_runtime_candidate(patch: &SkillPatchCandidate) -> SkillRuntimeCandidate {
    SkillRuntimeCandidate {
        id: patch.id.clone(),
        name: format!("Patch candidate for {}", patch.target_skill_id),
        description: patch.diff_summary.clone(),
        source: SkillSource::GeneratedPatch,
        trust_level: SkillTrustLevel::AgentCreated,
        status: SkillStatus::Candidate,
        disabled: false,
        review_required: true,
        task_family: None,
        lineage_task_families: Vec::new(),
        tool_pattern: Vec::new(),
        tags: vec!["generated_patch".into()],
        category: Some("skill_patch_candidate".into()),
        agents: Vec::new(),
        channels: Vec::new(),
        platforms: Vec::new(),
        required_tools: Vec::new(),
        required_tool_roles: Vec::new(),
        required_model_lanes: Vec::new(),
        required_modalities: Vec::new(),
        required_setup: Vec::new(),
        source_ref: Some(format!("memory:{}", patch.id)),
        content_chars: patch.proposed_body.chars().count(),
        relevance_score: 0.0,
    }
}

pub async fn runtime_skill_resolution_report(
    config: &synapse_domain::config::schema::Config,
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    available_tools: Vec<String>,
    available_tool_roles: Vec<String>,
    limit: usize,
) -> Result<SkillResolutionReport> {
    let mut candidates = load_skills_with_config(&config.workspace_dir, config)
        .iter()
        .map(|skill| loaded_skill_to_runtime_candidate(skill, &config.workspace_dir))
        .collect::<Vec<_>>();

    let learned = memory.list_skills(&agent_id.to_string(), limit).await?;
    candidates.extend(learned.iter().map(SkillRuntimeCandidate::from_memory_skill));

    let patch_category = skill_patch_candidate_service::skill_patch_candidate_memory_category();
    let patch_entries = memory
        .list(Some(&patch_category), None, limit)
        .await
        .unwrap_or_default();
    candidates.extend(
        patch_entries
            .iter()
            .filter_map(skill_patch_candidate_service::parse_skill_patch_candidate_entry)
            .map(|patch| patch_candidate_to_runtime_candidate(&patch)),
    );

    let request = SkillLoadRequest {
        agent_id: agent_id.to_string(),
        platform: Some(std::env::consts::OS.to_string()),
        available_tools,
        available_tool_roles,
        prompt_budget: SkillPromptBudget {
            max_catalog_entries: candidates.len().max(1),
            max_preloaded_skills: 0,
            max_skill_chars: 0,
        },
        ..SkillLoadRequest::default()
    };
    Ok(resolve_skill_states(&request, candidates))
}

pub async fn format_runtime_skill_status(
    config: &synapse_domain::config::schema::Config,
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    available_tools: Vec<String>,
    available_tool_roles: Vec<String>,
    limit: usize,
) -> Result<String> {
    format_runtime_skill_status_view(
        config,
        memory,
        agent_id,
        available_tools,
        available_tool_roles,
        limit,
        RuntimeSkillStatusView::All,
    )
    .await
}

pub async fn format_runtime_skill_status_view(
    config: &synapse_domain::config::schema::Config,
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    available_tools: Vec<String>,
    available_tool_roles: Vec<String>,
    limit: usize,
    view: RuntimeSkillStatusView,
) -> Result<String> {
    let report = runtime_skill_resolution_report(
        config,
        memory,
        agent_id,
        available_tools,
        available_tool_roles,
        limit,
    )
    .await?;
    Ok(format_skill_resolution_report_view_text(&report, view))
}

pub async fn list_skill_use_traces(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<SkillUseTrace>> {
    let limit = limit.max(1);
    let category = skill_use_trace_memory_category();
    let prefix = skill_use_trace_memory_key_prefix(agent_id);
    let entries = memory
        .list(Some(&category), None, limit.saturating_mul(4))
        .await?;
    let mut traces = entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix))
        .filter_map(parse_skill_use_trace_entry)
        .collect::<Vec<_>>();
    traces.sort_by(|left, right| {
        right
            .observed_at_unix
            .cmp(&left.observed_at_unix)
            .then_with(|| right.id.cmp(&left.id))
    });
    traces.truncate(limit);
    Ok(traces)
}

pub async fn list_skill_activation_traces(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<SkillActivationTrace>> {
    let limit = limit.max(1);
    let category = skill_activation_trace_memory_category();
    let prefix = skill_activation_trace_memory_key_prefix(agent_id);
    let entries = memory
        .list(Some(&category), None, limit.saturating_mul(4))
        .await?;
    let mut traces = entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix))
        .filter_map(parse_skill_activation_trace_entry)
        .collect::<Vec<_>>();
    traces.truncate(limit);
    Ok(traces)
}

pub async fn format_skill_use_traces_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    limit: usize,
) -> Result<String> {
    let traces = list_skill_use_traces(memory, agent_id, limit).await?;
    if traces.is_empty() {
        return Ok(format!("No skill use traces found for agent {agent_id}."));
    }

    let mut lines = vec![format!(
        "Skill use traces for agent {agent_id} ({}):",
        traces.len()
    )];
    for trace in &traces {
        lines.extend(format_skill_use_trace_lines(trace));
    }
    Ok(lines.join(
        "
",
    ))
}

pub async fn skill_health_report_for_agent(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_limit: usize,
    trace_limit: usize,
) -> Result<SkillHealthReport> {
    let skill_limit = skill_limit.max(1);
    let skills = memory
        .list_skills(&agent_id.to_string(), skill_limit)
        .await?;
    let traces = list_skill_use_traces(memory, agent_id, trace_limit.max(1)).await?;
    let activation_traces =
        list_skill_activation_traces(memory, agent_id, trace_limit.max(1)).await?;
    let rollback_records = list_skill_patch_rollback_records(memory, trace_limit.max(1))
        .await?
        .into_iter()
        .filter(|record| record.agent_id == agent_id)
        .collect::<Vec<_>>();
    let learned = skills
        .iter()
        .filter(|skill| skill.origin == SkillOrigin::Learned)
        .cloned()
        .collect::<Vec<_>>();
    let review_decisions = review_learned_skills(&learned, &[]);
    Ok(build_skill_health_report(
        agent_id.to_string(),
        &skills,
        &traces,
        &activation_traces,
        &rollback_records,
        &review_decisions,
        chrono::Utc::now().timestamp(),
        skill_limit,
    ))
}

pub async fn format_skill_health_report_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_limit: usize,
    trace_limit: usize,
) -> Result<String> {
    let report = skill_health_report_for_agent(memory, agent_id, skill_limit, trace_limit).await?;
    Ok(format_skill_health_report_text(&report))
}

#[derive(Debug, Clone, Serialize)]
pub struct AppliedSkillHealthCleanupDecision {
    pub skill_id: String,
    pub skill_name: String,
    pub previous_status: SkillStatus,
    pub target_status: SkillStatus,
    pub reason: String,
}

pub async fn apply_skill_health_cleanup_decisions(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    decisions: &[SkillHealthCleanupDecision],
) -> Result<Vec<AppliedSkillHealthCleanupDecision>> {
    let mut applied = Vec::new();
    for decision in decisions {
        memory
            .update_skill(
                &decision.skill_id,
                empty_status_update(decision.target_status.clone()),
                &agent_id.to_string(),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to apply skill health cleanup decision for {}",
                    decision.skill_id
                )
            })?;
        applied.push(AppliedSkillHealthCleanupDecision {
            skill_id: decision.skill_id.clone(),
            skill_name: decision.skill_name.clone(),
            previous_status: decision.current_status.clone(),
            target_status: decision.target_status.clone(),
            reason: decision.reason.clone(),
        });
    }
    Ok(applied)
}

pub async fn format_skill_health_cleanup_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_limit: usize,
    trace_limit: usize,
    apply: bool,
) -> Result<String> {
    let report = skill_health_report_for_agent(memory, agent_id, skill_limit, trace_limit).await?;
    let decisions = skill_health_cleanup_decisions(&report);
    let mut lines = vec![format_skill_health_report_text(&report)];

    if decisions.is_empty() {
        lines.push("No skill cleanup lifecycle changes are eligible.".into());
        return Ok(lines.join(
            "
",
        ));
    }

    lines.push(String::new());
    lines.push(format!(
        "Eligible cleanup lifecycle changes ({}):",
        decisions.len()
    ));
    for decision in &decisions {
        lines.push(format!(
            "- {} ({}) {} -> {} reason={}",
            decision.skill_name,
            decision.skill_id,
            decision.current_status,
            decision.target_status,
            decision.reason
        ));
    }

    if apply {
        let applied = apply_skill_health_cleanup_decisions(memory, agent_id, &decisions).await?;
        lines.push(format!("Applied {} cleanup decisions.", applied.len()));
    } else {
        lines.push(
            "Dry run. Use `synapseclaw skills health --apply` or `/skills health --apply` to write these status changes."
                .into(),
        );
    }

    Ok(lines.join(
        "
",
    ))
}

pub async fn format_skill_patch_candidate_diff_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    candidate_ref: &str,
    limit: usize,
) -> Result<String> {
    let candidate = resolve_skill_patch_candidate_for_display(memory, candidate_ref, limit).await?;
    let target_skill = memory
        .list_skills(&agent_id.to_string(), 512)
        .await?
        .into_iter()
        .find(|skill| skill.id == candidate.target_skill_id);
    Ok(format_skill_patch_candidate_diff_text(
        &candidate,
        target_skill.as_ref(),
    ))
}

#[derive(Debug, Clone)]
pub struct SkillPatchApplyOutcome {
    pub agent_id: String,
    pub candidate_id: String,
    pub target_skill_id: String,
    pub skill_name: String,
    pub previous_version: u32,
    pub new_version: u32,
    pub rollback_skill_id: String,
    pub apply_report: SkillPatchApplyReport,
}

pub async fn apply_skill_patch_candidate(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    candidate_ref: &str,
    limit: usize,
) -> Result<SkillPatchApplyOutcome> {
    let candidate = resolve_skill_patch_candidate_for_display(memory, candidate_ref, limit).await?;
    let target_skill = memory
        .list_skills(&agent_id.to_string(), 512)
        .await?
        .into_iter()
        .find(|skill| skill.id == candidate.target_skill_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Target learned skill not found for agent {agent_id}: {}",
                candidate.target_skill_id
            )
        })?;

    let policy = SkillCandidateEvalPolicy::default();
    let apply_report = evaluate_skill_patch_for_apply(&candidate, &target_skill, &policy);
    if !apply_report.apply_allowed {
        bail!(
            "Skill patch candidate {} cannot be applied: {}",
            candidate.id,
            apply_report.reason
        );
    }

    let now = chrono::Utc::now();
    let rollback_skill = build_skill_patch_rollback_skill(&target_skill, &candidate, agent_id, now);
    let rollback_skill_id = rollback_skill.id.clone();
    memory.store_skill(rollback_skill).await?;

    memory
        .update_skill(
            &target_skill.id,
            SkillUpdate {
                increment_success: false,
                increment_fail: false,
                new_description: None,
                new_content: Some(candidate.proposed_body.clone()),
                new_task_family: None,
                new_tool_pattern: None,
                new_lineage_task_families: None,
                new_tags: None,
                new_status: Some(SkillStatus::Active),
            },
            &agent_id.to_string(),
        )
        .await?;

    let updated_skill = memory
        .list_skills(&agent_id.to_string(), 512)
        .await?
        .into_iter()
        .find(|skill| skill.id == target_skill.id)
        .unwrap_or_else(|| {
            let mut fallback = target_skill.clone();
            fallback.version = target_skill.version.saturating_add(1);
            fallback.content = candidate.proposed_body.clone();
            fallback.status = SkillStatus::Active;
            fallback
        });

    let mut applied_candidate = candidate.clone();
    applied_candidate.status = SkillStatus::Active;
    memory
        .store_episode(
            skill_patch_candidate_service::skill_patch_candidate_to_memory_entry(
                &applied_candidate,
                now,
            )?,
        )
        .await?;

    let apply_record = SkillPatchApplyRecord {
        id: format!(
            "{}:{}",
            stable_skill_id_component(&candidate.id),
            updated_skill.version
        ),
        candidate_id: candidate.id.clone(),
        target_skill_id: target_skill.id.clone(),
        agent_id: agent_id.to_string(),
        previous_version: target_skill.version,
        new_version: updated_skill.version,
        rollback_skill_id: rollback_skill_id.clone(),
        diff_summary: candidate.diff_summary.clone(),
        procedure_claims: candidate.procedure_claims.clone(),
        provenance: candidate.provenance.clone(),
        eval_reason: apply_report.reason.to_string(),
        applied_at_unix: now.timestamp(),
    };
    memory
        .store_episode(
            skill_patch_candidate_service::skill_patch_apply_to_memory_entry(&apply_record, now)?,
        )
        .await?;

    Ok(SkillPatchApplyOutcome {
        agent_id: agent_id.to_string(),
        candidate_id: candidate.id,
        target_skill_id: target_skill.id,
        skill_name: target_skill.name,
        previous_version: target_skill.version,
        new_version: updated_skill.version,
        rollback_skill_id,
        apply_report,
    })
}

pub async fn apply_skill_patch_candidate_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    candidate_ref: &str,
    limit: usize,
) -> Result<String> {
    let outcome = apply_skill_patch_candidate(memory, agent_id, candidate_ref, limit).await?;
    Ok(format_skill_patch_apply_outcome_text(&outcome))
}

#[derive(Debug, Clone)]
pub struct SkillPatchRollbackOutcome {
    pub agent_id: String,
    pub rollback_ref: String,
    pub apply_record_id: String,
    pub candidate_id: String,
    pub target_skill_id: String,
    pub skill_name: String,
    pub from_version: u32,
    pub restored_from_version: u32,
    pub new_version: u32,
    pub rollback_skill_id: String,
}

pub async fn format_skill_patch_versions_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_ref: Option<&str>,
    limit: usize,
) -> Result<String> {
    let skill_filter = match skill_ref.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw) => Some(resolve_skill_version_filter(memory, agent_id, raw).await?),
        None => None,
    };
    let apply_records = list_skill_patch_apply_records(memory, limit.max(1)).await?;
    let rollback_records = list_skill_patch_rollback_records(memory, limit.max(1)).await?;
    Ok(format_skill_patch_versions_text(
        agent_id,
        skill_filter.as_ref(),
        &apply_records,
        &rollback_records,
        limit.max(1),
    ))
}

pub async fn rollback_skill_patch(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    rollback_ref: &str,
    limit: usize,
) -> Result<SkillPatchRollbackOutcome> {
    let apply_record = resolve_skill_patch_apply_record(memory, rollback_ref, limit).await?;
    let target = memory
        .get_skill_by_id(&apply_record.target_skill_id, &agent_id.to_string())
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Target skill not found for rollback: {}",
                apply_record.target_skill_id
            )
        })?;
    if !matches!(target.origin, SkillOrigin::Learned | SkillOrigin::Manual) {
        bail!(
            "Refusing to rollback non-local skill '{}' with origin {}",
            target.name,
            target.origin
        );
    }
    if target.version != apply_record.new_version {
        bail!(
            "Refusing rollback for '{}': current version is v{}, apply record expects v{}",
            target.name,
            target.version,
            apply_record.new_version
        );
    }

    let rollback_skill = memory
        .get_skill_by_id(&apply_record.rollback_skill_id, &agent_id.to_string())
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Rollback snapshot not found: {}",
                apply_record.rollback_skill_id
            )
        })?;
    validate_rollback_snapshot(&rollback_skill, &apply_record)?;

    memory
        .update_skill(
            &target.id,
            SkillUpdate {
                increment_success: false,
                increment_fail: false,
                new_description: Some(rollback_skill.description.clone()),
                new_content: Some(rollback_skill.content.clone()),
                new_task_family: Some(rollback_skill.task_family.clone()),
                new_tool_pattern: Some(rollback_skill.tool_pattern.clone()),
                new_lineage_task_families: Some(rollback_skill.lineage_task_families.clone()),
                new_tags: Some(rollback_skill.tags.clone()),
                new_status: Some(SkillStatus::Active),
            },
            &agent_id.to_string(),
        )
        .await?;

    let updated_skill = memory
        .get_skill_by_id(&target.id, &agent_id.to_string())
        .await?
        .unwrap_or_else(|| {
            let mut fallback = target.clone();
            fallback.version = target.version.saturating_add(1);
            fallback.content = rollback_skill.content.clone();
            fallback.status = SkillStatus::Active;
            fallback
        });
    let now = chrono::Utc::now();
    let rollback_record = SkillPatchRollbackRecord {
        id: format!(
            "{}:{}",
            stable_skill_id_component(&apply_record.id),
            updated_skill.version
        ),
        apply_record_id: apply_record.id.clone(),
        candidate_id: apply_record.candidate_id.clone(),
        target_skill_id: apply_record.target_skill_id.clone(),
        agent_id: agent_id.to_string(),
        from_version: target.version,
        restored_from_version: apply_record.previous_version,
        new_version: updated_skill.version,
        rollback_skill_id: apply_record.rollback_skill_id.clone(),
        reason: "operator_rollback".to_string(),
        rolled_back_at_unix: now.timestamp(),
    };
    memory
        .store_episode(
            skill_patch_candidate_service::skill_patch_rollback_to_memory_entry(
                &rollback_record,
                now,
            )?,
        )
        .await?;

    Ok(SkillPatchRollbackOutcome {
        agent_id: agent_id.to_string(),
        rollback_ref: rollback_ref.to_string(),
        apply_record_id: apply_record.id,
        candidate_id: apply_record.candidate_id,
        target_skill_id: target.id,
        skill_name: target.name,
        from_version: target.version,
        restored_from_version: rollback_record.restored_from_version,
        new_version: updated_skill.version,
        rollback_skill_id: rollback_record.rollback_skill_id,
    })
}

pub async fn rollback_skill_patch_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    rollback_ref: &str,
    limit: usize,
) -> Result<String> {
    let outcome = rollback_skill_patch(memory, agent_id, rollback_ref, limit).await?;
    Ok(format_skill_patch_rollback_outcome_text(&outcome))
}

#[derive(Debug, Clone)]
pub struct UserAuthoredSkillCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub body: String,
    pub task_family: Option<String>,
    pub tool_pattern: Vec<String>,
    pub tags: Vec<String>,
    pub status: SkillStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserAuthoredSkillCreateOutcome {
    pub agent_id: String,
    pub skill_id: String,
    pub skill_name: String,
    pub status: SkillStatus,
    pub origin: SkillOrigin,
    pub version: u32,
    pub description: String,
    pub task_family: Option<String>,
    pub tool_pattern: Vec<String>,
    pub tags: Vec<String>,
    pub audit_files_scanned: usize,
}

pub async fn create_user_authored_skill(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    request: UserAuthoredSkillCreateRequest,
) -> Result<UserAuthoredSkillCreateOutcome> {
    let audit = audit::audit_skill_markdown_content(&request.body);
    if !audit.is_clean() {
        bail!("User skill audit failed: {}", audit.summary());
    }

    let input = UserAuthoredSkillInput {
        name: request.name,
        description: request.description,
        body: request.body,
        task_family: request.task_family,
        tool_pattern: request.tool_pattern,
        tags: request.tags,
        status: request.status,
    };
    let built = build_user_authored_skill(
        agent_id,
        input,
        &UserAuthoredSkillPolicy::default(),
        chrono::Utc::now(),
    )
    .map_err(|report| anyhow::anyhow!(format_user_skill_validation_error(&report)))?;

    let mut stored_skill = built.skill;
    let skill_id = memory.store_skill(stored_skill.clone()).await?;
    stored_skill.id = skill_id.clone();

    Ok(UserAuthoredSkillCreateOutcome {
        agent_id: agent_id.to_string(),
        skill_id,
        skill_name: stored_skill.name,
        status: stored_skill.status,
        origin: stored_skill.origin,
        version: stored_skill.version,
        description: stored_skill.description,
        task_family: stored_skill.task_family,
        tool_pattern: stored_skill.tool_pattern,
        tags: stored_skill.tags,
        audit_files_scanned: audit.files_scanned,
    })
}

pub async fn create_user_authored_skill_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    request: UserAuthoredSkillCreateRequest,
) -> Result<String> {
    let outcome = create_user_authored_skill(memory, agent_id, request).await?;
    Ok(format_user_authored_skill_create_outcome_text(&outcome))
}

#[derive(Debug, Clone, Default)]
pub struct UserSkillUpdateRequest {
    pub skill_ref: String,
    pub description: Option<String>,
    pub body: Option<String>,
    pub task_family: Option<String>,
    pub tool_pattern: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub status: Option<SkillStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserSkillUpdateOutcome {
    pub agent_id: String,
    pub skill_id: String,
    pub skill_name: String,
    pub origin: SkillOrigin,
    pub previous_version: u32,
    pub new_version: u32,
    pub previous_status: SkillStatus,
    pub new_status: SkillStatus,
    pub rollback_skill_id: String,
    pub apply_record_id: String,
    pub diff_summary: String,
    pub audit_files_scanned: usize,
}

pub async fn update_user_skill(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    request: UserSkillUpdateRequest,
) -> Result<UserSkillUpdateOutcome> {
    let target = resolve_skill_ref_including_id(memory, agent_id, &request.skill_ref).await?;
    if !matches!(target.origin, SkillOrigin::Manual | SkillOrigin::Learned) {
        bail!(
            "Refusing to update non-local skill '{}' with origin {}",
            target.name,
            target.origin
        );
    }
    if target.status == SkillStatus::Deprecated {
        bail!(
            "Refusing to update deprecated skill '{}'; roll back or recreate it first",
            target.name
        );
    }

    let audit_files_scanned = if let Some(body) = request.body.as_deref() {
        let audit = audit::audit_skill_markdown_content(body);
        if !audit.is_clean() {
            bail!("User skill audit failed: {}", audit.summary());
        }
        audit.files_scanned
    } else {
        0
    };

    let has_update = request.description.is_some()
        || request.body.is_some()
        || request.task_family.is_some()
        || request.tool_pattern.is_some()
        || request.tags.is_some()
        || request.status.is_some();
    if !has_update {
        bail!("No skill update fields provided.");
    }

    let now = chrono::Utc::now();
    let change_id = format!(
        "operator-update-{}-{}",
        stable_skill_id_component(&target.id),
        now.timestamp()
    );
    let rollback_skill =
        build_skill_rollback_snapshot(&target, &change_id, "operator-update", agent_id, now);
    let rollback_skill_id = rollback_skill.id.clone();
    memory.store_skill(rollback_skill).await?;

    let diff_summary = user_skill_update_diff_summary(&target, &request);
    memory
        .update_skill(
            &target.id,
            SkillUpdate {
                increment_success: false,
                increment_fail: false,
                new_description: request.description.clone(),
                new_content: request.body.clone(),
                new_task_family: request.task_family.clone().map(Some),
                new_tool_pattern: request.tool_pattern.clone(),
                new_lineage_task_families: None,
                new_tags: request.tags.clone(),
                new_status: request.status.clone(),
            },
            &agent_id.to_string(),
        )
        .await?;

    let updated_skill = memory
        .get_skill_by_id(&target.id, &agent_id.to_string())
        .await?
        .unwrap_or_else(|| {
            let mut fallback = target.clone();
            fallback.version = target.version.saturating_add(1);
            if let Some(description) = request.description.clone() {
                fallback.description = description;
            }
            if let Some(body) = request.body.clone() {
                fallback.content = body;
            }
            if let Some(task_family) = request.task_family.clone() {
                fallback.task_family = Some(task_family);
            }
            if let Some(tool_pattern) = request.tool_pattern.clone() {
                fallback.tool_pattern = tool_pattern;
            }
            if let Some(tags) = request.tags.clone() {
                fallback.tags = tags;
            }
            if let Some(status) = request.status.clone() {
                fallback.status = status;
            }
            fallback
        });

    let apply_record = SkillPatchApplyRecord {
        id: format!(
            "{}:{}",
            stable_skill_id_component(&change_id),
            updated_skill.version
        ),
        candidate_id: change_id,
        target_skill_id: target.id.clone(),
        agent_id: agent_id.to_string(),
        previous_version: target.version,
        new_version: updated_skill.version,
        rollback_skill_id: rollback_skill_id.clone(),
        diff_summary: diff_summary.clone(),
        procedure_claims: Vec::new(),
        provenance: Vec::new(),
        eval_reason: "operator_update".to_string(),
        applied_at_unix: now.timestamp(),
    };
    memory
        .store_episode(
            skill_patch_candidate_service::skill_patch_apply_to_memory_entry(&apply_record, now)?,
        )
        .await?;

    Ok(UserSkillUpdateOutcome {
        agent_id: agent_id.to_string(),
        skill_id: target.id,
        skill_name: target.name,
        origin: target.origin,
        previous_version: target.version,
        new_version: updated_skill.version,
        previous_status: target.status,
        new_status: updated_skill.status,
        rollback_skill_id,
        apply_record_id: apply_record.id,
        diff_summary,
        audit_files_scanned,
    })
}

pub async fn update_user_skill_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    request: UserSkillUpdateRequest,
) -> Result<String> {
    let outcome = update_user_skill(memory, agent_id, request).await?;
    Ok(format_user_skill_update_outcome_text(&outcome))
}

pub fn user_authored_skill_request_from_runtime_command(
    name: &str,
    body: &str,
    metadata: &RuntimeUserSkillCreateMetadata,
) -> UserAuthoredSkillCreateRequest {
    UserAuthoredSkillCreateRequest {
        name: name.to_string(),
        description: None,
        body: body.to_string(),
        task_family: metadata.task_family.clone(),
        tool_pattern: metadata.tool_pattern.clone(),
        tags: metadata.tags.clone(),
        status: SkillStatus::Active,
    }
}

#[derive(Debug, Clone)]
pub struct UserSkillPackageExportRequest {
    pub skill_ref: String,
    pub destination: Option<PathBuf>,
    pub package_name: Option<String>,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserSkillPackageExportOutcome {
    pub agent_id: String,
    pub skill_id: String,
    pub skill_name: String,
    pub origin: SkillOrigin,
    pub status: SkillStatus,
    pub version: u32,
    pub package_dir: PathBuf,
    pub skill_file: PathBuf,
    pub diff_summary: String,
    pub audit_files_scanned: usize,
}

#[derive(Debug, Clone, Serialize)]
struct MemorySkillPackageFrontmatter {
    name: String,
    description: String,
    version: String,
    author: String,
    tags: Vec<String>,
    origin: String,
    status: String,
    source_skill_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_family: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lineage_task_families: Vec<String>,
}

pub async fn export_user_skill_package(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    workspace_dir: &Path,
    request: UserSkillPackageExportRequest,
) -> Result<UserSkillPackageExportOutcome> {
    if request.destination.is_some() && request.package_name.is_some() {
        bail!("Use either --to or --name for skill export, not both.");
    }

    let skill = resolve_learned_skill_ref(memory, agent_id, &request.skill_ref).await?;
    if !matches!(skill.origin, SkillOrigin::Manual | SkillOrigin::Learned) {
        bail!(
            "Refusing to export non-local skill '{}' with origin {}",
            skill.name,
            skill.origin
        );
    }

    let package_dir = resolve_skill_export_destination(
        workspace_dir,
        &skill,
        &request.destination,
        &request.package_name,
    )?;
    let skill_file = package_dir.join("SKILL.md");
    if package_dir.exists() {
        let metadata = std::fs::symlink_metadata(&package_dir)
            .with_context(|| format!("failed to inspect {}", package_dir.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "Refusing to export into symlinked package directory: {}",
                package_dir.display()
            );
        }
        if !metadata.is_dir() {
            bail!(
                "Skill export destination must be a directory: {}",
                package_dir.display()
            );
        }
    }
    if skill_file.exists() && !request.overwrite {
        bail!(
            "Skill package already exists at {}; pass --overwrite to replace SKILL.md",
            skill_file.display()
        );
    }

    std::fs::create_dir_all(&package_dir)
        .with_context(|| format!("failed to create {}", package_dir.display()))?;
    let previous = if skill_file.exists() {
        Some(
            std::fs::read_to_string(&skill_file)
                .with_context(|| format!("failed to read {}", skill_file.display()))?,
        )
    } else {
        None
    };
    let rendered = render_memory_skill_package_markdown(&skill)?;
    let diff_summary = skill_package_export_diff_summary(previous.as_deref(), &rendered);
    std::fs::write(&skill_file, rendered)
        .with_context(|| format!("failed to write {}", skill_file.display()))?;

    let audit = match enforce_skill_security_audit(&package_dir) {
        Ok(report) => report,
        Err(error) => {
            if let Some(previous) = previous {
                let _ = std::fs::write(&skill_file, previous);
            } else {
                let _ = std::fs::remove_file(&skill_file);
            }
            return Err(error);
        }
    };

    Ok(UserSkillPackageExportOutcome {
        agent_id: agent_id.to_string(),
        skill_id: skill.id,
        skill_name: skill.name,
        origin: skill.origin,
        status: skill.status,
        version: skill.version,
        package_dir,
        skill_file,
        diff_summary,
        audit_files_scanned: audit.files_scanned,
    })
}

pub async fn export_user_skill_package_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    workspace_dir: &Path,
    request: UserSkillPackageExportRequest,
) -> Result<String> {
    let outcome = export_user_skill_package(memory, agent_id, workspace_dir, request).await?;
    Ok(format_user_skill_package_export_outcome_text(&outcome))
}

#[derive(Debug, Clone)]
pub struct SkillPackageScaffoldRequest {
    pub name: String,
    pub description: Option<String>,
    pub task_family: Option<String>,
    pub tool_pattern: Vec<String>,
    pub tags: Vec<String>,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillPackageScaffoldOutcome {
    pub package_dir: PathBuf,
    pub skill_file: PathBuf,
    pub references_dir: PathBuf,
    pub templates_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub audit_files_scanned: usize,
}

pub fn scaffold_skill_package(
    workspace_dir: &Path,
    request: SkillPackageScaffoldRequest,
) -> Result<SkillPackageScaffoldOutcome> {
    let package_name = validate_skill_package_dir_name(&request.name)?;
    let package_dir = skills_dir(workspace_dir).join(package_name);
    let skill_file = package_dir.join("SKILL.md");
    if skill_file.exists() && !request.overwrite {
        bail!(
            "Skill package already exists at {}; pass --overwrite to replace SKILL.md",
            skill_file.display()
        );
    }
    std::fs::create_dir_all(&package_dir)
        .with_context(|| format!("failed to create {}", package_dir.display()))?;
    let references_dir = package_dir.join("references");
    let templates_dir = package_dir.join("templates");
    let assets_dir = package_dir.join("assets");
    std::fs::create_dir_all(&references_dir)
        .with_context(|| format!("failed to create {}", references_dir.display()))?;
    std::fs::create_dir_all(&templates_dir)
        .with_context(|| format!("failed to create {}", templates_dir.display()))?;
    std::fs::create_dir_all(&assets_dir)
        .with_context(|| format!("failed to create {}", assets_dir.display()))?;

    let rendered = render_skill_package_scaffold_markdown(&request);
    std::fs::write(&skill_file, rendered)
        .with_context(|| format!("failed to write {}", skill_file.display()))?;
    let audit = enforce_skill_security_audit(&package_dir)?;
    Ok(SkillPackageScaffoldOutcome {
        package_dir,
        skill_file,
        references_dir,
        templates_dir,
        assets_dir,
        audit_files_scanned: audit.files_scanned,
    })
}

pub fn format_skill_package_scaffold_outcome_text(outcome: &SkillPackageScaffoldOutcome) -> String {
    format!(
        "Created skill package scaffold.\nPackage: {}\nSkill file: {}\nReferences: {}\nTemplates: {}\nAssets: {}\nAudit: passed ({} files scanned).",
        outcome.package_dir.display(),
        outcome.skill_file.display(),
        outcome.references_dir.display(),
        outcome.templates_dir.display(),
        outcome.assets_dir.display(),
        outcome.audit_files_scanned
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAutoPromotionEvaluation {
    pub candidate_id: String,
    pub target_skill_id: String,
    pub target_skill_name: Option<String>,
    pub target_found: bool,
    pub report: Option<SkillPatchAutoPromotionReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAutoPromotionAppliedPatch {
    pub candidate_id: String,
    pub target_skill_id: String,
    pub skill_name: String,
    pub previous_version: u32,
    pub new_version: u32,
    pub rollback_skill_id: String,
}

impl From<SkillPatchApplyOutcome> for SkillAutoPromotionAppliedPatch {
    fn from(outcome: SkillPatchApplyOutcome) -> Self {
        Self {
            candidate_id: outcome.candidate_id,
            target_skill_id: outcome.target_skill_id,
            skill_name: outcome.skill_name,
            previous_version: outcome.previous_version,
            new_version: outcome.new_version,
            rollback_skill_id: outcome.rollback_skill_id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAutoPromotionApplyError {
    pub candidate_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillAutoPromotionRun {
    pub agent_id: String,
    pub apply: bool,
    pub policy: SkillPatchAutoPromotionPolicy,
    pub evaluations: Vec<SkillAutoPromotionEvaluation>,
    pub applied_patches: Vec<SkillAutoPromotionAppliedPatch>,
    pub apply_errors: Vec<SkillAutoPromotionApplyError>,
}

pub fn skill_auto_promotion_policy_from_config(
    config: &synapse_domain::config::schema::SkillsAutoPromotionConfig,
) -> SkillPatchAutoPromotionPolicy {
    SkillPatchAutoPromotionPolicy {
        enabled: config.enabled,
        min_successful_live_traces: config.min_successful_live_traces.max(1),
        trace_window_limit: config.trace_window_limit.max(1),
        max_recent_blocking_traces: config.max_recent_blocking_traces,
    }
}

pub async fn run_skill_patch_auto_promotion(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    policy: &SkillPatchAutoPromotionPolicy,
    limit: usize,
    apply: bool,
) -> Result<SkillAutoPromotionRun> {
    let candidate_limit = limit.max(1);
    let candidates = list_queued_skill_patch_candidates(memory, candidate_limit).await?;
    let learned = memory.list_skills(&agent_id.to_string(), 512).await?;
    let targets = learned
        .into_iter()
        .map(|skill| (skill.id.clone(), skill))
        .collect::<HashMap<_, _>>();
    let trace_limit = candidate_limit
        .saturating_mul(policy.trace_window_limit.max(1))
        .clamp(1, 1_000);
    let traces = list_skill_use_traces(memory, agent_id, trace_limit).await?;
    let eval_policy = SkillCandidateEvalPolicy::default();
    let mut evaluations = Vec::with_capacity(candidates.len());
    let mut applied_patches = Vec::new();
    let mut apply_errors = Vec::new();

    for candidate in candidates {
        let Some(target) = targets.get(&candidate.target_skill_id) else {
            evaluations.push(SkillAutoPromotionEvaluation {
                candidate_id: candidate.id,
                target_skill_id: candidate.target_skill_id,
                target_skill_name: None,
                target_found: false,
                report: None,
            });
            continue;
        };

        let report = evaluate_skill_patch_for_auto_promotion(
            &candidate,
            target,
            &traces,
            policy,
            &eval_policy,
        );
        let should_apply = apply && report.auto_promotion_allowed;
        evaluations.push(SkillAutoPromotionEvaluation {
            candidate_id: candidate.id.clone(),
            target_skill_id: candidate.target_skill_id.clone(),
            target_skill_name: Some(target.name.clone()),
            target_found: true,
            report: Some(report),
        });

        if should_apply {
            match apply_skill_patch_candidate(memory, agent_id, &candidate.id, candidate_limit)
                .await
            {
                Ok(outcome) => applied_patches.push(outcome.into()),
                Err(error) => apply_errors.push(SkillAutoPromotionApplyError {
                    candidate_id: candidate.id,
                    error: error.to_string(),
                }),
            }
        }
    }

    Ok(SkillAutoPromotionRun {
        agent_id: agent_id.to_string(),
        apply,
        policy: policy.clone(),
        evaluations,
        applied_patches,
        apply_errors,
    })
}

pub async fn format_skill_auto_promotion_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    policy: &SkillPatchAutoPromotionPolicy,
    limit: usize,
    apply: bool,
) -> Result<String> {
    let run = run_skill_patch_auto_promotion(memory, agent_id, policy, limit, apply).await?;
    Ok(format_skill_auto_promotion_run_text(&run))
}

pub async fn format_learned_skill_review_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    limit: usize,
    apply: bool,
) -> Result<String> {
    let skills = memory.list_skills(&agent_id.to_string(), limit).await?;
    let learned = skills
        .into_iter()
        .filter(|skill| skill.origin == synapse_domain::domain::memory::SkillOrigin::Learned)
        .collect::<Vec<_>>();
    let decisions = review_learned_skills(&learned, &[]);

    if decisions.is_empty() {
        return Ok(format!(
            "No learned skill review actions for agent {agent_id}."
        ));
    }

    let mut lines = vec![format!(
        "Learned skill review for agent {agent_id} ({}):",
        decisions.len()
    )];
    for decision in &decisions {
        lines.extend(format_skill_review_decision_lines(decision));
    }

    if apply {
        for decision in &decisions {
            memory
                .update_skill(
                    &decision.skill_id,
                    empty_status_update(status_from_review_action(&decision.action)),
                    &agent_id.to_string(),
                )
                .await?;
        }
        lines.push(format!("Applied {} review decisions.", decisions.len()));
    } else {
        lines.push("Dry run. Use `/skills review --apply` to write these status changes.".into());
    }
    Ok(lines.join(
        "
",
    ))
}

pub async fn update_learned_skill_status_output(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_ref: &str,
    target_status: SkillStatus,
) -> Result<String> {
    let skill = resolve_learned_skill_ref(memory, agent_id, skill_ref).await?;

    if !matches!(skill.origin, SkillOrigin::Learned | SkillOrigin::Manual) {
        bail!(
            "Refusing to mutate non-local skill '{}' with origin {}",
            skill.name,
            skill.origin
        );
    }

    memory
        .update_skill(
            &skill.id,
            empty_status_update(target_status.clone()),
            &agent_id.to_string(),
        )
        .await?;

    Ok(format!(
        "Updated memory-backed skill '{}' ({}) for agent {agent_id}: {} -> {}",
        skill.name, skill.id, skill.status, target_status
    ))
}

fn format_skill_review_decision_lines(decision: &SkillReviewDecision) -> Vec<String> {
    let mut lines = vec![format!(
        "- {} [{} -> {}]: {}",
        decision.skill_name,
        skill_review_action_name(&decision.action),
        decision.target_status,
        decision.reason
    )];
    lines.push(format!("  id: {}", decision.skill_id));
    if !decision.lineage_task_families.is_empty() {
        lines.push(format!(
            "  lineage: {}",
            decision.lineage_task_families.join(", ")
        ));
    }
    lines
}

fn resolve_skill_agent(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
) -> String {
    agent
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| crate::agent::resolve_agent_id(config))
}

enum SkillAccess {
    Direct {
        memory: std::sync::Arc<dyn synapse_memory::UnifiedMemoryPort>,
        agent_id: String,
    },
    Gateway(GatewaySkillClient),
}

struct GatewaySkillClient {
    client: reqwest::Client,
    base_url: String,
    token: String,
    agent_id: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillListResponse {
    agent_id: String,
    #[serde(default)]
    skills: Vec<GatewaySkillProjection>,
    #[serde(default)]
    patch_candidates: Vec<GatewaySkillPatchCandidateProjection>,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillProjection {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    task_family: Option<String>,
    #[serde(default)]
    lineage_task_families: Vec<String>,
    #[serde(default)]
    tool_pattern: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    success_count: u32,
    #[serde(default)]
    fail_count: u32,
    #[serde(default)]
    version: u32,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillPatchCandidateProjection {
    id: String,
    target_skill_id: String,
    target_version: u32,
    diff_summary: String,
    #[serde(default)]
    procedure_claims: Vec<GatewaySkillPatchProcedureClaim>,
    #[serde(default)]
    replay_criteria: Vec<String>,
    #[serde(default)]
    eval_results: Vec<serde_json::Value>,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillPatchProcedureClaim {
    tool_name: String,
    failure_kind: String,
    suggested_action: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillUseTraceProjection {
    id: String,
    skill_id: String,
    #[serde(default)]
    task_family: Option<String>,
    #[serde(default)]
    route_model: Option<String>,
    #[serde(default)]
    tool_pattern: Vec<String>,
    outcome: String,
    #[serde(default)]
    verification: Option<String>,
    #[serde(default)]
    repair_evidence_count: usize,
    observed_at_unix: i64,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillUseTraceResponse {
    agent_id: String,
    #[serde(default)]
    traces: Vec<GatewaySkillUseTraceProjection>,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillHealthResponse {
    agent_id: String,
    report: SkillHealthReport,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillHealthApplyResponse {
    agent_id: String,
    output: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillPatchDiffResponse {
    agent_id: String,
    output: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillReviewResponse {
    agent_id: String,
    #[serde(default)]
    decisions: Vec<GatewaySkillReviewDecision>,
    #[serde(default)]
    applied_decisions: Vec<GatewayAppliedSkillDecision>,
    #[serde(default)]
    decision_count: usize,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillReviewDecision {
    skill_id: String,
    skill_name: String,
    #[serde(default)]
    lineage_task_families: Vec<String>,
    action: String,
    target_status: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct GatewayAppliedSkillDecision {
    skill_id: String,
    skill_name: String,
    target_status: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct GatewaySkillStatusUpdateResponse {
    agent_id: String,
    skill_id: String,
    skill_name: String,
    previous_status: String,
    target_status: String,
}

async fn create_skill_access(
    config: &synapse_domain::config::schema::Config,
    agent_id: &str,
) -> Result<SkillAccess> {
    if let Ok(gateway) = create_gateway_skill_client(config, agent_id) {
        if gateway.is_available().await {
            return Ok(SkillAccess::Gateway(gateway));
        }
    }

    match synapse_memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        agent_id,
        config.api_key.as_deref(),
    )
    .await
    {
        Ok(backend)
            if backend.surreal.is_some() || config.memory.backend.eq_ignore_ascii_case("none") =>
        {
            Ok(SkillAccess::Direct {
                memory: backend.memory,
                agent_id: agent_id.to_string(),
            })
        }
        Ok(_) | Err(_) => create_gateway_skill_access(config, agent_id),
    }
}

fn create_gateway_skill_access(
    config: &synapse_domain::config::schema::Config,
    agent_id: &str,
) -> Result<SkillAccess> {
    Ok(SkillAccess::Gateway(create_gateway_skill_client(
        config, agent_id,
    )?))
}

fn create_gateway_skill_client(
    config: &synapse_domain::config::schema::Config,
    agent_id: &str,
) -> Result<GatewaySkillClient> {
    let Some((base_url, token)) = skill_gateway_credentials(config) else {
        bail!(
            "skill memory backend is unavailable for direct CLI access and no authenticated gateway fallback is configured; \
             if the daemon is running, set agents_ipc.gateway_url and agents_ipc.proxy_token or stop the daemon for direct access"
        );
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to create skills gateway client")?;

    Ok(GatewaySkillClient {
        client,
        base_url,
        token,
        agent_id: agent_id.to_string(),
    })
}

fn skill_gateway_credentials(
    config: &synapse_domain::config::schema::Config,
) -> Option<(String, String)> {
    let token = config
        .agents_ipc
        .proxy_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let base_url = config
        .agents_ipc
        .gateway_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_gateway_url(config));
    Some((base_url.trim_end_matches('/').to_string(), token))
}

fn default_gateway_url(config: &synapse_domain::config::schema::Config) -> String {
    let host = match config.gateway.host.trim() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
        value if value.contains(':') && !value.starts_with('[') => format!("[{value}]"),
        value => value.to_string(),
    };
    format!("http://{}:{}", host, config.gateway.port)
}

fn gateway_response_agent<'a>(
    gateway: &'a GatewaySkillClient,
    response_agent: &'a str,
) -> Result<&'a str> {
    let response_agent = response_agent.trim();
    if response_agent.is_empty() {
        return Ok(gateway.agent_id.as_str());
    }
    if response_agent != gateway.agent_id {
        bail!(
            "skills gateway returned agent {response_agent}, but CLI requested {}; use that agent's config or a broker proxy route",
            gateway.agent_id
        );
    }
    Ok(response_agent)
}

impl GatewaySkillClient {
    async fn is_available(&self) -> bool {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        self.client
            .get(url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    async fn get_json<T>(&self, path: &str, limit: usize) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = self.url(path);
        let response = self
            .client
            .get(&url)
            .query(&[("limit", limit.to_string())])
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("failed to reach skills gateway at {url}"))?;
        decode_gateway_response(response, path).await
    }

    async fn get_json_query<T>(&self, path: &str, query: &[(&str, String)]) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = self.url(path);
        let response = self
            .client
            .get(&url)
            .query(query)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("failed to reach skills gateway at {url}"))?;
        decode_gateway_response(response, path).await
    }

    async fn post_json<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let url = self.url(path);
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to reach skills gateway at {url}"))?;
        decode_gateway_response(response, path).await
    }
}

async fn decode_gateway_response<T>(response: reqwest::Response, path: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| truncate_for_cli(&body, 240));
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!(
                "skills gateway route {path} is unavailable (HTTP 404); rebuild/install/restart the daemon before using CLI gateway fallback"
            );
        }
        bail!("skills gateway route {path} returned HTTP {status}: {detail}");
    }

    match serde_json::from_str(&body) {
        Ok(value) => Ok(value),
        Err(error) => {
            let trimmed = body.trim_start();
            if trimmed.starts_with('<') || trimmed.starts_with("<!DOCTYPE") {
                bail!(
                    "skills gateway route {path} did not return JSON; the running daemon likely needs rebuild/install/restart for Slice 5 routes"
                );
            }
            Err(error).with_context(|| format!("skills gateway route {path} returned invalid JSON"))
        }
    }
}

fn empty_status_update(status: SkillStatus) -> SkillUpdate {
    SkillUpdate {
        increment_success: false,
        increment_fail: false,
        new_description: None,
        new_content: None,
        new_task_family: None,
        new_tool_pattern: None,
        new_lineage_task_families: None,
        new_tags: None,
        new_status: Some(status),
    }
}

fn print_learned_skill(skill: &MemorySkill) {
    println!(
        "  {} [{} / {} / {}] v{} successes={} failures={}",
        console::style(&skill.name).white().bold(),
        skill.origin,
        skill.status,
        memory_skill_source_lane(skill),
        skill.version,
        skill.success_count,
        skill.fail_count
    );
    if !skill.id.trim().is_empty() {
        println!("    id: {}", skill.id);
    }
    if let Some(task_family) = skill
        .task_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        println!("    task_family: {task_family}");
    }
    if !skill.lineage_task_families.is_empty() {
        println!("    lineage: {}", skill.lineage_task_families.join(", "));
    }
    if !skill.tool_pattern.is_empty() {
        println!("    tools: {}", skill.tool_pattern.join(" -> "));
    }
    if !skill.description.trim().is_empty() {
        println!("    {}", truncate_for_cli(&skill.description, 140));
    }
}

fn print_gateway_learned_skill(skill: &GatewaySkillProjection) {
    println!(
        "  {} [{} / {} / {}] v{} successes={} failures={}",
        console::style(&skill.name).white().bold(),
        value_or_unknown(&skill.origin),
        value_or_unknown(&skill.status),
        gateway_skill_source_lane(skill),
        skill.version,
        skill.success_count,
        skill.fail_count
    );
    if !skill.id.trim().is_empty() {
        println!("    id: {}", skill.id);
    }
    if let Some(task_family) = skill
        .task_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        println!("    task_family: {task_family}");
    }
    if !skill.lineage_task_families.is_empty() {
        println!("    lineage: {}", skill.lineage_task_families.join(", "));
    }
    if !skill.tool_pattern.is_empty() {
        println!("    tools: {}", skill.tool_pattern.join(" -> "));
    }
    if !skill.description.trim().is_empty() {
        println!("    {}", truncate_for_cli(&skill.description, 140));
    }
}

fn memory_skill_source_lane(skill: &MemorySkill) -> &'static str {
    if skill.tags.iter().any(|tag| tag == "rollback") {
        "rollback-snapshot"
    } else if skill.tags.iter().any(|tag| tag == "ported-skill-package") {
        "ported-package"
    } else if skill.origin == SkillOrigin::Manual {
        "manual-memory"
    } else if skill.origin == SkillOrigin::Learned {
        "learned"
    } else if skill.origin == SkillOrigin::Imported {
        "imported-package"
    } else {
        "skill"
    }
}

fn gateway_skill_source_lane(skill: &GatewaySkillProjection) -> &'static str {
    if skill.tags.iter().any(|tag| tag == "rollback") {
        "rollback-snapshot"
    } else if skill.tags.iter().any(|tag| tag == "ported-skill-package") {
        "ported-package"
    } else {
        match skill.origin.as_str() {
            "manual" => "manual-memory",
            "learned" => "learned",
            "imported" => "imported-package",
            _ => "skill",
        }
    }
}

fn skill_use_outcome_name(outcome: &SkillUseOutcome) -> &'static str {
    match outcome {
        SkillUseOutcome::Succeeded => "succeeded",
        SkillUseOutcome::Failed => "failed",
        SkillUseOutcome::Repaired => "repaired",
    }
}

fn skill_health_severity_name(severity: SkillHealthSeverity) -> &'static str {
    match severity {
        SkillHealthSeverity::Healthy => "healthy",
        SkillHealthSeverity::Watch => "watch",
        SkillHealthSeverity::Review => "review",
        SkillHealthSeverity::Deprecated => "deprecated",
    }
}

fn format_skill_health_report_text(report: &SkillHealthReport) -> String {
    if report.items.is_empty() {
        return format!("No skills found for agent {}.", report.agent_id);
    }

    let mut lines = vec![
        format!(
            "Skill health for agent {}: total={}, healthy={}, watch={}, review={}, deprecated={} ({} use traces inspected)",
            report.agent_id,
            report.summary.total,
            report.summary.healthy,
            report.summary.watch,
            report.summary.review,
            report.summary.deprecated,
            report.inspected_traces
        ),
        format!(
            "Typed evidence: activations={} rollbacks={}",
            report.inspected_activation_traces, report.inspected_rollbacks
        ),
        String::new(),
    ];
    for item in &report.items {
        lines.extend(format_skill_health_item_lines(item));
    }
    lines.join(
        "
",
    )
}

fn format_skill_health_item_lines(item: &SkillHealthItem) -> Vec<String> {
    let mut lines = vec![format!(
        "- {} ({}) [{}/{} v{}] severity={} recommendation={}",
        item.name,
        item.skill_id,
        item.origin,
        item.status,
        item.version,
        skill_health_severity_name(item.severity),
        item.recommendation.as_str()
    )];
    lines.push(format!(
        "  counters: success={} fail={} traces={} ok={} failed={} repaired={}",
        item.success_count,
        item.fail_count,
        item.usage.trace_total,
        item.usage.trace_succeeded,
        item.usage.trace_failed,
        item.usage.trace_repaired
    ));
    lines.push(format!(
        "  utility: selected={} read={} helped={} failed={} repaired={} blocked={} rollbacks={}",
        item.utility.selected_count,
        item.utility.read_count,
        item.utility.helped_count,
        item.utility.failed_count,
        item.utility.repaired_count,
        item.utility.blocked_count,
        item.utility.rollback_count
    ));
    if let Some(last_used_at) = item.usage.last_used_at_unix {
        lines.push(format!("  last_used_at: {last_used_at}"));
    }
    if !item.signals.is_empty() {
        lines.push(format!(
            "  signals: {}",
            item.signals
                .iter()
                .map(|signal| signal.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(action) = &item.review_action {
        let reason = item.review_reason.as_deref().unwrap_or("unknown");
        lines.push(format!(
            "  review: action={} reason={reason}",
            skill_review_action_name(action)
        ));
    }
    lines
}

fn format_skill_use_trace_lines(trace: &SkillUseTrace) -> Vec<String> {
    let mut lines = vec![format!(
        "- {} [{}] skill={} observed_at={}",
        trace.id,
        skill_use_outcome_name(&trace.outcome),
        trace.skill_id,
        trace.observed_at_unix
    )];
    if let Some(task_family) = trace
        .task_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("  task_family: {task_family}"));
    }
    if let Some(route_model) = trace
        .route_model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("  route_model: {route_model}"));
    }
    if !trace.tool_pattern.is_empty() {
        lines.push(format!("  tools: {}", trace.tool_pattern.join(" -> ")));
    }
    if let Some(verification) = trace
        .verification
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!(
            "  verification: {}",
            truncate_for_cli(verification, 180)
        ));
    }
    if !trace.repair_evidence.is_empty() {
        lines.push(format!(
            "  repair_evidence_count: {}",
            trace.repair_evidence.len()
        ));
    }
    lines
}

fn print_skill_use_trace(trace: &SkillUseTrace) {
    for line in format_skill_use_trace_lines(trace) {
        println!("  {line}");
    }
}

fn print_gateway_skill_use_trace(trace: &GatewaySkillUseTraceProjection) {
    println!(
        "  - {} [{}] skill={} observed_at={}",
        trace.id,
        value_or_unknown(&trace.outcome),
        trace.skill_id,
        trace.observed_at_unix
    );
    if let Some(task_family) = trace
        .task_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        println!("    task_family: {task_family}");
    }
    if let Some(route_model) = trace
        .route_model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        println!("    route_model: {route_model}");
    }
    if !trace.tool_pattern.is_empty() {
        println!("    tools: {}", trace.tool_pattern.join(" -> "));
    }
    if let Some(verification) = trace
        .verification
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        println!("    verification: {}", truncate_for_cli(verification, 180));
    }
    if trace.repair_evidence_count > 0 {
        println!("    repair_evidence_count: {}", trace.repair_evidence_count);
    }
}

fn print_skill_patch_candidate(candidate: &SkillPatchCandidate) {
    println!(
        "  {} [{}] target={}@v{}",
        console::style(&candidate.id).white().bold(),
        candidate.status,
        candidate.target_skill_id,
        candidate.target_version
    );
    if !candidate.diff_summary.trim().is_empty() {
        println!("    {}", truncate_for_cli(&candidate.diff_summary, 180));
    }
    if !candidate.procedure_claims.is_empty() {
        println!(
            "    claims: {}",
            candidate
                .procedure_claims
                .iter()
                .map(format_skill_patch_procedure_claim)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    if !candidate.replay_criteria.is_empty() {
        println!("    replay: {}", candidate.replay_criteria.join("; "));
    }
    if !candidate.eval_results.is_empty() {
        let (passed, failed, missing) = replay_eval_counts(candidate.eval_results.iter().map(
            |result| match result.status {
                SkillReplayEvalStatus::Passed => "passed",
                SkillReplayEvalStatus::Failed => "failed",
                SkillReplayEvalStatus::Missing => "missing",
            },
        ));
        println!("    eval: passed={passed} failed={failed} missing={missing}");
    }
}

fn print_gateway_skill_patch_candidate(candidate: &GatewaySkillPatchCandidateProjection) {
    println!(
        "  {} [{}] target={}@v{}",
        console::style(&candidate.id).white().bold(),
        value_or_unknown(&candidate.status),
        candidate.target_skill_id,
        candidate.target_version
    );
    if !candidate.diff_summary.trim().is_empty() {
        println!("    {}", truncate_for_cli(&candidate.diff_summary, 180));
    }
    if !candidate.procedure_claims.is_empty() {
        println!(
            "    claims: {}",
            candidate
                .procedure_claims
                .iter()
                .map(format_gateway_skill_patch_procedure_claim)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    if !candidate.replay_criteria.is_empty() {
        println!("    replay: {}", candidate.replay_criteria.join("; "));
    }
    if !candidate.eval_results.is_empty() {
        let (passed, failed, missing) = replay_eval_counts(
            candidate
                .eval_results
                .iter()
                .filter_map(|result| result.get("status").and_then(serde_json::Value::as_str)),
        );
        println!("    eval: passed={passed} failed={failed} missing={missing}");
    }
}

fn format_skill_patch_candidate_diff_text(
    candidate: &SkillPatchCandidate,
    target_skill: Option<&MemorySkill>,
) -> String {
    let mut lines = vec![
        format!("Skill patch diff for {}:", candidate.id),
        format!(
            "  target: {}@v{}",
            candidate.target_skill_id, candidate.target_version
        ),
        format!("  status: {}", candidate.status),
    ];
    if !candidate.diff_summary.trim().is_empty() {
        lines.push(format!(
            "  summary: {}",
            truncate_for_cli(&candidate.diff_summary, 220)
        ));
    }
    if !candidate.procedure_claims.is_empty() {
        lines.push(format!(
            "  claims: {}",
            candidate
                .procedure_claims
                .iter()
                .map(format_skill_patch_procedure_claim)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !candidate.replay_criteria.is_empty() {
        lines.push(format!(
            "  replay: {}",
            candidate.replay_criteria.join("; ")
        ));
    }
    if !candidate.eval_results.is_empty() {
        let (passed, failed, missing) = replay_eval_counts(candidate.eval_results.iter().map(
            |result| match result.status {
                SkillReplayEvalStatus::Passed => "passed",
                SkillReplayEvalStatus::Failed => "failed",
                SkillReplayEvalStatus::Missing => "missing",
            },
        ));
        lines.push(format!(
            "  eval: passed={passed} failed={failed} missing={missing}"
        ));
    }

    match target_skill {
        Some(skill) => {
            lines.push(format!(
                "  current: {} [{} / {}] v{}",
                skill.name, skill.origin, skill.status, skill.version
            ));
            if skill.version != candidate.target_version {
                lines.push(format!(
                    "  warning: current skill version is v{}, candidate targets v{}",
                    skill.version, candidate.target_version
                ));
            }
            lines.push(format!(
                "  body_chars: current={} proposed={}",
                skill.content.chars().count(),
                candidate.proposed_body.chars().count()
            ));
            lines.extend(skill_patch_body_delta_lines(
                &skill.content,
                &candidate.proposed_body,
                12,
            ));
        }
        None => {
            lines.push("  current: target skill not found in learned-skill memory".into());
            lines.push(format!(
                "  proposed_body_chars: {}",
                candidate.proposed_body.chars().count()
            ));
        }
    }

    lines.join("\n")
}

fn format_skill_patch_apply_outcome_text(outcome: &SkillPatchApplyOutcome) -> String {
    format!(
        "Applied skill patch candidate {} to '{}' ({}) for agent {}: v{} -> v{}\nRollback snapshot: {}",
        outcome.candidate_id,
        outcome.skill_name,
        outcome.target_skill_id,
        outcome.agent_id,
        outcome.previous_version,
        outcome.new_version,
        outcome.rollback_skill_id
    )
}

fn format_skill_patch_rollback_outcome_text(outcome: &SkillPatchRollbackOutcome) -> String {
    format!(
        "Rolled back skill patch {} for '{}' ({}) on agent {}: v{} -> rollback v{} -> current v{}\nApply record: {}\nRollback snapshot: {}",
        outcome.rollback_ref,
        outcome.skill_name,
        outcome.target_skill_id,
        outcome.agent_id,
        outcome.from_version,
        outcome.restored_from_version,
        outcome.new_version,
        outcome.apply_record_id,
        outcome.rollback_skill_id
    )
}

pub fn format_user_authored_skill_create_outcome_text(
    outcome: &UserAuthoredSkillCreateOutcome,
) -> String {
    let mut lines = vec![format!(
        "Created user-authored skill '{}' ({}) for agent {}: origin={} status={} v{}",
        outcome.skill_name,
        outcome.skill_id,
        outcome.agent_id,
        outcome.origin,
        outcome.status,
        outcome.version
    )];
    if !outcome.description.trim().is_empty() {
        lines.push(format!(
            "Description: {}",
            truncate_for_cli(&outcome.description, 180)
        ));
    }
    if let Some(task_family) = outcome
        .task_family
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Task family: {task_family}"));
    }
    if !outcome.tool_pattern.is_empty() {
        lines.push(format!("Tool hints: {}", outcome.tool_pattern.join(" -> ")));
    }
    if !outcome.tags.is_empty() {
        lines.push(format!("Tags: {}", outcome.tags.join(", ")));
    }
    lines.push(format!(
        "Audit: passed ({} inline markdown item scanned).",
        outcome.audit_files_scanned
    ));
    lines.join("\n")
}

pub fn format_user_skill_update_outcome_text(outcome: &UserSkillUpdateOutcome) -> String {
    let mut lines = vec![format!(
        "Updated skill '{}' ({}) for agent {}: origin={} v{}->v{} status {}->{}",
        outcome.skill_name,
        outcome.skill_id,
        outcome.agent_id,
        outcome.origin,
        outcome.previous_version,
        outcome.new_version,
        outcome.previous_status,
        outcome.new_status
    )];
    if !outcome.diff_summary.trim().is_empty() {
        lines.push(format!(
            "Summary: {}",
            truncate_for_cli(&outcome.diff_summary, 220)
        ));
    }
    lines.push(format!("Version record: {}", outcome.apply_record_id));
    lines.push(format!("Rollback snapshot: {}", outcome.rollback_skill_id));
    if outcome.audit_files_scanned > 0 {
        lines.push(format!(
            "Audit: passed ({} inline markdown item scanned).",
            outcome.audit_files_scanned
        ));
    }
    lines.join("\n")
}

pub fn format_user_skill_package_export_outcome_text(
    outcome: &UserSkillPackageExportOutcome,
) -> String {
    format!(
        "Exported skill package '{}' ({}) for agent {}: origin={} status={} v{}\nPackage: {}\nSkill file: {}\nDiff: {}\nAudit: passed ({} files scanned).",
        outcome.skill_name,
        outcome.skill_id,
        outcome.agent_id,
        outcome.origin,
        outcome.status,
        outcome.version,
        outcome.package_dir.display(),
        outcome.skill_file.display(),
        outcome.diff_summary,
        outcome.audit_files_scanned
    )
}

fn format_user_skill_validation_error(report: &UserAuthoredSkillValidationReport) -> String {
    if report.findings.is_empty() {
        "User skill validation failed".to_string()
    } else {
        format!("User skill validation failed: {}", report.summary())
    }
}

fn user_skill_update_diff_summary(
    target: &MemorySkill,
    request: &UserSkillUpdateRequest,
) -> String {
    let mut changes = Vec::new();
    if request.description.is_some() {
        changes.push("description".to_string());
    }
    if let Some(body) = request.body.as_deref() {
        let delta = line_multiset_delta(body, &target.content, 4);
        let removed = line_multiset_delta(&target.content, body, 4);
        changes.push(format!("body +{} -{}", delta.total, removed.total));
    }
    if request.task_family.is_some() {
        changes.push("task_family".to_string());
    }
    if request.tool_pattern.is_some() {
        changes.push("tool_pattern".to_string());
    }
    if request.tags.is_some() {
        changes.push("tags".to_string());
    }
    if let Some(status) = request.status.as_ref() {
        changes.push(format!("status {}->{status}", target.status));
    }
    if changes.is_empty() {
        "operator update".to_string()
    } else {
        format!("operator update: {}", changes.join(", "))
    }
}

pub fn format_skill_auto_promotion_run_text(run: &SkillAutoPromotionRun) -> String {
    let mode = if run.apply { "apply" } else { "dry-run" };
    let mut lines = vec![
        format!("Skill patch auto-promotion for agent {} ({mode}):", run.agent_id),
        format!(
            "Policy: enabled={} min_successful_live_traces={} trace_window_limit={} max_recent_blocking_traces={}",
            run.policy.enabled,
            run.policy.min_successful_live_traces,
            run.policy.trace_window_limit,
            run.policy.max_recent_blocking_traces
        ),
    ];

    if run.evaluations.is_empty() {
        lines.push("No generated skill patch candidates found.".into());
    } else {
        for evaluation in &run.evaluations {
            match &evaluation.report {
                Some(report) => {
                    let state = if report.auto_promotion_allowed {
                        "eligible"
                    } else {
                        "blocked"
                    };
                    let target_name = evaluation
                        .target_skill_name
                        .as_deref()
                        .unwrap_or(&evaluation.target_skill_id);
                    lines.push(format!(
                        "- {} -> {}: {} ({})",
                        evaluation.candidate_id,
                        target_name,
                        state,
                        report.reason.as_str()
                    ));
                    lines.push(format!(
                        "  apply_gate={} live_success={}/{} blocking={}/{} considered={}",
                        report.apply_report.reason,
                        report.successful_trace_count,
                        report.required_successful_trace_count,
                        report.blocking_trace_count,
                        report.max_allowed_blocking_trace_count,
                        report.considered_trace_count
                    ));
                }
                None => {
                    lines.push(format!(
                        "- {} -> {}: blocked (target_skill_missing)",
                        evaluation.candidate_id, evaluation.target_skill_id
                    ));
                }
            }
        }
    }

    if run.apply {
        if !run.policy.enabled {
            lines.push(
                "No patches applied because [skills.auto_promotion].enabled is false.".into(),
            );
        } else if run.applied_patches.is_empty() {
            lines.push("No eligible patches were applied.".into());
        } else {
            lines.push(format!("Applied {} patch(es):", run.applied_patches.len()));
            for patch in &run.applied_patches {
                lines.push(format!(
                    "- {} -> '{}' ({}): v{} -> v{}; rollback={}",
                    patch.candidate_id,
                    patch.skill_name,
                    patch.target_skill_id,
                    patch.previous_version,
                    patch.new_version,
                    patch.rollback_skill_id
                ));
            }
        }
        for error in &run.apply_errors {
            lines.push(format!(
                "- apply_error {}: {}",
                error.candidate_id, error.error
            ));
        }
    } else if run.policy.enabled {
        lines.push(
            "Dry run. Use `skills autopromote --apply` or `/skills autopromote --apply` to apply eligible patches."
                .into(),
        );
    } else {
        lines.push("Dry run. Set `[skills.auto_promotion].enabled=true` and use `skills autopromote --apply` or `/skills autopromote --apply` to apply eligible patches.".into());
    }

    lines.join(
        "
",
    )
}

#[derive(Debug, Clone)]
struct SkillVersionFilter {
    skill_id: String,
    skill_name: Option<String>,
}

fn format_skill_patch_versions_text(
    agent_id: &str,
    filter: Option<&SkillVersionFilter>,
    apply_records: &[SkillPatchApplyRecord],
    rollback_records: &[SkillPatchRollbackRecord],
    limit: usize,
) -> String {
    let mut applies = apply_records
        .iter()
        .filter(|record| {
            filter
                .map(|filter| record.target_skill_id == filter.skill_id)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    applies.sort_by(|left, right| {
        right
            .applied_at_unix
            .cmp(&left.applied_at_unix)
            .then_with(|| right.id.cmp(&left.id))
    });
    applies.truncate(limit);

    let mut rollbacks = rollback_records
        .iter()
        .filter(|record| {
            filter
                .map(|filter| record.target_skill_id == filter.skill_id)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    rollbacks.sort_by(|left, right| {
        right
            .rolled_back_at_unix
            .cmp(&left.rolled_back_at_unix)
            .then_with(|| right.id.cmp(&left.id))
    });
    rollbacks.truncate(limit);

    if applies.is_empty() && rollbacks.is_empty() {
        return match filter {
            Some(filter) => format!(
                "No skill version records found for agent {agent_id}, skill {}.",
                filter
                    .skill_name
                    .as_deref()
                    .unwrap_or(filter.skill_id.as_str())
            ),
            None => format!("No skill version records found for agent {agent_id}."),
        };
    }

    let title = match filter {
        Some(filter) => format!(
            "Skill versions for agent {agent_id}, skill {} ({}):",
            filter
                .skill_name
                .as_deref()
                .unwrap_or(filter.skill_id.as_str()),
            filter.skill_id
        ),
        None => format!("Skill versions for agent {agent_id}:"),
    };
    let mut lines = vec![title];

    if !applies.is_empty() {
        lines.push("Applied changes:".into());
        for record in applies {
            lines.push(format!(
                "- {} candidate={} target={} v{}->v{} rollback={} at={}",
                record.id,
                record.candidate_id,
                record.target_skill_id,
                record.previous_version,
                record.new_version,
                record.rollback_skill_id,
                record.applied_at_unix
            ));
            if !record.diff_summary.trim().is_empty() {
                lines.push(format!(
                    "  summary: {}",
                    truncate_for_cli(&record.diff_summary, 180)
                ));
            }
            if !record.procedure_claims.is_empty() {
                lines.push(format!(
                    "  claims: {}",
                    record
                        .procedure_claims
                        .iter()
                        .map(format_skill_patch_procedure_claim)
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
        }
    }

    if !rollbacks.is_empty() {
        lines.push("Rollbacks:".into());
        for record in rollbacks {
            lines.push(format!(
                "- {} apply={} target={} v{}->v{} restored_from=v{} snapshot={} at={}",
                record.id,
                record.apply_record_id,
                record.target_skill_id,
                record.from_version,
                record.new_version,
                record.restored_from_version,
                record.rollback_skill_id,
                record.rolled_back_at_unix
            ));
        }
    }

    lines.join("\n")
}

fn build_skill_patch_rollback_skill(
    target: &MemorySkill,
    candidate: &SkillPatchCandidate,
    agent_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> MemorySkill {
    build_skill_rollback_snapshot(target, &candidate.id, "patch-candidate", agent_id, now)
}

fn build_skill_rollback_snapshot(
    target: &MemorySkill,
    change_id: &str,
    change_source: &str,
    agent_id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> MemorySkill {
    let rollback_id = format!(
        "rollback-{}-v{}",
        stable_skill_id_component(change_id),
        target.version
    );
    let mut tags = target.tags.clone();
    for tag in [
        "rollback".to_string(),
        format!("{change_source}:{change_id}"),
        format!("target-skill:{}", target.id),
        format!("target-version:{}", target.version),
    ] {
        if !tags.iter().any(|existing| existing == &tag) {
            tags.push(tag);
        }
    }

    MemorySkill {
        id: rollback_id,
        name: format!("{} rollback before {}", target.name, change_id),
        description: target.description.clone(),
        content: target.content.clone(),
        task_family: target.task_family.clone(),
        tool_pattern: target.tool_pattern.clone(),
        lineage_task_families: target.lineage_task_families.clone(),
        tags,
        success_count: target.success_count,
        fail_count: target.fail_count,
        version: target.version,
        origin: SkillOrigin::Learned,
        status: SkillStatus::Deprecated,
        created_by: agent_id.to_string(),
        created_at: now,
        updated_at: now,
    }
}

fn skill_patch_body_delta_lines(current: &str, proposed: &str, max_lines: usize) -> Vec<String> {
    if current.trim() == proposed.trim() {
        return vec!["  body_delta: none".into()];
    }

    let added = line_multiset_delta(proposed, current, max_lines);
    let removed = line_multiset_delta(current, proposed, max_lines);
    let mut lines = vec![format!("  body_delta: +{} -{}", added.total, removed.total)];
    if !added.preview.is_empty() {
        lines.push("  added_preview:".into());
        lines.extend(
            added
                .preview
                .iter()
                .map(|line| format!("    + {}", truncate_for_cli(line, 180))),
        );
    }
    if !removed.preview.is_empty() {
        lines.push("  removed_preview:".into());
        lines.extend(
            removed
                .preview
                .iter()
                .map(|line| format!("    - {}", truncate_for_cli(line, 180))),
        );
    }
    lines
}

struct LineDeltaPreview {
    total: usize,
    preview: Vec<String>,
}

fn line_multiset_delta(primary: &str, baseline: &str, max_lines: usize) -> LineDeltaPreview {
    let mut baseline_counts = HashMap::<String, usize>::new();
    for line in baseline.lines().map(str::trim_end) {
        *baseline_counts.entry(line.to_string()).or_default() += 1;
    }

    let mut total = 0usize;
    let mut preview = Vec::new();
    for line in primary.lines().map(str::trim_end) {
        let count = baseline_counts.entry(line.to_string()).or_default();
        if *count > 0 {
            *count -= 1;
            continue;
        }
        total += 1;
        if preview.len() < max_lines {
            preview.push(line.to_string());
        }
    }

    LineDeltaPreview { total, preview }
}

fn format_skill_patch_procedure_claim(claim: &SkillPatchProcedureClaim) -> String {
    format!(
        "tool={} failure={} action={}",
        claim.tool_name, claim.failure_kind, claim.suggested_action
    )
}

fn format_gateway_skill_patch_procedure_claim(claim: &GatewaySkillPatchProcedureClaim) -> String {
    format!(
        "tool={} failure={} action={}",
        claim.tool_name, claim.failure_kind, claim.suggested_action
    )
}

fn value_or_unknown(value: &str) -> &str {
    if value.trim().is_empty() {
        "unknown"
    } else {
        value
    }
}

fn replay_eval_counts<'a>(statuses: impl Iterator<Item = &'a str>) -> (usize, usize, usize) {
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut missing = 0usize;
    for status in statuses {
        match status {
            "passed" => passed += 1,
            "failed" => failed += 1,
            "missing" => missing += 1,
            _ => {}
        }
    }
    (passed, failed, missing)
}

fn print_gateway_skill_review_decision(decision: &GatewaySkillReviewDecision) {
    println!(
        "  {} [{} -> {}] — {}",
        console::style(&decision.skill_name).white().bold(),
        decision.action,
        decision.target_status,
        decision.reason
    );
    println!("    id: {}", decision.skill_id);
    if !decision.lineage_task_families.is_empty() {
        println!("    lineage: {}", decision.lineage_task_families.join(", "));
    }
}

fn print_gateway_applied_skill_decision(decision: &GatewayAppliedSkillDecision) {
    println!(
        "  {} [{}] — {}",
        console::style(&decision.skill_name).white().bold(),
        decision.target_status,
        decision.reason
    );
    println!("    id: {}", decision.skill_id);
}

fn print_skill_review_decision(decision: &SkillReviewDecision) {
    println!(
        "  {} [{} -> {}] — {}",
        console::style(&decision.skill_name).white().bold(),
        skill_review_action_name(&decision.action),
        decision.target_status,
        decision.reason
    );
    println!("    id: {}", decision.skill_id);
    if !decision.lineage_task_families.is_empty() {
        println!("    lineage: {}", decision.lineage_task_families.join(", "));
    }
}

fn print_skill_replay_report(report: &SkillReplayHarnessReport) {
    println!(
        "Skill patch replay for {} {}:",
        report.candidate_kind, report.candidate_id
    );
    println!(
        "  promotion_allowed={} reason={} passed={} failed={} missing={}",
        report.promotion_report.promotion_allowed,
        report.promotion_report.reason,
        report.promotion_report.passed_count,
        report.promotion_report.failed_count,
        report.promotion_report.missing_count
    );
    for result in &report.results {
        let status = match result.status {
            SkillReplayEvalStatus::Passed => "passed",
            SkillReplayEvalStatus::Failed => "failed",
            SkillReplayEvalStatus::Missing => "missing",
        };
        println!("  - {}: {}", status, result.criterion);
        if let Some(evidence) = result
            .evidence
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            println!("    {}", truncate_for_cli(evidence, 220));
        }
    }
    println!();
}

fn print_gateway_skill_replay_report(value: &serde_json::Value) {
    let candidate_id = value
        .get("candidate_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let candidate_kind = value
        .get("candidate_kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("patch");
    let promotion = value
        .get("promotion_report")
        .and_then(serde_json::Value::as_object);
    println!("Skill patch replay for {candidate_kind} {candidate_id}:");
    if let Some(promotion) = promotion {
        println!(
            "  promotion_allowed={} reason={} passed={} failed={} missing={}",
            promotion
                .get("promotion_allowed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            promotion
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
            promotion
                .get("passed_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            promotion
                .get("failed_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            promotion
                .get("missing_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        );
    }
    if let Some(results) = value.get("results").and_then(serde_json::Value::as_array) {
        for result in results {
            let status = result
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let criterion = result
                .get("criterion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown criterion");
            println!("  - {status}: {criterion}");
            if let Some(evidence) = result
                .get("evidence")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                println!("    {}", truncate_for_cli(evidence, 220));
            }
        }
    }
    println!();
}

fn skill_review_action_name(action: &SkillReviewAction) -> &'static str {
    match action {
        SkillReviewAction::PromoteToActive => "promote_to_active",
        SkillReviewAction::DowngradeToCandidate => "downgrade_to_candidate",
        SkillReviewAction::Deprecate => "deprecate",
    }
}

pub fn status_from_review_action(action: &SkillReviewAction) -> SkillStatus {
    match action {
        SkillReviewAction::PromoteToActive => SkillStatus::Active,
        SkillReviewAction::DowngradeToCandidate => SkillStatus::Candidate,
        SkillReviewAction::Deprecate => SkillStatus::Deprecated,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AppliedSkillReviewDecision {
    pub skill_id: String,
    pub skill_name: String,
    pub target_status: SkillStatus,
    pub reason: &'static str,
}

pub async fn apply_skill_review_decisions(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    decisions: &[SkillReviewDecision],
) -> Result<Vec<AppliedSkillReviewDecision>> {
    let mut applied = Vec::new();
    for decision in decisions {
        let status = status_from_review_action(&decision.action);
        memory
            .update_skill(
                &decision.skill_id,
                empty_status_update(status.clone()),
                &agent_id.to_string(),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to apply skill review decision for {}",
                    decision.skill_id
                )
            })?;
        applied.push(AppliedSkillReviewDecision {
            skill_id: decision.skill_id.clone(),
            skill_name: decision.skill_name.clone(),
            target_status: status,
            reason: decision.reason,
        });
    }
    Ok(applied)
}

fn parse_create_skill_status(raw: &str) -> Result<SkillStatus> {
    match raw.trim().to_lowercase().as_str() {
        "active" => Ok(SkillStatus::Active),
        "candidate" => Ok(SkillStatus::Candidate),
        other => bail!("invalid skill status `{other}`; expected active or candidate"),
    }
}

fn parse_update_skill_status(raw: &str) -> Result<SkillStatus> {
    match raw.trim().to_lowercase().as_str() {
        "active" => Ok(SkillStatus::Active),
        "candidate" => Ok(SkillStatus::Candidate),
        "deprecated" | "rejected" | "reject" => Ok(SkillStatus::Deprecated),
        other => bail!("invalid skill status `{other}`; expected active, candidate, or deprecated"),
    }
}

fn truncate_for_cli(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in value.trim().chars().take(max_chars) {
        out.push(ch);
    }
    if value.trim().chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn stable_skill_id_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skill-patch".to_string()
    } else {
        trimmed
    }
}

fn resolve_skill_export_destination(
    workspace_dir: &Path,
    skill: &MemorySkill,
    destination: &Option<PathBuf>,
    package_name: &Option<String>,
) -> Result<PathBuf> {
    if let Some(destination) = destination {
        return Ok(destination.clone());
    }

    let package_name = match package_name {
        Some(package_name) => validate_skill_package_dir_name(package_name)?,
        None => slugify_skill_package_name(&skill.name, &skill.id),
    };
    Ok(skills_dir(workspace_dir).join(package_name))
}

fn validate_skill_package_dir_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() || name == "." || name == ".." {
        bail!("Skill package name must not be empty or a special path component.");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("Skill package name must be a single directory name.");
    }
    Ok(name.to_string())
}

fn slugify_skill_package_name(name: &str, fallback_id: &str) -> String {
    let mut out = String::new();
    let mut pending_separator = false;
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('-');
            }
            for lowered in ch.to_lowercase() {
                out.push(lowered);
            }
            pending_separator = false;
        } else {
            pending_separator = true;
        }
        if out.chars().count() >= 80 {
            break;
        }
    }

    let slug = out.trim_matches('-').to_string();
    if !slug.is_empty() {
        return slug;
    }

    let fallback = stable_skill_id_component(fallback_id);
    if fallback == "skill-patch" {
        "skill".to_string()
    } else {
        fallback
    }
}

fn render_memory_skill_package_markdown(skill: &MemorySkill) -> Result<String> {
    let frontmatter = MemorySkillPackageFrontmatter {
        name: skill.name.clone(),
        description: skill.description.clone(),
        version: skill.version.to_string(),
        author: skill.created_by.clone(),
        tags: skill.tags.clone(),
        origin: skill.origin.to_string(),
        status: skill.status.to_string(),
        source_skill_id: skill.id.clone(),
        task_family: skill.task_family.clone(),
        tools: skill.tool_pattern.clone(),
        lineage_task_families: skill.lineage_task_families.clone(),
    };
    let yaml = serde_yaml::to_string(&frontmatter)
        .context("failed to render skill package frontmatter")?;
    let yaml = yaml
        .trim_start_matches("---\n")
        .trim_end_matches('\n')
        .trim_end();
    let body = skill.content.trim();
    Ok(format!("---\n{yaml}\n---\n\n{body}\n"))
}

fn skill_package_export_diff_summary(previous: Option<&str>, rendered: &str) -> String {
    let Some(previous) = previous else {
        return format!("create SKILL.md ({} chars)", rendered.chars().count());
    };
    if previous == rendered {
        return "overwrite SKILL.md with no content changes".to_string();
    }
    format!(
        "overwrite SKILL.md (old {} chars, new {} chars)",
        previous.chars().count(),
        rendered.chars().count()
    )
}

fn render_skill_package_scaffold_markdown(request: &SkillPackageScaffoldRequest) -> String {
    #[derive(Serialize)]
    struct ScaffoldFrontmatter<'a> {
        name: &'a str,
        description: &'a str,
        version: &'a str,
        status: &'a str,
        origin: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_family: Option<&'a str>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
    }

    let description = request
        .description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Operator-authored skill.");
    let frontmatter = ScaffoldFrontmatter {
        name: request.name.trim(),
        description,
        version: "1",
        status: "active",
        origin: "manual",
        task_family: request.task_family.as_deref(),
        tools: request.tool_pattern.clone(),
        tags: request.tags.clone(),
    };
    let yaml = serde_yaml::to_string(&frontmatter)
        .unwrap_or_default()
        .trim_start_matches("---\n")
        .trim_end_matches('\n')
        .trim_end()
        .to_string();
    format!(
        "---\n{yaml}\n---\n\n# {}\n\n## When to use\n\nDescribe the task pattern this skill should help with.\n\n## Procedure\n\n1. Add the smallest reliable steps.\n2. Reference supporting files with relative paths when needed.\n3. Keep secrets and credentials out of the skill body.\n",
        request.name.trim()
    )
}

async fn list_queued_skill_patch_candidates(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    limit: usize,
) -> Result<Vec<SkillPatchCandidate>> {
    let category = skill_patch_candidate_service::skill_patch_candidate_memory_category();
    let entries = memory.list(Some(&category), None, limit).await?;
    Ok(entries
        .iter()
        .filter_map(skill_patch_candidate_service::parse_skill_patch_candidate_entry)
        .collect())
}

async fn list_skill_patch_apply_records(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    limit: usize,
) -> Result<Vec<SkillPatchApplyRecord>> {
    let category = skill_patch_candidate_service::skill_patch_apply_memory_category();
    let entries = memory.list(Some(&category), None, limit).await?;
    Ok(entries
        .iter()
        .filter_map(skill_patch_candidate_service::parse_skill_patch_apply_entry)
        .collect())
}

async fn list_skill_patch_rollback_records(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    limit: usize,
) -> Result<Vec<SkillPatchRollbackRecord>> {
    let category = skill_patch_candidate_service::skill_patch_rollback_memory_category();
    let entries = memory.list(Some(&category), None, limit).await?;
    Ok(entries
        .iter()
        .filter_map(skill_patch_candidate_service::parse_skill_patch_rollback_entry)
        .collect())
}

async fn resolve_skill_patch_candidate_for_display(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    candidate_ref: &str,
    limit: usize,
) -> Result<SkillPatchCandidate> {
    let needle = candidate_ref.trim();
    if needle.is_empty() {
        bail!("skill patch candidate id must not be empty");
    }
    list_queued_skill_patch_candidates(memory, limit.max(1))
        .await?
        .into_iter()
        .find(|candidate| {
            candidate.id == needle
                || skill_patch_candidate_service::skill_patch_candidate_memory_key(candidate)
                    == needle
        })
        .ok_or_else(|| anyhow::anyhow!("No skill patch candidate found: {needle}"))
}

async fn resolve_skill_version_filter(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_ref: &str,
) -> Result<SkillVersionFilter> {
    let skill = resolve_skill_ref_including_id(memory, agent_id, skill_ref).await?;
    Ok(SkillVersionFilter {
        skill_id: skill.id,
        skill_name: Some(skill.name),
    })
}

async fn resolve_skill_patch_apply_record(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    rollback_ref: &str,
    limit: usize,
) -> Result<SkillPatchApplyRecord> {
    let needle = rollback_ref.trim();
    if needle.is_empty() {
        bail!("skill patch rollback ref must not be empty");
    }
    list_skill_patch_apply_records(memory, limit.max(1))
        .await?
        .into_iter()
        .find(|record| {
            record.id == needle
                || skill_patch_candidate_service::skill_patch_apply_memory_key(record) == needle
                || record.candidate_id == needle
                || record.rollback_skill_id == needle
        })
        .ok_or_else(|| anyhow::anyhow!("No skill patch apply record found: {needle}"))
}

async fn resolve_skill_ref_including_id(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_ref: &str,
) -> Result<MemorySkill> {
    let needle = skill_ref.trim();
    if needle.is_empty() {
        bail!("skill id/name must not be empty");
    }
    if let Some(skill) = memory
        .get_skill_by_id(&needle.to_string(), &agent_id.to_string())
        .await?
    {
        return Ok(skill);
    }
    resolve_learned_skill_ref(memory, agent_id, needle).await
}

fn validate_rollback_snapshot(
    rollback_skill: &MemorySkill,
    apply_record: &SkillPatchApplyRecord,
) -> Result<()> {
    if rollback_skill.origin != SkillOrigin::Learned {
        bail!(
            "Rollback snapshot {} is not a learned skill snapshot",
            apply_record.rollback_skill_id
        );
    }
    if rollback_skill.status != SkillStatus::Deprecated {
        bail!(
            "Rollback snapshot {} must be deprecated to stay out of runtime retrieval",
            apply_record.rollback_skill_id
        );
    }
    if rollback_skill.version != apply_record.previous_version {
        bail!(
            "Rollback snapshot {} has version v{}, apply record expects v{}",
            apply_record.rollback_skill_id,
            rollback_skill.version,
            apply_record.previous_version
        );
    }
    Ok(())
}

async fn list_learned_skills(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let skills = memory
                .list_skills(&agent_id, skill_list_filter_fetch_limit(limit))
                .await?;
            let learned = skills
                .iter()
                .filter(|skill| {
                    skill.origin == synapse_domain::domain::memory::SkillOrigin::Learned
                })
                .take(limit)
                .collect::<Vec<_>>();

            if learned.is_empty() {
                println!("No learned skills found for agent {agent_id}.");
                return Ok(());
            }

            println!("Learned skills for agent {agent_id} ({}):\n", learned.len());
            for skill in learned {
                print_learned_skill(skill);
            }
            println!();
            Ok(())
        }
        SkillAccess::Gateway(gateway) => gateway_list_learned_skills(&gateway, limit, false).await,
    }
}

async fn list_user_authored_skills(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let skills = memory
                .list_skills(&agent_id, skill_list_filter_fetch_limit(limit))
                .await?;
            let authored = skills
                .iter()
                .filter(|skill| skill.origin == SkillOrigin::Manual)
                .take(limit)
                .collect::<Vec<_>>();

            if authored.is_empty() {
                println!("No user-authored skills found for agent {agent_id}.");
                return Ok(());
            }

            println!(
                "User-authored skills for agent {agent_id} ({}):\n",
                authored.len()
            );
            for skill in authored {
                print_learned_skill(skill);
            }
            println!();
            Ok(())
        }
        SkillAccess::Gateway(gateway) => gateway_list_user_authored_skills(&gateway, limit).await,
    }
}

fn load_file_backed_runtime_skills_for_cli(
    workspace_dir: &Path,
    config: &synapse_domain::config::schema::Config,
) -> Vec<Skill> {
    load_file_backed_runtime_skills(workspace_dir, config)
}

fn skill_list_filter_fetch_limit(limit: usize) -> usize {
    limit.max(100).saturating_mul(4).min(1000)
}

async fn gateway_list_user_authored_skills(
    gateway: &GatewaySkillClient,
    limit: usize,
) -> Result<()> {
    let response: GatewaySkillListResponse =
        gateway.get_json("/api/skills/authored", limit).await?;
    let response_agent = gateway_response_agent(gateway, &response.agent_id)?;
    if response.skills.is_empty() {
        println!(
            "No user-authored skills found for agent {}.",
            response_agent
        );
        return Ok(());
    }

    println!(
        "User-authored skills for agent {} ({}):\n",
        response_agent,
        response.skills.len()
    );
    for skill in &response.skills {
        print_gateway_learned_skill(skill);
    }
    println!();
    Ok(())
}

fn user_authored_skill_request_from_parts(
    name: Option<String>,
    description: Option<String>,
    body: Option<String>,
    from_file: Option<&Path>,
    task_family: Option<String>,
    tools: Vec<String>,
    tags: Vec<String>,
    status: String,
) -> Result<UserAuthoredSkillCreateRequest> {
    let status = parse_create_skill_status(&status)?;
    let (file_name, file_description, file_body, file_tags) = match from_file {
        Some(path) => {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read skill markdown {}", path.display()))?;
            let parsed = parse_skill_markdown(&content);
            (
                parsed.meta.name,
                parsed.meta.description,
                Some(parsed.body),
                parsed.meta.tags,
            )
        }
        None => (None, None, None, Vec::new()),
    };
    let body = body.or(file_body).ok_or_else(|| {
        anyhow::anyhow!("skill body is required; pass --body or --from-file SKILL.md")
    })?;
    let name = name.or(file_name).ok_or_else(|| {
        anyhow::anyhow!("skill name is required; pass --name or frontmatter name")
    })?;
    let mut tags = tags;
    tags.extend(file_tags);

    Ok(UserAuthoredSkillCreateRequest {
        name,
        description: description.or(file_description),
        body,
        task_family,
        tool_pattern: tools,
        tags,
        status,
    })
}

fn user_skill_update_request_from_parts(
    skill_ref: String,
    description: Option<String>,
    body: Option<String>,
    from_file: Option<&Path>,
    task_family: Option<String>,
    tools: Vec<String>,
    tags: Vec<String>,
    status: Option<String>,
) -> Result<UserSkillUpdateRequest> {
    let (file_description, file_body, file_tags) = match from_file {
        Some(path) => {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read skill markdown {}", path.display()))?;
            let parsed = parse_skill_markdown(&content);
            (parsed.meta.description, Some(parsed.body), parsed.meta.tags)
        }
        None => (None, None, Vec::new()),
    };
    let mut merged_tags = tags;
    merged_tags.extend(file_tags);
    Ok(UserSkillUpdateRequest {
        skill_ref,
        description: description.or(file_description),
        body: body.or(file_body),
        task_family,
        tool_pattern: (!tools.is_empty()).then_some(tools),
        tags: (!merged_tags.is_empty()).then_some(merged_tags),
        status: status
            .as_deref()
            .map(parse_update_skill_status)
            .transpose()?,
    })
}

async fn create_user_authored_skill_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    request: UserAuthoredSkillCreateRequest,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output =
                create_user_authored_skill_output(memory.as_ref(), &agent_id, request).await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_create_user_authored_skill(&gateway, request).await
        }
    }
}

async fn update_user_skill_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    request: UserSkillUpdateRequest,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output = update_user_skill_output(memory.as_ref(), &agent_id, request).await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => gateway_update_user_skill(&gateway, request).await,
    }
}

async fn gateway_update_user_skill(
    gateway: &GatewaySkillClient,
    request: UserSkillUpdateRequest,
) -> Result<()> {
    let response: serde_json::Value = gateway
        .post_json(
            "/api/skills/update",
            &serde_json::json!({
                "skill": request.skill_ref,
                "description": request.description,
                "body": request.body,
                "task_family": request.task_family,
                "tool_pattern": request.tool_pattern,
                "tags": request.tags,
                "status": request.status.map(|status| status.to_string()),
            }),
        )
        .await?;
    gateway_response_agent(
        gateway,
        response
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    println!(
        "{}",
        response
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Updated skill.")
    );
    Ok(())
}

async fn gateway_create_user_authored_skill(
    gateway: &GatewaySkillClient,
    request: UserAuthoredSkillCreateRequest,
) -> Result<()> {
    let response: serde_json::Value = gateway
        .post_json(
            "/api/skills/create",
            &serde_json::json!({
                "name": request.name,
                "description": request.description,
                "body": request.body,
                "task_family": request.task_family,
                "tool_pattern": request.tool_pattern,
                "tags": request.tags,
                "status": request.status.to_string(),
            }),
        )
        .await?;
    gateway_response_agent(
        gateway,
        response
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    println!(
        "{}",
        response
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Created user-authored skill.")
    );
    Ok(())
}

async fn export_user_skill_package_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    request: UserSkillPackageExportRequest,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output = export_user_skill_package_output(
                memory.as_ref(),
                &agent_id,
                &config.workspace_dir,
                request,
            )
            .await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => gateway_export_user_skill_package(&gateway, request).await,
    }
}

async fn gateway_export_user_skill_package(
    gateway: &GatewaySkillClient,
    request: UserSkillPackageExportRequest,
) -> Result<()> {
    if request.destination.is_some() {
        bail!("skills export --to requires direct memory access; omit --to or stop the daemon.");
    }
    let response: serde_json::Value = gateway
        .post_json(
            "/api/skills/export",
            &serde_json::json!({
                "skill": request.skill_ref,
                "package_name": request.package_name,
                "overwrite": request.overwrite,
            }),
        )
        .await?;
    gateway_response_agent(
        gateway,
        response
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    println!(
        "{}",
        response
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Exported skill package.")
    );
    Ok(())
}

async fn list_learned_skill_candidates(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let skills = memory.list_skills(&agent_id, limit).await?;
            let candidates = skills
                .iter()
                .filter(|skill| {
                    skill.origin == synapse_domain::domain::memory::SkillOrigin::Learned
                        && skill.status == SkillStatus::Candidate
                })
                .collect::<Vec<_>>();
            let patch_candidates =
                list_queued_skill_patch_candidates(memory.as_ref(), limit).await?;

            if candidates.is_empty() && patch_candidates.is_empty() {
                println!("No skill candidates found for agent {agent_id}.");
                return Ok(());
            }

            if !candidates.is_empty() {
                println!(
                    "Learned skill candidates for agent {agent_id} ({}):\n",
                    candidates.len()
                );
                for skill in candidates {
                    print_learned_skill(skill);
                }
                println!();
            }

            if !patch_candidates.is_empty() {
                println!(
                    "Generated skill patch candidates for agent {agent_id} ({}):\n",
                    patch_candidates.len()
                );
                for candidate in &patch_candidates {
                    print_skill_patch_candidate(candidate);
                }
                println!();
            }
            Ok(())
        }
        SkillAccess::Gateway(gateway) => gateway_list_learned_skills(&gateway, limit, true).await,
    }
}

async fn list_skill_use_trace_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let traces = list_skill_use_traces(memory.as_ref(), &agent_id, limit).await?;
            if traces.is_empty() {
                println!("No skill use traces found for agent {agent_id}.");
                return Ok(());
            }

            println!(
                "Skill use traces for agent {agent_id} ({}):\n",
                traces.len()
            );
            for trace in &traces {
                print_skill_use_trace(trace);
            }
            println!();
            Ok(())
        }
        SkillAccess::Gateway(gateway) => gateway_list_skill_use_traces(&gateway, limit).await,
    }
}

async fn show_skill_health_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
    trace_limit: usize,
    apply: bool,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output = if apply {
                format_skill_health_cleanup_output(
                    memory.as_ref(),
                    &agent_id,
                    limit,
                    trace_limit,
                    true,
                )
                .await?
            } else {
                format_skill_health_cleanup_output(
                    memory.as_ref(),
                    &agent_id,
                    limit,
                    trace_limit,
                    false,
                )
                .await?
            };
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_show_skill_health(&gateway, limit, trace_limit, apply).await
        }
    }
}

async fn show_skill_patch_candidate_diff_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    candidate: String,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output = format_skill_patch_candidate_diff_output(
                memory.as_ref(),
                &agent_id,
                &candidate,
                limit,
            )
            .await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_show_skill_patch_candidate_diff(&gateway, &candidate, limit).await
        }
    }
}

async fn gateway_list_learned_skills(
    gateway: &GatewaySkillClient,
    limit: usize,
    candidates_only: bool,
) -> Result<()> {
    let path = if candidates_only {
        "/api/skills/candidates"
    } else {
        "/api/skills/learned"
    };
    let response: GatewaySkillListResponse = gateway.get_json(path, limit).await?;
    let label = if candidates_only {
        "Learned skill candidates"
    } else {
        "Learned skills"
    };

    let response_agent = gateway_response_agent(gateway, &response.agent_id)?;
    if response.skills.is_empty() && (!candidates_only || response.patch_candidates.is_empty()) {
        println!(
            "No {} found for agent {}.",
            label.to_lowercase(),
            response_agent
        );
        return Ok(());
    }

    if !response.skills.is_empty() {
        println!(
            "{label} for agent {} ({}):\n",
            response_agent,
            response.skills.len()
        );
        for skill in &response.skills {
            print_gateway_learned_skill(skill);
        }
        println!();
    }

    if candidates_only && !response.patch_candidates.is_empty() {
        println!(
            "Generated skill patch candidates for agent {} ({}):\n",
            response_agent,
            response.patch_candidates.len()
        );
        for candidate in &response.patch_candidates {
            print_gateway_skill_patch_candidate(candidate);
        }
        println!();
    }
    Ok(())
}

async fn gateway_list_skill_use_traces(gateway: &GatewaySkillClient, limit: usize) -> Result<()> {
    let response: GatewaySkillUseTraceResponse =
        gateway.get_json("/api/skills/traces", limit).await?;
    let response_agent = gateway_response_agent(gateway, &response.agent_id)?;
    if response.traces.is_empty() {
        println!("No skill use traces found for agent {}.", response_agent);
        return Ok(());
    }

    println!(
        "Skill use traces for agent {} ({}):\n",
        response_agent,
        response.traces.len()
    );
    for trace in &response.traces {
        print_gateway_skill_use_trace(trace);
    }
    println!();
    Ok(())
}

async fn gateway_show_skill_health(
    gateway: &GatewaySkillClient,
    limit: usize,
    trace_limit: usize,
    apply: bool,
) -> Result<()> {
    if apply {
        let response: GatewaySkillHealthApplyResponse = gateway
            .post_json(
                "/api/skills/health/apply",
                &serde_json::json!({
                    "limit": limit,
                    "trace_limit": trace_limit,
                }),
            )
            .await?;
        let _ = gateway_response_agent(gateway, &response.agent_id)?;
        println!("{}", response.output);
    } else {
        let response: GatewaySkillHealthResponse = gateway
            .get_json_query(
                "/api/skills/health",
                &[
                    ("limit", limit.to_string()),
                    ("trace_limit", trace_limit.to_string()),
                ],
            )
            .await?;
        let _ = gateway_response_agent(gateway, &response.agent_id)?;
        println!("{}", format_skill_health_report_text(&response.report));
    }
    Ok(())
}

async fn gateway_show_skill_patch_candidate_diff(
    gateway: &GatewaySkillClient,
    candidate: &str,
    limit: usize,
) -> Result<()> {
    let response: GatewaySkillPatchDiffResponse = gateway
        .post_json(
            "/api/skills/candidates/diff",
            &serde_json::json!({
                "candidate": candidate,
                "limit": limit,
            }),
        )
        .await?;
    let _ = gateway_response_agent(gateway, &response.agent_id)?;
    println!("{}", response.output);
    Ok(())
}

async fn apply_skill_patch_candidate_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    candidate: String,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output =
                apply_skill_patch_candidate_output(memory.as_ref(), &agent_id, &candidate, limit)
                    .await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_apply_skill_patch_candidate(&gateway, &candidate, limit).await
        }
    }
}

async fn gateway_apply_skill_patch_candidate(
    gateway: &GatewaySkillClient,
    candidate: &str,
    limit: usize,
) -> Result<()> {
    let response: serde_json::Value = gateway
        .post_json(
            "/api/skills/candidates/apply",
            &serde_json::json!({
                "candidate": candidate,
                "limit": limit,
            }),
        )
        .await?;
    let response_agent = gateway_response_agent(
        gateway,
        response
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    let candidate_id = response
        .get("candidate_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(candidate);
    let skill_name = response
        .get("skill_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown skill");
    let target_skill_id = response
        .get("target_skill_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let previous_version = response
        .get("previous_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let new_version = response
        .get("new_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let rollback_skill_id = response
        .get("rollback_skill_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    println!(
        "Applied skill patch candidate {candidate_id} to '{skill_name}' ({target_skill_id}) for agent {response_agent}: v{previous_version} -> v{new_version}\nRollback snapshot: {rollback_skill_id}"
    );
    Ok(())
}

async fn show_skill_patch_versions_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    skill: Option<String>,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output = format_skill_patch_versions_output(
                memory.as_ref(),
                &agent_id,
                skill.as_deref(),
                limit,
            )
            .await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_show_skill_patch_versions(&gateway, skill.as_deref(), limit).await
        }
    }
}

async fn gateway_show_skill_patch_versions(
    gateway: &GatewaySkillClient,
    skill: Option<&str>,
    limit: usize,
) -> Result<()> {
    let mut query = vec![("limit", limit.to_string())];
    if let Some(skill) = skill.map(str::trim).filter(|value| !value.is_empty()) {
        query.push(("skill", skill.to_string()));
    }
    let url = gateway.url("/api/skills/versions");
    let response = gateway
        .client
        .get(&url)
        .query(&query)
        .bearer_auth(&gateway.token)
        .send()
        .await
        .with_context(|| format!("failed to reach skills gateway at {url}"))?;
    let value: serde_json::Value =
        decode_gateway_response(response, "/api/skills/versions").await?;
    gateway_response_agent(
        gateway,
        value
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    println!(
        "{}",
        value
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("No skill version output returned.")
    );
    Ok(())
}

async fn rollback_skill_patch_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    rollback: String,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output =
                rollback_skill_patch_output(memory.as_ref(), &agent_id, &rollback, limit).await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_rollback_skill_patch(&gateway, &rollback, limit).await
        }
    }
}

async fn gateway_rollback_skill_patch(
    gateway: &GatewaySkillClient,
    rollback: &str,
    limit: usize,
) -> Result<()> {
    let response: serde_json::Value = gateway
        .post_json(
            "/api/skills/rollback",
            &serde_json::json!({
                "rollback": rollback,
                "limit": limit,
            }),
        )
        .await?;
    let response_agent = gateway_response_agent(
        gateway,
        response
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    let rollback_ref = response
        .get("rollback_ref")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(rollback);
    let skill_name = response
        .get("skill_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown skill");
    let target_skill_id = response
        .get("target_skill_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let from_version = response
        .get("from_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let restored_from_version = response
        .get("restored_from_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let new_version = response
        .get("new_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let apply_record_id = response
        .get("apply_record_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let rollback_skill_id = response
        .get("rollback_skill_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    println!(
        "Rolled back skill patch {rollback_ref} for '{skill_name}' ({target_skill_id}) on agent {response_agent}: v{from_version} -> rollback v{restored_from_version} -> current v{new_version}\nApply record: {apply_record_id}\nRollback snapshot: {rollback_skill_id}"
    );
    Ok(())
}

async fn auto_promote_skill_patches_command(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
    apply: bool,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    let policy = skill_auto_promotion_policy_from_config(&config.skills.auto_promotion);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let output = format_skill_auto_promotion_output(
                memory.as_ref(),
                &agent_id,
                &policy,
                limit,
                apply,
            )
            .await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_auto_promote_skill_patches(&gateway, limit, apply).await
        }
    }
}

async fn gateway_auto_promote_skill_patches(
    gateway: &GatewaySkillClient,
    limit: usize,
    apply: bool,
) -> Result<()> {
    let value: serde_json::Value = if apply {
        gateway
            .post_json(
                "/api/skills/autopromote/apply",
                &serde_json::json!({ "limit": limit }),
            )
            .await?
    } else {
        gateway.get_json("/api/skills/autopromote", limit).await?
    };
    gateway_response_agent(
        gateway,
        value
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
    )?;
    println!(
        "{}",
        value
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("No skill auto-promotion output returned.")
    );
    Ok(())
}

async fn review_learned_skill_memory(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    limit: usize,
    apply: bool,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let skills = memory.list_skills(&agent_id, limit).await?;
            let learned = skills
                .into_iter()
                .filter(|skill| {
                    skill.origin == synapse_domain::domain::memory::SkillOrigin::Learned
                })
                .collect::<Vec<_>>();
            let decisions = review_learned_skills(&learned, &[]);

            if decisions.is_empty() {
                println!("No learned skill review actions for agent {agent_id}.");
                return Ok(());
            }

            println!(
                "Learned skill review for agent {agent_id} ({}):\n",
                decisions.len()
            );
            for decision in &decisions {
                print_skill_review_decision(decision);
            }

            if apply {
                apply_skill_review_decisions(memory.as_ref(), &agent_id, &decisions).await?;
                println!("\nApplied {} review decisions.", decisions.len());
            } else {
                println!("\nDry run. Re-run with --apply to write these status changes.");
            }
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_review_learned_skills(&gateway, limit, apply).await
        }
    }
}

async fn gateway_review_learned_skills(
    gateway: &GatewaySkillClient,
    limit: usize,
    apply: bool,
) -> Result<()> {
    if apply {
        let response: GatewaySkillReviewResponse = gateway
            .post_json(
                "/api/skills/review/apply",
                &serde_json::json!({ "limit": limit }),
            )
            .await?;
        let response_agent = gateway_response_agent(gateway, &response.agent_id)?;
        if response.applied_decisions.is_empty() {
            println!(
                "No learned skill review actions applied for agent {}.",
                response_agent
            );
            return Ok(());
        }
        println!(
            "Applied learned skill review for agent {} ({}):\n",
            response_agent,
            response.applied_decisions.len()
        );
        for decision in &response.applied_decisions {
            print_gateway_applied_skill_decision(decision);
        }
        println!("\nApplied {} review decisions.", response.decision_count);
        return Ok(());
    }

    let response: GatewaySkillReviewResponse =
        gateway.get_json("/api/skills/review", limit).await?;
    let response_agent = gateway_response_agent(gateway, &response.agent_id)?;
    if response.decisions.is_empty() {
        println!(
            "No learned skill review actions for agent {}.",
            response_agent
        );
        return Ok(());
    }

    println!(
        "Learned skill review for agent {} ({}):\n",
        response_agent,
        response.decisions.len()
    );
    for decision in &response.decisions {
        print_gateway_skill_review_decision(decision);
    }
    println!("\nDry run. Re-run with --apply to write these status changes.");
    Ok(())
}

fn build_cli_replay_tools(
    config: &synapse_domain::config::schema::Config,
    memory: Arc<dyn synapse_memory::UnifiedMemoryPort>,
) -> Result<Vec<Box<dyn crate::tools::Tool>>> {
    let runtime: Arc<dyn synapse_domain::ports::runtime::RuntimeAdapter> =
        Arc::from(crate::runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(
        synapse_security::security_factory::security_policy_from_config(
            &config.autonomy,
            &config.workspace_dir,
        ),
    );
    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };
    let (tools, _, _) = crate::tools::RuntimeToolRegistryFactory::build(
        crate::tools::RuntimeToolRegistryInputs {
            config: Arc::new(config.clone()),
            security: &security,
            runtime,
            memory,
            composio_key,
            composio_entity_id,
            browser_config: &config.browser,
            http_config: &config.http_request,
            web_fetch_config: &config.web_fetch,
            workspace_dir: &config.workspace_dir,
            agents: &config.agents,
            default_api_key: config.api_key.as_deref(),
            root_config: config,
        },
        crate::tools::RuntimeToolPorts::default(),
    );
    Ok(tools)
}

fn print_cli_tool_contract_inventory(
    config: &synapse_domain::config::schema::Config,
) -> Result<()> {
    let memory: Arc<dyn synapse_memory::UnifiedMemoryPort> =
        Arc::new(synapse_memory::NoopUnifiedMemory);
    let tools = build_cli_replay_tools(config, memory)?;
    print!(
        "{}",
        crate::tools::format_tool_contract_inventory_report(&tools)
    );
    Ok(())
}

async fn test_skill_patch_candidate(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    candidate: String,
    limit: usize,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let tools = build_cli_replay_tools(config, Arc::clone(&memory))?;
            let report = replay::run_and_store_skill_patch_replay(
                memory.as_ref(),
                &agent_id,
                &candidate,
                &tools,
                limit,
            )
            .await?;
            print_skill_replay_report(&report);
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            let response: serde_json::Value = gateway
                .post_json(
                    "/api/skills/candidates/test",
                    &serde_json::json!({
                        "candidate": candidate,
                        "limit": limit,
                    }),
                )
                .await?;
            gateway_response_agent(
                &gateway,
                response
                    .get("agent_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            )?;
            print_gateway_skill_replay_report(
                response.get("report").unwrap_or(&serde_json::Value::Null),
            );
            Ok(())
        }
    }
}

async fn resolve_learned_skill_ref(
    memory: &dyn synapse_memory::UnifiedMemoryPort,
    agent_id: &str,
    skill_ref: &str,
) -> Result<MemorySkill> {
    let needle = skill_ref.trim();
    if needle.is_empty() {
        bail!("skill id/name must not be empty");
    }

    let skills = memory.list_skills(&agent_id.to_string(), 512).await?;
    if let Some(skill) = skills.iter().find(|skill| skill.id == needle) {
        return Ok(skill.clone());
    }
    let normalized_needle = needle.to_lowercase();
    if let Some(skill) = skills
        .iter()
        .find(|skill| skill.name.to_lowercase() == normalized_needle)
    {
        return Ok(skill.clone());
    }
    if let Some(skill) = memory.get_skill(needle, &agent_id.to_string()).await? {
        return Ok(skill);
    }

    bail!("No learned skill found for agent {agent_id}: {skill_ref}")
}

async fn update_learned_skill_status(
    config: &synapse_domain::config::schema::Config,
    agent: Option<String>,
    skill_ref: String,
    target_status: SkillStatus,
) -> Result<()> {
    let agent_id = resolve_skill_agent(config, agent);
    match create_skill_access(config, &agent_id).await? {
        SkillAccess::Direct { memory, agent_id } => {
            let skill = resolve_learned_skill_ref(memory.as_ref(), &agent_id, &skill_ref).await?;

            if !matches!(skill.origin, SkillOrigin::Learned | SkillOrigin::Manual) {
                bail!(
                    "Refusing to mutate non-local skill '{}' with origin {}",
                    skill.name,
                    skill.origin
                );
            }

            let output = update_user_skill_output(
                memory.as_ref(),
                &agent_id,
                UserSkillUpdateRequest {
                    skill_ref: skill.id,
                    description: None,
                    body: None,
                    task_family: None,
                    tool_pattern: None,
                    tags: None,
                    status: Some(target_status),
                },
            )
            .await?;
            println!("{output}");
            Ok(())
        }
        SkillAccess::Gateway(gateway) => {
            gateway_update_learned_skill_status(&gateway, &skill_ref, target_status).await
        }
    }
}

async fn gateway_update_learned_skill_status(
    gateway: &GatewaySkillClient,
    skill_ref: &str,
    target_status: SkillStatus,
) -> Result<()> {
    let path = match target_status {
        SkillStatus::Active => "/api/skills/promote",
        SkillStatus::Candidate => "/api/skills/demote",
        SkillStatus::Deprecated => "/api/skills/reject",
    };
    let response: GatewaySkillStatusUpdateResponse = gateway
        .post_json(path, &serde_json::json!({ "skill": skill_ref }))
        .await?;
    let response_agent = gateway_response_agent(gateway, &response.agent_id)?;
    println!(
        "Updated learned skill '{}' ({}) for agent {}: {} -> {}",
        response.skill_name,
        response.skill_id,
        response_agent,
        response.previous_status,
        response.target_status
    );
    Ok(())
}

/// Handle the `skills` CLI command
#[allow(clippy::too_many_lines)]
pub async fn handle_command(
    command: crate::commands::SkillCommands,
    config: &synapse_domain::config::schema::Config,
) -> Result<()> {
    let workspace_dir = &config.workspace_dir;
    match command {
        crate::commands::SkillCommands::List => {
            let skills = load_file_backed_runtime_skills_for_cli(workspace_dir, config);
            if skills.is_empty() {
                if config.skills.port_workspace_packages_on_start {
                    println!("No file-backed runtime skills are active.");
                    println!();
                    println!(
                        "  Workspace SKILL.md packages are ported to memory on startup; use `synapseclaw skills authored` and `synapseclaw skills learned`."
                    );
                    println!(
                        "  Create one: synapseclaw skills create --name <name> --body <markdown>"
                    );
                } else {
                    println!("No skills installed.");
                    println!();
                    println!("  Create one: mkdir -p ~/.synapseclaw/workspace/skills/my-skill");
                    println!("              echo '# My Skill' > ~/.synapseclaw/workspace/skills/my-skill/SKILL.md");
                    println!();
                    println!("  Or install: synapseclaw skills install <source>");
                }
            } else {
                println!("Installed skills ({}):", skills.len());
                println!();
                for skill in &skills {
                    println!(
                        "  {} {} — {}",
                        console::style(&skill.name).white().bold(),
                        console::style(format!("v{}", skill.version)).dim(),
                        skill.description
                    );
                    if !skill.tools.is_empty() {
                        println!(
                            "    Tools: {}",
                            skill
                                .tools
                                .iter()
                                .map(|t| t.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    if !skill.tags.is_empty() {
                        println!("    Tags:  {}", skill.tags.join(", "));
                    }
                }
            }
            println!();
            Ok(())
        }
        crate::commands::SkillCommands::Learned { agent, limit } => {
            list_learned_skills(config, agent, limit).await
        }
        crate::commands::SkillCommands::Authored { agent, limit } => {
            list_user_authored_skills(config, agent, limit).await
        }
        crate::commands::SkillCommands::Create {
            name,
            description,
            body,
            from_file,
            task_family,
            tools,
            tags,
            status,
            agent,
        } => {
            let request = user_authored_skill_request_from_parts(
                name,
                description,
                body,
                from_file.as_deref(),
                task_family,
                tools,
                tags,
                status,
            )?;
            create_user_authored_skill_command(config, agent, request).await
        }
        crate::commands::SkillCommands::Export {
            skill,
            agent,
            to,
            package_name,
            overwrite,
        } => {
            let request = UserSkillPackageExportRequest {
                skill_ref: skill,
                destination: to,
                package_name,
                overwrite,
            };
            export_user_skill_package_command(config, agent, request).await
        }
        crate::commands::SkillCommands::Update {
            skill,
            description,
            body,
            from_file,
            task_family,
            tools,
            tags,
            status,
            agent,
        } => {
            let request = user_skill_update_request_from_parts(
                skill,
                description,
                body,
                from_file.as_deref(),
                task_family,
                tools,
                tags,
                status,
            )?;
            update_user_skill_command(config, agent, request).await
        }
        crate::commands::SkillCommands::Scaffold {
            name,
            description,
            task_family,
            tools,
            tags,
            overwrite,
        } => {
            let outcome = scaffold_skill_package(
                workspace_dir,
                SkillPackageScaffoldRequest {
                    name,
                    description,
                    task_family,
                    tool_pattern: tools,
                    tags,
                    overwrite,
                },
            )?;
            println!("{}", format_skill_package_scaffold_outcome_text(&outcome));
            Ok(())
        }
        crate::commands::SkillCommands::Review {
            agent,
            limit,
            apply,
        } => review_learned_skill_memory(config, agent, limit, apply).await,
        crate::commands::SkillCommands::Promote { skill, agent } => {
            update_learned_skill_status(config, agent, skill, SkillStatus::Active).await
        }
        crate::commands::SkillCommands::Demote { skill, agent } => {
            update_learned_skill_status(config, agent, skill, SkillStatus::Candidate).await
        }
        crate::commands::SkillCommands::Reject { skill, agent } => {
            update_learned_skill_status(config, agent, skill, SkillStatus::Deprecated).await
        }
        crate::commands::SkillCommands::Status => {
            let skills = load_file_backed_runtime_skills_for_cli(workspace_dir, config);
            let report = resolve_loaded_skill_status(&skills, workspace_dir);
            print_skill_resolution_report(&report, |_| true);
            Ok(())
        }
        crate::commands::SkillCommands::Blocked => {
            let skills = load_file_backed_runtime_skills_for_cli(workspace_dir, config);
            let report = resolve_loaded_skill_status(&skills, workspace_dir);
            print_skill_resolution_report(&report, |state| {
                !matches!(state, SkillRuntimeState::Active)
            });
            Ok(())
        }
        crate::commands::SkillCommands::Candidates { agent, limit } => {
            let skills = load_file_backed_runtime_skills_for_cli(workspace_dir, config);
            let report = resolve_loaded_skill_status(&skills, workspace_dir);
            print_skill_resolution_report_if_nonempty(&report, |state| {
                state == SkillRuntimeState::Candidate
            });
            list_learned_skill_candidates(config, agent, limit).await
        }
        crate::commands::SkillCommands::Test {
            candidate,
            agent,
            limit,
        } => test_skill_patch_candidate(config, agent, candidate, limit).await,
        crate::commands::SkillCommands::Tools => print_cli_tool_contract_inventory(config),
        crate::commands::SkillCommands::Traces { agent, limit } => {
            list_skill_use_trace_command(config, agent, limit).await
        }
        crate::commands::SkillCommands::Health {
            agent,
            limit,
            trace_limit,
            apply,
        } => show_skill_health_command(config, agent, limit, trace_limit, apply).await,
        crate::commands::SkillCommands::Diff {
            candidate,
            agent,
            limit,
        } => show_skill_patch_candidate_diff_command(config, agent, candidate, limit).await,
        crate::commands::SkillCommands::Apply {
            candidate,
            agent,
            limit,
        } => apply_skill_patch_candidate_command(config, agent, candidate, limit).await,
        crate::commands::SkillCommands::Versions {
            skill,
            agent,
            limit,
        } => show_skill_patch_versions_command(config, agent, skill, limit).await,
        crate::commands::SkillCommands::Rollback {
            rollback,
            agent,
            limit,
        } => rollback_skill_patch_command(config, agent, rollback, limit).await,
        crate::commands::SkillCommands::Autopromote {
            agent,
            limit,
            apply,
        } => auto_promote_skill_patches_command(config, agent, limit, apply).await,
        crate::commands::SkillCommands::Audit { source } => {
            let source_path = PathBuf::from(&source);
            let target = if source_path.exists() {
                source_path
            } else {
                skills_dir(workspace_dir).join(&source)
            };

            if !target.exists() {
                anyhow::bail!("Skill source or installed skill not found: {source}");
            }

            let report = audit::audit_skill_directory(&target)?;
            if report.is_clean() {
                println!(
                    "  {} Skill audit passed for {} ({} files scanned).",
                    console::style("✓").green().bold(),
                    target.display(),
                    report.files_scanned
                );
                return Ok(());
            }

            println!(
                "  {} Skill audit failed for {}",
                console::style("✗").red().bold(),
                target.display()
            );
            for finding in report.findings {
                println!("    - {finding}");
            }
            anyhow::bail!("Skill audit failed.");
        }
        crate::commands::SkillCommands::Install { source } => {
            println!("Installing skill from: {source}");

            let skills_path = skills_dir(workspace_dir);
            std::fs::create_dir_all(&skills_path)?;

            if is_git_source(&source) {
                let (installed_dir, files_scanned) =
                    install_git_skill_source(&source, &skills_path)
                        .with_context(|| format!("failed to install git skill source: {source}"))?;
                println!(
                    "  {} Skill installed and audited: {} ({} files scanned)",
                    console::style("✓").green().bold(),
                    installed_dir.display(),
                    files_scanned
                );
            } else {
                let (dest, files_scanned) = install_local_skill_source(&source, &skills_path)
                    .with_context(|| format!("failed to install local skill source: {source}"))?;
                println!(
                    "  {} Skill installed and audited: {} ({} files scanned)",
                    console::style("✓").green().bold(),
                    dest.display(),
                    files_scanned
                );
            }

            println!("  Security audit completed successfully.");
            Ok(())
        }
        crate::commands::SkillCommands::Remove { name } => {
            // Reject path traversal attempts
            if name.contains("..") || name.contains('/') || name.contains('\\') {
                anyhow::bail!("Invalid skill name: {name}");
            }

            let skill_path = skills_dir(workspace_dir).join(&name);

            // Verify the resolved path is actually inside the skills directory
            let canonical_skills = skills_dir(workspace_dir)
                .canonicalize()
                .unwrap_or_else(|_| skills_dir(workspace_dir));
            if let Ok(canonical_skill) = skill_path.canonicalize() {
                if !canonical_skill.starts_with(&canonical_skills) {
                    anyhow::bail!("Skill path escapes skills directory: {name}");
                }
            }

            if !skill_path.exists() {
                anyhow::bail!("Skill not found: {name}");
            }

            std::fs::remove_dir_all(&skill_path)?;
            println!(
                "  {} Skill '{}' removed.",
                console::style("✓").green().bold(),
                name
            );
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::similar_names)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use synapse_memory::{EpisodicMemoryPort, SkillMemoryPort, UnifiedMemoryPort};

    fn open_skills_env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_LOCK.get_or_init(|| Mutex::new(()))
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

    #[test]
    fn load_empty_skills_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills = load_skills(dir.path());
        assert!(skills.is_empty());
    }

    #[test]
    fn load_skill_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "test-skill"
description = "A test skill"
version = "1.0.0"
tags = ["test"]

[[tools]]
name = "hello"
description = "Says hello"
kind = "shell"
command = "echo hello"
"#,
        )
        .unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
        assert_eq!(skills[0].tools.len(), 1);
        assert_eq!(skills[0].tools[0].name, "hello");
    }

    #[test]
    fn load_skill_from_md() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("md-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            "# My Skill\nThis skill does cool things.\n",
        )
        .unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "md-skill");
        assert!(skills[0].description.contains("cool things"));
    }

    #[test]
    fn load_skill_from_md_frontmatter_uses_metadata_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("md-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Use this skill for PDFs\nversion: 1.2.3\nauthor: maintainer\ntags:\n  - docs\n  - pdf\n---\n# PDF Processing Guide\nExtract text carefully.\n",
        )
        .unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(skills[0].description, "Use this skill for PDFs");
        assert_eq!(skills[0].version, "1.2.3");
        assert_eq!(skills[0].author.as_deref(), Some("maintainer"));
        assert_eq!(skills[0].tags, vec!["docs", "pdf"]);
        assert!(skills[0].prompts[0].contains("# PDF Processing Guide"));
        assert!(!skills[0].prompts[0].contains("name: pdf"));
    }

    #[test]
    fn skills_to_prompt_empty() {
        let prompt = skills_to_prompt(&[], Path::new("/tmp"));
        assert!(prompt.is_empty());
    }

    #[test]
    fn skills_to_prompt_with_skills() {
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "A test".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![],
            prompts: vec!["Do the thing.".to_string()],
            location: None,
        }];
        let prompt = skills_to_prompt(&skills, Path::new("/tmp"));
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>test</name>"));
        assert!(prompt.contains("loaded on demand"));
        assert!(!prompt.contains("<instruction>Do the thing.</instruction>"));
    }

    #[test]
    fn skills_to_prompt_full_mode_includes_instructions() {
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "A test".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![],
            prompts: vec!["Do the thing.".to_string()],
            location: None,
        }];
        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp"),
            synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>test</name>"));
        assert!(prompt.contains("<instruction>Do the thing.</instruction>"));
    }

    #[test]
    fn skills_to_prompt_compact_mode_omits_instructions_and_tools() {
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "A test".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![SkillTool {
                name: "run".to_string(),
                description: "Run task".to_string(),
                kind: "shell".to_string(),
                command: "echo hi".to_string(),
                args: HashMap::new(),
            }],
            prompts: vec!["Do the thing.".to_string()],
            location: Some(PathBuf::from("/tmp/workspace/skills/test/SKILL.md")),
        }];
        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp/workspace"),
            synapse_domain::config::schema::SkillsPromptInjectionMode::Compact,
        );

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>test</name>"));
        assert!(prompt.contains("<location>skills/test/SKILL.md</location>"));
        assert!(prompt.contains("loaded on demand"));
        assert!(!prompt.contains("<instructions>"));
        assert!(!prompt.contains("<instruction>Do the thing.</instruction>"));
        assert!(!prompt.contains("<tools>"));
    }

    #[test]
    fn skills_to_prompt_compact_mode_dedupes_and_caps_catalog() {
        let mut skills = vec![
            Skill {
                name: "release-check".to_string(),
                description: "Imported release check".to_string(),
                version: "1.0.0".to_string(),
                author: Some("besoeasy/open-skills".to_string()),
                tags: vec!["open-skills".to_string()],
                tools: vec![],
                prompts: vec!["Imported instructions should not appear.".to_string()],
                location: Some(PathBuf::from(
                    "/tmp/open-skills/skills/release-check/SKILL.md",
                )),
            },
            Skill {
                name: "release-check".to_string(),
                description: "Manual release check".to_string(),
                version: "1.0.0".to_string(),
                author: None,
                tags: vec![],
                tools: vec![],
                prompts: vec!["Manual instructions should not appear.".to_string()],
                location: Some(PathBuf::from(
                    "/tmp/workspace/skills/release-check/SKILL.md",
                )),
            },
        ];
        for index in 0..9 {
            skills.push(Skill {
                name: format!("extra-{index}"),
                description: format!("Extra skill {index}"),
                version: "1.0.0".to_string(),
                author: None,
                tags: vec![],
                tools: vec![],
                prompts: vec![format!("Extra instructions {index}")],
                location: None,
            });
        }

        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp/workspace"),
            synapse_domain::config::schema::SkillsPromptInjectionMode::Compact,
        );

        assert_eq!(prompt.matches("  <skill>").count(), 8);
        assert!(prompt.contains("<description>Manual release check</description>"));
        assert!(!prompt.contains("<description>Imported release check</description>"));
        assert!(prompt.contains("<skills_omitted count=\"2\""));
        assert!(!prompt.contains("Manual instructions should not appear"));
        assert!(!prompt.contains("Imported instructions should not appear"));
        assert!(!prompt.contains("Extra instructions"));
    }

    #[test]
    fn skills_to_prompt_compact_mode_prioritizes_manual_before_cap() {
        let mut skills = Vec::new();
        for index in 0..9 {
            skills.push(Skill {
                name: format!("open-skill-{index}"),
                description: format!("Imported skill {index}"),
                version: "1.0.0".to_string(),
                author: Some("besoeasy/open-skills".to_string()),
                tags: vec!["open-skills".to_string()],
                tools: vec![],
                prompts: vec![format!("Imported instructions {index}")],
                location: Some(PathBuf::from(format!(
                    "/tmp/open-skills/skills/open-skill-{index}/SKILL.md"
                ))),
            });
        }
        skills.push(Skill {
            name: "workspace-critical".to_string(),
            description: "Workspace critical procedure".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![],
            prompts: vec!["Manual instructions should not appear.".to_string()],
            location: Some(PathBuf::from(
                "/tmp/workspace/skills/workspace-critical/SKILL.md",
            )),
        });

        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp/workspace"),
            synapse_domain::config::schema::SkillsPromptInjectionMode::Compact,
        );

        assert_eq!(prompt.matches("  <skill>").count(), 8);
        assert!(prompt.contains("<name>workspace-critical</name>"));
        assert!(prompt.contains("<description>Workspace critical procedure</description>"));
        assert!(prompt.contains("<origin>manual</origin>"));
        assert!(prompt.contains("<skills_omitted count=\"2\""));
        assert!(!prompt.contains("Manual instructions should not appear"));
        assert!(!prompt.contains("Imported instructions"));
    }

    #[test]
    fn init_skills_creates_readme() {
        let dir = tempfile::tempdir().unwrap();
        init_skills_dir(dir.path()).unwrap();
        assert!(dir.path().join("skills").join("README.md").exists());
    }

    #[test]
    fn init_skills_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        init_skills_dir(dir.path()).unwrap();
        init_skills_dir(dir.path()).unwrap(); // second call should not fail
        assert!(dir.path().join("skills").join("README.md").exists());
    }

    #[tokio::test]
    async fn create_user_authored_skill_stores_manual_active_skill() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();

        let outcome = create_user_authored_skill(
            &memory,
            "test",
            UserAuthoredSkillCreateRequest {
                name: "Matrix release check".into(),
                description: None,
                body: "# Matrix release check\n\nFind the local checkout and compare release tags."
                    .into(),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                tags: vec!["matrix".into()],
                status: SkillStatus::Active,
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome.origin, SkillOrigin::Manual);
        assert_eq!(outcome.status, SkillStatus::Active);
        assert_eq!(outcome.version, 1);
        assert_eq!(outcome.audit_files_scanned, 1);

        let stored = memory
            .get_skill_by_id(&outcome.skill_id, &"test".to_string())
            .await
            .unwrap()
            .expect("stored skill");
        assert_eq!(stored.origin, SkillOrigin::Manual);
        assert_eq!(stored.status, SkillStatus::Active);
        assert_eq!(stored.created_by, "test");
        assert!(stored.tags.contains(&"user-authored".to_string()));
        assert!(stored.content.contains("compare release tags"));

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = dir.path().join("workspace");
        let report =
            runtime_skill_resolution_report(&config, &memory, "test", Vec::new(), Vec::new(), 10)
                .await
                .unwrap();
        let decision = report
            .decisions
            .iter()
            .find(|decision| decision.name == "Matrix release check")
            .expect("manual skill governance decision");
        assert_eq!(
            decision.source,
            synapse_domain::application::services::skill_governance_service::SkillSource::Manual
        );
        assert_eq!(
            decision.state,
            synapse_domain::application::services::skill_governance_service::SkillRuntimeState::Active
        );
        assert!(decision.prompt_projection.is_some());

        let output = format_user_authored_skill_create_outcome_text(&outcome);
        assert!(output.contains("Created user-authored skill"));
        assert!(output.contains("origin=manual"));
    }

    #[tokio::test]
    async fn user_skill_update_records_version_and_rolls_back_manual_skill() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        let created = create_user_authored_skill(
            &memory,
            "test",
            UserAuthoredSkillCreateRequest {
                name: "Manual matrix skill".into(),
                description: Some("Original description".into()),
                body: "# Manual matrix skill\n\nOriginal body.".into(),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into()],
                tags: vec!["matrix".into()],
                status: SkillStatus::Active,
            },
        )
        .await
        .unwrap();

        let update = update_user_skill(
            &memory,
            "test",
            UserSkillUpdateRequest {
                skill_ref: created.skill_id.clone(),
                description: Some("Updated description".into()),
                body: Some("# Manual matrix skill\n\nUpdated body.".into()),
                task_family: Some("release-audit-v2".into()),
                tool_pattern: Some(vec!["repo_discovery".into(), "git_operations".into()]),
                tags: Some(vec!["matrix".into(), "updated".into()]),
                status: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(update.previous_version, 1);
        assert_eq!(update.new_version, 2);
        assert!(update.apply_record_id.starts_with("operator-update-"));
        let updated = memory
            .get_skill_by_id(&created.skill_id, &"test".to_string())
            .await
            .unwrap()
            .expect("updated skill");
        assert_eq!(updated.origin, SkillOrigin::Manual);
        assert_eq!(updated.version, 2);
        assert!(updated.content.contains("Updated body"));
        assert_eq!(updated.task_family.as_deref(), Some("release-audit-v2"));
        assert!(updated.tags.contains(&"updated".to_string()));

        let versions =
            format_skill_patch_versions_output(&memory, "test", Some(&created.skill_id), 10)
                .await
                .unwrap();
        assert!(versions.contains("Applied changes"));
        assert!(versions.contains("operator update"));

        let rollback = rollback_skill_patch(&memory, "test", &update.apply_record_id, 10)
            .await
            .unwrap();
        assert_eq!(rollback.from_version, 2);
        assert_eq!(rollback.restored_from_version, 1);
        assert_eq!(rollback.new_version, 3);
        let restored = memory
            .get_skill_by_id(&created.skill_id, &"test".to_string())
            .await
            .unwrap()
            .expect("restored skill");
        assert_eq!(restored.origin, SkillOrigin::Manual);
        assert_eq!(
            restored.content.trim_end(),
            "# Manual matrix skill\n\nOriginal body."
        );
        assert_eq!(restored.description, "Original description");
        assert_eq!(restored.version, 3);
    }

    #[test]
    fn scaffold_skill_package_creates_audited_bundle_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = scaffold_skill_package(
            dir.path(),
            SkillPackageScaffoldRequest {
                name: "manual-release-skill".into(),
                description: Some("Manual release workflow.".into()),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into()],
                tags: vec!["release".into()],
                overwrite: false,
            },
        )
        .unwrap();

        assert!(outcome.skill_file.exists());
        assert!(outcome.references_dir.is_dir());
        assert!(outcome.templates_dir.is_dir());
        assert!(outcome.assets_dir.is_dir());
        let body = std::fs::read_to_string(&outcome.skill_file).unwrap();
        assert!(body.contains("manual-release-skill"));
        assert!(body.contains("Manual release workflow."));
    }

    #[tokio::test]
    async fn deterministic_review_fixture_promotes_generated_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        let skill_id = memory
            .store_skill(MemorySkill {
                id: "generated-candidate".into(),
                name: "matrix-release-check".into(),
                description: "Generated candidate from repeated Matrix release checks.".into(),
                content: "# Matrix release check\n\nUse repo discovery, then compare tags.".into(),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                lineage_task_families: vec!["release-audit".into()],
                tags: vec!["generated".into()],
                success_count: 5,
                fail_count: 0,
                version: 1,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Candidate,
                created_by: "test".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let learned = memory.list_skills(&"test".to_string(), 10).await.unwrap();
        let decisions = review_learned_skills(&learned, &[]);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].skill_id, skill_id);
        assert_eq!(decisions[0].action, SkillReviewAction::PromoteToActive);

        let applied = apply_skill_review_decisions(&memory, "test", &decisions)
            .await
            .unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].target_status, SkillStatus::Active);
        assert_eq!(applied[0].reason, "repeated_successes");

        let promoted = memory
            .get_skill_by_id(&skill_id, &"test".to_string())
            .await
            .unwrap()
            .expect("promoted skill");
        assert_eq!(promoted.status, SkillStatus::Active);
        assert_eq!(promoted.version, 2);
    }

    #[tokio::test]
    async fn health_cleanup_apply_demotes_failure_dominant_learned_skill() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        memory
            .store_skill(MemorySkill {
                id: "failing-active".into(),
                name: "failing-active".into(),
                description: "Failure dominant learned skill.".into(),
                content: "# Failing active\n\nUse structured tools.".into(),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                lineage_task_families: vec!["release-audit".into()],
                tags: vec!["generated".into()],
                success_count: 1,
                fail_count: 3,
                version: 1,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Active,
                created_by: "test".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let output = format_skill_health_cleanup_output(&memory, "test", 10, 10, true)
            .await
            .unwrap();
        assert!(output.contains("Applied 1 cleanup decisions."));

        let updated = memory
            .get_skill_by_id(&"failing-active".to_string(), &"test".to_string())
            .await
            .unwrap()
            .expect("updated skill");
        assert_eq!(updated.status, SkillStatus::Candidate);
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn export_user_skill_package_writes_audited_loadable_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();

        let created = create_user_authored_skill(
            &memory,
            "test",
            UserAuthoredSkillCreateRequest {
                name: "Matrix release check".into(),
                description: Some("Compare a local Matrix checkout with upstream tags.".into()),
                body: "# Matrix release check\n\nUse repo discovery, then compare release tags."
                    .into(),
                task_family: Some("release-audit".into()),
                tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
                tags: vec!["matrix".into(), "release".into()],
                status: SkillStatus::Active,
            },
        )
        .await
        .unwrap();

        let exported = export_user_skill_package(
            &memory,
            "test",
            &workspace,
            UserSkillPackageExportRequest {
                skill_ref: created.skill_id.clone(),
                destination: None,
                package_name: Some("matrix-release-check".into()),
                overwrite: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            exported.package_dir,
            workspace.join("skills").join("matrix-release-check")
        );
        assert!(exported.skill_file.is_file());
        assert_eq!(exported.audit_files_scanned, 2);
        assert!(exported.diff_summary.starts_with("create SKILL.md"));
        let rendered = fs::read_to_string(&exported.skill_file).unwrap();
        assert!(rendered.contains("source_skill_id:"));
        assert!(rendered.contains("task_family: release-audit"));
        assert!(rendered.contains("- repo_discovery"));

        let loaded = load_skills(&workspace);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Matrix release check");
        assert_eq!(
            loaded[0].description,
            "Compare a local Matrix checkout with upstream tags."
        );
        assert!(loaded[0]
            .prompts
            .first()
            .is_some_and(|body| body.contains("compare release tags")));

        let updated = update_user_skill(
            &memory,
            "test",
            UserSkillUpdateRequest {
                skill_ref: created.skill_id.clone(),
                description: None,
                body: Some(
                    "# Matrix release check\n\nCompare release tags and verify changelog.".into(),
                ),
                task_family: None,
                tool_pattern: None,
                tags: None,
                status: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.new_version, 2);
        let overwritten = export_user_skill_package(
            &memory,
            "test",
            &workspace,
            UserSkillPackageExportRequest {
                skill_ref: created.skill_id.clone(),
                destination: None,
                package_name: Some("matrix-release-check".into()),
                overwrite: true,
            },
        )
        .await
        .unwrap();
        assert!(overwritten.diff_summary.starts_with("overwrite SKILL.md"));
        let overwritten_body = fs::read_to_string(&overwritten.skill_file).unwrap();
        assert!(overwritten_body.contains("verify changelog"));
    }

    #[tokio::test]
    async fn ports_workspace_skill_package_to_memory_and_moves_source() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let package_dir = workspace.join("skills").join("legacy-release-check");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(
            package_dir.join("SKILL.md"),
            "---\nname: legacy-release-check\ndescription: Legacy package skill.\ntags:\n  - release\n---\n# Legacy release check\n\nCompare local tags with upstream.\n",
        )
        .unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();

        let report = port_workspace_skill_packages_to_memory(&memory, "test", &workspace)
            .await
            .unwrap();

        assert_eq!(report.scanned, 1);
        assert_eq!(report.imported, 1);
        assert_eq!(report.moved, 1);
        assert!(!package_dir.exists());
        assert!(workspace
            .join("skills")
            .join(PORTED_SKILLS_DIR_NAME)
            .join("legacy-release-check")
            .join("SKILL.md")
            .is_file());
        let stored = memory
            .get_skill("legacy-release-check", &"test".to_string())
            .await
            .unwrap()
            .expect("ported memory skill");
        assert_eq!(stored.origin, SkillOrigin::Manual);
        assert_eq!(stored.status, SkillStatus::Active);
        assert!(stored.tags.iter().any(|tag| tag == "ported-skill-package"));
        assert!(stored.content.contains("Compare local tags"));
        assert!(load_skills(&workspace).is_empty());

        let second_report = port_workspace_skill_packages_to_memory(&memory, "test", &workspace)
            .await
            .unwrap();
        assert_eq!(second_report.scanned, 0);
        assert_eq!(second_report.imported, 0);
        assert_eq!(second_report.moved, 0);
        let stored_again = memory
            .list_skills(&"test".to_string(), 10)
            .await
            .unwrap()
            .into_iter()
            .filter(|skill| skill.name == "legacy-release-check")
            .collect::<Vec<_>>();
        assert_eq!(stored_again.len(), 1);
    }

    #[tokio::test]
    async fn ports_nested_loose_and_existing_workspace_skill_sources() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let skills_dir = workspace.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        let loose_file = skills_dir.join("loose-release-check.md");
        fs::write(
            &loose_file,
            "---\nname: loose-release-check\ndescription: Loose markdown skill.\n---\n# Loose release check\n\nCompare tags from a loose file.\n",
        )
        .unwrap();

        let nested_package = skills_dir.join("nested").join("matrix-release-check");
        fs::create_dir_all(&nested_package).unwrap();
        fs::write(
            nested_package.join("SKILL.md"),
            "---\nname: nested-matrix-release-check\ndescription: Nested package skill.\n---\n# Nested release check\n\nCompare Matrix tags.\n",
        )
        .unwrap();

        let existing_package = skills_dir.join("existing-release-check");
        fs::create_dir_all(&existing_package).unwrap();
        fs::write(
            existing_package.join("SKILL.md"),
            "---\nname: existing-release-check\ndescription: Already imported skill.\n---\n# Existing release check\n\nShould move without duplicate import.\n",
        )
        .unwrap();

        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        memory
            .store_skill(MemorySkill {
                id: "existing-release-check".into(),
                name: "existing-release-check".into(),
                description: "Already imported skill.".into(),
                content: "# Existing release check".into(),
                task_family: None,
                tool_pattern: Vec::new(),
                lineage_task_families: Vec::new(),
                tags: Vec::new(),
                success_count: 0,
                fail_count: 0,
                version: 1,
                origin: SkillOrigin::Manual,
                status: SkillStatus::Active,
                created_by: "test".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let report = port_workspace_skill_packages_to_memory(&memory, "test", &workspace)
            .await
            .unwrap();

        assert_eq!(report.scanned, 3);
        assert_eq!(report.imported, 2);
        assert_eq!(report.skipped_existing, 1);
        assert_eq!(report.moved, 3);
        assert!(!loose_file.exists());
        assert!(!nested_package.exists());
        assert!(!existing_package.exists());
        assert!(skills_dir
            .join(PORTED_SKILLS_DIR_NAME)
            .join("loose-release-check.md")
            .is_file());
        assert!(skills_dir
            .join(PORTED_SKILLS_DIR_NAME)
            .join("matrix-release-check")
            .join("SKILL.md")
            .is_file());
        assert!(skills_dir
            .join(PORTED_SKILLS_DIR_NAME)
            .join("existing-release-check")
            .join("SKILL.md")
            .is_file());
        assert!(memory
            .get_skill("loose-release-check", &"test".to_string())
            .await
            .unwrap()
            .is_some());
        assert!(memory
            .get_skill("nested-matrix-release-check", &"test".to_string())
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn apply_skill_patch_candidate_updates_skill_and_records_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        memory
            .store_skill(MemorySkill {
                id: "skill-a".into(),
                name: "Skill A".into(),
                description: "test".into(),
                content: "# Skill\n\nOriginal body".into(),
                task_family: Some("test".into()),
                tool_pattern: vec!["probe".into()],
                lineage_task_families: Vec::new(),
                tags: Vec::new(),
                success_count: 1,
                fail_count: 0,
                version: 1,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Active,
                created_by: "test".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let target = memory
            .list_skills(&"test".to_string(), 10)
            .await
            .unwrap()
            .into_iter()
            .find(|skill| skill.name == "Skill A")
            .expect("stored target skill");
        let candidate = SkillPatchCandidate {
            id: "patch-a".into(),
            target_skill_id: target.id.clone(),
            target_version: target.version,
            diff_summary: "add repair note".into(),
            proposed_body: "# Skill\n\nOriginal body\n\nUse probe repair guidance.".into(),
            procedure_claims: vec![SkillPatchProcedureClaim {
                tool_name: "probe".into(),
                failure_kind: "schema_mismatch".into(),
                suggested_action: "adjust_arguments_or_target".into(),
            }],
            provenance: vec![synapse_domain::application::services::skill_governance_service::SkillEvidenceRef {
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
            replay_criteria: vec!["replay probe".into(), "compare with-skill".into()],
            eval_results: vec![
                synapse_domain::application::services::skill_governance_service::SkillReplayEvalResult {
                    criterion: "replay probe".into(),
                    status: SkillReplayEvalStatus::Passed,
                    evidence: Some("passed replay".into()),
                    observed_at_unix: 100,
                },
                synapse_domain::application::services::skill_governance_service::SkillReplayEvalResult {
                    criterion: "compare with-skill".into(),
                    status: SkillReplayEvalStatus::Passed,
                    evidence: Some("passed comparison".into()),
                    observed_at_unix: 100,
                },
            ],
            status: SkillStatus::Candidate,
        };
        memory
            .store_episode(
                skill_patch_candidate_service::skill_patch_candidate_to_memory_entry(
                    &candidate, now,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let outcome = apply_skill_patch_candidate(&memory, "test", "patch-a", 10)
            .await
            .unwrap();

        assert_eq!(outcome.previous_version, 1);
        assert_eq!(outcome.new_version, 2);
        let updated = memory
            .list_skills(&"test".to_string(), 10)
            .await
            .unwrap()
            .into_iter()
            .find(|skill| skill.id == target.id)
            .expect("updated target skill");
        assert_eq!(updated.content, candidate.proposed_body);
        assert_eq!(updated.version, 2);
        let rollback = memory
            .get_skill("Skill A rollback before patch-a", &"test".to_string())
            .await
            .unwrap()
            .expect("rollback snapshot");
        assert_eq!(rollback.status, SkillStatus::Deprecated);
        assert_eq!(rollback.content, "# Skill\n\nOriginal body");

        let apply_entries = memory
            .list(
                Some(&skill_patch_candidate_service::skill_patch_apply_memory_category()),
                None,
                10,
            )
            .await
            .unwrap();
        let record = apply_entries
            .iter()
            .find_map(skill_patch_candidate_service::parse_skill_patch_apply_entry)
            .expect("apply audit record");
        assert_eq!(record.candidate_id, "patch-a");
        assert_eq!(record.rollback_skill_id, outcome.rollback_skill_id);

        let versions_output =
            format_skill_patch_versions_output(&memory, "test", Some(&target.id), 10)
                .await
                .unwrap();
        assert!(versions_output.contains("Applied changes"));
        assert!(versions_output.contains(&record.id));

        let rollback = rollback_skill_patch(&memory, "test", &record.id, 10)
            .await
            .unwrap();
        assert_eq!(rollback.from_version, 2);
        assert_eq!(rollback.restored_from_version, 1);
        assert_eq!(rollback.new_version, 3);
        let restored = memory
            .get_skill_by_id(&target.id, &"test".to_string())
            .await
            .unwrap()
            .expect("restored target skill");
        assert_eq!(restored.content, "# Skill\n\nOriginal body");
        assert_eq!(restored.version, 3);

        let rollback_entries = memory
            .list(
                Some(&skill_patch_candidate_service::skill_patch_rollback_memory_category()),
                None,
                10,
            )
            .await
            .unwrap();
        let rollback_record = rollback_entries
            .iter()
            .find_map(skill_patch_candidate_service::parse_skill_patch_rollback_entry)
            .expect("rollback audit record");
        assert_eq!(rollback_record.apply_record_id, record.id);
        assert_eq!(rollback_record.rollback_skill_id, outcome.rollback_skill_id);
    }

    #[tokio::test]
    async fn auto_promotion_applies_only_when_policy_and_live_traces_pass() {
        let dir = tempfile::tempdir().unwrap();
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        memory
            .store_skill(MemorySkill {
                id: "skill-auto".into(),
                name: "Auto Skill".into(),
                description: "test".into(),
                content: "# Skill\n\nOriginal body".into(),
                task_family: Some("test".into()),
                tool_pattern: vec!["probe".into()],
                lineage_task_families: Vec::new(),
                tags: Vec::new(),
                success_count: 2,
                fail_count: 0,
                version: 1,
                origin: SkillOrigin::Learned,
                status: SkillStatus::Active,
                created_by: "test".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let target = memory
            .list_skills(&"test".to_string(), 10)
            .await
            .unwrap()
            .into_iter()
            .find(|skill| skill.name == "Auto Skill")
            .expect("stored target skill");
        let candidate = SkillPatchCandidate {
            id: "patch-auto".into(),
            target_skill_id: target.id.clone(),
            target_version: target.version,
            diff_summary: "add verified procedure".into(),
            proposed_body: "# Skill\n\nOriginal body\n\nUse verified probe procedure.".into(),
            procedure_claims: vec![SkillPatchProcedureClaim {
                tool_name: "probe".into(),
                failure_kind: "schema_mismatch".into(),
                suggested_action: "adjust_arguments_or_target".into(),
            }],
            provenance: vec![synapse_domain::application::services::skill_governance_service::SkillEvidenceRef {
                kind: synapse_domain::application::services::skill_governance_service::SkillEvidenceKind::RepairTrace,
                id: "repair-auto".into(),
                summary: None,
                metadata: serde_json::json!({
                    "repair_outcome": "resolved",
                    "tool_name": "probe",
                    "failure_kind": "schema_mismatch",
                    "suggested_action": "adjust_arguments_or_target",
                }),
            }],
            replay_criteria: vec!["replay probe".into(), "compare with-skill".into()],
            eval_results: vec![
                synapse_domain::application::services::skill_governance_service::SkillReplayEvalResult {
                    criterion: "replay probe".into(),
                    status: SkillReplayEvalStatus::Passed,
                    evidence: Some("passed replay".into()),
                    observed_at_unix: 100,
                },
                synapse_domain::application::services::skill_governance_service::SkillReplayEvalResult {
                    criterion: "compare with-skill".into(),
                    status: SkillReplayEvalStatus::Passed,
                    evidence: Some("passed comparison".into()),
                    observed_at_unix: 100,
                },
            ],
            status: SkillStatus::Candidate,
        };
        memory
            .store_episode(
                skill_patch_candidate_service::skill_patch_candidate_to_memory_entry(
                    &candidate, now,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        for idx in 0..2 {
            let trace = SkillUseTrace {
                id: format!("live-success-{idx}"),
                skill_id: target.id.clone(),
                task_family: Some("test".into()),
                route_model: Some("test-model".into()),
                tool_pattern: vec!["probe".into()],
                outcome: SkillUseOutcome::Succeeded,
                verification: Some("live run succeeded".into()),
                repair_evidence: Vec::new(),
                observed_at_unix: 1_000 + idx,
            };
            memory
                .store_episode(
                    synapse_domain::application::services::skill_trace_service::skill_use_trace_to_memory_entry(
                        "test",
                        &trace,
                        now + chrono::Duration::seconds(idx),
                        None,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }

        let disabled_policy = SkillPatchAutoPromotionPolicy {
            enabled: false,
            ..SkillPatchAutoPromotionPolicy::default()
        };
        let disabled_run =
            run_skill_patch_auto_promotion(&memory, "test", &disabled_policy, 10, true)
                .await
                .unwrap();
        assert!(disabled_run.applied_patches.is_empty());
        assert!(format_skill_auto_promotion_run_text(&disabled_run)
            .contains("[skills.auto_promotion].enabled is false"));

        let enabled_policy = SkillPatchAutoPromotionPolicy {
            enabled: true,
            min_successful_live_traces: 2,
            trace_window_limit: 5,
            max_recent_blocking_traces: 0,
        };
        let dry_run = run_skill_patch_auto_promotion(&memory, "test", &enabled_policy, 10, false)
            .await
            .unwrap();
        assert!(dry_run
            .evaluations
            .iter()
            .filter_map(|evaluation| evaluation.report.as_ref())
            .any(|report| report.auto_promotion_allowed));
        assert!(format_skill_auto_promotion_run_text(&dry_run).contains("eligible"));

        let applied = run_skill_patch_auto_promotion(&memory, "test", &enabled_policy, 10, true)
            .await
            .unwrap();
        assert_eq!(applied.applied_patches.len(), 1);
        let updated = memory
            .get_skill_by_id(&target.id, &"test".to_string())
            .await
            .unwrap()
            .expect("updated target skill");
        assert_eq!(updated.content, candidate.proposed_body);
        assert_eq!(updated.version, 2);
    }

    #[test]
    fn load_nonexistent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("nonexistent");
        let skills = load_skills(&fake);
        assert!(skills.is_empty());
    }

    #[test]
    fn load_ignores_files_in_skills_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        // A file, not a directory — should be ignored
        fs::write(skills_dir.join("not-a-skill.txt"), "hello").unwrap();
        let skills = load_skills(dir.path());
        assert!(skills.is_empty());
    }

    #[test]
    fn load_ignores_dir_without_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let empty_skill = skills_dir.join("empty-skill");
        fs::create_dir_all(&empty_skill).unwrap();
        // Directory exists but no SKILL.toml or SKILL.md
        let skills = load_skills(dir.path());
        assert!(skills.is_empty());
    }

    #[test]
    fn load_multiple_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");

        for name in ["alpha", "beta", "gamma"] {
            let skill_dir = skills_dir.join(name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("# {name}\nSkill {name} description.\n"),
            )
            .unwrap();
        }

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 3);
    }

    #[test]
    fn toml_skill_with_multiple_tools() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("multi-tool");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "multi-tool"
description = "Has many tools"
version = "2.0.0"
author = "tester"
tags = ["automation", "devops"]

[[tools]]
name = "build"
description = "Build the project"
kind = "shell"
command = "cargo build"

[[tools]]
name = "test"
description = "Run tests"
kind = "shell"
command = "cargo test"

[[tools]]
name = "deploy"
description = "Deploy via HTTP"
kind = "http"
command = "https://api.example.com/deploy"
"#,
        )
        .unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        let s = &skills[0];
        assert_eq!(s.name, "multi-tool");
        assert_eq!(s.version, "2.0.0");
        assert_eq!(s.author.as_deref(), Some("tester"));
        assert_eq!(s.tags, vec!["automation", "devops"]);
        assert_eq!(s.tools.len(), 3);
        assert_eq!(s.tools[0].name, "build");
        assert_eq!(s.tools[1].kind, "shell");
        assert_eq!(s.tools[2].kind, "http");
    }

    #[test]
    fn toml_skill_minimal() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("minimal");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            r#"
[skill]
name = "minimal"
description = "Bare minimum"
"#,
        )
        .unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].version, "0.1.0"); // default version
        assert!(skills[0].author.is_none());
        assert!(skills[0].tags.is_empty());
        assert!(skills[0].tools.is_empty());
    }

    #[test]
    fn toml_skill_invalid_syntax_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("broken");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(skill_dir.join("SKILL.toml"), "this is not valid toml {{{{").unwrap();

        let skills = load_skills(dir.path());
        assert!(skills.is_empty()); // broken skill is skipped
    }

    #[test]
    fn md_skill_heading_only() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("heading-only");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(skill_dir.join("SKILL.md"), "# Just a Heading\n").unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "No description");
    }

    #[test]
    fn skills_to_prompt_includes_tools() {
        let skills = vec![Skill {
            name: "weather".to_string(),
            description: "Get weather".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![SkillTool {
                name: "get_weather".to_string(),
                description: "Fetch forecast".to_string(),
                kind: "shell".to_string(),
                command: "curl wttr.in".to_string(),
                args: HashMap::new(),
            }],
            prompts: vec![],
            location: None,
        }];
        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp"),
            synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.contains("weather"));
        assert!(prompt.contains("<name>get_weather</name>"));
        assert!(prompt.contains("<description>Fetch forecast</description>"));
        assert!(prompt.contains("<kind>shell</kind>"));
    }

    #[test]
    fn skills_to_prompt_escapes_xml_content() {
        let skills = vec![Skill {
            name: "xml<skill>".to_string(),
            description: "A & B".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            tags: vec![],
            tools: vec![],
            prompts: vec!["Use <tool> & check \"quotes\".".to_string()],
            location: None,
        }];

        let prompt = skills_to_prompt_with_mode(
            &skills,
            Path::new("/tmp"),
            synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
        );
        assert!(prompt.contains("<name>xml&lt;skill&gt;</name>"));
        assert!(prompt.contains("<description>A &amp; B</description>"));
        assert!(prompt.contains(
            "<instruction>Use &lt;tool&gt; &amp; check &quot;quotes&quot;.</instruction>"
        ));
    }

    #[test]
    fn git_source_detection_accepts_remote_protocols_and_scp_style() {
        let sources = [
            "https://github.com/some-org/some-skill.git",
            "http://github.com/some-org/some-skill.git",
            "ssh://git@github.com/some-org/some-skill.git",
            "git://github.com/some-org/some-skill.git",
            "git@github.com:some-org/some-skill.git",
            "git@localhost:skills/some-skill.git",
        ];

        for source in sources {
            assert!(
                is_git_source(source),
                "expected git source detection for '{source}'"
            );
        }
    }

    #[test]
    fn git_source_detection_rejects_local_paths_and_invalid_inputs() {
        let sources = [
            "./skills/local-skill",
            "/tmp/skills/local-skill",
            "C:\\skills\\local-skill",
            "git@github.com",
            "ssh://",
            "not-a-url",
            "dir/git@github.com:org/repo.git",
        ];

        for source in sources {
            assert!(
                !is_git_source(source),
                "expected local/invalid source detection for '{source}'"
            );
        }
    }

    #[test]
    fn skills_dir_path() {
        let base = std::path::Path::new("/home/user/.synapseclaw");
        let dir = skills_dir(base);
        assert_eq!(dir, PathBuf::from("/home/user/.synapseclaw/skills"));
    }

    #[test]
    fn toml_prefers_over_md() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("dual");
        fs::create_dir_all(&skill_dir).unwrap();

        fs::write(
            skill_dir.join("SKILL.toml"),
            "[skill]\nname = \"from-toml\"\ndescription = \"TOML wins\"\n",
        )
        .unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# From MD\nMD description\n").unwrap();

        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "from-toml"); // TOML takes priority
    }

    #[test]
    fn open_skills_enabled_resolution_prefers_env_then_config_then_default_false() {
        assert!(!open_skills_enabled_from_sources(None, None));
        assert!(open_skills_enabled_from_sources(Some(true), None));
        assert!(!open_skills_enabled_from_sources(Some(true), Some("0")));
        assert!(open_skills_enabled_from_sources(Some(false), Some("yes")));
        // Invalid env values should fall back to config.
        assert!(open_skills_enabled_from_sources(
            Some(true),
            Some("invalid")
        ));
        assert!(!open_skills_enabled_from_sources(
            Some(false),
            Some("invalid")
        ));
    }

    #[test]
    fn resolve_open_skills_dir_resolution_prefers_env_then_config_then_home() {
        let home = Path::new("/tmp/home-dir");
        assert_eq!(
            resolve_open_skills_dir_from_sources(
                Some("/tmp/env-skills"),
                Some("/tmp/config"),
                Some(home)
            ),
            Some(PathBuf::from("/tmp/env-skills"))
        );
        assert_eq!(
            resolve_open_skills_dir_from_sources(
                Some("   "),
                Some("/tmp/config-skills"),
                Some(home)
            ),
            Some(PathBuf::from("/tmp/config-skills"))
        );
        assert_eq!(
            resolve_open_skills_dir_from_sources(None, None, Some(home)),
            Some(PathBuf::from("/tmp/home-dir/open-skills"))
        );
        assert_eq!(resolve_open_skills_dir_from_sources(None, None, None), None);
    }

    #[test]
    fn load_skills_with_config_reads_open_skills_dir_without_network() {
        let _env_guard = open_skills_env_lock().lock().unwrap();
        let _enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let _dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        fs::create_dir_all(workspace_dir.join("skills")).unwrap();

        let open_skills_dir = dir.path().join("open-skills-local");
        fs::create_dir_all(open_skills_dir.join("skills/http_request")).unwrap();
        fs::write(open_skills_dir.join("README.md"), "# open skills\n").unwrap();
        fs::write(
            open_skills_dir.join("CONTRIBUTING.md"),
            "# contribution guide\n",
        )
        .unwrap();
        fs::write(
            open_skills_dir.join("skills/http_request/SKILL.md"),
            "# HTTP request\nFetch API responses.\n",
        )
        .unwrap();

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = workspace_dir.clone();
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());

        let skills = load_skills_with_config(&workspace_dir, &config);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "http_request");
        assert_ne!(skills[0].name, "CONTRIBUTING");
    }

    #[test]
    fn file_backed_runtime_skills_keep_imported_when_workspace_porting_enabled() {
        let _env_guard = open_skills_env_lock().lock().unwrap();
        let _enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let _dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        let workspace_skill = workspace_dir.join("skills/release-audit");
        fs::create_dir_all(&workspace_skill).unwrap();
        fs::write(
            workspace_skill.join("SKILL.md"),
            "---\nname: release-audit\ndescription: Workspace release audit.\n---\n# Release Audit\nWorkspace-only procedure.\n",
        )
        .unwrap();

        let open_skills_dir = dir.path().join("open-skills-local");
        fs::create_dir_all(open_skills_dir.join("skills/pdf")).unwrap();
        fs::write(
            open_skills_dir.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: Imported PDF workflow.\n---\n# PDF\nInspect PDF documents.\n",
        )
        .unwrap();

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = workspace_dir.clone();
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());
        config.skills.port_workspace_packages_on_start = true;

        let skills = load_file_backed_runtime_skills(&workspace_dir, &config);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(infer_loaded_skill_origin(&skills[0]), "imported");
        assert!(!skills.iter().any(|skill| skill.name == "release-audit"));
    }

    #[tokio::test]
    async fn sync_file_backed_skill_index_stores_compact_imported_card() {
        let _env_guard = open_skills_env_lock().lock().unwrap();
        let _enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let _dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        fs::create_dir_all(workspace_dir.join("skills")).unwrap();
        let open_skills_dir = dir.path().join("open-skills-local");
        fs::create_dir_all(open_skills_dir.join("skills/pdf")).unwrap();
        fs::write(
            open_skills_dir.join("skills/pdf/SKILL.md"),
            format!(
                "---\nname: pdf\ndescription: Imported PDF workflow.\n---\n# PDF\n{}tail-marker\n",
                "Inspect PDF documents safely. ".repeat(120)
            ),
        )
        .unwrap();

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = workspace_dir.clone();
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());
        config.skills.port_workspace_packages_on_start = true;
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();

        let report =
            sync_file_backed_skill_index_to_memory(&memory, "test", &workspace_dir, &config)
                .await
                .unwrap();

        assert_eq!(report.scanned, 1);
        assert_eq!(report.indexed, 1);
        let stored = memory
            .get_skill("pdf", &"test".to_string())
            .await
            .unwrap()
            .expect("indexed imported package skill");
        assert_eq!(stored.origin, SkillOrigin::Imported);
        assert_eq!(stored.status, SkillStatus::Active);
        assert!(stored
            .tags
            .iter()
            .any(|tag| tag == FILE_BACKED_SKILL_INDEX_TAG));
        assert!(stored
            .tags
            .iter()
            .any(|tag| tag.starts_with(FILE_BACKED_SKILL_SOURCE_REF_TAG_PREFIX)));
        assert!(stored
            .tags
            .iter()
            .any(|tag| tag.starts_with(FILE_BACKED_SKILL_CONTENT_HASH_TAG_PREFIX)));
        assert!(stored.content.contains("[package-skill-index]"));
        assert!(stored.content.contains("activation: use skill_read"));
        assert!(!stored.content.contains("tail-marker"));
    }

    #[tokio::test]
    async fn sync_file_backed_skill_index_deprecates_stale_imported_card() {
        let _env_guard = open_skills_env_lock().lock().unwrap();
        let _enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let _dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        fs::create_dir_all(workspace_dir.join("skills")).unwrap();
        let open_skills_dir = dir.path().join("open-skills-local");
        fs::create_dir_all(open_skills_dir.join("skills/pdf")).unwrap();
        fs::write(
            open_skills_dir.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: Imported PDF workflow.\n---\n# PDF\nInspect PDF documents safely.\n",
        )
        .unwrap();

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = workspace_dir.clone();
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());
        config.skills.port_workspace_packages_on_start = true;
        let memory = synapse_memory::SurrealMemoryAdapter::new(
            &dir.path().join("memory.surreal").to_string_lossy(),
            std::sync::Arc::new(synapse_memory::embeddings::NoopEmbedding),
            "test".into(),
        )
        .await
        .unwrap();

        sync_file_backed_skill_index_to_memory(&memory, "test", &workspace_dir, &config)
            .await
            .unwrap();
        config.skills.open_skills_enabled = false;
        let report =
            sync_file_backed_skill_index_to_memory(&memory, "test", &workspace_dir, &config)
                .await
                .unwrap();

        assert_eq!(report.scanned, 0);
        assert_eq!(report.deprecated_stale, 1);
        let stored = memory
            .get_skill("pdf", &"test".to_string())
            .await
            .unwrap()
            .expect("stale indexed imported package skill");
        assert_eq!(stored.status, SkillStatus::Deprecated);
    }

    #[test]
    fn gateway_skill_list_response_deserializes_patch_candidates() {
        let response: GatewaySkillListResponse = serde_json::from_value(serde_json::json!({
            "agent_id": "agent",
            "skills": [],
            "patch_candidates": [{
                "id": "patch-skill-1",
                "target_skill_id": "skill-1",
                "target_version": 2,
                "diff_summary": "Add repair guidance",
                "replay_criteria": ["replay successful repair trace"],
                "status": "candidate"
            }]
        }))
        .unwrap();

        assert_eq!(response.agent_id, "agent");
        assert!(response.skills.is_empty());
        assert_eq!(response.patch_candidates.len(), 1);
        assert_eq!(response.patch_candidates[0].target_skill_id, "skill-1");
        assert_eq!(response.patch_candidates[0].target_version, 2);
    }

    #[test]
    fn load_open_skill_md_frontmatter_uses_metadata_and_strips_block() {
        let _env_guard = open_skills_env_lock().lock().unwrap();
        let _enabled_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        let _dir_guard = EnvVarGuard::unset("SYNAPSECLAW_OPEN_SKILLS_DIR");

        let dir = tempfile::tempdir().unwrap();
        let workspace_dir = dir.path().join("workspace");
        fs::create_dir_all(workspace_dir.join("skills")).unwrap();

        let open_skills_dir = dir.path().join("open-skills-local");
        fs::create_dir_all(open_skills_dir.join("skills/pdf")).unwrap();
        fs::write(
            open_skills_dir.join("skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: Use this skill whenever the user needs PDF help.\nauthor: community\ntags:\n  - parser\n---\n# PDF Guide\nInspect files safely.\n",
        )
        .unwrap();

        let mut config = synapse_domain::config::schema::Config::default();
        config.workspace_dir = workspace_dir.clone();
        config.skills.open_skills_enabled = true;
        config.skills.open_skills_dir = Some(open_skills_dir.to_string_lossy().to_string());

        let skills = load_skills_with_config(&workspace_dir, &config);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "pdf");
        assert_eq!(
            skills[0].description,
            "Use this skill whenever the user needs PDF help."
        );
        assert_eq!(skills[0].author.as_deref(), Some("community"));
        assert!(skills[0].tags.iter().any(|tag| tag == "parser"));
        assert!(skills[0].tags.iter().any(|tag| tag == "open-skills"));
        assert!(skills[0].prompts[0].contains("# PDF Guide"));
        assert!(!skills[0].prompts[0].contains("description: Use this skill"));
    }
}

#[cfg(test)]
mod symlink_tests;
