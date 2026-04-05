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

pub(crate) fn build_removed_job_fact(
    tool_name: &str,
    action: &str,
    job_id: &str,
) -> AgentToolFact {
    AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: vec![FocusEntity {
            kind: "scheduled_job".into(),
            name: job_id.to_string(),
            metadata: Some(action.to_string()),
        }],
        slots: vec![
            DialogueSlot::observed("schedule_action", action.to_string()),
            DialogueSlot::observed("job_id", job_id.to_string()),
            DialogueSlot::observed("job_enabled", "false".to_string()),
        ],
    }
}
