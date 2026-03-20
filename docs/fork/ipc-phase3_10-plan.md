# IPC Phase 3.10: Push Loop Prevention

Phase 3.9: operator control plane | **Phase 3.10: push loop prevention** | Phase 4.0: modular core refactor

---

## Problem

Push delivery (Phase 3.9 Step 1, PR #130) introduced a production bug: agents get stuck in **infinite acknowledgment loops**.

When Agent A sends a message to Agent B:

1. Broker pushes to B → B's inbox processor runs `agent::run()` → LLM reads message, generates acknowledgment → sends reply to A
2. Broker pushes to A → A's inbox processor runs `agent::run()` → LLM reads ack, generates ack-of-ack → sends reply to B
3. Repeat indefinitely

**Observed in production** (`marketing-lead` ↔ `copywriter`, 2026-03-19): 8+ meaningless acknowledgment exchanges in 2 minutes ("ok understood" → "confirmed" → "acknowledged your confirmation" → ...), then `marketing-lead` stuck polling empty inbox. Both agents burned tokens on Qwen API calls for zero useful work.

Root cause: `agent_inbox_processor()` (`gateway/mod.rs:1148`) unconditionally invokes a full `agent::run()` on every push signal with the prompt "Check your IPC inbox… Process all pending messages and respond appropriately." Each response creates a new `message_id`, so existing dedup (by message_id) cannot break the cycle.

### Why existing protections don't help

| Protection | Why insufficient |
|-----------|-----------------|
| Push dedup set (`message_id`) | Deduplicates same message_id; each response is a new message |
| Per-agent rate limit (`max_messages_per_hour=60`) | 60/hr ≈ 1/min — loop runs faster but stays within limit |
| Session max exchanges (`session_max_exchanges=10`) | Only for lateral same-level + requires matching `session_id`; agents sending without session_id bypass entirely |
| Tool iteration limit (`MAX_TOOL_ITERATIONS=10`) | Per-run limit; loop is cross-run (each push creates a new run) |

### Research context

Analysis of AutoGen, Langroid, CrewAI, MetaGPT, ChatDev, and OpenAI Swarm confirmed that prompt-only fixes are unreliable (~+14% improvement per MAST paper, arXiv:2503.13657). Robust approaches combine **structural filtering** (MetaGPT's `cause_by` routing, ChatDev's phase decomposition) with **hard counters** (AutoGen's `max_consecutive_auto_reply`, Langroid's `max_stalled_steps`).

---

## Scope

### In scope

1. **Kind-based push filtering** — only auto-process `task`/`query` message kinds; `text`/`result` delivered but not auto-processed
2. **Per-peer auto-reply counter** — max N consecutive auto-replies to same peer before suppression, with cooldown reset
3. **One-way dispatch mode** — configurable flag: subordinate agents (higher trust level number) cannot auto-reply to push from a superior; results are sent via poll/manual only
4. **Improved inbox processing prompt** — explicit anti-acknowledgment instruction
5. **Configurable thresholds** — `push_max_auto_replies`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way`

### Non-goals

- Semantic similarity loop detection (embedding-based dedup — overhead not justified yet)
- Conversation state machines in message envelope (future enhancement)
- Broker-side loop detection (this is agent-side only; push delivery still best-effort)
- Changes to IPC send flow or ACL rules (existing `validate_send` unchanged)
- Guaranteed delivery / replacing polling (push remains best-effort)

---

## Architecture

### Push loop suppression flow

```
Broker                          Agent Gateway
  │                                │
  │── POST /api/ipc/push ────────>│
  │   {message_id, from, kind}    │
  │                                │
  │                          ┌─────┴──────┐
  │                          │ Kind filter │
  │                          └─────┬──────┘
  │                           kind ∈ auto_kinds?
  │                          /              \
  │                        yes               no
  │                         │                 │
  │                   ┌─────┴──────┐     log + skip
  │                   │ One-way    │     (await poll)
  │                   │ check      │
  │                   └─────┬──────┘
  │                    one_way + subordinate?
  │                   /              \
  │                 no                yes
  │                  │                 │
  │            ┌─────┴──────┐     log + skip
  │            │ Peer count │     (no auto-reply)
  │            └─────┬──────┘
  │             count < max?
  │            /              \
  │          yes               no
  │           │                 │
  │     agent::run()        log WARN
  │     (inbox check)       (suppressed)
  │           │
  │     count++ for peer
  │
  │<── 202 Accepted ──────────│
```

Push always returns 202 (message is delivered to inbox regardless). Suppression only affects whether `agent::run()` is triggered automatically.

### One-way dispatch model

```
Operator (L1)                 Broker                    Worker (L3)
  │                             │                          │
  │── send task ───────────────>│── push (kind=task) ────>│
  │                             │                          │
  │                             │   push_one_way=true      │
  │                             │   L1 < L3 (superior)     │
  │                             │   ∴ auto-process: YES    │
  │                             │                          │
  │                             │   Worker processes task  │
  │                             │   Worker sends result    │
  │                             │                          │
  │                             │── push (kind=result) ──>│ (to operator)
  │                             │                          │
  │   push_one_way=true         │                          │
  │   L3 > L1 (subordinate)    │                          │
  │   ∴ auto-process: NO        │                          │
  │   (result waits for poll)   │                          │
```

In one-way mode, push auto-processing only fires when the sender has **equal or lower** trust level number (i.e., equal or higher authority). A subordinate's response is delivered to inbox but does not trigger automatic processing — the operator picks it up on next poll or manual inbox check. This prevents subordinate responses from cascading into ack loops.

Trust level semantics: L1 (operator) > L2 (supervisor) > L3 (worker) > L4 (restricted). "Subordinate" = higher trust level number.

### Signal channel change

Current push signal carries no metadata:

```
UnboundedSender<()>  →  agent_inbox_processor receives ()
```

New signal carries peer identity and message kind:

```
UnboundedSender<PushMeta>  →  agent_inbox_processor receives { from_agent, kind, from_trust_level }
```

This enables kind filtering, one-way check, and per-peer counting without additional lookups.

---

## Implementation Steps

### Step 1: `PushMeta` struct and signal channel type

Add `PushMeta` struct to `src/gateway/ipc.rs` (after `PushJob`):

```rust
#[derive(Debug, Clone)]
pub struct PushMeta {
    pub from_agent: String,
    pub kind: String,
    pub from_trust_level: Option<u8>,
}
```

Change `AppState.ipc_push_signal` from `UnboundedSender<()>` to `UnboundedSender<PushMeta>` in `src/gateway/mod.rs`. Update channel creation at spawn site.

### Step 2: Config fields

Add to `AgentsIpcConfig` in `src/config/schema.rs`:

- `push_max_auto_replies: u32` — max consecutive auto-replies to same peer (default: 3)
- `push_peer_cooldown_secs: u64` — cooldown before resetting per-peer counter (default: 300)
- `push_auto_process_kinds: Vec<String>` — kinds that trigger auto-processing (default: `["task", "query"]`)
- `push_one_way: bool` — one-way dispatch mode, subordinates can't trigger auto-processing on superiors (default: false)

### Step 3: Kind-based filtering + one-way check in push receiver

Modify `handle_ipc_push_notification()` in `src/gateway/ipc.rs`:

- Read `push_auto_process_kinds` from config
- Only send `PushMeta` signal if kind is in the auto-process list
- For non-matching kinds: log at DEBUG, return 202, message stays in inbox

One-way check also happens here: if `push_one_way=true` and sender's trust level > this agent's trust level (sender is subordinate), skip signaling.

### Step 4: Per-peer counter and improved prompt in inbox processor

Rewrite `agent_inbox_processor()` in `src/gateway/mod.rs`:

- Accept `UnboundedReceiver<PushMeta>` instead of `UnboundedReceiver<()>`
- Maintain `HashMap<String, PeerState>` tracking `auto_reply_count` and `last_processed` per peer
- On signal: coalesce (100ms drain), check per-peer counter < max, invoke `agent::run()` if allowed
- After successful run: increment counters, update timestamps
- Reset counter when cooldown elapsed
- Improved prompt with anti-ack instruction

---

## Verification Checklist

### Kind filtering
- [ ] `kind=task` push → auto-processed
- [ ] `kind=query` push → auto-processed
- [ ] `kind=text` push → 202 returned, no `agent::run()` triggered
- [ ] `kind=result` push → 202 returned, no `agent::run()` triggered
- [ ] Config override: adding `"text"` to `push_auto_process_kinds` enables text auto-processing

### Per-peer counter
- [ ] First `task` from new peer → always processes (count starts at 0)
- [ ] 4th consecutive auto-reply to same peer → suppressed with WARN log
- [ ] After 5min cooldown → counter resets, new pushes process normally
- [ ] Different peers have independent counters
- [ ] Coalescing preserved (multiple rapid pushes from same peer → single run)

### One-way mode
- [ ] `push_one_way=false` (default) → all pushes processed normally (existing behavior)
- [ ] `push_one_way=true`, L1→L3 push → L3 auto-processes (superior → subordinate)
- [ ] `push_one_way=true`, L3→L1 push → L1 does NOT auto-process (subordinate → superior)
- [ ] `push_one_way=true`, L3→L3 push → auto-processes if kind matches (lateral = same level, no suppression)
- [ ] One-way suppressed messages still in inbox, readable via poll

### Prompt
- [ ] Prompt includes anti-ack instruction
- [ ] Agent does not generate pointless acknowledgments on auto-processed pushes

### Rollback
- [ ] `push_max_auto_replies=1000` effectively disables counter
- [ ] `push_auto_process_kinds=["task","query","text","result"]` restores old kind behavior
- [ ] `push_one_way=false` disables one-way mode

---

## Risks

| Risk | Mitigation |
|------|-----------|
| Legitimate `text` messages not auto-processed | Configurable via `push_auto_process_kinds`; messages still in inbox for poll/manual check |
| Per-peer counter too aggressive for long task chains | Default 3 is generous for ping-pong; genuine multi-step task delegation uses `session_id` and explicit `task`/`result` kinds, not free-form `text` |
| One-way mode blocks legitimate subordinate-initiated alerts | One-way is opt-in (default: false); subordinates can still send messages, they just don't trigger auto-processing on the superior |
| Cooldown too long/short for specific workflows | Configurable `push_peer_cooldown_secs`; 300s default balances loop prevention vs responsiveness |
| Signal channel type change breaks tests | All test `AppState` initializations use `ipc_push_signal: None` — type-agnostic via inference |

---

## Decisions

1. **Kind-based filtering is the primary defense** — structural, not heuristic. Inspired by MetaGPT's `cause_by` routing.
2. **Per-peer counter inspired by AutoGen's `max_consecutive_auto_reply`** — the most battle-tested pattern across multi-agent frameworks.
3. **Prompt improvement is supplementary, not relied upon** — LLMs may ignore instructions; the structural layers provide hard guarantees.
4. **Agent-side only** — no broker changes needed. Push is still delivered (202), messages are in inbox. Only auto-processing is gated.
5. **`text` excluded from auto-processing by default** — the primary loop vector. Operator can opt in.
6. **One-way mode is opt-in** — default `false` preserves existing behavior. When enabled, provides MetaGPT-style instructor/assistant asymmetry using existing trust levels.
7. **No semantic similarity detection in v1** — embeddings add latency and complexity. The structural approach (kind filter + counter) is sufficient. Can be added in a future phase if needed.
