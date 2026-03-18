# IPC Phase 4.0: Modular Core Refactor

Phase 3.7b: session intelligence | **Phase 4.0: modular core refactor** | Phase 4.1: federated execution

---

## What Phase 4.0 gives

Six promises to the fork:

1. **One application core** — business semantics stop living in scattered `telegram` / `matrix` / `gateway` / `tools` branches.
2. **Capability-driven channels** — scheduled notifications, heartbeat, approvals, replies, and similar flows depend on channel capabilities, not channel names.
3. **Fixed boundaries for human and agent transports** — inbound/outbound human channels, web chat, and inter-agent IPC become adapters to the same core use cases.
4. **Pluggable memory tiers** — working memory, session memory, and long-term memory are explicit ports, not hidden assumptions inside unrelated modules.
5. **One conversation store contract** — web chat, channel conversations, and later IPC transcripts can sit on one durable transcript/session model.
6. **Fork maintainability** — the refactor reduces merge surface with upstream by moving fork-specific semantics into a fork-owned core.

---

## Why Phase 4.0 exists

The current codebase already has real subsystems, but the application core is spread across too many places:

- `src/channels/mod.rs` wires transports, conversation flow, runtime commands, history, and channel-specific behavior.
- `src/daemon/mod.rs` handles heartbeat delivery via hardcoded channel-name whitelists.
- `src/cron/scheduler.rs` delivers announcements via manual `match` on channel names.
- `src/gateway/ipc.rs`, `src/gateway/ws.rs`, and `src/tools/*` each own their own pieces of session/run logic.
- memory backends exist, but memory is still not modeled as a first-class multi-tier architecture shared by chat, agents, and IPC.

This breaks down as the fork grows:

1. **Too many channels** — the config already knows many channels, but high-level use cases still forget some of them because behavior is wired by transport name.
2. **Too many orchestration hubs** — channels, gateway, tools, and agent runtime all know business semantics.
3. **Too many partial storage models** — channel session persistence, web chat persistence, IPC persistence, and memory live side by side without a single conversation substrate.
4. **Too much fork merge risk** — every new feature wants to touch multiple upstream-owned hotspots.

Phase 4.0 fixes the architectural shape before introducing larger platform changes like federated execution.

---

## Goals

1. Introduce a **fork-owned application core** with obvious modularity.
2. Replace transport-name branching with a **channel capability model**.
3. Define one **canonical inbound envelope** and one **canonical outbound intent**.
4. Define one **conversation/session store contract** for chat-first workloads.
5. Define one **unified run substrate** for chat runs, IPC execution, and future external workers.
6. Make **three-tier memory** explicit and swappable behind ports.
7. Move approval, scheduling, delivery, and IPC routing semantics into application services.
8. Define a clean seam for external coding workers (for example Codex or Claude Code) as bounded leaf executors, not as replacement core runtimes.
9. Keep the migration incremental and upstream-sync-friendly.

---

## Non-goals

1. No big-bang rewrite.
2. No immediate rewrite of every channel implementation.
3. No immediate replacement of provider implementations.
4. No federated multi-host execution in this phase.
5. No new memory engine as a mandatory dependency.
6. No attempt to unify every existing storage format in one PR.
7. No visual policy editor in this phase.

---

## Design stance

### Business semantics live in the core

The fork-owned application core owns:

- routing decisions
- delivery policy
- approvals and escalation
- IPC and human-channel flow semantics
- conversation lifecycle
- memory tier usage policy
- session/run state

### Transports are adapters

Telegram, Signal, Matrix, Slack, Discord, WhatsApp, web chat, IPC broker, and cron/heartbeat triggers are not business logic owners. They translate between external protocols and the canonical application model.

### External coding workers are leaf executors

External coding systems such as Codex, Claude Code, or future specialized implementation engines belong behind a dedicated core port. They are not providers, not memory owners, and not replacement application cores.

Their role is narrow:
- receive bounded implementation tasks
- operate inside a repo/worktree or coding sandbox
- return structured progress, questions, artifacts, and results

The fork-owned core remains the source of truth for:
- orchestration
- trust and approvals
- IPC policy
- conversation and memory state
- routing and escalation

### Scheduling is not a channel capability

A channel may support `send_text`. That is enough for scheduled notifications. Scheduling itself belongs to application policy and scheduler ports, not to the channel.

### Capability checks replace name checks

Application services must stop branching on strings like `"telegram"` or `"signal"`. They must ask whether the selected adapter exposes the required capabilities.

### Strangler refactor over rewrite

New logic lands in fork-owned modules first. Existing upstream paths are gradually re-routed to those modules.

---

## Target module layout

```text
src/
  fork_core/
    domain/
      channel.rs
      conversation.rs
      ipc.rs
      memory.rs
      approval.rs
      run.rs
      implementation.rs
    application/
      services/
        delivery_service.rs
        inbound_message_service.rs
        approval_service.rs
        conversation_service.rs
        ipc_service.rs
        memory_service.rs
      use_cases/
        send_scheduled_notification.rs
        handle_inbound_message.rs
        start_conversation_run.rs
        dispatch_ipc_message.rs
        request_approval.rs
        review_quarantine_item.rs
        spawn_child_agent.rs
        delegate_implementation_task.rs
    ports/
      channel_registry.rs
      conversation_store.rs
      memory_tiers.rs
      approval.rs
      scheduler.rs
      runtime.rs
      coding_worker.rs
      audit.rs
      identity.rs
      ipc_bus.rs
      summary.rs
  fork_adapters/
    channels/
    gateway/
    ipc/
    storage/
    memory/
    approval/
    runtime/
    coding_workers/
```

The exact filenames can vary, but the boundary may not:

- `fork_core` owns semantics
- `fork_adapters` owns translation and infrastructure
- upstream subsystems remain as shells/adapters as much as possible

---

## Domain concepts

### Canonical inbound envelope

Every inbound message-like event becomes one application envelope:

```text
InboundEnvelope {
  source_kind: web | channel | ipc | cron | system,
  source_adapter: telegram | matrix | signal | ipc | web | ...,
  actor_id: String,
  conversation_ref: String,
  reply_ref: Option<String>,
  thread_ref: Option<String>,
  content: String,
  attachments: Vec<AttachmentRef>,
  trust_context: TrustContext,
  received_at: Timestamp,
  metadata: Map<String, Json>,
}
```

Purpose:
- human channels, web chat, and IPC stop inventing separate quasi-message models
- application services reason on one input type

### Canonical outbound intent

The core emits an intent, not a transport-specific API call:

```text
OutboundIntent {
  intent_kind: reply | notify | approval_request | escalation | draft_update,
  conversation_ref: String,
  target_ref: String,
  thread_ref: Option<String>,
  content: RenderableContent,
  required_capabilities: Vec<ChannelCapability>,
  degradation_policy: DegradationPolicy,
  metadata: Map<String, Json>,
}
```

Purpose:
- application logic says what must happen
- adapter decides how to express it on a specific transport

### Conversation

A durable conversation/session object shared by web chat first and extensible to channels/IPC later.

```text
Conversation {
  key: String,
  kind: web | channel | ipc,
  owner_scope: String,
  label: Option<String>,
  summary: Option<String>,
  current_goal: Option<String>,
  last_active: Timestamp,
  message_count: u32,
  input_tokens: u64,
  output_tokens: u64,
  metadata: Map<String, Json>,
}
```

### Conversation event

Transcript storage must be event-oriented, not only `user/assistant` text:

```text
ConversationEvent {
  id: i64,
  conversation_key: String,
  event_type: user | assistant | tool_call | tool_result | error | interrupted | system,
  actor: String,
  run_id: Option<String>,
  tool_name: Option<String>,
  content_json: Json,
  input_tokens: Option<u32>,
  output_tokens: Option<u32>,
  timestamp: Timestamp,
}
```

### Memory tiers

Three explicit tiers:

1. **Working memory** — in-run transient context, not durable beyond the active runtime.
2. **Session memory** — session summary, current goal, recent artifacts, durable conversation-level state.
3. **Long-term memory** — semantic/project memory, vector or document-backed, cross-session and cross-agent if desired.

### Approval request

A first-class object, not a chat-side effect:

```text
ApprovalRequest {
  id: String,
  origin_kind: channel | ipc | sop | runtime,
  requested_by: String,
  action_summary: String,
  risk: low | medium | high | critical,
  conversation_key: Option<String>,
  run_id: Option<String>,
  status: pending | approved | denied | expired,
}
```

### Run

A first-class runtime execution record:

```text
Run {
  run_id: String,
  conversation_key: String,
  origin_kind: web | channel | ipc | spawn,
  state: queued | running | completed | interrupted | failed | cancelled,
  started_at: Timestamp,
  finished_at: Option<Timestamp>,
}
```

### Implementation run

A structured execution record for bounded external coding work:

```text
ImplementationRun {
  run_id: String,
  task_id: String,
  worker_ref: String,
  conversation_key: Option<String>,
  state: queued | dispatching | running | blocked | approval_required | completed | failed | cancelled | interrupted,
  started_at: Timestamp,
  finished_at: Option<Timestamp>,
  input_tokens: Option<u64>,
  output_tokens: Option<u64>,
  metadata: Map<String, Json>,
}
```

### Implementation task

A bounded task that can be executed by a specialized coding worker:

```text
ImplementationTask {
  task_id: String,
  objective: String,
  repo_ref: String,
  worktree_ref: Option<String>,
  constraints: Vec<String>,
  allowed_paths: Vec<String>,
  allowed_tools: Vec<String>,
  tests_to_run: Vec<String>,
  timeout_secs: u64,
  expected_output: patch | branch | report,
}
```

Design rule: an implementation task is not a free-form chat prompt. It is a structured execution contract between the orchestration core and a specialized coding worker.

### Implementation run event

External worker progress is not modeled as free-form chat turns or ad hoc IPC kinds:

```text
ImplementationRunEvent {
  id: i64,
  run_id: String,
  event_type: progress | question | artifact | blocked | approval_required | result | failure,
  content_json: Json,
  created_at: Timestamp,
}
```

### Implementation artifact

```text
ImplementationArtifact {
  run_id: String,
  artifact_kind: patch | changed_files | test_report | log | bundle,
  uri: String,
  metadata: Map<String, Json>,
}
```

---

## Channel capability model

Phase 4.0 introduces explicit capability descriptors.

```text
ChannelCapabilities {
  send_text,
  receive_text,
  threads,
  reactions,
  typing,
  attachments,
  rich_formatting,
  interactive_approval,
  draft_updates,
  edit_message,
  read_history,
  webhook_inbound,
  streaming_updates,
}
```

### Important distinction

#### Channel capability

What the transport adapter can do.

Examples:
- can send text
- can edit a message
- can show typing
- can receive inbound webhooks

#### Application policy

What the product wants to do.

Examples:
- heartbeat every N minutes
- approval required for risky action
- escalation to operator
- quarantine lane behavior

#### Transport-specific rendering

How a given adapter maps the canonical intent into platform syntax.

Examples:
- Slack thread reply
- Telegram HTML/Markdown formatting
- Matrix room reply markup

#### Delivery constraints and degradation

What to do when a capability is absent.

Examples:
- no threads → send as flat reply
- no reactions → skip reaction
- no interactive approval → route to web/Matrix approval center
- no rich formatting → downgrade to plain text

---

## What belongs where

### Application layer owns

- heartbeat policy
- scheduled notification policy
- approval routing
- escalation routing
- IPC trust and routing semantics
- conversation lifecycle
- run lifecycle
- summary update policy
- memory write/read policy
- fallback/degradation policy

### Adapter layer owns

- Telegram API calls
- Signal send/listen translation
- Matrix room/thread mapping
- Slack draft edit mechanics
- webhook payload parsing
- channel-specific formatting/rendering
- platform message IDs and reply references
- provider API translation
- SQLite/Qdrant/Postgres implementation details

### The core must not own

- raw Telegram chat IDs
- Matrix room protocol details
- Slack API peculiarities
- direct `match` on transport names for migrated use cases

---

## Ports

### Primary ports (use cases exposed by the core)

1. `HandleInboundMessage`
2. `SendScheduledNotification`
3. `RequestApproval`
4. `StartConversationRun`
5. `AbortConversationRun`
6. `DispatchIpcMessage`
7. `ReviewQuarantineItem`
8. `SpawnChildAgent`
9. `ResumeConversation`
10. `DelegateImplementationTask`

These are what adapters call.

### Secondary ports (infrastructure the core depends on)

1. `ChannelRegistryPort`
   - resolve channel adapter by ref
   - return capabilities
   - send outbound intent

2. `ConversationStorePort`
   - create/list/get/update/delete conversations
   - append/list conversation events
   - store summary/current_goal/token counts

3. `RunStorePort`
   - create/get/update execution runs
   - append/list run events
   - persist progress, artifacts, usage, and terminal results
   - unify chat runs, IPC execution runs, and future external coding worker runs

4. `MemoryTiersPort`
   - get/set session memory
   - write long-term memory
   - retrieve long-term context

5. `ApprovalPort`
   - create approval request
   - fetch decision

6. `SchedulerPort`
   - schedule notification
   - delay/retry job

7. `RuntimePort`
   - run agent turn
   - abort run
   - spawn child

8. `IpcBusPort`
   - send/receive IPC intents
   - map IPC envelopes to/from application messages

9. `IdentityPort`
   - pairing metadata
   - token metadata
   - revoke / downgrade / key registration

10. `AuditPort`
   - persist domain/audit events

11. `SummaryPort`
    - generate/update summaries and goals using configured model policy

12. `CodingWorkerPort`
    - submit a bounded implementation task to an external coding worker
    - stream or poll progress/questions/artifacts
    - map worker updates into `RunStorePort`
    - collect final result, failure, or approval request

---

## Conversation store and chat database

Phase 3.7 introduced SQLite-backed chat sessions. Phase 4.0 turns that into a general **conversation store port**.

### Why a dedicated conversation DB still matters

The fork needs a chat-friendly store that supports:

- session list + recent preview
- stable message/event IDs
- run tracking
- token usage aggregation
- summary/current_goal
- future search/export
- later reuse for channel and IPC transcripts

A generic memory backend is not the right place for hot chat/session state.

### v1 conversation schema

```sql
CREATE TABLE conversations (
    key            TEXT PRIMARY KEY,
    kind           TEXT NOT NULL,      -- web | channel | ipc
    owner_scope    TEXT NOT NULL,
    label          TEXT,
    summary        TEXT,
    current_goal   TEXT,
    created_at     INTEGER NOT NULL,
    last_active    INTEGER NOT NULL,
    message_count  INTEGER DEFAULT 0,
    input_tokens   INTEGER DEFAULT 0,
    output_tokens  INTEGER DEFAULT 0,
    metadata_json  TEXT
);

CREATE TABLE conversation_events (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_key TEXT NOT NULL REFERENCES conversations(key),
    event_type     TEXT NOT NULL,
    actor          TEXT NOT NULL,
    run_id         TEXT,
    tool_name      TEXT,
    content_json   TEXT NOT NULL,
    input_tokens   INTEGER,
    output_tokens  INTEGER,
    created_at     INTEGER NOT NULL
);

CREATE TABLE conversation_runs (
    run_id         TEXT PRIMARY KEY,
    conversation_key TEXT NOT NULL REFERENCES conversations(key),
    state          TEXT NOT NULL,
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    metadata_json  TEXT
);
```

### Design rule

The conversation DB is **not** the long-term memory engine.
It is the durable operational store for chat/session/run state.

---

## Run substrate

Phase 4.0 introduces a **unified run substrate** alongside the conversation store.

Why this matters:

- the current codebase has several partial run/session systems
- chat runs, spawn runs, cron execution, and future coding-worker runs should not each invent their own lifecycle table
- external coding workers need more than `task/result`; they need durable progress, artifacts, and terminal status

### Design rule

Conversation storage and run storage are related but distinct:

- `ConversationStorePort` owns conversations and transcript events
- `RunStorePort` owns execution lifecycle

This keeps the architecture honest:

- not every run is a chat message
- not every progress update belongs in the transcript
- approvals, blocking, artifacts, and execution status need a first-class runtime model

### Initial implementation stance

The first `CodingWorkerPort` implementation should ride on:

- existing IPC task/result routing
- a new `external_runs` / `external_run_events` store behind `RunStorePort`
- existing agent registration and trust model

It should **not** ride on:

- `Provider`
- `DelegateTool`
- `NodeRegistry` transport

### Why not provider / delegate / nodes

- `Provider` is a model backend contract, not a worker runtime contract
- `DelegateTool` is an isolated one-shot turn, not a persistent implementation worker
- `nodes` is useful transport plumbing, but not an execution substrate

The external worker seam therefore attaches closest to IPC plus a unified run store.

---

## Memory architecture

Phase 4.0 makes memory explicit instead of incidental.

### Tier 1: Working memory

Scope:
- current run / current turn
- short-lived scratch context
- hidden internal runtime state

Properties:
- in-process
- not durable across restart
- not intended for cross-session recall

### Tier 2: Session memory

Scope:
- current goal
- rolling session summary
- pinned artifacts
- recent important facts for this conversation/session

Properties:
- durable
- conversation-scoped
- stored through `ConversationStorePort` and `MemoryTiersPort`

### Tier 3: Long-term memory

Scope:
- project facts
- operator preferences
- cross-session knowledge
- optionally semantic/vector memory

Properties:
- durable
- not chat-DB-specific
- backed by existing memory backends via adapter (`sqlite`, `qdrant`, `lucid`, `markdown`, future external engines)

### Integration rule

Conversation/session state must not depend on a specific long-term memory engine.
Long-term memory remains pluggable.

---
## External coding workers

Phase 4.0 does **not** adopt an external coding system as a new application core. It prepares the correct seam so one can be integrated later without distorting the fork architecture.

### Role in the architecture

External coding workers are **leaf executors** for implementation-heavy tasks:

- patching code
- editing files
- running repo-local commands/tests
- answering codebase-specific implementation questions

Examples may include Codex, Claude Code, or future workers with similar execution-oriented runtimes.

### What they are not

They are not:

- the global conversation/session store
- the owner of long-term memory
- the trust or approval policy engine
- the replacement for IPC or orchestration
- a generic provider plug-in for ordinary chat traffic

### Required contract

The core should be able to hand off a structured `ImplementationTask` and receive a structured result:

```text
CodingWorkerResult {
  task_id: String,
  state: completed | blocked | failed | approval_required,
  summary: String,
  changed_files: Vec<String>,
  test_results: Vec<String>,
  questions: Vec<String>,
  artifacts: Vec<String>,
}
```

### Design rule

If the fork later integrates an external coding engine, it must happen through `CodingWorkerPort`. It must not be embedded as a second competing application core or disguised as a thin `Provider` implementation.

### Initial transport stance

The first realistic `CodingWorkerPort` implementation is IPC-backed:

- dispatch task via IPC
- track lifecycle via `RunStorePort`
- persist progress/artifacts/result in dedicated run records
- reuse existing trust, registration, and session correlation from IPC where appropriate

This keeps the seam narrow and avoids inventing a second orchestration substrate.

---

## Use cases to migrate first

### Slice 1: Scheduled notifications and heartbeat delivery

Why first:
- clearest example of transport-name branching today
- bounded scope
- high architectural payoff
- easy to verify with capabilities

Target:
- `SendScheduledNotification` use case depends on `send_text`
- heartbeat auto-detect and validation stop hardcoding a narrow channel whitelist
- adding a channel with `send_text` automatically makes it eligible where policy allows

### Slice 2: Inbound channel message handling

Why second:
- establishes canonical inbound envelope
- starts moving business semantics out of `channels/mod.rs`

Target:
- adapters translate platform events into `InboundEnvelope`
- `HandleInboundMessage` owns conversation routing, approvals, and core flow
- channel-specific code stops making business decisions

### Slice 3: Conversation store extraction

Why third:
- Phase 3.7 chat DB already exists
- this is the right moment to generalize it behind a port before more features pile on top

Target:
- web chat uses `ConversationStorePort`
- session summaries/current goals/runs are first-class
- future channel conversations and IPC transcripts can reuse the same store

### Slice 4: Approval and quarantine orchestration

Why fourth:
- approval semantics are currently split across approval manager, SOP, channels, IPC, and UI

Target:
- `RequestApproval` and `ReviewQuarantineItem` become core use cases
- channels, web UI, and IPC approval paths become adapters

### Slice 5: IPC bridging

Why fifth:
- IPC already has strong policy/runtime semantics; it should plug into the same conversation and run model, not invent its own parallel orchestration story forever

Target:
- selected IPC flows emit/use the same core run/conversation events
- no forced unification of everything in one go

### Slice 6: Memory tiers wiring

Why last:
- after conversation and use-case boundaries exist, memory integration becomes much cleaner and lower-risk

### Slice 7: External coding worker seam

Why after the core boundaries:
- before this point the fork still lacks a clean place to attach an external coding engine
- adding Codex/Claude Code too early would duplicate the agent core instead of extending it

Target:
- `CodingWorkerPort` and `DelegateImplementationTask` exist as explicit seams
- `RunStorePort` carries progress/artifacts/result lifecycle
- one narrow implementation-task protocol is fixed
- initial transport rides on IPC plus unified run storage
- no external worker becomes a new provider or a new source of truth for orchestration

---

Target:
- session summary/current goal become session memory
- long-term memory adapters are consulted through explicit policy
- future memory engines can be swapped without touching chat or channels code

---

## Migration strategy

### Rule 1: no big-bang rewrite

Every migrated slice must have:
- old path still working until cutover
- one new fork-owned service
- tests proving no product regression

### Rule 2: minimize upstream merge pain

Prefer:
- new fork-owned files
- small adapter shims in upstream-owned hotspots
- narrow hook points

Avoid:
- giant rewrites of `channels/mod.rs`
- giant rewrites of `gateway/*`
- broad changes inside provider/runtime internals unless strictly required

### Rule 3: migrate by use case, not by directory

Do not start with “rewrite channels module”.
Start with “make scheduled notification capability-driven”.

### Rule 4: keep dual-path time short

A migrated slice should cut over quickly after tests are green. Long-term dual routing creates drift.

---

## Fork vs upstream boundary

### Fork-owned

- application core semantics
- capability registry
- conversation store contract
- memory tier orchestration
- approval/quarantine orchestration
- IPC/human-channel unification logic

### Prefer upstream-owned or reused

- raw transport clients
- provider SDK bindings
- low-level sandbox/runtime mechanisms where adequate
- generic storage adapters when they are neutral enough

### Candidate upstreamables later

- generic channel capability metadata
- generic conversation store abstractions if they become transport-agnostic enough
- small runtime hooks that reduce fork surface

### Keep fork-only for now

- trust hierarchy semantics
- quarantine/operator flows
- approval orchestration model
- capability-driven policy routing tied to fork product behavior

---

## Risks

1. **Too much abstraction too early**
   - mitigation: migrate one vertical slice at a time

2. **Dual-path drift**
   - mitigation: short cutover windows, explicit progress checklist

3. **Lowest-common-denominator channels**
   - mitigation: use degradation policy, do not flatten capabilities away

4. **Conversation DB tries to become memory engine**
   - mitigation: explicit separation between conversation store and long-term memory

5. **Merge pain with upstream**
   - mitigation: fork-owned core + narrow shims

6. **Refactor without product payoff**
   - mitigation: first slices must visibly improve heartbeat/delivery/chat/session consistency

---

## Verification checklist

Phase 4.0 is successful when all are true:

1. A migrated use case no longer branches on `telegram/slack/matrix/...` names in application logic.
2. Adding a channel with `send_text` makes it eligible for scheduled notification/heartbeat where policy allows.
3. Human channels, web chat, and IPC use canonical envelopes/intents in migrated slices.
4. Web chat sits on `ConversationStorePort`, not bespoke embedded logic.
5. Session memory and long-term memory are distinct in code and persistence.
6. At least one use case moved out of `channels/mod.rs`, one out of `daemon/mod.rs`, and one out of `cron/scheduler.rs`.
7. No new fork feature requires sprinkling transport-specific behavior in multiple places.
8. Upstream sync hotspots are reduced, not increased.

---

## Recommended implementation order

1. Add `fork_core` / `fork_adapters` skeleton.
2. Define `ChannelCapabilities`, `InboundEnvelope`, `OutboundIntent`.
3. Introduce `ConversationStorePort` over the current chat/session SQLite path.
4. Migrate scheduled notification delivery.
5. Migrate heartbeat target resolution/validation.
6. Migrate one inbound human channel path through `HandleInboundMessage`.
7. Extract approval/quarantine services.
8. Bridge selected IPC flows into the same core model.
9. Add `MemoryTiersPort` adapters over current memory stack.
10. Remove migrated transport-name branching from old paths.

---

## Relationship to the roadmap

- Phase 3.7 / 3.7b made the web chat usable.
- Phase 4.0 makes the **platform architecture** usable and maintainable.
- Phase 4.1 can then safely tackle federated execution / multi-host placement without stacking more complexity on a scattered core.

Phase 4.0 is the architecture phase that should happen before larger substrate swaps or external memory frameworks.
