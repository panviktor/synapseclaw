# IPC Phase 4.0 Progress

**Status**: groundwork in progress

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

| Step | Status | Description |
|------|--------|-------------|
| 1 | **DONE** | Create `fork_core` / `fork_adapters` module skeleton and document ownership boundaries |
| 2 | **DONE** | Define canonical `OutboundIntent`, `ChannelCapabilities`, `ChannelRegistryPort` trait + `CachedChannelRegistry` adapter |
| 3 | **DONE** | `ConversationStorePort` trait + `ChatDbConversationStore` adapter over existing `ChatDb` SQLite |
| 4 | **DONE** | `RunStorePort` trait + `ChatDbRunStore` adapter with `runs` + `run_events` tables in ChatDb |
| 5 | **DONE** | Migrate scheduled notification delivery to capability-driven `ChannelRegistryPort.deliver()` |
| 6 | **DONE** | Migrate heartbeat delivery + validation away from hardcoded channel-name matches |
| 7 | **DONE** | Route inbound messages through `InboundEnvelope` at dispatch boundary + `HandleInboundMessage` application module |
| 8 | TODO | Extract approval/quarantine orchestration into fork-owned application services |
| 9 | TODO | Bridge selected IPC flows into the same conversation/run model |
| 10 | TODO | Add `MemoryTiersPort` adapters for working/session/long-term memory |
| 11 | TODO | Define `CodingWorkerPort` and `DelegateImplementationTask` as a narrow seam for external coding executors over IPC + `RunStorePort` |
| 12 | TODO | Remove migrated transport-name branching from old paths |
| 13 | TODO | Add verification tests for capability routing, conversation storage, run storage, and adapter boundaries |
| 14 | TODO | Update docs, delta registry, and sync notes after first migrated slices land |

---

## Critical acceptance criteria

1. At least one migrated use case no longer depends on transport names.
2. Heartbeat/scheduled notifications work for any channel that satisfies required capabilities and policy.
3. Web chat uses `ConversationStorePort`, not hardcoded embedded storage logic.
4. Chat, IPC execution, and future external workers share a unified `RunStorePort` contract instead of separate ad hoc run tables.
5. Session memory and long-term memory are explicit and separated.
6. New fork logic lands in fork-owned modules instead of upstream hotspots by default.
7. External coding engines can only attach through a narrow port and do not become new application cores.

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
