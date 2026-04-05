use crate::domain::standing_order::SystemEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemEventComponentStatus {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct HeartbeatProjection {
    pub total_ticks: u64,
    pub consecutive_successes: u64,
    pub consecutive_failures: u64,
    pub avg_tick_duration_ms: f64,
    pub active_task_count: usize,
    pub executed_task_count: usize,
    pub high_priority_task_count: usize,
}

#[derive(Debug, Clone)]
pub struct SystemEventProjectionInput {
    pub event: SystemEvent,
    pub timestamp_rfc3339: String,
    pub agent_id: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub components: Vec<SystemEventComponentStatus>,
    pub heartbeat: Option<HeartbeatProjection>,
}

pub fn render_system_event_report(input: &SystemEventProjectionInput) -> String {
    let component_summary = if input.components.is_empty() {
        "no components reported yet".to_string()
    } else {
        input.components
            .iter()
            .map(|component| format!("{}={}", component.name, component.status))
            .collect::<Vec<_>>()
            .join(", ")
    };

    match &input.event {
        SystemEvent::RuntimeRestarted => format!(
            "[Restart Report]\nTime: {}\nAgent: {}\nUptime: {}s\nComponents: {}",
            input.timestamp_rfc3339,
            input.agent_id.as_deref().unwrap_or("unknown"),
            input.uptime_seconds.unwrap_or(0),
            component_summary
        ),
        SystemEvent::HeartbeatTick => {
            let metrics_line = input
                .heartbeat
                .as_ref()
                .map(|metrics| {
                    format!(
                        "Ticks: {} | Success streak: {} | Failure streak: {} | Avg tick: {:.0}ms | Tasks: {}/{} | High priority: {}",
                        metrics.total_ticks,
                        metrics.consecutive_successes,
                        metrics.consecutive_failures,
                        metrics.avg_tick_duration_ms,
                        metrics.executed_task_count,
                        metrics.active_task_count,
                        metrics.high_priority_task_count
                    )
                })
                .unwrap_or_else(|| "Ticks: unavailable".to_string());
            format!(
                "[Heartbeat Report]\nTime: {}\n{}\nComponents: {}",
                input.timestamp_rfc3339, metrics_line, component_summary
            )
        }
        SystemEvent::OperatorEvent { text } => format!(
            "[System Event]\nTime: {}\nEvent: {}\nComponents: {}",
            input.timestamp_rfc3339, text, component_summary
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(name: &str, status: &str) -> SystemEventComponentStatus {
        SystemEventComponentStatus {
            name: name.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn renders_restart_report_with_agent_and_uptime() {
        let report = render_system_event_report(&SystemEventProjectionInput {
            event: SystemEvent::RuntimeRestarted,
            timestamp_rfc3339: "2026-04-05T12:00:00Z".into(),
            agent_id: Some("local-agent".into()),
            uptime_seconds: Some(42),
            components: vec![component("gateway", "ok"), component("heartbeat", "ok")],
            heartbeat: None,
        });

        assert!(report.contains("[Restart Report]"));
        assert!(report.contains("Agent: local-agent"));
        assert!(report.contains("Uptime: 42s"));
        assert!(report.contains("gateway=ok, heartbeat=ok"));
    }

    #[test]
    fn renders_heartbeat_report_with_metrics() {
        let report = render_system_event_report(&SystemEventProjectionInput {
            event: SystemEvent::HeartbeatTick,
            timestamp_rfc3339: "2026-04-05T12:00:00Z".into(),
            agent_id: None,
            uptime_seconds: None,
            components: vec![component("heartbeat", "ok")],
            heartbeat: Some(HeartbeatProjection {
                total_ticks: 7,
                consecutive_successes: 3,
                consecutive_failures: 0,
                avg_tick_duration_ms: 125.4,
                active_task_count: 4,
                executed_task_count: 2,
                high_priority_task_count: 1,
            }),
        });

        assert!(report.contains("[Heartbeat Report]"));
        assert!(report.contains("Ticks: 7"));
        assert!(report.contains("Success streak: 3"));
        assert!(report.contains("Avg tick: 125ms"));
        assert!(report.contains("Tasks: 2/4"));
        assert!(report.contains("High priority: 1"));
    }

    #[test]
    fn renders_operator_event_report() {
        let report = render_system_event_report(&SystemEventProjectionInput {
            event: SystemEvent::OperatorEvent {
                text: "reload config".into(),
            },
            timestamp_rfc3339: "2026-04-05T12:00:00Z".into(),
            agent_id: None,
            uptime_seconds: None,
            components: vec![],
            heartbeat: None,
        });

        assert!(report.contains("[System Event]"));
        assert!(report.contains("Event: reload config"));
        assert!(report.contains("no components reported yet"));
    }
}
