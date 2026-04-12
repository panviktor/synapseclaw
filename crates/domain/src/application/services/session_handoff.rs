use crate::application::services::retrieval_service::{RunRecipeSearchMatch, SessionSearchMatch};
use crate::application::services::runtime_assumptions::{self, RuntimeAssumption};
use crate::application::services::turn_interpretation::{ReferenceCandidate, TurnInterpretation};
use crate::domain::conversation_target::ConversationDeliveryTarget;
use crate::domain::memory::MemoryEntry;
use crate::domain::turn_admission::{
    admission_repair_hint_label, AdmissionRepairHint, CandidateAdmissionReason,
};
use crate::domain::user_profile::DELIVERY_TARGET_PREFERENCE_KEY;

const MAX_DEFAULTS: usize = 6;
const MAX_ANCHORS: usize = 4;
const MAX_UNRESOLVED: usize = 3;
const MAX_ITEM_CHARS: usize = 180;
const MAX_TASK_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionHandoffReason {
    ContextOverflow,
    RouteSwitch,
    Compaction,
    CapabilityRepair,
    SessionResume,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHandoffPacket {
    pub reason: SessionHandoffReason,
    #[serde(default)]
    pub recommended_action: Option<String>,
    #[serde(default)]
    pub active_task: Option<String>,
    #[serde(default)]
    pub current_defaults: Vec<String>,
    #[serde(default)]
    pub anchors: Vec<String>,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<RuntimeAssumption>,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionHandoffInput<'a> {
    pub user_message: &'a str,
    pub interpretation: Option<&'a TurnInterpretation>,
    pub recent_admission_repair: Option<AdmissionRepairHint>,
    pub recent_admission_reasons: &'a [CandidateAdmissionReason],
    pub recalled_entries: &'a [MemoryEntry],
    pub session_matches: &'a [SessionSearchMatch],
    pub run_recipes: &'a [RunRecipeSearchMatch],
}

pub fn build_session_handoff_packet(
    input: SessionHandoffInput<'_>,
) -> Option<SessionHandoffPacket> {
    let reason = handoff_reason(
        input.recent_admission_repair,
        input.recent_admission_reasons,
    )?;
    let mut current_defaults = collect_current_defaults(input.interpretation);
    dedup_and_truncate(&mut current_defaults, MAX_DEFAULTS);

    let mut anchors = collect_context_anchors(
        input.recalled_entries,
        input.session_matches,
        input.run_recipes,
    );
    dedup_and_truncate(&mut anchors, MAX_ANCHORS);

    let mut unresolved_questions = input
        .interpretation
        .map(|interpretation| interpretation.clarification_candidates.clone())
        .unwrap_or_default();
    dedup_and_truncate(&mut unresolved_questions, MAX_UNRESOLVED);

    let assumptions = runtime_assumptions::build_runtime_assumptions(
        runtime_assumptions::RuntimeAssumptionInput {
            user_message: input.user_message,
            interpretation: input.interpretation,
            recent_admission_repair: input.recent_admission_repair,
            recent_admission_reasons: input.recent_admission_reasons,
        },
    );

    Some(SessionHandoffPacket {
        reason,
        recommended_action: input
            .recent_admission_repair
            .map(admission_repair_hint_label),
        active_task: bounded_non_empty(input.user_message, MAX_TASK_CHARS),
        current_defaults,
        anchors,
        unresolved_questions,
        assumptions,
    })
}

pub fn format_session_handoff_packet(packet: &SessionHandoffPacket) -> String {
    let mut lines = vec![
        "[session-handoff]".to_string(),
        format!("reason: {}", session_handoff_reason_name(packet.reason)),
    ];
    if let Some(action) = packet.recommended_action.as_deref() {
        lines.push(format!("recommended_action: {action}"));
    }
    if let Some(task) = packet.active_task.as_deref() {
        lines.push(format!("active_task: {task}"));
    }
    append_list(&mut lines, "current_defaults", &packet.current_defaults);
    append_list(&mut lines, "anchors", &packet.anchors);
    append_list(
        &mut lines,
        "unresolved_questions",
        &packet.unresolved_questions,
    );
    append_list(
        &mut lines,
        "assumptions",
        &packet
            .assumptions
            .iter()
            .map(runtime_assumptions::format_runtime_assumption)
            .collect::<Vec<_>>(),
    );
    lines.push("[/session-handoff]".to_string());
    lines.push(String::new());
    lines.join("\n")
}

pub fn parse_session_handoff_packet_value(
    value: &serde_json::Value,
) -> anyhow::Result<Option<SessionHandoffPacket>> {
    if value.is_null() {
        return Ok(None);
    }
    if !value.is_object() {
        anyhow::bail!("handoff_packet must be an object when provided");
    }

    let packet = serde_json::from_value::<SessionHandoffPacket>(value.clone())
        .map_err(|error| anyhow::anyhow!("invalid handoff_packet: {error}"))?;
    Ok(Some(bound_session_handoff_packet(packet)))
}

pub fn bound_session_handoff_packet(mut packet: SessionHandoffPacket) -> SessionHandoffPacket {
    packet.recommended_action = packet
        .recommended_action
        .as_deref()
        .and_then(|value| bounded_non_empty(value, MAX_ITEM_CHARS));
    packet.active_task = packet
        .active_task
        .as_deref()
        .and_then(|value| bounded_non_empty(value, MAX_TASK_CHARS));
    dedup_and_truncate(&mut packet.current_defaults, MAX_DEFAULTS);
    dedup_and_truncate(&mut packet.anchors, MAX_ANCHORS);
    dedup_and_truncate(&mut packet.unresolved_questions, MAX_UNRESOLVED);
    packet.assumptions = runtime_assumptions::bound_runtime_assumptions(packet.assumptions);
    packet
}

pub fn session_handoff_reason_name(reason: SessionHandoffReason) -> &'static str {
    match reason {
        SessionHandoffReason::ContextOverflow => "context_overflow",
        SessionHandoffReason::RouteSwitch => "route_switch",
        SessionHandoffReason::Compaction => "compaction",
        SessionHandoffReason::CapabilityRepair => "capability_repair",
        SessionHandoffReason::SessionResume => "session_resume",
    }
}

fn handoff_reason(
    repair: Option<AdmissionRepairHint>,
    reasons: &[CandidateAdmissionReason],
) -> Option<SessionHandoffReason> {
    match repair {
        Some(AdmissionRepairHint::StartFreshHandoff) => Some(SessionHandoffReason::ContextOverflow),
        Some(AdmissionRepairHint::SwitchToLane(_))
        | Some(AdmissionRepairHint::SwitchToToolCapableReasoning) => {
            Some(SessionHandoffReason::RouteSwitch)
        }
        Some(AdmissionRepairHint::CompactSession) => Some(SessionHandoffReason::Compaction),
        Some(AdmissionRepairHint::RefreshCapabilityMetadata(_)) => {
            Some(SessionHandoffReason::CapabilityRepair)
        }
        None if reasons.iter().any(|reason| {
            matches!(
                reason,
                CandidateAdmissionReason::CandidateWindowExceeded
                    | CandidateAdmissionReason::ProviderContextOverflowRisk
            )
        }) =>
        {
            Some(SessionHandoffReason::ContextOverflow)
        }
        None if reasons
            .iter()
            .any(|reason| matches!(reason, CandidateAdmissionReason::CandidateWindowNearLimit)) =>
        {
            Some(SessionHandoffReason::Compaction)
        }
        None => None,
    }
}

fn collect_current_defaults(interpretation: Option<&TurnInterpretation>) -> Vec<String> {
    let mut defaults = Vec::new();
    let Some(interpretation) = interpretation else {
        return defaults;
    };

    if let Some(profile) = interpretation.user_profile.as_ref() {
        for (key, value) in profile.iter().take(6) {
            let value = profile.get_text(key).unwrap_or_else(|| value.to_string());
            defaults.push(format!("{key}={value}"));
        }
        if let Some(target) = profile.get_delivery_target(DELIVERY_TARGET_PREFERENCE_KEY) {
            defaults.push(format!(
                "profile_delivery_target={}",
                format_delivery_target(&target)
            ));
        }
    }
    if let Some(target) = interpretation.configured_delivery_target.as_ref() {
        defaults.push(format!(
            "configured_delivery_target={}",
            format_delivery_target(target)
        ));
    }
    if let Some(conversation) = interpretation.current_conversation.as_ref() {
        defaults.push(format!(
            "current_conversation_adapter={},threaded={}",
            conversation.adapter, conversation.has_thread
        ));
    }
    if let Some(state) = interpretation.dialogue_state.as_ref() {
        if let Some(target) = state.recent_delivery_target.as_ref() {
            defaults.push(format!(
                "recent_delivery_target={}",
                format_delivery_target(target)
            ));
        }
        if let Some(resource) = state.recent_resource.as_ref() {
            defaults.push(format!(
                "recent_resource={},host={}",
                resource.locator,
                resource.host.as_deref().unwrap_or("unknown")
            ));
        }
        if let Some(search) = state.recent_search.as_ref() {
            if let Some(query) = search.query.as_deref() {
                defaults.push(format!("recent_search_query={query}"));
            }
        }
        for candidate in interpretation.reference_candidates.iter().take(2) {
            defaults.push(format_reference_candidate(candidate));
        }
    }

    defaults
}

fn collect_context_anchors(
    recalled_entries: &[MemoryEntry],
    session_matches: &[SessionSearchMatch],
    run_recipes: &[RunRecipeSearchMatch],
) -> Vec<String> {
    let mut anchors = Vec::new();
    anchors.extend(
        recalled_entries
            .iter()
            .take(2)
            .filter_map(|entry| bounded_non_empty(&entry.content, MAX_ITEM_CHARS))
            .map(|content| format!("memory={content}")),
    );
    anchors.extend(
        session_matches
            .iter()
            .take(1)
            .filter_map(|session| {
                session
                    .summary
                    .as_deref()
                    .or(session.recap.as_deref())
                    .and_then(|summary| bounded_non_empty(summary, MAX_ITEM_CHARS))
            })
            .map(|summary| format!("session={summary}")),
    );
    anchors.extend(
        run_recipes
            .iter()
            .take(1)
            .filter_map(|recipe| bounded_non_empty(&recipe.summary, MAX_ITEM_CHARS))
            .map(|summary| format!("recipe={summary}")),
    );
    anchors
}

fn format_delivery_target(target: &ConversationDeliveryTarget) -> String {
    match target {
        ConversationDeliveryTarget::CurrentConversation => "current_conversation".to_string(),
        ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } => match thread_ref.as_deref() {
            Some(thread) => format!("{channel}:{recipient}#{thread}"),
            None => format!("{channel}:{recipient}"),
        },
    }
}

fn format_reference_candidate(candidate: &ReferenceCandidate) -> String {
    format!(
        "reference_candidate={}:{}",
        candidate.source_name(),
        candidate.value
    )
}

trait ReferenceCandidateSourceName {
    fn source_name(&self) -> &'static str;
}

impl ReferenceCandidateSourceName for ReferenceCandidate {
    fn source_name(&self) -> &'static str {
        match self.source {
            crate::application::services::turn_interpretation::ReferenceSource::ConfiguredRuntime => {
                "configured_runtime"
            }
            crate::application::services::turn_interpretation::ReferenceSource::DialogueState => {
                "dialogue_state"
            }
            crate::application::services::turn_interpretation::ReferenceSource::UserProfile => {
                "user_profile"
            }
            crate::application::services::turn_interpretation::ReferenceSource::CurrentConversation => {
                "current_conversation"
            }
        }
    }
}

fn append_list(lines: &mut Vec<String>, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for value in values {
        lines.push(format!("- {value}"));
    }
}

fn dedup_and_truncate(values: &mut Vec<String>, limit: usize) {
    let mut seen = Vec::<String>::new();
    values.retain_mut(|value| {
        if let Some(bounded) = bounded_non_empty(value, MAX_ITEM_CHARS) {
            *value = bounded;
        } else {
            return false;
        }
        if seen.iter().any(|existing| existing == value) {
            false
        } else {
            seen.push(value.clone());
            true
        }
    });
    values.truncate(limit);
}

fn bounded_non_empty(value: &str, max_chars: usize) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else if collapsed.chars().count() <= max_chars {
        Some(collapsed)
    } else {
        Some(format!(
            "{}...",
            collapsed.chars().take(max_chars).collect::<String>()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::turn_interpretation::TurnInterpretation;
    use crate::config::schema::CapabilityLane;
    use crate::domain::memory::{MemoryCategory, MemoryEntry};
    use crate::domain::user_profile::UserProfile;

    #[test]
    fn builds_context_overflow_handoff_packet_with_bounded_fields() {
        let interpretation = TurnInterpretation {
            user_profile: Some({
                let mut profile = UserProfile::default();
                profile.set("workspace_anchor", serde_json::json!("Borealis"));
                profile.set("project_alias", serde_json::json!("Borealis"));
                profile
            }),
            ..Default::default()
        };
        let packet = build_session_handoff_packet(SessionHandoffInput {
            user_message: "Continue the current long analysis after switching to a smaller model.",
            interpretation: Some(&interpretation),
            recent_admission_repair: Some(AdmissionRepairHint::StartFreshHandoff),
            recent_admission_reasons: &[CandidateAdmissionReason::CandidateWindowExceeded],
            recalled_entries: &[MemoryEntry {
                id: "m1".into(),
                key: "m1".into(),
                content: "Early anchor: freedom and responsibility.".into(),
                category: MemoryCategory::Conversation,
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: Some(0.9),
            }],
            session_matches: &[],
            run_recipes: &[],
        })
        .expect("packet");

        assert_eq!(packet.reason, SessionHandoffReason::ContextOverflow);
        assert_eq!(
            packet.recommended_action.as_deref(),
            Some("start_fresh_handoff")
        );
        assert!(packet
            .current_defaults
            .contains(&"workspace_anchor=Borealis".into()));
        assert!(packet
            .anchors
            .iter()
            .any(|anchor| anchor.contains("freedom and responsibility")));
        assert!(packet.assumptions.iter().any(|assumption| assumption.kind
            == runtime_assumptions::RuntimeAssumptionKind::ProfileFact
            && assumption.value == "workspace_anchor=Borealis"));
    }

    #[test]
    fn formats_handoff_packet_for_prompt_or_user_surface() {
        let packet = SessionHandoffPacket {
            reason: SessionHandoffReason::RouteSwitch,
            recommended_action: Some(admission_repair_hint_label(
                AdmissionRepairHint::SwitchToLane(CapabilityLane::ImageGeneration),
            )),
            active_task: Some("Generate an asset with a capable route".into()),
            current_defaults: vec!["project_alias=Borealis".into()],
            anchors: vec!["memory=Use structured markers for media routing".into()],
            unresolved_questions: vec!["which target lane".into()],
            assumptions: vec![runtime_assumptions::RuntimeAssumption {
                kind: runtime_assumptions::RuntimeAssumptionKind::ProfileFact,
                source: runtime_assumptions::RuntimeAssumptionSource::UserProfile,
                freshness: runtime_assumptions::RuntimeAssumptionFreshness::SessionRecent,
                confidence_basis_points: 8_500,
                value: "project_alias=Borealis".into(),
                invalidation: runtime_assumptions::RuntimeAssumptionInvalidation::ProfileUpdate,
                replacement_path:
                    runtime_assumptions::RuntimeAssumptionReplacementPath::UpdateProfile,
            }],
        };

        let formatted = format_session_handoff_packet(&packet);
        assert!(formatted.contains("[session-handoff]"));
        assert!(formatted.contains("reason: route_switch"));
        assert!(formatted.contains("recommended_action: switch_lane:image_generation"));
        assert!(formatted.contains("assumptions:"));
        assert!(formatted.contains("kind=profile_fact"));
        assert!(formatted.contains("value=project_alias=Borealis"));
        assert!(formatted.contains("[/session-handoff]"));
    }

    #[test]
    fn parses_and_bounds_handoff_packet_value() {
        let value = serde_json::json!({
            "reason": "context_overflow",
            "active_task": "x".repeat(400),
            "current_defaults": ["project_alias=Borealis", "project_alias=Borealis"],
            "anchors": ["a", "b", "c", "d", "e"],
            "unresolved_questions": ["q1", "q2", "q3", "q4"]
        });

        let packet = parse_session_handoff_packet_value(&value)
            .unwrap()
            .expect("packet");

        assert_eq!(packet.reason, SessionHandoffReason::ContextOverflow);
        assert!(packet.active_task.unwrap().chars().count() <= MAX_TASK_CHARS + 3);
        assert_eq!(packet.current_defaults, vec!["project_alias=Borealis"]);
        assert_eq!(packet.anchors.len(), MAX_ANCHORS);
        assert_eq!(packet.unresolved_questions.len(), MAX_UNRESOLVED);
    }

    #[test]
    fn rejects_unknown_handoff_packet_fields() {
        let value = serde_json::json!({
            "reason": "compaction",
            "extra": "not part of the shared schema"
        });

        assert!(parse_session_handoff_packet_value(&value).is_err());
    }
}
