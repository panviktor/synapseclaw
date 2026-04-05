use synapse_cron::{CronJob, Schedule, SessionTarget};
use synapse_domain::domain::dialogue_state::{DialogueSlot, FocusEntity};
use synapse_domain::ports::agent_runtime::AgentToolFact;

pub(crate) fn schedule_kind(schedule: &Schedule) -> &'static str {
    match schedule {
        Schedule::Cron { .. } => "cron",
        Schedule::At { .. } => "at",
        Schedule::Every { .. } => "every",
    }
}

pub(crate) fn build_job_fact(tool_name: &str, action: &str, job: &CronJob) -> AgentToolFact {
    let mut slots = vec![
        DialogueSlot::observed("schedule_action", action.to_string()),
        DialogueSlot::observed("job_id", job.id.clone()),
        DialogueSlot::observed("job_type", <&'static str>::from(job.job_type.clone())),
        DialogueSlot::observed("schedule_kind", schedule_kind(&job.schedule)),
        DialogueSlot::observed("job_enabled", job.enabled.to_string()),
        DialogueSlot::observed("job_next_run", job.next_run.to_rfc3339()),
        DialogueSlot::observed(
            "session_target",
            match job.session_target {
                SessionTarget::Isolated => "isolated",
                SessionTarget::Main => "main",
            },
        ),
        DialogueSlot::observed("delete_after_run", job.delete_after_run.to_string()),
    ];

    if let Some(name) = &job.name {
        slots.push(DialogueSlot::observed("job_name", name.clone()));
    }
    if let Some(model) = &job.model {
        slots.push(DialogueSlot::observed("job_model", model.clone()));
    }
    if !job.delivery.mode.trim().is_empty() {
        slots.push(DialogueSlot::observed("delivery_mode", job.delivery.mode.clone()));
    }
    if let Some(channel) = &job.delivery.channel {
        slots.push(DialogueSlot::observed("delivery_channel", channel.clone()));
    }
    if let Some(to) = &job.delivery.to {
        slots.push(DialogueSlot::observed("delivery_to", to.clone()));
    }
    if let Some(thread_ref) = &job.delivery.thread_ref {
        slots.push(DialogueSlot::observed(
            "delivery_thread_ref",
            thread_ref.clone(),
        ));
    }

    AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: vec![FocusEntity {
            kind: "scheduled_job".into(),
            name: job.id.clone(),
            metadata: Some(schedule_kind(&job.schedule).to_string()),
        }],
        slots,
    }
}

pub(crate) fn build_job_reference_fact(
    tool_name: &str,
    action: &str,
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
        slots: vec![
            DialogueSlot::observed("schedule_action", action.to_string()),
            DialogueSlot::observed("job_id", job_id.to_string()),
        ],
    }
}

pub(crate) fn build_removed_job_fact(
    tool_name: &str,
    action: &str,
    job_id: &str,
) -> AgentToolFact {
    let mut fact = build_job_reference_fact(tool_name, action, job_id, Some(action));
    fact.slots
        .push(DialogueSlot::observed("job_enabled", "false".to_string()));
    fact
}

pub(crate) fn build_job_run_history_fact(
    tool_name: &str,
    job_id: &str,
    run_count: usize,
    latest_status: Option<&str>,
    latest_duration_ms: Option<i64>,
) -> AgentToolFact {
    let mut fact = build_job_reference_fact(tool_name, "runs", job_id, Some("run_history"));
    fact.slots.push(DialogueSlot::observed(
        "run_history_count",
        run_count.to_string(),
    ));
    if let Some(status) = latest_status {
        fact.slots
            .push(DialogueSlot::observed("latest_run_status", status.to_string()));
    }
    if let Some(duration_ms) = latest_duration_ms {
        fact.slots.push(DialogueSlot::observed(
            "latest_run_duration_ms",
            duration_ms.to_string(),
        ));
    }
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
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "schedule_action" && slot.value == "inspect")
        );
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "job_id" && slot.value == "job-123")
        );
    }

    #[test]
    fn build_job_run_history_fact_emits_run_history_slots() {
        let fact = build_job_run_history_fact("cron_runs", "job-123", 4, Some("ok"), Some(250));

        assert_eq!(fact.focus_entities[0].metadata.as_deref(), Some("run_history"));
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "run_history_count" && slot.value == "4")
        );
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "latest_run_status" && slot.value == "ok")
        );
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "latest_run_duration_ms" && slot.value == "250")
        );
    }
}
