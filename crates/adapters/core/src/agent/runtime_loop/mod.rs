use crate::runtime;
use crate::tools::{self, Tool};
use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::Write;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use synapse_domain::config::schema::{Config, ContextCompressionConfig};
use synapse_domain::domain::util::truncate_with_ellipsis;
use synapse_domain::ports::approval::ApprovalPort;
use synapse_infra::approval::ApprovalManager;
use synapse_memory::{self, MemoryCategory, UnifiedMemoryPort};
use synapse_observability::{self, runtime_trace, Observer, ObserverEvent};
use synapse_providers::multimodal;
use synapse_providers::{
    self, ChatMessage, ChatRequest, Provider, ProviderCapabilityError, ToolCall,
};
use synapse_security::security_policy_from_config;
use tokio_util::sync::CancellationToken;

pub(crate) use crate::agent::autosave_memory_key;
/// Minimum characters per chunk when relaying LLM text to a streaming draft.
const STREAM_CHUNK_MIN_CHARS: usize = 80;

/// Default maximum agentic tool-use iterations per user message to prevent runaway loops.
/// Used as a safe fallback when `max_tool_iterations` is unset or configured as zero.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 10;

// ── Tool filtering — delegated to domain services ───────────────────
//
// The actual filtering logic lives in `synapse_domain::application::services::tool_filtering`.
// These re-exports keep helpers available inside the internal runtime-loop module
// while external callers use `crate::agent::*` re-exports instead.

pub(crate) use synapse_domain::application::services::tool_filtering::compute_excluded_mcp_tools;
#[cfg(test)]
pub(crate) use synapse_domain::application::services::tool_filtering::{
    filter_by_allowed_tools, filter_tool_specs_for_turn, glob_match,
};

/// Scrub credentials from tool output — delegated to `synapse_security`.
pub(crate) use synapse_security::scrub_credentials;

// ── History compaction — delegated to domain services ────────────────
//
// Constants and pure functions for history management live in
// `synapse_domain::application::services::history_compaction`.

use synapse_domain::application::services::history_compaction as compaction;
#[cfg(test)]
use synapse_domain::application::services::history_compaction::{
    estimate_history_tokens, DEFAULT_MAX_HISTORY_MESSAGES,
};

/// Minimum interval between progress sends to avoid flooding the draft channel.
pub(crate) const PROGRESS_MIN_INTERVAL_MS: u64 = 500;

/// Sentinel value sent through on_delta to signal the draft updater to clear accumulated text.
/// Used before streaming the final answer so progress lines are replaced by the clean response.
pub(crate) const DRAFT_CLEAR_SENTINEL: &str = "\x00CLEAR\x00";

/// Extract a short hint from tool call arguments for progress display.
fn truncate_tool_args_for_progress(name: &str, args: &serde_json::Value, max_len: usize) -> String {
    let hint = match name {
        "shell" => args.get("command").and_then(|v| v.as_str()),
        "file_read" | "file_write" => args.get("path").and_then(|v| v.as_str()),
        _ => args
            .get("action")
            .and_then(|v| v.as_str())
            .or_else(|| args.get("query").and_then(|v| v.as_str())),
    };
    match hint {
        Some(s) => truncate_with_ellipsis(s, max_len),
        None => String::new(),
    }
}

/// Convert a tool registry to OpenAI function-calling format for native tool support.
fn tools_to_openai_format(tools_registry: &[Box<dyn Tool>]) -> Vec<serde_json::Value> {
    tools_registry
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name(),
                    "description": tool.description(),
                    "parameters": tool.parameters_schema()
                }
            })
        })
        .collect()
}

fn memory_session_id_from_state_file(path: &Path) -> Option<String> {
    let raw = path.to_string_lossy().trim().to_string();
    if raw.is_empty() {
        return None;
    }

    Some(format!("cli:{raw}"))
}

/// Thin wrapper: delegates to domain `trim_history`.
fn trim_history(history: &mut Vec<ChatMessage>, max_history: usize) {
    compaction::trim_history(history, max_history);
}

/// Auto-compact conversation history using domain policy + provider summarization.
async fn auto_compact_history(
    history: &mut Vec<ChatMessage>,
    provider: &dyn Provider,
    model: &str,
    max_history: usize,
    max_context_tokens: usize,
    compression: &ContextCompressionConfig,
) -> Result<bool> {
    let policy = compaction::HistoryCompressionPolicy::from(compression);
    if !policy.enabled {
        return Ok(false);
    }

    let compaction_threshold_tokens =
        compaction::history_compression_threshold_tokens(max_context_tokens.max(1), &policy);
    let Some((start, compact_end, transcript)) = compaction::prepare_compaction_with_policy(
        history,
        max_history,
        compaction_threshold_tokens,
        &policy,
    ) else {
        return Ok(false);
    };

    let summarizer_user =
        compaction::compaction_summarizer_prompt_with_policy(&transcript, None, &policy, None);

    let summary_raw = provider
        .chat_with_system(
            Some(compaction::COMPACTION_SUMMARIZER_SYSTEM),
            &summarizer_user,
            model,
            0.2,
        )
        .await
        .unwrap_or_default();

    compaction::apply_compaction_with_policy(
        history,
        start,
        compact_end,
        &summary_raw,
        &transcript,
        &policy,
    );

    Ok(true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InteractiveSessionState {
    version: u32,
    history: Vec<ChatMessage>,
}

impl InteractiveSessionState {
    fn from_history(history: &[ChatMessage]) -> Self {
        Self {
            version: 1,
            history: history.to_vec(),
        }
    }
}

fn load_interactive_session_history(path: &Path, system_prompt: &str) -> Result<Vec<ChatMessage>> {
    if !path.exists() {
        return Ok(vec![ChatMessage::system(system_prompt)]);
    }

    let raw = std::fs::read_to_string(path)?;
    let mut state: InteractiveSessionState = serde_json::from_str(&raw)?;
    if state.history.is_empty() {
        state.history.push(ChatMessage::system(system_prompt));
    } else if state.history.first().map(|msg| msg.role.as_str()) != Some("system") {
        state.history.insert(0, ChatMessage::system(system_prompt));
    }

    Ok(state.history)
}

fn save_interactive_session_history(path: &Path, history: &[ChatMessage]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let payload = serde_json::to_string_pretty(&InteractiveSessionState::from_history(history))?;
    std::fs::write(path, payload)?;
    Ok(())
}

/// Resolve the canonical agent ID from config.
/// Prefers `agents_ipc.agent_id`, falls back to `agents_ipc.role`, then `"default"`.
pub fn resolve_agent_id(config: &synapse_domain::config::schema::Config) -> String {
    config.agents_ipc.agent_id.clone().unwrap_or_else(|| {
        let role = config.agents_ipc.role.clone();
        if !role.is_empty() {
            tracing::debug!("No explicit agent_id, using IPC role: {role}");
            role
        } else {
            tracing::warn!("No agent_id or role configured, using 'default'");
            "default".to_string()
        }
    })
}

/// Build context preamble by searching memory for relevant entries.
/// Entries with a hybrid score below `min_relevance_score` are dropped to
/// prevent unrelated memories from bleeding into the conversation.
async fn build_context(
    mem: &dyn UnifiedMemoryPort,
    user_msg: &str,
    min_relevance_score: f64,
    session_id: Option<&str>,
    agent_id: &str,
) -> String {
    let mut context = String::new();

    // ── Core memory blocks (MemGPT pattern) ──────────────────────
    // Always-in-prompt blocks: persona, user_knowledge, task_state, domain.
    if let Ok(blocks) = mem.get_core_blocks(&agent_id.to_string()).await {
        tracing::info!(
            agent_id,
            blocks = blocks.len(),
            "memory.context.core_blocks"
        );
        for block in &blocks {
            if !block.content.trim().is_empty() {
                let _ = writeln!(context, "<{}>", block.label);
                let _ = writeln!(context, "{}", block.content.trim());
                let _ = writeln!(context, "</{}>", block.label);
            }
        }
        if !context.is_empty() {
            context.push('\n');
        }
    }

    // ── Relevant memories for this message ───────────────────────
    if let Ok(entries) = mem.recall(user_msg, 5, session_id).await {
        tracing::info!(
            query_len = user_msg.len(),
            results = entries.len(),
            "memory.context.recall"
        );
        let relevant: Vec<_> = entries
            .iter()
            .filter(|e| match e.score {
                Some(score) => score >= min_relevance_score,
                None => true,
            })
            .collect();

        if !relevant.is_empty() {
            context.push_str("[Memory context]\n");
            for entry in &relevant {
                if synapse_memory::is_assistant_autosave_key(&entry.key) {
                    continue;
                }
                if synapse_domain::domain::util::should_skip_autosave_content(&entry.content) {
                    continue;
                }
                // Skip entries containing tool_result blocks — they can leak
                // stale tool output from previous heartbeat ticks into new
                // sessions, presenting the LLM with orphan tool_result data.
                if entry.content.contains("<tool_result") {
                    continue;
                }
                let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
            }
            if context == "[Memory context]\n" {
                context.clear();
            } else {
                context.push('\n');
            }
        }
    }

    // ── Relevant skills ────────────────────────────────────────────
    let skill_query = synapse_domain::domain::memory::MemoryQuery {
        text: user_msg.to_string(),
        embedding: None,
        agent_id: agent_id.to_string(),
        categories: Vec::new(),
        include_shared: false,
        time_range: None,
        limit: 3,
    };
    if let Ok(skills) = mem.find_skills(&skill_query).await {
        tracing::info!(skills = skills.len(), "memory.context.skills");
        for skill in &skills {
            if !skill.content.trim().is_empty() {
                let _ = writeln!(context, "<skill name=\"{}\">", skill.name);
                let _ = writeln!(context, "{}", skill.content.trim());
                let _ = writeln!(context, "</skill>");
            }
        }
    }

    // ── Related entities (conditional: only if recall found relevant memories) ──
    if !context.is_empty() {
        let entity_query = synapse_domain::domain::memory::MemoryQuery {
            text: user_msg.to_string(),
            embedding: None,
            agent_id: agent_id.to_string(),
            categories: Vec::new(),
            include_shared: false,
            time_range: None,
            limit: 3,
        };
        if let Ok(entities) = mem.search_entities(&entity_query).await {
            tracing::info!(entities = entities.len(), "memory.context.entities");
            for entity in &entities {
                if let Some(ref summary) = entity.summary {
                    let _ = writeln!(
                        context,
                        "<entity name=\"{}\" type=\"{}\">",
                        entity.name, entity.entity_type
                    );
                    let _ = writeln!(context, "{}", summary);
                    let _ = writeln!(context, "</entity>");
                }
            }
        }
    }

    tracing::info!(context_chars = context.len(), "memory.context.assembled");

    context
}

mod cli_run;
pub(super) mod tool_call_parsing;
pub(super) mod tool_execution;

// Re-export public API from sub-modules.
pub use cli_run::{process_message, run, run_with_shared_memory};
#[allow(unused_imports)]
pub(crate) use synapse_domain::application::services::tool_filtering::build_tool_instructions;
#[allow(unused_imports)]
pub(crate) use tool_call_parsing::ParsedToolCall;
#[allow(unused_imports)]
pub(crate) use tool_execution::{agent_turn, is_tool_loop_cancelled, ToolLoopCancelled};
pub(crate) use tool_execution::{execute_one_tool, run_tool_call_loop, ToolExecutionOutcome};

#[cfg(test)]
pub(crate) use tool_call_parsing::{
    build_native_assistant_history, build_native_assistant_history_from_parsed_calls,
    detect_tool_call_parse_issue, extract_json_values, parse_arguments_value,
    parse_tool_call_value, parse_tool_calls, parse_tool_calls_from_json_value,
    resolve_display_text, strip_think_tags, strip_tool_result_blocks,
};
#[cfg(test)]
pub(crate) use tool_execution::should_execute_tools_in_parallel;

#[cfg(test)]
mod tests;
