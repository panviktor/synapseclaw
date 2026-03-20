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

Root cause has **two layers**:

1. **Wake-up is unscoped** — `handle_ipc_push_notification()` sends a bare `()` signal with no context about who sent what.
2. **Inbox processing is unscoped** — `agent_inbox_processor()` invokes `agent::run()` with "process **all** pending messages and respond appropriately." Even if the wake-up signal was filtered, the inbox sweep fetches everything, including `text`/`result` messages from other peers that weren't part of the trigger.

Fixing only the wake-up signal (layer 1) without scoping the inbox run (layer 2) leaves a gap: a legitimate `task` push triggers a run that also processes accumulated `text` messages, potentially re-entering the ack loop.

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

1. **Kind-based push filtering** — only auto-process `task`/`query`/`result` message kinds; `text` delivered but not auto-processed
2. **Scoped inbox processing** — push-triggered run processes only messages from the triggering peer and matching kinds, not the entire inbox
3. **Per-peer auto-process counter** — max N consecutive push-triggered runs for same peer before suppression, with cooldown reset
4. **One-way dispatch mode** — configurable flag: subordinate agents (higher trust level number) cannot trigger auto-processing on a superior; messages are sent via poll/manual only
5. **Improved inbox processing prompt** — explicit anti-acknowledgment instruction, scoped to triggering peer
6. **Configurable thresholds** — `push_max_auto_processes`, `push_peer_cooldown_secs`, `push_auto_process_kinds`, `push_one_way`

### Non-goals

- Semantic similarity loop detection (embedding-based dedup — overhead not justified yet)
- Conversation state machines in message envelope (future enhancement)
- Broker-side loop detection (this is agent-side only; push delivery still best-effort)
- Changes to IPC send flow or ACL rules (existing `validate_send` unchanged)
- Guaranteed delivery / replacing polling (push remains best-effort)

---

## Architecture

### Key design principle: scope the processing unit

The fundamental fix is **not** just filtering wake-up signals — it's ensuring that the processing unit (the `agent::run()` invocation) is scoped to the triggering context:

```
Current (broken):
  push from peer X (kind=task) → agent::run("process ALL pending messages")
  → LLM calls agents_inbox → gets full inbox (text from Y, result from Z, task from X)
  → processes everything → replies to X, Y, Z → triggers pushes to all

Fixed:
  push from peer X (kind=task) → inbox processor pre-fetches messages (from=X, kinds=auto_kinds)
  → formats as context → agent::run("Here are messages from X: [...]. Process them.")
  → LLM processes injected messages directly → no agents_inbox call needed
  → replies to X only → counter increments for X only
```

This is achieved by **pre-fetching and injecting** messages into the prompt, not by asking the LLM to call `agents_inbox` with filters. The LLM never touches the inbox tool in push-triggered runs — it receives pre-fetched messages as context. This is an enforced structural guarantee: even a misbehaving model cannot bypass the scope because it has no access to the unfiltered inbox.

The `agents_inbox` tool remains available for manual/CLI/heartbeat use, and optionally gains `from_agent`/`kinds` filter parameters as a general improvement. But push-triggered runs do not rely on it.

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
  │            │ Peer count │     (no auto-process)
  │            └─────┬──────┘
  │             count < max?
  │            /              \
  │          yes               no
  │           │                 │
  │     agent::run()        log WARN
  │     (SCOPED inbox)      (suppressed)
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

### Trust level source

`from_trust_level` in `PushMeta` **must not** come from the HTTP push body (untrusted, broker-controlled payload). The agent-side push receiver resolves it by looking up the sender in its local IPC DB via `message_id`:

```
push body: { message_id: 42, from: "copywriter", kind: "text" }
                                    │
                    ┌───────────────┘
                    ▼
    local IPC DB: SELECT from_trust_level FROM messages WHERE id = 42
                    │
                    ▼
    PushMeta { from_trust_level: Some(3) }
```

If the message is not yet in local DB (push arrived before poll), the push receiver calls `GET /api/ipc/inbox` (which also fetches the message) and extracts `from_trust_level` from the response. If that also fails, `from_trust_level` is set to `None`.

**Fail-closed contract for `push_one_way`**: when `push_one_way=true` and `from_trust_level` is `None` (unknown), auto-processing is **skipped** — the message waits for poll. This is fail-closed: we never auto-process a push from a sender whose trust level we cannot verify when one-way mode is active. When `push_one_way=false`, unknown trust level is irrelevant (one-way check is not performed).

The broker's `from_trust_level` field on stored messages is authoritative because it's set at insert time from authenticated token metadata.

---

## Implementation Steps

### Step 1: `PushMeta` struct and signal channel type

Add `PushMeta` struct to `src/gateway/ipc.rs` (after `PushJob`):

```rust
#[derive(Debug, Clone)]
pub struct PushMeta {
    pub from_agent: String,
    pub kind: String,
    /// Resolved from local IPC DB, NOT from push HTTP body.
    pub from_trust_level: Option<u8>,
}
```

Change `AppState.ipc_push_signal` from `UnboundedSender<()>` to `UnboundedSender<PushMeta>` in `src/gateway/mod.rs`. Update channel creation at spawn site.

In `handle_ipc_push_notification`: resolve `from_trust_level` by looking up `message_id` in local IPC DB. If not found (message not yet polled), set `None`.

### Step 2: Config fields

Add to `AgentsIpcConfig` in `src/config/schema.rs`:

- `push_max_auto_processes: u32` — max consecutive push-triggered runs for same peer (default: 3)
- `push_peer_cooldown_secs: u64` — cooldown before resetting per-peer counter (default: 300)
- `push_auto_process_kinds: Vec<String>` — kinds that trigger auto-processing (default: `["task", "query", "result"]`)
- `push_one_way: bool` — one-way dispatch mode, subordinates can't trigger auto-processing on superiors (default: false)

Note: `result` is included in default auto-process kinds because legitimate orchestration chains (L1 sends task → L3 sends result → L1 needs to process result to continue) require it. The per-peer counter and one-way mode prevent result-triggered loops.

### Step 3: Kind-based filtering + one-way check in push receiver

Modify `handle_ipc_push_notification()` in `src/gateway/ipc.rs`:

- Read `push_auto_process_kinds` from config
- Only send `PushMeta` signal if kind is in the auto-process list
- For non-matching kinds: log at DEBUG, return 202, message stays in inbox

One-way check also happens here: if `push_one_way=true` and `from_trust_level > this agent's trust level` (sender is subordinate), skip signaling.

### Step 4: Pre-fetch + inject (enforced scoped processing)

The inbox processor **pre-fetches** scoped messages and injects them into the prompt. The LLM never calls `agents_inbox` in push-triggered runs — it receives messages as context.

Implementation in `agent_inbox_processor()` (`src/gateway/mod.rs`):

1. After coalescing and peer-counter check, call `GET /api/ipc/inbox?from={peer}&kinds={auto_kinds}` directly (HTTP call to self or direct DB call if same-process broker)
2. Format fetched messages into structured context
3. Pass as prompt to `agent::run()`

```rust
// Pre-fetch: inbox processor fetches scoped messages itself
let url = format!("{}/api/ipc/inbox?from={}&kinds={}",
    self_gateway_url, peer, auto_kinds.join(","));
let messages = fetch_scoped_inbox(&url, &proxy_token).await;

// Inject: messages become prompt context, not something the LLM fetches
let prompt = format!(
    "[IPC push: {} new message(s) from \"{peer}\"]\n\n\
     {formatted_messages}\n\n\
     Process the messages above and take action if required.\n\
     IMPORTANT: Do NOT send acknowledgments, confirmations, or \
     \"understood\" messages. Only reply if the message requires \
     concrete action or contains a question that needs answering.",
    messages.len(), peer = peer
);
```

This is the **hard structural guarantee**: the LLM cannot bypass scoping because it never has access to the unfiltered inbox. It sees only pre-fetched messages from the triggering peer.

**Broker-side change** (minimal): add optional `from` and `kinds` query params to `GET /api/ipc/inbox` endpoint (`handle_ipc_inbox` in `src/gateway/ipc.rs`). These filter the `fetch_inbox()` SQL query. Params are optional — omitting them returns the full inbox (backward-compatible).

**`agents_inbox` tool** (`src/tools/agents_ipc.rs`): optionally gains `from_agent`/`kinds` params as a general improvement for CLI/heartbeat use. But push-triggered runs do not use this tool — they use the pre-fetch path.

### Step 5: Per-peer counter and coalescing in inbox processor

Rewrite `agent_inbox_processor()` in `src/gateway/mod.rs`:

- Accept `UnboundedReceiver<PushMeta>` instead of `UnboundedReceiver<()>`
- Maintain `HashMap<String, PeerState>` tracking `auto_process_count` and `last_processed` per peer
- Coalescing with one run per peer:

```
Coalescing semantics:

  Signals arrive: [X/task, X/result, Y/task, X/task]
                   ──── 100ms drain ────
  Deduplicate to unique peers: [X, Y]

  For each peer (sequentially):
    1. Check peer counter < max
    2. Pre-fetch scoped inbox (from=peer, kinds=auto_kinds)
    3. If messages found → agent::run() with injected messages
    4. Increment counter for peer
```

Each peer gets its own `agent::run()` with its own scoped context. Multiple peers within the same coalescing window are processed **sequentially**, not merged into a single run. This ensures:
- Each run's scope is exactly one peer
- Counter increments are precise (one per peer per run)
- No cross-peer message leakage

- After successful run: increment `auto_process_count` for that peer, update timestamp
- Reset counter when cooldown elapsed

Counter semantics: `auto_process_count` counts how many times a push from peer X triggered an `agent::run()`, not how many replies were sent. This is an honest metric — we can't reliably count outgoing replies to specific peers without inspecting the run's tool calls. The counter serves as a circuit breaker: "stop auto-processing pushes from X after N runs, regardless of whether those runs actually replied."

---

## Verification Checklist

### Kind filtering
- [ ] `kind=task` push → auto-processed
- [ ] `kind=query` push → auto-processed
- [ ] `kind=result` push → auto-processed (needed for orchestration chains)
- [ ] `kind=text` push → 202 returned, no `agent::run()` triggered
- [ ] Config override: adding `"text"` to `push_auto_process_kinds` enables text auto-processing
- [ ] Config override: removing `"result"` disables result auto-processing

### Scoped inbox processing (pre-fetch + inject)
- [ ] Push from peer X → inbox processor pre-fetches messages from X only
- [ ] Pre-fetched messages injected into prompt — LLM does not call `agents_inbox`
- [ ] Messages from peer Y not visible in X-triggered run (hard guarantee)
- [ ] Accumulated `text` messages from other peers not swept into task-triggered run
- [ ] Manual `agents_inbox` (CLI/heartbeat) still returns full inbox (unaffected)
- [ ] `GET /api/ipc/inbox?from=X&kinds=task,query,result` returns correct scoped subset

### Per-peer counter
- [ ] First `task` from new peer → always processes (count starts at 0)
- [ ] 4th consecutive push-triggered run for same peer → suppressed with WARN log
- [ ] After 5min cooldown → counter resets, new pushes process normally
- [ ] Different peers have independent counters
- [ ] Coalescing preserved (multiple rapid pushes from same peer → single run)

### One-way mode
- [ ] `push_one_way=false` (default) → all pushes processed normally (existing behavior)
- [ ] `push_one_way=true`, L1→L3 push → L3 auto-processes (superior → subordinate)
- [ ] `push_one_way=true`, L3→L1 push → L1 does NOT auto-process (subordinate → superior)
- [ ] `push_one_way=true`, L3→L3 push → auto-processes if kind matches (lateral = same level, no suppression)
- [ ] `push_one_way=true`, unknown trust level → fail-closed: auto-processing skipped
- [ ] One-way suppressed messages still in inbox, readable via poll

### Legitimate workflow chains
- [ ] L1 sends `task` → L3 processes → L3 sends `result` → L1 auto-processes result → workflow completes
- [ ] Multi-step delegation: L1→L3 task, L3→L1 result, L1→L3 follow-up task — each step processes correctly
- [ ] Same-level `query`/`result` exchange works when allowed by ACL
- [ ] Suppressed messages don't starve — they remain in inbox and are picked up by next poll/heartbeat/manual check
- [ ] Heartbeat-triggered inbox check still processes full inbox (heartbeat doesn't use push path)

### Prompt
- [ ] Prompt includes anti-ack instruction
- [ ] Prompt names the triggering peer
- [ ] Agent does not generate pointless acknowledgments on auto-processed pushes

### Rollback
- [ ] `push_max_auto_processes=1000` effectively disables counter
- [ ] `push_auto_process_kinds=["task","query","text","result"]` restores old kind behavior
- [ ] `push_one_way=false` disables one-way mode

---

## Risks

| Risk | Mitigation |
|------|-----------|
| Legitimate `text` messages not auto-processed | Configurable via `push_auto_process_kinds`; messages still in inbox for poll/manual check |
| Scoped inbox misses urgent messages from other peers | Scoping only affects push-triggered runs. Heartbeat, CLI, and manual inbox checks still process full inbox. |
| Per-peer counter too aggressive for long task chains | Default 3 is generous; counter tracks runs, not replies. Legitimate multi-step chains use different kinds (task→result→task) and get 3 runs per peer before suppression. |
| One-way mode blocks legitimate subordinate-initiated alerts | One-way is opt-in (default: false); subordinates can still send messages, they just don't trigger auto-processing on the superior |
| Cooldown too long/short for specific workflows | Configurable `push_peer_cooldown_secs`; 300s default balances loop prevention vs responsiveness |
| Signal channel type change breaks tests | All test `AppState` initializations use `ipc_push_signal: None` — type-agnostic via inference |
| `from_trust_level` lookup fails (message not yet in local DB) | Fallback to `None`; when `push_one_way=true`, fail-closed (skip auto-processing). When `push_one_way=false`, irrelevant. |
| Pre-fetch adds HTTP call latency before `agent::run()` | Same-machine loopback (~1ms). Acceptable — the alternative (LLM calling agents_inbox) is ~100ms+ anyway. |
| LLM has `agents_inbox` tool available and could call it anyway | Tool remains available for legitimate use (manual queries, multi-peer workflows). But push-triggered prompt gives pre-fetched messages as context, so LLM has no reason to call inbox. If it does, it gets the full inbox — but the counter and kind filter still limit the loop. Defense in depth. |
| Sequential per-peer runs slow for many concurrent peers | Uncommon scenario. Each run takes 2-30s depending on model. For 3+ concurrent peers, consider parallel runs in a future enhancement. |

---

## Decisions

1. **Pre-fetch + inject is the structural guarantee** — kind filtering on wake-up signal alone is insufficient because the LLM could fetch the entire inbox. The inbox processor pre-fetches scoped messages and injects them as prompt context. The LLM never calls `agents_inbox` in push-triggered runs, so it cannot bypass the scope. This is an enforced guarantee, not a prompt-level request.
2. **`result` included in default auto-process kinds** — legitimate orchestration chains (task→result→continue) require auto-processing of results. The per-peer counter and one-way mode prevent result-triggered loops without breaking orchestration.
3. **Counter is `auto_process_count`, not `auto_reply_count`** — we count push-triggered runs per peer, not outgoing replies. This is an honest metric: we can't reliably track per-peer outgoing messages without parsing tool call results. The counter is a circuit breaker, not a precise reply tracker.
4. **`from_trust_level` resolved from local IPC DB, not HTTP body; fail-closed** — the push payload is broker-originated but the trust level must be verified. The broker's stored `from_trust_level` (set from authenticated token metadata at insert time) is authoritative. Agent-side receiver looks it up by `message_id`. When `push_one_way=true` and trust cannot be resolved, auto-processing is skipped (fail-closed).
5. **Kind-based filtering is the first defense line** — structural, not heuristic. Inspired by MetaGPT's `cause_by` routing. `text` is excluded by default as the primary loop vector.
6. **Per-peer counter inspired by AutoGen's `max_consecutive_auto_reply`** — the most battle-tested pattern across multi-agent frameworks. Renamed to reflect actual semantics.
7. **Prompt improvement is supplementary, not relied upon** — LLMs may ignore instructions; the structural layers (scoped inbox + kind filter + counter) provide hard guarantees.
8. **One run per peer, sequential** — coalesced signals from multiple peers produce separate `agent::run()` calls, one per peer, processed sequentially. No merged multi-peer runs. This ensures precise counter tracking and prevents cross-peer message leakage.
9. **Agent-side + minimal broker change** — wake-up filtering and counter are agent-side only. Broker-side change is minimal: optional `from`/`kinds` filter params on `GET /api/ipc/inbox`.
10. **One-way mode is opt-in** — default `false` preserves existing behavior. When enabled, provides MetaGPT-style instructor/assistant asymmetry using existing trust levels.
11. **No semantic similarity detection in v1** — embeddings add latency and complexity. The structural approach is sufficient. Can be added in a future phase if needed.
