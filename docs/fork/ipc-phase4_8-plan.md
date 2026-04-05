# Phase 4.8: Embedding-First Memory & Everyday Intelligence

Phase 4.7: deterministic user context & task resolution | **Phase 4.8: embedding-first memory & everyday intelligence** | next: Phase 4.9 self-learning, skill evolution & memory quality

---

## Problem

SynapseClaw now has stronger runtime primitives than before:

- unified prompt assembly
- current-conversation targets
- standing orders
- session search
- typed user profile groundwork
- structured learning/memory foundation

But everyday assistant behavior is still not where it needs to be.

The system still risks falling into two bad modes:

1. **prompt-only intelligence**
   context exists, but the runtime does not deterministically resolve it
2. **phrase-engine intelligence**
   product behavior is recreated with brittle lexical rules that do not scale

That second path is unacceptable. We cannot hardcode dozens of languages,
slangs, task phrasings, and edge cases. It will regress, fragment across web
and channels, and ultimately feel worse than simpler systems.

The next step must make SynapseClaw:

- **more convenient than OpenClaw**
- **more universal than Hermes**
- **more structured than prompt-only systems**
- **more local-first than cloud-first stacks**

without turning product logic into a giant bag of heuristics.

---

## Target

Build a memory and resolution system where:

1. **embeddings do the bulk of retrieval work**
2. **typed runtime state handles hard facts and hard defaults**
3. **structured interpretation is narrow and bounded**
4. **human-readable projections remain inspectable**
5. **web and channels share the same resolver stack**

In short:

```text
embedding-first retrieval
+ typed runtime state
+ bounded structured interpretation
+ human-readable memory projections
= everyday intelligence without phrase-engine hacks
```

---

## Research Basis

This phase should explicitly combine the best ideas from current top systems.

### OpenClaw

Best parts worth preserving or surpassing:

- extremely inspectable memory
- session-first model
- clear split between durable memory and daily/session notes
- hybrid search as a practical default
- product feel: memory is understandable by humans

Sources:

- <https://docs.openclaw.ai/concepts/memory>
- <https://docs.openclaw.ai/session>
- <https://docs.openclaw.ai/context/>
- <https://telegra.ph/Pamyat-OpenClaw-04-03-3>

### Hermes Agent

Best parts worth preserving or surpassing:

- bounded curated persistent memory
- explicit high-level tools
- session-aware search
- skills/task scaffolding without exposing plumbing to the user

Sources:

- <https://hermes-agent.nousresearch.com/docs/user-guide/features/overview>
- <https://hermes-agent.nousresearch.com/docs/user-guide/features/tools/>
- <https://hermes-agent.nousresearch.com/docs/user-guide/features/memory/>
- <https://hermes-agent.nousresearch.com/docs/user-guide/sessions/>

### Letta

Best part worth preserving:

- always-on curated memory blocks for durable identity/context

Source:

- <https://docs.letta.com/guides/core-concepts/memory/memory-blocks/>

### LangGraph

Best part worth preserving:

- clean distinction between thread state and long-term memory

Source:

- <https://docs.langchain.com/oss/javascript/langgraph/memory>

### Rasa

Best part worth preserving:

- slot/state thinking for dialogue resolution

Source:

- <https://rasa.com/docs/reference/primitives/slots/>

---

## Strategic Position

We should **not** copy any one of these systems.

Instead:

- take OpenClaw's inspectability
- take Hermes' product primitives
- take Letta's durable always-on context
- take LangGraph's thread-vs-long-term split
- take Rasa's slot/state discipline
- and put the core retrieval load on **embedding models**, preferably local-first

That combination can beat each individual system:

- easier to inspect than pure vector-memory systems
- more structured than Markdown-only systems
- more local-first than cloud-heavy stacks
- more universal than phrase/rule engines

---

## Design Principles

### 1. Embeddings do retrieval, not business logic

Embeddings should answer:

- what prior session is relevant?
- what note or precedent is closest?
- what prior successful run resembles this request?
- what memory entry matches this paraphrase?

Embeddings should **not** decide:

- whether the user's timezone is UTC or Europe/Berlin
- what the current conversation target is
- whether we are replying in the same chat

Those are typed runtime facts.

### 2. Typed state beats free-text inference for hard facts

The runtime should not ask the model to guess:

- preferred language
- timezone
- default city
- current delivery target
- focus item in a comparison set

These must exist in typed stores or typed session state.

### 3. Structured interpretation must be narrow

We still need per-turn interpretation, but it must be bounded:

- references
- temporal scope
- delivery scope
- whether defaults are requested
- whether clarification is needed

No giant phrase tables.

### 4. Human-readable projections are product features

One reason OpenClaw feels sane is that its memory is visible.

SynapseClaw should keep structured stores as source of truth, but project them
into human-readable surfaces:

- durable profile view
- daily notes / session recap view
- recent precedent / recipe view

### 5. Local-first by default

The main work should run on:

- local embedding models where possible
- cheap bounded local classifiers/parsers where needed
- deterministic code for routing

Cloud LLMs remain useful, but should not be the only thing making the system
look intelligent.

---

## Desired Architecture

```text
Inbound turn
  -> typed runtime facts
  -> bounded turn interpretation
  -> resolution router
  -> embedding-backed retrieval
  -> deterministic resolver choice
  -> prompt assembly
  -> response
  -> post-turn typed updates + memory projections
```

### Canonical resolution ladder

```text
explicit user input
-> current conversation context
-> dialogue state / working state
-> structured user profile
-> session / precedent / recipe search
-> long-term semantic memory
-> narrow clarification
```

This should live in application services, not in prompts and not in channel hacks.

---

## Memory Model We Should Ship

### Layer 1 — Typed Runtime Facts

Hard runtime truth:

- current conversation target
- thread target
- actor identity key
- structured user profile
- active standing orders

### Layer 2 — Working State

Session-scoped, short-lived:

- focus entities
- comparison set
- named slots
- last tool subjects
- unresolved ambiguity

### Layer 3 — Session / Daily Memory

Human-readable recent memory:

- daily notes
- session recaps
- compact transcript projections

### Layer 4 — Long-Term Durable Memory

- curated durable user/project memory
- semantic entities/facts
- skills
- recipes / precedents

### Layer 5 — Retrieval Index

Hybrid index across the above layers:

- BM25 / FTS for exactness
- embeddings for semantic recall
- temporal decay
- MMR / diversity
- optional source weighting

This is the main “intelligence backbone”.

---

## Phase Slices

## Slice 1 — Freeze and Remove Legacy Heuristic Paths

### Goal

Stop further growth of phrase-based product logic.

### Work

- remove remaining runtime reliance on lexical intent classifiers
- remove side-question phrase tables from live routing
- remove run-recipe family inference based on phrase matching
- keep only typed stores and neutral transport/runtime behavior

### Acceptance criteria

1. No live routing depends on big lists of hardcoded phrases.
2. No channel/web divergence depends on phrase-specific rules.
3. Residual heuristics are documented and isolated behind compatibility shims only if absolutely necessary.

---

## Slice 2 — Local-First Embedding Backbone

### Goal

Make embeddings the default retrieval engine across session search, precedent lookup,
memory recall, and historical recap.

### Work

- define one canonical hybrid retrieval service
- use one embedding provider contract across:
  - episodic recall
  - session search
  - precedent search
  - recipe search
- prefer local embeddings by default:
  - `bge-small`
  - `e5-small`
  - `embeddinggemma`
  - or equivalent local-compatible provider
- keep cloud providers as optional upgrades, not the default requirement
- do not optimize for the full embedding catalog exposed by providers such as
  OpenRouter:
  - maintain a small validated shortlist of general-purpose embedding profiles
    for SynapseClaw's main assistant workloads
  - expect many other models to be niche, legacy, multilingual-specialized,
    code-specialized, multimodal-specialized, or otherwise unvalidated for our
    default path
  - treat non-shortlisted models as experimental until they pass the eval
    harness for recall, session search, precedent lookup, and multilingual
    everyday assistant tasks
- add an `EmbeddingProfile` / calibration layer so retrieval does not assume all
  embedding models behave the same:
  - `dimensions`
  - `distance_metric`
  - `normalize_output`
  - `query_prefix`
  - `document_prefix`
  - `supports_multilingual`
  - `supports_code`
  - `recommended_chunk_chars`
  - `recommended_top_k`
  - `provider_family`
- version stored vectors by embedding profile / model id and define reindex rules
  when the embedding model changes
- add source weighting:
  - profile / durable memory
  - session recaps
  - transcripts
  - recipes
  - skills
- add temporal decay and MMR as first-class query options

### Why this beats OpenClaw/Hermes

- keeps OpenClaw's hybrid practicality
- makes retrieval broader than Markdown-only files
- stays local-first instead of cloud-first
- stays provider-agnostic instead of overfitting retrieval to one embedding family

### Acceptance criteria

1. `session_search` uses the shared hybrid retrieval service.
2. precedent search and recipe lookup use the same backbone.
3. retrieval quality improves on paraphrases and cross-lingual phrasing without phrase tables.
4. local embeddings are production-grade, not just a fallback demo mode.
5. changing the embedding model does not silently corrupt retrieval quality; the
   runtime knows how to calibrate or reindex for that profile.

---

## Slice 3 — Structured User Profile as Real Runtime Data

### Goal

Finish the move from soft `user_knowledge` text to a typed user profile.

### Work

- persist `UserProfile` as first-class runtime data
- add controlled sync into human-readable projections/core blocks
- define capture/update paths:
  - explicit user corrections/preferences
  - operator edits
  - future structured parsers
- add explicit fields:
  - `preferred_language`
  - `timezone`
  - `default_city`
  - `communication_style`
  - `known_environments`
  - `default_delivery_target`

### Constraint

No “parse arbitrary free text into profile” heuristic explosion.
Capture must be:

- explicit tool/update path
- bounded structured parser
- operator input

### Acceptance criteria

1. profile is no longer just prompt-visible; it is runtime-resolved.
2. default language/timezone/city resolution does not depend on free-text memory matching.
3. profile can be projected into human-readable memory views without becoming the source of truth.

---

## Slice 4 — Dialogue State From Typed Updates

### Goal

Turn `DialogueState` into a real working-state layer, not a regex engine.

### Work

- update working state from typed events:
  - tool subjects/results
  - explicit structured turn interpretation
  - known current targets
- keep:
  - focus entities
  - comparison set
  - slots
  - last tool subjects
- stop auto-extracting cities/services/timezones from raw strings via ad-hoc rules

### Acceptance criteria

1. dialogue state updates are event-driven and structured.
2. no weather/city/service regex path remains on the hot path.
3. comparison follow-ups resolve from working state, not from lucky semantic recall.

---

## Slice 5 — Bounded Turn Interpretation

### Goal

Introduce a narrow structured per-turn interpretation layer without recreating a phrase-engine.

### Work

Add a typed interpretation schema such as:

```rust
pub struct TurnInterpretation {
    pub references: Vec<ReferenceCandidate>,
    pub delivery_scope: Option<DeliveryScope>,
    pub temporal_scope: Option<TemporalScope>,
    pub defaults_requested: Vec<DefaultKind>,
    pub clarification_candidates: Vec<String>,
}
```

Interpretation should be produced by one of:

- deterministic transport/runtime facts
- small bounded parser
- future small local instruct model

not by a giant bag of string rules.

### Acceptance criteria

1. interpretation output is typed and bounded.
2. interpretation is shared by web and channels.
3. interpretation can be evaluated independently of the chat model.

---

## Slice 6 — Unified Resolution Router

### Goal

Make subsystem choice deterministic.

### Work

Add an application-level router that decides:

- resolve from current conversation?
- resolve from dialogue state?
- resolve from user profile?
- run session/precedent search?
- use long-term memory?
- ask clarification?

This router should consume:

- typed runtime facts
- turn interpretation
- retrieval results
- confidence/coverage signals

### Acceptance criteria

1. same request class routes the same way in web and channels.
2. historical questions use historical resolvers first.
3. repeat-work questions use precedent/recipe resolvers first.
4. clarification happens only after resolver exhaustion or low confidence.

---

## Slice 7 — Past Work, Precedents, and Recipes

### Goal

Make “what did we do?” and “do it like last time” first-class behaviors.

### Work

- replace phrase-based `task_family` inference with embedding-backed precedent search
- index:
  - prior successful runs
  - compact run summaries
  - tool sequences
  - approval/constraint notes
- distinguish:
  - session recap
  - prior precedent
  - reusable recipe
  - skill

### Important principle

Recipes should not be guessed from words like `deploy` or `restart`.
They should be found by similarity over prior successful work and task context.

### Acceptance criteria

1. “do it like last time” can resolve against prior successful precedent without phrase matching.
2. the runtime can explain whether it used session recap, recipe, or skill.
3. recipe memory complements skill memory instead of duplicating it badly.

---

## Slice 8 — Human-Readable Memory Projections

### Goal

Be at least as understandable as OpenClaw while remaining more structured underneath.

### Work

Project structured stores into readable views:

- `USER_PROFILE.md` or equivalent readable profile projection
- daily memory / session recap documents
- precedent/recipe summaries
- inspectable session recap snippets

Important:

- these are **projections**, not the only source of truth
- they must be cheap to regenerate
- they should align with the web workbench

### Acceptance criteria

1. an operator can inspect what the system “knows” without querying raw DB rows.
2. durable profile and recent working memory are understandable to humans.
3. inspectability matches or beats OpenClaw’s Markdown ergonomics.

---

## Slice 9 — Clarification Policy

### Goal

Make clarification narrow, justified, and rare.

### Work

- clarify only after resolver stack fails
- ask bounded disambiguation when candidate set exists
- include known default as an option when appropriate
- log why clarification was necessary

### Acceptance criteria

1. no generic “which city/language/timezone?” when defaults exist.
2. clarification names the candidate set when possible.
3. unnecessary clarification rate drops measurably.

---

## Slice 10 — Everyday Intelligence Eval Harness

### Goal

Turn “feels smarter” into measurable regression tests.

### Golden scenarios

- “What’s the weather?”
- “Translate to my language”
- “Remind me tomorrow”
- “Send it to our chat”
- “What did we discuss last week?”
- “Do it like last time”
- “The second one”
- “Restart that service”
- “Is it still failing?”

### For each scenario, record

- selected resolver
- whether defaults were used
- whether session/precedent search was used
- whether clarification happened
- whether clarification was narrow or generic

### Comparative target

We should beat:

- OpenClaw on structured/default-driven resolution
- Hermes on typed runtime determinism
- prompt-only systems on repeatability

### Acceptance criteria

1. everyday regressions are visible in CI/dev validation.
2. we can compare local embedding configs vs cloud configs.
3. resolver quality is measurable separately from the chat model.

---

## Storage & Compute Strategy

### Main work should be on embeddings

This phase should prefer:

- local embedding model for indexing/querying
- deterministic code for routing
- bounded parser for typed interpretation

It should avoid:

- pushing all intelligence into a large chat model
- making cloud LLM calls the only reason the system behaves well
- solving retrieval by prompt stuffing

### Recommended baseline

- local embedding provider enabled by default
- hybrid retrieval on by default
- structured profile + working state always available
- cloud LLM optional for richer summarization or repair paths

---

## Architecture Fit

### Domain / application

Add or strengthen:

- `TurnInterpretation`
- `ResolutionRouter`
- `ReferenceResolver`
- `UserProfileResolver`
- `PrecedentResolver`
- `SessionRecapResolver`
- `ClarificationPolicy`
- `EverydayEvalHarness`

### Ports

Add or strengthen:

- `UserProfileStorePort`
- `SessionRecapStorePort`
- `PrecedentStorePort`
- shared retrieval/query port

### Adapters

Implement:

- local embedding provider defaults
- shared hybrid retrieval adapter
- profile/recap/projection stores
- indexed precedent search

---

## Non-goals

- adding another giant phrase classifier
- creating per-channel product behavior hacks
- replacing embeddings with only symbolic rules
- replacing typed state with free-text memory blocks
- making cloud LLMs mandatory for core assistant competence

---

## Execution Order

Recommended order:

1. freeze legacy heuristics
2. local-first embedding backbone
3. structured user profile
4. typed dialogue state updates
5. bounded turn interpretation
6. unified resolution router
7. precedent/recipe retrieval
8. human-readable projections
9. clarification policy
10. eval harness

This order front-loads the parts that most improve everyday competence.

---

## PR Structure

Suggested breakdown:

1. `phase4_8a`: heuristic freeze + runtime cleanup
2. `phase4_8b`: shared local-first hybrid retrieval service
3. `phase4_8c`: user profile as first-class runtime data
4. `phase4_8d`: typed dialogue state updates
5. `phase4_8e`: bounded turn interpretation
6. `phase4_8f`: resolution router
7. `phase4_8g`: precedent/recipe retrieval
8. `phase4_8h`: readable projections
9. `phase4_8i`: clarification policy
10. `phase4_8j`: eval harness

---

## Success Criteria

This phase is successful when:

1. SynapseClaw stops feeling like a system that merely has memory and starts feeling like a system that resolves context correctly.
2. OpenClaw remains easier to inspect in spirit, but SynapseClaw matches that inspectability while surpassing it in structure and universality.
3. Hermes remains strong on product tools, but SynapseClaw surpasses it on deterministic typed resolution across channels and web.
4. Most everyday assistant competence comes from embeddings + typed state + routing, not from giant prompt heuristics.
5. Local-first configurations are good enough that the system still feels smart without depending on cloud-only magic.

---

## Expected Outcome

After Phase 4.8, SynapseClaw should feel like:

- OpenClaw in inspectability
- Hermes in product fluency
- Letta in durable context
- LangGraph/Rasa in state discipline

but stronger than all of them in one combined property:

**embedding-first, typed, local-first everyday intelligence**

without falling back into brittle heuristic code.
