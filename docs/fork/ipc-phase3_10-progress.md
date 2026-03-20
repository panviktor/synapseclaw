# Phase 3.10 Progress: Push Loop Prevention

## Status: Pending

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | `PushMeta` struct + signal channel type | pending | — | Signal carries `from_agent`, `kind`, `from_trust_level` (from local DB, not HTTP body) |
| 2 | Config fields | pending | — | `push_max_auto_processes`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way` |
| 3 | Kind-based filtering + one-way check | pending | — | Only `task`/`query`/`result` trigger auto-processing; one-way suppresses subordinate→superior |
| 4 | Scoped inbox processing | pending | — | `agents_inbox` tool gains `from_agent`/`kinds` filters; broker inbox endpoint adds query params |
| 5 | Per-peer counter + scoped prompt | pending | — | `HashMap<peer, PeerState>` with `auto_process_count`; prompt names triggering peer |

---

## Verification

### Kind filtering
- [ ] `kind=task` push → auto-processed
- [ ] `kind=query` push → auto-processed
- [ ] `kind=result` push → auto-processed
- [ ] `kind=text` push → 202 returned, no `agent::run()`
- [ ] Config override adds/removes kinds

### Scoped inbox processing
- [ ] Push from peer X → `agents_inbox` called with `from_agent=X`
- [ ] Messages from peer Y not swept into X-triggered run
- [ ] Manual/heartbeat inbox check still returns full inbox
- [ ] Filter params optional, defaults match current behavior

### Per-peer counter
- [ ] First message from new peer → processes
- [ ] 4th consecutive push-triggered run for same peer → suppressed (WARN log)
- [ ] Counter resets after cooldown (300s default)
- [ ] Independent counters per peer
- [ ] Coalescing preserved

### One-way mode
- [ ] `push_one_way=false` (default) → existing behavior preserved
- [ ] `push_one_way=true`, superior→subordinate → auto-processed
- [ ] `push_one_way=true`, subordinate→superior → NOT auto-processed
- [ ] `push_one_way=true`, lateral (same level) → auto-processed if kind matches
- [ ] Suppressed messages still readable via inbox poll

### Legitimate workflow chains
- [ ] L1 task → L3 result → L1 processes result → workflow completes
- [ ] Multi-step delegation converges correctly
- [ ] Suppressed messages not starved (heartbeat/poll picks them up)

### Prompt
- [ ] Anti-ack instruction in prompt
- [ ] Prompt names triggering peer
- [ ] Agent does not send pointless acknowledgments
