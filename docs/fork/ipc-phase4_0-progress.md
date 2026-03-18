# IPC Phase 4.0 Progress

**Status**: not started

Phase 3.7b: session intelligence | **Phase 4.0: modular core refactor** | Phase 4.1: federated execution

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
| 1 | TODO | Create `fork_core` / `fork_adapters` module skeleton and document ownership boundaries |
| 2 | TODO | Define canonical `InboundEnvelope`, `OutboundIntent`, and `ChannelCapabilities` types |
| 3 | TODO | Add `ConversationStorePort` over the current chat/session SQLite implementation |
| 4 | TODO | Add `RunStorePort` and define unified run records/events for chat, IPC, and external workers |
| 5 | TODO | Migrate scheduled notification delivery to capability-driven `SendScheduledNotification` |
| 6 | TODO | Migrate heartbeat target validation/auto-detect away from hardcoded channel-name whitelists |
| 7 | TODO | Route one inbound human channel through `HandleInboundMessage` use case |
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
