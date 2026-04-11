//! Shared runtime system-prompt construction.
//!
//! This is transport-neutral prompt assembly used by channel, gateway, and CLI
//! entrypoints. Concrete adapters may still decide when and how to attach the
//! prompt to provider calls.

use crate::skills::{self, Skill};
use std::fmt::Write;
use std::path::Path;
use synapse_domain::config::schema::{IdentityConfig, SkillsPromptInjectionMode};
use synapse_infra::identity;

pub const BOOTSTRAP_MAX_CHARS: usize = 20_000;

/// Load workspace identity files and build a system prompt.
///
/// Daily memory files (`memory/*.md`) are not injected; they are accessed
/// on-demand via memory tools and structured context.
pub fn build_system_prompt(
    workspace_dir: &Path,
    model_name: &str,
    tools: &[(&str, &str)],
    skills: &[Skill],
    identity_config: Option<&IdentityConfig>,
    bootstrap_max_chars: Option<usize>,
) -> String {
    build_system_prompt_with_mode(
        workspace_dir,
        model_name,
        tools,
        skills,
        identity_config,
        bootstrap_max_chars,
        false,
        SkillsPromptInjectionMode::Full,
    )
}

pub fn build_system_prompt_with_mode(
    workspace_dir: &Path,
    model_name: &str,
    tools: &[(&str, &str)],
    skills: &[Skill],
    identity_config: Option<&IdentityConfig>,
    bootstrap_max_chars: Option<usize>,
    native_tools: bool,
    skills_prompt_mode: SkillsPromptInjectionMode,
) -> String {
    let mut prompt = String::with_capacity(8192);

    prompt.push_str(
        "## CRITICAL: No Tool Narration\n\n\
         NEVER narrate, announce, describe, or explain your tool usage to the user. \
         Do NOT say things like 'Let me check...', 'I will use http_request to...', \
         'I'll fetch that for you', 'Searching now...', or 'Using the web_search tool'. \
         The user must ONLY see the final answer. Tool calls are invisible infrastructure — \
         never reference them. If you catch yourself starting a sentence about what tool \
         you are about to use or just used, DELETE it and give the answer directly.\n\n",
    );

    if !tools.is_empty() {
        prompt.push_str("## Tools\n\n");
        if native_tools {
            prompt.push_str(
                "Tool definitions are registered out-of-band via native tool calling.\n\
                 Use the tool interface directly when action is required.\n\
                 Do not restate tool schemas or invent ad hoc JSON formats in prose.\n\n",
            );
            let names = tools
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            if !names.is_empty() {
                let _ = writeln!(prompt, "Registered tool names: {names}\n");
            }
        } else {
            prompt.push_str("You have access to the following tools:\n\n");
            for (name, desc) in tools {
                let _ = writeln!(prompt, "- **{name}**: {desc}");
            }
            prompt.push('\n');
        }
    }

    if native_tools {
        prompt.push_str(
            "## Your Task\n\n\
             When the user sends a message, respond naturally. Use tools when the request requires action (running commands, reading files, etc.).\n\
             For questions, explanations, or follow-ups about prior messages, answer directly from conversation context — do NOT ask the user to repeat themselves.\n\
             Do NOT: summarize this configuration, describe your capabilities, or output step-by-step meta-commentary.\n\n",
        );
    } else {
        prompt.push_str(
            "## Your Task\n\n\
             When the user sends a message, ACT on it. Use the tools to fulfill their request.\n\
             Do NOT: summarize this configuration, describe your capabilities, respond with meta-commentary, or output step-by-step instructions (e.g. \"1. First... 2. Next...\").\n\
             Instead: emit actual <tool_call> tags when you need to act. Just do what they ask.\n\n",
        );
    }

    prompt.push_str("## Safety\n\n");
    prompt.push_str(
        "- Do not exfiltrate private data.\n\
         - Do not run destructive commands without asking.\n\
         - Do not bypass oversight or approval mechanisms.\n\
         - Prefer `trash` over `rm` (recoverable beats gone forever).\n\
         - When in doubt, ask before acting externally.\n\n",
    );

    if !skills.is_empty() {
        prompt.push_str(&skills::skills_to_prompt_with_mode(
            skills,
            workspace_dir,
            skills_prompt_mode,
        ));
        prompt.push_str("\n\n");
    }

    let _ = writeln!(
        prompt,
        "## Workspace\n\nWorking directory: `{}`\n",
        workspace_dir.display()
    );

    prompt.push_str("## Project Context\n\n");
    append_identity_context(
        &mut prompt,
        workspace_dir,
        identity_config,
        bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS),
    );

    let now = chrono::Local::now();
    let _ = writeln!(
        prompt,
        "## Current Date & Time\n\n{} ({})\n",
        now.format("%Y-%m-%d %H:%M:%S"),
        now.format("%Z")
    );

    let host =
        hostname::get().map_or_else(|_| "unknown".into(), |h| h.to_string_lossy().to_string());
    let _ = writeln!(
        prompt,
        "## Runtime\n\nHost: {host} | OS: {} | Model: {model_name}\n",
        std::env::consts::OS,
    );

    prompt.push_str("## Channel Capabilities\n\n");
    prompt.push_str("- You are running as a messaging bot. Your response is automatically sent back to the user's channel.\n");
    prompt.push_str("- You do NOT need to ask permission to respond — just respond directly.\n");
    prompt.push_str("- NEVER repeat, describe, or echo credentials, tokens, API keys, or secrets in your responses.\n");
    prompt.push_str("- If a tool output contains credentials, they have already been redacted — do not mention them.\n");
    prompt.push_str("- When a user sends a voice note, it is automatically transcribed to text. Your text reply is automatically converted to a voice note and sent back. Do NOT attempt to generate audio yourself — TTS is handled by the channel.\n");
    prompt.push_str("- NEVER narrate or describe your tool usage. Do NOT say 'Let me fetch...', 'I will use...', 'Searching...', or similar. Give the FINAL ANSWER only — no intermediate steps, no tool mentions, no progress updates.\n\n");

    if prompt.is_empty() {
        "You are SynapseClaw, a fast and efficient AI assistant built in Rust. Be helpful, concise, and direct."
            .to_string()
    } else {
        prompt
    }
}

fn append_identity_context(
    prompt: &mut String,
    workspace_dir: &Path,
    identity_config: Option<&IdentityConfig>,
    max_chars: usize,
) {
    if let Some(config) = identity_config {
        if identity::is_aieos_configured(config) {
            match identity::load_aieos_identity(config, workspace_dir) {
                Ok(Some(aieos_identity)) => {
                    let aieos_prompt = identity::aieos_to_system_prompt(&aieos_identity);
                    if !aieos_prompt.is_empty() {
                        prompt.push_str(&aieos_prompt);
                        prompt.push_str("\n\n");
                    }
                }
                Ok(None) => load_openclaw_bootstrap_files(prompt, workspace_dir, max_chars),
                Err(error) => {
                    eprintln!(
                        "Warning: Failed to load AIEOS identity: {error}. Using OpenClaw format."
                    );
                    load_openclaw_bootstrap_files(prompt, workspace_dir, max_chars);
                }
            }
        } else {
            load_openclaw_bootstrap_files(prompt, workspace_dir, max_chars);
        }
    } else {
        load_openclaw_bootstrap_files(prompt, workspace_dir, max_chars);
    }
}

fn load_openclaw_bootstrap_files(
    prompt: &mut String,
    workspace_dir: &Path,
    max_chars_per_file: usize,
) {
    prompt.push_str(
        "Structured core memory and turn context carry persona, user preferences, and task state.\n\
         Only static identity metadata is injected below.\n\
         Do NOT suggest reading or editing workspace bootstrap docs with `file_read` or `file_edit` unless the user explicitly asks to inspect or edit them.\n\
         For durable preferences, task state, and long-term memory, prefer `core_memory_update`, `memory_store`, `memory_recall`, and `user_profile`.\n\n",
    );

    for filename in ["IDENTITY.md"] {
        inject_workspace_file(prompt, workspace_dir, filename, max_chars_per_file);
    }
}

fn inject_workspace_file(
    prompt: &mut String,
    workspace_dir: &Path,
    filename: &str,
    max_chars: usize,
) {
    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return;
            }
            let _ = writeln!(prompt, "### {filename}\n");
            let truncated = if trimmed.chars().count() > max_chars {
                trimmed
                    .char_indices()
                    .nth(max_chars)
                    .map(|(idx, _)| &trimmed[..idx])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            if truncated.len() < trimmed.len() {
                prompt.push_str(truncated);
                let _ = writeln!(
                    prompt,
                    "\n\n[... truncated at {max_chars} chars — use `read` for full file]\n"
                );
            } else {
                prompt.push_str(trimmed);
                prompt.push_str("\n\n");
            }
        }
        Err(_) => {
            let _ = writeln!(prompt, "### {filename}\n\n[File not found: {filename}]\n");
        }
    }
}
