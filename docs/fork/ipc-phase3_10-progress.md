# Phase 3.10 Progress: Push Loop Prevention

## Status: Pending

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | `PushMeta` struct + signal channel type | pending | — | Signal carries `from_agent`, `kind`, `from_trust_level` (from local DB via `peek_inbox`, not HTTP body) |
| 2 | Config fields | pending | — | `push_max_auto_processes`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way` |
| 3 | Kind-based filtering + one-way check | pending | — | Only `task`/`query`/`result` trigger auto-processing; one-way suppresses subordinate→superior; fail-closed on unknown trust |
| 4 | Pre-fetch + inject (enforced scoped processing) | pending | — | `peek_inbox()` (non-consuming) + `ack_messages()` (after successful run); LLM never calls `agents_inbox`; at-least-once delivery |
| 5 | Per-peer counter + coalescing | pending | — | One `agent::run()` per peer (sequential); `HashMap<peer, PeerState>` with `auto_process_count` |

---

## Verification

### Kind filtering
- [ ] `kind=task` push → auto-processed
- [ ] `kind=query` push → auto-processed
- [ ] `kind=result` push → auto-processed
- [ ] `kind=text` push → 202 returned, no `agent::run()`
- [ ] Config override adds/removes kinds

### Scoped inbox processing (peek + inject + ack)
- [ ] Push from peer X → `peek_inbox(from=X)` called (non-consuming)
- [ ] Pre-fetched messages injected into prompt — LLM does not call `agents_inbox`
- [ ] Messages from peer Y not visible in X-triggered run (hard guarantee)
- [ ] Messages marked read only after successful `agent::run()`
- [ ] Failed/timed-out run → messages stay unread, picked up by next cycle
- [ ] Manual/heartbeat inbox check still uses consuming `fetch_inbox()` (unaffected)
- [ ] No peek/fetch TTL cleanup race

### Per-peer counter + coalescing
- [ ] First message from new peer → processes
- [ ] 4th consecutive push-triggered run for same peer → suppressed (WARN log)
- [ ] Counter resets after cooldown (300s default)
- [ ] Independent counters per peer
- [ ] Coalesced multi-peer signals → one run per peer, sequential

### One-way mode
- [ ] `push_one_way=false` (default) → existing behavior preserved
- [ ] `push_one_way=true`, superior→subordinate → auto-processed
- [ ] `push_one_way=true`, subordinate→superior → NOT auto-processed
- [ ] `push_one_way=true`, lateral (same level) → auto-processed if kind matches
- [ ] `push_one_way=true`, unknown trust level → fail-closed (skipped)
- [ ] Suppressed messages still readable via inbox poll

### Legitimate workflow chains
- [ ] L1 task → L3 result → L1 processes result → workflow completes
- [ ] Multi-step delegation converges correctly
- [ ] Suppressed messages not starved (heartbeat/poll picks them up)
- [ ] Duplicate processing on re-delivery is safe (idempotent)

### Prompt
- [ ] Anti-ack instruction in prompt
- [ ] Prompt contains pre-fetched messages, not inbox fetch instruction
- [ ] Agent does not send pointless acknowledgments
