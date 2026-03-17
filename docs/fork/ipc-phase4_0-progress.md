# IPC Phase 4.0 Progress

**Status**: not started

Phase 3.7b: session intelligence | **Phase 4.0: modular core refactor** | Phase 4.1: federated execution

---

## Goal

Refactor the fork toward a pragmatic ports-and-adapters architecture with:

- fork-owned application core
- capability-driven channel behavior
- unified conversation store contract
- explicit memory tiers
- lower merge surface with upstream

---

## Checklist

| Step | Status | Description |
|------|--------|-------------|
| 1 | TODO | Create `fork_core` / `fork_adapters` module skeleton and document ownership boundaries |
| 2 | TODO | Define canonical `InboundEnvelope`, `OutboundIntent`, and `ChannelCapabilities` types |
| 3 | TODO | Add `ConversationStorePort` over the current chat/session SQLite implementation |
| 4 | TODO | Migrate scheduled notification delivery to capability-driven `SendScheduledNotification` |
| 5 | TODO | Migrate heartbeat target validation/auto-detect away from hardcoded channel-name whitelists |
| 6 | TODO | Route one inbound human channel through `HandleInboundMessage` use case |
| 7 | TODO | Extract approval/quarantine orchestration into fork-owned application services |
| 8 | TODO | Bridge selected IPC flows into the same conversation/run model |
| 9 | TODO | Add `MemoryTiersPort` adapters for working/session/long-term memory |
| 10 | TODO | Remove migrated transport-name branching from old paths |
| 11 | TODO | Add verification tests for capability routing, conversation storage, and adapter boundaries |
| 12 | TODO | Update docs, delta registry, and sync notes after first migrated slices land |

---

## Critical acceptance criteria

1. At least one migrated use case no longer depends on transport names.
2. Heartbeat/scheduled notifications work for any channel that satisfies required capabilities and policy.
3. Web chat uses `ConversationStorePort`, not hardcoded embedded storage logic.
4. Session memory and long-term memory are explicit and separated.
5. New fork logic lands in fork-owned modules instead of upstream hotspots by default.

---

## Review checkpoints

### Checkpoint A — foundation

After steps 1-3:
- module boundaries are clear
- core types are fixed
- conversation store contract is stable enough for downstream migration

### Checkpoint B — first product slice

After steps 4-5:
- scheduled delivery and heartbeat prove the capability model works
- transport-name branching starts shrinking in real product flows

### Checkpoint C — core orchestration

After steps 6-8:
- inbound message flow, approvals, and selected IPC paths route through the new core
- conversation/run semantics stop being duplicated across web/channel/ipc

### Checkpoint D — memory and cutover

After steps 9-12:
- memory tiers are explicit
- migrated old paths are removed or minimized
- fork surface is smaller and easier to sync with upstream
