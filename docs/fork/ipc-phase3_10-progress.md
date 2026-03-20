# Phase 3.10 Progress: Push Loop Prevention

## Status: Pending

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | `PushMeta` struct + signal channel type | pending | — | Signal carries `from_agent`, `kind`, `from_trust_level` (from local DB, not HTTP body) |
| 2 | Config fields | pending | — | `push_max_auto_processes`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way` |
| 3 | Kind-based filtering + one-way check | pending | — | Only `task`/`query`/`result` trigger auto-processing; one-way suppresses subordinate→superior; fail-closed on unknown trust |
| 4 | Pre-fetch + inject (enforced scoped processing) | pending | — | Inbox processor fetches scoped messages, injects as prompt context; LLM never calls `agents_inbox`; broker gains `from`/`kinds` inbox params |
| 5 | Per-peer counter + coalescing | pending | — | One `agent::run()` per peer (sequential); `HashMap<peer, PeerState>` with `auto_process_count` |

---

## Verification

### Kind filtering
- [ ] `kind=task` push → auto-processed
- [ ] `kind=query` push → auto-processed
- [ ] `kind=result` push → auto-processed
- [ ] `kind=text` push → 202 returned, no `agent::run()`
- [ ] Config override adds/removes kinds

### Scoped inbox processing (pre-fetch + inject)
- [ ] Push from peer X → inbox processor pre-fetches from X only
- [ ] Pre-fetched messages injected into prompt — LLM does not call `agents_inbox`
- [ ] Messages from peer Y not visible in X-triggered run (hard guarantee)
- [ ] Accumulated text from other peers not swept into task-triggered run
- [ ] Manual/heartbeat inbox check still returns full inbox
- [ ] `GET /api/ipc/inbox?from=X&kinds=task,query,result` returns scoped subset

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

### Prompt
- [ ] Anti-ack instruction in prompt
- [ ] Prompt contains pre-fetched messages, not inbox fetch instruction
- [ ] Agent does not send pointless acknowledgments
