# Phase 4.10: Context Engine, Prompt Economy & Progressive Loading

Phase 4.9: self-learning, skill evolution & memory quality | **Phase 4.10: context engine, prompt economy & progressive loading** | next: Phase 4.11 runtime self-diagnostics & capability governance

---

## Problem

After Phase 4.8 and 4.9, SynapseClaw is materially better at:

- typed runtime state
- memory retrieval
- self-learning
- inspectable projections
- tool-heavy live turns

But one major architectural weakness remains:

```text
the model still sees too much context too often,
and the runtime still decides too little before asking the model
```

That shows up in three different failure modes:

1. **prompt replay waste**
   even after recent compaction work, the system still has too much hidden
   prompt state and too little formal control over what is sent per cycle

2. **bootstrap overreach**
   legacy workspace-doc assumptions (`SOUL.md`, `USER.md`, `AGENTS.md`,
   `TOOLS.md`, `MEMORY.md`) were designed for a more Markdown-driven runtime
   and still leak into behavior, planning, and developer expectations

3. **model-driven default resolution where deterministic runtime should win**
   examples:
   - implicit weather/time city
   - implicit delivery target for “send it there”
   - project/workspace context carry-over

This phase should not add more prompt hacks.

It should replace ad hoc prompt growth with a real context engine:

- stable cold-start bootstrap
- compact provider-facing shadow history
- progressive project-context discovery
- typed runtime default resolution
- explicit observability of what the provider actually receives

---

## Target

Build a context architecture where:

1. **cold-start bootstrap happens once**
2. **provider-facing history is separate from audit history**
3. **workspace/project docs load progressively, not eagerly**
4. **typed runtime defaults beat prose hints**
5. **context size is measurable and inspectable**
6. **all providers share the same prompt-economy model**
7. **cheap-model condensation is built into the runtime**
8. **cheap-route regression testing is the default validation lane**

In short:

```text
stable bootstrap snapshot
+ compact provider shadow history
+ progressive project context discovery
+ deterministic typed defaults
+ cheap-model condensation
+ cheap-route regression lane
+ context observability
= smarter and cheaper runtime turns
```

### Context budget targets

These are runtime targets, not just aspirations:

- baseline info/reply turn:
  - target: `<= 3.5k chars`
  - current tolerated ceiling while 4.10 is still landing: `<= 5.5k chars`
- simple tool turn:
  - target: `<= 5.5k chars`
  - tolerated ceiling: `<= 7.0k chars`
- heavy tool turn before condensation:
  - allowed to exceed the simple-turn ceiling transiently
  - must compress back down after the tool cycle stabilizes
- post-condensation heavy turn:
  - target: back near `5k-6k chars`

Runtime work in 4.10 should make those budgets:

- measurable in logs/observability
- enforceable in tests
- hard to regress silently

---

## Research Position

Phase 4.10 should borrow ideas selectively, not copy any one system.

### OpenAI / Codex

Useful ideas:

- server-side conversation continuation via Responses API and
  `previous_response_id`
- native tool-oriented response shape instead of chat-transcript-only loops
- clearer separation between stable instructions and per-turn input

What to preserve and adapt:

- use provider-native continuation where it is real and measurable
- avoid assuming one provider-specific flow for everyone else
- keep a provider-agnostic compact replay fallback

References:

- <https://developers.openai.com/api/docs/guides/migrate-to-responses>
- <https://platform.openai.com/docs/guides/tools-local-shell>

### OpenClaw

Useful ideas:

- explicit context tooling
- inspectable prompt/context surfaces
- pluggable context-engine concept

What not to copy literally:

- eager bootstrap injection of many workspace files on every turn
- `MEMORY.md` as a durable prompt ballast

Reference:

- <https://docs.openclaw.ai/concepts/system-prompt>
- <https://docs.openclaw.ai/concepts/context>

### Hermes Agent

Useful ideas:

- progressive context discovery
- prompt caching and compression mindset
- separation between trajectory/audit data and provider-facing prompt state

What to preserve and extend:

- nearest context file by directory scope
- compact prompt assembly
- structured rather than purely Markdown-driven runtime

Reference:

- <https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop/>
- <https://hermes-agent.nousresearch.com/docs/developer-guide/context-compression-and-caching/>
- <https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files/>

### Claude Code

Useful ideas:

- automatic prompt caching as a first-class assumption
- explicit `/compact` flow for long conversations
- layered memory files (`CLAUDE.md`) with subdirectory-specific lazy loading
- ability to disable non-essential model calls

What to preserve and surpass:

- automatic cache friendliness
- hierarchical context files
- fewer non-critical model calls in operational paths

References:

- <https://docs.claude.com/en/docs/claude-code/model-config>
- <https://docs.claude.com/en/docs/claude-code/memory>
- <https://docs.claude.com/en/docs/claude-code/slash-commands>
- <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- <https://platform.claude.com/docs/en/build-with-claude/context-editing>

### Cursor

Useful ideas:

- automatic chat summarization for long dialogues
- file/folder condensation rather than always sending full contents
- explicit distinction between summarized history and condensed file context

References:

- <https://docs.cursor.com/agent/chat/summarization>

### Aider

Useful ideas:

- repository map instead of eager full-file loading
- dynamic token budget for repo context
- prompt caching that keeps stable prefix components cache-friendly

What to preserve and adapt:

- repo-map-like compact project context
- dynamic project-context sizing based on chat state
- model can ask for full files only when needed

References:

- <https://aider.chat/docs/usage/caching.html>
- <https://aider.chat/docs/repomap.html>
- <https://aider.chat/docs/faq.html>

### OpenCode

Useful ideas:

- agent-specific prompt files instead of one giant universal prompt
- permission-driven agent specialization

Reference:

- <https://opencode.ai/docs/agents/>
- <https://opencode.ai/docs/rules/>
- <https://opencode.ai/docs/permissions/>

### ClawMem

Useful ideas:

- context surfacing as a precise hook, not a mandatory startup dump
- explicit warning that session bootstrap can waste thousands of tokens before
  the first real task

What to preserve:

- prefer precise retrieval / surfacing at the point of need
- avoid eager context ballast at session start

Reference:

- <https://yoloshii.github.io/ClawMem/>

### SynapseClaw’s stronger position

Unlike both of those systems, we already have:

- typed user profile
- typed dialogue state
- typed delivery targets
- typed procedural memory / skills / recipes
- structured memory projections

So the goal is not “better prompt templates”.

The goal is:

```text
let typed runtime decide more,
and let prompt context carry less
```

We also already have access to a cheaper-model lane.

Phase 4.10 should use it deliberately for condensation and summarization instead
of pretending every context-reduction problem belongs inside the main model loop.

---

## Design Principles

### 1. Bootstrap is a snapshot, not a loop

At session start, build a stable bootstrap snapshot.

This should contain only what is truly global and identity-critical:

- identity metadata
- persona guidance
- safety/runtime policy

It should not be rebuilt or replayed on every tool iteration.

### 2. Audit history and provider history are different artifacts

We must keep:

- full audit history for UI/debug/learning

But the provider should receive:

- compact shadow history
- current unresolved state
- current turn results

These are different products with different requirements.

### 3. Project context must be progressive

Workspace docs should not be always-on prompt baggage.

Instead:

- discover project/local instruction files by path scope
- load the nearest relevant one only when the task actually enters that scope
- cache the result per session / scope

This is especially important for:

- `AGENTS.md`
- `CLAUDE.md`
- subdirectory agent instruction files

This should be lazy subtree loading by default:

- load root-scope instructions only when a task enters that project
- load nested instructions only when a task enters that subtree
- cache the loaded result per session/scope

### 4. Defaults must be runtime-resolved, not prompt-suggested

The model should not “decide” defaults that the runtime already knows.

Examples:

- weather/time without explicit city -> dynamic profile fact such as `weather_city`
- “send it there” -> dynamic `delivery_target_preference` fact or `recent_delivery_target`
- “switch back there” -> dialogue-state workspace anchor

These should be resolved structurally before the model improvises.

### 5. Context must be measurable

We need first-class visibility into:

- bootstrap chars
- dynamic system chars
- compact chat-history chars
- current-turn chars
- tool-result chars
- per-iteration provider payload size

Without that, prompt economy work will regress invisibly.

### 6. Provider optimization should be layered

Provider-specific continuation is valuable, but only after the generic model is sane.

Order:

1. generic compact replay for everyone
2. progressive context discovery
3. cheap-model condensation
4. deterministic runtime defaults
5. provider-native continuation where supported

Provider-native continuation should be capability-driven, not guessed.

That means:

- adapter-level capability advertisement
- runtime selection based on capability, not prompt text
- compact replay remains the universal fallback

For OpenAI-family providers, that points directly at Responses-style chaining via
`previous_response_id`.

### 8. Shared runtime must speak one tool language

The shared runtime must not accumulate vendor-specific tool dialects.

Canonical rule:

- native structured tool calls when the provider supports them
- otherwise one fallback envelope only:
  - `<tool_call>{ "name": "...", "arguments": { ... } }</tool_call>`

Everything else is non-canonical:

- `<invoke ...><parameter ...>`
- GLM shorthand like `tool/param>value`
- perl/hash-ref `TOOL_CALL`
- provider-specific alias tool names
- legacy argument shapes

Those are not shared-runtime features.

If a specific provider ever genuinely needs special parsing, that behavior must
live at the adapter boundary, not in the common agent loop or domain/runtime
policy.

### 7. Condensation is not one thing

We need at least three different condensed artifacts:

1. **dialogue compaction**
   summarize older multi-turn chat into a compact semantic state
2. **large-file / large-doc condensation**
   cache a smaller representation of bulky files
3. **project brief / repo brief**
   maintain a structural overview that is cheaper than file archaeology

These should be generated by the cheaper-model lane where possible, while
preserving:

- the full audit transcript
- raw files on disk
- exact tool traces outside the provider-facing prompt

---

## Workstreams

### A. Cold-Start Bootstrap Snapshot

Build an explicit bootstrap snapshot model and keep it stable.

Scope:

- make `MEMORY.md` fully deprecated and absent from runtime/bootstrap
- keep only truly global identity/persona/runtime sections in always-on bootstrap
- stop treating project/workspace docs as baseline prompt material

Expected outcome:

- smaller stable system prompt
- no repeated file bootstrap thinking on normal turns

### B. Provider Shadow History

Formalize provider-facing history as a dedicated artifact.

Scope:

- compact provider history separate from full audit history
- retain only recent chat context plus current unresolved tool cycle
- preserve system/runtime blocks explicitly rather than by replaying all prior messages

Expected outcome:

- smaller tool-loop payloads
- less repeated ballast across provider calls

### C. Progressive Project Context Discovery

Introduce lazy loading of project instruction files.

Scope:

- nearest-scope file discovery
- session/path caching
- on-demand injection only when task enters that scope

Expected outcome:

- no eager `AGENTS.md` / `CLAUDE.md` bloat
- better locality for multi-project or nested workspaces

### D. Deterministic Default Resolution

Move defaults out of prompt prose and into runtime routing.

Scope:

- weather/time city default
- implicit delivery target
- workspace/resource “there” resolution
- stronger integration with `resolution_router`

Expected outcome:

- fewer model guesses
- fewer wrong defaults despite correct memory

### E. Context Observability

Add structured visibility into provider-facing context.

Scope:

- per-iteration context stats
- trace/log output
- harness-friendly reporting
- eventual UI/debug surface

Expected outcome:

- prompt economy work becomes measurable and enforceable

### F. Context Condensation Layer

Introduce a dedicated condensation/summarization layer.

Scope:

- cheap-model summarizer for older chat segments
- large-file condensation cache
- project/repo brief generation
- explicit invalidation when the underlying source changes

Expected outcome:

- smaller provider payloads without semantic collapse
- less pressure to reload bulky docs/files
- clearer distinction between raw artifacts and provider-facing summaries

### G. Provider Continuation

Only after A-F are in place.

Scope:

- provider capability for native continuation / server-side state
- use on supported providers
- preserve generic compact replay fallback for others

Expected outcome:

- cheaper iterative turns on providers that support continuation

---

## Implementation Order

### Current status

- overlap/research audit:
  - see [ipc-phase4_10-overlap-audit-2026-04-10.md](ipc-phase4_10-overlap-audit-2026-04-10.md)
    before implementing Slices 12-23
- consolidation note:
  - Slices 12-18 are upgrade slices over already-landed primitives, not parallel replacement systems.
  - Do not re-implement profile resolution, compaction, admission, or retrieval hygiene from scratch.
  - Each remaining slice must state:
    - which landed base it upgrades
    - which weak heuristic or threshold it replaces
    - which new invariant becomes enforceable after the upgrade
- status audit correction (2026-04-12):
  - `landed` below means a usable base layer exists, not that every slice is
    phase-close complete.
  - Slices 1-5 and 9 are treated as closed at code level.
  - Slice 8 is code-closed but still live-unvalidated on an official/key-based
    Responses continuation endpoint.
  - Slices 6, 7, 10, 11, 12, 13, 14, 15, 16, and 18 remain partial or have
    explicit follow-through tails listed in their slice sections.
  - Slice 17 is code-landed except live large-window -> small-window route
    downgrade validation.
  - Slice 19 has an initial typed-assumption layer; Slices 20-23 are planned /
    not yet implemented.
  - Slices 24-26 are treated as code-closed extraction/parity hardening, with
    future summary/run-lifecycle unification intentionally left outside their
    current scope.

- landed:
  - Slice 1: provider-facing context accounting and observability
  - Slice 2: typed implicit delivery-target resolution through runtime state
  - Slice 3: non-mutating structured recall for direct-resolution turns
  - Slice 4: live dialogue-history compaction with provider-summary carry-over
  - Slice 5: deterministic runtime execution for common delivery / mutation turns
  - Slice 6: cheap-model condensation lane for history and summaries
  - Slice 7: progressive scoped instruction loading for nearest-scope project context
  - Slice 9: strict canonical tool protocol in shared runtime paths
    - follow-through: shared text fallback now rejects bare OpenAI-shaped /
      canonical `tool_calls` JSON unless it is inside the canonical
      `<tool_call>...</tool_call>` envelope; provider-native JSON must arrive
      through structured provider response fields or be normalized in the
      provider adapter
    - native structured tool-call ids are still preserved through the shared
      runtime loop
  - Slice 10 groundwork:
    - lane candidate schema and manual profile metadata
    - preset expansion (`chatgpt`, `claude`, `openrouter`, `gemini`, `local`)
    - route-switch preflight for larger-window -> smaller-window moves
    - preset-first onboarding path
    - lane-aware runtime help/config surfaces
    - channel runtime fix so provider route changes are no longer decorative
    - first live capability-routing consumer for `multimodal_understanding`
      in channel turns with structured image markers
    - channel turn routing now sees cached model-profile metadata rather than
      only stripped provider/model tuples
    - route state now carries `lane` and `candidate_index` alongside `provider/model`
    - provider+model capability checks can fall back to cached profile metadata
      instead of relying only on already-warmed provider instances
    - preset actions now emit typed routing facts
    - first-class media lanes now exist in schema/help/onboarding for:
      `image_generation`, `audio_generation`, `video_generation`, and `music_generation`
    - current-route capability routing now respects the active resolved profile,
      so media turns no longer reroute away from a route that already supports
      the required modality
    - specialized lanes can now implicitly reuse a reasoning candidate when
      that candidate's resolved profile confidently covers the requested
      capability, so all-in-one models no longer need to be duplicated
      manually across every media lane
    - live web `Agent` now performs same-provider per-turn capability reroute,
      not only channel path routing, while still avoiding hidden cross-provider
      hot-swaps mid-turn
    - `/model` help now shows current route feature coverage and explicit
      route limits (`ctx` / `output`) instead of only provider/model strings
    - `/model` help now resolves effective lanes through the same runtime
      lane resolver, so implicit reasoning fallbacks are visible instead of
      only explicit config lanes
    - status note: this is not a full Slice 10 close; remaining Slice 10 work
      is now mostly registry/profile clean-up, adapter-heuristic shrinkage, and
      keeping lane/candidate-first explanations consistent as Slice 14/18
      continue hardening.
- code-closed / live-unvalidated:
  - Slice 8:
    - adapter-local provider-native continuation scaffolding exists for `openai-codex`
    - custom `Responses` endpoint + API-key mode now avoids Codex-only transport headers
      and enables response storage only for custom continuation-capable endpoints
    - default deployed Codex backend still rejects `previous_response_id`, so continuation
      remains capability-gated / disabled there
    - no final live validation against an official key-based `Responses` endpoint has been run yet
- newly landed / partial:
  - Slice 11 initial guardrail layer:
    - domain-owned `TurnAdmissionSnapshot`, `ContextPressureState`, and admission reasons
    - `TurnAdmissionPolicy` service for intent + capability + context-pressure preflight
    - channel path now computes an admission decision before provider execution
    - channel path can reroute or block obviously wrong turns before the provider call
    - agent loop now logs admission decisions and compacts before provider call when
      the policy marks the turn as `critical`
    - route state now carries the most recent admission snapshot and typed reasons
    - admission now classifies and guards `image/audio/video/music` generation turns
      instead of only multimodal-understanding/image/audio
    - pressured turns with low-confidence / unknown context-window metadata now
      carry an explicit `window_metadata_unknown` admission reason instead of
      pretending the target window is trustworthy
  - Slice 12 initial registry hardening:
    - `ResolvedModelProfile` now carries field-level provenance
      (`manual_config`, `cached_provider_catalog`, etc.)
    - cached profile freshness (`observed_at_unix`) now survives into resolved profiles
    - runtime help now surfaces profile source information for the active route and lane previews
    - specialized-lane auto-selection no longer silently falls back to the first
      candidate when capability metadata is unknown
  - Slice 12 follow-through partial:
    - `ResolvedModelProfile` now exposes field freshness and confidence
      (`explicit/curated/fresh/aging/stale`, `high/medium/low/unknown`)
    - specialized-lane routing and admission now reject candidates whose
      capability metadata is too stale or low-confidence
    - runtime help now surfaces profile quality, not just profile source
    - stale capability metadata now appears as an explicit admission reason
      instead of collapsing into generic `MissingFeature`
    - route-switch preflight now consumes context-window values only when their
      resolved profile confidence is at least `medium`
    - manual synthetic route-switch targets are marked as `manual_config`, so
      explicit operator-supplied windows are still trusted
    - provider-call capability checks now use a shared domain service fed by
      `image_marker_count + provider capabilities + resolved route profile`;
      web live Agent and channel/shared-loop provider calls both use this guard
      instead of owning separate vision-input logic
    - channel/shared-loop execution now receives the resolved provider:model
      route profile, so a provider-wide conservative capability flag no longer
      blocks a confidently multimodal route
    - runtime feature coverage now filters capabilities through the same
      confidence/freshness policy, so stale low-confidence catalog entries are
      not shown as usable modalities in `/model` diagnostics
  - Slice 13 initial pressure snapshot:
    - `ProviderContextBudgetInput` now tracks artifact-level breakdown for:
      - bootstrap
      - core memory
      - runtime interpretation
      - scoped context
      - resolution
      - prior chat
      - current turn
    - budget assessment now emits a `ContextBudgetSnapshot` with:
      - stable vs dynamic system chars
      - protected vs removable chars
      - chars over target / chars over ceiling
      - estimated tokens and headroom to target / ceiling
      - primary ballast artifact
    - adapter-side stats conversion now stays outside domain boundaries
    - channel-side history admission now derives the same artifact breakdown
      instead of treating all system prompt chars as undifferentiated bootstrap
    - agent runtime logs now expose pressure snapshot details before provider execution
    - route-switch preflight now reserves output headroom, exposes a safe
      context budget, and uses that budget instead of raw window size when
      deciding `safe / compact / too_large`
    - provider-facing budget tier now also reserves bounded output headroom,
      so `healthy / caution / over_budget` is no longer purely char-threshold based
    - scoped-context pressure follow-through:
      - inferred scoped instruction loads from recent typed workspace/resource/search state
        are now capped at 1 file / 900 chars
      - structured media/vision turns no longer inherit scoped context from stale
        recent workspace/resource state unless the user also supplies an explicit
        non-media path
      - `[IMAGE:...]` control markers are no longer treated as scoped filesystem hints
    - runtime-interpretation pressure follow-through:
      - structured media/vision turns use a compact runtime block with only
        profile language/style, not full working-state/current-conversation/bounded
        interpretation ballast
    - artifact-aware provider-prune policy now gives the adapter a deterministic
      pressure response:
      - drop `[scoped-context]` when it is removable ballast
      - compact oversized `[runtime-interpretation]` before the provider call
      - recompute provider-facing stats after pruning
    - provider-facing context budget now consumes trusted target profile limits:
      - `context_window_tokens`
      - `max_output_tokens`
      - field-level profile confidence gates whether those limits are trusted
      - low-confidence / stale metadata stays on the compact legacy budget
    - live agent and channel admission now feed the effective route profile into
      the same budget path, so large-window candidates no longer lose scoped or
      runtime context only because of the old fixed heavy-turn ceiling
    - Hermes-style window-ratio follow-through:
      - model context is treated as `input + output`
      - provider-facing safe input budget subtracts reserved output headroom
      - compression threshold is `50%` of safe input
      - hard safety ceiling is `85%` of safe input
      - large trusted windows such as `2M` scale by ratio instead of hitting
        old fixed scaled caps
      - low-confidence / unknown window metadata still falls back to the
        compact legacy char budget
      - `[compression]` now exposes the Hermes-style operator knobs:
        enable/disable, threshold ratio, protected head/tail, summary ratio,
        source/summary caps, and persistent cache TTL/max entries
      - live agent compaction uses trusted `context_window_tokens` and
        `max_output_tokens` from the current resolved model profile; unknown
        profile metadata falls back to the legacy `agent.max_context_tokens`
        budget instead of inventing model-specific constants
      - compaction boundaries now explicitly preserve protected head/tail and
        avoid splitting assistant tool-call / role=`tool` result groups
      - CLI auto-compaction now consumes the same compression policy path
    - route-switch preflight now consumes `ContextBudgetSnapshot` directly and
      carries the typed condensation recommendation alongside the window status
    - live agent context logs now include condensation mode, target artifact,
      minimum reclaim chars, and whether cached condensed artifact reuse is preferred
    - history compaction now uses a shared runtime cache service/port, still
      scoped per workspace agent and keyed by source transcript, compression
      policy, and trusted context window digest; repeated compaction of the same
      source no longer re-burns the summary lane
    - condensed artifact cache is now persistent under the workspace state dir,
      TTL-bounded by default to 2 days, and LRU-capped by config; this makes
      restart/fleet behavior closer to Hermes-style prompt caching while staying
      provider-agnostic
    - compression policy now supports per-route overrides selected by
      `hint` / `provider` / `model` / `lane`, composed in deterministic selector
      order rather than model-specific Rust match arms
    - compaction now resolves those route-specific compression
      settings before thresholding, cache lookup, and cache eviction
    - route/runtime inspection now carries condensed artifact cache stats
      (`entries`, `hits`, `max`, `ttl`, `loaded`) and active effective
      compression policy (`threshold`, `target`, protected head/tail, summary
      ratio, source/summary caps) through `RouteSelection` and `/models`, so
      cache behavior is visible without reading workspace state
    - web runtime now resolves those cache/policy stats against the active
      provider/model route; channel `/model` uses the same `RouteSelection`
      surface and shared cache service, so it can show real `entries`/`hits`
      without reaching into a web `Agent` cache
  - Slice 14 follow-through partial:
    - the same structured marker routing now covers image/audio/video/music
      generation instead of relying on free-text phrase detection
    - universal all-in-one reasoning candidates can satisfy media-generation
      lanes when their resolved feature metadata confidently advertises support
    - text-only/current routes still fail early or reroute through admission
      rather than silently absorbing media turns
  - Slice 15 follow-through partial:
    - bounded route-admission and tool-repair traces now feed back into
      `[execution-guidance]` as recent failure / recent admission hints
    - `/model` and `/models` surfaces now expose retained repair traces and
      admission outcomes for operator debugging
    - turn tool narrowing now consumes a conservative subset of recent repair
      hints, suppressing a just-failed tool only when an alternative of the
      same runtime role exists
  - Slice 16 follow-through partial:
    - autosave and consolidation policy now share one domain-owned governor
      instead of backend-specific skip hooks
    - repetition-aware gating now catches repeated multi-word patterns, not
      only single-token chants
    - long low-information procedural turns are less likely to leak into
      raw autosave or background consolidation
    - the same governor now gates cheap background mutation / promotion paths,
      so low-information repetition and structured control noise no longer
      bypass autosave rules and still learn recipes or mutations
    - retrieval reranking now uses the governor-owned low-anchor noise penalty
      for `daily` / `precedent` memories instead of path-local retrieval hacks
    - `memory_recall` is now a `historical_lookup` tool, not a memory mutation
      tool, so direct resolved-state turns do not keep it in the mutation-safe
      tool subset
    - central AUDN-lite mutation evaluation now calls the same governor before
      durable writes, with explicit write classes and typed consolidation
      `memory_update` parsing
    - status note: this is still partial, but the bypass audit is now narrowed
      to broader semantic/paraphrase loop detection, adapter-only graph
      extraction tails, and future write classes.
  - Phase-close validation harness:
    - added `dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh`
    - base pack covers `cheap`, `deepseek`, and `gpt-5.4` route smoke:
      tool call, working-chain recall, CJK recall, and no-mutation-on-recall signal
    - media pack uses only structured markers:
      `[GENERATE:IMAGE]`, `[GENERATE:AUDIO]`, `[GENERATE:VIDEO]`,
      `[GENERATE:MUSIC]`, and `[IMAGE:...]`
    - the pack captures systemd journal signals for:
      provider-facing context size, admission intent/action, embedding store/failure,
      and history compaction / summary-lane readiness
    - provider-context budget violations are explicit warnings by default and
      can be made hard failures with `STRICT_CONTEXT_BUDGET=1`
    - the expensive long semantic dialogue check is opt-in with `RUN_HEAVY=1`
      and should be run only at slice-close points
  - OpenRouter Gemma paid candidates are now catalog-driven and treated as
    standard optional/test routes, not default routes:
    - curated id: `google/gemma-4-31b-it`
    - efficient 26B A4B id: `google/gemma-4-26b-a4b-it`
    - pricing/profile/route-alias metadata live in `model_catalog.json`, not
      runtime match arms
    - bundled `/model` aliases include `gemma31b` and `gemma26b` while the
      default preset remains `chatgpt`
    - provider model cache still wins when fresh, then bundled/local catalog
      profile metadata is used as fallback
  - OpenRouter Grok 4.20 is now catalog-driven as a standard optional/test route:
    - curated id: `x-ai/grok-4.20`
    - aliases: `grok420` and `grok-4.20`
    - profile metadata records the OpenRouter route's `2_000_000` token context
      window and `66_000` max output
    - pricing metadata records OpenRouter's current `2.0 / 6.0` input/output
      per-million-token prices
    - stale `grok-4.1` examples were removed from tool schema descriptions and
      the ambiguous Venice curated list rather than being guessed into another
      provider's support matrix
  - OpenRouter reasoning-control follow-through:
    - runtime `reasoning_enabled` / `reasoning_effort` now reach the OpenRouter
      adapter as the provider-native `reasoning` request object
    - unset still omits the field and keeps provider/model defaults
    - `reasoning = true` is not treated as globally better; policy should enable
      it only for routes/turns that need the extra reasoning budget
    - OpenRouter's normalized `reasoning` response field is mapped back into
      the shared `reasoning_content` response field
  - Hermes context-safety follow-through:
    - provider-reported input/prompt token usage now feeds the next history
      compaction decision when the provider exposes it, while char/token
      estimates remain the fallback
    - resumed web sessions now run a high-water session-hygiene compaction
      before agent execution instead of relying only on in-turn compaction
    - channel sessions now compact provider-facing history when admission
      marks the turn as requiring compaction, preserving system blocks and the
      recent non-system tail
    - channel history compaction now rewrites the persistent session provider
      history through `SessionBackend::replace` instead of compacting only the
      in-memory map
    - `SessionBackend::replace` is now explicitly message-history-only and
      preserves rolling summaries; JSONL, SQLite, and SurrealDB backends
      implement the summary-safe replace contract
    - session-hygiene compaction now drops leading non-system orphan turns after
      trimming, so the retained provider history does not start with an orphan
      assistant/tool-result segment
    - old oversized tool results are replaced with compact typed placeholders
      before summary-lane calls so compaction does not burn context on stale
      raw tool output
    - post-compaction tool protocol sanitization removes orphan results and
      inserts bounded explicit stub results only when a surviving tool call
      would otherwise be invalid
    - web and channel model switches now share a domain `RouteSwitchPreflightResolution`
      compaction state-machine instead of duplicating local preflight loops
    - channel `/model` now preflights the target route profile before mutating
      route state, compacts when the target window only needs hygiene, and
      blocks the switch when the current provider-facing context still cannot fit
  - endpoint-aware model cache follow-through:
    - live model cache entries are now scoped by normalized `provider + endpoint`
      in onboarding and runtime profile lookup
    - the live web `Agent`, web model help, and channel model routing pass the
      configured provider endpoint into the same model-profile catalog path
    - same provider/model through native API vs aggregator can now resolve
      different cached context windows, output ceilings, and capability
      metadata without colliding
    - provider identity inference from custom `api_url` is now catalog-driven
      via editable `api_base_urls`, so runtime profile lookup can use native
      provider metadata for OpenAI-compatible endpoints without Rust model/URL
      match arms
  - Slice 11 route-state follow-through:
    - `/model <hint>` now preserves the matched route's capability lane in
      channel route state instead of collapsing the route to provider/model only
    - web runtime route switching now stores the same active lane/candidate
      identity on the `Agent`, so `/model` help can render the active lane and
      route-specific compression cache stats consistently with channel help
    - `CapabilityLane` now has a shared domain display label, and web/channel
      `/model` switch responses render the active lane when a switch is lane-aware
  - web/channel runtime-command parity follow-through:
    - common provider/model command presentation now lives in the domain
      runtime-command presentation service, with adapter-specific help still
      limited to live transport state such as route inspection and model cache stats
    - adapter parity is now pinned by an adapter-core `RuntimeAdapterContract`
      trait describing shared decision ownership and explicit web/channel
      transport/lifecycle differences
    - duplicated web/channel `CommandEffect` execution was collapsed into the
      adapter-core `execute_runtime_command_effect` executor; adapters now
      implement a narrow `RuntimeCommandHost` for lifecycle hooks such as
      route mutation, provider initialization, session clear, and live help
      surfaces
    - `/providers` and `/model` help rendering now flows through typed
      `RouteSelection` / config snapshots instead of adapter-owned pre-rendered
      strings
    - provider/model route mutations now flow through typed request/outcome
      structures, including model-switch blocked outcomes, so web and channel
      can keep lifecycle differences without forking semantics
    - channel provider route changes now wait for adapter-level
      validation/canonicalization before mutating route state, so aliases and
      unknown providers cannot leave raw provider ids in the runtime route
    - channel `/new` and `/model` side effects now run through the
      `RuntimeCommandHost` lifecycle hook instead of the pre-validation domain
      command classifier
    - route diagnostic reset is a shared `RouteSelection` helper, and
      channel model-switch preflight uses shared domain history-budget helpers
      before applying route mutation
    - web and channel runtime-command hot paths instantiate their concrete
      contract implementations and use the trait for presentation options and
      provider alias canonicalization, so the trait is not just a test-only
      descriptor
    - conformance tests now reject local web/channel runtime-command presentation
      strings, so future success/error text must go through the shared domain
      presentation primitives
    - provider alias canonicalization is shared by web and channel through the
      adapter-core runtime route helper because the concrete provider catalog
      belongs to the provider adapter layer, not the domain
    - web model-switch preflight now returns a structured blocked outcome and
      uses the same domain formatter as channel `/model` instead of emitting an
      ad hoc context-budget error string
    - web model-switch preflight now resolves the target route profile through
      the same config/catalog route-selection profile path as channel, so
      route-local profile metadata is not lost on `/model` switches
    - generic and channel system-prompt builders now have explicit surfaces;
      channel-only transport guidance is no longer injected into generic
      runtime prompts
    - web and channel remain separate adapters for transport/lifecycle, but
      route preflight, context-budget reporting, route diagnostics rendering,
      command effects, lane labels, and formatting primitives now share the
      same runtime/domain path where the dependency direction allows it
  - Test/contract hardening follow-through:
    - `synapse_channels` unit test target now compiles its session-backend tests
      against the async `SessionBackend` contract
    - the channel `reqwest` dependency explicitly enables the `query` feature
      required by channel API clients
    - regression tests cover no-runtime/current-thread runtime persistence
      bridge behavior and summary-preserving session replace
- next:
  - Slice 12 follow-through:
    - widen provenance beyond cached provider catalogs as more profile data moves into catalogs
    - keep auditing new tool-capability decisions so they enter through the
      shared domain capability service rather than web/channel-specific guards
  - Slice 13 follow-through:
    - move selected compression presets into the model catalog if route-specific
      pressure behavior becomes common enough to share out of the box
    - keep the pluggable `ContextEngine` interface deferred until the current
      domain pressure service/port boundary is stable enough to avoid creating
      a parallel context subsystem
  - reasoning-control follow-through:
    - promote provider reasoning controls from global runtime override to a
      capability/lane policy once the model-profile registry exposes support
      and provider cost tradeoffs consistently
    - add full `reasoning_details` preservation when the shared provider
      response/history model can carry provider-native reasoning blocks without
      leaking adapter-specific shapes into the core runtime
  - Slice 11 follow-through:
    - continue unifying any remaining channel/web admission-state persistence
      edge cases after live validation, but route mutation now preserves lane
      identity on both paths
  - quality tail after Slice 6/7:
    - long-dialogue semantic anchor ranking
    - pure-dialogue graph hygiene
    - better cheap-lane use of already-loaded scoped context on ambiguous prompts
  - provider-native continuation on an endpoint that actually advertises / accepts it
  - Slice 16 follow-through:
    - remaining policy-bypass audit is now narrowed after central mutation
      governance and graph-extraction hygiene landed; keep watching future
      write classes and adapter paths that introduce new durable-write sources
    - strengthen low-information paraphrase loop detection beyond lexical
      repetition gates
    - re-run the expensive long-dialogue semantic check only at slice-close
      points
  - Slice 17:
    - bounded structured handoff packets landed for route/admission pressure
      surfaces; cross-channel transition and delegation now have a shared packet
      shape to reuse instead of new prose-only summaries

### Slice 1

- document Phase 4.10
- formalize provider-facing context snapshot
- add context-size observability

### Slice 2

- resolve implicit delivery target from typed turn state instead of prompt prose
- expose per-turn defaults through a scoped runtime context port
- wire `message_send` to prefer recent delivery target, then user-profile fact

### Slice 3

- implement non-mutating structured recall for:
  - direct working-chain recap
  - direct weather-city fact recap
  - direct current-conversation / recent-target recap
- narrow tool exposure to zero when typed runtime state is already sufficient

### Slice 4

- implement live dialogue-history compaction:
  - summarize older multi-turn chat segments
  - preserve latest compaction summary in provider-facing context
  - harden gateway/history bookkeeping when compaction shrinks history

### Slice 5

- implement deterministic runtime execution for:
  - common local profile mutations
  - common task-state mutations
  - configured delivery turns
- goal:
  - cheap models should not need perfect native tool-calling behavior for routine local/external intents

### Slice 6

- add cheap-model condensation for:
  - older dialogue
  - large doc/file summaries
  - project brief / repo brief
- cheap summarizer lane should stay almost stateless by default:
  - bounded input chunks
  - tiny stable system prompt
  - no full replay of prior compaction dialogue
  - prefer map/reduce style condensation for very large inputs
  - avoid recursive "conversation with the compactor" patterns
- provider-native continuation for the cheap summarizer is optional and low priority:
  - only enable it when a specific adapter shows a real measurable gain
  - default design remains short stateless summarization jobs
- current status:
  - agent history compaction, web session summarization, and channel summaries now resolve
    their summarizer lane through one domain service
  - precedence is explicit `[summary]` config -> `summary_model` -> `cheap` route -> current route
  - live daemon validation confirmed compaction on a long cheap-route dialogue
    (`history_len_before=54`, `history_len_after=25`)
  - procedural skill count stayed flat during the pure semantic run (`list_skills = 61`)
- post-slice hardening validated on a fresh long cheap-route dialogue:
  - late-anchor compare turn now completes after a single `memory_recall`
  - repeated `memory_recall` churn no longer reproduced in the same scenario
  - provider-facing context stayed bounded with `prior_chat_messages = 6`
- 2026-04-12 follow-through:
  - turn-context hybrid recall now passes candidate entries through the same
    governor-backed memory reranker as `memory_recall`, so daily/precedent noise
    and low-information loops are not ranked differently only because recall
    entered through the automatic context path
  - low-information detection now catches repeated semantic shingles, not only
    exact token chants or contiguous repeated phrases
- remaining quality tail after the hardening:
  - recall ranking is better, but still needs live long-dialogue validation on
    concept-heavy semantic sessions after the hybrid-rerank follow-through
  - pure-dialogue extraction is quieter, but anchor-like conceptual turns can still emit
    concept entities/relationships that may be too generic

### Slice 7

- add progressive project-context discovery
- cache nearest-scope instruction files per session
- preserve hexagonal boundaries:
  - domain decides when scoped context is relevant
  - adapter discovers nearest `AGENTS.md` / `CLAUDE.md`
  - runtime injects a dedicated `[scoped-context]` block only when needed
- current status:
  - wired into both web/gateway and channel runtime paths
  - live subtree validation loaded real scoped context:
    - `scoped_context_chars = 295` on the successful direct subtree prompt
  - a direct `gpt-5.4` scoped prompt returned the exact local confirmation phrase:
    - `SUBTREE_SCOPE_CONFIRMED`
  - media/vision turns now suppress stale inferred scoped context and keep explicit
    user path hints as the only way to force scoped project instructions into that turn
  - scoped-context blocks now carry explicit bounded metadata
    (`active_for_this_turn`, `use_before_workspace_or_bootstrap_lookup`) so cheap
    routes have a clearer typed instruction to consume already-loaded scoped
    context before generic workspace/bootstrap discovery
  - cheap route behavior still needs live validation on weaker or more ambiguous
    prompts after the scoped-context block hardening

### Slice 8

- add provider-native continuation support where it genuinely helps
- preserve hexagonal boundaries:
  - provider-native continuation state must live in the provider adapter
  - shared runtime stays on one canonical tool protocol and compact replay fallback
- current status:
  - adapter-local response-id tracking and delta-input assembly were implemented in
    `openai_codex`
  - custom `Responses` endpoint mode now cleanly separates:
    - Codex backend transport quirks
    - official/custom API-key `Responses` transport
  - official/custom endpoint path now:
    - suppresses Codex-only transport headers
    - suppresses Codex-only `reasoning.encrypted_content` include fields
    - enables `store=true` only for custom continuation-capable endpoints
  - live validation on the deployed Codex backend showed:
    - `store=true` is rejected (`Store must be set to false`)
    - `previous_response_id` is also rejected as an unsupported parameter
  - therefore Slice 8 is closed at code level, but still lacks live validation on a
    real official/API-key endpoint:
    - the architecture is prepared
    - the default deployed Codex backend is not continuation-capable
    - continuation remains capability-gated / disabled by default on that route

### Slice 9

- enforce a strict canonical tool protocol in shared runtime paths:
  - native structured tool calls
  - one fallback `<tool_call>{json}</tool_call>` envelope
- move any provider-specific dialect handling to adapter-local code only if it
  is ever still needed
- explicitly keep OpenAI/Codex-specific recovery in the provider adapter rather
  than in shared runtime:
  - if `openai-codex` emits malformed canonical text envelopes, normalize them
    at the adapter boundary
  - shared native dispatcher must not recover provider-specific text envelopes
- remove shared-runtime dependence on GLM / perl / minimax / XML-parameter
  fallback dialects
- remove shared-runtime tolerance for alias JSON shapes such as:
  - `parameters`
  - `call_id`
  - `tool_call_id`
  when those appear outside provider-native adapters

### Slice 10

- evolve runtime routing from provider-brand heuristics toward explicit capability lanes:
  - `reasoning`
  - `cheap_reasoning`
  - `embedding`
  - `image_generation`
  - `audio_generation`
  - `multimodal_understanding`
- keep the core/runtime contract capability-based rather than vendor-named:
  - adapters advertise what they can do
  - runtime resolves intent/modality to a capability lane
  - adapters then map that lane to concrete model ids and endpoints
- allow providers like Kimi / Z.AI / future multimodal vendors to fit without
  polluting core logic with brand-specific routing rules
- preserve hexagonal boundaries:
  - capability resolution in domain/runtime
  - vendor-specific API quirks only in provider adapters
- represent each route as an ordered lane candidate, not a single brand shortcut:
  - candidate `0` is the default
  - later candidates are fallbacks or manual runtime alternatives
  - candidate identity is `provider + model + adapter/runtime profile`, not just `model`
- support automatic candidate-profile enrichment by default:
  - best-effort metadata from cached/provider model catalogs
  - manual override in config for `context_window_tokens`, `max_output_tokens`, and features
  - same model id through different providers is treated as a different candidate profile
- add safe route-switch policy for large-window -> small-window moves:
  - switching from a larger context window to a smaller one must use the target
    candidate's metadata, not the current route's assumptions
  - before switching, runtime must preflight the current provider-facing context
    against the target window
  - if the target lane is tighter, compact first; if it still does not fit,
    refuse the switch and require a new session or explicit handoff summary
- feature metadata must be explicit and candidate-scoped:
  - `thinking/reasoning` as the default lane does not imply vision/image/audio support
  - image/audio/multimodal capabilities must resolve from candidate metadata
- use this slice to prepare a future config evolution away from only:
  - `default_provider`
  - `default_model`
  toward lane-aware routing for:
  - main reasoning
  - cheap summarization / compaction
  - image
  - audio
  - embeddings
- current status:
  - explicit candidate metadata exists for context window, output ceiling, and features
  - ordered lane candidates are resolved through domain services rather than brand-only hints
  - presets now expand into the same lane model instead of creating a second routing system
  - onboarding starts from simple presets and then populates the richer lane-aware config
  - preset seeds, provider defaults, curated model lists, and default pricing now live in a
    built-in external catalog instead of hardcoded Rust match arms
  - users can now materialize and edit a local override catalog next to `config.toml`
    via `synapseclaw models catalog init`; runtime merges that file over the built-in catalog
  - `/model` help and routing config inspection now surface preset/effective-lane information
  - channel runtime now respects per-turn provider route changes instead of always using the startup provider instance
  - channel capability routing now consumes cached/provider profile metadata through a port
    instead of operating on a metadata-blind synthetic route view
  - route state now stores `provider + model + lane + candidate_index` for future
    continuity-aware lane routing
  - provider+model vision checks now have a catalog-backed fallback when a
    non-default provider instance has not been warmed yet
  - `model_routing_config` preset operations now participate in typed routing facts
  - remaining work:
    - historical note: earlier remaining items about image/audio/video/music
      first-class turn routing are now mostly owned by Slice 14 and the shared
      marker/admission path, not by a separate Slice 10 routing fork
    - provider capability metadata is still narrower than the eventual lane matrix
    - a few provider-local model-family heuristics still remain adapter-side
      (for example reasoning-effort clamps and embedding-family inference)
    - route state stores lane/candidate identity, and route-switch UX now renders lane
      for lane-aware switches; downstream runtime surfaces should keep moving toward
      lane/candidate-first explanations
    - keep auditing that new routing decisions enter through domain/profile
      services rather than brand-string heuristics

### Slice 11

- add a first-class turn admission / guardrail layer ahead of provider invocation:
  - turn intent category is resolved in domain/runtime before model selection
  - the runtime computes an allowed lane/candidate set for that turn
  - the runtime rejects impossible or unsafe provider/model choices before the provider is called
- keep the core flow obvious and inspectable:
  - `turn intent -> capability requirement -> candidate filter -> context budget preflight -> execution`
  - avoid hidden model misuse caused by prompt-only steering
- protect against context overflow structurally, not only after the fact:
  - define context-pressure states:
    - `healthy`
    - `warning`
    - `critical`
    - `overflow_risk`
  - bind them to deterministic policy:
    - `healthy`: normal execution
    - `warning`: prefer compact replay / condensed context
    - `critical`: mandatory compaction or lane downgrade before provider call
    - `overflow_risk`: block direct execution until compaction / handoff / new session
- protect against wrong-model execution structurally:
  - a default reasoning candidate is not implicitly allowed for vision, image, audio,
    music, or video turns
  - unsupported tool-calling candidates are blocked from tool-heavy turns
  - candidates with insufficient context window are blocked from current-turn execution
    unless preflight compaction succeeds
- preserve hexagonal boundaries:
  - turn classification and admission policy live in domain services
  - provider adapters only advertise capabilities / limits and expose provider-native quirks
  - channels/web/gateway only surface the resulting decision
- borrow selectively from Hermes and OpenClaw:
  - Hermes-like runtime preflight and budget-aware compression mindset
  - OpenClaw-like explicit capability slots for specialized modalities
  - do not copy either system's product-specific config shape literally
- deliverables:
  - `TurnAdmissionPolicy` domain service
  - `ContextPressureState` domain model
  - `CandidateAdmissionDecision` with structured reasons
  - runtime logging / observability for admission decisions
  - route state enriched with the last admission outcome
- current status:
  - landed:
    - domain-owned admission snapshot / pressure-state types
    - structured admission reasons
    - agent pre-provider admission logging
    - agent pre-provider compaction when admission marks the turn as `critical`
    - channel admission preflight with reroute/block before provider invocation
    - route state includes the latest admission snapshot
  - remaining:
    - widen intent consumers past the current multimodal + specialized-lane protection
    - add direct image/audio/video generation admission paths
  - continue making runtime UX surfaces display admission state explicitly beyond
    `/model` help and lane-aware switch responses
    - settle whether admission snapshots should remain ephemeral or be persisted
      across route overrides/new sessions
- expected outcome:
  - fewer hidden planner/provider mismatches
  - fewer late failures from context overflow
  - clearer core control flow before each provider call
  - safer future support for image/audio/video/music-capable models

### Slice 12

- upgrade target:
  - strengthens Slice 10 groundwork instead of replacing it
  - upgrades the current `ResolvedModelProfile` plus bundled/local/catalog merge into a provenance-aware registry
- add a first-class model-profile registry layer so capability/limit knowledge
  stops being fragmented across:
  - built-in catalog
  - cached provider catalogs
  - adapter heuristics
  - ad hoc config overrides
- make profile resolution explicit and ordered:
  - manual candidate profile in `config.toml`
  - local user `model_catalog.json` override
  - cached live provider catalog
  - built-in bundled catalog
  - adapter-local fallback defaults
- treat `provider + model + runtime profile` as the profile identity:
  - same model family through OpenRouter/native/direct provider is not assumed
    to have the same context window, max output, or capabilities
- lift more provider/model knowledge into data instead of code:
  - `context_window_tokens`
  - `max_output_tokens`
  - capability flags
  - cost/latency tier
  - optional notes about tool-calling / continuation / prompt-caching support
- reduce adapter-side model-family heuristics over time:
  - reasoning-effort clamp rules
  - embedding-family inference
  - modality capability guesses
- preserve hexagonal boundaries:
  - domain resolves the effective model profile
  - infra/adapters load catalogs and live caches
  - provider adapters only retain wire-level quirks that truly cannot be data-driven
- deliverables:
  - `ResolvedModelProfile` as a first-class domain object
  - profile provenance in logs / route inspection
  - a unified merge policy for bundled catalog + local override + cached metadata
  - explicit “unknown capability” handling instead of silent optimistic fallback
- expected outcome:
  - runtime can reason about providers/models more like Hermes
  - fewer hidden wrong-model selections
  - fewer stale hardcoded model assumptions in Rust

### Slice 13

- upgrade target:
  - strengthens Slices 4, 6, and 11 instead of creating a second compaction system
  - upgrades threshold-based compaction and admission into a full pressure manager
- turn compaction into a full context-pressure manager rather than a single
  thresholded history summary mechanism
- move from “compaction exists” to “budget-aware execution policy”:
  - estimate target-candidate budget before provider call
  - reserve headroom for tool schemas, output, and follow-up turns
  - protect recent tail turns and active commitments
  - choose the right condensation strategy for the pressure state
- add multi-tier reactions to pressure:
  - `healthy`: normal execution
  - `warning`: trim ballast, prefer cached condensed artifacts
  - `critical`: mandatory compaction / lane downgrade / summary handoff
  - `overflow_risk`: block or require explicit handoff/new session
- add target-aware switching and continuation budget policy:
  - large-window -> small-window switch must re-evaluate the active context
    against the target candidate budget
  - if needed, compact first; if still unsafe, refuse or require handoff
- distinguish compaction inputs:
  - dialogue history
  - large file/doc summaries
  - tool-result traces
  - scoped context artifacts
  so we do not over-compress the wrong thing
- keep the cheap summarizer almost stateless:
  - bounded chunks
  - tiny stable prompt
  - map/reduce for large inputs
  - no “long conversation with the compactor”
- deliverables:
  - `ContextBudgetSnapshot`
  - target-candidate token/char budget policy
  - reusable condensed artifact cache keyed by source digest
  - route-switch preflight integrated with pressure manager
- current status:
  - landed:
    - `ContextBudgetSnapshot` tracks provider-facing artifact pressure for
      bootstrap, core memory, runtime interpretation, scoped context,
      resolution, prior chat, and current turn
    - provider context budgeting consumes trusted target profile
      `context_window_tokens` and `max_output_tokens` with confidence gates
    - model context is treated as `input + output`; safe input subtracts
      reserved output headroom before pressure and compaction thresholds
    - default compression trigger is `50%` of safe input and hard safety
      ceiling is `85%` of safe input
    - large trusted windows such as `2M` scale by ratio instead of hitting the
      old fixed heavy-turn caps
    - low-confidence or unknown window metadata falls back to the compact legacy
      char budget instead of inventing model-specific limits
    - admission can request pre-provider compaction, and route-switch preflight
      can recommend compaction or block a big-window -> small-window switch
    - pressure pruning can drop removable `[scoped-context]` and compact
      oversized `[runtime-interpretation]` before the provider call
    - `[compression]` exposes Hermes-style knobs for enable/disable, threshold,
      retained tail ratio, protected head/tail, summary ratio, source/summary
      caps, cache TTL, and cache entry cap
    - per-route compression overrides select by `hint`, `provider`, `model`,
      and `lane` without Rust model match arms
    - history compaction cache is now a shared runtime port with persistent
      TTL/LRU storage under workspace state and cache keys scoped by transcript,
      policy, and trusted context-window digest
    - web and channel route inspection both expose effective compression policy
      and real shared cache `entries` / `hits`
    - compaction preserves protected head/tail and avoids splitting assistant
      tool-call / role=`tool` result groups
    - CLI auto-compaction consumes the same compression policy path
    - provider-reported usage tokens can now override estimated prompt tokens
      for the next compaction decision
    - resumed web sessions run a high-water session-hygiene compaction before
      provider execution, and channel sessions compact provider-facing history
      when admission marks the turn as requiring compaction
    - old oversized tool results are pruned into compact placeholders before
      the summary lane sees the compaction transcript
    - post-compaction sanitization removes orphan tool results and inserts
      bounded stub results for surviving tool calls that would otherwise lose
      their paired result
    - web/channel model-switch preflight uses the same domain resolution
      state-machine for compact/reassess/block decisions
  - still open after Hermes source audit:
    - no pluggable context-engine interface yet; keep this deferred until the
      domain service/port boundary is stable
- completed Hermes-derived follow-through:
  - provider usage feedback:
    - records actual provider-reported input tokens after a turn when available
    - prefers those tokens over char estimates for the next compaction decision
    - falls back to char/token estimates when usage is absent
  - session hygiene safety net:
    - inspects long web sessions before resumed agent execution
    - uses a high-water threshold around the safety ceiling rather than the
      normal 50% compressor trigger
    - keeps a hard message-count valve to break runaway history growth when API
      failures prevent fresh token telemetry
    - keeps channel provider-facing history bounded when admission already
      requires compaction
  - cheap tool-result pruning:
    - before summary-lane calls, replaces old oversized tool outputs with a
      short typed placeholder when they are outside the protected recent context
    - preserves enough head/tail detail for commands, errors, paths, and
      concrete values to remain visible to the summary prompt
  - post-compaction tool protocol sanitizer:
    - validates assistant tool-call and role=`tool` result pairing after compaction
    - removes results whose call id no longer has a surviving assistant call
    - inserts a bounded explicit stub result only when a surviving assistant call
      would otherwise be missing its result
  - route-switch preflight unification:
    - shared domain `RouteSwitchPreflightResolution` owns compact/reassess/pass-limit policy
    - web `Agent` and channel `/model` both use that policy while keeping transport-specific
      session lifecycle in their adapters
    - channel route state is not mutated when the target safe context budget is still exceeded
  - remaining non-copied Hermes item:
    - pluggable `ContextEngine` is intentionally deferred until the existing
      domain pressure services and ports stabilize
  - avoid copying Hermes' stringy path detector directly:
    - scoped context should continue moving through typed tool facts / scoped
      instruction ports
    - free-text shell/path extraction can be adapter-local fallback only, not
      core routing policy
- expected outcome:
  - context overflow becomes rare and explainable
  - model switches across very different windows become safe by construction
  - long-dialogue behavior becomes more stable after compaction

### Slice 14

- upgrade target:
  - strengthens Slice 10 routing and Slice 11 admission
  - upgrades the current single live multimodal consumer into a full modality matrix
- make modality routing first-class instead of leaving it as a future extension
  of reasoning lanes
- define explicit turn classes and capability targets for:
  - `multimodal_understanding`
  - `image_generation`
  - `audio_generation`
  - `video_generation`
  - `music_generation`
- add deterministic admission and lane selection for these turns:
  - text-only candidates cannot be selected for media generation
  - tool-only reasoning lanes cannot silently absorb image/audio turns
  - unsupported candidates fail early with a clear reason
- keep execution flow obvious:
  - `turn intent -> modality requirement -> lane chooser -> admission -> provider/tool execution`
- make the same logic available across:
  - CLI
  - gateway/web
  - Matrix/Telegram/other channel handlers
- align presets and onboarding with modality lanes:
  - presets can seed modality lanes when safe
  - local/user catalogs can override modality defaults cleanly
  - channel UX can explain why a route/model was chosen or rejected
- expected outcome:
  - fewer attempts to use the wrong model for images/audio/video/music
  - clearer core flow for future multimodal providers like Kimi/GLM/OpenAI/Gemini
  - less brand-specific logic leaking into the runtime
  - current status:
  - landed:
    - explicit schema lanes/features for `video_generation` and `music_generation`
    - turn capability inference now recognizes:
      - structured image markers for multimodal understanding
      - explicit structured generation markers for image/audio/video/music turns
      - those markers now flow through a shared domain `turn_markup` parser
        instead of duplicated `contains()/starts_with()` checks across routing,
        autosave/governor, and inbound history normalization
    - admission now maps those turn intents into explicit capability lanes
    - blocked-turn UX now explains missing image/audio/video/music lanes explicitly
    - onboarding/provider-catalog parsing now preserves explicit `video` and `music`
      output modalities when a provider catalog surfaces them
    - runtime help/config surfaces render the new lanes/features
  - remaining:
    - presets/local catalogs do not yet seed safe built-in video/music candidates by default
    - modality inference remains conservative and marker-based; richer intent recognition
      should come later through typed interpretation or an explicit classifier, not keyword lists
    - more route/runtime UX should render lane identity first and `provider/model` second
    - status note: do not reintroduce keyword phrase lists for media detection;
      until typed interpretation exists, structured markers are the accepted
      deterministic interface.

### Slice 15

- upgrade target:
  - strengthens Slice 11 admission and the existing loop/failure suppression work
  - upgrades scattered failure handling into a bounded typed repair ledger
- add first-class explainable self-repair for both routing and tools:
  - why a lane/candidate was chosen
  - why another candidate/tool path was rejected
  - why a tool failed
  - what the runtime/model tried next and why
- make the reasoning visible to the model in a structured, bounded form rather
  than relying on repeated prompt re-interpretation:
  - `last_route_decision`
  - `last_tool_failure`
  - `last_repair_attempt`
  - `recommended_next_action`
- support tool-aware self-repair explicitly:
  - schema mismatch
  - permission/security rejection
  - missing auth / missing API key
  - wrong target / unresolved target
  - context-pressure refusal before tool execution
  - provider/tool capability mismatch
- preserve hexagonal boundaries:
  - domain owns `RepairDecision`, `FailureReason`, and retention policy
  - adapters translate provider/tool/runtime errors into typed reasons
  - channels/web surfaces only render the structured explanation
- keep it bounded so it does not become a second long-term memory system:
  - store only compact typed repair summaries
  - retention window: a few days max (default 48 hours)
  - evict aggressively by age and count
  - do not promote this directly into durable profile/recipe memory unless a
    separate learning rule chooses to do so
- expose this in operator/runtime UX:
  - `/model` and route inspection can explain the latest lane decision
  - runtime traces can explain the latest tool failure and repair choice
  - assistant can answer “why did you choose that?” from structured state,
    not invented prose
  - current status:
  - landed first pass:
    - route admission state now carries a typed `recommended_action`
    - runtime help surfaces can show the suggested next route-repair step
      (switch lane / compact / refresh metadata / fresh handoff)
    - route admission retention is now also a bounded short ledger:
      - TTL `48h`
      - max retained admission outcomes `4`
      - adjacent duplicate outcomes collapse by signature
    - `/model` and `/models` can now show recent retained route-admission
      outcomes, not only the most recent one
    - tool execution now emits a typed `ToolRepairTrace` for:
      unknown tool / policy block / duplicate invocation / runtime error /
      reported failure
    - channel route state and web `/model` help can surface the latest tool
      repair trace from structured state
    - tool repair retention is now a bounded short ledger:
      - TTL `48h`
      - max retained traces `8`
      - adjacent duplicate failures collapse by signature
    - tool/runtime state now carries distinct repair traces across a whole turn,
      not only the last failure
    - `/model` and `/models` now preview the most recent retained repair traces,
      not just the retained-count summary
    - failed tool results now carry a compact `[tool_repair]` footer with
      `kind/action/detail`, so the model itself can see bounded typed repair
      context on the next reasoning step instead of inferring only from raw
      prose errors
    - turn-context `execution_guidance` now also carries bounded recent
      tool-failure hints, so normal reasoning turns can see “do not immediately
      retry the same failing path” without relying only on operator surfaces
    - turn-context `execution_guidance` now also carries bounded recent
      route-admission hints (`reasons` + `recommended_action`) for both channel
      routes and the live web/agent session path, so the model can see the last
      “wrong lane / stale metadata / near-limit” outcome without re-inferring it
      from raw failures or operator help text
    - live `Agent` tool execution now reuses the same canonical executor as the
      loop/runtime path instead of keeping a separate failure-handling branch
    - tool runtime errors now classify richer typed repair kinds where a real
      typed signal already exists:
      - provider capability mismatch -> `capability_mismatch`
      - HTTP 401/403 -> `auth_failure`
      - `std::io::NotFound` -> `missing_resource`
      - `std::io::PermissionDenied` -> `policy_blocked`
      - HTTP 413 -> `context_limit_exceeded`
      - timeout-like IO / reqwest failures -> `timeout`
      - structured argument/JSON decode failures -> `schema_mismatch`
    - capability mismatch traces now prefer an explicit `switch_route_lane(...)`
      repair action when the missing capability maps to a known lane
    - external callers no longer use `agent::loop_::*` import paths for
      `resolve_agent_id` or canonical tool runtime execution; the old `loop_`
      namespace has been collapsed into an internal `runtime_loop` module
      instead of a public runtime axis
    - inbound channel reactions now use canonical upstream `event_ref`
      instead of the conversation/session key as fake `message_id`
    - `AgentRuntimePort` now returns typed `AgentRuntimeErrorKind`
      (`timeout`, `context_limit_exceeded`, `capability_mismatch`,
      `auth_failure`, `runtime_failure`) so the channel use-case no longer
      parses provider/runtime error strings directly for timeout or
      context-overflow recovery
  - remaining:
    - broaden repair-ledger consumers beyond current route/help/operator surfaces
    - extend typed failure reasons further without falling back to string parsing
      in core/runtime
    - decide whether recent route-admission hints should remain “last distinct
      outcome only” or become a tiny bounded ledger parallel to tool repairs
- expected outcome:
  - fewer opaque failures
  - fewer repeated bad tool attempts
  - clearer user-facing explanations for route/tool choices
  - a foundation for future autonomous recovery without prompt bloat

### Slice 16

- upgrade target:
  - strengthens the Slice 6/7 retrieval hardening already landed
  - upgrades ad hoc ranking/entity filters into an explicit memory-quality policy
- add a memory-quality governor so long dialogue and autosave do not pollute
  retrieval with weak or generic facts
- make memory writes more explicit by class:
  - `preference`
  - `task_state`
  - `fact_anchor`
  - `recipe`
  - `failure_pattern`
  - `ephemeral_repair_trace`
- add write-budget and promotion rules:
  - generic abstract claims from ordinary conversation should not become durable memory
  - durable writes require stronger evidence or repeated confirmation
  - ephemeral repair traces must never bypass the governor into profile memory
- improve long-dialogue anchor quality:
  - suppress generic semantic graph junk
  - preserve meaningful early/late anchors through compaction
  - favor typed anchors over low-signal precedent/daily noise
  - current status:
  - landed first pass:
    - current entity/relationship acceptance rules were lifted into domain-owned
      `memory_quality_governor`
    - `entity_extractor` now consumes policy verdicts instead of owning the
      hygiene rules itself
    - learning evidence now distinguishes external procedural evidence from
      internal maintenance evidence (`memory/session/precedent/routing`)
    - `post_turn_orchestrator` now runs accepted learning assessments through the
      governor before mutation / recipe promotion
    - internal-only procedural turns no longer start procedural learning or
      reflection just because they touched `memory_recall` / session / routing tools
    - post-turn orchestration now passes a typed reflection outcome hint into the
      memory adapter
    - reflection fallback string-matching has been removed from the memory adapter;
      reflection now requires typed `ReflectionOutcome` from orchestration
    - background consolidation is now gated by a domain-owned verdict instead of
      a raw length/actionable-evidence shortcut
    - internal-only procedural turns no longer start consolidation simply because
      the user message is long
    - reflection/consolidation minimum thresholds now live inside governor
      verdicts instead of path-local branching in orchestration
    - the default autosave/reflection/consolidation thresholds now also live in
      governor-owned constants instead of duplicated orchestration/runtime
      constants
    - raw conversation autosave is now governed by a typed autosave verdict
      instead of path-local length checks alone
    - live agent, channel inbound path, CLI runtime loop, and gateway/webhook
      autosave now share that autosave verdict so explicit control turns and
      structured media markers do not get written into conversation memory just
      because they are long
    - raw conversation autosave now uses distinct canonical keys across live
      agent, runtime loop, gateway, and channel inbound paths:
      - live agent now uses the shared autosave key helper instead of fixed `user_msg`
      - channel inbound now carries optional typed `event_ref` on `InboundEnvelope`
      - when present, channel autosave keys are derived from upstream event ids
      - when absent, channel autosave falls back to a bounded receipt-based key
    - `should_consolidate_memory` now uses the same typed autosave gate instead
      of drifting back to a raw length threshold
    - inbound runtime error details are now sanitized at the adapter boundary
      before reaching domain error handling
    - backend-specific `should_skip_autosave` hooks were removed; autosave policy
      now has a single source of truth in the domain governor/util layer
    - the governor now has a first repetition-aware gate:
      long low-information repetition is skipped for raw autosave and
      background consolidation unless stronger typed evidence already exists
    - model-invented generic world-knowledge relationships are now blocked
      primarily by the durable typed consolidation gate before graph extraction,
      rather than by language-specific role-name or suffix filters
    - standalone generic concept entities now require an accepted relationship
      endpoint before they can be stored, so pure dialogue does not create loose
      abstract graph nodes without a useful anchor
    - low-confidence relationships between generic concept endpoints are
      rejected by the same domain governor, then applied by the adapter
      extractor before graph writes
    - mutation writes now carry an explicit durable write class
      (`preference`, `task_state`, `fact_anchor`, `recipe`, `failure_pattern`,
      `ephemeral_repair_trace`, `generic_dialogue`) instead of relying only on
      source/category inference
    - the AUDN-lite mutation service now calls the memory-quality governor
      before recall/add/update/delete, so consolidation, explicit learning,
      precedent writes, and failure-pattern writes share the same durable-write
      stop-line
    - consolidation now asks the compacting model for a typed
      `memory_update` object; old string-shaped compact outputs still parse
      for history safety, but are classified as `generic_dialogue` and rejected
      before durable memory mutation
    - generic dialogue and ephemeral repair traces are rejected before durable
      memory mutation, while specific project/runtime task-state updates remain
      writable
    - graph extraction now requires a durable typed consolidation update
      (`preference`, `task_state`, or `fact_anchor`) before it runs; ordinary
      dialogue, `generic_dialogue`, `null`, and legacy string-shaped updates do
      not enter the entity/relationship graph path
    - generic concept hygiene now relies on extractor type and relationship
      endpoint/confidence gates rather than language-specific role-name,
      suffix, or phrase filters
    - provider-facing current-session context now uses a bounded
      relevant/head/tail selector instead of tail-only chat replay, so explicit
      early and late anchors from the active long dialogue can survive when the
      persistent session search intentionally excludes the current session
    - current-session relevance scoring uses corpus-weighted query terms from
      the active session history; it does not carry a built-in
      language-specific token blacklist or phrase-specific anchor rules
    - focused cheap-route long-dialogue semantic regression passed after the
      selector fix:
      - session: `phase410-long-semantic-focused-1775951001`
      - report: `/tmp/synapseclaw-long-semantic-1775951001`
      - retained anchors: `freedom`, `responsibility`, `joy`, `alignment`
      - provider context rows: 21
      - embedding rows: 55
      - compaction rows: 0 because the live route stayed inside its effective
        context budget
  - remaining:
    - strengthen repetition-aware policy beyond the current lexical first pass
      for broader low-information paraphrase loops
    - continue auditing retrieval ranking for concept-heavy sessions after real
      compaction, because the passing focused run did not need a compaction row
    - keep generic world-knowledge relationships and generic consolidation
      `memory_update` outputs as regression cases for memory hygiene, alongside
      pure-dialogue graph extraction bypasses
- expected outcome:
  - better retrieval quality
  - less memory pollution
  - more trustworthy long-dialogue behavior

### Slice 17

- upgrade target:
  - complements Slice 13 pressure handling and Slice 10 route switching
  - provides a typed bridge when compaction or reroute is not enough
- add structured session handoff packets instead of relying only on prose summaries
- support handoffs for:
  - large-window -> small-window route switches
  - web -> Matrix/Telegram/channel transitions
  - main agent -> helper agent delegation
  - session resume after critical compaction or overflow-risk refusal
- keep the packet typed and bounded:
  - active task
  - commitments
  - unresolved questions
  - current defaults/targets
  - relevant recent failures or cautions
- use this as the preferred bridge when a route switch or context budget does
  not allow continuing with the full active context
- current status:
  - landed first pass:
    - shared domain `session_handoff` service builds a bounded packet from typed
      turn interpretation, admission repair hints, recalled anchors, session
      matches, and run recipes
    - `turn_context` includes the formatted packet in the shared provider-facing
      resolution context, so web/channel call paths can consume the same shape
      instead of diverging prompt prose
    - blocked admission responses now surface the same handoff packet for
      domain use-case and live-agent paths
    - helper-agent delegation now accepts an optional strict `handoff_packet`
      object and prepends the bounded shared packet before `[Context]` / `[Task]`
    - `agents_spawn` now accepts the same optional strict `handoff_packet`
      object and prepends it before the spawned agent task in both legacy and
      broker-backed modes
  - remaining:
    - add live route-downgrade validation with large-window -> small-window
      model switches after the long-dialogue semantic pack is green
- expected outcome:
  - safer cross-model and cross-channel continuity
  - less reliance on free-form summaries
  - better recovery from aggressive compaction or route downgrades

### Slice 18

- upgrade target:
  - enriches Slice 12 registry rather than replacing config/catalog defaults
  - removes more adapter heuristics without putting live probing on the hot path
- add a background capability/profile probe to improve candidate metadata without
  putting discovery on the hot path
- probe and cache, best-effort:
  - context window
  - max output
  - tool-calling support
  - multimodal/image/audio/media support
  - continuation/prompt-caching hints when providers expose them
- keep provider probing adapter-local and asynchronous:
  - no per-turn live probing
  - cache by `provider + model + runtime profile`
  - expose freshness/provenance in route inspection
- let this enrich Slice 12 model-profile resolution rather than replacing
  explicit config or bundled defaults
- current status:
  - landed:
    - bundled `model_catalog.json` seeds presets, provider defaults, route
      aliases, curated models, pricing, and provider:model profile metadata
    - local user `model_catalog.json` can be materialized with
      `synapseclaw models catalog init` and is merged over the bundled catalog
    - cached provider catalog metadata can supply context window, max output,
      and feature profile data when fresh
    - `/model` and `/models` surface profile provenance, freshness/confidence,
      current route limits, feature coverage, native context policy, and cache
      stats
    - `models refresh` supports the current provider set, including OpenRouter,
      DeepSeek, xAI, GLM/Z.AI, Qwen, Gemini, Anthropic, OpenAI, and local
      OpenAI-compatible runtimes
    - OpenRouter Gemma paid routes and Grok 4.20 are catalog-driven optional
      routes, not Rust hardcoded defaults
    - live model cache entries are now scoped by normalized endpoint in
      addition to provider name
    - runtime model-profile lookup, web route inspection, channel routing, and
      onboarding cache operations all consume the same endpoint-aware cache key
    - cached profile metadata from one endpoint no longer leaks into the same
      provider/model when another endpoint is configured
    - generic OpenAI-compatible `/models` refresh now persists common
      provider-exposed context-window and max-output metadata when endpoints
      expose fields such as `context_length`, `max_context_length`,
      `max_model_len`, `max_input_tokens`, `max_output_tokens`, or
      `max_completion_tokens`
    - provider context-window error classification is now centralized in the
      provider adapter layer and reused by reliable-provider retry/fallback
      suppression plus the channel runtime `AgentRuntimeErrorKind` mapping
    - failed-turn context-limit observations now flow through a typed domain
      port into the endpoint-aware model cache when the provider error exposes
      a trustworthy lower context window; the cache repair only lowers or fills
      unknown context windows, never raises them
    - web `Agent` and channel `AgentRuntimePort` now record those observations
      through the same `WorkspaceModelProfileCatalog` cache path
  - still open after Hermes source audit:
    - no models.dev-style provider-aware registry source yet
    - no explicit probe-down tier strategy for unknown/local endpoints yet
  - status note:
    - treat Slice 18 as partial: catalog/cache/refresh/endpoint-aware lookup are
      landed, but external registry ingestion and explicit unknown-endpoint
      probe-down strategy are not complete.
- Hermes-derived model-window resolver follow-through:
  - keep explicit user override first
  - completed: persistent live model cache lookup is now endpoint-aware, so the
    runtime no longer assumes the same model id has the same window everywhere
  - use live endpoint metadata where providers expose useful `/models` fields
    such as `context_length`, `max_context_length`, `max_model_len`,
    `max_input_tokens`, or `max_output_tokens`
  - completed: infer provider identity from catalog-owned base URLs when a
    custom endpoint is configured, so provider-aware metadata can still be used
  - completed: parse context-limit errors only in adapter-local provider code
    and convert them into typed runtime route-repair hints
  - completed: persist failed-turn context-limit observations as typed
    endpoint-aware profile/cache updates when a trustworthy lower window can be
    inferred
  - keep broad hardcoded family fallbacks out of core Rust; if unavoidable,
    they belong in local/bundled catalog data with provenance and freshness
  - consider a probe-down tier strategy only as an explicit adapter fallback
    for unknown/local endpoints, never on the hot path for known providers
- expected outcome:
  - better automatic candidate knowledge
  - fewer wrong assumptions for aggregator/native provider differences
  - less dependence on hardcoded adapter heuristics

### Slice 19

- upgrade target:
  - builds on Slice 15 self-repair, Slice 17 handoff packets, and Slice 11 admission
  - makes runtime assumptions explicit instead of leaving them implicit in prompt prose
- add a first-class assumption tracker for active runtime hypotheses such as:
  - resolved delivery target
  - active weather-city / local-timezone facts
  - currently trusted credential or auth profile
  - expected tool capability / route capability
  - assumed current task / branch / workspace anchor
- every assumption must carry:
  - source
  - freshness
  - confidence
  - invalidation trigger
  - replacement path if disproved
- use assumptions in self-repair:
  - failures should invalidate or downgrade the exact broken assumption
  - handoff packets should carry active assumptions explicitly
- keep assumptions bounded and mostly ephemeral:
  - scoped to active sessions/routes unless promoted by a separate policy
  - aggressively evicted when stale or contradicted
- current status:
  - landed:
    - `runtime_assumptions` domain service models bounded typed runtime
      assumptions with kind/source/freshness/confidence/invalidation/replacement
      path
    - assumptions are derived from structured turn interpretation and recent
      route admission/repair state, not prompt phrase matching
    - `SessionHandoffPacket` carries bounded assumptions so route switches,
      compaction, and fresh handoff surfaces can preserve current hypotheses
      explicitly
    - helper-agent handoff schemas accept the same strict assumption objects
    - bounded session/runtime assumption ledger now merges observed assumptions
      and downgrades challenged assumption kinds after runtime/provider/tool
      failures
    - channel runtime persists the assumption ledger on `RouteSelection`; web
      runtime keeps the same ledger on the live `Agent` and exposes it through
      route inspection
  - still open:
    - promotion from ephemeral assumptions into durable memory remains
      intentionally blocked until a separate policy gate exists
- expected outcome:
  - less hidden guesswork
  - cleaner failure diagnosis
  - fewer repeated retries on already-broken assumptions

### Slice 20

- upgrade target:
  - builds on Slice 19 assumption tracking and Slice 16 memory-quality policy
  - adds a typed knowledge-state layer rather than relying on undifferentiated “memory”
- add an epistemic state model so the system distinguishes:
  - `known`
  - `inferred`
  - `stale`
  - `contradictory`
  - `needs_verification`
  - `unknown`
- make epistemic state explicit for both runtime and memory-backed facts:
  - route/model capabilities
  - delivery/runtime defaults
  - retrieved anchors
  - external facts and recency-sensitive knowledge
- require freshness/confidence/source on epistemic entries
- let admission, retrieval, and self-repair consume epistemic state directly
  instead of treating all recalled facts as equally trustworthy
- current status:
  - landed:
    - `epistemic_state` domain service defines a typed bounded state model for
      runtime assumptions, model-profile facts, and memory entries
    - model-profile context-window facts now map source/freshness/confidence
      into `known`/`inferred`/`stale`/`needs_verification`/`unknown`
    - memory recall lines surface `state/source/confidence` metadata next to
      anchors so provider-facing recall is no longer undifferentiated memory
    - retrieval reranking and resolution-plan evidence consume epistemic
      memory state, so weak/needs-verification memory is penalized before it
      can outrank stronger anchors
    - domain tests cover stale model-profile facts and low-confidence memory
      requiring verification, plus epistemic rerank and resolution-score
      adjustment
  - still open:
    - self-repair does not yet consume epistemic state directly beyond the
      runtime assumption ledger freshness/confidence fields
    - delivery/runtime defaults and external recency-sensitive facts are not
      fully projected through the same epistemic surface yet
- expected outcome:
  - fewer overconfident wrong decisions
  - better contradiction handling
  - more honest “I know / I infer / I need to verify” behavior

### Slice 21

- upgrade target:
  - builds on Slices 15, 18, 19, and 20
  - adds a background diagnostic layer without pushing more prompt/state into the hot path
- add a background watchdog plus compact world-state digest that monitors:
  - repeated tool failures
  - route/candidate degradation
  - stale capability profiles
  - rising context pressure
  - retrieval pollution
  - channel health / recent delivery failures
  - memory backend / embedding backend degradation
- watchdog outputs must be typed and bounded:
  - compact alerts
  - degraded subsystem flags
  - recommended repair action
  - freshness / last-seen timestamps
- keep watchdog state ephemeral and separate from durable memory
- expose the digest in operator/runtime inspection and as bounded runtime context
  only when relevant
- current status:
  - landed:
    - `runtime_watchdog` domain service builds a bounded typed digest from
      route admissions, tool repair traces, challenged runtime assumptions,
      context-cache pressure, and generic subsystem observations
    - watchdog alerts carry typed subsystem/severity/reason/recommended action
      and dedupe/truncate before leaving the domain service
    - `/model` and `/providers` runtime help both use the same adapter-core
      watchdog renderer, so web/channel command surfaces do not fork the logic
    - domain and adapter tests cover context overflow, challenged assumptions,
      repeated tool failures, metadata refresh guidance, and shared provider
      help rendering
  - still open:
    - no autonomous background polling loop is wired yet
    - live memory/embedding/channel health observations are not injected into
      the digest outside explicit callers yet
    - bounded runtime-context injection is not enabled yet; current exposure is
      operator/runtime help only
- expected outcome:
  - better self-diagnosis without prompt bloat
  - earlier detection of degraded subsystems
  - fewer surprises during long-running agent sessions

### Slice 22

- upgrade target:
  - builds on Slices 15, 19, and 20
  - upgrades one-shot repair into measured calibration and retrospective improvement
- add a confidence ledger plus counterfactual review for:
  - route choice
  - tool choice
  - retrieval choice
  - delivery confidence
- compare predicted success against actual outcomes:
  - “expected to work” vs “failed”
  - “insufficient confidence” vs “succeeded anyway”
- store only compact typed outcome comparisons, not full reflective essays
- use this ledger to:
  - suppress repeated bad choices
  - improve future repair suggestions
  - surface low-confidence paths before they fail noisily
- current status:
  - landed:
    - `runtime_calibration` domain service records compact typed route/tool/
      retrieval/delivery outcome comparisons
    - high-confidence failures become `overconfident_failure` records with a
      suppress-choice recommendation
    - low-confidence successes become `underconfident_success` records that can
      be kept as positive evidence
    - calibration history is TTL-bounded, count-bounded, and deduped by
      decision kind/signature/comparison
    - live `Agent` sessions now keep an ephemeral calibration ledger, cleaned by
      the runtime trace janitor
    - provider route-call success/failure and tool-call success/failure now emit
      typed calibration observations without promoting them into durable memory
    - `/model` and `/providers` now expose the compact calibration ledger through
      the shared runtime route help path
    - route/tool suppression now consumes typed calibration suppression keys:
      - route admission can reroute away from a recently overconfident failing
        route when a safe same-lane alternative exists
      - tool narrowing can hide a recently overconfident failing tool when a
        same-role alternative exists
  - still open:
    - live retrieval/delivery-specific call sites do not emit dedicated
      calibration observations yet
- expected outcome:
  - more calibrated runtime decisions
  - better post-failure learning without turning every turn into a reflection step

### Slice 23

- upgrade target:
  - supports Slices 15, 16, 19, 20, 21, and 22
  - prevents the new typed traces from turning into a second junk memory system
- add a background janitor for ephemeral runtime traces:
  - self-repair ledgers
  - assumption records
  - watchdog alerts
  - calibration outcomes
  - short-lived handoff artifacts
- janitor responsibilities:
  - TTL expiry
  - dedupe
  - bounded-count eviction
  - compacting repeated failure classes
  - promotion only through explicit policy gates
- current status:
  - landed:
    - `runtime_trace_janitor` domain service cleans short-lived tool-repair,
      watchdog, calibration, and session-handoff artifacts with TTL/dedupe/count
      bounds
    - runtime assumptions are re-bounded through the existing typed assumption
      ledger, without inventing schema-specific user fields
    - repeated tool-failure classes, challenged assumptions, critical watchdog
      alerts, and overconfident calibration failures become typed promotion
      candidates behind explicit gates only
    - `Agent::turn` invokes the janitor through the shared runtime path, so web
      and channel sessions do not fork cleanup behavior
  - still open:
    - there is no independent timer thread yet; cleanup is lazy per turn
    - calibration/watchdog/handoff histories are cleaned when a caller provides
      them, but only tool-repair and assumption ledgers are currently stored on
      the live `Agent`
- expected outcome:
  - bounded metacognitive state
  - less self-generated noise
  - cleaner long-running behavior

### Slice 24

- runtime adapter contract and command parity hardening:
  - builds on Slice 11 route-state follow-through, Slice 13 context
    budget/preflight, and Slice 15 route diagnostics
  - prevents web/channel drift by routing runtime commands through one
    adapter-core executor while preserving separate transport/lifecycle adapters
- implementation:
  - `RuntimeAdapterContract` describes the web/channel surface, transport,
    lifecycle, capabilities, and shared decision ownership
  - `RuntimeCommandHost` exposes typed host hooks for provider/model help
    snapshots, provider/model route mutations, and session clear
  - `execute_runtime_command_effect` owns common alias canonicalization,
    command-effect execution, switch success/failure/block rendering, and
    provider/model help rendering from typed snapshots
  - adapters own only lifecycle-specific effects: provider initialization,
    live-agent route mutation, inbound-session route state, and transport clear
    semantics
  - channel provider route mutation happens after adapter validation and
    canonicalization, not in the pre-validation domain command classifier
  - channel clear-session and model-switch mutation happen in the adapter
    command host, keeping domain command parsing/effect construction
    side-effect-light
  - shared domain helpers own route diagnostic reset and provider-history
    budget extraction, so web/channel follow-through does not copy those rules
  - conformance tests reject adapter-local runtime-command presentation strings
    and cover typed help rendering through the common executor
- invariant:
  - web/channel adapters must not render runtime command success/error text
    directly
  - web/channel adapters must not fork `/providers` or `/model` help rendering
  - new command behavior must enter through domain services or adapter-core
    contract hooks before being wired into concrete adapters
- expected outcome:
  - less web/channel feature drift
  - clearer hexagonal boundary between runtime decisions and adapter lifecycle
  - safer future extraction of route preflight, context budget diagnostics, and
    command effects into shared ports/services

### Slice 25

- tool notification mapper parity:
  1. extract observer-event to tool-notification interpretation into adapter-core
  2. keep web JSON payload shape transport-specific and backward-compatible
  3. keep channel text payload shape transport-specific and backward-compatible
  4. preserve web duplicate suppression through shared notification signatures
  5. preserve channel `tools_used` lifecycle state outside the shared mapper
  6. centralize safe argument/output preview truncation for tool notifications
  7. cover mapper behavior with shape and UTF-8 safety tests
  8. leave SSE/global observability as a later extension unless it starts
     sharing web/channel lifecycle semantics
- invariant:
  - web/channel observers must not reinterpret raw tool events independently
  - shared mapper must not own transport delivery, session lifecycle, or agent
    state mutation
- expected outcome:
  - fewer hidden web/channel notification divergences
  - cleaner adapter split between event semantics and transport mechanics

### Slice 26

- web/channel extraction debt hardening:
  1. move shared system prompt/bootstrap construction out of `channels`
  2. keep existing `crate::channels::build_system_prompt*` call sites
     backward-compatible through re-export only
  3. move tool narration and isolated tool JSON artifact cleanup out of
     `channels`
  4. move provider-history normalization/trimming helpers out of `channels`
  5. move runtime tool-notification observer forwarding out of web/channel
     while keeping transport payload rendering in adapter sinks
  6. keep transport lifecycle code separate for web socket RPC and channel
     listeners
  7. keep channel-only supervision/typing code separate unless a second
     transport needs it
  8. leave summary/run-lifecycle unification as a typed service follow-up
     unless it can be done without crossing store/lifecycle boundaries
  9. add tests/guards that prove extraction preserves prompt, sanitizer, and
     history hygiene behavior
- invariant:
  - `channels/mod.rs` must not be the home for shared runtime prompt or tool
    artifact semantics
  - moved helpers must not gain web/channel side effects
- implementation status:
  - runtime system prompt/bootstrap lives in adapter-core runtime prompt module,
    with channel re-exports kept only for backward-compatible call sites
  - tool artifact cleanup and provider-history hygiene live outside
    `channels/mod.rs`
  - tool notification event interpretation and observer forwarding are shared;
    web/channel retain only transport-specific JSON/text sinks
  - channel health supervision helpers live outside `channels/mod.rs`
  - summary/run-lifecycle unification is intentionally left as a typed service
    follow-up because the current web and channel stores/lifecycles are not the
    same boundary
  - status note:
    - Slice 24-26 are code-closed for the current extraction/parity target.
    - Any future web/channel run-lifecycle unification should be planned as a
      separate typed service slice, not reopened as hidden Slice 26 scope creep.
- expected outcome:
  - less channel-monolith drift
  - clearer adapter-core seams for prompt assembly and response cleanup
  - safer next step toward a shared run lifecycle service

---

## Validation

### Context economy checks

- ordinary memory-setting turns should not read bootstrap docs
- provider-facing payload should stay bounded across tool iterations
- same turn with 2-3 tool cycles should not re-send giant historical ballast
- condensed artifacts should be reused until their source inputs change

### Everyday-flow checks

- `Atlas` / `Borealis` working-chain isolation
- recency override (`hotfix-17` -> `hotfix-18`)
- Matrix target resolution without workspace archaeology
- weather/time using the correct user-profile weather-city fact

### Language checks

- Chinese / Japanese / Korean working-chain turns
- preference update with non-Latin location names
- no UTF-8 trimming crashes

### Provider checks

- the phase-close live pack is now the mandatory provider/capability/context harness:
  - `dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh`
  - it records per-case JSON, provider-context TSV, systemd journal slices, embedding
    signals, compaction signals, and admission signals under a report directory
  - multimodal-understanding checks now assert both content and prompt hygiene:
    - the default test image is a 16x16 white PNG
    - a vision-capable route should answer `White`
    - a vision-capable route must not fall into memory/workspace/web tool archaeology
- compact replay works on every provider
- OpenRouter/native/provider-specific routes may expose different context windows,
  max output, or features for the same model family; candidate metadata must
  capture those differences
- OpenRouter Gemma paid-route smoke covers standard optional aliases
  `gemma31b` and `gemma26b`; keep them available for tests/manual routing but
  do not make them default.
- direct DeepSeek provider validation should use official API model ids:
  - `deepseek-chat`
  - `deepseek-reasoner`
  OpenClaw's bundled provider plugin uses the same OpenAI-compatible direct
  provider shape and does not expose a special direct `deepseek-v4` model id.
- no provider regresses because continuation is optional
- OpenAI-family providers can opt into `previous_response_id`-style chaining
  only when the adapter advertises support
- shared runtime accepts only the canonical tool protocol
- any provider-specific protocol shim is adapter-local, never shared-runtime
- cheap-route live delivery and profile mutation smoke still work under the
  strict protocol, or fail honestly as provider drift rather than being
  silently recovered by shared-runtime alias parsers

### Capability-routing checks

- lane candidates resolve in order:
  - default candidate first
  - later candidates available as fallback/manual alternatives
- automatic candidate metadata is used when cached/provider catalogs have it
- manual candidate profile overrides win over auto-resolved metadata
- `big-window -> small-window` route switch:
  - mandatory regression
  - switch from a larger-window candidate to a smaller-window candidate
  - verify target-aware preflight triggers compaction or blocks the switch
    before the provider is called
- feature gating:
  - a default reasoning candidate without vision must not be used for image turns
  - a multimodal or image-generation candidate must be selected only when its
    capability metadata says it can handle the request

### Guardrail checks

- turn admission is explicit before provider call:
  - the runtime records the resolved turn intent
  - the runtime records the required capability lane(s)
  - the runtime records why a candidate was admitted, rerouted, or rejected
- overflow prevention:
  - for a current provider-facing payload near the target candidate window,
    preflight must move the turn into `warning`, `critical`, or `overflow_risk`
    rather than calling the provider blindly
  - a `big-window -> small-window` switch with an oversized current context
    must either compact first or refuse the switch
- wrong-model prevention:
  - text-only candidates must be blocked from image/audio/video/music turns
  - non-tool candidates must be blocked from tool-heavy turns
  - embedding-only candidates must never be selected for reasoning turns
- observability:
  - admission outcome and context-pressure state must be visible in logs / runtime
    inspection without reading prompt internals

### Model-profile checks

- effective model profile provenance is inspectable:
  - manual candidate override
  - local `model_catalog.json`
  - cached provider catalog
  - bundled catalog
  - adapter fallback
- same model family through different providers can produce different effective
  capabilities/windows without colliding
- unknown capability or unknown window data does not silently pass as “good enough”
- adapter-side heuristics continue to shrink as structured profile data grows

### Modality-routing checks

- image/audio/video/music turns resolve through explicit lanes rather than
  falling back to generic reasoning by accident
- live-pack modality tests must use structured markers, not keyword guessing:
  - `[IMAGE:...]` for multimodal understanding
  - `[GENERATE:IMAGE]`, `[GENERATE:AUDIO]`, `[GENERATE:VIDEO]`,
    `[GENERATE:MUSIC]` for generation lanes
- media markers should narrow provider-visible tools to zero unless a dedicated
  adapter-local media tool is explicitly selected; generic memory/search/workspace
  tools must not be exposed just because the model can call tools
- presets and local catalogs can seed modality lanes without changing core logic
- a request for media generation on a text-only route fails early and clearly
- multimodal-understanding, image generation, and audio generation each show a
  deterministic admission outcome before execution

### Self-repair checks

- after a route or tool failure, runtime records a compact typed explanation:
  - what failed
  - why it failed
  - what alternative was chosen or recommended
- the assistant can answer “why did you choose that?” or “why did it fail?”
  from structured recent state rather than recomputing or hallucinating
- repeated tool failures of the same class are suppressed:
  - no immediate retry loop on the same rejected schema/permission/auth error
- self-repair state is ephemeral:
  - retained for a short TTL (target default: 48 hours)
  - bounded by count and age
  - not treated as durable user/profile memory by default

### Assumption-tracking checks

- active runtime assumptions are inspectable and typed:
  - source
  - freshness
  - confidence
  - invalidation trigger
- when a failure disproves an assumption, the exact assumption is downgraded or invalidated
  rather than leaving the runtime in an ambiguous state
- repeated failures do not continue to rely on already-invalid assumptions

### Epistemic-state checks

- recalled/runtime facts are not all treated as equally trustworthy
- the runtime can distinguish at least:
  - `known`
  - `inferred`
  - `stale`
  - `contradictory`
  - `needs_verification`
- contradictory or stale facts produce conservative routing/tool decisions
  instead of confident reuse

### Memory-quality checks

- generic abstract conversation lines do not become durable memory anchors
- typed anchors survive compaction better than precedent/daily noise
- ephemeral repair traces stay ephemeral unless separately promoted
- long pure-dialogue runs do not create fake procedural skills or junk graph nodes

### Handoff checks

- a forced route downgrade can emit a structured handoff packet instead of only
  prose summary
- helper-agent delegation can receive the bounded handoff packet without replaying
  the full parent context
- channel transitions can preserve active commitments/defaults through the packet

### Watchdog checks

- background watchdog state is bounded and does not sit on the hot path
- repeated degradation signals produce compact alerts rather than unbounded trace spam
- world-state digest can surface:
  - degraded route/candidate health
  - stale capability metadata
  - repeated tool failures
  - channel/delivery degradation

### Calibration checks

- route/tool confidence can be compared with actual outcomes
- repeated low-quality choices become less likely after counterfactual/outcome capture
- calibration records remain compact and ephemeral, not a second transcript

### Janitor checks

- ephemeral metacognitive traces obey TTL/count limits
- duplicate repair/assumption/watchdog/calibration records collapse cleanly
- janitor cleanup does not delete durable memory or active handoff artifacts prematurely

### Capability-probe checks

- background probe results enrich cached candidate metadata without blocking turns
- probe provenance/freshness is visible in route inspection
- missing probe data degrades conservatively rather than optimistically

### Condensation checks

- older dialogue chunks can be summarized without losing active commitments,
  defaults, or unresolved tasks
- large docs/files are condensed once and reused until they change
- the cheap summarizer path does not overwrite or distort typed runtime state

### Long-dialogue semantic checks

- mandatory regression:
  - run a long pure-dialogue session with no operational task, no file work,
    and no external side effects
  - example shape:
    - 20-40 turns of ordinary discussion such as meaning-of-life /
      philosophy / personal reflection
- measure provider-facing context size during the run:
  - before compaction
  - after compaction
  - near the end of the dialogue
- verify semantic retention:
  - early points from the first quarter of the conversation are still
    answerable near the end
  - late points from the final quarter are also answerable
  - the provider-facing context stays within budget after compaction rather
    than growing linearly with turn count
- current empirical status after Slice 16/17 close validation:
  - mechanics: pass
    - long cheap-route dialogue stayed alive through 20 turns
    - provider-facing history stayed compact through bounded current-session
      relevant/head/tail selection
    - procedural skill promotion did not appear in the transcript
  - semantic retention: pass on focused cheap-route live run
    - report: `/tmp/synapseclaw-long-semantic-1775951001`
    - final answer retained both the early anchor (`freedom` +
      `responsibility`) and late anchor (`joy` + `alignment`)
    - provider context rows: 21
    - embedding rows: 55
    - compaction rows: 0 because the selected live route stayed within budget
    - treat the remaining risk as a real-compaction / route-downgrade ranking
      validation item, not a known failure of the long-dialogue selector
- verify memory / embedding behavior:
  - episodic or semantic recall anchors may be created
  - stable user-preference learning is acceptable if the user actually states
    a durable preference
  - procedural recipe / workflow skill promotion should *not* trigger from a
    pure philosophical conversation
  - if such a dialogue produces operational skills, run recipes, or similar
    procedural artifacts, treat that as a memory-quality bug
- verify no hidden degradation:
  - after the long-dialogue run, re-run a normal everyday-flow regression and
    confirm recall and defaults still behave correctly
- live-pack execution:
  - use `RUN_HEAVY=1 dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh`
    only at slice-close points
  - the default pack intentionally records compaction/embedding signals but does
    not force the expensive long dialogue

---

## Acceptance Criteria

Phase 4.10 is successful when:

1. normal turns do not depend on eager workspace-doc bootstrap
2. provider-facing context is compact and inspectable
3. implicit defaults are resolved structurally, not guessed
4. project-context docs load only when relevant
5. CJK and multilingual everyday flows remain stable
6. the runtime becomes cheaper and less noisy without losing memory quality
7. cheap-model condensation reduces payload size without changing correctness
8. provider/model selection is guarded structurally before execution rather than
   relying on prompt steering and late provider errors

See also:

- [Phase 4.10 Audit](./ipc-phase4_10-audit.md)
