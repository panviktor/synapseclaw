# Memory Learning Foundation Plan

## Context

SynapseClaw already has a substantial memory system:

- SurrealDB-centered storage
- core blocks, episodic memory, semantic entities/facts, skills, reflections
- background consolidation and prompt optimization
- ongoing prompt-assembly unification in [`memory-unification-plan.md`](memory-unification-plan.md)

But before a stronger UI can honestly present "the agent learns", the backend needs a tighter learning contract. Right now the system stores and recalls memory, but several important behaviors are still either too coarse or too implicit:

- long-term memory mostly grows by append-style writes
- forgetting/retention is still relatively primitive
- explicit user corrections are not clearly separated from background learning
- shared memory across agents needs stronger ownership/scoping semantics
- UI-facing learning events are not yet a stable first-class contract

This plan defines the **pre-UI backend layer** for memory learning and self-improvement. It is the practical subset worth taking from the research material in `/tmp/compass_artifact.md`, adapted for SynapseClaw’s current hexagonal Rust architecture and multi-agent runtime.

---

## Goals

1. Reduce noisy append-only memory growth with explicit mutation decisions.
2. Make retention/forgetting more deterministic and cheaper than LLM-heavy memory management.
3. Separate **hot-path explicit memory capture** from **background learning**.
4. Prevent cross-agent memory collisions by default through scoped namespaces/ownership.
5. Emit a stable learning/event contract that later powers unified prompt assembly and UI.

## Non-goals

- Reproducing Mem0, Graphiti, or MemGPT wholesale
- Per-fact multi-call LLM loops on every turn
- Full graph-heavy memory extraction on the hot path
- LLM-first conflict arbitration as the default write policy
- Frontend implementation details

---

## Selected Imports From Research

From [`/tmp/compass_artifact.md`](/tmp/compass_artifact.md), the practical ideas worth adopting are:

- **AUDN-style mutation semantics**: `Add / Update / Delete / Noop`
- **Retention scoring**: `relevance + recency + importance + frequency`
- **Hot-path vs background learning split**
- **Deterministic memory pressure/budget discipline**
- **Scoped/shared namespace model for multi-agent memory**

What we intentionally do **not** import as-is:

- full per-fact Mem0 pipelines with many LLM calls
- full MemGPT OS-style memory orchestration
- full Graphiti-scale graph extraction on every turn
- default LLM arbitration for memory conflicts

---

## Architectural Position

This plan sits **before UI** and **alongside memory unification**.

Order of concern:

1. **Memory Learning Foundation**
   Defines what it means for memory to change, decay, merge, and emit learning events.
2. **Memory Unification**
   Defines how those memory layers are assembled into prompt context across web/channels.
3. **UI / Workbench**
   Visualizes learning, memory usage, budgets, and multi-agent behavior.

Intended reading/execution sequence:

`memory-learning-foundation-plan.md` → `memory-unification-plan.md` → `multi-agent-memory-ui-plan.md`

---

## Design Principles

### 1. Deterministic wrappers around LLM judgment

LLMs may propose learning actions, but they should do so into a narrow schema. Storage mutation, ownership, and fallback behavior remain deterministic code.

### 2. Cheap by default

The default path should be:

- one consolidation-style extraction step
- one optional reflection step
- zero or one mutation-decision step for long-term memory updates

not N LLM calls for N candidate facts.

### 3. Explicit beats inferred

If the user says:

- "remember this"
- "that's wrong"
- "I prefer X"
- "use Y from now on"

the system should treat that as a high-confidence hot-path learning signal.

### 4. Ownership first, sharing second

Each agent should own what it writes by default. Shared memory is explicit and policy-driven.

### 5. UI should consume contracts, not storage trivia

Frontend surfaces must not guess what happened by diffing raw rows. The learning layer should emit canonical events and read-models.

---

## Phase 1 — Memory Mutation Semantics (`AUDN-lite`)

### Problem

Current long-term memory updates are too append-oriented. This makes it hard to:

- avoid duplicates
- correct outdated preferences/facts
- explain why memory changed

### Goal

Introduce a narrow mutation contract inspired by AUDN:

- `Add`
- `Update`
- `Delete`
- `Noop`

### New domain types

**New file**: `crates/domain/src/domain/memory_mutation.rs`

```rust
pub enum MemoryMutationAction {
    Add,
    Update { target_id: String },
    Delete { target_id: String },
    Noop,
}

pub struct MemoryMutationCandidate {
    pub category: MemoryCategory,
    pub text: String,
    pub confidence: f32,
    pub source_turn_id: Option<String>,
}

pub struct MemoryMutationDecision {
    pub action: MemoryMutationAction,
    pub candidate: MemoryMutationCandidate,
    pub reason: String,
}
```

### New application service

**New file**: `crates/domain/src/application/services/memory_mutation.rs`

Responsibilities:

- accept extracted candidates from consolidation or explicit user signal
- fetch a small shortlist of similar existing memories
- ask a constrained LLM or deterministic matcher for `Add / Update / Delete / Noop`
- apply the mutation through existing memory ports
- emit canonical events

### Constraints

- one turn may produce multiple candidates
- each candidate is matched against a **shortlist**, not the whole store
- fallback must exist if the model returns malformed JSON
- `Noop` must be cheap and common

---

## Phase 2 — Retention, Forgetting, and Memory Pressure

### Problem

Simple decay/GC is not enough for a learning system that wants to:

- remember durable user preferences
- forget stale conversation noise
- keep recall relevant under token limits

### Goal

Add deterministic retention scoring using four signals:

- `relevance`
- `recency`
- `importance`
- `frequency`

### New application service

**New file**: `crates/domain/src/application/services/retention.rs`

```rust
pub struct RetentionScore {
    pub relevance: f64,
    pub recency: f64,
    pub importance: f64,
    pub frequency: f64,
    pub total: f64,
}

pub struct RetentionPolicy {
    pub episodic_half_life_hours: f64,
    pub daily_half_life_hours: f64,
    pub reflection_half_life_hours: f64,
    pub min_keep_score: f64,
}
```

### Usage

- rank episodic memories before recall injection
- guide low-importance GC
- decide what to compact first under memory pressure
- surface a stable `importance`/`heat` signal to the future UI

### Recommended policy direction

- episodic chat memory decays fastest
- daily summaries decay slower
- explicit user preferences and procedural skills decay slowest

### Memory pressure

This phase also defines **what gets dropped or compressed first** when prompt/context pressure rises:

1. low-score episodic recall
2. older episodic recall
3. long-tail entities
4. long-tail skills
5. core blocks never auto-evicted

This complements, but does not replace, the separate prompt-budget work in [`memory-unification-plan.md`](memory-unification-plan.md).

---

## Phase 3 — Hot-Path Explicit Capture vs Background Learning

### Problem

Not all learning signals are equal.

- Some should affect memory immediately.
- Some should be learned later, in background.

### Goal

Split learning into two distinct paths.

### 3a. Hot-path explicit capture

**New file**: `crates/domain/src/application/services/learning_signals.rs`

Detect explicit high-signal cases:

- direct memory commands
- corrections
- stated preferences
- stable identity facts
- operator instructions like "from now on"

Output:

```rust
pub enum LearningSignal {
    ExplicitPreference,
    ExplicitCorrection,
    ExplicitInstruction,
    BackgroundOnly,
}
```

Behavior:

- explicit signals can bypass heavier reflection logic
- they go straight into mutation evaluation with higher confidence

### 3b. Background learning

Keep existing consolidation/reflection style learning for:

- session summaries
- extracted entities/facts
- procedural lessons
- optimization candidates

But move the policy of **when** to do which into stable domain/application services rather than duplicating gates across callsites.

---

## Phase 4 — Multi-Agent Namespaces and Shared Memory Policy

### Problem

In a multi-agent runtime, "shared memory" can become accidental overwrite unless ownership is explicit.

### Goal

Use a namespace-oriented model:

- agent-private writes
- controlled shared reads
- explicit promotion into shared memory

### Implementation (delivered)

Implemented via the existing `Visibility` enum (`Private | SharedWith(Vec<AgentId>) | Global`)
and `memory_sharing` service in `crates/domain/src/application/services/memory_sharing.rs`:

- **`Visibility`** enum replaces the planned `MemoryNamespace` — same semantics, reuses existing domain type
- **`validate_promotion()`** — owner-only, no demotion, SharedWith can widen
- **`resolve_conflict()`** — authority > recency > confidence (deterministic, no LLM)
- **`promote_visibility()`** port method on `UnifiedMemoryPort`
- Schema: `shared_with` + `visibility` fields on all memory tables

### Policy

- default writes go to `Private` (agent-scoped)
- promotion to `SharedWith` / `Global` is explicit (owner-only)
- cross-agent overwrite is not allowed by default
- same-fact conflicts resolve deterministically by:
  - authority
  - recency
  - confidence
- LLM arbitration is reserved for exceptional/manual flows, not default storage behavior

---

## Phase 5 — Stable Events and Read Models

### Problem

The future UI cannot be built on top of raw row-diffs.

### Goal

Emit stable learning events and expose stable read-models.

### Canonical events

Potential event set:

- `memory.candidate_extracted`
- `memory.mutation_decided`
- `memory.mutation_applied`
- `memory.retention_scored`
- `memory.skill_created`
- `memory.skill_updated`
- `memory.reflection_stored`
- `memory.core_blocks_updated`
- `memory.prompt_optimization_applied`

These events should be emitted from the application layer, not inferred in the frontend.

### Read-models

Potential gateway-facing aggregates:

- `MemoryOverview`
- `TurnLearningReport`
- `AgentLearningStats`
- `SkillSummary`
- `OptimizationHistory`

These are the contracts consumed later by:

- [`memory-unification-plan.md`](memory-unification-plan.md)
- [`multi-agent-memory-ui-plan.md`](multi-agent-memory-ui-plan.md)

---

## Integration With Existing Plans

## Relationship to Memory Unification

[`memory-unification-plan.md`](memory-unification-plan.md) remains the plan for:

- prompt assembly
- shared turn-context formatting
- session scoping
- unified post-turn execution paths

This foundation plan supplies the semantics that unification should eventually consume:

- mutation decisions
- retention policy
- learning signal classification
- canonical learning events

## Relationship to UI

[`multi-agent-memory-ui-plan.md`](multi-agent-memory-ui-plan.md) should not ship on assumptions alone. It should visualize:

- learning decisions from this plan
- prompt-context contracts from the unification plan

That is why this plan comes first.

---

## Delivery Plan

## Phase A — Mutation core

Files:

- `crates/domain/src/domain/memory_mutation.rs`
- `crates/domain/src/application/services/memory_mutation.rs`
- adapter-side decision/extractor helpers

Deliverables:

- `AUDN-lite` decision model
- constrained/fallback JSON handling
- mutation application service

## Phase B — Retention core

Files:

- `crates/domain/src/application/services/retention.rs`
- memory adapter ranking/GC integration

Deliverables:

- retention score model
- category-aware decay profiles
- better recall ordering and GC policy

## Phase C — Learning signal split

Files:

- `crates/domain/src/application/services/learning_signals.rs`
- post-turn orchestration callsites

Deliverables:

- explicit hot-path capture
- background-only classification
- cheaper and clearer learning paths

## Phase D — Namespace policy

Files:

- `crates/domain/src/domain/memory_namespace.rs`
- memory ports/adapters as needed
- gateway read models

Deliverables:

- private/shared namespace model
- deterministic conflict handling
- clearer multi-agent ownership semantics

## Phase E — Events and read-models

Files:

- application services
- gateway event surfaces
- read-model structs/endpoints

Deliverables:

- stable event contract
- UI-safe learning summaries

---

## Critical Files

| File | Purpose |
|------|---------|
| `crates/domain/src/domain/memory.rs` | existing memory types and category semantics |
| `crates/domain/src/ports/memory.rs` | memory port extension points |
| `crates/domain/src/application/services/memory_service.rs` | current recall/retention-adjacent policies |
| `crates/adapters/core/src/memory_adapters/consolidation.rs` | current extracted memory updates |
| `crates/adapters/core/src/memory_adapters/skill_learner.rs` | reflection → skill pipeline |
| `crates/adapters/core/src/memory_adapters/prompt_optimizer.rs` | self-improvement target surface |
| `crates/adapters/memory/src/surrealdb_adapter.rs` | ranking, GC, ownership/scoping, mutation persistence |
| `crates/adapters/core/src/gateway/ws.rs` | future event emission surface for web chat |
| `crates/domain/src/application/use_cases/handle_inbound_message.rs` | future event emission surface for channels |

---

## Verification

1. `cargo test -q -p synapse_domain --lib` passes.
2. Long-term memory updates can produce `Add / Update / Delete / Noop`, not only append.
3. Explicit corrections/preferences are captured without requiring full reflection flow.
4. Recall ranking improves under noisy histories.
5. Shared memory no longer implies accidental cross-agent overwrite.
6. Gateway/application layer exposes stable learning events suitable for UI.

---

## Success Criteria

This plan is successful when:

- memory growth becomes more selective
- learning decisions are explainable
- self-improvement is visible as a contract, not a side effect
- multi-agent memory ownership is clear
- the UI can later say "the agent learned X" without guessing

That is the backend foundation the UI should stand on.
