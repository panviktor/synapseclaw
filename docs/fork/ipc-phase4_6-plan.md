# Phase 4.6: Agent Product Intelligence

Phase 4.5: pipeline hardening | **Phase 4.6: agent product intelligence** | next: TBD

---

## Problem

SynapseClaw already has a strong low-level runtime:

- unified prompt assembly
- structured memory learning
- channel/session intelligence
- multi-agent IPC
- a broad tool surface

But it still underperforms on some simple real user tasks because the product layer is too low-level.

Typical failure mode:

1. the user asks for a natural task like "after restart, report back here"
2. the agent decomposes it into raw plumbing (`cron_add`, shell, config search, room lookup)
3. it asks for data the runtime already knows
4. it promises actions before prerequisites are verified
5. it looks less capable than simpler systems with better product primitives

This is not primarily a model-quality problem. It is a **missing abstraction problem**.

The system already knows:

- current source adapter
- current conversation/session
- current reply target / room / chat
- current thread

But that context is not available as a first-class action target for scheduling, delivery, or proactive work. The agent therefore has to reconstruct obvious things with tools, which makes it look dumb.

---

## Research Basis

This phase is informed by the current behavior and public docs of:

- OpenClaw:
  - sessions are first-class and channel-scoped
  - session tools are explicit (`sessions_list`, `sessions_history`, `sessions_send`, `sessions_spawn`)
  - cron/system flows are gateway-owned
  - system events and heartbeat are product features, not prompt hacks
  - loop detection is a configurable runtime guardrail
- Hermes Agent:
  - ships with high-level orchestration tools like `todo`, `clarify`, `cronjob`, `send_message`, `session_search`
  - treats persistent memory as bounded and curated
  - separates always-available memory from session search
  - leans heavily on skills and task scaffolding, not just raw terminal/file tools

Useful source references:

- OpenClaw sessions: <https://docs.openclaw.ai/session>
- OpenClaw session tools: <https://docs.openclaw.ai/concepts/session-tool>
- OpenClaw system helpers: <https://docs.openclaw.ai/cli/system>
- OpenClaw cron hardening / `sessionTarget`: <https://docs.openclaw.ai/plans/cron-add-hardening>
- OpenClaw loop detection: <https://docs.openclaw.ai/gateway/configuration-reference>
- Hermes README: <https://github.com/NousResearch/hermes-agent>
- Hermes tools/toolsets: <https://hermes-agent.nousresearch.com/docs/user-guide/features/tools/>
- Hermes memory: <https://hermes-agent.nousresearch.com/docs/user-guide/features/memory/>
- Hermes skills: <https://hermes-agent.nousresearch.com/docs/user-guide/features/skills/>

---

## Diagnosis

What OpenClaw and Hermes get right that SynapseClaw still lacks:

1. **First-class current-session / current-conversation targeting**
   The agent can act "here" without asking for room IDs or chat IDs it already has.

2. **High-level orchestration verbs**
   Tools like `todo`, `clarify`, `send_message`, and `session_search` let the model solve product tasks without reconstructing them from shell + grep + config files.

3. **Gateway-owned proactive flows**
   Restart reports, heartbeats, standing orders, and scheduled announcements are product concepts, not ad-hoc scripts.

4. **Planning guardrails**
   The system prevents no-progress tool flailing and avoids saying "I'll do X" before it knows it can actually do X.

5. **Channel-smart task shaping**
   Messaging surfaces should get a smaller, smarter task vocabulary than the web/operator console.

SynapseClaw today has the infrastructure pieces, but not the product-layer contract that turns them into "obviously smart" behavior.

---

## Goal

Make SynapseClaw feel smarter in channels and web by adding product-native task abstractions above the existing runtime.

Specifically:

1. Let the agent target **the current conversation** as a first-class resource.
2. Replace common low-level multi-tool decompositions with a few high-level orchestration tools/services.
3. Make proactive work (restart reports, standing orders, heartbeat-triggered follow-ups) product-native.
4. Add planner guardrails so the agent clarifies or validates prerequisites before committing.
5. Keep the implementation aligned with the hexagonal architecture.

---

## Non-goals

- Rewriting the memory system again
- Reverting the compact human-first channel UX
- Re-introducing raw backend/storage errors into user channels
- Solving every low-level tool bug inside this phase
- Replacing the current multi-agent IPC plan
- Building the full web UI for these features in this phase

Notes:

- The Surreal vector recall parser bug is a separate memory bugfix track.
- Backend/storage error leakage to channels is already treated as fixed and is not the target of this phase.

---

## Design Principles

### 1. Product verbs beat plumbing

If a user intent is common and obvious, the runtime should expose it as a direct action.

Bad:

- find room id
- inspect systemd
- grep config
- write shell script
- create cron

Good:

- deliver here
- subscribe restart report here
- ask one clarification
- search past sessions

### 2. Current conversation is a first-class runtime object

The inbound envelope already knows where the conversation lives. Tools should be able to say "here" or "current conversation" without rediscovering IDs.

### 3. Planner policy belongs in application, not in adapters

No-promise-before-proof, loop prevention, and prerequisite validation must be application-level behavior, not per-channel hacks.

### 4. Channels are conversational; web is observability

Messaging surfaces get compact, human-facing actions. Deep trace, raw telemetry, and operator detail stay in the web UI.

### 5. Structured scaffolding beats hidden prompt cleverness

To make the agent feel smarter, give it:

- task lists
- clarification primitives
- session search
- standing orders

not just more prompt text.

---

## Phase Slices

## Slice 1 — Current Conversation Target

### Problem

The agent already knows the current room/chat/thread via the inbound envelope, but tools like scheduled delivery still require explicit `channel` + `to`.

This causes absurd behavior like asking for a Matrix room ID while already inside that room.

### Goal

Introduce a first-class delivery target that can mean:

- `current_conversation`
- `explicit(channel, recipient, thread)`

### Design

Add a canonical target type:

```rust
pub enum DeliveryTarget {
    CurrentConversation,
    Explicit {
        channel: String,
        recipient: String,
        thread_ref: Option<String>,
    },
}
```

And a runtime context projection:

```rust
pub struct CurrentConversationTarget {
    pub source_adapter: String,
    pub conversation_ref: String,
    pub reply_ref: String,
    pub thread_ref: Option<String>,
}
```

This target is created from `InboundEnvelope` once and passed into tool execution / post-turn actions as runtime context.

### Scope

- extend cron delivery config to accept `current_conversation`
- add a first-class `message_send` tool that can use `current_conversation`
- let future proactive flows bind to `current_conversation` without manual room lookup

### Files

| File | Change |
|------|--------|
| `crates/domain/src/domain/channel.rs` or new `domain/conversation_target.rs` | Add `CurrentConversationTarget` / `DeliveryTarget` |
| `crates/domain/src/domain/config.rs` | Update delivery config projections |
| `crates/domain/src/application/services/delivery_service.rs` | Resolve `CurrentConversation` deterministically |
| `crates/domain/src/application/use_cases/handle_inbound_message.rs` | Pass current conversation target into runtime/tool context |
| `crates/adapters/tools/src/cron_add.rs` | Accept `delivery.target = "current_conversation"` |
| `crates/adapters/tools/src/schedule.rs` | Update guidance and schema |
| `crates/adapters/tools/src/` | Add real `message_send` tool (the parser alias exists; the tool does not) |

### Acceptance criteria

1. In Matrix/Telegram/Slack, "send it here" works without asking for channel ID.
2. Scheduled delivery can bind to the current room/chat.
3. The agent no longer needs shell/config archaeology to reply back to the same conversation.

---

## Slice 2 — High-Level Orchestration Tools

### Problem

SynapseClaw has many low-level tools, but it lacks some of the orchestration-layer verbs that make Hermes feel competent in day-to-day chat work.

Today the model must simulate these patterns manually:

- multi-step planning
- asking structured clarifying questions
- sending a proactive message
- searching prior sessions for something previously discussed

### Goal

Add a small set of first-class orchestration tools:

- `todo`
- `clarify`
- `message_send`
- `session_search`

### Design

#### `todo`

Session-scoped task ledger for multi-step planning.

Operations:

- `add`
- `list`
- `update`
- `complete`
- `remove`
- `clear`

This gives the model a bounded task scratchpad instead of forcing it to keep plans implicit in chat history.

#### `clarify`

Structured ask tool for missing critical inputs.

Supports:

- open question
- short options
- optional recommendation

This should be the standard way to ask for missing parameters instead of free-form wandering or premature promises.

#### `message_send`

First-class outbound message tool for:

- current conversation
- another explicit conversation
- future proactive/system flows

#### `session_search`

Search prior sessions/transcripts with compact recap output, hiding raw tool internals by default.

This is the "did we talk about this last week?" tool that belongs above long-term memory.

### Files

| File | Change |
|------|--------|
| `crates/domain/src/ports/` | Add task/clarification/session-search ports as needed |
| `crates/domain/src/application/services/` | Add orchestration services for todo/clarify/session search |
| `crates/adapters/tools/src/` | Implement `todo`, `clarify`, `message_send`, `session_search` |
| `crates/adapters/core/src/gateway/**` | Expose response/read-model support for web later |
| session storage adapters | Support efficient transcript lookup for `session_search` |

### Acceptance criteria

1. Multi-step tasks can be externalized into a task list.
2. The agent asks one structured clarification instead of hunting for obvious missing data.
3. The agent can proactively message the current conversation or another explicit one.
4. The agent can search past sessions without abusing long-term memory.

---

## Slice 3 — Standing Orders, System Events, and Restart-Native Reports

### Problem

Users want product-native automation like:

- "after restart, report back here"
- "every morning send me a system summary"
- "when heartbeat sees urgent work, notify this room"

Today the agent falls back to shell scripts and cron plumbing because there is no structured concept for standing orders bound to a conversation.

### Goal

Make proactive instructions a first-class runtime feature.

### Design

Add:

```rust
pub struct StandingOrder {
    pub id: String,
    pub kind: StandingOrderKind,
    pub delivery: DeliveryTarget,
    pub enabled: bool,
}

pub enum StandingOrderKind {
    RestartReport,
    HeartbeatReport,
    ScheduledPrompt { prompt: String, schedule: Schedule },
    CustomSystemEvent { prompt: String },
}
```

And a system-event queue:

```rust
pub enum SystemEvent {
    RuntimeRestarted,
    HeartbeatTick,
    OperatorEnqueued { text: String },
}
```

The existing heartbeat/runtime hooks then become the execution engine for standing orders rather than forcing the model to build shell scripts.

### Scope

- bind restart report subscriptions to current conversation
- route startup/runtime restart into a `RuntimeRestarted` event
- let heartbeat consume pending standing orders
- keep cron for generic scheduling, but give the agent a better default abstraction

### Files

| File | Change |
|------|--------|
| `crates/domain/src/domain/` | Add `standing_order.rs` / `system_event.rs` |
| `crates/domain/src/ports/` | Add standing-order store port |
| `crates/domain/src/application/services/` | Add standing-order orchestration |
| `crates/adapters/core/src/heartbeat/**` | Consume standing orders + system events |
| `crates/adapters/core/src/daemon/**` / gateway startup hooks | Emit restart/system events |
| `crates/adapters/tools/src/` | Add `standing_order_*` tools or a single `standing_order` tool |

### Acceptance criteria

1. "After restart, report here" is a single product-native action.
2. Restart reports can be bound to the current Matrix/Telegram/Slack conversation.
3. No shell script is required for ordinary proactive notification cases.
4. Heartbeat/system events become structured runtime inputs, not ad-hoc prompt tricks.

---

## Slice 4 — Planner Guardrails and Smart Channel Tool Profiles

### Problem

Even with better tools, the agent will still look dumb if it:

- promises actions before prerequisites are known
- keeps retrying low-signal shell/file tools
- uses deep infrastructure tools in messaging when a high-level action exists

### Goal

Add application-level planner guardrails and a smarter tool profile for messaging channels.

### Design

#### 4a. Task intent classification

Introduce an application service that classifies user requests into high-level intents:

- `Answer`
- `ActNow`
- `Schedule`
- `Subscribe`
- `Clarify`
- `SideQuestion`

This is not for model routing. It is for planner policy.

#### 4b. Actionability / prerequisite check

Before the agent says "I'll set that up", it must know whether:

- the target is known
- the permission exists
- the tool path is allowed
- the schedule payload is valid

If not, the policy should prefer:

- one clarification
- or one validation step

not a premature commitment.

#### 4c. Channel tool profile

Messaging channels should bias toward:

- `clarify`
- `todo`
- `message_send`
- `session_search`
- `standing_order`
- `cron_add`

and only then fall back to low-level shell/file search.

#### 4d. No-progress loop heuristics

Borrow the spirit of OpenClaw’s loop detection:

- repeated same tool + same arguments
- alternating no-progress pairs
- repeated denied shell commands
- repeated config/file hunts for already-known current-conversation data

### Files

| File | Change |
|------|--------|
| `crates/domain/src/application/services/` | Add task-intent and actionability services |
| `crates/adapters/core/src/channels/mod.rs` | Use messaging-specific tool profiles |
| `crates/adapters/core/src/agent/loop_.rs` | Add no-progress heuristics / planner hooks |
| `crates/adapters/observability/**` | Add metrics/events for loop and clarification triggers |

### Acceptance criteria

1. The agent no longer asks for room/chat IDs when `current_conversation` is available.
2. The agent does not promise configuration changes before validating prerequisites.
3. Messaging runs prefer high-level orchestration tools over shell/config spelunking.
4. Repeated no-progress tool loops are detected and stopped earlier.

---

## Slice 5 — Side Questions and Session-Pure Work

### Problem

Users often ask a quick tangent while a larger task is in progress. Without an explicit model for side questions, the current session can become muddled and the main task derails.

### Goal

Add an explicit side-question path that is session-light and does not poison the main task state.

### Design

Introduce a side-question mode:

- ephemeral
- not persisted to standing orders / todo by default
- minimal memory writeback
- optionally marked by the user (`btw`, `/aside`) or inferred from planner policy

This mirrors the product advantage of "answer the tangent, then resume the main task" without spawning a full subagent every time.

### Files

| File | Change |
|------|--------|
| `crates/domain/src/application/services/` | Add side-question policy |
| `crates/adapters/core/src/gateway/ws.rs` | Support side-question session mode in web |
| `crates/domain/src/application/use_cases/handle_inbound_message.rs` | Support side-question mode in channels |

### Acceptance criteria

1. A small tangent does not derail the main task/task list.
2. Side-question answers stay lightweight and do not trigger unnecessary standing-order/task changes.
3. Channels and web use the same policy.

---

## Slice 6 — Dialogue State and Referential Resolution

### Problem

Some of the most embarrassing "memory failures" are not actually long-term memory failures.

Example:

1. the user asks for weather in two cities
2. the agent answers both
3. the user asks "what's the weather?"
4. the agent asks "for which city?"

This happens because the system is trying to solve a **working-memory / dialogue-state** problem with semantic recall alone.

Current memory enrichment is keyed mainly off the current utterance text, while short follow-ups often omit the subject entirely:

- "and the second one?"
- "what about prod?"
- "restart that service"
- "send it here"
- "is it still failing?"

These are not primarily long-term memory tasks. They are:

- reference resolution
- active-topic tracking
- slot filling
- comparison tracking
- session-scoped working state

### Goal

Add a first-class `DialogueState` / `WorkingState` layer that sits **above session history** and **below long-term memory**.

This layer should make short follow-ups feel obvious instead of forcing the agent to infer everything from recall.

### Design

Introduce a thread/session-scoped state object:

```rust
pub struct DialogueState {
    pub active_intent: Option<IntentKind>,
    pub focus_entities: Vec<FocusEntity>,
    pub slots: Vec<DialogueSlot>,
    pub comparison_set: Vec<FocusEntity>,
    pub last_tool_subjects: Vec<ToolSubject>,
    pub ambiguity: Option<AmbiguityState>,
}
```

Supporting ideas:

- `focus_entities`: current city / service / host / environment / file / ticket / branch
- `slots`: structured fields for common flows (`location`, `service_name`, `environment`, `time_range`, `delivery_target`)
- `comparison_set`: "Berlin + Tbilisi", "staging + prod", "service A + service B"
- `last_tool_subjects`: structured frames from tool output, not just raw text
- `ambiguity`: whether the next short follow-up can be resolved automatically or needs a targeted clarification

### Behavior

When the user asks a short follow-up:

- first check `DialogueState`
- then check current-session history / `session_search`
- only then fall back to long-term memory

Examples:

- if `comparison_set = [Berlin, Tbilisi]`, then "what's the weather?" should either:
  - answer for both
  - or ask a targeted clarification: "Berlin or Tbilisi?"
- if `focus_entities = [synapseclaw.service]`, then "restart it" should resolve `it`
- if `delivery_target = current conversation`, then "send it here" should not ask for room ID

### Why this is different from existing memory

This is not:

- another semantic graph
- another long-term memory write
- another prompt-only hint

This is a **deterministic, ephemeral, session-scoped state layer**.

Comparable patterns from other systems:

- OpenClaw: session-first task and context model
- Hermes: `session_search` plus curated memory/tools
- LangGraph: explicit short-term thread state
- Rasa: slot-based dialogue state
- Letta: always-available structured memory blocks for high-priority facts

### Scope

#### 6a. New domain state types

- `DialogueState`
- `DialogueSlot`
- `FocusEntity`
- `ToolSubject`
- `AmbiguityState`

#### 6b. Dialogue-state updater

New application service that updates state each turn from:

- user utterance
- current tool results
- selected response outcome

This should be mostly deterministic with a narrow optional extraction step, not another broad memory pipeline.

#### 6c. Structured tool result frames

Tools should be able to emit compact structured subjects like:

```rust
ToolSubject::Weather { location: "Berlin", condition: "rain", temperature_c: 12.0 }
ToolSubject::Service { name: "synapseclaw", state: "running" }
ToolSubject::DeliveryTarget { channel: "matrix", recipient: "...", thread_ref: None }
```

This gives the next user turn a reliable bridge from "tool result happened" to "the user is referring to that thing".

#### 6d. Ambiguity resolver

Before a generic clarify, run a narrow resolver:

- if one focus entity is dominant → answer directly
- if two candidates are active → ask a targeted clarify
- if the request is naturally plural → answer for both

#### 6e. State-aware continuation policy

`ContinuationPolicy` today controls memory enrichment budgets. It should grow a sibling policy for dialogue state so channels/web do not lose active referents on cheap continuation turns.

### Files

| File | Change |
|------|--------|
| `crates/domain/src/domain/` | Add `dialogue_state.rs` |
| `crates/domain/src/ports/` | Add dialogue-state store port if needed |
| `crates/domain/src/application/services/` | Add `dialogue_state`, `reference_resolution`, `ambiguity_resolution` services |
| `crates/domain/src/application/use_cases/handle_inbound_message.rs` | Read/write dialogue state on channel turns |
| `crates/adapters/core/src/gateway/ws.rs` | Read/write dialogue state on web turns |
| tool adapters | Emit structured `ToolSubject` metadata where useful |
| session storage adapters | Persist session-scoped dialogue state cheaply |

### Acceptance criteria

1. After asking about multiple cities/services/environments, short follow-ups resolve against current session state rather than long-term memory only.
2. The agent uses targeted clarifications ("Berlin or Tbilisi?") instead of generic ones ("which city?") when the candidate set is known.
3. "Send it here", "restart it", and similar references resolve from current dialogue state when possible.
4. Channels and web share the same dialogue-state policy.
5. The system does not promote transient dialogue state into long-term memory by default.

---

## Execution Order

Recommended order:

1. **Slice 1** — current-conversation target
2. **Slice 2** — orchestration tools (`todo`, `clarify`, `message_send`, `session_search`)
3. **Slice 6** — dialogue state and referential resolution
4. **Slice 4** — planner guardrails + channel tool profiles
5. **Slice 3** — standing orders / restart-native reports
6. **Slice 5** — side questions

Rationale:

- Slice 1 fixes the most embarrassing failure mode immediately.
- Slice 2 gives the planner better verbs.
- Slice 6 fixes the "obvious follow-up question" class of failures that semantic memory alone cannot solve.
- Slice 4 makes the planner actually prefer the right verbs and clarification behavior.
- Slice 3 then upgrades proactive work onto the same abstractions.
- Slice 5 is valuable, but can come after the main intelligence surface exists.

---

## PR Structure

| PR | Slices | Title |
|----|--------|-------|
| PR A | 1 | `feat(runtime): current-conversation delivery targets` |
| PR B | 2 | `feat(tools): add orchestration tools for tasking and clarification` |
| PR C | 6 | `feat(agent): add dialogue state and referential resolution` |
| PR D | 4 | `feat(agent): planner guardrails and messaging tool profiles` |
| PR E | 3 | `feat(runtime): standing orders and restart-native reports` |
| PR F | 5 | `feat(agent): side-question mode` |

---

## Relation to Existing Plans

This phase depends on work already captured in:

- [`memory-learning-foundation-plan.md`](memory-learning-foundation-plan.md)
- [`memory-unification-plan.md`](memory-unification-plan.md)

It should also feed:

- [`multi-agent-memory-ui-plan.md`](multi-agent-memory-ui-plan.md)

Why:

- the smarter product layer needs stable memory + prompt assembly beneath it
- the future UI needs honest read-models for tasks, standing orders, current-target delivery, and planner decisions

This phase is the missing middle layer between:

- "the agent technically has many capabilities"
- and
- "the agent feels obviously competent in chat"

---

## Success Criteria

Phase 4.6 is successful when all of the following are true:

1. In channels, "send/report back here" works without asking for room IDs already known to the runtime.
2. The agent has first-class orchestration tools for planning, clarification, messaging, and session search.
3. Short follow-up questions resolve against current dialogue state instead of relying on long-term memory alone.
4. Restart reports and similar proactive tasks can be bound to the current conversation natively.
5. The planner prefers validation/clarification over blind low-level tool flailing.
6. Messaging runs feel materially closer to OpenClaw/Hermes in product behavior, without abandoning SynapseClaw’s Rust/hexagonal architecture.
