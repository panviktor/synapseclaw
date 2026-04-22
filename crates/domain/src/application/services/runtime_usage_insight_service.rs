use crate::application::services::runtime_decision_trace::RuntimeDecisionTrace;
use crate::application::services::runtime_watchdog::RuntimeWatchdogAlert;
use crate::config::schema::{Config, ModelPricing};
use crate::domain::tool_repair::ToolRepairTrace;
use crate::ports::route_selection::RouteSelection;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePricingStatus {
    Known,
    #[default]
    Unknown,
    Included,
    ProviderReported,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeUsageLedger {
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub unknown_input_token_count: u64,
    pub unknown_output_token_count: u64,
    pub unknown_cached_token_count: u64,
    pub compaction_count: u64,
    pub handoff_count: u64,
    pub compaction_cache_hits: u64,
    pub tool_failure_count: u64,
    pub tool_failure_classes: u64,
    pub repaired_tool_count: u64,
    pub watchdog_alert_count: u64,
    pub expensive_test_count: u64,
    pub estimated_cost_microusd: u64,
    pub pricing_known_count: u64,
    pub pricing_unknown_count: u64,
    pub pricing_included_count: u64,
    pub pricing_provider_reported_count: u64,
    pub last_summary_provider: Option<String>,
    pub last_summary_model: Option<String>,
    pub max_pressure_before_basis_points: u32,
    pub max_pressure_after_basis_points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeUsageInsightSnapshot {
    pub provider: String,
    pub model: String,
    pub lane: Option<String>,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub unknown_input_token_count: u64,
    pub unknown_output_token_count: u64,
    pub unknown_cached_token_count: u64,
    pub compaction_count: u64,
    pub handoff_count: u64,
    pub compaction_cache_hits: u64,
    pub max_pressure_before_basis_points: u32,
    pub max_pressure_after_basis_points: u32,
    pub tool_failure_count: u64,
    pub tool_failure_classes: u64,
    pub repaired_tool_count: u64,
    pub watchdog_alert_count: u64,
    pub expensive_test_count: u64,
    pub estimated_cost_microusd: u64,
    pub pricing_status_counts: RuntimePricingStatusCounts,
    pub tool_failure_breakdown: Vec<RuntimeUsageToolFailureBreakdown>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RuntimePricingStatusCounts {
    pub known: u64,
    pub unknown: u64,
    pub included: u64,
    pub provider_reported: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeUsageToolFailureBreakdown {
    pub tool_name: String,
    pub failure_kind: String,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeUsageRecordInput<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub pricing_status: RuntimePricingStatus,
    pub estimated_cost_microusd: u64,
}

pub fn record_runtime_usage(
    mut ledger: RuntimeUsageLedger,
    input: RuntimeUsageRecordInput<'_>,
) -> RuntimeUsageLedger {
    ledger.request_count = ledger.request_count.saturating_add(1);
    if let Some(value) = input.input_tokens {
        ledger.input_tokens = ledger.input_tokens.saturating_add(value);
    } else {
        ledger.unknown_input_token_count = ledger.unknown_input_token_count.saturating_add(1);
    }
    if let Some(value) = input.output_tokens {
        ledger.output_tokens = ledger.output_tokens.saturating_add(value);
    } else {
        ledger.unknown_output_token_count = ledger.unknown_output_token_count.saturating_add(1);
    }
    if let Some(value) = input.cached_input_tokens {
        ledger.cached_input_tokens = ledger.cached_input_tokens.saturating_add(value);
    } else {
        ledger.unknown_cached_token_count = ledger.unknown_cached_token_count.saturating_add(1);
    }
    ledger.estimated_cost_microusd = ledger
        .estimated_cost_microusd
        .saturating_add(input.estimated_cost_microusd);
    match input.pricing_status {
        RuntimePricingStatus::Known => {
            ledger.pricing_known_count = ledger.pricing_known_count.saturating_add(1)
        }
        RuntimePricingStatus::Unknown => {
            ledger.pricing_unknown_count = ledger.pricing_unknown_count.saturating_add(1)
        }
        RuntimePricingStatus::Included => {
            ledger.pricing_included_count = ledger.pricing_included_count.saturating_add(1)
        }
        RuntimePricingStatus::ProviderReported => {
            ledger.pricing_provider_reported_count =
                ledger.pricing_provider_reported_count.saturating_add(1)
        }
    }
    ledger.last_summary_provider = Some(input.provider.to_string());
    ledger.last_summary_model = Some(input.model.to_string());
    ledger
}

pub fn build_runtime_usage_insight_snapshot(
    route: &RouteSelection,
    config: &Config,
) -> RuntimeUsageInsightSnapshot {
    let tool_failure_breakdown = aggregate_tool_failures(&route.recent_tool_repairs);
    RuntimeUsageInsightSnapshot {
        provider: route.provider.clone(),
        model: route.model.clone(),
        lane: route.lane.map(|lane| lane.as_str().to_string()),
        request_count: route.usage_ledger.request_count,
        input_tokens: route.usage_ledger.input_tokens,
        output_tokens: route.usage_ledger.output_tokens,
        cached_input_tokens: route.usage_ledger.cached_input_tokens,
        unknown_input_token_count: route.usage_ledger.unknown_input_token_count,
        unknown_output_token_count: route.usage_ledger.unknown_output_token_count,
        unknown_cached_token_count: route.usage_ledger.unknown_cached_token_count,
        compaction_count: route.usage_ledger.compaction_count,
        handoff_count: route.usage_ledger.handoff_count,
        compaction_cache_hits: route.usage_ledger.compaction_cache_hits,
        max_pressure_before_basis_points: route.usage_ledger.max_pressure_before_basis_points,
        max_pressure_after_basis_points: route.usage_ledger.max_pressure_after_basis_points,
        tool_failure_count: route.usage_ledger.tool_failure_count,
        tool_failure_classes: route.usage_ledger.tool_failure_classes,
        repaired_tool_count: route.usage_ledger.repaired_tool_count,
        watchdog_alert_count: route
            .usage_ledger
            .watchdog_alert_count
            .max(route.watchdog_alerts.len() as u64),
        expensive_test_count: route.usage_ledger.expensive_test_count,
        estimated_cost_microusd: if config.cost.enabled {
            route.usage_ledger.estimated_cost_microusd
        } else {
            0
        },
        pricing_status_counts: RuntimePricingStatusCounts {
            known: route.usage_ledger.pricing_known_count,
            unknown: route.usage_ledger.pricing_unknown_count,
            included: route.usage_ledger.pricing_included_count,
            provider_reported: route.usage_ledger.pricing_provider_reported_count,
        },
        tool_failure_breakdown,
    }
}

pub fn runtime_pricing_status_for_route(
    prices: &std::collections::HashMap<String, ModelPricing>,
    cost_enabled: bool,
    provider: &str,
    model: &str,
) -> (RuntimePricingStatus, Option<ModelPricing>) {
    if !cost_enabled {
        return (RuntimePricingStatus::Included, None);
    }
    let key = model_pricing_lookup_key(provider, model);
    match prices.get(&key).cloned() {
        Some(pricing) => (RuntimePricingStatus::Known, Some(pricing)),
        None => (RuntimePricingStatus::Unknown, None),
    }
}

pub fn estimate_usage_cost_microusd(
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    pricing: Option<&ModelPricing>,
) -> u64 {
    let Some(pricing) = pricing else {
        return 0;
    };
    let input_cost =
        (input_tokens.unwrap_or(0) as f64 / 1_000_000.0) * pricing.input.max(0.0) * 1_000_000.0;
    let output_cost =
        (output_tokens.unwrap_or(0) as f64 / 1_000_000.0) * pricing.output.max(0.0) * 1_000_000.0;
    (input_cost + output_cost).round().max(0.0) as u64
}

pub fn pressure_basis_points(estimated_tokens: usize, ceiling_tokens: usize) -> u32 {
    if ceiling_tokens == 0 {
        return 0;
    }
    ((estimated_tokens as u128)
        .saturating_mul(10_000)
        .checked_div(ceiling_tokens as u128)
        .unwrap_or(0)
        .min(10_000)) as u32
}

pub fn update_usage_ledger_from_trace(
    mut ledger: RuntimeUsageLedger,
    trace: &RuntimeDecisionTrace,
) -> RuntimeUsageLedger {
    let before = pressure_basis_points(
        trace.context.estimated_total_tokens,
        trace.context.ceiling_total_tokens,
    );
    ledger.max_pressure_before_basis_points = ledger.max_pressure_before_basis_points.max(before);
    if trace.context.requires_compaction {
        ledger.compaction_count = ledger.compaction_count.saturating_add(1);
    }
    if let Some(cache) = trace.context.cache {
        ledger.compaction_cache_hits = ledger.compaction_cache_hits.saturating_add(cache.hits);
    }
    let after = trace
        .notes
        .iter()
        .rev()
        .find_map(parse_post_compaction_pressure_note)
        .unwrap_or(before);
    ledger.max_pressure_after_basis_points = ledger.max_pressure_after_basis_points.max(after);
    ledger
}

pub fn update_usage_ledger_from_handoff_artifacts(
    mut ledger: RuntimeUsageLedger,
    handoff_count: usize,
) -> RuntimeUsageLedger {
    ledger.handoff_count = ledger.handoff_count.max(handoff_count as u64);
    ledger
}

pub fn update_usage_ledger_from_tool_repairs(
    mut ledger: RuntimeUsageLedger,
    repairs: &[ToolRepairTrace],
) -> RuntimeUsageLedger {
    if repairs.is_empty() {
        return ledger;
    }
    ledger.tool_failure_count = repairs.len() as u64;
    ledger.tool_failure_classes = aggregate_tool_failures(repairs).len() as u64;
    ledger.repaired_tool_count = repairs
        .iter()
        .filter(|repair| {
            repair.repair_outcome != crate::domain::tool_repair::ToolRepairOutcome::Failed
        })
        .count() as u64;
    ledger
}

pub fn update_usage_ledger_from_watchdog_alerts(
    mut ledger: RuntimeUsageLedger,
    alerts: &[RuntimeWatchdogAlert],
) -> RuntimeUsageLedger {
    ledger.watchdog_alert_count = alerts.len() as u64;
    ledger
}

fn aggregate_tool_failures(
    repairs: &[ToolRepairTrace],
) -> Vec<RuntimeUsageToolFailureBreakdown> {
    let mut counts = BTreeMap::<(String, String), u64>::new();
    for repair in repairs {
        let key = (
            repair.tool_name.clone(),
            crate::domain::tool_repair::tool_failure_kind_name(repair.failure_kind).to_string(),
        );
        *counts.entry(key).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((tool_name, failure_kind), count)| RuntimeUsageToolFailureBreakdown {
            tool_name,
            failure_kind,
            count,
        })
        .collect()
}

fn model_pricing_lookup_key(provider: &str, model: &str) -> String {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() {
        model.to_string()
    } else if model.starts_with(provider) || model.contains('/') {
        model.to_string()
    } else {
        format!("{provider}/{model}")
    }
}

fn parse_post_compaction_pressure_note(
    note: &crate::application::services::runtime_decision_trace::RuntimeTraceNote,
) -> Option<u32> {
    if note.kind != "post_compaction_pressure" {
        return None;
    }
    note.detail
        .split("basis_points=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u32>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::runtime_decision_trace::RuntimeTraceNote;
    use crate::config::schema::Config;
    use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction, ToolRepairOutcome, ToolRepairTrace};
    use crate::ports::route_selection::RouteSelection;

    #[test]
    fn cost_microusd_uses_model_pricing() {
        let pricing = ModelPricing {
            input: 2.5,
            output: 10.0,
        };
        let cost = estimate_usage_cost_microusd(Some(2_000), Some(500), Some(&pricing));
        assert!(cost > 0);
    }

    #[test]
    fn post_compaction_pressure_note_is_parsed() {
        let note = RuntimeTraceNote {
            observed_at_unix: 1,
            kind: "post_compaction_pressure".into(),
            detail: "basis_points=4321 estimated_tokens=1200 ceiling_tokens=2777".into(),
        };
        assert_eq!(parse_post_compaction_pressure_note(&note), Some(4321));
    }

    #[test]
    fn snapshot_preserves_unknown_pricing_and_failure_breakdown() {
        let mut route = RouteSelection {
            provider: "openrouter".into(),
            model: "unknown-model".into(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: vec![],
            last_tool_repair: None,
            recent_tool_repairs: vec![],
            context_cache: None,
            assumptions: vec![],
            calibrations: vec![],
            watchdog_alerts: vec![],
            handoff_artifacts: vec![],
            runtime_decision_traces: vec![],
            usage_ledger: RuntimeUsageLedger::default(),
        };
        route.usage_ledger.pricing_unknown_count = 2;
        route.recent_tool_repairs = vec![
            ToolRepairTrace {
                tool_name: "weather".into(),
                failure_kind: ToolFailureKind::ReportedFailure,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                repair_outcome: ToolRepairOutcome::Resolved,
                ..ToolRepairTrace::default()
            },
            ToolRepairTrace {
                tool_name: "weather".into(),
                failure_kind: ToolFailureKind::ReportedFailure,
                suggested_action: ToolRepairAction::AdjustArgumentsOrTarget,
                repair_outcome: ToolRepairOutcome::Resolved,
                ..ToolRepairTrace::default()
            },
        ];

        let snapshot = build_runtime_usage_insight_snapshot(&route, &Config::default());
        assert_eq!(snapshot.pricing_status_counts.unknown, 2);
        assert_eq!(snapshot.tool_failure_breakdown.len(), 1);
        assert_eq!(snapshot.tool_failure_breakdown[0].tool_name, "weather");
        assert_eq!(snapshot.tool_failure_breakdown[0].count, 2);
    }
}
