# Phase 4.10 Audit — External Agent Patterns vs SynapseClaw

Date: 2026-04-08

Purpose:
- compare real agent products, not just our own code
- decide what SynapseClaw should copy, adapt, or reject for context handling
- re-audit our current runtime after the first 4.10 prompt-economy slice

Related:
- [Phase 4.10 Plan](./ipc-phase4_10-plan.md)
- [Live Runtime Validation — 2026-04-08](./live-runtime-validation-2026-04-08.md)

---

## Executive Read

The common pattern across stronger agent products is not:

```text
keep replaying more markdown forever
```

It is closer to:

```text
stable prefix
+ compact or cached continuation
+ progressive context loading
+ explicit observability
```

SynapseClaw already has an advantage those products often lack:

- typed user profile
- typed dialogue state
- typed delivery targets
- typed procedural memory and skills
- structured memory projections

So our best move is not to imitate their prompt files literally.

Our best move is:

1. keep bootstrap small and stable
2. use typed runtime to resolve defaults before the model guesses
3. keep provider-facing history compact
4. load project instructions progressively by scope
5. expose context size and contributors like a first-class runtime surface

## 2026-04-12 Follow-Up

- Runtime model switching now follows the same typed selector path in web and
  channel: effective capability lanes first, catalog aliases second, then an
  explicit unresolved model selector.
- Legacy route aliases / `embedding_routes` are no longer lane-resolution
  fallbacks for `resolve_lane_candidates`, summary routing, query-classifier
  overrides, or `/model` command effects.
- Matched lane candidates preserve `lane + candidate_index` through the shared
  runtime-command adapter contract, which keeps route state aligned with the
  candidate-profile registry instead of only storing provider/model strings.
- `/model` help now renders effective capability lanes and catalog aliases
  instead of promoting configured route aliases as a first-class runtime
  surface.
- `model_routing_config` no longer creates or removes editable
  editable route-table entries; scenario upserts now write the selected capability lane's
  ordered candidate list and classification rules resolve through the shared
  lane/catalog selector path.
- Live web/Agent query classification now follows the same lane/catalog
  selector resolver instead of maintaining an Agent-local route-alias
  hint/model map.
- Live Agent and CLI provider-router alias tables are now derived from the
  shared effective-lane/catalog helper instead of passing configured
  route-table directly into the router.
- Provider-router aliases now include typed `provider:model` keys for every
  lane candidate, preventing selector/router drift for non-primary candidates.
- Channel inbound/use-case route snapshots no longer carry the legacy route table
  through turn execution; they now pass lane/preset state only.
- Config/API/catalog surfaces now expose user-defined catalog shortcuts as
  `route_aliases`, with no old Rust route-table field or catalog compatibility
  alias left in the runtime code path.

---

## Product Patterns

### 1. OpenAI / Codex

Observed pattern:

- OpenAI’s Responses API supports chaining via `previous_response_id`.
- The Responses API separates stable instructions from per-turn input more
  cleanly than legacy chat-completions style message replay.
- OpenAI’s local-shell guide is explicitly tool-oriented and shaped around
  iterative external execution rather than giant free-form transcript replay.

What this implies:

- Codex-like systems can preserve conversational continuity without naively
  replaying the entire turn history every cycle.
- For OpenAI-family providers, provider-native continuation is a real
  optimization path, not just a theoretical one.

What we should take:

- provider continuation as an optional capability
- smaller provider input after the first tool cycle
- stable instruction prefix + incremental turn state

What we should not do:

- make the whole runtime depend on one provider-specific continuation model

References:

- <https://developers.openai.com/api/docs/guides/migrate-to-responses>
- <https://platform.openai.com/docs/guides/tools-local-shell>

### 2. Claude Code

Observed pattern:

- Claude Code loads layered memory files (`~/.claude/CLAUDE.md`,
  project `CLAUDE.md`, deprecated `CLAUDE.local.md`).
- It recursively discovers relevant memory files by directory scope.
- Nested subtree `CLAUDE.md` files are only included when files in that subtree
  are actually read.
- It provides `/compact` and `/memory`, and Anthropic exposes prompt caching and
  context editing features explicitly.

What this implies:

- progressive project-context discovery beats eager full bootstrap
- context maintenance is treated as a product capability, not hidden plumbing

What we should take:

- nearest-scope instruction discovery
- lazy subtree loading
- explicit compact/inspect primitives

What we should not do:

- reintroduce a Markdown-first memory model as the primary continuity substrate

References:

- <https://docs.claude.com/en/docs/claude-code/memory>
- <https://docs.claude.com/en/docs/claude-code/slash-commands>
- <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- <https://platform.claude.com/docs/en/build-with-claude/context-editing>

### 3. Cursor

Observed pattern:

- Cursor distinguishes rules that are `Always`, `Auto Attached`,
  `Agent Requested`, or `Manual`.
- It also distinguishes long-chat summarization from large-file condensation.
- Nested `.cursor/rules` directories scope instructions by folder.

What this implies:

- not every rule belongs in every prompt
- “available but not injected” is a useful first-class state

What we should take:

- instruction inclusion modes:
  - always-on bootstrap
  - auto-attached by scope
  - agent-requested/on-demand
- distinct handling for chat history vs large code/doc context

What we should not do:

- hide rule selection behind opaque prompt magic with no observability

References:

- <https://docs.cursor.com/agent/chat/summarization>
- <https://docs.cursor.com/en/context>

### 4. Aider

Observed pattern:

- Aider keeps a concise repository map rather than sending the whole repo.
- It sends only the most relevant portions of the repo map that fit the budget.
- It optimizes prompt caching around a stable prefix: system prompt, repo map,
  read-only files, editable files.

What this implies:

- a compact project brief can outperform repeated full-file loading
- project context should be budgeted and relevance-ranked

What we should take:

- a repo-brief / project-map artifact
- token-budget-aware project context
- explicit stable-prefix thinking for providers with caching

What we should not do:

- re-open large instruction files repeatedly when a compact structural summary
  would do

References:

- <https://aider.chat/docs/repomap.html>
- <https://aider.chat/docs/usage/caching.html>

### 5. OpenClaw

Observed pattern:

- OpenClaw has an explicit context-engine abstraction.
- Session pruning trims old tool results from the in-memory prompt but does not
  rewrite the transcript.
- `/context` reports sizes and top contributors instead of dumping raw prompt.

What this implies:

- context assembly should be replaceable and inspectable
- audit history and model-visible history are distinct products

What we should take:

- a first-class `ContextEngine` concept
- pruning / compaction as explicit lifecycle responsibilities
- operator-visible context accounting

What we should not do:

- copy the historical eager-bootstrap behavior literally

References:

- <https://docs.openclaw.ai/concepts/context-engine>
- <https://docs.openclaw.ai/concepts/context>
- <https://docs.openclaw.ai/concepts/session-pruning>

### 6. Hermes Agent

Observed pattern:

- Hermes documents context compression and prompt caching as part of agent-loop
  design.
- It uses progressive context-file discovery rather than dumping every context
  file on launch.

2026-04-11 source audit update:

- Hermes has a pluggable `ContextEngine` interface, but SynapseClaw should defer
  a pluggable engine layer until the domain pressure service/port boundary is
  stable.
- Hermes' compressor tracks provider-reported prompt/input tokens after
  responses and uses those tokens for future compression decisions; SynapseClaw
  now uses provider-reported input tokens as the next compaction pressure input
  when they are available.
- Hermes' gateway has a pre-agent session hygiene safety net for already-large
  transcripts; SynapseClaw now has a high-water web-session hygiene path and a
  channel provider-history compaction valve when admission already requires
  compaction.
- The channel compaction valve now rewrites persistent provider history through
  a message-history-only session replace contract, while preserving rolling
  session summaries. This keeps model-visible history compact without treating
  summaries as disposable transcript rows.
- Provider-call capability guards now live in a shared domain service rather
  than diverging between web and channel paths. Both web live Agent and
  channel/shared-loop provider calls evaluate image input against provider
  capabilities plus the resolved provider:model route profile.
- Hermes prunes old large tool results before summarization and sanitizes
  tool-call/tool-result pair integrity after compaction; SynapseClaw now does
  the same through typed placeholders and a protocol-aware sanitizer.
- Hermes resolves context windows through explicit config, persistent
  model+endpoint cache, live endpoint metadata, provider-aware registry lookup,
  and thin fallbacks; SynapseClaw now scopes cached live model/profile metadata
  by endpoint and infers provider identity from catalog-owned base URLs, while
  external provider-aware registry sources and adapter-local context-limit
  feedback remain future Slice 18 work.
- Hermes' progressive subdirectory hint tracker is useful, but its shell/path
  string extraction should not be copied into core policy; SynapseClaw should
  keep scoped context driven by typed tool facts and adapter-local fallbacks.

What this implies:

- compact continuation should be a default runtime concern, not a later cleanup
- project-context loading should be tied to actual file/task scope

What we should take:

- cache-friendly stable prefix
- progressive discovery
- compression as a core loop concern
- provider usage feedback for compaction
- pre-agent session hygiene
- cheap old-tool-result pruning before summary-lane calls
- post-compaction tool protocol sanitization
- endpoint-aware model context-window resolution

References:

- <https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop/>
- <https://hermes-agent.nousresearch.com/docs/developer-guide/context-compression-and-caching/>
- <https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files/>
- local source audit clone: `/home/protosik00/hermes-agent`

### 7. OpenCode

Observed pattern:

- OpenCode supports project and global `AGENTS.md` rules.
- It supports custom prompt files per agent, step limits, and per-agent
  permissions.
- It also keeps rules/config separate from permission policy.

What this implies:

- agent specialization belongs in explicit agent config, not in one giant shared
  prompt
- step limits and permission policy are part of context economy too

What we should take:

- better separation between agent identity and runtime policy
- per-agent context/policy envelopes

What we should not do:

- turn project rules into always-on universal ballast for every session

References:

- <https://opencode.ai/docs/agents/>
- <https://opencode.ai/docs/rules/>
- <https://opencode.ai/docs/permissions/>

### 8. ClawMem

Observed pattern:

- ClawMem explicitly warns that `session-bootstrap` can inject roughly 2000
  tokens before the user types anything.
- It favors precise context surfacing at the point of need.

What this implies:

- “load everything at startup” is known-bad even in adjacent ecosystems

What we should take:

- no unconditional session bootstrap dump
- retrieval/surfacing only when the task actually needs it

Reference:

- <https://yoloshii.github.io/ClawMem/>

---

## Re-Audit Of SynapseClaw

### 2026-04-12 Phase 4.10 Code Audit Update

Current status:

1. **Prompt/context architecture is materially stronger than the 2026-04-08 baseline**
   - provider-facing history is separated from audit history in the active
     runtime paths
   - compaction preserves protected head/tail and avoids splitting
     tool-call/tool-result groups
   - provider usage tokens can feed the next compaction pressure decision
   - condensed artifact cache is a shared persistent runtime service/port,
     visible to both web and channel diagnostics

2. **Shared runtime tool protocol is now strict**
   - shared text/XML tool-call fallback and dead presentation scrubbers were
     removed
   - raw `<tool_call>` output is treated as a defect unless a concrete provider
     adapter owns a narrowly-scoped normalization path
   - GLM/minimax/perl/XML-style dialects are not common-runtime features

3. **Model/provider request knowledge moved further out of Rust hot paths**
   - fixed OpenAI temperature rules and reasoning-effort aliases now resolve
     through `model_catalog.json` request policies
   - OpenAI, OpenAI Codex, OpenRouter, and generic OpenAI-compatible request
     controls no longer depend on broad model-family match arms
   - configured OpenAI-compatible `/models` refresh endpoints now derive from
     the effective `api_url`, so native/aggregator/local endpoints do not share
     stale default-provider discovery assumptions

4. **Hermes parity is closer on context safety**
   - model context is treated as input plus reserved output
   - compression threshold and hard ceiling scale by trusted window ratio
   - large-window to small-window switches preflight against the target route
   - web/channel model-switch and command semantics share the same runtime
     command/effect/preflight surfaces where lifecycle allows

5. **Memory hygiene is substantially better but still not fully done**
   - old `Children learn_from Parents`-style generic graph pollution is blocked
     through governor/extractor gates rather than phrase lists
   - raw conversation autosave and consolidation now share one governor path
   - remaining risk is concept-heavy long-dialogue ranking after real
     compaction, not the already-fixed string-pattern examples

Still open after this audit:

1. **No pluggable `ContextEngine` interface yet**
   - this is intentionally deferred until the current domain pressure
     service/port boundary is stable enough to avoid a parallel context system

2. **Provider-native continuation remains live-unvalidated**
   - `openai-codex` scaffolding exists, but the deployed Codex backend rejected
     `previous_response_id`; an official/custom key-based Responses endpoint
     still needs validation

3. **Slice 18 remains partial**
   - endpoint-aware cache and `/models` refresh are in place
   - no external models.dev-style registry source yet
   - no active probe-down request ladder for unknown/local endpoints yet

4. **Reasoning controls are policy-backed but not fully autonomous**
   - request controls now resolve from catalog request policies
   - fuller lane/cost-aware reasoning selection and provider-native
     `reasoning_details` preservation remain future work

5. **Long-dialogue quality still needs the expensive closeout run**
   - focused cheap-route semantic run passed without procedural skill growth
   - it did not produce a real compaction row, so compaction-plus-ranking still
     needs validation at a slice-close point

### What is already moving in the right direction

1. `MEMORY.md` is no longer part of runtime bootstrap or scaffolding.
2. Provider-facing history is smaller than before after the compact replay work.
3. Everyday memory turns no longer depend on reading `SOUL.md` / `USER.md` on
   every cycle.
4. CJK working-chain storage/recall works after the UTF-8 trimming fix.
5. Progressive scoped instruction loading exists in both web and channel paths.
6. Context pressure now consumes trusted model windows, provider-reported usage
   tokens, and endpoint-scoped cached model metadata when available.
7. Compaction now has Hermes-style hygiene for old large tool results and
   post-compaction tool-call/result protocol repair.
8. Web and channel provider-call capability checks now share one domain service
   for multimodal/image-input admission at the final provider-call boundary.

### What is still behind the better agent products

1. **No first-class context engine yet**
   - assembly behavior is still spread across runtime pieces instead of being a
     named lifecycle surface

2. **Progressive project-context discovery is first pass only**
   - scoped lazy rule loading exists, but there is not yet a repo/project brief
     that can replace file archaeology for broad repository questions

3. **No explicit repo/project brief**
   - we still fall back to file archaeology where a compact structural project
     brief or scoped instruction summary should exist

4. **Deterministic default resolution is still incomplete**
   - a dynamic weather-city profile fact can be recalled correctly but is not
     consistently applied on fresh weather/time turns
   - Matrix send can still prefer workspace archaeology over direct configured
     target routing

5. **Observability is still too weak**
   - we have local logging, but no durable `/context`-like operator surface for:
     - system/bootstrap size
     - provider-history size
     - recall/skills contributions
     - per-iteration payload growth

6. **Provider continuation is not implemented**
   - OpenAI-family providers still use compact replay, not native chained
     response state

---

## Best-Fit Architecture For SynapseClaw

The smartest fit is not “become Cursor” or “become Claude Code”.

It is:

### A. Stable bootstrap snapshot

Load once per session:
- identity
- persona
- safety/runtime policy

Do not re-inject raw bootstrap files every turn.

### B. Provider shadow history

Keep two artifacts:
- full audit transcript for UI/debug/learning
- compact provider-facing history for model calls

### C. Progressive scoped instructions

Support:
- root project rules
- nearest-scope nested rules
- agent-requested/on-demand rules

This should be closer to:
- Claude Code subtree memory discovery
- Cursor `Auto Attached` / `Agent Requested`

than to raw eager Markdown replay.

### D. Typed fact resolvers

Resolve before the model improvises:
- dynamic weather-location fact
- delivery target preference
- workspace/resource anchor
- likely channel or route

### E. Project brief / repo brief

Introduce a compact structural project summary that is cheaper than reopening
instruction files or wandering the workspace.

### F. Context observability

Expose something like:

- bootstrap chars
- provider-history chars
- current-turn chars
- tool-result chars
- recalled-memory chars
- scoped-instruction contributors

### G. Optional provider-native continuation

Implement after A-F, not before.

OpenAI-family providers can benefit first.
Everyone else keeps compact replay.

---

## Recommended 4.10 Implementation Order

1. **Freeze the architecture**
   - keep `MEMORY.md` removed
   - reject prompt-text default hacks
   - commit the 4.10 plan and this audit

2. **Lift context assembly into a named engine**
   - unify bootstrap, shadow history, pruning, compaction, and scoped instruction
     loading behind one runtime component

3. **Add context observability**
   - first logs
   - then operator/debug surface

4. **Implement deterministic default resolution**
   - weather/time city
   - delivery target
   - “there” / “back there” references

5. **Implement progressive rule loading**
   - scoped `AGENTS.md` / compatible rule discovery
   - session cache

6. **Add project brief / repo brief**
   - structural, budget-aware, reusable

7. **Add provider continuation**
   - only for providers where it is officially supported and actually reduces
     replay

---

## Short Conclusion

The best external agents do not win by stuffing more text into the prompt.

They win by:

- keeping the stable prefix stable
- deciding more outside the model
- loading context progressively
- compressing or continuing intelligently
- making context visible and measurable

That is exactly the right direction for SynapseClaw 4.10 too.
