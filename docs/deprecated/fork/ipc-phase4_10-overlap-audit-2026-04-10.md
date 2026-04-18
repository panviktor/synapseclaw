# Phase 4.10 Overlap Audit

Date: 2026-04-10

Updated: 2026-04-11 after the Slice 12/13 follow-through and local Hermes
source audit.

## Purpose

This audit answers a specific question:

- which of the current `4.10` follow-up slices are genuinely new,
- which are upgrades of ideas already promised in earlier plans,
- and where the current implementation should be revisited with stronger
  algorithms informed by product research and scientific literature rather than
  additional thin wrappers or renamed heuristics.

## High-Level Verdict

The current codebase is **not stuck because nothing exists**.

It is stuck because several important systems now exist only in **first-pass**
form:

- compact context exists and is now artifact/window-aware, but runtime hygiene
  still lacks several Hermes-style safety valves
- model-profile routing exists and now carries provenance/freshness/confidence,
  but endpoint-aware context-window discovery is still incomplete
- admission exists, but only for a narrow subset of turn classes
- retrieval hardening exists, but not yet as an explicit memory-quality policy

So the right next move is **algorithmic strengthening**, not more parallel
micro-systems.

## Earlier Plans That Already Foreshadowed Current Work

### Typed handoff and deterministic bridges

Already present in earlier plans:

- [ipc-phase4_1-plan.md](ipc-phase4_1-plan.md) — deterministic multi-agent workflows with **typed handoffs**
- [ipc-phase4_6-plan.md](ipc-phase4_6-plan.md) — **state-aware continuation policy** and dialogue-state bridge

Current code baseline:

- [route_switch_preflight.rs](../../crates/domain/src/application/services/route_switch_preflight.rs)
- [turn_defaults.rs](../../crates/domain/src/domain/turn_defaults.rs)
- [turn_defaults_resolution.rs](../../crates/domain/src/application/services/turn_defaults_resolution.rs)

Audit verdict:

- `Slice 17` is **not a brand-new idea**
- it is the correct stronger realization of earlier typed-handoff/continuation commitments

Research-driven upgrade required:

- **yes**
- current code should be revisited with stronger checkpoint/handoff patterns,
  especially from durable execution systems such as LangGraph docs on durable
  execution and checkpointing

### Memory quality, decay, confidence, promotion

Already present in earlier plans:

- [ipc-phase4_9-plan.md](ipc-phase4_9-plan.md) — memory quality, compaction,
  decay, confidence-aware promotion, retrieval calibration
- [memory-learning-foundation-plan.md](memory-learning-foundation-plan.md) —
  `resolve_conflict()`, confidence, decay, promotion, explicit signals
- [ipc-phase4_3-plan.md](ipc-phase4_3-plan.md) — fact confidence, reflections,
  skills, decay/GC

Current code baseline:

- [retrieval_service.rs](../../crates/domain/src/application/services/retrieval_service.rs)
- [memory_recall.rs](../../crates/adapters/tools/src/memory_recall.rs)
- [entity_extractor.rs](../../crates/adapters/core/src/memory_adapters/entity_extractor.rs)
- [post_turn_orchestrator.rs](../../crates/domain/src/application/services/post_turn_orchestrator.rs)
- [surrealdb_adapter.rs](../../crates/adapters/memory/src/surrealdb_adapter.rs)

Audit verdict:

- `Slice 16` and `Slice 23` are **explicit upgrades of prior memory-quality work**
- current implementation is functional but still too patchy:
  - retrieval reranking
  - entity filtering
  - decay and promotion
  all exist, but not yet under one clear governor

Research-driven upgrade required:

- **yes**
- current memory-quality code should be revisited with stronger research-backed
  ideas around memory tiers, compaction, and promotion

### Capability-driven routing and modality separation

Already present in earlier plans:

- [channel-triage.md](channel-triage.md) — capability-driven channels
- current `4.10` groundwork already introduced lanes and candidate profiles

Current code baseline:

- [model_lane_resolution.rs](../../crates/domain/src/application/services/model_lane_resolution.rs)
- [turn_model_routing.rs](../../crates/domain/src/application/services/turn_model_routing.rs)
- [turn_admission.rs](../../crates/domain/src/application/services/turn_admission.rs)
- [model_catalog.rs](../../crates/domain/src/config/model_catalog.rs)

Audit verdict:

- `Slice 12` and `Slice 14` are **not separate inventions**
- they are the correct stronger form of already-landed capability routing

Research-driven upgrade required:

- `Slice 12`: **medium**
- `Slice 14`: **medium**
- mostly product/system research, with some supporting literature

### Dialogue state and active assumptions

Already present in earlier plans:

- [ipc-phase4_6-plan.md](ipc-phase4_6-plan.md) — current-conversation targets,
  dialogue state, ambiguity resolution, state-aware continuation

Current code baseline:

- [turn_defaults.rs](../../crates/domain/src/domain/turn_defaults.rs)
- [turn_defaults_resolution.rs](../../crates/domain/src/application/services/turn_defaults_resolution.rs)
- [message_send.rs](../../crates/adapters/tools/src/message_send.rs)

Audit verdict:

- `Slice 19` is **the correct explicit evolution** of earlier dialogue-state ideas
- current system already carries implicit assumptions, but only as scattered defaults

Research-driven upgrade required:

- **yes**
- current code should be revisited using dialogue-state / belief-tracking ideas,
  not only more ad hoc per-tool defaults

### Self-repair, reflection, and post-failure learning

Already present in earlier plans:

- [ipc-phase4_3-plan.md](ipc-phase4_3-plan.md) — reflections and lessons
- [ipc-phase4_9-plan.md](ipc-phase4_9-plan.md) — failure memory, promotion/deprecation,
  inspectable learning
- [memory-learning-foundation-plan.md](memory-learning-foundation-plan.md) —
  explicit reflection, conflict resolution, explicit hot-path signals

Current code baseline:

- [turn_admission.rs](../../crates/domain/src/application/services/turn_admission.rs)
- [loop_detection.rs](../../crates/domain/src/application/services/loop_detection.rs)
- [post_turn_orchestrator.rs](../../crates/domain/src/application/services/post_turn_orchestrator.rs)

Audit verdict:

- `Slice 15` and `Slice 22` are **not invented from zero**
- the current code now has first-pass bounded typed repair and calibration
  ledgers; remaining work is policy depth, opaque provider/tool error coverage,
  and calibration behavior as adjacent epistemic/assumption paths harden

Research-driven upgrade required:

- **yes**
- this should be guided by tool-aware self-correction and reflection research,
  not by more free-form prompt reflection

## Current Code: What Is “Base but Too Weak”

### 1. Model profile resolution exists, but discovery is still incomplete

Current implementation:

- [model_lane_resolution.rs](../../crates/domain/src/application/services/model_lane_resolution.rs)
- [model_profile_catalog.rs](../../crates/domain/src/ports/model_profile_catalog.rs)

Current status:

- `ResolvedModelProfile` now carries:
  - `context_window_tokens`
  - `max_output_tokens`
  - `features`
  - provenance
  - freshness
  - confidence
  - explicit unknown-state
- bundled/local/cached catalogs can feed route profiles
- `/model` and `/models` surface profile provenance and quality
- cached live model/profile metadata is now scoped by normalized endpoint in
  addition to provider name
- custom endpoint provider inference is catalog-driven through editable
  `api_base_urls`, not Rust model/URL match arms

Current weakness:

- no models.dev-style provider-aware registry source yet
- no external provider-aware registry ingestion yet
- no active probe-down tier strategy for unknown/local endpoints yet; the current
  path is endpoint-aware `/models` refresh plus typed context-limit observations
  from failed turns and operator catalog overrides

Implication:

- `Slice 12` / `Slice 18` should strengthen the existing profile resolver
  instead of adding another metadata layer

### 2. Context pressure is artifact-aware, but runtime hygiene gaps remain

Current implementation:

- [provider_context_budget.rs](../../crates/domain/src/application/services/provider_context_budget.rs)
- [history_compaction.rs](../../crates/domain/src/application/services/history_compaction.rs)
- [turn_admission.rs](../../crates/domain/src/application/services/turn_admission.rs)
- [history_compaction_cache.rs](../../crates/domain/src/ports/history_compaction_cache.rs)

Current status:

- `ContextBudgetSnapshot` tracks artifact-level provider-facing pressure
- trusted model context is treated as `input + output`
- safe input subtracts reserved output headroom
- compression threshold is `50%` of safe input
- hard safety ceiling is `85%` of safe input
- large trusted windows scale by ratio
- route-switch preflight can compact or block big-window -> small-window moves
- compaction has protected head/tail and avoids splitting tool-call/tool-result
  groups
- persistent shared compaction cache is visible through web and channel route
  inspection
- provider-reported input/prompt token usage can feed the next compaction decision
- web/channel session hygiene now provides a pre-provider guard for already-bloated
  provider-facing history
- old large tool results are pruned before summary-lane calls
- post-compaction tool protocol sanitization removes orphan results and inserts
  bounded missing-result stubs

Current weakness after Hermes source audit:

- no pluggable context-engine interface yet; defer until the domain service/port
  boundary stabilizes

Implication:

- `Slice 13` should continue as a follow-through on the existing pressure
  manager, not a replacement subsystem

### 3. Admission exists, but the intent/capability matrix is narrow

Current implementation:

- [turn_admission.rs](../../crates/domain/src/application/services/turn_admission.rs)

Current weakness:

- strong for:
  - reasoning
  - tool-heavy turns
  - multimodal-understanding basics
- still narrow for:
  - image/audio/video generation
  - nuanced repair states
  - full cross-channel consistency

Implication:

- `Slice 14` and `Slice 15` should extend this matrix rather than wrap it externally

### 4. Retrieval hardening exists, but policy is still implicit

Current implementation:

- [retrieval_service.rs](../../crates/domain/src/application/services/retrieval_service.rs)
- [memory_recall.rs](../../crates/adapters/tools/src/memory_recall.rs)
- [entity_extractor.rs](../../crates/adapters/core/src/memory_adapters/entity_extractor.rs)

Current weakness:

- retrieval pollution is managed by targeted fixes
- generic concept pollution is managed by filters
- there is no explicit memory-quality policy object yet

Implication:

- `Slice 16` should consolidate these behaviors into one governor

## Slice-by-Slice Audit

| Slice | Earlier plan overlap | Current code baseline exists? | Research-driven upgrade required? | Notes |
|---|---|---:|---:|---|
| 12 `ResolvedModelProfile` registry | capability-driven channels, current `4.10` routing groundwork | yes | medium | provenance/confidence, endpoint-scoped live cache, and catalog-driven URL inference landed; next upgrade is provider-aware registry/error feedback |
| 13 context-pressure manager | `4.10` compaction + `4.6` continuation/state bridge | yes | yes | artifact/window-aware pressure plus Hermes-style hygiene/tool-result pruning landed; defer pluggable context-engine layer |
| 14 modality routing | capability routing already present in channels + `4.10` lanes | yes | medium | finish matrix, do not add a second router |
| 15 explainable self-repair | reflections/failure memory in `4.3/4.9` | partial | yes | first-pass typed repair ledger landed; continue opaque-error and policy hardening |
| 16 memory-quality governor | `4.9` memory quality + foundation plan | yes | yes | governor policy layer landed; continue regression/live hardening for concept-heavy sessions and real-compaction ranking |
| 17 typed handoff packets | `4.1` typed handoffs + `4.6` continuation policy | partial | yes | make handoff a first-class typed bridge |
| 18 background capability probe | current catalog/profile groundwork | partial | medium | endpoint-scoped cache, catalog-driven URL inference, and typed context-limit observations landed; add external registry ingestion and optional probe-down fallback |
| 19 assumption tracker | `4.6` dialogue state and referential resolution | partial | yes | promote implicit defaults into explicit assumptions |
| 20 epistemic state | `4.3` fact confidence + foundation conflict rules | partial | yes | upgrade confidence/conflict into proper knowledge-state categories |
| 21 watchdog + digest | `4.1` resilient execution and existing health checks | partial | medium | strong systems pattern, less prior direct code |
| 22 calibration + counterfactual | `4.9` retrieval calibration / confidence thresholds | partial | yes | should build on structured outcomes, not prose reflection |
| 23 janitor | `4.9` compaction/decay + foundation GC/decay | yes | yes | prevent new typed traces from becoming junk memory |

## Research Tracks That Should Inform Upgrades

These are the main research/product tracks worth consulting before strengthening the current code:

- **memory tiers / virtual context**
  - MemGPT: <https://arxiv.org/abs/2310.08560>
- **long-context prompt compression**
  - LongLLMLingua: <https://arxiv.org/abs/2310.06839>
- **verbal reflection / self-repair**
  - Reflexion: <https://arxiv.org/abs/2303.11366>
- **tool-assisted critique / self-correction**
  - CRITIC: <https://arxiv.org/abs/2305.11738>
- **iterative self-feedback**
  - Self-Refine: <https://arxiv.org/abs/2303.17651>
- **skill reuse / lifelong accumulation**
  - Voyager: <https://arxiv.org/abs/2305.16291>

Non-paper systems references also remain directly relevant:

- Hermes context compression / provider runtime / fallback models:
  - provider usage feedback into future compaction decisions
  - pre-agent gateway/session hygiene safety net
  - cheap pruning of old tool results before summary-lane calls
  - post-compaction tool-pair sanitizer
  - endpoint-aware context-window cache
  - catalog-driven provider URL inference
  - typed context-limit feedback into endpoint-aware cache repair landed
  - still open: external provider-aware registry ingestion and optional active
    probe-down for unknown/local endpoints
- OpenClaw context engine and modality-specific model slots
- LangGraph durable execution / checkpointing

## Practical Guidance

When implementing the remaining slices:

1. **Prefer strengthening current primitives over adding siblings**
   - strengthen `ResolvedModelProfile`
   - strengthen `TurnAdmissionPolicy`
   - strengthen retrieval/memory policy
   - strengthen compaction into pressure management

2. **If a prior plan already promised the behavior, mark the code path as an upgrade target**
   - not as “new subsystem”

3. **Use research to replace weak heuristics**
   - char-only budgets
   - single-threshold compaction
   - prose-only failure memory
   - generic semantic extraction filters

4. **Keep the hot path small**
   - watchdog, calibration, probe, janitor stay mostly background/ephemeral

## Bottom Line

The biggest risk now is **not missing concepts**.

The biggest risk is continuing to layer new names over already-landed base
implementations instead of upgrading those base implementations with stronger,
more principled algorithms.
