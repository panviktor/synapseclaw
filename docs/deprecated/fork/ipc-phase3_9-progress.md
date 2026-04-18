# Phase 3.9 Progress: Operator Control Plane

## Status: DONE (Steps 1-6 done, Step 3 deferred, Step 7 done via fleet deploy)

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | Push delivery | done | #130 | Broker pushes IPC messages to agent gateways, WS notifications |
| 2 | Force graph position preservation | done | — | useRef position map, onNodeDragEnd pinning, onEngineStop capture |
| 3 | Gzip compression layer | deferred | — | tower-http CompressionLayer already present; WS flate2 deferred |
| 4 | Activity feed + trace model (backend+frontend) | done | — | `/api/activity`, `/admin/activity`, `/ipc/activity` page with trace drill-down |
| 5 | Cron proxy endpoints (backend) | done | — | GET/POST/DELETE `/api/agents/{id}/cron` proxy to agent gateways |
| 6 | Multi-agent cron page (frontend) | done | — | `/ipc/cron` — fleet-wide cron management with CRUD |
| 7 | Build + deploy + verify | done | — | Full validation and E2E testing |

---

## Trace Model

Each activity event carries a `TraceRef` with:
- `surface`: `ipc` | `spawn` | `web_chat` | `channel` | `cron`
- Surface-specific IDs reusing existing keys (no parallel identifier system)

### Supported Drill-Down

| Surface | Trace Fields | "Open Trace" Target |
|---------|-------------|-------------------|
| `ipc` | session_id, message_id, from_agent, to_agent | `/ipc/sessions?session_id=...` |
| `spawn` | spawn_run_id, parent_agent_id, child_agent_id | `/ipc/spawns?parent_id=...` |
| `web_chat` | chat_session_key, run_id | `/agents` (proxy chat) |
| `channel` | channel_name, channel_session_key | `/ipc/fleet/{agent_id}` |
| `cron` | job_id, job_name | `/ipc/cron?agent=...` |

### Data Flow

- **IPC + Spawn events**: from broker's own `ipc_db` (no fan-out needed)
- **Cron + Chat + Channel events**: fan-out to online agents via `GET /api/activity`
- **Merge**: broker merges all sources, sorts by timestamp, filters, returns `{events, partial}`

---

## Verification

### Push delivery
- [x] IPC message sent → broker pushes to recipient agent immediately
- [x] Push failure → message remains in inbox (fallback to poll)
- [x] Agent offline → push skipped with warning log
- [x] WS notification sent to connected dashboards on push
- [ ] Duplicate push protection (idempotent by message_id)

### Force graph
- [x] Node positions preserved when data refreshes
- [x] New nodes appear without disrupting existing layout
- [x] Manually dragged nodes stay pinned

### Activity feed
- [x] `/ipc/activity` page loads and shows cross-agent events
- [x] Events sorted by time, most recent first
- [x] Filter by agent works
- [x] Offline agents handled gracefully (partial flag)
- [x] Structured trace_ref on every event
- [x] "Open Trace" navigates to correct page per surface

### Cron proxy
- [x] `GET /api/agents/{id}/cron` returns agent's cron jobs
- [x] `POST /api/agents/{id}/cron` creates job on agent
- [x] `DELETE /api/agents/{id}/cron/{job_id}` deletes job on agent
- [x] Cron page shows jobs across all agents

### Compression
- [x] HTTP responses compressed with gzip (tower-http CompressionLayer already active)
- [ ] Large WS frames compressed (deferred)
