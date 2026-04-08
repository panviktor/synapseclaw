use synapse_cron::{CronJob, Schedule, SessionTarget};
use synapse_domain::domain::conversation_target::ConversationDeliveryTarget;
use synapse_domain::domain::tool_fact::{
    ScheduleAction, ScheduleFact, ScheduleJobType, ScheduleKind, ScheduleTarget, ToolFactPayload,
    TypedToolFact,
};

pub(crate) fn schedule_kind(schedule: &Schedule) -> ScheduleKind {
    match schedule {
        Schedule::Cron { .. } => ScheduleKind::Cron,
        Schedule::At { .. } => ScheduleKind::At,
        Schedule::Every { .. } => ScheduleKind::Every,
    }
}

pub(crate) fn build_job_fact(tool_name: &str, action: &str, job: &CronJob) -> TypedToolFact {
    TypedToolFact {
        tool_id: tool_name.to_string(),
        payload: ToolFactPayload::Schedule(ScheduleFact {
            action: parse_schedule_action(action),
            job_type: Some(parse_job_type(job)),
            schedule_kind: Some(schedule_kind(&job.schedule)),
            job_id: Some(job.id.clone()),
            annotation: None,
            timezone: match &job.schedule {
                Schedule::Cron { tz, .. } => tz.clone(),
                _ => None,
            },
            target: Some(ScheduleTarget {
                session: Some(match job.session_target {
                    SessionTarget::Isolated => "isolated".to_string(),
                    SessionTarget::Main => "main".to_string(),
                }),
                delivery: delivery_target_from_job(job),
            }),
            run_count: None,
            last_status: None,
            last_duration_ms: None,
        }),
    }
}

pub(crate) fn build_job_reference_fact(
    tool_name: &str,
    _action: &str,
    job_id: &str,
    metadata: Option<&str>,
) -> TypedToolFact {
    TypedToolFact {
        tool_id: tool_name.to_string(),
        payload: ToolFactPayload::Schedule(ScheduleFact {
            action: ScheduleAction::Inspect,
            job_type: None,
            schedule_kind: None,
            job_id: Some(job_id.to_string()),
            annotation: metadata.map(str::to_string),
            timezone: None,
            target: None,
            run_count: None,
            last_status: None,
            last_duration_ms: None,
        }),
    }
}

pub(crate) fn build_removed_job_fact(tool_name: &str, action: &str, job_id: &str) -> TypedToolFact {
    build_job_reference_fact(tool_name, action, job_id, Some(action))
}

pub(crate) fn build_job_run_history_fact(
    tool_name: &str,
    job_id: &str,
    run_count: usize,
    latest_status: Option<&str>,
    latest_duration_ms: Option<i64>,
) -> TypedToolFact {
    TypedToolFact {
        tool_id: tool_name.to_string(),
        payload: ToolFactPayload::Schedule(ScheduleFact {
            action: ScheduleAction::Inspect,
            job_type: None,
            schedule_kind: None,
            job_id: Some(job_id.to_string()),
            annotation: Some("run_history".to_string()),
            timezone: None,
            target: None,
            run_count: Some(run_count),
            last_status: latest_status.map(str::to_string),
            last_duration_ms: latest_duration_ms,
        }),
    }
}

fn parse_schedule_action(action: &str) -> ScheduleAction {
    match action.trim().to_ascii_lowercase().as_str() {
        "create" => ScheduleAction::Create,
        "update" => ScheduleAction::Update,
        "remove" | "delete" => ScheduleAction::Remove,
        "run" => ScheduleAction::Run,
        "list" => ScheduleAction::List,
        _ => ScheduleAction::Inspect,
    }
}

fn parse_job_type(job: &CronJob) -> ScheduleJobType {
    match job.job_type {
        synapse_cron::JobType::Agent => {
            if job.delivery.mode.eq_ignore_ascii_case("announce") {
                ScheduleJobType::Delivery
            } else if job
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case("heartbeat"))
            {
                ScheduleJobType::Heartbeat
            } else {
                ScheduleJobType::Agent
            }
        }
        synapse_cron::JobType::Shell => ScheduleJobType::Shell,
    }
}

fn delivery_target_from_job(job: &CronJob) -> Option<ConversationDeliveryTarget> {
    if !job.delivery.mode.eq_ignore_ascii_case("announce") {
        return None;
    }

    match (&job.delivery.channel, &job.delivery.to) {
        (Some(channel), Some(recipient)) => Some(ConversationDeliveryTarget::Explicit {
            channel: channel.clone(),
            recipient: recipient.clone(),
            thread_ref: job.delivery.thread_ref.clone(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_job_reference_fact_keeps_reference_generic() {
        let fact = build_job_reference_fact("cron_runs", "inspect", "job-123", Some("history"));

        let projected = fact.projected_focus_entities();
        assert_eq!(projected[0].kind, "scheduled_job");
        assert_eq!(projected[0].name, "job-123");
        assert_eq!(projected[0].metadata.as_deref(), Some("history"));
        assert!(fact
            .projected_subjects()
            .iter()
            .any(|subject| subject == "job-123"));
    }

    #[test]
    fn build_job_run_history_fact_emits_run_history_slots() {
        let fact = build_job_run_history_fact("cron_runs", "job-123", 4, Some("ok"), Some(250));

        let projected = fact.projected_focus_entities();
        assert_eq!(projected[0].metadata.as_deref(), Some("run_history"));
        assert!(fact
            .projected_focus_entities()
            .iter()
            .any(|entity| entity.kind == "run_history" && entity.name == "4"));
    }
}
