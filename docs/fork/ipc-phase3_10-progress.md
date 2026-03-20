# Phase 3.10 Progress: Push Loop Prevention

## Status: Pending

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | `PushMeta` struct + signal channel type | pending | — | Signal carries `from_agent`, `kind`, `from_trust_level` |
| 2 | Config fields | pending | — | `push_max_auto_replies`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way` |
| 3 | Kind-based filtering + one-way check | pending | — | Only `task`/`query` trigger auto-processing; one-way suppresses subordinate→superior |
| 4 | Per-peer counter + improved prompt | pending | — | `HashMap<peer, PeerState>` in inbox processor, anti-ack prompt |

---

## Verification

### Kind filtering
- [ ] `kind=task` push → auto-processed
- [ ] `kind=query` push → auto-processed
- [ ] `kind=text` push → 202 returned, no `agent::run()`
- [ ] `kind=result` push → 202 returned, no `agent::run()`
- [ ] Config override adds `"text"` to auto-process kinds

### Per-peer counter
- [ ] First message from new peer → processes
- [ ] 4th consecutive reply to same peer → suppressed (WARN log)
- [ ] Counter resets after cooldown (300s default)
- [ ] Independent counters per peer
- [ ] Coalescing preserved

### One-way mode
- [ ] `push_one_way=false` (default) → existing behavior preserved
- [ ] `push_one_way=true`, superior→subordinate → auto-processed
- [ ] `push_one_way=true`, subordinate→superior → NOT auto-processed
- [ ] `push_one_way=true`, lateral (same level) → auto-processed if kind matches
- [ ] Suppressed messages still readable via inbox poll

### Prompt
- [ ] Anti-ack instruction in prompt
- [ ] Agent does not send pointless acknowledgments
