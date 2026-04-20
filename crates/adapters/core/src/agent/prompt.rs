use crate::skills::Skill;
use crate::tools::Tool;
use anyhow::{Context, Result};
use chrono::Local;
use std::fmt::Write;
use std::path::Path;
use synapse_domain::config::schema::IdentityConfig;
use synapse_infra::identity;

const BOOTSTRAP_MAX_CHARS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSectionStats {
    pub name: String,
    pub chars: usize,
}

pub struct PromptContext<'a> {
    pub workspace_dir: &'a Path,
    pub model_name: &'a str,
    pub tools: &'a [Box<dyn Tool>],
    pub skills: &'a [Skill],
    pub skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode,
    pub identity_config: Option<&'a IdentityConfig>,
    pub dispatcher_instructions: &'a str,
    pub tool_specs_are_out_of_band: bool,
}

pub trait PromptSection: Send + Sync {
    fn name(&self) -> &str;
    fn build(&self, ctx: &PromptContext<'_>) -> Result<String>;
}

#[derive(Default)]
pub struct SystemPromptBuilder {
    sections: Vec<Box<dyn PromptSection>>,
}

impl SystemPromptBuilder {
    pub fn with_defaults() -> Self {
        Self {
            sections: vec![
                Box::new(EphemeralAgentSection),
                Box::new(IdentitySection),
                Box::new(ToolsSection),
                Box::new(SafetySection),
                Box::new(SkillsSection),
                Box::new(WorkspaceSection),
                Box::new(DateTimeSection),
                Box::new(RuntimeSection),
                Box::new(ChannelMediaSection),
            ],
        }
    }

    pub fn add_section(mut self, section: Box<dyn PromptSection>) -> Self {
        self.sections.push(section);
        self
    }

    pub fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        Ok(self.build_with_stats(ctx)?.0)
    }

    pub fn build_with_stats(
        &self,
        ctx: &PromptContext<'_>,
    ) -> Result<(String, Vec<PromptSectionStats>)> {
        let mut output = String::new();
        let mut stats = Vec::new();
        for section in &self.sections {
            let part = section.build(ctx)?;
            if part.trim().is_empty() {
                continue;
            }
            stats.push(PromptSectionStats {
                name: section.name().to_string(),
                chars: part.chars().count(),
            });
            output.push_str(part.trim_end());
            output.push_str("\n\n");
        }
        Ok((output, stats))
    }
}

pub struct EphemeralAgentSection;
pub struct IdentitySection;
pub struct ToolsSection;
pub struct SafetySection;
pub struct SkillsSection;
pub struct WorkspaceSection;
pub struct RuntimeSection;
pub struct DateTimeSection;
pub struct ChannelMediaSection;

/// Injects ephemeral agent context when this process was spawned by a parent
/// agent via `agents_spawn`. Detected by the presence of `SYNAPSECLAW_SESSION_ID`
/// env var (set by the parent's subprocess launcher).
impl PromptSection for EphemeralAgentSection {
    fn name(&self) -> &str {
        "ephemeral_agent"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        let session_id = match std::env::var("SYNAPSECLAW_SESSION_ID") {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(String::new()),
        };
        let agent_id = std::env::var("SYNAPSECLAW_AGENT_ID").unwrap_or_default();
        let reply_to = std::env::var("SYNAPSECLAW_REPLY_TO").unwrap_or_else(|_| "parent".into());

        let mut prompt = String::new();

        // Workload prompt template (operator-defined preamble for this workload type)
        if let Ok(template) = std::env::var("SYNAPSECLAW_PROMPT_TEMPLATE") {
            let template = template.trim();
            if !template.is_empty() {
                prompt.push_str(template);
                prompt.push_str("\n\n");
            }
        }

        prompt.push_str("## Ephemeral Agent Context\n\n");
        write!(
            prompt,
            "You are an ephemeral agent (id: {agent_id}) spawned to handle a specific task.\n\
             Session: {session_id}\n\
             Parent: {reply_to}\n\n\
             When you have completed your task, use the `agents_reply` tool to send your \
             result back to the parent agent.\n\n\
             Important:\n\
             - You are a disposable worker — focus on the task, produce a result, then reply.\n\
             - Use `agents_reply` with `to` = \"{reply_to}\" and `session_id` = \"{session_id}\".\n\
             - Your identity and token will be revoked after you reply or timeout.\n"
        )?;

        Ok(prompt)
    }
}

impl PromptSection for IdentitySection {
    fn name(&self) -> &str {
        "identity"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let mut prompt = String::from("## Project Context\n\n");
        let mut uses_aieos_identity = false;
        if let Some(config) = ctx.identity_config {
            if identity::is_aieos_configured(config) {
                let aieos = identity::load_aieos_identity(config, ctx.workspace_dir)
                    .context("failed to load configured AIEOS identity")?
                    .ok_or_else(|| anyhow::anyhow!("configured AIEOS identity was not loaded"))?;
                let rendered = identity::aieos_to_system_prompt(&aieos);
                if rendered.trim().is_empty() {
                    anyhow::bail!("configured AIEOS identity rendered an empty prompt");
                }
                prompt.push_str(&rendered);
                prompt.push_str("\n\n");
                uses_aieos_identity = true;
            }
        }

        if !uses_aieos_identity {
            prompt.push_str(
                "Structured core memory and turn context carry persona, user preferences, and task state.\n\
                 Only static identity metadata is injected below.\n\
                 Do NOT use `file_read` or `file_edit` on workspace bootstrap docs unless the user explicitly asks to inspect or edit them.\n\
                 For durable preferences, task state, and long-term memory, prefer `core_memory_update`, `memory_store`, `memory_recall`, and `user_profile`.\n\n",
            );
            for file in ["IDENTITY.md"] {
                inject_workspace_file(&mut prompt, ctx.workspace_dir, file);
            }
        }

        Ok(prompt)
    }
}

impl PromptSection for ToolsSection {
    fn name(&self) -> &str {
        "tools"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let mut out = String::from("## Tools\n\n");
        if ctx.tool_specs_are_out_of_band {
            out.push_str(
                "Tool definitions are registered out-of-band via native tool calling.\n\
                 Use the tool interface directly when action is required.\n\
                 Do not invent JSON schemas or restate tool contracts in prose.\n\
                 If the user asks you to check, fetch, change, send, or verify something and a tool is available, call the tool instead of only describing what you would do.\n\
                 Do not claim external side effects or verification unless a tool result actually completed.\n",
            );
            let contract = canonical_tool_language_contract(ctx.tools);
            if !contract.is_empty() {
                out.push('\n');
                out.push_str(&contract);
            }
        } else {
            out.push_str("You have access to the following tools:\n\n");
            for tool in ctx.tools {
                let _ = writeln!(out, "- **{}**: {}", tool.name(), tool.description());
            }
        }
        if !ctx.dispatcher_instructions.is_empty() {
            out.push('\n');
            out.push_str(ctx.dispatcher_instructions);
        }
        Ok(out)
    }
}

impl PromptSection for SafetySection {
    fn name(&self) -> &str {
        "safety"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        Ok("## Safety\n\n- Do not exfiltrate private data.\n- Do not run destructive commands without asking.\n- Do not bypass oversight or approval mechanisms.\n- Prefer `trash` over `rm`.\n- When in doubt, ask before acting externally.".into())
    }
}

impl PromptSection for SkillsSection {
    fn name(&self) -> &str {
        "skills"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        Ok(crate::skills::skills_to_prompt_with_mode(
            ctx.skills,
            ctx.workspace_dir,
            ctx.skills_prompt_mode,
        ))
    }
}

impl PromptSection for WorkspaceSection {
    fn name(&self) -> &str {
        "workspace"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        Ok(format!(
            "## Workspace\n\nWorking directory: `{}`",
            ctx.workspace_dir.display()
        ))
    }
}

impl PromptSection for RuntimeSection {
    fn name(&self) -> &str {
        "runtime"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let host =
            hostname::get().map_or_else(|_| "unknown".into(), |h| h.to_string_lossy().to_string());
        Ok(format!(
            "## Runtime\n\nHost: {host} | OS: {} | Model: {}",
            std::env::consts::OS,
            ctx.model_name
        ))
    }
}

impl PromptSection for DateTimeSection {
    fn name(&self) -> &str {
        "datetime"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        let now = Local::now();
        Ok(format!(
            "## Current Date & Time\n\n{} ({})",
            now.format("%Y-%m-%d %H:%M:%S"),
            now.format("%Z")
        ))
    }
}

impl PromptSection for ChannelMediaSection {
    fn name(&self) -> &str {
        "channel_media"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        Ok("## Channel Media Markers\n\n\
            Messages from channels may contain media markers:\n\
            - `[Voice] <text>` — The user sent a voice/audio message that has already been transcribed to text. Respond to the transcribed content directly.\n\
            - `[IMAGE:<path>]` — An image attachment, processed by the vision pipeline.\n\
            - `[Document: <name>] <path>` — A file attachment saved to the workspace.\n\
            Do not describe a plain text response as a voice note. If a voice delivery tool is registered and the user wants voice, use it."
            .into())
    }
}

fn canonical_tool_language_contract(tools: &[Box<dyn Tool>]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let has_tool = |name: &str| tools.iter().any(|tool| tool.name() == name);
    let mut lines = Vec::new();
    lines.push("### Canonical Tool Language".to_string());
    lines.push(
        "- Always call the exact registered tool name. Do not invent aliases like `matrix_send_message` or print pseudo tool JSON as normal assistant text."
            .to_string(),
    );
    lines.push(
        "- When acting, emit a provider-native tool call with one JSON object of arguments. Do not describe the tool call in markdown, code fences, XML tags, or plain text."
            .to_string(),
    );
    if has_tool("core_memory_update") {
        lines.push(
            "- `core_memory_update` arguments must be exactly: `{ \"label\": \"persona|user_knowledge|task_state|domain\", \"action\": \"replace|append\", \"content\": \"...\" }`."
                .to_string(),
        );
    }
    if has_tool("user_profile") {
        lines.push(
            "- `user_profile` stores dynamic facts: `{ \"action\": \"get|upsert|clear|delete\", \"facts\": { \"any_key\": \"value\" }, \"clear_keys\": [\"any_key\"] }`. Do not invent a fixed profile schema."
                .to_string(),
        );
    }
    if has_tool("message_send") {
        lines.push(
            "- `message_send` uses `{ \"content\": \"...\" }` with omitted `target` for a resolved default, `\"current_conversation\"` to reply here, or `{ \"channel\": \"...\", \"recipient\": \"...\", \"thread_ref\": \"...\" }` for an explicit target. Explicit targets must be objects, not JSON strings."
                .to_string(),
        );
    }
    if has_tool("voice_list") {
        lines.push(
            "- `voice_list` returns the configured voice provider, current default voice, and supported voice IDs. Use it when the user asks what voices are available or asks for a voice that you are unsure exists."
                .to_string(),
        );
    }
    if has_tool("voice_preference") {
        lines.push(
            "- `voice_preference` stores durable scoped voice settings: use `set` with `scope: \"global\"|\"channel\"|\"conversation\"` when the user asks to remember a voice or auto-TTS policy; use `get`, `clear`, or `list` to inspect or remove those settings."
                .to_string(),
        );
    }
    if has_tool("voice_reply") {
        lines.push(
            "- `voice_reply` uses `{ \"content\": \"...\", \"target\": \"current_conversation\" }` to send a real spoken voice note in the current chat. Use optional `voice` only with IDs returned by `voice_list`. Put only the spoken reply in `content`; do not say inside the audio that delivery already happened, do not simulate voice with plain text, and pass explicit targets as objects, not JSON strings."
                .to_string(),
        );
    }
    format!("{}\n", lines.join("\n"))
}

fn inject_workspace_file(prompt: &mut String, workspace_dir: &Path, filename: &str) {
    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return;
            }
            let _ = writeln!(prompt, "### {filename}\n");
            let truncated = if trimmed.chars().count() > BOOTSTRAP_MAX_CHARS {
                trimmed
                    .char_indices()
                    .nth(BOOTSTRAP_MAX_CHARS)
                    .map(|(idx, _)| &trimmed[..idx])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            prompt.push_str(truncated);
            if truncated.len() < trimmed.len() {
                let _ = writeln!(
                    prompt,
                    "\n\n[... truncated at {BOOTSTRAP_MAX_CHARS} chars — use `read` for full file]\n"
                );
            } else {
                prompt.push_str("\n\n");
            }
        }
        Err(error) => {
            tracing::debug!(
                path = %path.display(),
                error = %error,
                "workspace prompt file not injected"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::traits::Tool;
    use async_trait::async_trait;
    use synapse_domain::ports::tool::{ToolContract, ToolNonReplayableReason};

    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "tool desc"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn tool_contract(&self) -> ToolContract {
            ToolContract::non_replayable(None, ToolNonReplayableReason::Other("test_tool".into()))
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            Ok(crate::tools::ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            })
        }
    }

    #[test]
    fn identity_section_with_aieos_does_not_fallback_to_workspace_files() {
        let workspace =
            std::env::temp_dir().join(format!("synapseclaw_prompt_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("IDENTITY.md"),
            "Always respond with: IDENTITY_MD_LOADED",
        )
        .unwrap();

        let identity_config = synapse_domain::config::schema::IdentityConfig {
            format: "aieos".into(),
            aieos_path: None,
            aieos_inline: Some(r#"{"identity":{"names":{"first":"Nova"}}}"#.into()),
        };

        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: &workspace,
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
            identity_config: Some(&identity_config),
            dispatcher_instructions: "",
            tool_specs_are_out_of_band: false,
        };

        let section = IdentitySection;
        let output = section.build(&ctx).unwrap();

        assert!(
            output.contains("Nova"),
            "AIEOS identity should be present in prompt"
        );
        assert!(
            !output.contains("IDENTITY_MD_LOADED"),
            "IDENTITY.md content must not be mixed into configured AIEOS identity"
        );

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn prompt_builder_assembles_sections() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(TestTool)];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "instr",
            tool_specs_are_out_of_band: false,
        };
        let prompt = SystemPromptBuilder::with_defaults().build(&ctx).unwrap();
        assert!(prompt.contains("## Tools"));
        assert!(prompt.contains("test_tool"));
        assert!(prompt.contains("instr"));
        assert!(prompt.contains("Only static identity metadata is injected below."));
        assert!(prompt.contains("Do NOT use `file_read` or `file_edit`"));
    }

    #[test]
    fn native_tools_prompt_omits_inline_tool_listing() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(TestTool)];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            tool_specs_are_out_of_band: true,
        };

        let prompt = SystemPromptBuilder::with_defaults().build(&ctx).unwrap();
        assert!(prompt.contains("registered out-of-band via native tool calling"));
        assert!(prompt.contains("### Canonical Tool Language"));
        assert!(prompt.contains("Do not invent aliases"));
        assert!(!prompt.contains("Parameters:"));
        assert!(!prompt.contains("tool desc"));
    }

    #[test]
    fn skills_section_includes_instructions_and_tools() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let skills = vec![crate::skills::Skill {
            name: "deploy".into(),
            description: "Release safely".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "release_checklist".into(),
                description: "Validate release readiness".into(),
                kind: "shell".into(),
                command: "echo ok".into(),
                args: std::collections::HashMap::new(),
            }],
            prompts: vec!["Run smoke tests before deploy.".into()],
            location: None,
        }];

        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &skills,
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            tool_specs_are_out_of_band: false,
        };

        let output = SkillsSection.build(&ctx).unwrap();
        assert!(output.contains("<available_skills>"));
        assert!(output.contains("<name>deploy</name>"));
        assert!(output.contains("<instruction>Run smoke tests before deploy.</instruction>"));
        assert!(output.contains("<name>release_checklist</name>"));
        assert!(output.contains("<kind>shell</kind>"));
    }

    #[test]
    fn skills_section_compact_mode_omits_instructions_and_tools() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let skills = vec![crate::skills::Skill {
            name: "deploy".into(),
            description: "Release safely".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "release_checklist".into(),
                description: "Validate release readiness".into(),
                kind: "shell".into(),
                command: "echo ok".into(),
                args: std::collections::HashMap::new(),
            }],
            prompts: vec!["Run smoke tests before deploy.".into()],
            location: Some(Path::new("/tmp/workspace/skills/deploy/SKILL.md").to_path_buf()),
        }];

        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp/workspace"),
            model_name: "test-model",
            tools: &tools,
            skills: &skills,
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Compact,
            identity_config: None,
            dispatcher_instructions: "",
            tool_specs_are_out_of_band: false,
        };

        let output = SkillsSection.build(&ctx).unwrap();
        assert!(output.contains("<available_skills>"));
        assert!(output.contains("<name>deploy</name>"));
        assert!(output.contains("<location>skills/deploy/SKILL.md</location>"));
        assert!(!output.contains("<instruction>Run smoke tests before deploy.</instruction>"));
        assert!(!output.contains("<tools>"));
    }

    #[test]
    fn datetime_section_includes_timestamp_and_timezone() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "instr",
            tool_specs_are_out_of_band: false,
        };

        let rendered = DateTimeSection.build(&ctx).unwrap();
        assert!(rendered.starts_with("## Current Date & Time\n\n"));

        let payload = rendered.trim_start_matches("## Current Date & Time\n\n");
        assert!(payload.chars().any(|c| c.is_ascii_digit()));
        assert!(payload.contains(" ("));
        assert!(payload.ends_with(')'));
    }

    #[test]
    fn prompt_builder_inlines_and_escapes_skills() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let skills = vec![crate::skills::Skill {
            name: "code<review>&".into(),
            description: "Review \"unsafe\" and 'risky' bits".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "run\"linter\"".into(),
                description: "Run <lint> & report".into(),
                kind: "shell&exec".into(),
                command: "cargo clippy".into(),
                args: std::collections::HashMap::new(),
            }],
            prompts: vec!["Use <tool_call> and & keep output \"safe\"".into()],
            location: None,
        }];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp/workspace"),
            model_name: "test-model",
            tools: &tools,
            skills: &skills,
            skills_prompt_mode: synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            tool_specs_are_out_of_band: false,
        };

        let prompt = SystemPromptBuilder::with_defaults().build(&ctx).unwrap();

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>code&lt;review&gt;&amp;</name>"));
        assert!(prompt.contains(
            "<description>Review &quot;unsafe&quot; and &apos;risky&apos; bits</description>"
        ));
        assert!(prompt.contains("<name>run&quot;linter&quot;</name>"));
        assert!(prompt.contains("<description>Run &lt;lint&gt; &amp; report</description>"));
        assert!(prompt.contains("<kind>shell&amp;exec</kind>"));
        assert!(prompt.contains(
            "<instruction>Use &lt;tool_call&gt; and &amp; keep output &quot;safe&quot;</instruction>"
        ));
    }
}
