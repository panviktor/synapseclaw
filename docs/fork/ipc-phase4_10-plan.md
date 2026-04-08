# Phase 4.10: Context Engine, Prompt Economy & Progressive Loading

Phase 4.9: self-learning, skill evolution & memory quality | **Phase 4.10: context engine, prompt economy & progressive loading** | next: runtime default-resolution and provider continuation polish

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

In short:

```text
stable bootstrap snapshot
+ compact provider shadow history
+ progressive project context discovery
+ deterministic typed defaults
+ cheap-model condensation
+ context observability
= smarter and cheaper runtime turns
```

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

- weather/time without city -> `user_profile.default_city`
- “send it there” -> `default_delivery_target` or `recent_delivery_target`
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

- landed:
  - Slice 1: provider-facing context accounting and observability
  - Slice 2: typed implicit delivery-target resolution through runtime state
- next:
  - condensation primitives for older dialogue, large docs, and repo brief
  - broader typed default resolution (`default_city`, workspace/resource anchors)
  - progressive scoped instruction loading

### Slice 1

- document Phase 4.10
- formalize provider-facing context snapshot
- add context-size observability

### Slice 2

- resolve implicit delivery target from typed turn state instead of prompt prose
- expose per-turn defaults through a scoped runtime context port
- wire `message_send` to prefer recent delivery target, then profile default

### Slice 3

- implement condensation primitives for:
  - older multi-turn chat segments
  - large doc/file summaries
  - project brief / repo brief

### Slice 4

- implement deterministic default resolution for:
  - weather/time city
  - implicit delivery target
  - “there” follow-ups

### Slice 5

- add progressive project-context discovery
- cache nearest-scope instruction files per session

### Slice 6

- add provider-native continuation support where it genuinely helps

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
- weather/time using the correct profile default city

### Language checks

- Chinese / Japanese / Korean working-chain turns
- preference update with non-Latin location names
- no UTF-8 trimming crashes

### Provider checks

- compact replay works on every provider
- no provider regresses because continuation is optional
- OpenAI-family providers can opt into `previous_response_id`-style chaining
  only when the adapter advertises support

### Condensation checks

- older dialogue chunks can be summarized without losing active commitments,
  defaults, or unresolved tasks
- large docs/files are condensed once and reused until they change
- the cheap summarizer path does not overwrite or distort typed runtime state

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

See also:

- [Phase 4.10 Audit](./ipc-phase4_10-audit.md)
