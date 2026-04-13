# Phase 4.10 Test Matrix

Purpose: keep the 4.10 closeout validation explicit before adding more code.
This is not a second implementation plan. It maps each slice to concrete
checks, preferring unit/domain tests first, then adapter integration tests, then
live provider runs only where provider behavior matters.

## Execution Gates

1. Fast local gate:
   - `cargo fmt --all --check`
   - `cargo check -q -p synapse_domain -p synapse_adapters -p synapseclaw --features channel-matrix`
2. Targeted 4.10 unit/integration gate:
   - run narrow domain/adapter tests for context pressure, route admission,
     tool narrowing, memory hygiene, self-repair, model profiles, and
     web/channel parity
3. Base live gate:
   - `RUN_REASONER=1 RUN_DOCTOR_MODELS=1 STRICT_RECALL_NO_MUTATION=1 dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh`
   - covers `cheap`, `deepseek`, `deepseek-reasoner`, `gpt-5.4`, CJK,
     structured media admission, live model switching, embedding/admission
     signals, and doctor models
4. Heavy live gate:
   - run only after the fast, targeted, and base live gates have no failures
   - `RUN_HEAVY=1 RUN_CONTEXT_OVERFLOW_SWITCH=1 dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh`
   - validates long-dialogue retention, context size, embedding signals, and
     no procedural-skill pollution
   - the overflow switch subcase validates large-window to small-window
     preflight against a small OpenRouter media route and must block or compact
     before applying the new route

Current live blocker:

- `2026-04-12` base pack found `deepseek_memory` recall emitting
  `core_memory_update` under strict no-mutation mode. This is a shared
  read-only recall/tool-narrowing policy bug to fix before any heavy run.
  Report: `/tmp/synapseclaw-phase410-base-1776030344`.

## Slice Coverage

### Slice 1: Context Snapshot

- Unit: provider-context budget snapshot includes stable/dynamic, protected/removable,
  primary ballast, headroom, and pressure tier.
- Live: provider-context TSV is produced for every live pack and has at least
  one row per provider call.

### Slice 2: Typed Defaults

- Unit: arbitrary user-profile facts stay dynamic key/value data; no fixed city,
  timezone, or response-style schema is required.
- Unit: typed delivery/default resolution uses runtime/profile state instead of
  prompt prose.

### Slice 3: Non-Mutating Structured Recall

- Unit: resolved-state recall exposes no memory/profile mutation tools.
- Unit: profile-fact resolution may keep external lookup only when the turn
  needs an external fact, but must not expose profile mutation by default.
- Live: `STRICT_RECALL_NO_MUTATION=1` must pass for `cheap`, `deepseek`,
  `deepseek-reasoner`, and `gpt-5.4`.

### Slice 4: Live History Compaction

- Unit: compaction preserves protected head/tail and does not split assistant
  tool-call / tool-result groups.
- Live: heavy long-dialogue run records context rows and either a compaction
  signal or a clear within-budget reason.

### Slice 5: Deterministic Runtime Execution

- Unit/integration: typed mutation and delivery flows prefer runtime execution
  paths where available instead of relying on cheap-model prompt compliance.
- Live: tool smoke creates the requested file exactly once and records typed
  tool outcomes.

### Slice 6: Cheap Condensation

- Unit: summary lane resolution is capability-lane based and does not consume
  legacy route aliases.
- Unit: compaction cache reuses unchanged source/policy/window digests.
- Live: long-dialogue run checks that procedural skill counts do not grow from
  pure philosophy dialogue.

### Slice 7: Progressive Scoped Context

- Unit: scoped-context selection suppresses stale inferred context on structured
  media turns.
- Unit: explicit non-media path hints can still load bounded scoped context.
- Live: scoped context should appear only when relevant and should stay bounded.

### Slice 8: Provider-Native Continuation

- Unit/adapter: continuation request shaping is adapter-local and capability-gated.
- Live: only run official/key-based Responses continuation when an endpoint
  actually accepts `previous_response_id`; the default Codex backend remains
  expected-not-supported.

### Slice 9: Strict Tool Protocol

- Unit: shared runtime accepts native structured tool calls only.
- Unit: no shared XML/text/GLM/Minimax/perl-style dialect fallback exists.
- Adapter: provider-specific normalization must live only in concrete adapters.

### Slice 10: Capability Lanes

- Unit: lane candidates resolve in ordered provider:model candidates with
  candidate-scoped profile metadata.
- Unit: catalog aliases and presets do not create a second route table.
- Unit/live: optional Gemma/Grok/DeepSeek routes remain catalog-driven and not
  Rust hardcoded defaults.
- Unit/live: OpenRouter MiniMax M2.7 is catalog-driven as a text/tool route; do
  not mark it as audio/video/music-capable unless provider metadata exposes
  those output modalities.
- Unit: OpenRouter media lanes use actual media-capable profiles separately:
  image (`google/gemini-3.1-flash-image-preview`), audio
  (`openai/gpt-audio-mini`), music (`google/lyria-3-*`), video
  (`google/veo-3.1`, `alibaba/wan-2.6`).

### Slice 11: Turn Admission

- Unit: media, tool-heavy, mutation, delivery, and overflow turns produce typed
  admission intent/action/reasons before provider call.
- Live: text-only routes block structured media-generation markers before the
  provider call.

### Slice 12: Model Profile Registry

- Unit: manual profile overrides win over local catalog, cached provider catalog,
  bundled catalog, then adapter fallback.
- Unit: endpoint-aware cache prevents native/aggregator/local profiles from
  colliding for the same provider:model string.

### Slice 13: Context Pressure Manager

- Unit: input budget reserves output headroom and scales by trusted context
  window instead of fixed model constants.
- Unit: large-window to small-window route preflight compacts or blocks before
  mutation.
- Unit/integration: compaction cache stats are surfaced through the shared web
  and channel route selection path.
- Live: optional expensive switch-overflow subcase primes a large provider
  context and attempts a switch to a small-window OpenRouter media route; the
  expected result is a clear block/compact outcome, not silent route loss.

### Slice 14: Modality Routing

- Unit: structured `[IMAGE:...]` and `[GENERATE:*]` markers resolve capability
  requirements without natural-language phrase lists.
- Unit/live: generic memory/search/workspace tools are hidden on media marker
  turns unless a dedicated media adapter is explicitly selected.
- Unit/live: media generation routes are capability/profile based, not inferred
  from provider name strings such as MiniMax/Kimi/Gemini.

### Slice 15: Self-Repair

- Unit: tool failures classify into typed repair kinds without duplicate string
  classifiers across web/channel.
- Unit: recent failures suppress only same-role alternatives and do not hide the
  only available tool.
- Integration/live: failed tool results carry bounded repair hints and do not
  retry the same bad path immediately.

### Slice 16: Memory Quality, Embedding, Self-Learning

- Unit: generic dialogue and ephemeral repair traces are rejected before durable
  memory mutation.
- Unit: concept-to-concept graph edges such as generic world-knowledge examples
  are rejected by the governor, not by phrase-specific filters.
- Unit: embedding profile resolution comes from catalog metadata; unknown
  embedding models disable embeddings rather than guessing.
- Live: long-dialogue semantic run checks early/late anchor retention, embedding
  signals, and no procedural skill promotion.

### Slice 17: Structured Handoff

- Unit: route-pressure handoff packets carry active task/defaults/assumptions/
  recent repairs and stay bounded.
- Live: large-window to small-window validation should produce compact handoff
  or block rather than silently losing context.

### Slice 18: Capability Probe/Profile Repair

- Unit/integration: `models refresh` respects provider endpoints and writes
  endpoint-aware cached profile metadata.
- Unit: typed context-limit observations can lower/fill unknown cached windows
  but never raise them optimistically.
- Live: doctor models for DeepSeek/OpenRouter passes in the base pack.
- Live/catalog: OpenRouter discovery is used to verify MiniMax text-only output
  versus actual media-capable OpenRouter models before updating bundled
  capabilities.
- Live: OpenRouter image generation smoke uses an explicit media lane and must
  return a real `[IMAGE:data:image/...]` marker, not an empty assistant turn or
  text-only description.

## OpenRouter/MiniMax Media Note

- OpenRouter currently exposes `minimax/minimax-m2.7` as `text->text`; it is
  useful as an agentic text/model-switch target, not as an audio/video/music
  generation lane.
- MiniMax direct media models belong behind a future MiniMax provider/tool
  adapter (`speech-*`, `MiniMax-Hailuo-*`, `music-*`). Until that adapter exists,
  do not fake media generation by sending media turns to a text chat model.
- For OpenRouter media tests, use models whose provider metadata advertises the
  required output modality and keep them in capability/profile data, not Rust
  match arms.
- Current adapter support is image-output first: OpenRouter `modalities` and
  `message.images` are mapped in the OpenRouter provider. Audio/video/music
  still need artifact-specific adapter follow-through before they are treated as
  fully delivered media artifacts.

### Slice 19: Assumption Tracker

- Unit: assumptions carry source/freshness/confidence/invalidation/replacement path.
- Unit: runtime/provider/tool failures challenge the exact assumption kind and
  handoff packets carry bounded active assumptions.

### Slice 20: Epistemic State

- Unit: stale/low-confidence profile and memory facts become
  `needs_verification` or `stale`, not `known`.
- Unit: retrieval/routing penalizes weaker epistemic facts before they outrank
  stronger anchors.

### Slice 21: Runtime Watchdog

- Unit: watchdog digests repeated failures, context pressure, stale metadata,
  and subsystem observations into bounded typed alerts.
- Integration: web and channel inject/render the same watchdog digest path.

### Slice 22: Runtime Calibration

- Unit: route/tool overconfident failures create bounded suppression records.
- Unit/integration: suppression affects future route/tool choices only when a
  safe same-lane or same-role alternative exists.

### Slice 23: Trace Janitor

- Unit: repair, assumption, watchdog, calibration, and handoff traces obey TTL,
  dedupe, and count bounds.
- Unit: janitor promotion candidates stay behind explicit gates and do not
  become durable user memory automatically.

### Slice 24: Runtime Adapter Contract

- Unit/conformance: web/channel runtime command presentation cannot be rendered
  locally outside shared primitives.
- Integration: provider/model switch outcomes and diagnostics flow through the
  common adapter-core executor.

### Slice 25: Tool Notification Mapper

- Unit: observer-event to notification interpretation is shared, with
  transport-specific JSON/text sinks only at the adapter edge.
- Unit: duplicate suppression and UTF-8-safe argument/output previews are common.

### Slice 26: Web/Channel Extraction

- Unit/integration: shared prompt/bootstrap/provider-history helpers remain
  outside `channels/mod.rs`.
- Unit: moved helpers do not reintroduce shared XML/tag sanitizers or
  transport side effects.

## Immediate Additions Before Next Heavy Run

1. Done: fix and test Slice 3 read-only recall narrowing so `core_memory_update`,
   `memory_store`, `memory_forget`, and `user_profile` are not visible when
   typed resolved state is sufficient.
2. Done: add a targeted test for profile-fact guidance that allows external lookup
   without exposing profile mutation.
3. Done: add a targeted 4.10 test command list/script that groups the most relevant
   domain and adapter tests above so phase-close validation is reproducible.
4. Done: re-run base live pack with strict recall no-mutation across multiple routes.
5. Done: re-run live model-switch smoke against the OpenRouter MiniMax route.
6. Done: run heavy long-dialogue and context-overflow switch after the base live
   pack is green; long semantic retention passed, and compaction is now required
   only when telemetry shows context pressure or `REQUIRE_COMPACTION_SIGNAL=1`.
