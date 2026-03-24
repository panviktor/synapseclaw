# IPC Phase 4.0 Progress

**Status**: all 7 application service slices complete; cleanup and verification remaining

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
| 2 | **DONE** | `inbound_message_service` + `HandleInboundMessage` — 7 ports, 7 adapters, orchestrator, old code deleted (−4287 lines) |
| 3 | **DONE** | `conversation_service` + `StartConversationRun` — session lifecycle, summary policy, run state machine |
| 4 | **DONE** | `approval_service` + `RequestApproval` + `ReviewQuarantineItem` — domain types, ports, policy, adapter |
| 5 | **DONE** | `ipc_service` + domain/ipc.rs + ports/ipc_bus.rs — ACL validation, routing, session limits |
| 6 | **DONE** | `memory_service` + `domain/memory.rs` — tier types, recall formatting, consolidation policy |
| 7 | **DONE** | `CodingWorkerPort` + `DelegateImplementationTask` — domain types, port, use case |

### Missing domain types

| Type | Status |
|------|--------|
| `domain/ipc.rs` | **DONE** |
| `domain/approval.rs` | **DONE** |
| `domain/memory.rs` | **DONE** |
| `domain/implementation.rs` | **DONE** |

### Ports

| Port | Status |
|------|--------|
| `ports/channel_registry.rs` | **DONE** |
| `ports/conversation_store.rs` | **DONE** |
| `ports/run_store.rs` | **DONE** |
| `ports/conversation_history.rs` | **DONE** |
| `ports/route_selection.rs` | **DONE** |
| `ports/agent_runtime.rs` | **DONE** |
| `ports/channel_output.rs` | **DONE** |
| `ports/hooks.rs` | **DONE** |
| `ports/session_summary.rs` | **DONE** |
| `ports/memory_tiers.rs` | TODO |
| `ports/approval.rs` | **DONE** |
| `ports/scheduler.rs` | TODO |
| `ports/ipc_bus.rs` | **DONE** |
| `ports/audit.rs` | TODO |
| `ports/identity.rs` | TODO |
| `ports/coding_worker.rs` | TODO |

### Cleanup and verification

| Item | Status |
|------|--------|
| Verification tests for capability routing, stores, adapter boundaries | TODO |
| Final docs + delta registry update | TODO |

---

## Critical acceptance criteria

| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | At least one migrated use case no longer depends on transport names | **DONE** | `DeliveryService` owns delivery policy; daemon/scheduler delegate to it |
| 2 | Heartbeat/scheduled notifications work for any channel that satisfies required capabilities and policy | **DONE** | Auto-detect uses `ChannelCapability::SendText` via registry; hardcoded priority removed |
| 3 | Web chat uses `ConversationStorePort`, not hardcoded embedded storage logic | **DONE** | ws.rs fully migrated to ConversationStorePort (PR #166) |
| 4 | Chat, IPC execution share unified `RunStorePort` | **DONE** | Web chat + IPC push runs tracked via RunStorePort (PRs #167, #168) |
| 5 | Session memory and long-term memory are explicit and separated | **NOT STARTED** | MemoryTiersPort not defined |
| 6 | New fork logic lands in fork-owned modules instead of upstream hotspots | **PARTIAL** | 1 application service (delivery_service). Remaining 5 services TODO |
| 7 | External coding engines can only attach through a narrow port | **NOT STARTED** | CodingWorkerPort not defined |

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

**Data flow:**

```
agent_inbox_processor (gateway)
  → IPC result arrives, agent::run() completes
  → scrub_credentials(last_text)
  → OutboundIntent::notify(relay_ch, relay_rcpt, scrubbed_text)
  → OutboundIntentSender.send()
  → outbound_intent_relay (daemon task)
  → CachedChannelRegistry::deliver()
    → resolve() (cached Arc<dyn Channel>)
    → capability check + degradation policy
    → channel.send()
  → user sees result in Matrix/Telegram
```

**Security:** Both auto-reply IPC payload and push relay text pass through
`scrub_credentials()`. Relay only fires when `pending_replies` is non-empty
(task/query delegation, not FYI text).

**Config to enable (per agent):**

```toml
[agents_ipc]
push_relay_channel = "matrix"        # or "telegram"
push_relay_recipient = "!room:server" # or chat_id
```

**What's NOT done yet:** `InboundEnvelope`, `ChannelCapabilities` as trait on
channel adapters, `fork_adapters` module.  These come in later steps.

### Checkpoint A — foundation

After steps 1-4:
- module boundaries are clear
- core types are fixed
- conversation store contract is stable enough for downstream migration
- run substrate contract is stable enough for downstream migration

### Checkpoint B — first product slice

After steps 5-6:
- scheduled delivery and heartbeat prove the capability model works
- transport-name branching starts shrinking in real product flows

### Checkpoint C — core orchestration

After steps 7-9:
- inbound message flow, approvals, and selected IPC paths route through the new core
- conversation/run semantics stop being duplicated across web/channel/ipc

### Checkpoint D — memory and coding-worker seam

After steps 10-14:
- memory tiers are explicit
- unified run storage is explicit
- external coding workers have a narrow, non-invasive attachment point
- migrated old paths are removed or minimized
- fork surface is smaller and easier to sync with upstream
