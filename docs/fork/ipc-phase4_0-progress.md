# IPC Phase 4.0 Progress

**Status**: all 7 application service slices complete; all acceptance criteria met

Phase 3.12: channel session intelligence | **Phase 4.0: modular core refactor** | Phase 4.1: federated execution

---

## Goal

Refactor the fork toward a pragmatic ports-and-adapters architecture with:

- fork-owned application core
- capability-driven channel behavior
- unified conversation store contract
- unified run substrate
- explicit memory tiers
- lower merge surface with upstream
- a clean seam for optional external coding workers

---

## Checklist

### Infrastructure (ports, domain types, adapters, wiring)

| Item | Status | PRs |
|------|--------|-----|
| `fork_core` / `fork_adapters` skeleton | **DONE** | #160 |
| `OutboundIntent`, `ChannelCapability`, `ChannelRegistryPort` | **DONE** | #160 |
| `CachedChannelRegistry` adapter (long-lived cached channels) | **DONE** | #160 |
| `OutboundIntentBus` (mpsc sender/receiver) | **DONE** | #160 |
| `InboundEnvelope` domain type + `to_channel_message()` bridge | **DONE** | #162 |
| `ConversationStorePort` trait + `ChatDbConversationStore` adapter | **DONE** | #163 |
| `RunStorePort` trait + `ChatDbRunStore` adapter + `runs`/`run_events` tables | **DONE** | #164 |
| `ConversationStorePort` wired into gateway AppState + REST `/api/conversations` | **DONE** | #165 |
| ws.rs migrated from ChatDb to ConversationStorePort (10 calls replaced) | **DONE** | #166 |
| `RunStorePort` wired into gateway AppState + REST `/api/runs` | **DONE** | #167 |
| IPC run tracking via RunStorePort (push-triggered runs) | **DONE** | #168 |
| Transport-name branching removed from application logic (capability-driven) | **DONE** | #169 |
| CLI standalone mode gets real `CachedChannelRegistry` (no fallbacks) | **DONE** | #169 |
| Cron `deliver_announcement()` uses `ChannelRegistryPort.deliver()` | **DONE** | #161 |
| Heartbeat delivery uses `ChannelRegistryPort` | **DONE** | #161 |
| `delivery_hints()` on `ChannelRegistryPort` (adapter-owned formatting) | **DONE** | #169 |
| `event_type` field in WS/REST (legacy `kind` removed from web UI) | **DONE** | #166 |

### Application services and use cases (plan slices 1-7)

| Slice | Status | Description |
|-------|--------|-------------|
| 1 | **DONE** | `delivery_service` + `SendScheduledNotification` — heartbeat/cron delivery policy moved into fork_core |
| 2 | **DONE** | `inbound_message_service` + `HandleInboundMessage` — 7 ports, 8 adapters, orchestrator, old code deleted (−4287 lines) |
| 3 | **DONE** | `conversation_service` + `StartConversationRun` + `AbortConversationRun` — session lifecycle, summary policy, run state machine |
| 4 | **DONE** | `approval_service` + `RequestApproval` + `ReviewQuarantineItem` — domain types, ports, policy, adapter |
| 5 | **DONE** | `ipc_service` + `DispatchIpcMessage` + domain/ipc.rs + ports/ipc_bus.rs — ACL validation, routing, session limits |
| 6 | **DONE** | `memory_service` + `domain/memory.rs` + `MemoryTiersPort` + `MemoryTiersAdapter` — tier types, recall formatting, consolidation policy |
| 7 | **DONE** | `CodingWorkerPort` + `DelegateImplementationTask` — domain types, port, use case |

### Domain types

| Type | Status |
|------|--------|
| `domain/channel.rs` | **DONE** — OutboundIntent, InboundEnvelope, ChannelCapability, SourceKind |
| `domain/conversation.rs` | **DONE** — ConversationSession, ConversationEvent, ConversationKind |
| `domain/run.rs` | **DONE** — Run, RunState, RunOrigin, RunEvent |
| `domain/ipc.rs` | **DONE** — IpcMessage, ValidatedSend, AclError, validate_send (7 ACL rules) |
| `domain/approval.rs` | **DONE** — ApprovalRequest, ApprovalResponse, QuarantineItem, ApprovalDecision |
| `domain/memory.rs` | **DONE** — MemoryCategory, MemoryEntry, SessionMemory, RecallConfig |
| `domain/implementation.rs` | **DONE** — ImplementationTask, CodingWorkerResult, ImplementationEvent |

### Ports (14 defined, all implemented)

| Port | Status | Adapter |
|------|--------|---------|
| `ports/channel_registry.rs` | **DONE** | `CachedChannelRegistry` |
| `ports/conversation_store.rs` | **DONE** | `ChatDbConversationStore` |
| `ports/run_store.rs` | **DONE** | `ChatDbRunStore` |
| `ports/conversation_history.rs` | **DONE** | `MutexMapConversationHistory` |
| `ports/route_selection.rs` | **DONE** | `MutexMapRouteSelection` |
| `ports/agent_runtime.rs` | **DONE** | `ChannelAgentRuntime` |
| `ports/channel_output.rs` | **DONE** | `ChannelOutputAdapter` |
| `ports/hooks.rs` | **DONE** | `HookRunnerAdapter` |
| `ports/session_summary.rs` | **DONE** | `SessionStoreAdapter` |
| `ports/memory.rs` (MemoryTiersPort) | **DONE** | `MemoryTiersAdapter` |
| `ports/approval.rs` | **DONE** | `ApprovalManager` (direct impl) |
| `ports/ipc_bus.rs` | **DONE** | gateway/ipc.rs (de facto adapter) |
| `ports/summary.rs` | **DONE** | `ProviderSummaryGenerator` |
| `ports/coding_worker.rs` | **DONE** | port defined; IPC-backed adapter deferred to Phase 4.1 |

### Deferred ports (not needed for Phase 4.0)

| Port | Reason |
|------|--------|
| `SchedulerPort` | Scheduling policy lives in `DeliveryService`; cron store is upstream-owned |
| `AuditPort` | Audit is cross-cutting; writes directly in gateway/ipc.rs |
| `IdentityPort` | Auth handled by IpcBusPort trust-level queries + pairing.rs |

### Use cases (8 of 10 implemented)

| Use Case | Status | Notes |
|----------|--------|-------|
| `HandleInboundMessage` | **DONE** | Slice 2: 24 behaviors, 7 ports, full orchestration |
| `StartConversationRun` | **DONE** | Slice 3: create + track + finalize (success/fail/interrupt) |
| `AbortConversationRun` | **DONE** | Guards terminal state, transitions to Cancelled |
| `RequestApproval` | **DONE** | Slice 4: approval workflow with session allowlist |
| `ReviewQuarantineItem` | **DONE** | Slice 4: promote/dismiss/list/quarantine_agent |
| `DispatchIpcMessage` | **DONE** | Slice 5: resolve → limit check → ACL validate → send |
| `DelegateImplementationTask` | **DONE** | Slice 7: submit to worker + track via RunStorePort |
| `SpawnChildAgent` | DEFERRED | Runtime spawn exists upstream; wrapping deferred to Phase 4.1 |
| `ResumeConversation` | DEFERRED | Session continuation implicit in conversation_service |
| `SendScheduledNotification` | **DONE** | Via DeliveryService (no separate use case file needed) |

### Cleanup and verification

| Item | Status |
|------|--------|
| Progress doc aligned with code reality | **DONE** |
| Delta registry updated (CORE-001..CORE-008) | **DONE** |
| 170+ fork_core unit tests passing | **DONE** |

---

## Critical acceptance criteria

| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | At least one migrated use case no longer depends on transport names | **DONE** | `DeliveryService` owns delivery policy; daemon/scheduler delegate to it |
| 2 | Heartbeat/scheduled notifications work for any channel that satisfies required capabilities and policy | **DONE** | Auto-detect uses `ChannelCapability::SendText` via registry; hardcoded priority removed |
| 3 | Web chat uses `ConversationStorePort`, not hardcoded embedded storage logic | **DONE** | ws.rs fully migrated to ConversationStorePort (PR #166) |
| 4 | Chat, IPC execution share unified `RunStorePort` | **DONE** | Web chat + IPC push runs tracked via RunStorePort (PRs #167, #168) |
| 5 | Session memory and long-term memory are explicit and separated | **DONE** | `MemoryTiersPort` defines two tiers: session (goal/summary via ConversationStorePort) + long-term (recall/store via Memory backends). `MemoryTiersAdapter` wraps both. |
| 6 | New fork logic lands in fork-owned modules instead of upstream hotspots | **DONE** | 7 application services, 8 use cases, 7 domain modules, 14 ports — all in fork_core. channels/mod.rs reduced by 4287 lines. |
| 7 | External coding engines can only attach through a narrow port | **DONE** | `CodingWorkerPort` (submit/poll/events/cancel) + `DelegateImplementationTask` use case. Domain types enforce bounded implementation contract. |

---

## Review checkpoints

### Pre-checkpoint: RunContext (PR #157)

`RunContext` (`src/agent/run_context.rs`) is the first concrete artifact toward
Phase 4.0's unified `Run` object.  It was introduced to solve the IPC auto-reply
safety net problem, but its design is intentionally Phase 4.0-aligned:

| RunContext (today) | Run (Phase 4.0) |
|--------------------|-----------------|
| `tool_events: Vec<ToolEvent>` | `ConversationEvent { event_type: tool_call }` |
| `was_ipc_reply_sent_for_session(sid)` | `RunStorePort::has_result(run_id)` |
| Passed via `Option<Arc<RunContext>>` to `agent::run()` | `RunStorePort` injected into core |
| Tracks tool name + success + IPC args | Tracks full event lifecycle |
| Created by gateway inbox processor | Created by any run origin (web, channel, IPC, spawn) |

**Migration path**: when Step 4 lands, `RunContext` gets absorbed into `Run` +
`RunStorePort`.  The `execute_one_tool` recording point stays — it just writes
to a port instead of an in-memory vec.  The gateway auto-reply becomes a
`RunStorePort` observer on run completion.

### Pre-checkpoint: OutboundIntent + ChannelRegistryPort (Steps 1-2)

First vertical slice of Phase 4.0 — hexagonal port/adapter pair.
Solves the concrete problem: push-triggered IPC results (e.g. copywriter →
marketing-lead) never reached the user's channel.

**What shipped:**

| Artifact | Location |
|----------|----------|
| `fork_core` module skeleton | `src/fork_core/{mod,domain/mod,domain/channel,bus,ports/mod,ports/channel_registry}.rs` |
| `fork_adapters` module skeleton | `src/fork_adapters/{mod,channels/mod,channels/registry}.rs` |
| `OutboundIntent`, `IntentKind`, `ChannelCapability`, `DegradationPolicy` | `src/fork_core/domain/channel.rs` |
| `ChannelRegistryPort` trait | `src/fork_core/ports/channel_registry.rs` |
| `CachedChannelRegistry` (long-lived adapters) | `src/fork_adapters/channels/registry.rs` |
| `OutboundIntentBus` (mpsc sender/receiver) | `src/fork_core/bus.rs` |
| Push relay with `scrub_credentials` + `pending_replies` guard | `src/gateway/mod.rs` |
| Auto-reply IPC payload scrubbed | `src/gateway/mod.rs` |
| `outbound_intent_relay` via ChannelRegistryPort | `src/daemon/mod.rs` |
| Config: `push_relay_channel`, `push_relay_recipient` | `src/config/schema.rs` |
| Matrix added to `build_channel_by_id` | `src/channels/mod.rs` |

### Checkpoint A — foundation ✅

After steps 1-4:
- module boundaries are clear
- core types are fixed
- conversation store contract is stable enough for downstream migration
- run substrate contract is stable enough for downstream migration

### Checkpoint B — first product slice ✅

After steps 5-6:
- scheduled delivery and heartbeat prove the capability model works
- transport-name branching starts shrinking in real product flows

### Checkpoint C — core orchestration ✅

After steps 7-9:
- inbound message flow, approvals, and selected IPC paths route through the new core
- conversation/run semantics stop being duplicated across web/channel/ipc

### Checkpoint D — memory and coding-worker seam ✅

After steps 10-14:
- memory tiers are explicit
- unified run storage is explicit
- external coding workers have a narrow, non-invasive attachment point
- migrated old paths are removed or minimized
- fork surface is smaller and easier to sync with upstream

---

## Summary statistics

| Metric | Value |
|--------|-------|
| Domain modules | 7 |
| Ports defined | 14 |
| Adapters | 11 (8 inbound + 2 storage + 1 channel registry) |
| Application services | 6 |
| Use cases | 8 (2 deferred to Phase 4.1) |
| fork_core unit tests | 170+ |
| Lines removed from channels/mod.rs | 4,287 |
| Total fork_core code | ~100KB |
| Total fork_adapters code | ~70KB |
