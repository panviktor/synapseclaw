# Phase 4.11: Runtime Self-Diagnostics & Capability Governance

Phase 4.10: context engine, prompt economy & progressive loading | **Phase 4.11: runtime self-diagnostics, capability governance & Hermes borrow backlog** | next: TBD

---

## Status

Draft only.

The Hermes-derived Phase 4.10 context-safety tail is now code-landed:

- provider-reported usage tokens feed the next compaction decision
- pre-agent gateway/session hygiene handles already-bloated transcripts
- old large tool results are cheaply pruned before summary-lane calls
- post-compaction tool protocol sanitizer preserves valid tool-call groups
- endpoint-aware context-window resolver/cache is good enough for route switches

This document remains a backlog until Phase 4.10 finishes final targeted
validation and closes the remaining non-Hermes quality/routing tails.

---

## Problem

Phases 4.8, 4.9, and 4.10 made the runtime stronger at:

- embedding-first memory and retrieval
- typed user/profile/dialogue state
- self-learning and memory quality
- context budgeting and provider-facing prompt economy
- capability-aware model routing

The remaining weakness is that the system is still not inspectable enough when
it chooses a route, tool, memory write, compaction policy, or repair path.

The next phase should make SynapseClaw better at answering:

- why did this turn use this model?
- why was this tool enabled or blocked?
- why did compaction happen now?
- which capability is missing: key, provider, route, model profile, adapter, or tool?
- what did the system learn from this failure, and when will that trace expire?
- which skills are active for this agent/channel/platform?

This is not a request for more prompt prose. It is a request for runtime-visible
diagnostics and governance around the primitives built in 4.8-4.10.

---

## Hermes Borrow Backlog

Hermes remains useful as a source of product/runtime primitives, not as a system
to copy one-to-one.

Strong ideas to adapt:

- context and compaction hygiene from `agent/context_compressor.py`
- pre-agent transcript hygiene from `gateway/run.py`
- endpoint-aware model context discovery from `agent/model_metadata.py`
- memory pre-compress hooks from `agent/memory_provider.py`
- auxiliary model selection from `agent/auxiliary_client.py`
- usage/cost/insights surfaces from `agent/usage_pricing.py` and `agent/insights.py`
- command/tool/skill discoverability from `hermes_cli/commands.py` and
  `hermes_cli/skills_config.py`

Ideas not to copy directly:

- a large slash-command surface before runtime policy is stable
- product-specific subscription checks as core architecture
- string/path heuristics in the domain runtime
- provider-specific tool dialects in the shared loop
- a plugin-style context-engine abstraction before the current hexagonal
  services stabilize

---

## Target

Build a runtime layer where:

1. model and tool choices are explainable without reading logs by hand
2. capability failures are classified into operator-actionable causes
3. repair traces are short-lived, structured, and useful for future turns
4. memory extraction before compaction is governed by pollution gates
5. skills and auxiliary model lanes are visible and governable per route
6. context and routing diagnostics remain shared by web and channels

In short:

```text
context-safe runtime
+ capability-aware model profiles
+ explainable tool/model decisions
+ short-lived repair memory
+ skill and auxiliary-lane governance
= an agent that can diagnose its own execution path
```

---

## Slices

### Slice 1: Runtime Decision Trace

Add a typed, bounded trace for each turn that records:

- selected route and fallback candidates
- model-profile source and freshness
- context-window confidence
- capability gates that passed or failed
- compaction policy decision and observed token pressure
- tool admission decisions

This should be visible through operator/debug surfaces, not injected into every
model prompt.

### Slice 2: Capability Doctor

Add a doctor-style view for model/tool readiness:

- provider key present or missing
- adapter available or missing
- live model metadata fresh, stale, or unknown
- native context window known, cached, guessed, or overridden
- supported modalities: text, tools, image, audio, video, multimodal
- reasoning controls supported, ignored, or unknown
- native continuation supported, ignored, or unknown

The goal is to prevent accidental use of the wrong model for a turn.

### Slice 3: Tool Self-Repair Trace

Store short-lived repair records for tool failures:

- failing tool name and typed error class
- selected route/model at failure time
- why the runtime/model attempted that tool
- repair action attempted
- outcome
- TTL, initially measured in days rather than permanent memory

These traces should help future turns avoid repeat failures without polluting
durable memory.

### Slice 4: Memory Pre-Compress Handoff

Before old context is dropped from provider-facing history, let the memory
governor inspect candidate material:

- stable user/profile facts
- successful procedural steps
- repeated tool failure patterns
- unresolved assumptions
- short-lived repair traces

This must use the existing memory hygiene gates. It must not reintroduce
low-quality triples like pure-dialogue family/teaching artifacts.

### Slice 5: Skills Governance

Make active skills inspectable and controllable by:

- agent
- channel/platform
- category
- capability requirement
- model/tool route

The runtime should be able to explain whether a skill is active, shadowed,
disabled, or blocked by a missing capability.

### Slice 6: Unified Auxiliary Model Resolver

Unify policy for non-primary model lanes:

- compaction
- embedding
- vision/image understanding
- image/audio/video generation
- web extraction
- tool-specific validators
- cheap reasoning/smoke lanes

Each lane should support ordered model candidates, provider-specific adapters,
capability checks, and explicit fallback behavior.

### Slice 7: Usage, Cost & Pressure Insights

Expose runtime insights that are useful for operating the agent:

- prompt/input tokens by route
- output tokens by route
- cached-token usage where provider reports it
- compaction count, cache hits, and cache entries
- average context pressure before and after compaction
- tool-call failure classes
- expensive-test counters

This should extend our existing observability instead of becoming a separate
product shell.

### Slice 8: Background Watchdog

Add a non-critical background diagnostic pass that can flag:

- repeated model/tool mismatch
- repeated compaction failures
- context pressure trending upward
- repair traces that keep recurring
- memory pollution candidates
- stale or contradictory model catalog entries

The watchdog should report and propose, not mutate high-trust state without
policy approval.

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

---

## Exit Criteria

Phase 4.11 is useful only if an operator can inspect one failed or degraded turn
and answer:

- which model was selected and why
- which fallback models were available
- what context-window estimate was used and how confident it was
- why a tool was attempted or blocked
- whether compaction was required, skipped, or failed
- whether a short-lived repair trace was stored
- whether a missing capability is a config issue, provider issue, adapter issue,
  or model-profile issue
