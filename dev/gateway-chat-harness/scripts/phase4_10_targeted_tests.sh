#!/usr/bin/env bash
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}"

cd "$ROOT"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

# Slices 1, 4, 6, 11, 13, 17: context snapshots, admission, pressure,
# compaction, condensation, handoff.
run cargo test -q -p synapse_domain provider_context_budget --lib
run cargo test -q -p synapse_domain route_switch_preflight --lib
run cargo test -q -p synapse_domain turn_admission --lib
run cargo test -q -p synapse_domain summary_route_resolution --lib
run cargo test -q -p synapse_domain session_handoff --lib

# Slices 2, 3, 5, 7, 14: typed defaults, read-only recall, scoped context,
# deterministic tool exposure, and modality markers.
run cargo test -q -p synapse_domain execution_guidance --lib
run cargo test -q -p synapse_domain turn_tool_narrowing --lib
run cargo test -q -p synapse_domain turn_context --lib
run cargo test -q -p synapse_domain scoped_instruction_resolution --lib
run cargo test -q -p synapse_domain turn_markup --lib
run cargo test -q -p synapse_domain turn_model_routing --lib

# Slices 10, 12, 18: capability lanes, model profiles, endpoint-aware metadata,
# provider options, and provider-layer marker parsing.
run cargo test -q -p synapse_domain model_lane_resolution --lib
run cargo test -q -p synapse_domain model_capability_support --lib
run cargo test -q -p synapse_providers provider_runtime_options --lib
run cargo test -q -p synapse_providers azure_provider --lib
run cargo test -q -p synapse_providers parse_image_marker_parts --lib
run cargo test -q -p synapse_tools model_routing_config --lib

# Slice 16: memory quality, embedding profile calibration, and self-learning
# gates.
run cargo test -q -p synapse_domain memory_quality_governor --lib
run cargo test -q -p synapse_domain post_turn_orchestrator --lib
run cargo test -q -p synapse_domain self_learning_eval_harness --lib
run cargo test -q -p synapse_memory embeddings --lib

# Slices 15, 19, 20, 21, 22, 23: repair traces, assumptions, epistemic state,
# watchdog, calibration, and janitor.
run cargo test -q -p synapse_domain route_admission_history --lib
run cargo test -q -p synapse_domain runtime_assumptions --lib
run cargo test -q -p synapse_domain epistemic_state --lib
run cargo test -q -p synapse_domain runtime_watchdog --lib
run cargo test -q -p synapse_domain runtime_calibration --lib
run cargo test -q -p synapse_domain runtime_trace_janitor --lib

# Slices 9, 24, 25, 26: native tool protocol and web/channel extraction/parity.
run cargo test -q -p synapse_adapters dispatcher --lib --features channel-matrix
run cargo test -q -p synapse_adapters runtime_adapter_contract --lib --features channel-matrix
run cargo test -q -p synapse_adapters runtime_routes --lib --features channel-matrix
run cargo test -q -p synapse_adapters context_engine --lib --features channel-matrix
run cargo test -q -p synapse_channels session_backend --lib --features channel-matrix

printf '\nPhase 4.10 targeted tests completed.\n'
