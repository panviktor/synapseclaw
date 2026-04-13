use crate::application::services::session_handoff::{
    format_session_handoff_packet, SessionHandoffPacket,
};
use crate::application::services::turn_admission::CandidateAdmissionDecision;
use crate::domain::turn_admission::{
    turn_intent_name, AdmissionRepairHint, CandidateAdmissionReason, ContextPressureState,
};

pub fn format_blocked_turn_admission_response(
    decision: &CandidateAdmissionDecision,
    handoff_packet: Option<&SessionHandoffPacket>,
) -> String {
    let base = if let Some(AdmissionRepairHint::RefreshCapabilityMetadata(lane)) =
        decision.recommended_action
    {
        format!(
            "Lane `{}` capability metadata is stale or low-confidence on the current route. Refresh model profiles or switch to a compatible lane and try again.",
            lane.as_str()
        )
    } else if decision.snapshot.pressure_state == ContextPressureState::OverflowRisk {
        match decision.recommended_action {
            Some(AdmissionRepairHint::StartFreshHandoff) => {
                "This turn is too large for the current route's safe context budget. Start a fresh handoff or switch to a larger-context model.".into()
            }
            _ => {
                "This turn is too large for the current route's safe context budget. Compact the session first or switch to a larger-context model.".into()
            }
        }
    } else if decision.reasons.iter().any(is_stale_or_unknown_metadata) {
        "The current route has incomplete capability metadata for this turn. Refresh model profiles or switch to a compatible lane and try again.".into()
    } else if let Some(lane) = decision.required_lane {
        format!(
            "Turn intent `{}` requires lane `{}`, but the current route cannot satisfy it. Switch to a compatible lane and try again.",
            turn_intent_name(decision.snapshot.intent),
            lane.as_str()
        )
    } else {
        "The current route cannot safely execute this turn. Switch to a compatible lane or start a fresh handoff.".into()
    };

    if let Some(packet) = handoff_packet {
        format!("{base}\n\n{}", format_session_handoff_packet(packet))
    } else {
        base
    }
}

fn is_stale_or_unknown_metadata(reason: &CandidateAdmissionReason) -> bool {
    matches!(
        reason,
        CandidateAdmissionReason::CapabilityMetadataUnknown(_)
            | CandidateAdmissionReason::CapabilityMetadataLowConfidence(_)
            | CandidateAdmissionReason::CapabilityMetadataStale(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::CapabilityLane;
    use crate::domain::turn_admission::{
        AdmissionRepairHint, CandidateAdmissionReason, ContextPressureState, TurnAdmissionAction,
        TurnAdmissionSnapshot, TurnIntentCategory,
    };

    fn blocked_decision(
        intent: TurnIntentCategory,
        pressure_state: ContextPressureState,
    ) -> CandidateAdmissionDecision {
        CandidateAdmissionDecision {
            snapshot: TurnAdmissionSnapshot {
                intent,
                pressure_state,
                action: TurnAdmissionAction::Block,
            },
            required_lane: None,
            route_override: None,
            reasons: Vec::new(),
            recommended_action: None,
            condensation_plan: None,
            requires_compaction: false,
        }
    }

    #[test]
    fn blocked_turn_response_mentions_stale_or_low_confidence_metadata() {
        let mut decision = blocked_decision(
            TurnIntentCategory::ImageGeneration,
            ContextPressureState::Warning,
        );
        decision.required_lane = Some(CapabilityLane::ImageGeneration);
        decision.reasons = vec![CandidateAdmissionReason::CapabilityMetadataLowConfidence(
            CapabilityLane::ImageGeneration,
        )];
        decision.recommended_action = Some(AdmissionRepairHint::RefreshCapabilityMetadata(
            CapabilityLane::ImageGeneration,
        ));

        let response = format_blocked_turn_admission_response(&decision, None);

        assert!(response.starts_with("Lane `image_generation`"));
        assert!(response.contains("stale or low-confidence"));
        assert!(response.contains("Refresh model profiles"));
    }

    #[test]
    fn blocked_turn_response_mentions_required_lane_for_modality() {
        let mut decision = blocked_decision(
            TurnIntentCategory::MusicGeneration,
            ContextPressureState::Warning,
        );
        decision.required_lane = Some(CapabilityLane::MusicGeneration);
        decision.reasons = vec![CandidateAdmissionReason::RequiresLane(
            CapabilityLane::MusicGeneration,
        )];

        let response = format_blocked_turn_admission_response(&decision, None);

        assert!(
            response.starts_with("Turn intent `music_generation` requires lane `music_generation`")
        );
        assert!(response.contains("current route cannot satisfy it"));
    }

    #[test]
    fn blocked_turn_response_mentions_safe_context_budget_on_overflow_risk() {
        let mut decision = blocked_decision(
            TurnIntentCategory::Reply,
            ContextPressureState::OverflowRisk,
        );
        decision.reasons = vec![CandidateAdmissionReason::CandidateWindowExceeded];
        decision.recommended_action = Some(AdmissionRepairHint::StartFreshHandoff);

        let response = format_blocked_turn_admission_response(&decision, None);

        assert!(response.contains("safe context budget"));
        assert!(response.contains("fresh handoff"));
    }
}
