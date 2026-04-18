# Phase 4.10 Slice Status Audit

Date: 2026-04-14

Scope: compare `docs/fork` Phase 4.10 claims with code and local verification. This is a slice status note, not an implementation plan.

## Verification Snapshot

- `cargo check -q -p synapse_domain -p synapse_adapters -p synapseclaw --features channel-matrix` passes with existing warnings.
- `bash dev/gateway-chat-harness/scripts/phase4_10_targeted_tests.sh` is not green: it stops at `cargo test -q -p synapse_providers provider_runtime_options --lib` because provider tests still initialize `ResponseMessage` / `NativeResponseMessage` with a stale `media_artifacts` field.
- The targeted script did pass the earlier domain checks for context budget, route-switch preflight, admission, summary route resolution, handoff, execution guidance, tool narrowing, turn context, scoped instruction resolution, turn markup, turn model routing, lane resolution, and capability support.
- Additional tail checks passed individually for `memory_quality_governor`, `post_turn_orchestrator`, `route_admission_history`, `tool_repair`, `runtime_assumptions`, `epistemic_state`, `runtime_watchdog`, `runtime_calibration`, and `runtime_trace_janitor`.
- `synapse_adapters` lib tests are also not green: adapter-core filters fail before running because channel/gateway test fixtures are stale around `PresentedOutput` and the new `runtime_mcp_activated_tools` `AppState` field.
- Latest retained live reports show the base pack and OpenRouter image-generation smoke passing with warnings. The retained heavy report passes semantic retention and overflow-switch blocking, but still records `FAIL compaction_signal`; the current script logic has since been changed to allow healthy large-window heavy runs without compaction, so this needs a fresh rerun if the report is used as closeout evidence.

## Slice Status

### Slice 1: Context Snapshot

Code-backed and effectively implemented. `provider_context_budget` defines the budget snapshot/tiers and live reports contain provider-context TSV rows, so this is not just a document claim; the remaining issue is tuning warning-level payload size, not missing machinery.

### Slice 2: Typed Defaults

Code-backed and implemented for the current delivery/default target path. `turn_defaults_context`, dynamic `user_profile` facts, and `message_send` target resolution exist, and the relevant domain checks passed; this is not docs-only.

### Slice 3: Non-Mutating Structured Recall

Code-backed and recently validated better than the stale blocker text in the test matrix suggests. The newer base live report shows cheap, DeepSeek, DeepSeek-reasoner, and GPT-5.4 recall turns did not emit `core_memory_update`, and the domain tool-narrowing/turn-context tests passed.

### Slice 4: Live History Compaction

Code-backed but not fully closeout-proven by the retained heavy report. The context engine and history hygiene code preserve compaction summaries and tool-call/result grouping, but the last saved heavy summary still has a compaction-signal failure and should be rerun under the current script semantics before calling this fully validated.

### Slice 5: Deterministic Runtime Execution

Code-backed for delivery/profile-style routine execution. `execution_guidance`, tool-role narrowing, `message_send`, and `user_profile` roles exist, and live tool smokes in the retained reports created the requested files exactly once.

### Slice 6: Cheap Condensation

Code-backed with a real summary-lane resolver and persistent history compaction cache. Treat it as partial closeout: long semantic retention and no procedural-skill pollution have live evidence, but dedicated real-pressure compaction/summary validation is still muddied by the stale heavy report.

### Slice 7: Progressive Scoped Context

Code-backed and wired through scoped instruction resolution plus adapter context loading. It is still partial by the plan's own standard because weaker/ambiguous cheap-route behavior after scoped-context hardening remains a live-quality tail, not just a missing doc update.

### Slice 8: Provider-Native Continuation

Code-backed but live-unvalidated on an official/key-based Responses endpoint. `openai_codex` has adapter-local `previous_response_id` and continuation gating, while the deployed Codex backend is documented as not supporting the feature, so "closed" here only means scaffolded and capability-gated.

### Slice 9: Strict Tool Protocol

Code-backed in the shared runtime path: provider defaults reject non-native tool fallback, and the tool loop requires native structured calls for tool-capable turns. Closeout validation is blocked indirectly because the full targeted script stops in `synapse_providers` tests before reaching the later adapter checks.

### Slice 10: Capability Lanes

Code-backed and partially validated at the domain layer. Lane candidate resolution, candidate profiles, catalog/preset surfaces, and route-state lane identity exist, but the provider test compile failure means the provider/profile side of the targeted gate is not green.

### Slice 11: Turn Admission

Code-backed and domain-validated. `turn_admission` has typed intent/action/reasons and passed its focused tests; remaining work is UX/persistence/audit coverage around all intent consumers rather than a missing implementation core.

### Slice 12: Model Profile Registry

Code-backed but not fully closeout-clean. `ResolvedModelProfile`, source/freshness/confidence, endpoint-aware cache paths, and catalog IO exist, yet the provider and adapter test targets currently fail before this can be declared fully validated.

### Slice 13: Context Pressure Manager

Code-backed with budget snapshots, trusted-window scaling, route-switch preflight, and compression policy plumbing. It remains partial because the retained heavy report does not provide a clean closeout signal for real compaction pressure, even though the current script now distinguishes healthy large-window runs from mandatory compaction cases.

### Slice 14: Modality Routing

Code-backed but partial. Structured media markers, lane requirements, admission blocking on text routes, and OpenRouter image-output live smoke exist; audio/video/music are still capability-routed but not fully artifact-delivered, and provider tests around media artifacts currently do not compile.

### Slice 15: Self-Repair

Code-backed with passing focused domain tests. Tool repair traces, route admission history, bounded ledgers, and execution-guidance repair hints exist; the remaining tail is continued audit of opaque provider errors and broader adapter validation.

### Slice 16: Memory Quality, Embedding, Self-Learning

Code-backed with passing focused domain tests for the governor and post-turn orchestration. The slice is still best described as partial hardening: generic graph pollution and durable mutation gates are implemented, but concept-heavy retrieval after real compaction remains an explicit quality tail.

### Slice 17: Structured Handoff

Code-backed and partly live-supported. The domain handoff packet exists and tests passed; the retained heavy report also shows the overflow route-switch case blocked rather than silently switching, but a fresh end-to-end downgrade/handoff closeout run would still be stronger evidence.

### Slice 18: Capability Probe/Profile Repair

Partial. Catalogs, endpoint-aware model cache, `/models` refresh paths, and context-limit observation repair are implemented, and OpenRouter image smoke passed, but the plan's own external registry/probe-down items are not done and provider tests currently fail to compile.

### Slice 19: Assumption Tracker

Code-backed typed base. Runtime assumptions, challenge/merge logic, formatting, and handoff integration exist and focused tests passed; durable promotion is intentionally not implemented yet.

### Slice 20: Epistemic State

Code-backed typed base. Epistemic state projection for runtime assumptions, model profiles, turn defaults, and memory entries exists and tests passed; deeper self-repair and recency-sensitive external fact integration remain open.

### Slice 21: Runtime Watchdog

Code-backed typed base, not a full autonomous watchdog system. Digest construction, alert formatting, subsystem observations, and tests exist, but there is no autonomous background polling loop yet.

### Slice 22: Runtime Calibration

Code-backed typed base. Route/tool/retrieval/delivery calibration records, suppression helpers, and focused tests exist; policy coverage is still a follow-through item rather than phase-closed behavior.

### Slice 23: Trace Janitor

Code-backed typed base. The janitor cleans repair, assumption, watchdog, calibration, and handoff traces with TTL/dedupe/count bounds, and focused tests passed; broader autonomous watchdog maintenance remains outside the implemented base.

### Slice 24: Runtime Adapter Contract

Code exists, but the "code-closed" claim is not test-clean right now. `runtime_adapter_contract` and shared command-effect surfaces are present, yet `synapse_adapters` lib tests fail to compile because nearby test fixtures still expect strings where `PresentedOutput` is now returned and omit `runtime_mcp_activated_tools`.

### Slice 25: Tool Notification Mapper

Code exists, but adapter closeout is blocked by the same `synapse_adapters` lib-test compile break. `runtime_tool_notifications` and `runtime_tool_observer` are present, so this is not docs-only; the problem is stale test-target wiring around the adapter crate.

### Slice 26: Web/Channel Extraction

Code exists for the extraction target, including runtime system prompt, context engine, history hygiene, and shared tool-notification modules outside the old channel monolith. Do not treat it as fully validated until the adapter lib-test fixture errors are fixed and the `context_engine` / runtime parity filters can actually run.

## Bottom Line

Most 4.10 slices have real code behind them; the plan is not merely aspirational. The main overclaims are closeout-level, not existence-level: provider tests, adapter lib tests, Slice 8 official continuation validation, Slice 18 external/probe-down profile discovery, media artifact completeness for audio/video/music, and fresh heavy compaction-pressure validation are not done yet.
