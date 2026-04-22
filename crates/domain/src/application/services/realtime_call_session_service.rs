use crate::ports::realtime_call::{
    validate_realtime_call_transition, RealtimeCallDirection, RealtimeCallKind, RealtimeCallOrigin,
    RealtimeCallSessionSnapshot, RealtimeCallState, RealtimeCallTriggerSource,
};
use chrono::{DateTime, Duration, Utc};

const MAX_END_REASON_CHARS: usize = 64;
const MAX_SUMMARY_CHARS: usize = 280;
const MAX_DECISIONS: usize = 6;
const MAX_DECISION_CHARS: usize = 140;

fn utc_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn bounded_text(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut bounded = trimmed.chars().take(limit).collect::<String>();
    if trimmed.chars().count() > limit {
        bounded.push_str("...");
    }
    Some(bounded)
}

fn bounded_decisions(items: &[String]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| bounded_text(item, MAX_DECISION_CHARS))
        .take(MAX_DECISIONS)
        .collect()
}

fn transition_call_state(call: &mut RealtimeCallSessionSnapshot, next: RealtimeCallState) {
    if validate_realtime_call_transition(call.state, next).is_ok() {
        call.state = next;
    }
}

fn transition_intermediate_state(
    current: RealtimeCallState,
    next: RealtimeCallState,
) -> Option<RealtimeCallState> {
    match (current, next) {
        (RealtimeCallState::Created | RealtimeCallState::Ringing, RealtimeCallState::Listening)
        | (RealtimeCallState::Created | RealtimeCallState::Ringing, RealtimeCallState::Speaking)
        | (RealtimeCallState::Created | RealtimeCallState::Ringing, RealtimeCallState::Thinking) => {
            Some(RealtimeCallState::Connected)
        }
        (RealtimeCallState::Connected, RealtimeCallState::Thinking) => {
            Some(RealtimeCallState::Listening)
        }
        _ => None,
    }
}

fn ensure_call_state(call: &mut RealtimeCallSessionSnapshot, next: RealtimeCallState) {
    if validate_realtime_call_transition(call.state, next).is_ok() {
        call.state = next;
        return;
    }
    if let Some(intermediate) = transition_intermediate_state(call.state, next) {
        ensure_call_state(call, intermediate);
        ensure_call_state(call, next);
    }
}

fn terminal_summary(
    call: &RealtimeCallSessionSnapshot,
    end_reason: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(objective) = call.objective.as_deref() {
        parts.push(format!("objective: {objective}"));
    }
    parts.push(format!("state: {}", state_label(call.state)));
    if let Some(end_reason) =
        end_reason.and_then(|reason| bounded_text(reason, MAX_END_REASON_CHARS))
    {
        parts.push(format!("reason: {end_reason}"));
    }
    parts.push(format!("turns: {}", call.message_count));
    if call.interruption_count > 0 {
        parts.push(format!("interruptions: {}", call.interruption_count));
    }
    bounded_text(&parts.join(" | "), MAX_SUMMARY_CHARS)
}

fn state_label(state: RealtimeCallState) -> &'static str {
    match state {
        RealtimeCallState::Created => "created",
        RealtimeCallState::Ringing => "ringing",
        RealtimeCallState::Connected => "connected",
        RealtimeCallState::Listening => "listening",
        RealtimeCallState::Thinking => "thinking",
        RealtimeCallState::Speaking => "speaking",
        RealtimeCallState::Ended => "ended",
        RealtimeCallState::Failed => "failed",
    }
}

fn apply_terminal_outcome(
    call: &mut RealtimeCallSessionSnapshot,
    end_reason: Option<&str>,
    summary: Option<&str>,
    decisions: &[String],
) {
    call.end_reason = end_reason.and_then(|reason| bounded_text(reason, MAX_END_REASON_CHARS));
    call.summary = summary
        .and_then(|summary| bounded_text(summary, MAX_SUMMARY_CHARS))
        .or_else(|| terminal_summary(call, end_reason));
    call.decisions = bounded_decisions(decisions);
}

fn merge_call_origin(call: &mut RealtimeCallSessionSnapshot, origin: &RealtimeCallOrigin) {
    if matches!(call.origin.source, RealtimeCallTriggerSource::Unknown)
        && !matches!(origin.source, RealtimeCallTriggerSource::Unknown)
    {
        call.origin.source = origin.source;
    }
    if call.origin.conversation_id.is_none() && origin.conversation_id.is_some() {
        call.origin.conversation_id = origin.conversation_id.clone();
    }
    if call.origin.channel.is_none() && origin.channel.is_some() {
        call.origin.channel = origin.channel.clone();
    }
    if call.origin.recipient.is_none() && origin.recipient.is_some() {
        call.origin.recipient = origin.recipient.clone();
    }
    if call.origin.thread_ref.is_none() && origin.thread_ref.is_some() {
        call.origin.thread_ref = origin.thread_ref.clone();
    }
}

fn upsert_session<'a>(
    sessions: &'a mut Vec<RealtimeCallSessionSnapshot>,
    channel: &str,
    kind: RealtimeCallKind,
    call_control_id: &str,
    direction: RealtimeCallDirection,
    origin: RealtimeCallOrigin,
    objective: Option<String>,
) -> &'a mut RealtimeCallSessionSnapshot {
    if let Some(index) = sessions
        .iter()
        .position(|call| call.call_control_id == call_control_id)
    {
        let call = &mut sessions[index];
        if matches!(call.direction, RealtimeCallDirection::Unknown)
            && !matches!(direction, RealtimeCallDirection::Unknown)
        {
            call.direction = direction;
        }
        merge_call_origin(call, &origin);
        if call.objective.is_none() && objective.is_some() {
            call.objective = objective;
        }
        return call;
    }

    let now = utc_timestamp();
    sessions.push(RealtimeCallSessionSnapshot {
        channel: channel.to_string(),
        kind,
        direction,
        origin,
        objective,
        call_control_id: call_control_id.to_string(),
        call_leg_id: None,
        call_session_id: None,
        state: RealtimeCallState::Created,
        created_at: now.clone(),
        updated_at: now,
        ended_at: None,
        end_reason: None,
        summary: None,
        decisions: Vec::new(),
        message_count: 0,
        interruption_count: 0,
        last_sequence: None,
    });
    sessions
        .last_mut()
        .expect("realtime call session was just inserted")
}

pub fn active_realtime_call_sessions(
    sessions: &[RealtimeCallSessionSnapshot],
) -> Vec<RealtimeCallSessionSnapshot> {
    sessions
        .iter()
        .filter(|session| !session.state.is_terminal())
        .cloned()
        .collect()
}

pub fn trim_recent_realtime_call_sessions(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    max_sessions: usize,
) {
    if sessions.len() > max_sessions {
        let overflow = sessions.len() - max_sessions;
        sessions.drain(0..overflow);
    }
}

pub fn cleanup_stale_realtime_call_sessions(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    now: DateTime<Utc>,
    idle_timeout_secs: i64,
) -> usize {
    if idle_timeout_secs <= 0 {
        return 0;
    }
    let cutoff = now - Duration::seconds(idle_timeout_secs);
    let now_rfc3339 = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut cleaned = 0_usize;

    for call in sessions.iter_mut() {
        if call.state.is_terminal() {
            continue;
        }
        let updated_at = match DateTime::parse_from_rfc3339(&call.updated_at) {
            Ok(updated_at) => updated_at.with_timezone(&Utc),
            Err(_) => continue,
        };
        if updated_at > cutoff {
            continue;
        }

        transition_call_state(call, RealtimeCallState::Failed);
        call.updated_at = now_rfc3339.clone();
        call.ended_at = Some(now_rfc3339.clone());
        apply_terminal_outcome(call, Some("idle_timeout"), None, &[]);
        cleaned = cleaned.saturating_add(1);
    }

    cleaned
}

pub fn record_realtime_inbound_call_started(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    channel: &str,
    kind: RealtimeCallKind,
    call_control_id: &str,
) {
    let call = upsert_session(
        sessions,
        channel,
        kind,
        call_control_id,
        RealtimeCallDirection::Inbound,
        RealtimeCallOrigin::inbound_transport(),
        None,
    );
    ensure_call_state(call, RealtimeCallState::Connected);
}

pub fn record_realtime_inbound_call_message(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    channel: &str,
    kind: RealtimeCallKind,
    call_control_id: &str,
    sequence: Option<u64>,
    is_interruption: bool,
) {
    let call = upsert_session(
        sessions,
        channel,
        kind,
        call_control_id,
        RealtimeCallDirection::Inbound,
        RealtimeCallOrigin::inbound_transport(),
        None,
    );
    if matches!(
        call.state,
        RealtimeCallState::Created | RealtimeCallState::Ringing
    ) {
        ensure_call_state(call, RealtimeCallState::Connected);
    }
    ensure_call_state(call, RealtimeCallState::Listening);
    call.updated_at = utc_timestamp();
    call.last_sequence = sequence;
    call.message_count = call.message_count.saturating_add(1);
    if is_interruption {
        call.interruption_count = call.interruption_count.saturating_add(1);
    }
}

pub fn record_realtime_call_state(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    channel: &str,
    kind: RealtimeCallKind,
    call_control_id: &str,
    direction: RealtimeCallDirection,
    state: RealtimeCallState,
) {
    let call = upsert_session(
        sessions,
        channel,
        kind,
        call_control_id,
        direction,
        RealtimeCallOrigin::default(),
        None,
    );
    ensure_call_state(call, state);
    call.updated_at = utc_timestamp();
    if state.is_terminal() {
        call.ended_at = Some(utc_timestamp());
        apply_terminal_outcome(call, None, None, &[]);
    }
}

pub fn record_realtime_call_state_with_context(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    channel: &str,
    kind: RealtimeCallKind,
    call_control_id: &str,
    direction: RealtimeCallDirection,
    state: RealtimeCallState,
    origin: RealtimeCallOrigin,
    call_session_id: Option<&str>,
    objective: Option<String>,
) {
    let call = upsert_session(
        sessions,
        channel,
        kind,
        call_control_id,
        direction,
        origin,
        objective,
    );
    if call.call_session_id.is_none() {
        call.call_session_id = call_session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
    }
    ensure_call_state(call, state);
    call.updated_at = utc_timestamp();
    if state.is_terminal() {
        call.ended_at = Some(utc_timestamp());
        apply_terminal_outcome(call, None, None, &[]);
    }
}

pub fn record_realtime_call_ended(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    call_control_id: &str,
    end_reason: Option<&str>,
    summary: Option<&str>,
    decisions: &[String],
) {
    if let Some(call) = sessions
        .iter_mut()
        .find(|call| call.call_control_id == call_control_id)
    {
        ensure_call_state(call, RealtimeCallState::Ended);
        call.updated_at = utc_timestamp();
        call.ended_at = Some(utc_timestamp());
        apply_terminal_outcome(call, end_reason, summary, decisions);
    }
}

pub fn record_realtime_call_outcome(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    call_control_id: &str,
    summary: Option<&str>,
    decisions: &[String],
) {
    if let Some(call) = sessions
        .iter_mut()
        .find(|call| call.call_control_id == call_control_id)
    {
        if let Some(summary) = summary.and_then(|summary| bounded_text(summary, MAX_SUMMARY_CHARS))
        {
            call.summary = Some(summary);
        }
        let decisions = bounded_decisions(decisions);
        if !decisions.is_empty() {
            call.decisions = decisions;
        }
        call.updated_at = utc_timestamp();
    }
}

pub fn record_realtime_outbound_call_started(
    sessions: &mut Vec<RealtimeCallSessionSnapshot>,
    channel: &str,
    kind: RealtimeCallKind,
    call_control_id: &str,
    call_leg_id: Option<&str>,
    call_session_id: Option<&str>,
    origin: RealtimeCallOrigin,
    objective: Option<String>,
) {
    let call = upsert_session(
        sessions,
        channel,
        kind,
        call_control_id,
        RealtimeCallDirection::Outbound,
        origin,
        objective,
    );
    call.call_leg_id = call_leg_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    call.call_session_id = call_session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    ensure_call_state(call, RealtimeCallState::Ringing);
    call.updated_at = utc_timestamp();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_message_updates_existing_session_counters() {
        let mut sessions = Vec::new();
        record_realtime_inbound_call_started(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-1",
        );
        record_realtime_inbound_call_message(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-1",
            Some(7),
            true,
        );

        assert_eq!(sessions.len(), 1);
        let call = &sessions[0];
        assert_eq!(call.direction, RealtimeCallDirection::Inbound);
        assert_eq!(call.state, RealtimeCallState::Listening);
        assert_eq!(call.message_count, 1);
        assert_eq!(call.interruption_count, 1);
        assert_eq!(call.last_sequence, Some(7));
    }

    #[test]
    fn outbound_start_and_end_preserve_session_identity() {
        let mut sessions = Vec::new();
        record_realtime_outbound_call_started(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-2",
            Some("leg-2"),
            Some("sess-2"),
            RealtimeCallOrigin::scheduled_job(),
            Some("Morning work briefing".into()),
        );
        record_realtime_call_state(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-2",
            RealtimeCallDirection::Outbound,
            RealtimeCallState::Speaking,
        );
        record_realtime_call_ended(
            &mut sessions,
            "call-2",
            Some("operator_hangup"),
            Some("Reviewed the task list and ended the call."),
            &["Keep the 10:00 planning call.".into()],
        );

        assert_eq!(sessions.len(), 1);
        let call = &sessions[0];
        assert_eq!(call.call_leg_id.as_deref(), Some("leg-2"));
        assert_eq!(call.call_session_id.as_deref(), Some("sess-2"));
        assert_eq!(call.origin.source, RealtimeCallTriggerSource::ScheduledJob);
        assert_eq!(call.objective.as_deref(), Some("Morning work briefing"));
        assert_eq!(call.state, RealtimeCallState::Ended);
        assert!(call.ended_at.is_some());
        assert_eq!(call.end_reason.as_deref(), Some("operator_hangup"));
        assert_eq!(
            call.summary.as_deref(),
            Some("Reviewed the task list and ended the call.")
        );
        assert_eq!(call.decisions, vec!["Keep the 10:00 planning call."]);
    }

    #[test]
    fn active_sessions_filter_terminals_and_trim_is_bounded() {
        let mut sessions = Vec::new();
        record_realtime_inbound_call_started(
            &mut sessions,
            "matrix",
            RealtimeCallKind::Audio,
            "call-a",
        );
        record_realtime_inbound_call_started(
            &mut sessions,
            "matrix",
            RealtimeCallKind::Audio,
            "call-b",
        );
        record_realtime_call_ended(&mut sessions, "call-a", None, None, &[]);

        let active = active_realtime_call_sessions(&sessions);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].call_control_id, "call-b");

        trim_recent_realtime_call_sessions(&mut sessions, 1);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].call_control_id, "call-b");
    }

    #[test]
    fn stale_active_sessions_are_failed_and_ended() {
        let mut sessions = vec![RealtimeCallSessionSnapshot {
            channel: "clawdtalk".into(),
            kind: RealtimeCallKind::Audio,
            direction: RealtimeCallDirection::Outbound,
            origin: RealtimeCallOrigin::api_request(),
            objective: Some("Check whether the store has the item in stock.".into()),
            call_control_id: "call-stale".into(),
            call_leg_id: None,
            call_session_id: None,
            state: RealtimeCallState::Listening,
            created_at: "2026-04-21T08:00:00Z".into(),
            updated_at: "2026-04-21T08:00:00Z".into(),
            ended_at: None,
            end_reason: None,
            summary: None,
            decisions: Vec::new(),
            message_count: 1,
            interruption_count: 0,
            last_sequence: Some(1),
        }];

        let cleaned = cleanup_stale_realtime_call_sessions(
            &mut sessions,
            DateTime::parse_from_rfc3339("2026-04-21T08:20:00Z")
                .unwrap()
                .with_timezone(&Utc),
            300,
        );

        assert_eq!(cleaned, 1);
        assert_eq!(sessions[0].state, RealtimeCallState::Failed);
        assert!(sessions[0].ended_at.is_some());
        assert_eq!(sessions[0].end_reason.as_deref(), Some("idle_timeout"));
        assert!(sessions[0].summary.is_some());
        assert_eq!(
            sessions[0].objective.as_deref(),
            Some("Check whether the store has the item in stock.")
        );
    }

    #[test]
    fn state_updates_can_walk_required_intermediate_steps() {
        let mut sessions = Vec::new();
        record_realtime_call_state(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-walk",
            RealtimeCallDirection::Unknown,
            RealtimeCallState::Thinking,
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, RealtimeCallState::Thinking);
        assert_eq!(
            sessions[0].origin.source,
            RealtimeCallTriggerSource::Unknown
        );
    }

    #[test]
    fn state_updates_with_context_preserve_room_and_origin() {
        let mut sessions = Vec::new();
        record_realtime_call_state_with_context(
            &mut sessions,
            "matrix",
            RealtimeCallKind::Audio,
            "$mx-call",
            RealtimeCallDirection::Inbound,
            RealtimeCallState::Ringing,
            RealtimeCallOrigin {
                source: RealtimeCallTriggerSource::InboundTransport,
                conversation_id: Some("!room:matrix.org".into()),
                channel: Some("matrix".into()),
                recipient: Some("@user:matrix.org".into()),
                thread_ref: None,
            },
            Some("!room:matrix.org"),
            None,
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, RealtimeCallState::Ringing);
        assert_eq!(sessions[0].direction, RealtimeCallDirection::Inbound);
        assert_eq!(
            sessions[0].call_session_id.as_deref(),
            Some("!room:matrix.org")
        );
        assert_eq!(
            sessions[0].origin.recipient.as_deref(),
            Some("@user:matrix.org")
        );
        assert_eq!(sessions[0].origin.channel.as_deref(), Some("matrix"));
    }

    #[test]
    fn non_terminal_call_can_record_compact_outcome_without_ending() {
        let mut sessions = Vec::new();
        record_realtime_inbound_call_started(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-outcome",
        );

        record_realtime_call_outcome(
            &mut sessions,
            "call-outcome",
            Some("User asked to skip the evening reminder."),
            &["Skip the evening reminder.".into()],
        );

        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("User asked to skip the evening reminder.")
        );
        assert_eq!(sessions[0].decisions, vec!["Skip the evening reminder."]);
        assert_eq!(sessions[0].state, RealtimeCallState::Connected);
        assert!(sessions[0].ended_at.is_none());
    }

    #[test]
    fn origin_metadata_is_merged_when_outbound_details_arrive_later() {
        let mut sessions = Vec::new();
        record_realtime_call_state(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-origin",
            RealtimeCallDirection::Outbound,
            RealtimeCallState::Ringing,
        );
        record_realtime_outbound_call_started(
            &mut sessions,
            "clawdtalk",
            RealtimeCallKind::Audio,
            "call-origin",
            Some("leg-origin"),
            None,
            RealtimeCallOrigin::chat_request(
                Some("matrix-ops".into()),
                Some("matrix".into()),
                Some("!ops:example".into()),
                Some("$thread".into()),
            ),
            Some("Call the operator back.".into()),
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].origin.source,
            RealtimeCallTriggerSource::ChatRequest
        );
        assert_eq!(sessions[0].origin.channel.as_deref(), Some("matrix"));
        assert_eq!(
            sessions[0].origin.recipient.as_deref(),
            Some("!ops:example")
        );
        assert_eq!(sessions[0].call_leg_id.as_deref(), Some("leg-origin"));
    }
}
