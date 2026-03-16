# IPC Phase 3.8: Broker-Centered Multi-Agent Dashboard

Phase 3.7b: session intelligence | **Phase 3.8: multi-agent dashboard** | Phase 4.0: modular core refactor

---

## Problem

Today: one daemon = one agent = one gateway = one dashboard. This works for one agent but becomes operationally painful for a family:

- **N ports** — each agent daemon runs its own gateway (42617, 42618, ...)
- **N SSH tunnels** — operator needs a tunnel per agent to access each dashboard
- **N browser tabs** — no unified view of the family
- **No agent selector** — "which agent am I talking to?" requires knowing ports
- **Fragmented control plane** — IPC admin, fleet view, quarantine review all per-broker, but chat is per-agent

Phase 3.5–3.6 gave us fleet visibility and provisioning on the broker. Phase 3.7–3.7b gave us durable chat sessions per agent. But the operator still can't **talk to different agents from one place**.

---

## Scope

**One browser → one broker → many agents.**

Broker becomes the single operator entrypoint. Browser connects only to broker's gateway. Broker proxies chat/session operations to the selected agent. One SSH tunnel is enough.

### In scope

1. Broker dashboard with agent selector dropdown
2. Broker proxies WS chat + session RPCs to selected agent's gateway
3. Agent health/status aggregation on broker
4. Agent registration via existing IPC pairing (no new auth model)
5. Graceful handling of offline agents
6. Minimal new CLI surface (reuses `daemon` + config)

### Non-goals

- Cross-agent merged chat sessions (sessions stay per-agent)
- Magical multi-agent reasoning router
- Browser-direct fanout to every agent port
- Replacing IPC with a new protocol
- Giant single process for the whole family
- Federated execution (that's Phase 4+)
- Runtime architecture rewrite (that's Phase 4.0)

---

## Runtime Topology

```
┌─────────────────────────────────────────────────┐
│  Operator Browser                                │
│  (one tab, one connection)                       │
└────────────────┬────────────────────────────────┘
                 │ WS + HTTP (one SSH tunnel)
                 ▼
┌─────────────────────────────────────────────────┐
│  Broker Daemon (port 42617)                      │
│  ┌─────────────┐ ┌───────────┐ ┌──────────────┐ │
│  │ Gateway      │ │ IPC Broker│ │ Chat Proxy   │ │
│  │ (dashboard,  │ │ (agents,  │ │ (WS relay to │ │
│  │  API, admin) │ │  messages)│ │  agent WS)   │ │
│  └─────────────┘ └───────────┘ └──────────────┘ │
│  ┌─────────────────────────────────────────────┐ │
│  │ Agent Registry (live health + gateway URLs) │ │
│  └─────────────────────────────────────────────┘ │
└────────┬──────────────┬──────────────┬──────────┘
         │              │              │
    HTTP/WS         HTTP/WS        HTTP/WS
         │              │              │
         ▼              ▼              ▼
┌──────────────┐ ┌──────────────┐ ┌──────────────┐
│ Agent: Opus  │ │ Agent: Daily │ │ Agent: Code  │
│ port 42618   │ │ port 42619   │ │ port 42620   │
│ (own daemon, │ │ (own daemon, │ │ (own daemon, │
│  own config, │ │  own config, │ │  own config, │
│  own chat DB)│ │  own chat DB)│ │  own chat DB)│
└──────────────┘ └──────────────┘ └──────────────┘
```

### Process count

- **1 broker daemon** — runs gateway + IPC broker + chat proxy
- **N agent daemons** — each runs gateway + channels + agent loop
- Total: N+1 OS processes
- Operator connects to broker only (1 tunnel, 1 tab)

---

## Broker Responsibilities

1. **Dashboard host** — serves web UI with agent selector
2. **Agent registry** — tracks which agents are alive, their gateway URLs, models, status
3. **Chat proxy** — relays WS chat/session RPCs to selected agent's gateway
4. **IPC broker** — existing Phase 1-3 IPC (messages, shared state, quarantine)
5. **Fleet admin** — existing Phase 3.5-3.6 screens (fleet, spawns, quarantine, audit)
6. **Health aggregation** — polls agent `/health` endpoints, shows combined status

### What broker does NOT do

- Does not run agent loops
- Does not hold agent chat sessions (those stay on agent daemons)
- Does not make LLM calls for agents
- Does not merge or transform chat messages
- Does not replace IPC — chat proxy is a separate parallel path

---

## Agent Responsibilities

1. **Own daemon** — runs its own gateway, channels, agent loop, scheduler
2. **Own chat DB** — sessions and messages stay local (Phase 3.7)
3. **Register with broker** — on startup, POST registration to broker
4. **Heartbeat** — periodic health ping so broker knows agent is alive
5. **Accept proxied WS** — broker connects to agent's `/ws/chat` and relays frames

### Agent config addition

```toml
[agents_ipc]
enabled = true
broker_url = "http://127.0.0.1:42617"
broker_token = "enc2:..."

# NEW: expose gateway URL for broker proxy (auto-detected if not set)
gateway_url = "http://127.0.0.1:42618"
```

---

## Agent Registry Model

Broker maintains a live registry (extends existing `NodeRegistry` or new `AgentRegistry`):

| Field | Type | Source |
|-------|------|--------|
| `agent_id` | String | From IPC TokenMetadata |
| `gateway_url` | String | From agent registration |
| `trust_level` | u8 | From IPC TokenMetadata |
| `role` | String | From IPC TokenMetadata |
| `model` | String | From agent `/api/status` |
| `status` | enum(online/offline/error) | From heartbeat |
| `last_seen` | timestamp | Updated on heartbeat |
| `uptime_seconds` | u64 | From agent `/api/status` |
| `channels` | Vec\<String\> | From agent `/api/status` |

### Registration flow

1. Agent daemon starts → pairing with broker (existing flow)
2. Agent POST `{gateway_url, model, channels}` to broker's new `/api/ipc/register-gateway` endpoint
3. Broker stores in registry, starts polling agent `/health` every 30s
4. If agent goes offline (3 missed health checks) → status = `offline`
5. Agent restart → re-registers automatically

---

## Browser ↔ Broker ↔ Agent Communication

### Agent selector

Browser dashboard sidebar (above session list) gets a dropdown:
- Lists all registered agents from broker's `/api/agents` endpoint
- Shows: agent_id, role, model, status (green/red dot)
- Default: last selected agent (localStorage)
- Switching agent: disconnects proxy WS, connects new one, loads that agent's sessions

### Chat proxy flow

```
Browser                    Broker                      Agent
  │                          │                           │
  │── WS /ws/chat ──────────>│                           │
  │   ?agent=opus            │                           │
  │                          │── WS /ws/chat ───────────>│
  │                          │   (broker's own token)     │
  │                          │                           │
  │── RPC sessions.list ────>│── RPC sessions.list ─────>│
  │<── rpc_response ─────────│<── rpc_response ──────────│
  │                          │                           │
  │── RPC chat.send ────────>│── RPC chat.send ─────────>│
  │<── tool_call push ───────│<── tool_call push ────────│
  │<── tool_result push ─────│<── tool_result push ──────│
  │<── rpc_response ─────────│<── rpc_response ──────────│
```

Broker is a **transparent WS relay** for chat:
- Browser opens WS to broker with `?agent=<agent_id>` param
- Broker looks up agent's `gateway_url` in registry
- Broker opens WS to agent's `/ws/chat` using broker's token
- All frames are forwarded bidirectionally (no parsing, no transformation)
- If agent disconnects, broker sends error frame to browser and closes

### Non-chat API proxy

For `/api/status`, `/api/nodes` on a specific agent:
- Browser calls broker `GET /api/agents/{agent_id}/status`
- Broker proxies HTTP GET to agent's `{gateway_url}/api/status`
- Returns response to browser

---

## Auth & Trust Model

- Browser authenticates with **broker** (existing pairing)
- Broker authenticates with **agents** (existing IPC pairing / broker_token)
- Browser never talks to agents directly
- Agent's chat session ownership uses **broker's token prefix** (not browser's)
- This means broker sees all sessions on all agents (by design — operator is admin)

---

## Failure & Restart Model

| Scenario | Behavior |
|----------|----------|
| Agent goes offline | Broker marks `offline` in registry. Selecting it shows "Agent offline" in chat. Sessions persist in agent's DB. |
| Agent restarts | Agent re-registers. Broker reconnects proxy if browser had it selected. Sessions resume from DB. |
| Broker restarts | Browser reconnects (existing WS reconnect). Agent registry rebuilds from next heartbeat cycle. No data loss — sessions are on agents. |
| Browser refresh | Reconnects to broker WS. Agent selector restores from localStorage. Sessions loaded from agent via proxy. |

---

## Relationship to Phase 3.7 / 3.7b

- **Chat sessions remain agent-local** — Phase 3.7 `ChatDb` stays on each agent
- **WS RPC protocol unchanged** — broker relays same RPCs that browser currently sends directly
- **Session summaries** — computed by agent, returned through proxy transparently
- **Live tool events** — forwarded through proxy transparently
- **Run lifecycle events** — forwarded through proxy transparently
- No changes needed to Phase 3.7/3.7b code on agents

## Relationship to Phase 4.0

- Phase 3.8 solves **runtime topology and operator UX**
- Phase 4.0 solves **internal application architecture** (capability model, conversation store, memory tiers)
- Phase 3.8 does NOT require Phase 4.0 changes
- Phase 4.0's `ConversationStore` port will eventually abstract over the per-agent chat DB, but that's orthogonal
- Proxy model from 3.8 survives the 4.0 refactor — it operates at transport level, not application level

---

## Implementation Steps

### Step 1: Agent gateway registration endpoint
- `POST /api/ipc/register-gateway` on broker — accepts `{gateway_url}` from authenticated agent
- Store in `IpcDb` or separate in-memory `AgentRegistry`
- Return OK

### Step 2: Broker health polling
- Broker polls each registered agent's `/health` every 30s
- Also fetches `/api/status` for model/channels/uptime
- Updates registry with status + metadata

### Step 3: Broker `/api/agents` endpoint
- `GET /api/agents` — returns list of registered agents with status, model, role
- Used by browser for agent selector dropdown

### Step 4: WS chat proxy on broker
- New WS endpoint: `/ws/chat/proxy?agent=<agent_id>`
- Broker looks up `gateway_url` from registry
- Opens upstream WS to agent's `/ws/chat` with broker's token
- Bidirectional frame relay (transparent, no parsing)
- Error handling: agent offline → error frame → close

### Step 5: HTTP API proxy
- `GET /api/agents/{agent_id}/status` → proxies to agent
- `GET /api/agents/{agent_id}/health` → proxies to agent
- Generic pattern for future per-agent API calls

### Step 6: Agent auto-registration on startup
- Agent daemon: after IPC pairing succeeds, POST `gateway_url` to broker
- Config: `gateway_url` in `[agents_ipc]` section (auto-detect from gateway bind if not set)

### Step 7: Browser agent selector UI
- Dropdown in chat sidebar (above session list)
- Fetches `/api/agents` from broker
- On switch: close current proxy WS, open new one with `?agent=<id>`
- Persist selected agent in localStorage

### Step 8: Agent status display
- Sidebar info panel (from 3.7b) shows selected agent's model, uptime, status
- Fetched via proxy `/api/agents/{id}/status`

---

## Verification Checklist

- [ ] Broker starts with `agents_ipc.enabled = true`
- [ ] Agent registers `gateway_url` with broker on startup
- [ ] Broker `/api/agents` returns list with status
- [ ] Browser agent selector shows all agents
- [ ] Selecting agent opens proxy WS, loads that agent's sessions
- [ ] Chat works through proxy (send, receive, tool events, abort)
- [ ] Agent going offline shows "offline" in selector
- [ ] Agent restart → browser can resume chat through proxy
- [ ] Broker restart → agents re-register, browser reconnects
- [ ] One SSH tunnel to broker is sufficient for full operation

---

## Risks

| Risk | Mitigation |
|------|-----------|
| WS proxy adds latency | Transparent relay (no parsing), same-machine connections are <1ms |
| Broker becomes SPOF | Agents continue running independently. Only dashboard access lost. |
| Auth complexity (browser→broker→agent) | Reuse existing pairing. Broker uses its own token with agents. |
| Session ownership confusion | Sessions use broker's token prefix on all agents. Operator is admin. |
| Registry stale on broker restart | Agents re-register on next heartbeat. Short gap (30s max). |
| N+1 process management | Existing `daemon` + systemd/launchd service model. No new commands needed. |

---

## CLI Surface

No new subcommands needed:
- Broker: `zeroclaw daemon` with `[agents_ipc] enabled = true`
- Agents: `zeroclaw daemon` with `[agents_ipc] broker_url = ...`
- Both use existing `service install` for systemd/launchd

Configuration is the differentiator, not commands.

---

## Decisions

1. **Broker is transparent relay, not application proxy** — no message transformation, no session merging, no LLM calls on behalf of agents.
2. **Sessions remain per-agent** — no cross-agent session model in v1.
3. **One process per agent** — no multi-agent-in-one-process model.
4. **Registration via IPC pairing** — no new auth mechanism.
5. **WS proxy, not HTTP long-poll** — preserves streaming, tool events, lifecycle events.
6. **Agent selector in sidebar** — not a separate page, integrated into chat flow.
