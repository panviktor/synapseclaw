# Phase 3.10 Progress: Push Loop Prevention

## Status: Implementation Complete

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | `PushMeta` struct + signal channel type | done | #133 | Signal carries `from_agent`, `kind`, `message_id` |
| 2 | Config fields | done | #133 | `push_max_auto_processes`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way` |
| 3 | Kind-based filtering | done | #133 | Only `task`/`query`/`result` trigger auto-processing; deferred to inbox processor |
| 4 | Broker-authoritative peek/ack | done | #135+ | `GET /api/ipc/inbox?peek=true` (non-consuming), `POST /api/ipc/ack` (explicit ack); local DB path removed |
| 5 | Per-peer counter + coalescing | done | #133 | One `agent::run()` per peer (sequential); `HashMap<peer, PeerState>` with `auto_process_count` |
| 6 | One-way trust check (broker-side) | done | #135+ | Trust level from broker peek response, not local DB; fail-closed on unknown trust |
| 7 | Deprecated local-DB path removed | done | #135+ | `agent_inbox_processor` uses HTTP client only; no `ipc_db` parameter |

---

## Architecture (final)

```
push arrives → kind filter (agent-side) → PushMeta{from, kind, message_id} → inbox processor
  → broker HTTP peek (GET /api/ipc/inbox?peek=true&from=X&kinds=task,query,result)
  → one-way trust check (from broker's from_trust_level)
  → per-peer counter check
  → inject messages into prompt → agent::run()
  → on success: broker HTTP ack (POST /api/ipc/ack {message_ids})
  → on failure: messages stay unread on broker
```

One production model. No local-DB fallback. No dual-path.

---

## Verification

### Kind filtering
- [x] `kind=task` push → auto-processed
- [x] `kind=query` push → auto-processed
- [x] `kind=result` push → auto-processed
- [x] `kind=text` push → 202 returned, no `agent::run()`
- [x] Config override adds/removes kinds

### Broker-authoritative peek/ack
- [x] Push from peer X → broker `peek_inbox(from=X)` called via HTTP (non-consuming)
- [x] Pre-fetched messages injected into prompt — LLM does not call `agents_inbox`
- [x] Messages from peer Y not visible in X-triggered run (hard guarantee)
- [x] Messages marked read only after successful `agent::run()` via `POST /api/ipc/ack`
- [x] Failed/timed-out run → messages stay unread on broker, picked up by next cycle
- [x] Manual/heartbeat inbox check still uses consuming `fetch_inbox()` (unaffected)
- [x] No local-DB dependency in push-triggered processing path

### Per-peer counter + coalescing
- [x] First message from new peer → processes
- [x] 4th consecutive push-triggered run for same peer → suppressed (WARN log)
- [x] Counter resets after cooldown (300s default)
- [x] Independent counters per peer
- [x] Coalesced multi-peer signals → one run per peer, sequential

### One-way mode
- [x] `push_one_way=false` (default) → existing behavior preserved
- [x] `push_one_way=true`, superior→subordinate → auto-processed
- [x] `push_one_way=true`, subordinate→superior → NOT auto-processed
- [x] `push_one_way=true`, lateral (same level) → auto-processed if kind matches
- [x] `push_one_way=true`, unknown trust level → fail-closed (skipped)
- [x] Suppressed messages still readable via inbox poll

### Tests (unit)
- [x] `peek_inbox_returns_messages_without_marking_read`
- [x] `peek_inbox_from_filter`
- [x] `peek_inbox_kinds_filter`
- [x] `ack_marks_peeked_messages_as_read`
- [x] `ack_only_affects_specified_ids`
- [x] `push_meta_carries_message_id`

### Prompt
- [x] Anti-ack instruction in prompt
- [x] Prompt contains pre-fetched messages, not inbox fetch instruction
