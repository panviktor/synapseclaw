# Phase 4.11: Runtime Self-Diagnostics & Capability Governance

Phase 4.10: context engine, prompt economy & progressive loading | **Phase
4.11: inspectable runtime control plane** | next: TBD

---

## Status

Slices 1-3 are closed as of 2026-04-14 after implementation, audit fixes, and
targeted verification. Remaining slices are still implementation-ready draft.

Phase 4.11 should start after the remaining Phase 4.10 validation tails are
resolved or explicitly carried forward:

- provider/runtime targeted tests are not fully green yet
- retained adapter lib-test fixture drift was repaired for the runtime-command
  and gateway AppState fixtures touched by Slice 2
- retained heavy compaction evidence needs a fresh closeout run

This plan incorporates the benchmark audit from
`docs/fork/ipc-phase4_11-subsystem-benchmark-audit.md`. The strongest external
ideas to borrow are:

- Hermes: practical context compression, memory provider hooks, auxiliary
  fallback routing, model metadata discovery, insights, doctor surfaces, and
  skill command discoverability
- OpenHands: context condenser and triggerable microagent/skill-style context
- AutoGPT: reusable block/workflow packaging and marketplace-style surfacing
- LangGraph: durable execution, checkpoints, long-term memory/store, replay, and
  fault-tolerant state management

---

## Target

Build a runtime control plane where one failed or degraded turn can be inspected
without reading raw logs or replaying prompt text by hand.

The phase must answer:

- why this model and route were selected
- why a fallback route was or was not available
- why a tool was enabled, blocked, attempted, or suppressed
- what context-window estimate and pressure tier were used
- whether compaction, condensation, or handoff was required
- what memory candidates were accepted or rejected and why
- whether a repair trace was stored, when it expires, and whether it may become
  a review candidate
- which skills and auxiliary lanes were active, shadowed, disabled, or blocked

This is not a prompt-prose phase. Diagnostics must be typed, bounded, inspectable
through shared web/channel surfaces, and excluded from model context unless a
specific runtime policy says they are relevant.

---

## Slices

### Slice 1: Runtime Decision Trace

Status: **closed** (2026-04-14).

Add a domain-owned `RuntimeDecisionTrace` for each turn.

Closeout notes:

- implemented bounded domain-owned runtime decision traces in route/session
  state
- wired trace capture through channel admission, live-agent admission, tool
  repair fragments, and post-turn memory/auxiliary learning decisions
- exposed trace diagnostics through the shared `/model` and `/providers`
  runtime rendering path
- audited and fixed trace-id collision risk and memory-reason redaction so raw
  memory payloads are not retained in trace reasons
- verified with targeted domain tests and normal `channel-matrix` check; adapter
  lib-test remains blocked by unrelated stale test fixtures

Required behavior:

- record route candidates, selected route, fallback candidates, and rejection
  reasons
- record model-profile source, freshness, confidence, context window, max output,
  and capability gates
- record context budget snapshot before/after compaction or handoff
- record tool admission decisions, tool suppression decisions, and repair hints
- record memory write decisions with durable write class and rejection reason
- record auxiliary lane choices for compaction, embedding, validators, media, web
  extraction, and smoke/cheap reasoning
- retain traces in bounded route/session state, with redaction of secrets and
  large payloads
- expose the trace through shared runtime diagnostics for web and channel paths
- do not inject the whole trace into normal provider prompts

Acceptance tests:

- synthetic over-budget turn produces a trace with target/ceiling pressure,
  selected reclaim action, and post-action pressure
- route fallback turn records candidates and why the chosen route won
- blocked tool turn records the tool name, role, admission reason, and visible
  diagnostic reason
- rejected memory mutation records write class and governor rejection reason
- web and channel runtime diagnostics render the same trace fields

### Slice 2: Capability Doctor

Status: **closed** (2026-04-14).

Upgrade the existing doctor into a runtime capability readiness graph.

Closeout notes:

- added a domain-owned typed `CapabilityDoctorReport` readiness graph for
  provider key, adapter, plan denial, model profile, route, lane, tools, memory,
  embeddings, channel delivery, reasoning controls, and native continuation
- exposed `/doctor` as a shared runtime command through the same web/channel
  adapter-core executor and deterministic text renderer
- wired `synapseclaw doctor` to include the typed capability section while
  keeping live model probing isolated in explicit `doctor models`
- reused endpoint-aware `WorkspaceModelProfileCatalog`, bundled catalog data,
  cached observations, and route profile confidence/freshness before falling
  back to low-confidence assumptions
- verified missing key, unsupported modality/tooling, stale metadata,
  context-limit profile monotonicity, and shared web/channel rendering with
  targeted domain and adapter tests
- follow-up audit fixed false readiness edges for web delivery, disabled
  reasoning controls, Codex/Gemini OAuth auth paths, Qwen OAuth envs, Bedrock
  AWS credential chains, and Ollama `:cloud` auth requirements

Required behavior:

- classify readiness for provider key, provider adapter, model profile, route,
  lane, tool registry, memory backend, embedding backend, and channel delivery
- distinguish missing key, missing adapter, stale catalog, unknown context
  window, unsupported modality, ignored reasoning controls, unsupported native
  continuation, and provider/plan denial
- use bundled catalog, user catalog, endpoint-aware cache, and typed provider
  observations before falling back to low-confidence assumptions
- keep live probing adapter-local and never on the hot path for normal turns
- expose doctor output as typed data plus shared text rendering

Acceptance tests:

- missing API key is reported as `missing_key`, not generic provider failure
- unsupported image/audio/video/tool lane is reported as unsupported modality or
  unsupported tool capability
- stale cached metadata is reported as stale with refresh recommendation
- context-limit observation lowers or fills an endpoint-specific model profile
  but does not raise it
- doctor output is deterministic across web/channel command surfaces

### Slice 3: Tool Self-Repair Trace

Status: **closed** (2026-04-14).

Promote the current tool repair base into a complete short-lived repair ledger.

Closeout notes:

- expanded `ToolRepairTrace` into a short-lived repair ledger with tool role,
  route/model, safe argument shape, admission state, attempt reason, repair
  outcome, repeat count, TTL, and typed suppression key
- wired enriched traces through live agent and channel/runtime tool loops for
  hook cancellation, approval denial, duplicate guard, execution errors, and
  reported tool failures
- added successful-tool observations that resolve the same tool warning or
  downgrade same-role alternatives instead of creating durable negative memory
- kept suppression and janitor promotion behind typed keys/gates, with repair
  traces remaining ephemeral and excluded from durable memory/profile writes
- exposed the richer repair ledger through runtime decision traces and shared
  `/model`/`/providers` diagnostics without raw tool argument values
- audit pass fixed execution-attempt classification, same-batch failure->success
  resolution, and diagnostic argument-shape rendering without key lowercasing

Required behavior:

- record tool name, tool role, failure kind, selected route/model, attempted
  arguments shape, admission state, why the tool was attempted, repair action,
  repair outcome, and TTL
- feed recent repair traces into runtime calibration and watchdog alerts
- allow route/tool suppression only through typed suppression keys, not string
  parsing
- create promotion candidates for repeated failure classes behind explicit
  janitor gates
- never promote raw repair traces directly into durable memory or user profile

Acceptance tests:

- repeated same tool failure dedupes into a bounded trace and alert
- successful repair clears or downgrades the next warning instead of producing a
  permanent negative memory
- failed repair can suppress the same failing tool when an equivalent same-role
  tool exists
- trace janitor expires old traces and caps the retained history

### Slice 4: Memory Pre-Compress Handoff

Add a governed memory handoff before provider-facing context is dropped.

Required behavior:

- identify the exact transcript region that will be summarized or discarded
- extract candidate stable profile facts, task-state facts, successful
  procedures, failure patterns, unresolved assumptions, and repair summaries
- pass every candidate through existing `memory_quality_governor`,
  learning-quality assessment, and durable write-class policy
- feed approved procedural material into recipe/precedent learning where
  appropriate
- pass compact handoff hints to the compactor so the summary preserves accepted
  facts even when no durable write is allowed
- reject generic dialogue, concept-only world-knowledge, raw tool dumps,
  malformed consolidation output, and ephemeral repair traces

Acceptance tests:

- stable project fact from dropped context is accepted with provenance
- generic dialogue and concept-to-concept graph material are rejected
- successful repeated procedure becomes precedent/recipe input, not profile
  prose
- repair trace becomes a short-lived review candidate, not durable memory
- forced compaction preserves approved handoff facts in the summary or trace

### Slice 5: Skills Governance

Make skill activation and blocking a first-class runtime decision.

Required behavior:

- resolve skill state by agent, channel/platform, category, capability
  requirement, model route, and tool route
- return one of: active, candidate, shadowed, disabled, incompatible,
  blocked_missing_capability, needs_setup, or deprecated
- preserve current skill-promotion policy: manual/imported skills shadow learned
  skills; repeated recipes promote only when thresholds and contradiction gates
  pass
- expose active and blocked skills through runtime diagnostics without loading
  every skill into provider context
- support operator review of learned/candidate skills before broad activation

Acceptance tests:

- manual active skill shadows learned skill with same task family
- disabled skill is not active for web or channel
- skill requiring an unavailable tool/model capability is blocked with a clear
  reason
- repeated successful recipe creates or refreshes a learned skill candidate
- contradictory failure cluster blocks promotion

### Slice 6: Unified Auxiliary Model Resolver

Unify all non-primary model lanes behind one policy.

Required behavior:

- support lanes for compaction, embedding, vision/image understanding,
  image/audio/video generation, web extraction, tool validators, and cheap
  reasoning/smoke checks
- resolve candidates in this precedence order: explicit per-lane config, user
  catalog override, bundled catalog, compatible reasoning/default route, then
  adapter fallback
- record provider/model/profile source/freshness/confidence in the decision
  trace
- support explicit fallback behavior for payment, connection, provider, and
  unsupported-capability failures
- keep provider-specific URL rewriting and protocol details in adapters
- do not live-probe unknown providers during the hot path

Acceptance tests:

- explicit compaction lane override wins over default route
- auto mode falls back to the next candidate on payment or connection failure
- unsupported modality candidate is skipped before provider call
- endpoint-aware profile cache prevents one provider endpoint from polluting
  another endpoint with the same model id
- decision trace records auxiliary lane candidate order and final selection

### Slice 7: Usage, Cost And Pressure Insights

Add operator-facing runtime insights that explain cost and context pressure.

Required behavior:

- aggregate prompt/input tokens, output tokens, cached tokens, and unknown-token
  counts by route, provider, model, lane, channel, and session
- aggregate compaction count, compaction cache hits, summary-lane model, pressure
  before/after compaction, and handoff count
- aggregate tool failure classes, repair outcomes, watchdog alerts, and
  expensive-test counters
- track pricing status as known, unknown, included, or actual-provider-reported
  where the adapter supplies it
- expose compact table and JSON forms through shared web/channel diagnostics
- avoid a separate product shell; this extends runtime observability

Acceptance tests:

- sessions with known pricing compute estimated cost and sessions with unknown
  pricing are flagged, not silently counted as zero-cost
- cached-token usage appears when provider reports it
- compaction pressure before/after appears for forced compaction scenarios
- tool failure classes aggregate by tool and route
- web/channel diagnostics render from the same typed snapshot

### Slice 8: Background Watchdog

Turn the current typed watchdog base into a non-mutating diagnostic pass.

Required behavior:

- periodically or opportunistically inspect bounded route/session state, recent
  decision traces, repair traces, calibration records, context pressure, memory
  health, embedding health, channel health, and model catalog freshness
- report repeated model/tool mismatch, repeated compaction failure, rising
  pressure trend, recurring repair trace, memory pollution candidate, stale model
  profile, and contradictory catalog entry
- emit compact alerts and proposed actions only; do not mutate durable memory,
  user profile, skills, or model catalog without an explicit policy gate
- run trace janitor maintenance for active web/channel route states
- expose watchdog digest through the same diagnostics surface as model/provider
  help

Acceptance tests:

- repeated route/model capability mismatch creates one deduped alert with refresh
  or switch recommendation
- repeated compaction failure creates a context-budget alert and does not retry
  recursively
- memory pollution candidate is reported but not written to durable memory
- stale model catalog entry produces a refresh recommendation
- janitor removes old alerts and handoff artifacts by TTL/count bounds

---

## Shared Test Plan

Required deterministic test groups:

- `runtime_decision_trace`: trace construction, redaction, web/channel parity
- `capability_doctor`: readiness classification and profile freshness
- `tool_repair_trace`: failure-class dedupe, suppression keys, TTL cleanup
- `memory_precompress_handoff`: governed candidate extraction and pollution
  rejection
- `skills_governance`: active/shadowed/disabled/blocked skill resolution
- `auxiliary_model_resolver`: candidate order, fallback, lane override, profile
  provenance
- `usage_pressure_insights`: token/cost/cache/compaction/failure aggregation
- `background_watchdog`: non-mutating alert generation and janitor maintenance

Required runtime/harness scenarios:

- long dialogue with early/late anchors and forced compaction
- large-window -> small-window route downgrade with handoff
- repeated tool failure followed by repair and later suppression
- learned skill promotion after repeated successful recipe
- skill blocked by missing tool/model capability
- stale endpoint-specific model profile repaired by typed context-limit
  observation
- web/channel route diagnostics parity

---

## Design Constraints

- Keep hexagonal boundaries: domain services define policy; adapters provide
  provider/platform details.
- Do not put Hermes-specific checks or subscription concepts in the core.
- Do not add phrase-engine routing.
- Do not put provider-specific tool dialects in the shared runtime.
- Do not make repair traces permanent by default.
- Do not make diagnostics another prompt ballast.
- Keep web and channel behavior behind the same runtime services.
- Treat OpenHands, AutoGPT, LangGraph, and Hermes as benchmark sources, not code
  to copy one-to-one.

---

## Exit Criteria

Phase 4.11 is complete only when:

- every slice has deterministic domain tests and shared web/channel rendering
  coverage where relevant
- at least one runtime/harness scenario validates context pressure, memory
  handoff, skill governance, auxiliary fallback, and watchdog behavior
- an operator can inspect a failed or degraded turn and answer which route/model
  was selected, which candidates were available, what context pressure existed,
  why tools were attempted or blocked, which memory writes were accepted or
  rejected, which skills were active or blocked, and what short-lived repair or
  watchdog traces exist
- diagnostics remain bounded and mostly out of provider prompts
- no slice is closed by adding only a display command or typed struct without
  validating a real runtime decision
