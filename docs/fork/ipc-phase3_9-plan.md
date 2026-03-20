# IPC Phase 3.9: Operator Control Plane

Phase 3.8: multi-agent dashboard | **Phase 3.9: operator control plane** | Phase 3.10: push loop prevention

---

## Problem

Phase 3.8 gave the operator a single-tab dashboard with agent selector and proxy chat. But several operational gaps remain:

- **No push delivery** — IPC messages sit in inbox until agent polls. Delegated tasks aren't processed until the next poll cycle (up to 60s delay).
- **No unified activity feed** — operator has no visibility into what happened across agents (delegated tasks, completions, errors). Must check each agent's chat individually.
- **No conversation traceability** — when agents talk to each other, spawn ephemeral children, or interact through external channels, it is hard to find the *real* underlying dialog. Operators have to manually hunt across IPC sessions, spawn runs, chat sessions, logs, and channel-specific stores.
- **Cron is single-agent** — each agent's cron jobs are managed per-daemon. Operator can't see or manage cron across the fleet from one place.
- **Force graph unusable on refresh** — topology graph resets node positions on every data update, making it impossible to read with more than a few nodes.

---

## Scope

### In scope

1. Broker pushes IPC messages to agent gateways via webhook (real-time delivery)
2. Force graph position preservation (stable layout across refreshes)
3. Gzip compression for WS and HTTP proxy traffic
4. Unified broker-only activity feed page (`/ipc/activity`) showing cross-agent events
5. Conversation traceability from activity events into the real IPC/chat/spawn/channel dialogs
6. Cron proxy endpoints on broker (list/create/delete across agents)
7. Broker-only multi-agent cron page (`/ipc/cron`)
8. Build, deploy, and verification

### Non-goals

- Cross-agent cron dependencies or DAG scheduling
- Activity feed persistence on broker (feed is assembled from live agent state)
- A universal cross-substrate conversation graph database in this phase
- Replacing IPC polling entirely (push is best-effort, polling remains as fallback)
- Agent-side cron engine changes (existing cron works, we only proxy)
- Runtime architecture changes (that's Phase 4.0)
- Replacing the existing local-agent `/cron` workbench page with a second duplicate implementation

---

## Architecture

### UI scope model

Phase 3.9 builds on Phase 3.8's one-frontend/two-mode shell:

- **Local agent mode** keeps the full per-agent workbench: Dashboard, Chat, Tools, Cron, Integrations, Memory, Config, Cost, Logs, Doctor
- **Broker mode** adds broker-only global pages such as `/ipc/activity` and `/ipc/cron`

This phase should not create a second copy of the single-agent workbench. It adds broker-global control-plane pages and keeps agent-scoped pages reusable. The design rule applies to the whole workbench, not only cron: `Memory`, `Logs`, `Dashboard`, and the other agent pages remain agent-scoped surfaces even when the browser is connected to broker mode.

This does **not** mean the broker UI cannot read them. It means the broker exposes them as **selected-agent views**, not as a flattened fleet-wide surface. An administrator should be able to open `Agent X -> Logs` or `Agent X -> Memory` from the broker and inspect that agent through proxy.

### Push delivery flow

```
Agent A                      Broker                       Agent B
  │                            │                             │
  │── POST /api/ipc/send ─────>│                             │
  │   {to: "agent-b", ...}     │                             │
  │                            │── POST /api/ipc/push ──────>│
  │                            │   Authorization: Bearer      │
  │                            │   <proxy_token>              │
  │                            │   {message payload}          │
  │                            │                             │
  │                            │<── 200 OK ──────────────────│
  │<── 200 OK ─────────────────│                             │
```

Broker delivers messages immediately after storing them. Uses the existing `proxy_token` from AgentRegistry (same token used for WS chat proxy in Phase 3.8). Delivery is best-effort — if agent is offline, message stays in inbox for poll-based retrieval.

### Activity feed data flow

```
Browser                     Broker                      Agents
  │                           │                            │
  │── GET /api/activity ─────>│                            │
  │                           │── GET /api/activity ──────>│ (fan-out to all online agents)
  │                           │<── [{events}] ────────────│
  │                           │   (merge + sort by time)   │
  │<── [{merged events}] ────│                            │
```

Activity feed is assembled on-demand by the broker, fan-out to all online agents. Each agent exposes a local `/api/activity` endpoint returning recent events (IPC sends/receives, cron runs, errors). Broker merges, deduplicates, and sorts.

### Conversation trace model

The activity feed is not just a flat event list. Each row must answer:

1. **what happened**
2. **which agent(s) were involved**
3. **which real dialog/run/session this belongs to**
4. **where the operator can open that dialog**

To do that, activity events need a structured trace reference instead of only human-readable text.

Minimal trace envelope:

```json
{
  "event_type": "ipc_send",
  "agent_id": "research",
  "timestamp": 1710000000,
  "trace_ref": {
    "surface": "ipc",
    "session_id": "sess_abc123",
    "message_id": 42,
    "from_agent": "research",
    "to_agent": "code",
    "parent_agent_id": null,
    "child_agent_id": null,
    "spawn_run_id": null,
    "chat_session_key": null,
    "channel_name": null,
    "channel_session_key": null,
    "run_id": null
  }
}
```

Supported `trace_ref.surface` values in v1:

- `ipc` — brokered agent-to-agent messages (`session_id`, `message_id`, `from_agent`, `to_agent`)
- `spawn` — ephemeral child execution (`spawn_run_id`, `parent_agent_id`, `child_agent_id`)
- `web_chat` — browser chat sessions (`chat_session_key`, optional `run_id`)
- `channel` — external human/channel conversations (`channel_name`, `channel_session_key`)
- `cron` — scheduled tasks (`job_id` may live in event payload if no stronger trace key exists)

Not every field is present for every surface. The contract is: if an operator sees an event, the event must carry enough metadata to open or filter to the real underlying dialog/run.

This model should reuse identifiers that already exist in the current codebase rather than invent a second parallel key system:

- IPC already has `session_id` and `message_id`
- ephemeral execution already has `spawn_runs.id`
- web chat already has `chat_sessions.key` and per-message `run_id`
- channel conversations already have stable `SessionStore` keys such as `channel_sender` or `channel_thread_sender`

### Trace drill-down behavior

Broker UI must expose a direct “open trace” action from the activity feed:

- `ipc` → open `/ipc/sessions?session_id=...`
- `spawn` → open `/ipc/spawns?session_id=...` or the specific spawn run detail
- `web_chat` → open `/agents/:agent_id/chat?session=...`
- `channel` → open the selected agent's logs or dedicated conversation view filtered by `channel_session_key`
- `cron` → open the selected agent's cron view, filtered to the relevant job when possible

This is especially important for:

- agent A ↔ agent B conversations
- parent agent ↔ ephemeral child execution chains
- hybrid flows where a channel message triggers IPC or spawn activity

The operator should not need to manually grep logs or click through unrelated pages to reconstruct the real dialog.

### Cron proxy topology

```
Browser                     Broker                      Agent
  │                           │                            │
  │── GET /api/agents/{id}   │                            │
  │   /cron ────────────────>│                            │
  │                           │── GET /api/cron ──────────>│
  │                           │   Authorization: Bearer     │
  │                           │   <proxy_token>             │
  │                           │<── [{cron jobs}] ──────────│
  │<── [{cron jobs}] ────────│                            │
  │                           │                            │
  │── POST /api/agents/{id}  │                            │
  │   /cron ────────────────>│                            │
  │                           │── POST /api/cron ─────────>│
  │                           │<── 201 Created ────────────│
  │<── 201 Created ──────────│                            │
```

Broker proxies cron CRUD to individual agents using the same proxy pattern as Phase 3.8 status/health proxying.

---

## Implementation Steps

### Step 1: Push delivery (DONE — PR #130)

Broker pushes IPC messages to agent gateways immediately after storing them.

- `POST /api/ipc/push` endpoint on agent gateway
- Broker calls push after `ipc_send` stores message
- Uses `proxy_token` from AgentRegistry for auth
- Best-effort delivery: logs warning on failure, message remains in inbox
- Agent processes pushed message through existing IPC handler
- WS notification to connected dashboards

### Step 2: Force graph position preservation

Fix the topology graph (`/ipc/fleet`) to preserve node positions across data refreshes.

- Store node positions in component state (or `useRef`)
- On data update, merge new node data with existing positions
- Only assign random positions to genuinely new nodes
- Pin nodes that user has manually dragged

### Step 3: Gzip compression layer

Add gzip compression for WS proxy and HTTP API responses.

- `tower-http` `CompressionLayer` on gateway HTTP routes
- WS frames: compress large payloads (>1KB) with `flate2`
- Reduces bandwidth for activity feed and cron list responses
- Transparent to browser (standard `Accept-Encoding: gzip`)

### Step 4: Activity feed page (frontend)

New page at `/ipc/activity` showing unified cross-agent activity.

- New React page component in `web/src/pages/ipc/Activity.tsx`
- Fetches `GET /api/activity` from broker
- Displays timeline of events: IPC messages, cron runs, errors
- Filterable by agent, event type, time range
- Each event row shows trace metadata (session/run/channel context when present)
- Each event row exposes “open trace” / “open related dialog”
- Auto-refresh on interval (30s) or manual refresh button
- Add navigation entry in the broker-only IPC sidebar section

### Step 5: Activity trace metadata (backend)

Enrich agent-local `/api/activity` so broker receives structured trace refs, not just flat event text.

- IPC events include `session_id`, `message_id`, `from_agent`, `to_agent`
- Spawn events include `spawn_run_id`, `parent_agent_id`, `child_agent_id`
- Web chat events include `chat_session_key` and `run_id` where available
- Channel events include `channel_name` and `channel_session_key` where available
- Cron events include stable job identity when available

Broker preserves these refs when merging activity streams.

### Step 6: Cron proxy endpoints (backend)

Broker proxies cron CRUD operations to individual agents.

- `GET /api/agents/{agent_id}/cron` — list agent's cron jobs
- `POST /api/agents/{agent_id}/cron` — create cron job on agent
- `DELETE /api/agents/{agent_id}/cron/{job_id}` — delete cron job on agent
- Uses existing proxy pattern (proxy_token auth, agent lookup from registry)
- Returns 503 if agent is offline

### Step 7: Multi-agent cron page (frontend)

Add a broker-global cron view while keeping the existing local `/cron` page as the single-agent workbench.

- New page at `/ipc/cron` for broker mode only
- Agent selector/filter to view per-agent or all-agent cron jobs
- Create/delete operations routed through broker proxy
- Shows agent name alongside each cron job
- Existing `/cron` remains the local or selected-agent workbench page
- Add navigation entry in broker-only IPC sidebar section

This is the pattern for the broader UI as well:

- broker-global pages such as `Activity` and fleet-wide cron are added under the IPC/control-plane section
- existing agent workbench pages such as `Dashboard`, `Memory`, and `Logs` remain selected-agent surfaces, not duplicated fleet pages
- broker mode must still let the operator open those selected-agent surfaces for any registered agent

### Step 8: Build + deploy + verify

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cd web && npm run build`
- E2E: broker + 2 agents, push delivery, activity feed, cron proxy

---

## Verification Checklist

### Push delivery
- [ ] IPC message sent → broker pushes to recipient agent immediately
- [ ] Push failure → message remains in inbox (fallback to poll)
- [ ] Agent offline → push skipped with warning log
- [ ] Duplicate push protection (idempotent by message_id)
- [ ] WS notification sent to connected dashboards on push

### Force graph
- [ ] Node positions preserved when data refreshes
- [ ] New nodes appear without disrupting existing layout
- [ ] Manually dragged nodes stay pinned
- [ ] Graph remains usable with 5+ agents

### Activity feed
- [ ] `/ipc/activity` page loads and shows cross-agent events
- [ ] Events sorted by time, most recent first
- [ ] Filter by agent works
- [ ] Filter by event type works
- [ ] IPC events link to real IPC session inspector
- [ ] Spawn events link to the correct parent/child run
- [ ] Channel-triggered events carry enough metadata to find the source conversation
- [ ] Operator can follow a multi-agent chain without manual log hunting
- [ ] Auto-refresh updates without losing scroll position
- [ ] Offline agents handled gracefully (partial results shown)

### Cron proxy
- [ ] `GET /api/agents/{id}/cron` returns agent's cron jobs
- [ ] `POST /api/agents/{id}/cron` creates job on agent
- [ ] `DELETE /api/agents/{id}/cron/{job_id}` deletes job on agent
- [ ] Offline agent → 503 response
- [ ] Cron page shows jobs across all agents
- [ ] Create/delete from cron page works through proxy

### Compression
- [ ] HTTP responses compressed with gzip when client accepts
- [ ] Large WS frames compressed
- [ ] No compression for small payloads (<1KB)

---

## Risks

| Risk | Mitigation |
|------|-----------|
| Push delivery adds load to broker on high IPC volume | Best-effort, async delivery. Broker doesn't wait for agent ACK before responding to sender. |
| Activity feed fan-out slow with many agents | Parallel fan-out with timeout. Partial results returned if some agents slow/offline. |
| Traceability metadata inconsistent across event sources | Start with a minimal shared `trace_ref` contract; allow partial refs but require every event to identify its real underlying surface. |
| Cron proxy adds latency vs direct agent access | Same-machine connections are <1ms. Acceptable for dashboard use. |
| Gzip CPU overhead | Only compress responses >1KB. `tower-http` handles this efficiently. |
| Force graph state management complexity | Keep it simple: position map in `useRef`, merge on update. No external state library. |

---

## Decisions

1. **Push is best-effort, not guaranteed** — polling remains as fallback. No complex retry/queue system.
2. **Activity feed is assembled on-demand** — no persistent event store on broker. Agents are the source of truth.
3. **Activity rows carry structured trace refs** — the feed must link to real dialogs, not just show text summaries.
4. **Cron proxy reuses Phase 3.8 proxy pattern** — same auth, same error handling, same proxy_token.
5. **Gzip via tower-http for HTTP, flate2 for WS** — standard, well-tested compression libraries.
6. **Force graph positions in component state** — simple, no persistence needed across page navigations.
7. **Activity endpoint on each agent** — agents expose their own recent events, broker merges. No centralized event bus.
