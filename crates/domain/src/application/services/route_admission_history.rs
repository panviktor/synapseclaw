use crate::domain::turn_admission::{AdmissionRepairHint, CandidateAdmissionReason};
use crate::ports::route_selection::RouteAdmissionState;

pub const ROUTE_ADMISSION_TRACE_TTL_SECS: i64 = 48 * 60 * 60;
pub const MAX_ROUTE_ADMISSION_HISTORY: usize = 4;

pub fn append_route_admission_state(
    history: &[RouteAdmissionState],
    next: Option<RouteAdmissionState>,
    now_unix: i64,
) -> Vec<RouteAdmissionState> {
    let mut bounded = history
        .iter()
        .filter(|state| state.observed_at_unix >= now_unix - ROUTE_ADMISSION_TRACE_TTL_SECS)
        .cloned()
        .collect::<Vec<_>>();

    if let Some(next) = next {
        if let Some(last) = bounded.last_mut() {
            if same_admission_signature(last, &next) {
                last.observed_at_unix = next.observed_at_unix;
            } else {
                bounded.push(next);
            }
        } else {
            bounded.push(next);
        }
    }

    if bounded.len() > MAX_ROUTE_ADMISSION_HISTORY {
        let overflow = bounded.len() - MAX_ROUTE_ADMISSION_HISTORY;
        bounded.drain(0..overflow);
    }

    bounded
}

pub fn recent_route_admission_reasons(
    history: &[RouteAdmissionState],
) -> Vec<CandidateAdmissionReason> {
    let mut reasons = Vec::new();
    for state in history.iter().rev() {
        for reason in &state.reasons {
            if !reasons.contains(reason) {
                reasons.push(reason.clone());
            }
            if reasons.len() >= MAX_ROUTE_ADMISSION_HISTORY {
                return reasons;
            }
        }
    }
    reasons
}

pub fn latest_route_admission_repair(
    history: &[RouteAdmissionState],
) -> Option<AdmissionRepairHint> {
    history
        .iter()
        .rev()
        .find_map(|state| state.recommended_action)
}

fn same_admission_signature(left: &RouteAdmissionState, right: &RouteAdmissionState) -> bool {
    left.snapshot == right.snapshot
        && left.reasons == right.reasons
        && left.recommended_action == right.recommended_action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::turn_admission::{
        AdmissionRepairHint, CandidateAdmissionReason, ContextPressureState, TurnAdmissionAction,
        TurnAdmissionSnapshot, TurnIntentCategory,
    };

    fn state(observed_at_unix: i64, reason: CandidateAdmissionReason) -> RouteAdmissionState {
        RouteAdmissionState {
            observed_at_unix,
            snapshot: TurnAdmissionSnapshot {
                intent: TurnIntentCategory::Deliver,
                pressure_state: ContextPressureState::Warning,
                action: TurnAdmissionAction::Reroute,
            },
            reasons: vec![reason],
            recommended_action: Some(AdmissionRepairHint::CompactSession),
        }
    }

    #[test]
    fn deduplicates_adjacent_matching_admissions() {
        let history = vec![state(100, CandidateAdmissionReason::ProviderContextWarning)];
        let updated = append_route_admission_state(
            &history,
            Some(state(200, CandidateAdmissionReason::ProviderContextWarning)),
            200,
        );

        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].observed_at_unix, 200);
    }

    #[test]
    fn drops_expired_admissions_and_caps_history() {
        let mut history = Vec::new();
        for (idx, reason) in [
            CandidateAdmissionReason::ProviderContextWarning,
            CandidateAdmissionReason::ProviderContextCritical,
            CandidateAdmissionReason::CandidateWindowNearLimit,
            CandidateAdmissionReason::CandidateWindowExceeded,
            CandidateAdmissionReason::ProviderContextOverflowRisk,
        ]
        .into_iter()
        .enumerate()
        {
            history.push(state(100 + idx as i64, reason));
        }

        let updated =
            append_route_admission_state(&history, None, 104 + ROUTE_ADMISSION_TRACE_TTL_SECS + 1);
        assert!(updated.is_empty());

        let capped = append_route_admission_state(
            &history,
            Some(state(500, CandidateAdmissionReason::ProviderContextWarning)),
            500,
        );
        assert_eq!(capped.len(), MAX_ROUTE_ADMISSION_HISTORY);
        assert_eq!(capped.last().expect("last").observed_at_unix, 500);
    }

    #[test]
    fn exposes_recent_distinct_reasons_and_latest_repair_for_runtime_context() {
        let history = vec![
            state(100, CandidateAdmissionReason::ProviderContextWarning),
            state(200, CandidateAdmissionReason::ProviderContextCritical),
            state(300, CandidateAdmissionReason::ProviderContextWarning),
        ];

        let reasons = recent_route_admission_reasons(&history);

        assert_eq!(
            reasons,
            vec![
                CandidateAdmissionReason::ProviderContextWarning,
                CandidateAdmissionReason::ProviderContextCritical
            ]
        );
        assert_eq!(
            latest_route_admission_repair(&history),
            Some(AdmissionRepairHint::CompactSession)
        );
    }
}
