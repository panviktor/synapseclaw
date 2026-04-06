use synapse_cron::{CronJob, Schedule, SessionTarget};
use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::ports::agent_runtime::AgentToolFact;

pub(crate) fn schedule_kind(schedule: &Schedule) -> &'static str {
    match schedule {
        Schedule::Cron { .. } => "cron",
        Schedule::At { .. } => "at",
        Schedule::Every { .. } => "every",
    }
}

pub(crate) fn build_job_fact(tool_name: &str, action: &str, job: &CronJob) -> AgentToolFact {
    AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: vec![FocusEntity {
            kind: "scheduled_job".into(),
            name: job.id.clone(),
            metadata: Some(format!(
                "{}:{}:{}",
                action,
                schedule_kind(&job.schedule),
                match job.session_target {
                    SessionTarget::Isolated => "isolated",
                    SessionTarget::Main => "main",
                }
            )),
        }],
        slots: Vec::new(),
    }
}

pub(crate) fn build_job_reference_fact(
    tool_name: &str,
    _action: &str,
    job_id: &str,
    metadata: Option<&str>,
) -> AgentToolFact {
    AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: vec![FocusEntity {
            kind: "scheduled_job".into(),
            name: job_id.to_string(),
            metadata: metadata.map(str::to_string),
        }],
        slots: Vec::new(),
    }
}

pub(crate) fn build_removed_job_fact(tool_name: &str, action: &str, job_id: &str) -> AgentToolFact {
    build_job_reference_fact(tool_name, action, job_id, Some(action))
}

pub(crate) fn build_job_run_history_fact(
    tool_name: &str,
    job_id: &str,
    run_count: usize,
    latest_status: Option<&str>,
    latest_duration_ms: Option<i64>,
) -> AgentToolFact {
    let mut fact = build_job_reference_fact(tool_name, "runs", job_id, Some("run_history"));
    fact.focus_entities.push(FocusEntity {
        kind: "run_history".into(),
        name: run_count.to_string(),
        metadata: latest_status
            .map(str::to_string)
            .or_else(|| latest_duration_ms.map(|duration| duration.to_string())),
    });
    fact
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_job_reference_fact_keeps_reference_generic() {
        let fact = build_job_reference_fact("cron_runs", "inspect", "job-123", Some("history"));

        assert_eq!(fact.focus_entities[0].kind, "scheduled_job");
        assert_eq!(fact.focus_entities[0].name, "job-123");
        assert_eq!(fact.focus_entities[0].metadata.as_deref(), Some("history"));
        assert!(fact.slots.is_empty());
    }

    #[test]
    fn build_job_run_history_fact_emits_run_history_slots() {
        let fact = build_job_run_history_fact("cron_runs", "job-123", 4, Some("ok"), Some(250));

        assert_eq!(
            fact.focus_entities[0].metadata.as_deref(),
            Some("run_history")
        );
        assert!(fact
            .focus_entities
            .iter()
            .any(|entity| entity.kind == "run_history" && entity.name == "4"));
    }
}
