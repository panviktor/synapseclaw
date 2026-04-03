//! Skill learning from pipeline run reflections.
//!
//! Phase 4.3 Slice 4: after each pipeline run completes, the SkillLearner
//! analyzes the outcome via LLM and optionally creates/updates a reusable skill.

use chrono::Utc;
use synapse_domain::domain::memory::{
    MemoryError, Reflection, ReflectionOutcome, Skill, SkillUpdate,
};
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_providers::traits::Provider;

const REFLECTION_PROMPT: &str = r#"You just completed a task. Analyze the outcome and extract lessons.

Task: {task_summary}
Outcome: {outcome}
Steps taken: {steps}
Errors encountered: {errors}

Respond ONLY with valid JSON:
{
  "what_worked": "specific techniques that helped",
  "what_failed": "specific mistakes or blockers",
  "lesson": "one concise takeaway for future similar tasks",
  "should_create_skill": true or false,
  "skill_name": "optional: short name for reusable procedure",
  "skill_content": "optional: step-by-step procedure in markdown"
}
Do not include any text outside the JSON object."#;

#[derive(Debug, serde::Deserialize)]
struct ReflectionAnalysis {
    what_worked: String,
    what_failed: String,
    lesson: String,
    #[serde(default)]
    should_create_skill: bool,
    skill_name: Option<String>,
    skill_content: Option<String>,
}

/// Run summary passed to the skill learner after pipeline completion.
#[derive(Debug, Clone)]
pub struct PipelineRunSummary {
    pub run_id: String,
    pub task: String,
    pub outcome: ReflectionOutcome,
    pub steps: Vec<String>,
    pub errors: Vec<String>,
}

impl PipelineRunSummary {
    fn steps_description(&self) -> String {
        if self.steps.is_empty() {
            "none".to_string()
        } else {
            self.steps.join("\n- ")
        }
    }

    fn errors_description(&self) -> String {
        if self.errors.is_empty() {
            "none".to_string()
        } else {
            self.errors.join("\n- ")
        }
    }
}

/// Analyze a pipeline run and create reflections + optional skills.
///
/// Called after pipeline completion. Fire-and-forget via tokio::spawn.
pub async fn reflect_on_run(
    provider: &dyn Provider,
    model: &str,
    memory: &dyn UnifiedMemoryPort,
    agent_id: &str,
    summary: &PipelineRunSummary,
) -> Result<(), MemoryError> {
    let prompt = REFLECTION_PROMPT
        .replace("{task_summary}", &summary.task)
        .replace("{outcome}", &summary.outcome.to_string())
        .replace("{steps}", &summary.steps_description())
        .replace("{errors}", &summary.errors_description());

    tracing::info!(
        agent_id,
        task = %summary.task.chars().take(80).collect::<String>(),
        outcome = %summary.outcome,
        "memory.skill_reflection.start"
    );

    let raw = provider
        .chat_with_system(None, &prompt, model, 0.1)
        .await
        .map_err(|e| MemoryError::Storage(format!("Reflection LLM call failed: {e}")))?;

    let analysis = parse_reflection_response(&raw)?;

    // Store reflection
    let reflection = Reflection {
        id: String::new(),
        agent_id: agent_id.to_string(),
        pipeline_run: Some(summary.run_id.clone()),
        task_summary: summary.task.clone(),
        outcome: summary.outcome.clone(),
        what_worked: analysis.what_worked,
        what_failed: analysis.what_failed,
        lesson: analysis.lesson.clone(),
        created_at: Utc::now(),
    };

    memory.store_reflection(reflection).await?;

    tracing::info!(
        agent_id,
        lesson = %analysis.lesson.chars().take(100).collect::<String>(),
        "memory.reflection.stored"
    );

    // Create or update skill if recommended
    if analysis.should_create_skill {
        if let (Some(name), Some(content)) = (&analysis.skill_name, &analysis.skill_content) {
            match memory.get_skill(name, &agent_id.to_string()).await? {
                Some(existing) => {
                    // Update existing skill
                    let update = SkillUpdate {
                        increment_success: summary.outcome == ReflectionOutcome::Success,
                        increment_fail: summary.outcome == ReflectionOutcome::Failure,
                        new_content: Some(content.clone()),
                    };
                    memory
                        .update_skill(&existing.id, update, &agent_id.to_string())
                        .await?;
                    tracing::info!(name = %name, version = existing.version + 1, "memory.skill.updated");
                }
                None => {
                    // Create new skill
                    let skill = Skill {
                        id: String::new(),
                        name: name.clone(),
                        description: analysis.lesson,
                        content: content.clone(),
                        tags: vec![],
                        success_count: u32::from(summary.outcome == ReflectionOutcome::Success),
                        fail_count: u32::from(summary.outcome == ReflectionOutcome::Failure),
                        version: 1,
                        created_by: agent_id.to_string(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                    };
                    memory.store_skill(skill).await?;
                    tracing::info!(name = %name, "memory.skill.created");
                }
            }
        }
    }

    Ok(())
}

fn parse_reflection_response(raw: &str) -> Result<ReflectionAnalysis, MemoryError> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    serde_json::from_str(cleaned)
        .map_err(|e| MemoryError::Storage(format!("Reflection parse error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_reflection() {
        let json = r#"{
            "what_worked": "Used incremental approach",
            "what_failed": "Initial prompt too vague",
            "lesson": "Break complex tasks into smaller steps",
            "should_create_skill": true,
            "skill_name": "incremental_refactoring",
            "skill_content": "1. Identify scope\n2. Create branch\n3. Make small changes\n4. Test each step"
        }"#;
        let result = parse_reflection_response(json).unwrap();
        assert!(result.should_create_skill);
        assert_eq!(
            result.skill_name.as_deref(),
            Some("incremental_refactoring")
        );
    }

    #[test]
    fn parse_reflection_no_skill() {
        let json = r#"{
            "what_worked": "Quick fix",
            "what_failed": "Nothing",
            "lesson": "Simple tasks need simple solutions",
            "should_create_skill": false
        }"#;
        let result = parse_reflection_response(json).unwrap();
        assert!(!result.should_create_skill);
        assert!(result.skill_name.is_none());
    }

    #[test]
    fn parse_reflection_code_block() {
        let json = "```json\n{\"what_worked\": \"x\", \"what_failed\": \"y\", \"lesson\": \"z\", \"should_create_skill\": false}\n```";
        let result = parse_reflection_response(json).unwrap();
        assert_eq!(result.lesson, "z");
    }

    #[test]
    fn parse_malformed_returns_error() {
        let result = parse_reflection_response("not json");
        assert!(result.is_err());
    }

    #[test]
    fn pipeline_run_summary_descriptions() {
        let summary = PipelineRunSummary {
            run_id: "r1".into(),
            task: "deploy".into(),
            outcome: ReflectionOutcome::Success,
            steps: vec!["build".into(), "test".into()],
            errors: vec![],
        };
        assert!(summary.steps_description().contains("build"));
        assert_eq!(summary.errors_description(), "none");
    }
}
