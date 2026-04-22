# Phase 4.10/4.11 Memory, Token Economy, and Learning Subsystem Map

Date: 2026-04-14

Scope: map the slices and parent subsystems responsible for memory, careful token
use, and self-learning through learned skills from repeated patterns. This is an
architecture/status note, not a new implementation plan.

## Short Answer

Memory is owned by the 4.8 embedding/retrieval base, the 4.9 learning and memory
quality pipeline, and the 4.10 hardening slices around memory quality, epistemic
state, handoff, watchdog, and trace cleanup. Phase 4.11 does not replace that
memory stack; it plans governance and diagnostics around memory pre-compress
handoff, repair traces, skill activation, capability checks, and watchdog
signals.

Token economy is mainly Phase 4.10 context-engine work: provider-facing context
accounting, history compaction, cheap condensation, progressive scoped context,
context pressure management, structured handoff, and model-profile context-window
resolution. Phase 4.11 adds visibility around those decisions through runtime
decision traces, unified auxiliary model lanes, usage/cost/pressure insights, and
background diagnostics.

Self-learning through new skills is primarily Phase 4.9: typed evidence becomes
learning candidates, candidates become profile facts, precedents, recipes, and
then repeated successful recipes can promote into learned skills. There is also
an older Phase 4.3 `SkillLearner` reflection path for pipeline runs, while 4.11
plans `Skills Governance` so active/candidate/shadowed/blocked skills become
inspectable per agent, channel, category, capability, and route.

## Responsibility Map

### Memory

The parent memory subsystem starts with the Phase 4.8 embedding-first retrieval
and memory backend, then Phase 4.9 adds learning evidence, precedents, recipe
evolution, failure learning, and memory compaction/quality control. Phase 4.10
Slice 16 is the central hardening slice: it moves durable write decisions behind
`memory_quality_governor`, explicit write classes, autosave gates, graph hygiene,
and post-turn orchestration gates.

Phase 4.10 Slice 20 adds typed epistemic state so memory-backed facts are not all
treated as equally trusted; recalled anchors can carry state/source/confidence
instead of becoming undifferentiated prompt text. Slices 19, 21, 22, and 23 add
bounded assumptions, watchdog observations, calibration records, and trace
janitor cleanup so runtime self-observation does not become a second junk memory
system.

Phase 4.11 Slice 4, `Memory Pre-Compress Handoff`, is planned on top of those
gates: before old provider history is dropped, the memory governor should inspect
stable profile facts, successful procedures, repeated failures, unresolved
assumptions, and short-lived repair traces. This is explicitly not meant to
reopen low-quality dialogue-to-memory extraction; the plan says to reuse existing
memory hygiene gates.

### Token Economy

The parent token-economy subsystem is Phase 4.10's context engine and prompt
economy work. Slice 1 gives provider-facing context snapshots and observability,
Slice 4 adds live history compaction, Slice 6 adds a cheap condensation lane,
Slice 7 loads scoped project context progressively, and Slice 13 handles context
pressure and route-switch preflight.

Slice 17 adds structured handoff packets when a full context cannot safely travel
across route, channel, helper-agent, or session boundaries. Slice 18 improves
model-profile and context-window resolution so the runtime does not make blind
budget decisions across native providers, aggregators, and OpenAI-compatible
endpoints.

Phase 4.11 extends this with Slice 1 `Runtime Decision Trace`, Slice 6 `Unified
Auxiliary Model Resolver`, Slice 7 `Usage, Cost & Pressure Insights`, and Slice 8
`Background Watchdog`. These are planned diagnostic/governance layers: they make
token pressure, compaction, auxiliary model selection, and recurring context
failures visible rather than adding more prompt prose.

### Self-Learning And Learned Skills

The parent self-learning subsystem is Phase 4.9. Slices 1 and 2 build typed
learning evidence and learning candidates; Slices 3, 4, and 5 turn accepted
evidence into safer user-profile updates, precedents, and evolving recipes; Slice
6 promotes strong repeated recipes into learned skills with origin/lifecycle
metadata.

Phase 4.9 Slice 7 remembers failure patterns safely, Slice 8 keeps learning
memory maintainable through compaction/quality flows, Slice 9 exposes
human-readable learning surfaces, and Slice 10 gives deterministic eval coverage.
The current 4.9 status says the core learning architecture is largely landed, so
this should be treated as a real subsystem, not just a doc aspiration.

Phase 4.10 Slice 16 protects that learner from bad inputs by blocking
internal-only procedural noise, generic dialogue, malformed consolidation output,
and generic concept graph pollution before durable memory mutation. Phase 4.11
Slice 5 `Skills Governance` is the planned next layer: it should explain why a
skill is active, shadowed, disabled, or blocked by a missing capability for a
particular agent/channel/route.

## Top 10 Important Subsystems

1. **Provider Context Budget And Context Snapshot** — exists, Phase 4.10 Slices
   1 and 13. This is the token-pressure measuring layer: it produces budget
   snapshots, tiers, context artifacts, preflight pressure decisions, and report
   rows. The implementation is code-backed, but real heavy compaction-pressure
   closeout still needs a clean fresh run because retained evidence is stale.

2. **History Compaction And Cheap Condensation** — exists with partial closeout,
   Phase 4.10 Slices 4 and 6. It keeps provider-facing history bounded through
   compaction summaries and a cheap summarizer lane instead of replaying raw
   bootstrap/history on every cycle. Phase 4.11 Slice 4 plans a pre-compress
   memory handoff before old context is dropped.

3. **Progressive Scoped Context Engine** — exists with a live-quality tail, Phase
   4.10 Slice 7. It loads nearest-scope project instructions only when relevant
   and keeps the decision shared between web and channel paths. It is implemented
   as a base, but weaker cheap-route behavior still needs more live validation.

4. **Embedding-First Memory And Recall Backend** — exists, rooted in Phase 4.8
   and consumed by Phase 4.9/4.10. This is the persistent memory substrate behind
   recall, store, profile, embeddings, vector lookup, and SurrealDB-backed memory
   adapters. It is not where every runtime trace should go; later phases add
   gates specifically to keep durable memory clean.

5. **Memory Quality Governor And Epistemic Memory State** — exists as a typed
   base, Phase 4.10 Slices 16 and 20. The governor decides what may become
   durable memory, while epistemic state marks facts as known, inferred, stale,
   contradictory, needs-verification, or unknown. The remaining risk is not a
   missing core, but continued regression/live hardening for concept-heavy
   sessions after real compaction.

6. **Learning Evidence, Candidate, Precedent, And Recipe Pipeline** — exists,
   Phase 4.9 Slices 1 through 5. This is the main self-learning pipeline: runtime
   facts become evidence envelopes, then candidates, then profile updates,
   precedents, recipes, and recipe lineage. It is the subsystem that turns
   repeated useful behavior into reusable procedural knowledge before skill
   promotion.

7. **Skill Promotion And Skills Governance** — promotion exists, governance is
   planned. Phase 4.9 Slice 6 promotes repeated successful recipes into learned
   skill candidates or active learned skills, while older Phase 4.3
   `SkillLearner` can reflect on pipeline runs. Phase 4.11 Slice 5 should add
   the missing operational view: which skills are active, shadowed, disabled, or
   blocked for this agent/channel/capability/model route.

8. **Runtime Assumptions And Structured Session Handoff** — exists as a typed
   base, Phase 4.10 Slices 17 and 19. Assumptions capture bounded runtime
   hypotheses with source/freshness/confidence/invalidation, and handoff packets
   carry active task, commitments, defaults, failures, and cautions across route
   or channel boundaries. Durable promotion from assumptions remains
   intentionally blocked until a separate policy gate exists.

9. **Tool Repair, Runtime Calibration, Watchdog, And Trace Janitor** — exists as
   typed bases, Phase 4.10 Slices 15, 21, 22, and 23. These subsystems remember
   recent repair attempts, compare expected vs actual outcomes, surface degraded
   subsystem alerts, and clean short-lived traces with TTL/dedupe/count bounds.
   Phase 4.11 Slices 1, 3, and 8 plan to make that diagnosis more explainable
   and more systematic, but not permanent memory by default.

10. **Model Profile Registry, Capability Lanes, Capability Doctor, And Auxiliary
    Resolver** — exists partially and expands in 4.11. Phase 4.10 Slices 10, 12,
    18, and 24 provide lane/profile/catalog/adapter-contract groundwork, including
    endpoint-aware context-window metadata and route inspection. Phase 4.11
    Slices 2, 6, and 7 plan the missing operator layer: readiness doctor,
    unified non-primary model lanes, and usage/cost/pressure insights.

## Status Caveat

This top 10 should be read as architecture/status, not full closeout. The Phase
4.10 audit still found provider and adapter test targets that do not compile
cleanly, plus stale heavy-report evidence around compaction pressure, so the
right statement is: most of the relevant subsystems are real and code-backed,
but several are partial or awaiting fresh validation rather than fully
phase-closed.
