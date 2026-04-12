//! Typed calibration ledger for runtime decisions.

pub const RUNTIME_CALIBRATION_TTL_SECS: i64 = 48 * 60 * 60;
pub const MAX_RUNTIME_CALIBRATION_RECORDS: usize = 12;
const HIGH_CONFIDENCE_BASIS_POINTS: u16 = 7_500;
const LOW_CONFIDENCE_BASIS_POINTS: u16 = 4_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCalibrationDecisionKind {
    RouteChoice,
    ToolChoice,
    RetrievalChoice,
    DeliveryChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCalibrationOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCalibrationComparison {
    MatchedExpectation,
    OverconfidentFailure,
    UnderconfidentSuccess,
    ExpectedLowConfidenceFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCalibrationAction {
    NoAction,
    SuppressChoice,
    KeepAsPositiveEvidence,
    InspectOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeCalibrationSuppressionKey {
    Route { provider: String, model: String },
    Tool { tool_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCalibrationObservation {
    pub decision_kind: RuntimeCalibrationDecisionKind,
    pub decision_signature: String,
    pub suppression_key: Option<RuntimeCalibrationSuppressionKey>,
    pub confidence_basis_points: u16,
    pub outcome: RuntimeCalibrationOutcome,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeCalibrationRecord {
    pub decision_kind: RuntimeCalibrationDecisionKind,
    pub decision_signature: String,
    pub suppression_key: Option<RuntimeCalibrationSuppressionKey>,
    pub confidence_basis_points: u16,
    pub outcome: RuntimeCalibrationOutcome,
    pub comparison: RuntimeCalibrationComparison,
    pub recommended_action: RuntimeCalibrationAction,
    pub observed_at_unix: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeCalibrationLedger {
    pub records: Vec<RuntimeCalibrationRecord>,
}

pub fn append_runtime_calibration_observation(
    history: &[RuntimeCalibrationRecord],
    observation: RuntimeCalibrationObservation,
    now_unix: i64,
) -> RuntimeCalibrationLedger {
    let mut records = clean_runtime_calibration_records(history, now_unix);
    let Some(record) = build_runtime_calibration_record(observation) else {
        return RuntimeCalibrationLedger { records };
    };

    if let Some(existing) = records.iter_mut().find(|existing| {
        existing.decision_kind == record.decision_kind
            && existing.decision_signature == record.decision_signature
            && existing.comparison == record.comparison
    }) {
        *existing = record;
    } else {
        records.push(record);
    }

    records.sort_by(|left, right| {
        right
            .observed_at_unix
            .cmp(&left.observed_at_unix)
            .then_with(|| left.decision_kind.cmp(&right.decision_kind))
            .then_with(|| left.decision_signature.cmp(&right.decision_signature))
    });
    records.truncate(MAX_RUNTIME_CALIBRATION_RECORDS);

    RuntimeCalibrationLedger { records }
}

pub fn clean_runtime_calibration_records(
    history: &[RuntimeCalibrationRecord],
    now_unix: i64,
) -> Vec<RuntimeCalibrationRecord> {
    let mut by_signature = std::collections::BTreeMap::<
        (
            RuntimeCalibrationDecisionKind,
            String,
            RuntimeCalibrationComparison,
        ),
        RuntimeCalibrationRecord,
    >::new();
    for record in retained_runtime_calibration_records(history, now_unix) {
        let signature = (
            record.decision_kind,
            record.decision_signature.clone(),
            record.comparison,
        );
        match by_signature.get_mut(&signature) {
            Some(existing) if existing.observed_at_unix < record.observed_at_unix => {
                *existing = record;
            }
            None => {
                by_signature.insert(signature, record);
            }
            Some(_) => {}
        }
    }

    let mut records = by_signature.into_values().collect::<Vec<_>>();
    records.sort_by(|left, right| {
        right
            .observed_at_unix
            .cmp(&left.observed_at_unix)
            .then_with(|| left.decision_kind.cmp(&right.decision_kind))
            .then_with(|| left.decision_signature.cmp(&right.decision_signature))
    });
    records.truncate(MAX_RUNTIME_CALIBRATION_RECORDS);
    records
}

pub fn build_runtime_calibration_record(
    observation: RuntimeCalibrationObservation,
) -> Option<RuntimeCalibrationRecord> {
    let signature = bounded_signature(&observation.decision_signature)?;
    let confidence = observation.confidence_basis_points.min(10_000);
    let comparison = compare_runtime_calibration(confidence, observation.outcome);
    Some(RuntimeCalibrationRecord {
        decision_kind: observation.decision_kind,
        decision_signature: signature,
        suppression_key: observation.suppression_key,
        confidence_basis_points: confidence,
        outcome: observation.outcome,
        comparison,
        recommended_action: action_for_runtime_calibration(comparison),
        observed_at_unix: observation.observed_at_unix,
    })
}

pub fn compare_runtime_calibration(
    confidence_basis_points: u16,
    outcome: RuntimeCalibrationOutcome,
) -> RuntimeCalibrationComparison {
    match (confidence_basis_points, outcome) {
        (confidence, RuntimeCalibrationOutcome::Failed)
            if confidence >= HIGH_CONFIDENCE_BASIS_POINTS =>
        {
            RuntimeCalibrationComparison::OverconfidentFailure
        }
        (confidence, RuntimeCalibrationOutcome::Succeeded)
            if confidence <= LOW_CONFIDENCE_BASIS_POINTS =>
        {
            RuntimeCalibrationComparison::UnderconfidentSuccess
        }
        (confidence, RuntimeCalibrationOutcome::Failed)
            if confidence <= LOW_CONFIDENCE_BASIS_POINTS =>
        {
            RuntimeCalibrationComparison::ExpectedLowConfidenceFailure
        }
        _ => RuntimeCalibrationComparison::MatchedExpectation,
    }
}

pub fn action_for_runtime_calibration(
    comparison: RuntimeCalibrationComparison,
) -> RuntimeCalibrationAction {
    match comparison {
        RuntimeCalibrationComparison::MatchedExpectation => RuntimeCalibrationAction::NoAction,
        RuntimeCalibrationComparison::OverconfidentFailure => {
            RuntimeCalibrationAction::SuppressChoice
        }
        RuntimeCalibrationComparison::UnderconfidentSuccess => {
            RuntimeCalibrationAction::KeepAsPositiveEvidence
        }
        RuntimeCalibrationComparison::ExpectedLowConfidenceFailure => {
            RuntimeCalibrationAction::InspectOutcome
        }
    }
}

pub fn runtime_calibration_decision_kind_name(
    kind: RuntimeCalibrationDecisionKind,
) -> &'static str {
    match kind {
        RuntimeCalibrationDecisionKind::RouteChoice => "route_choice",
        RuntimeCalibrationDecisionKind::ToolChoice => "tool_choice",
        RuntimeCalibrationDecisionKind::RetrievalChoice => "retrieval_choice",
        RuntimeCalibrationDecisionKind::DeliveryChoice => "delivery_choice",
    }
}

pub fn runtime_calibration_outcome_name(outcome: RuntimeCalibrationOutcome) -> &'static str {
    match outcome {
        RuntimeCalibrationOutcome::Succeeded => "succeeded",
        RuntimeCalibrationOutcome::Failed => "failed",
    }
}

pub fn runtime_calibration_comparison_name(
    comparison: RuntimeCalibrationComparison,
) -> &'static str {
    match comparison {
        RuntimeCalibrationComparison::MatchedExpectation => "matched_expectation",
        RuntimeCalibrationComparison::OverconfidentFailure => "overconfident_failure",
        RuntimeCalibrationComparison::UnderconfidentSuccess => "underconfident_success",
        RuntimeCalibrationComparison::ExpectedLowConfidenceFailure => {
            "expected_low_confidence_failure"
        }
    }
}

pub fn runtime_calibration_action_name(action: RuntimeCalibrationAction) -> &'static str {
    match action {
        RuntimeCalibrationAction::NoAction => "no_action",
        RuntimeCalibrationAction::SuppressChoice => "suppress_choice",
        RuntimeCalibrationAction::KeepAsPositiveEvidence => "keep_as_positive_evidence",
        RuntimeCalibrationAction::InspectOutcome => "inspect_outcome",
    }
}

pub fn should_suppress_runtime_choice(
    records: &[RuntimeCalibrationRecord],
    suppression_key: &RuntimeCalibrationSuppressionKey,
) -> bool {
    records.iter().any(|record| {
        record.recommended_action == RuntimeCalibrationAction::SuppressChoice
            && record.suppression_key.as_ref() == Some(suppression_key)
    })
}

pub fn should_suppress_route_choice(
    records: &[RuntimeCalibrationRecord],
    provider: &str,
    model: &str,
) -> bool {
    should_suppress_runtime_choice(
        records,
        &RuntimeCalibrationSuppressionKey::Route {
            provider: provider.to_string(),
            model: model.to_string(),
        },
    )
}

pub fn should_suppress_tool_choice(records: &[RuntimeCalibrationRecord], tool_name: &str) -> bool {
    should_suppress_runtime_choice(
        records,
        &RuntimeCalibrationSuppressionKey::Tool {
            tool_name: tool_name.to_string(),
        },
    )
}

fn retained_runtime_calibration_records(
    history: &[RuntimeCalibrationRecord],
    now_unix: i64,
) -> Vec<RuntimeCalibrationRecord> {
    history
        .iter()
        .filter(|record| {
            record.observed_at_unix >= now_unix.saturating_sub(RUNTIME_CALIBRATION_TTL_SECS)
        })
        .cloned()
        .collect()
}

fn bounded_signature(value: &str) -> Option<String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        decision_kind: RuntimeCalibrationDecisionKind,
        signature: &str,
        confidence_basis_points: u16,
        outcome: RuntimeCalibrationOutcome,
        observed_at_unix: i64,
    ) -> RuntimeCalibrationObservation {
        RuntimeCalibrationObservation {
            decision_kind,
            decision_signature: signature.into(),
            suppression_key: None,
            confidence_basis_points,
            outcome,
            observed_at_unix,
        }
    }

    #[test]
    fn high_confidence_failure_becomes_suppression_signal() {
        let record = build_runtime_calibration_record(observation(
            RuntimeCalibrationDecisionKind::RouteChoice,
            "route:cheap_lane",
            9_000,
            RuntimeCalibrationOutcome::Failed,
            100,
        ))
        .unwrap();

        assert_eq!(
            record.comparison,
            RuntimeCalibrationComparison::OverconfidentFailure
        );
        assert_eq!(
            record.recommended_action,
            RuntimeCalibrationAction::SuppressChoice
        );
    }

    #[test]
    fn suppression_uses_typed_key_not_signature_parsing() {
        let record = build_runtime_calibration_record(RuntimeCalibrationObservation {
            decision_kind: RuntimeCalibrationDecisionKind::ToolChoice,
            decision_signature: "opaque-display-only".into(),
            suppression_key: Some(RuntimeCalibrationSuppressionKey::Tool {
                tool_name: "message_send".into(),
            }),
            confidence_basis_points: 9_000,
            outcome: RuntimeCalibrationOutcome::Failed,
            observed_at_unix: 100,
        })
        .unwrap();

        assert!(should_suppress_tool_choice(&[record], "message_send"));
    }

    #[test]
    fn low_confidence_success_becomes_positive_evidence() {
        let record = build_runtime_calibration_record(observation(
            RuntimeCalibrationDecisionKind::ToolChoice,
            "message_send",
            3_000,
            RuntimeCalibrationOutcome::Succeeded,
            100,
        ))
        .unwrap();

        assert_eq!(
            record.comparison,
            RuntimeCalibrationComparison::UnderconfidentSuccess
        );
        assert_eq!(
            record.recommended_action,
            RuntimeCalibrationAction::KeepAsPositiveEvidence
        );
    }

    #[test]
    fn ledger_dedupes_expires_and_caps_records() {
        let old = build_runtime_calibration_record(observation(
            RuntimeCalibrationDecisionKind::DeliveryChoice,
            "old",
            9_000,
            RuntimeCalibrationOutcome::Failed,
            1,
        ))
        .unwrap();
        let ledger = append_runtime_calibration_observation(
            &[old],
            observation(
                RuntimeCalibrationDecisionKind::DeliveryChoice,
                "delivery:matrix",
                8_000,
                RuntimeCalibrationOutcome::Failed,
                200,
            ),
            200 + RUNTIME_CALIBRATION_TTL_SECS + 1,
        );

        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].decision_signature, "delivery:matrix");

        let updated = append_runtime_calibration_observation(
            &ledger.records,
            observation(
                RuntimeCalibrationDecisionKind::DeliveryChoice,
                "delivery:matrix",
                8_500,
                RuntimeCalibrationOutcome::Failed,
                300,
            ),
            300,
        );

        assert_eq!(updated.records.len(), 1);
        assert_eq!(updated.records[0].confidence_basis_points, 8_500);

        let mut records = updated.records;
        for index in 0..MAX_RUNTIME_CALIBRATION_RECORDS + 2 {
            records = append_runtime_calibration_observation(
                &records,
                observation(
                    RuntimeCalibrationDecisionKind::RetrievalChoice,
                    &format!("retrieval:{index}"),
                    5_000,
                    RuntimeCalibrationOutcome::Succeeded,
                    400 + index as i64,
                ),
                500,
            )
            .records;
        }
        assert_eq!(records.len(), MAX_RUNTIME_CALIBRATION_RECORDS);
    }

    #[test]
    fn clean_records_applies_ttl_and_count_bounds() {
        let mut records = Vec::new();
        records.push(
            build_runtime_calibration_record(observation(
                RuntimeCalibrationDecisionKind::RouteChoice,
                "old",
                9_000,
                RuntimeCalibrationOutcome::Failed,
                1,
            ))
            .unwrap(),
        );
        records.push(
            build_runtime_calibration_record(observation(
                RuntimeCalibrationDecisionKind::ToolChoice,
                "tool:dupe",
                4_000,
                RuntimeCalibrationOutcome::Succeeded,
                600,
            ))
            .unwrap(),
        );
        records.push(
            build_runtime_calibration_record(observation(
                RuntimeCalibrationDecisionKind::ToolChoice,
                "tool:dupe",
                4_500,
                RuntimeCalibrationOutcome::Succeeded,
                700,
            ))
            .unwrap(),
        );
        for index in 0..MAX_RUNTIME_CALIBRATION_RECORDS + 2 {
            records.push(
                build_runtime_calibration_record(observation(
                    RuntimeCalibrationDecisionKind::ToolChoice,
                    &format!("tool:{index}"),
                    4_000,
                    RuntimeCalibrationOutcome::Succeeded,
                    500 + index as i64,
                ))
                .unwrap(),
            );
        }

        let cleaned =
            clean_runtime_calibration_records(&records, 500 + RUNTIME_CALIBRATION_TTL_SECS + 1);

        assert_eq!(cleaned.len(), MAX_RUNTIME_CALIBRATION_RECORDS);
        assert!(cleaned
            .iter()
            .all(|record| record.decision_signature != "old"));
        assert_eq!(
            cleaned
                .iter()
                .filter(|record| record.decision_signature == "tool:dupe")
                .count(),
            1
        );
        assert!(cleaned.iter().any(|record| {
            record.decision_signature == "tool:dupe" && record.confidence_basis_points == 4_500
        }));
    }
}
