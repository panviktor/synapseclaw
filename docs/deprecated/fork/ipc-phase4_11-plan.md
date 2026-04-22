# Phase 4.11: Runtime Self-Diagnostics & Capability Governance

Phase 4.10: context engine, prompt economy & progressive loading | **Phase
4.11: inspectable runtime control plane** | next: TBD

---

## Status

Slices 1-4 are closed as of 2026-04-14 after implementation, audit fixes, and
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
- Mem0/Hermes: automatic turn-bound memory prefetch, background memory sync,
  reranked semantic search, provider lifecycle hooks, and circuit-breaker
  behavior for memory backends
- Zep/CrewAI: automatic task-start memory retrieval, task-end storage, and
  runtime recall without requiring a user phrase
- EdgeQuake, Graphiti, Cognee, and A-mem: hybrid graph/vector retrieval,
  temporal/provenance-aware facts, query expansion, and agentic memory linking
  as benchmarks for recall quality

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
- whether relevant durable memory was considered before broad live discovery,
  and why it was accepted, rejected, or treated as stale
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
- live CLI audit with a neutral local Matrix homeserver discovery task fixed
  repairable shell-policy hints and raised the no-progress hard cap enough for
  bounded local inventory turns

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

Status: **closed** (2026-04-14).

Closeout notes:

- added a domain-owned pre-compress handoff service that runs before
  live-agent compaction or channel session-hygiene trimming drops provider
  history
- centralized projected tool-call markers behind a domain formatter/parser so
  handoff logic is not matching ad hoc local strings
- limited hot-path extraction to structured runtime artifacts: projected tool
  sequences and typed `ToolRepairTrace` records; ordinary multilingual text is
  not promoted to durable memory by local heuristics
- trusted typed facts now require runtime-projected `system`/`assistant`
  anchors, and procedural recipes require runtime-projected assistant tool-call
  anchors; user-supplied marker text is treated as untrusted dialogue
- routed recipe candidates through precedent similarity, failure patterns
  through failure similarity, and all other candidates through the existing
  memory mutation/governor path
- passed bounded preservation hints into the compactor only when structural
  candidates exist, while generic dialogue remains a rejected/no-op write class
- wired handoff memory decisions into runtime decision traces for both
  live-agent and channel compaction paths
- audit pass closed bypasses in web resume/context-limit recovery, web/channel
  route-switch preflight, and legacy interactive CLI auto-compaction
- reread audit tightened the channel/context-recovery path so candidate
  extraction receives only the exact dropped messages and provenance records
  original dropped-message indices

Add a governed memory handoff before provider-facing context is dropped.

Required behavior:

- identify the exact transcript region that will be summarized or discarded
- do not infer stable profile facts or task-state facts from arbitrary user or
  assistant text; those require explicit memory/consolidation or future typed
  fact sources
- extract procedural and failure candidates only from standardized projected
  tool-call history and typed repair traces
- pass every candidate through existing `memory_quality_governor`, similarity
  checks, and durable write-class policy
- feed approved procedural material into recipe/precedent learning where
  appropriate
- pass compact handoff hints to the compactor so the summary preserves accepted
  structural candidates even when no durable write is allowed
- reject generic dialogue, concept-only world-knowledge, raw tool dumps,
  malformed consolidation output, and ephemeral repair traces

Acceptance tests:

- stable-looking multilingual or local-infra text from dropped context is not
  promoted without a typed source
- generic dialogue and concept-to-concept graph material are rejected
- structured tool sequence becomes precedent/recipe input, not profile prose
- typed repair trace becomes failure-pattern/review input while raw repair trace
  detail remains ephemeral
- forced compaction preserves approved structural handoff hints in the summary
  or trace

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

### Slice 11: Implicit Memory Recall And Verification

Status: **planned**.

Make durable memory a typed retrieval prior before expensive or broad live
discovery, even when the user does not explicitly say "use memory". The recall
path must reduce total context and tool work, not add memory text to an already
large provider prompt.

Motivation from the live Matrix/Tuwunel audit: the agent correctly used
`memory_recall` when prompted with "use long-term memory", but a neutral prompt
about the local Matrix server skipped memory and repeated broad shell discovery.
This slice closes that gap by making recall a runtime policy decision, not a
phrase-engine behavior.

External benchmark findings:

- Mem0's Hermes integration prefetches relevant memories before the next turn,
  syncs completed turns in the background, and keeps the chat path non-blocking
  with circuit-breaker behavior.
- Local Hermes confirms a pluggable memory provider lifecycle with
  `prefetch`, `queue_prefetch`, `sync_turn`, and `on_pre_compress` hooks as an
  interface benchmark.
- Zep with CrewAI and CrewAI's own memory system both retrieve relevant context
  automatically at task start and store after task completion, so recall is not
  dependent on the user asking for memory.
- LangGraph separates thread-scoped short-term state from cross-thread
  long-term stores, and supports semantic search/filtering for long-term
  memory.
- EdgeQuake, Graphiti, Cognee, and A-mem point toward higher-quality recall:
  graph+vector retrieval, query expansion, temporal/provenance windows,
  auto-routing recall, and agentic memory linking/evolution.

Required behavior:

- derive a typed `ImplicitMemoryRecallPlan` from the user turn before broad
  shell, filesystem, web, or package-discovery tools are planned
- build recall queries from intent, entities, task family, workspace/session
  scope, and domain aliases; do not require a magic phrase such as "use memory"
- query durable memory with hybrid matching: embedding similarity, lexical
  aliases, stable keys, categories, provenance, and optional graph/entity links
- scope recall to bounded classes such as `core`, `project`, `local_infra`,
  `procedural`, and `recent_success`; never dump raw memory into the prompt
- classify retrieved facts as stable anchors, volatile facts, procedural hints,
  or rejected candidates before model/tool use
- keep accepted anchors in typed runtime state for planner/tool admission first,
  and avoid provider-prompt insertion by default
- if final answer generation truly needs a memory hint, pass only a tiny
  bounded pointer or summary with provenance and mutable-fact verification
  policy; never pass full memory text
- use accepted memory to narrow subsequent tool plans so the system spends fewer
  tokens on broad discovery, repeated inventory scans, and redundant evidence
- use accepted memory as a tool prior: if local-infra memory says
  `tuwunel.service`, prefer minimal verification of that service/package before
  wide `ps`, `find`, `systemctl list-units`, or port scans
- force live verification for stale or mutable facts such as package versions,
  release versions, service status, paths that may move, and upstream URLs
- fall back to bounded live discovery when memory is absent, low-confidence,
  contradictory, out of scope, or expired
- record recall query, matched keys, confidence, staleness, accepted anchors,
  rejected candidates, verification policy, and first tool-prior decision in
  `RuntimeDecisionTrace`
- keep recall diagnostics available in shared web/channel surfaces without
  exposing raw memory payloads or secrets

Acceptance tests:

- a stored `local_infra_matrix_homeserver` memory is recalled for "What is our
  self-hosted Matrix server?" without an explicit memory phrase
- accepted local-infra memory causes the first verification tool plan to target
  the remembered service/package, not a wide host inventory scan
- stale version memory is not trusted as final truth until live package and
  upstream checks refresh it
- conflicting memories create a low-confidence or verification-first decision
  instead of a confident answer
- no relevant memory produces a bounded discovery plan and a trace entry saying
  recall was attempted but empty or rejected
- runtime diagnostics show recall query, accepted/rejected candidate ids,
  staleness, and verification policy with redacted payloads
- context-size accounting proves implicit recall reduced or preserved provider
  prompt size and did not add memory payloads by default
- web/channel/gateway harness passes the Matrix/Tuwunel scenario without the
  words "use long-term memory" in the user message

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
- `implicit_memory_recall`: pre-tool recall gating, hybrid alias/entity query,
  stale-fact verification, tool-prior narrowing, diagnostics redaction

Required runtime/harness scenarios:

- long dialogue with early/late anchors and forced compaction
- large-window -> small-window route downgrade with handoff
- repeated tool failure followed by repair and later suppression
- learned skill promotion after repeated successful recipe
- skill blocked by missing tool/model capability
- stale endpoint-specific model profile repaired by typed context-limit
  observation
- web/channel route diagnostics parity
- local-infra memory recall without explicit memory instruction: store a
  Matrix/Tuwunel fact, ask a neutral Matrix-server question, verify recall
  happens before shell discovery, and require minimal live verification before
  final answer

---

## Design Constraints

- Keep hexagonal boundaries: domain services define policy; adapters provide
  provider/platform details.
- Do not put Hermes-specific checks or subscription concepts in the core.
- Do not add phrase-engine routing.
- Do not make durable memory recall depend on a user phrase; recall must be
  triggered by typed intent, scope, and bounded retrieval policy.
- Do not put provider-specific tool dialects in the shared runtime.
- Do not make repair traces permanent by default.
- Treat implicit memory as a hypothesis for mutable facts; verify before final
  answers when service status, installed versions, releases, URLs, or paths can
  drift.
- Do not make diagnostics another prompt ballast.
- Do not solve implicit recall by adding memory blocks to the provider prompt:
  recall must primarily shape typed planner/tool decisions.
- Keep web and channel behavior behind the same runtime services.
- Treat OpenHands, AutoGPT, LangGraph, Hermes, Mem0, Zep, CrewAI, EdgeQuake,
  Graphiti, Cognee, and A-mem as benchmark sources, not code to copy one-to-one.

---

## Exit Criteria

Phase 4.11 is complete only when:

- every slice has deterministic domain tests and shared web/channel rendering
  coverage where relevant
- at least one runtime/harness scenario validates context pressure, memory
  handoff, skill governance, auxiliary fallback, and watchdog behavior
- an operator can inspect a failed or degraded turn and answer which route/model
  was selected, which candidates were available, what context pressure existed,
  why tools were attempted or blocked, which memory recalls and writes were
  accepted or rejected, which skills were active or blocked, and what
  short-lived repair or watchdog traces exist
- diagnostics remain bounded and mostly out of provider prompts
- no slice is closed by adding only a display command or typed struct without
  validating a real runtime decision
