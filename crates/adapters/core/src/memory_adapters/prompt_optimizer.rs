//! Prompt Optimizer — Phase 4.4: self-improving agent instructions.
//!
//! Analyzes accumulated reflections (what worked, what failed) and proposes
//! targeted changes to core memory blocks via LLM-driven optimization.
//!
//! Runs as Phase 3 of the ConsolidationWorker (after importance decay + GC).
//! Pattern: LangMem gradient (think/analyze/propose) + Letta sleep-time (background).

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use synapse_domain::domain::memory::{AgentId, MemoryCategory, MemoryError, MemoryQuery};
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_providers::traits::Provider;

const OPTIMIZATION_PROMPT: &str = r#"You are a prompt optimization engine for an AI agent. Analyze the agent's reflections from recent tasks and propose specific, targeted changes to the agent's instruction blocks.

These instruction blocks are ALWAYS present in the agent's context. Changes here directly affect behavior.

Current instruction blocks:
<persona>
{persona}
</persona>
<domain>
{domain}
</domain>

Recent reflections (newest first):
{reflections}

Respond ONLY with valid JSON:
{
  "analysis": "2-3 sentence summary of patterns found across reflections",
  "changes": [
    {
      "block": "domain or persona or task_state",
      "action": "append or replace",
      "content": "specific concise instruction to add",
      "reason": "why this helps, citing specific reflection patterns"
    }
  ],
  "no_change_reason": null
}

If no changes are warranted, return:
{
  "analysis": "summary of why no changes needed",
  "changes": [],
  "no_change_reason": "specific reason"
}

Rules:
- Only propose changes supported by 2+ reflections (not one-off events)
- Prefer "append" over "replace" (less destructive)
- Keep each change to 1-2 sentences maximum
- Maximum 3 changes per cycle
- Never remove working instructions — only add or refine
- Focus on actionable behavior changes, not abstract principles
- Write changes in the same language the reflections use"#;

/// Result of one optimization cycle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptOptimization {
    pub id: String,
    pub agent_id: String,
    pub timestamp: String,
    pub reflections_analyzed: usize,
    pub changes: Vec<PromptChange>,
    pub analysis: String,
    pub previous_blocks: HashMap<String, String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PromptChange {
    pub block: String,
    pub action: String,
    pub content: String,
    pub reason: String,
}

#[derive(Debug, serde::Deserialize)]
struct OptimizationResponse {
    analysis: String,
    #[serde(default)]
    changes: Vec<OptimizationChange>,
    #[serde(default)]
    no_change_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OptimizationChange {
    block: String,
    action: String,
    content: String,
    reason: String,
}

/// Run a prompt optimization cycle.
///
/// Collects reflections since `since`, analyzes patterns via LLM,
/// and applies changes to core memory blocks.
///
/// Returns `None` if insufficient data or no changes proposed.
pub async fn optimize_prompt(
    provider: &dyn Provider,
    model: &str,
    memory: &dyn UnifiedMemoryPort,
    agent_id: &str,
    min_reflections: usize,
) -> Result<Option<PromptOptimization>, MemoryError> {
    tracing::info!(agent_id, "prompt.optimization.start");

    // 1. Collect recent reflections
    let query = MemoryQuery {
        text: String::new(),
        embedding: None,
        agent_id: agent_id.to_string(),
        include_shared: false,
        time_range: None,
        limit: 50,
    };

    let reflections = memory
        .get_failure_patterns(&agent_id.to_string(), 50)
        .await?;
    let successes = memory.get_relevant_reflections(&query).await?;

    let total = reflections.len() + successes.len();
    if total < min_reflections {
        tracing::debug!(
            agent_id,
            reflections = total,
            min = min_reflections,
            "prompt.optimization.skip_insufficient_data"
        );
        return Ok(None);
    }

    // 2. Format reflections for LLM
    let mut formatted = String::new();
    for r in reflections.iter().chain(successes.iter()) {
        formatted.push_str(&format!(
            "- [{}] Task: {} | Worked: {} | Failed: {} | Lesson: {}\n",
            r.outcome, r.task_summary, r.what_worked, r.what_failed, r.lesson
        ));
    }

    // 3. Get current core blocks
    let blocks = memory
        .get_core_blocks(&agent_id.to_string())
        .await
        .unwrap_or_default();

    let persona = blocks
        .iter()
        .find(|b| b.label == "persona")
        .map(|b| b.content.as_str())
        .unwrap_or("(empty)");
    let domain = blocks
        .iter()
        .find(|b| b.label == "domain")
        .map(|b| b.content.as_str())
        .unwrap_or("(empty)");

    // Snapshot for rollback
    let mut previous_blocks = HashMap::new();
    for block in &blocks {
        previous_blocks.insert(block.label.clone(), block.content.clone());
    }

    // 4. LLM analysis
    let prompt = OPTIMIZATION_PROMPT
        .replace("{persona}", persona)
        .replace("{domain}", domain)
        .replace("{reflections}", &formatted);

    let raw = provider
        .chat_with_system(None, &prompt, model, 0.2)
        .await
        .map_err(|e| MemoryError::Storage(format!("Optimization LLM call failed: {e}")))?;

    let response = parse_optimization_response(&raw)?;

    tracing::info!(
        agent_id,
        analysis = %response.analysis,
        "prompt.optimization.analysis"
    );

    if response.changes.is_empty() {
        tracing::info!(
            agent_id,
            reason = response.no_change_reason.as_deref().unwrap_or("none"),
            "prompt.optimization.no_changes"
        );
        return Ok(None);
    }

    // 5. Apply changes
    let mut applied_changes = Vec::new();
    for change in &response.changes {
        if !["domain", "persona", "task_state"].contains(&change.block.as_str()) {
            tracing::warn!(
                block = %change.block,
                "prompt.optimization.invalid_block"
            );
            continue;
        }

        match change.action.as_str() {
            "append" => {
                memory
                    .append_core_block(&agent_id.to_string(), &change.block, &change.content)
                    .await?;
            }
            "replace" => {
                memory
                    .update_core_block(&agent_id.to_string(), &change.block, change.content.clone())
                    .await?;
            }
            other => {
                tracing::warn!(action = other, "prompt.optimization.invalid_action");
                continue;
            }
        }

        tracing::info!(
            block = %change.block,
            action = %change.action,
            reason = %change.reason,
            "prompt.optimization.change"
        );

        applied_changes.push(PromptChange {
            block: change.block.clone(),
            action: change.action.clone(),
            content: change.content.clone(),
            reason: change.reason.clone(),
        });
    }

    if applied_changes.is_empty() {
        return Ok(None);
    }

    // 6. Store optimization record
    let optimization = PromptOptimization {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent_id.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        reflections_analyzed: total,
        changes: applied_changes,
        analysis: response.analysis,
        previous_blocks,
    };

    if let Ok(record_json) = serde_json::to_string(&optimization) {
        let key = format!("prompt_opt_{}", &optimization.id[..8]);
        let _ = memory
            .store(
                &key,
                &record_json,
                &MemoryCategory::Custom("prompt_optimization".into()),
                None,
            )
            .await;
    }

    tracing::info!(
        agent_id,
        changes = optimization.changes.len(),
        reflections = optimization.reflections_analyzed,
        "prompt.optimization.applied"
    );

    Ok(Some(optimization))
}

fn parse_optimization_response(raw: &str) -> Result<OptimizationResponse, MemoryError> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned)
        .map_err(|e| MemoryError::Storage(format!("Optimization response parse error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_optimization() {
        let json = r#"{
            "analysis": "Agent repeatedly fails at delegation tasks",
            "changes": [
                {
                    "block": "domain",
                    "action": "append",
                    "content": "When delegating tasks, always include deadline and expected format.",
                    "reason": "3 reflections showed vague delegation leading to poor results"
                }
            ],
            "no_change_reason": null
        }"#;
        let result = parse_optimization_response(json).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].block, "domain");
        assert_eq!(result.changes[0].action, "append");
    }

    #[test]
    fn parse_no_changes() {
        let json = r#"{
            "analysis": "All recent reflections show successful outcomes",
            "changes": [],
            "no_change_reason": "No patterns of failure detected"
        }"#;
        let result = parse_optimization_response(json).unwrap();
        assert!(result.changes.is_empty());
        assert!(result.no_change_reason.is_some());
    }

    #[test]
    fn parse_code_block_wrapped() {
        let json = "```json\n{\"analysis\": \"test\", \"changes\": [], \"no_change_reason\": \"test\"}\n```";
        let result = parse_optimization_response(json).unwrap();
        assert_eq!(result.analysis, "test");
    }

    #[test]
    fn parse_malformed() {
        let result = parse_optimization_response("not json");
        assert!(result.is_err());
    }
}
