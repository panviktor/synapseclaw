# Phase 4.11 Subsystem Benchmark Audit

Date: 2026-04-14

Scope: compare the ten SynapseClaw memory/token/learning/runtime subsystems
against Hermes, OpenHands, AutoGPT, and LangGraph, then extract improvements
that should shape Phase 4.11. The scale is research-ideal: 10/10 means the
best useful agent-runtime behavior I would want, not the easiest patch.

## Sources

- SynapseClaw local docs and code:
  - `docs/fork/ipc-phase4_10-4_11-memory-token-learning-subsystems.md`
  - `docs/fork/ipc-phase4_10-slice-status-audit.md`
  - `crates/domain/src/application/services/*`
  - `crates/adapters/core/src/doctor/mod.rs`
- Hermes local checkout:
  - `<local-hermes-agent-checkout>/agent/context_compressor.py`
  - `<local-hermes-agent-checkout>/agent/memory_provider.py`
  - `<local-hermes-agent-checkout>/agent/memory_manager.py`
  - `<local-hermes-agent-checkout>/agent/auxiliary_client.py`
  - `<local-hermes-agent-checkout>/agent/model_metadata.py`
  - `<local-hermes-agent-checkout>/agent/insights.py`
  - `<local-hermes-agent-checkout>/agent/skill_commands.py`
  - `<local-hermes-agent-checkout>/hermes_cli/doctor.py`
- OpenHands:
  - https://github.com/All-Hands-AI/OpenHands
  - https://docs.openhands.dev/sdk/guides/context-condenser
  - https://docs.openhands.dev/usage/prompting/microagents-overview
- AutoGPT:
  - https://github.com/Significant-Gravitas/AutoGPT
  - https://docs.agpt.co/
- LangGraph:
  - https://github.com/langchain-ai/langgraph
  - https://docs.langchain.com/oss/python/langgraph/persistence

## Scoring Rubric

- 10: working, observable, failure-aware algorithm with durable product impact.
- 7-9: strong real implementation, but missing live validation, policy depth, or
  operator-facing diagnosis.
- 4-6: useful typed base or product surface, but still partial, mostly local, or
  lacking closed-loop behavior.
- 1-3: scaffold or manual surface without a strong runtime algorithm.
- 0: absent.

## Score Matrix

| # | Subsystem | SynapseClaw | Hermes | OpenHands | AutoGPT | LangGraph |
|---|---|---:|---:|---:|---:|---:|
| 1 | Provider context budget and context snapshot | 7 | 8 | 7 | 5 | 6 |
| 2 | History compaction and cheap condensation | 6 | 8 | 8 | 5 | 6 |
| 3 | Progressive scoped context engine | 7 | 7 | 8 | 4 | 5 |
| 4 | Embedding-first memory and recall backend | 7 | 8 | 5 | 5 | 8 |
| 5 | Memory quality governor and epistemic memory state | 7 | 5 | 5 | 4 | 7 |
| 6 | Learning evidence, candidate, precedent, and recipe pipeline | 7 | 7 | 5 | 6 | 6 |
| 7 | Skill promotion and skills governance | 6 | 8 | 8 | 6 | 4 |
| 8 | Runtime assumptions and structured session handoff | 7 | 6 | 6 | 5 | 9 |
| 9 | Tool repair, runtime calibration, watchdog, and trace janitor | 6 | 6 | 6 | 5 | 8 |
| 10 | Model profile registry, capability lanes, doctor, and auxiliary resolver | 7 | 8 | 6 | 5 | 5 |

## Subsystem Audit

### 1. Provider Context Budget And Context Snapshot

SynapseClaw already has a real typed budget layer:
`ProviderContextBudgetInput`, `ContextBudgetSnapshot`, pressure tiers, artifact
classes, and condensation plans. The weakness is that the budget is still mostly
an internal decision and retained validation around real pressure is not clean
enough.

Hermes is stronger on practical compaction mechanics: it feeds usage into a
compressor, protects head/tail by token budget, prunes old tool results, and
logs compression outcomes. OpenHands is strong on product-side context
condensing; LangGraph is strong as a persistent execution substrate but does not
own a full product-level context budget policy; AutoGPT is weaker here because
its block/workflow model is not centered on prompt-pressure control.

4.11 improvement: add a bounded `RuntimeDecisionTrace` entry for every turn
that records budget snapshot, route context window source, compaction decision,
observed usage, and whether pressure changed after compaction. Test by building
a synthetic over-budget history and asserting that the trace explains target,
ceiling, selected artifact, and compaction/handoff outcome.

### 2. History Compaction And Cheap Condensation

SynapseClaw has route-aware compression policy, source and summary caps, hygiene
trimming, tool protocol sanitization, and a cheap summary lane. It is behind
Hermes and OpenHands because Hermes keeps richer summarizer input, iterative
summary updates, scaled summary budgets, cooldown on summarizer failure, and
tool-call/result boundary alignment in one focused compressor.

The target is not to copy Hermes' Python object model, but to borrow the
algorithms that increase usefulness: token-budget tail protection, richer
tool-call/result serialization, iterative update of existing compaction
summary, and explicit fallback marker when summarization fails. AutoGPT does
not provide a stronger reference for this subsystem; LangGraph gives durable
checkpointing patterns but not the summarizer policy.

4.11 improvement: upgrade compaction to emit a typed pre/post pressure record
and to keep a richer structured compaction summary with sections for goal,
constraints, progress, files, next steps, critical context, and tool patterns.
Test by forcing two compactions and asserting that the second summary preserves
the first summary's critical facts, keeps recent tail messages, and never leaves
orphaned tool results.

### 3. Progressive Scoped Context Engine

SynapseClaw has a real scoped-context base: domain decides relevance, adapters
discover nearby `AGENTS.md`/`CLAUDE.md`, and both web/channel paths consume the
same context shape. The open tail is live-quality behavior on weaker cheap routes
and the lack of a direct operator explanation for why a scoped context file was
loaded or suppressed.

OpenHands' microagent model is the strongest comparison point because it treats
repository/user/org knowledge as discoverable, triggerable context rather than
always-on prompt ballast. Hermes has usable skill slash commands and external
skill directories; AutoGPT is more workflow-centric; LangGraph can implement the
state transitions but does not define this product behavior by itself.

4.11 improvement: make scoped-context admission part of the decision trace with
source path, relevance reason, char budget, suppression reason, and route
confidence. Test by running path-hinted, media-only, and ambiguous prompts and
asserting that scoped context is loaded only for the intended cases and the trace
explains the decision.

### 4. Embedding-First Memory And Recall Backend

SynapseClaw has a real memory substrate through unified memory ports, embeddings,
SurrealDB/vector adapters, profile stores, recall/store/forget tools, and turn
context reranking. Hermes is strong because memory is a product-level plugin
contract: built-in memory is always present, one external provider can be active,
providers can prefetch, sync turns, mirror explicit memory writes, and contribute
before compression.

LangGraph is strong at long-term memory and persistence as an application
substrate; OpenHands and AutoGPT are not stronger product references for
embedding-first personal memory. SynapseClaw's gap is not storage existence; it
is the lack of a unified pre-compress memory handoff and inspection view that
shows why a recall was selected, demoted, or rejected.

4.11 improvement: add `MemoryPreCompressHandoff` that turns the soon-to-be
dropped transcript region into candidate stable facts, procedures, failure
patterns, and assumptions, then routes every candidate through the existing
governor. Test by mixing stable project facts with generic dialogue and asserting
that only accepted durable write classes reach memory mutation.

### 5. Memory Quality Governor And Epistemic Memory State

SynapseClaw is ahead here. The governor blocks internal-only procedural noise,
generic dialogue, ephemeral repair traces, low-information repetition, malformed
consolidation output, unanchored generic concept nodes, and abstract
concept-to-concept graph edges. Epistemic state adds known/inferred/stale/
contradictory/needs-verification/unknown metadata to runtime and memory facts.

Hermes' provider hooks are useful, but its memory manager is more of an
orchestration contract than a typed quality policy. LangGraph gives solid
persistence and memory primitives but not this exact quality gate; OpenHands and
AutoGPT are weaker on memory pollution governance.

4.11 improvement: feed memory write decisions into the decision trace and
watchdog, including durable write class, rejected reason, evidence source, and
epistemic state. Test by forcing conflicting profile facts, stale recall, and a
repair trace; the system must downgrade or reject them without prompt prose
hacks.

### 6. Learning Evidence, Candidate, Precedent, And Recipe Pipeline

SynapseClaw has a real deterministic learning pipeline: typed tool facts become
evidence envelopes, candidates, profile patches, precedents, recipes, and failure
patterns without an extra model call on the hot path. This is stronger than a
generic "remember this" tool, but it still needs better product feedback loops
and eval scenarios that prove repeated runs reduce future work.

Hermes claims a self-improving loop and has skill/memory tooling, but the most
visible local algorithms are product-facing skill management, memory provider
hooks, and fuzzy skill patching rather than the same typed recipe pipeline.
AutoGPT's block platform and marketplace are good references for reusable
workflow packaging; LangGraph is good for persistence and replay; OpenHands is
good for context/microagent activation.

4.11 improvement: add learning review traces that show evidence -> candidate ->
assessment -> mutation/recipe/skill decision, without storing essays. Test with
three repeated successful tool patterns and one failure cluster; only the stable
recipe should become a skill promotion candidate.

### 7. Skill Promotion And Skills Governance

SynapseClaw has deterministic promotion from repeated successful recipes into
candidate or active learned skills, with thresholds, lineage, shadowing by
manual/imported skills, and failure-cluster contradiction checks. What is missing
is runtime governance: "is this skill active for this route/channel/tool
capability, and why?"

Hermes and OpenHands are stronger product references here. Hermes scans skill
directories, handles slash command activation, injects skill config, supports
external dirs, and has fuzzy skill patch tests. OpenHands microagents give a
clean model for triggerable knowledge and skill-like context. AutoGPT contributes
marketplace/block packaging ideas; LangGraph is not a skill product by itself.

4.11 improvement: add a `SkillGovernance` resolver that returns active,
shadowed, disabled, incompatible, blocked_missing_capability, or needs_setup for
each relevant skill. Test with learned/manual name collisions, disabled skills,
channel-incompatible skills, and missing tool/model capability.

### 8. Runtime Assumptions And Structured Session Handoff

SynapseClaw has a typed base: assumptions carry kind/source/freshness/confidence/
invalidation/replacement path, and handoff packets carry active task, defaults,
commitments, questions, failures, and cautions. The weakness is that handoff is
not yet a measured continuity algorithm across forced compaction, route
downgrade, helper delegation, and web/channel boundaries.

LangGraph is the strongest external reference because durable execution,
checkpoints, replay/time travel, and long-term memory are central features.
Hermes has structured compaction summaries and session hygiene, while OpenHands
has context condensation; AutoGPT has workflow state but less conversational
handoff depth.

4.11 improvement: attach handoff packet decisions to runtime traces and add a
handoff quality test pack. Test large-window -> small-window downgrade, web ->
channel carry-over, and helper-agent delegation; the resumed route must preserve
active task, explicit defaults, unresolved assumptions, and recent failure
cautions.

### 9. Tool Repair, Runtime Calibration, Watchdog, And Trace Janitor

SynapseClaw has typed bases for tool repair, calibration comparisons, watchdog
alerts, and janitor cleanup. It is not yet ideal because the watchdog is not an
autonomous diagnostic loop and repair/calibration state is not unified into one
operator-facing turn diagnosis.

Hermes' doctor and auxiliary fallback behavior are useful pragmatic references,
but Hermes is less typed around repair traces. LangGraph gives the best substrate
reference for recoverable executions and replay; OpenHands has strong agent
runtime/error handling as a product, but not the same typed watchdog; AutoGPT is
less targeted for this slice.

4.11 improvement: add a non-mutating background watchdog pass that consumes
decision traces, repair traces, context pressure, memory-governor rejections, and
model catalog staleness, then emits bounded proposals. Test repeated tool
failures, repeated compaction failures, stale model profile, and memory pollution
candidates; the watchdog must report/propose, not mutate durable state.

### 10. Model Profile Registry, Capability Lanes, Doctor, And Auxiliary Resolver

SynapseClaw has strong groundwork: lane resolution, candidate profiles,
source/freshness/confidence, bundled/user catalogs, endpoint-aware cache, context
limit observations, and a doctor module. Hermes is stronger today on unified
auxiliary routing: one client handles compression, web extraction, vision, local
custom endpoints, provider aliases, payment/connection fallback, and model
overrides.

OpenHands has good provider/product integration but is less relevant as a
capability doctor reference. AutoGPT has platform/provider setup concerns but not
as rich a model-profile resolver. LangGraph is generally provider-agnostic; it
does not solve product-level capability readiness for us.

4.11 improvement: create a domain-level auxiliary lane resolver and a runtime
capability doctor that classifies missing key, missing adapter, stale catalog,
unsupported modality, unknown context window, ignored reasoning controls, and
continuation unsupported. Test ordered fallback, explicit per-lane override,
provider error fallback, stale profile warning, and no-hot-path probing.

## Current Phase 4.11 Slice Audit

| 4.11 slice | Current score | Main gap | Required upgrade |
|---|---:|---|---|
| Runtime Decision Trace | 4 | Planned only; diagnostics spread across budget, admission, repair, watchdog. | Add one bounded turn trace joining route, context, tool, memory, and auxiliary decisions. |
| Capability Doctor | 5 | Existing doctor checks config/env/model probes, not full runtime readiness graph. | Classify provider/key/adapter/catalog/modality/reasoning/continuation/tool readiness. |
| Tool Self-Repair Trace | 6 | Tool repair exists, but not fully tied to rationale, future suppression, and operator view. | Store short-lived repair records with route/model, why-attempted, action, outcome, TTL. |
| Memory Pre-Compress Handoff | 4 | Planned; memory governor exists but no unified pre-compress candidate path. | Extract candidates from dropped context and pass through governor/write-class policy. |
| Skills Governance | 4 | Skill promotion exists, but active/blocked/shadowed state is not first-class runtime policy. | Resolve skill state by agent, channel, category, capability, and model/tool route. |
| Unified Auxiliary Model Resolver | 5 | Lane resolution exists, but auxiliary tasks are not unified across every non-primary lane. | Add one policy for compaction, embedding, vision, media, web extraction, validators, smoke lanes. |
| Usage, Cost & Pressure Insights | 4 | Provider context report exists; cost/pressure/tool-failure insight surface is not complete. | Aggregate tokens, cost status, cache use, compactions, pressure deltas, and failure classes. |
| Background Watchdog | 5 | Typed digest exists; autonomous non-mutating diagnostic pass is not wired. | Periodic/passive watchdog over traces, catalog freshness, memory health, and context pressure. |

## Cross-Subsystem Test Matrix

1. Context pressure: synthetic history exceeds target, triggers compaction or
   handoff, and decision trace records target, ceiling, before/after pressure,
   and selected reclaim action.
2. Compaction quality: two forced compactions preserve critical files, commands,
   task state, and tool patterns while keeping tool-call/result groups valid.
3. Scoped context: path-hinted prompt loads nearest scope; media-only prompt
   suppresses stale scoped context; trace explains both decisions.
4. Memory handoff: pre-compress candidate extraction accepts stable project fact
   and successful procedure, rejects generic dialogue and ephemeral repair trace.
5. Epistemic recall: stale/low-confidence recall is demoted behind stronger
   known anchors and surfaced with state/source/confidence.
6. Learning pipeline: repeated successful typed tool pattern creates recipe and
   skill candidate; contradictory failure cluster blocks promotion.
7. Skill governance: manual skill shadows learned skill; disabled skill is not
   active; missing capability blocks activation with a diagnostic reason.
8. Handoff continuity: large-window -> small-window downgrade carries active
   task, defaults, assumptions, and recent failure cautions without full replay.
9. Watchdog and janitor: repeated tool failure creates bounded alert and
   promotion candidate, then TTL/dedupe removes old traces without durable write.
10. Capability and auxiliary lanes: explicit per-lane override wins; auto mode
    falls through ordered candidates on payment/connection/provider errors; no
    unknown endpoint probe runs on the hot path.

## Phase 4.11 Recommendation

The most useful 4.11 is not "more memory" and not "more prompt instructions".
It should be an inspectable runtime control plane: decision trace first,
capability doctor second, then memory pre-compress handoff, skills governance,
auxiliary resolver, usage/cost/pressure insights, and a background watchdog.

Closeout should require both deterministic tests and at least one runtime/harness
scenario per major behavior. A slice should not be closed if it only adds a
display command or a typed struct without proving that the underlying runtime
decision changes product behavior.
