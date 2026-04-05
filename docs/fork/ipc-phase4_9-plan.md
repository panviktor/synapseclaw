# Phase 4.9: Self-Learning, Skill Evolution & Memory Quality

Phase 4.8: embedding-first memory & everyday intelligence | **Phase 4.9: self-learning, skill evolution & memory quality** | next: TBD

---

## Problem

Phase 4.8 is making the runtime materially better at:

- retrieval
- typed working state
- session/precedent lookup
- bounded resolution
- inspectable memory projections

That is necessary, but it is not the same thing as real self-learning.

Right now the system is stronger at **finding** relevant context than at
**improving itself** from repeated use.

The biggest remaining gaps are:

1. **memory is better, but learning is still shallow**
   the agent can retrieve more context, but it still does not reliably turn
   repeated evidence into better durable profile/defaults, better recipes, or
   better reusable skills

2. **successful runs do not yet evolve into a strong procedural layer**
   precedent retrieval exists, but skill/recipe evolution is still weaker than
   it should be

3. **post-turn learning is still closer to “candidate write path” than to
   “continuous improvement system”**

4. **we still need a product-grade answer to**
   - “do it like last time”
   - “after restart report here”
   - “my default language/timezone/city”
   - “this workflow keeps working, promote it”
   - “this approach keeps failing, stop repeating it”

5. **memory quality is not the same as memory quantity**
   we need better promotion, merging, compaction, decay, and inspection

The next phase should make SynapseClaw better not only at recall, but at:

- learning stable user defaults
- learning successful procedures
- learning from failures safely
- restructuring memory over time
- exposing that learning clearly to operators

---

## Target

Build a learning system where:

1. **structured evidence becomes typed learning candidates**
2. **embeddings do the heavy lifting for similarity, merge, dedupe, and clustering**
3. **typed stores hold hard facts, not prompt guesses**
4. **successful runs evolve into reusable recipes and skills**
5. **repeated stable evidence upgrades user profile automatically and safely**
6. **memory compacts and reorganizes itself instead of only growing**
7. **operators can inspect what changed and why**

In short:

```text
typed runtime evidence
+ embedding-backed similarity and clustering
+ safe promotion / merge / decay policies
+ inspectable learning projections
= real self-learning instead of just better recall
```

---

## Research Basis

This phase should combine the strongest ideas from the systems we have already
studied, but go further.

### OpenClaw

Best parts worth preserving or surpassing:

- inspectable memory
- understandable session notes / durable memory split
- practical, human-readable product feel

OpenClaw is strong at convenience, but SynapseClaw should surpass it in:

- structured learning contracts
- typed defaults/profile
- procedural memory evolution
- multi-agent-safe memory organization

Sources:

- <https://docs.openclaw.ai/concepts/memory>
- <https://docs.openclaw.ai/context/>
- <https://docs.openclaw.ai/session>
- <https://telegra.ph/Pamyat-OpenClaw-04-03-3>

### Hermes Agent

Best parts worth preserving or surpassing:

- bounded memory surfaces
- session-aware product primitives
- clean high-level tool layer

Hermes is useful as a reminder that product intelligence comes from the right
primitives, not from exposing plumbing.

Sources:

- <https://hermes-agent.nousresearch.com/docs/user-guide/features/tools/>
- <https://hermes-agent.nousresearch.com/docs/user-guide/features/memory/>
- <https://hermes-agent.nousresearch.com/docs/user-guide/sessions/>

### Letta

Best part worth preserving:

- durable always-on identity/context blocks

Source:

- <https://docs.letta.com/guides/core-concepts/memory/memory-blocks/>

### LangGraph

Best part worth preserving:

- explicit split between thread state and long-term memory

Source:

- <https://docs.langchain.com/oss/javascript/langgraph/memory>

### Rasa

Best part worth preserving:

- typed slot/state thinking instead of phrase heuristics

Source:

- <https://rasa.com/docs/reference/primitives/slots/>

### Mem0 / similar learning-first memory systems

Best part worth preserving:

- selective memory formation instead of saving everything

SynapseClaw should beat this by making the promotion logic inspectable,
typed, and compatible with skill/recipe evolution, not only fact capture.

---

## Strategic Position

Phase 4.8 made the system better at **resolving a turn**.

Phase 4.9 should make the system better at **becoming better after many turns**.

We should aim for this position:

- more inspectable than opaque auto-memory systems
- more structured than Markdown-only memory systems
- stronger at procedural learning than OpenClaw
- more universal than static rule/phrase engines
- more local-first than cloud-heavy “agent learning” stacks

This phase must explicitly avoid two bad paths:

1. **black-box learning**
   the system changes, but nobody can understand why

2. **heuristic fake learning**
   the system pretends to learn via brittle special cases and string rules

---

## Design Principles

### 1. Learning starts from typed evidence

The source of truth should be:

- structured tool facts
- structured runtime context
- resolution outcomes
- explicit user corrections
- successful / failed run outcomes
- repeated observations over time

Not:

- regex over assistant text
- keyword lists
- giant phrase tables

### 2. Embeddings do similarity work, not hard-fact arbitration

Embeddings should decide:

- which prior run is similar
- whether two recipe candidates should merge
- whether a new memory entry matches an existing one
- which session recap is most relevant
- which successful workflows cluster together

Embeddings should not decide:

- current delivery target
- timezone
- preferred language
- whether the user explicitly corrected a fact

Those should be typed or inferred through bounded contracts.

### 3. Learning must distinguish memory kinds

We should stop treating “memory” as one flat bucket.

Phase 4.9 should explicitly separate:

- **user profile memory**
- **episodic/session memory**
- **precedent / run memory**
- **recipe / procedural memory**
- **skill memory**
- **negative / failure memory**

Each layer should have different promotion, decay, merge, and retrieval rules.

### 4. Skills and recipes are not the same

We need both:

- **recipes**: concrete successful patterns / precedents / task flows
- **skills**: generalized reusable procedures that survive beyond one case

Recipes should be able to:

- accumulate evidence
- merge with nearby recipes
- split when patterns diverge
- promote into generalized skills

### 5. Learning quality matters more than write volume

The goal is not to store more things.

The goal is to make the agent:

- ask fewer unnecessary clarifications
- reuse proven procedures more often
- stop repeating failed patterns
- keep durable defaults accurate

### 6. Operators must be able to inspect change

Every important learning transition should be explainable:

- why this profile field changed
- why this recipe was promoted
- why these memories merged
- why this skill was deprecated

---

## Desired Architecture

```text
turn / run execution
  -> typed runtime evidence
  -> learning evidence envelope
  -> candidate formation
  -> embedding-backed similarity / clustering / dedupe
  -> policy: promote / merge / update / reject / decay
  -> typed stores + readable projections
  -> eval + operator inspection
```

### Core Learning Loop

```text
runtime evidence
-> candidate memory delta
-> compare against nearby profile / session / recipe / skill items
-> decide update vs append vs merge vs reject
-> persist typed change
-> refresh retrieval docs / projections
-> surface explainable diff
```

---

## Canonical Learning Layers

### Layer 1 — User Profile

Stable defaults and durable preferences:

- preferred language
- timezone
- default city
- communication style
- known environments
- default delivery target

Promotion rule:

- repeated consistent evidence
- or explicit user instruction

Not:

- one random model guess

### Layer 2 — Episodic / Session Memory

What happened in a particular conversation or run:

- compact recap
- resolved entities / references
- important facts introduced
- decisions reached
- short-lived but retrievable history

### Layer 3 — Precedents / Run Memory

Concrete successful or failed task executions:

- task family
- inputs / scope
- tools used
- outcome quality
- operator approvals
- runtime environment

This is the substrate for:

- “do it like last time”
- workflow reuse
- success/failure comparison

### Layer 4 — Recipes

Reusable patterns mined from precedents:

- step structure
- tool sequence
- known good parameters / constraints
- success envelope

Recipes are still concrete and somewhat specific.

### Layer 5 — Skills

Generalized procedures promoted from strong recipes:

- reusable capability
- higher abstraction than one precedent
- explanation + contract + scope

### Layer 6 — Negative Memory

Safe anti-pattern memory:

- repeated failures
- blocked commands
- bad delivery targets
- deprecated workflow variants

This should reduce repeated waste without becoming toxic or overfitted.

---

## Scope

Phase 4.9 should include:

- learning evidence envelopes
- user profile learning
- recipe evolution
- skill promotion/deprecation
- compaction and merge policies
- failure-aware learning
- inspectable change surfaces
- evals for self-learning quality

Phase 4.9 should **not** include:

- a return to phrase-engine routing
- hardcoded lists of user phrasings
- opaque auto-learning with no inspection path
- giant new tool surface exposed to users

---

## Slices

### Slice 1 — Learning Evidence Envelope

Create a canonical typed evidence structure emitted after turns/runs.

It should include:

- tool facts
- retrieval hits used
- resolution source chosen
- clarification outcome
- explicit user correction signals
- run success/failure status
- delivery outcome
- approvals / security blocks

This becomes the input for all learning paths.

### Slice 2 — Candidate Formation Pipeline

Convert evidence into typed candidate deltas:

- profile candidate
- episodic recap candidate
- precedent candidate
- recipe update candidate
- skill update candidate
- failure-memory candidate

No lexical guesswork. Only typed evidence + bounded transforms.

### Slice 3 — User Profile Learning

Upgrade `UserProfile` from operator-managed store to true learning layer.

Required behavior:

- repeated evidence strengthens a candidate
- conflicting evidence lowers confidence
- explicit user instruction wins immediately
- fields can decay or be deprecated when superseded

### Slice 4 — Precedent Learning

Persist richer successful/failed run records as searchable precedents.

Need:

- better task-family grouping
- success/failure metadata
- environment metadata
- delivery context
- approval context

### Slice 5 — Recipe Evolution

Build a recipe evolution engine on top of precedents.

Required behavior:

- cluster similar successful precedents
- merge near-duplicate recipes
- split diverged ones
- attach confidence / support counts
- keep human-readable summaries

### Slice 6 — Skill Promotion & Restructuring

Promote strong recipes into reusable skills.

Required behavior:

- promote when support is high enough
- demote / deprecate stale or failing skills
- preserve lineage: skill <- recipe cluster <- precedents
- track supporting evidence count

### Slice 7 — Failure Learning

Store safe negative learning:

- failed recipes
- blocked commands
- repeated dead ends
- invalid delivery patterns

This should inform resolver and planner behavior without becoming a brittle ban list.

### Slice 8 — Memory Compaction & Quality Control

Add compaction/merge/rewrite flows that improve memory quality over time.

Required behavior:

- merge duplicates
- rewrite noisy episodic entries into compact recaps
- retire low-value stale items
- preserve provenance and inspectability

### Slice 9 — Human-Readable Learning Surfaces

Extend readable projections with learning-specific views:

- learned profile changes
- active recipes
- promoted skills
- recent precedent clusters
- deprecated patterns
- recent failures avoided

This is necessary to beat OpenClaw on inspectability.

### Slice 10 — Self-Learning Eval Harness

Extend evals beyond turn resolution into multi-turn / multi-run improvement.

We need goldens for:

- repeated default capture
- recipe reuse
- skill promotion
- failure avoidance
- clarification reduction over time
- contradictory updates

---

## Embedding-First Requirements

The main learning workload should sit on embeddings:

- precedent similarity
- recipe clustering
- skill promotion candidates
- dedupe shortlist generation
- contradiction shortlist
- merge candidates
- compaction rewrite grouping

But the system must stay compatible with strongly different embedding models.

Therefore Phase 4.9 assumes the `4.8` embedding profile work and must keep:

- profile-aware retrieval calibration
- model/version boundaries
- reindex support
- local-first operation
- external providers as first-class option

We should maintain a **small validated shortlist** of embedding profiles for
learning-critical paths, rather than pretending all models are equivalent.

---

## Safety & Guardrails

Phase 4.9 must keep strong safety boundaries.

Learning must never:

- silently loosen tool security
- learn unsafe commands as “preferred”
- overwrite operator intent without provenance
- leak channel-specific secrets into durable memory
- let one conversation poison another agent’s defaults

Required invariants:

- scoped learning writes
- explainable provenance
- confidence-aware promotion
- explicit contradiction handling
- safe failure memory

---

## Execution Order

Recommended order:

1. Slice 1 — Learning Evidence Envelope
2. Slice 2 — Candidate Formation Pipeline
3. Slice 4 — Precedent Learning
4. Slice 5 — Recipe Evolution
5. Slice 3 — User Profile Learning
6. Slice 6 — Skill Promotion & Restructuring
7. Slice 7 — Failure Learning
8. Slice 8 — Memory Compaction & Quality Control
9. Slice 9 — Human-Readable Learning Surfaces
10. Slice 10 — Self-Learning Eval Harness

This order keeps the foundation principled:

- evidence first
- then precedents
- then recipes
- then skills
- then quality/evals

---

## PR Structure

Suggested PR split:

1. `learning-evidence-envelope`
2. `precedent-candidate-pipeline`
3. `recipe-clustering-and-merge`
4. `user-profile-learning`
5. `skill-promotion-and-lineage`
6. `failure-memory`
7. `memory-compaction-quality`
8. `learning-projections`
9. `self-learning-evals`

---

## Success Criteria

Phase 4.9 is successful when:

1. repeated evidence improves `UserProfile` deterministically
2. successful runs accumulate into reusable recipes
3. strong recipe clusters promote into inspectable skills
4. failed patterns are remembered safely and reduce repeated waste
5. memory quality improves over time through merge/compaction/decay
6. operators can inspect important learning deltas
7. everyday evals show measurable improvement after repeated runs
8. the system gets more useful over time without becoming a heuristic mess

---

## End State

After Phase 4.9, SynapseClaw should no longer be merely:

- a system with memory
- a system with retrieval
- a system with bounded runtime state

It should become:

- a system that **learns durable defaults safely**
- a system that **turns repeated success into reusable capability**
- a system that **remembers failures without becoming brittle**
- a system that **keeps memory quality high instead of just accumulating text**
- a system that is **more inspectable than OpenClaw and more structurally capable than Hermes**

That is the point where “self-learning” starts to be a product reality,
not just an architectural aspiration.
