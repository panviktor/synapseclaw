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
4. Broker-to-agent auth (dedicated proxy token, issued at pairing)
5. Multi-instance service model (templated systemd/launchd units)
6. Graceful handling of offline agents
7. Agent auto-registration with periodic re-registration

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
│  │ AgentRegistry (live health + gateway URLs)  │ │
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

- **1 broker daemon** — runs gateway + IPC broker + chat proxy + agent registry
- **N agent daemons** — each runs gateway + channels + agent loop
- Total: N+1 OS processes
- Operator connects to broker only (1 tunnel, 1 tab)

---

## Multi-Instance Service Model

**Decision:** Current service layer uses fixed names (`zeroclaw.service`, `com.zeroclaw.daemon`). This is a blocker for N+1 processes. We need templated multi-instance units.

### Config directory layout

```
~/.zeroclaw/                        # broker (default)
  config.toml
  workspace/
~/.zeroclaw/agents/opus/            # agent: opus
  config.toml
  workspace/
~/.zeroclaw/agents/daily/           # agent: daily
  config.toml
  workspace/
~/.zeroclaw/agents/code/            # agent: code
  config.toml
  workspace/
```

### CLI

New `--instance <name>` flag on `daemon` and `service` commands:

```bash
# Broker (default instance, no flag needed)
zeroclaw daemon
zeroclaw service install

# Agent instances
zeroclaw daemon --instance opus
zeroclaw service install --instance opus
zeroclaw service install --instance daily
zeroclaw service install --instance code
```

`--instance <name>` sets config dir to `~/.zeroclaw/agents/<name>/`.

### systemd (Linux)

Templated user unit: `~/.config/systemd/user/zeroclaw@.service`

```ini
[Unit]
Description=ZeroClaw Agent (%i)
After=default.target

[Service]
Type=simple
ExecStart=%h/.local/bin/zeroclaw daemon --instance %i
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

Note: uses `WantedBy=default.target` (user-level), not `multi-user.target` (system-level). Consistent with current `service install` which uses `systemctl --user`.

Usage:
```bash
# Broker (default instance)
systemctl --user enable --now zeroclaw.service

# Agents
systemctl --user enable --now zeroclaw@opus.service
systemctl --user enable --now zeroclaw@daily.service
systemctl --user enable --now zeroclaw@code.service
```

### launchd (macOS)

Per-instance plist: `com.zeroclaw.<instance>.plist`

```bash
zeroclaw service install                    # → com.zeroclaw.daemon.plist
zeroclaw service install --instance opus    # → com.zeroclaw.agent-opus.plist
zeroclaw service install --instance daily   # → com.zeroclaw.agent-daily.plist
```

### Windows

Per-instance scheduled task: `ZeroClaw Agent (<instance>)`

```bash
zeroclaw service install --instance opus    # → "ZeroClaw Agent (opus)" task
```

### OpenRC (Linux, non-systemd)

**Out of scope for Phase 3.8.** Current code supports OpenRC (`service/mod.rs:17`) but multi-instance templating for OpenRC is non-trivial (no native `@` template). Follow-up if needed — systemd covers the primary Linux target.

---

## Broker → Agent Auth Model

**Decision:** Dedicated proxy token. Not reuse of `broker_token` (which is agent→broker, not broker→agent).

### The problem with existing tokens

- `broker_token` in agent config = agent's credential to call broker API (agent→broker direction)
- Broker has no credential to call agent gateway API (broker→agent direction)
- ipc-quickstart.md: "broker itself does not need broker_url or broker_token"

### Solution: Bidirectional pairing

When agent pairs with broker (`POST /pair`), broker already stores the agent's token hash in `paired_tokens`. The **same token** that agent uses for IPC can be used by broker to call agent's gateway — but broker doesn't know the raw token (only the hash).

**v1 approach:** During agent registration (`POST /api/ipc/register-gateway`), agent includes a **proxy_token** — a fresh bearer token that broker stores and uses for WS proxy connections to agent's gateway.

```
Agent config:
[agents_ipc]
broker_url = "http://127.0.0.1:42617"
broker_token = "enc2:..."        # agent→broker auth (existing)
gateway_url = "http://127.0.0.1:42618"
proxy_token = "enc2:..."         # broker→agent auth (NEW, auto-generated on first start)
```

Registration payload:
```json
POST /api/ipc/register-gateway
Authorization: Bearer <broker_token>
{
  "gateway_url": "http://127.0.0.1:42618",
  "proxy_token": "zc_proxy_<random>"
}
```

Broker stores `proxy_token` in `AgentRegistry`. When proxying WS:
```
Broker → Agent WS: /ws/chat
  Sec-WebSocket-Protocol: zeroclaw.v1, bearer.<proxy_token>
```

Token is never sent in query string — avoids leaking in logs/diagnostics. Uses the same subprotocol auth path that `ws.rs` already supports (precedence: header > subprotocol > query).

Agent's gateway validates `proxy_token` via normal pairing check. Agent adds it to its own `paired_tokens` on first registration.

### Auth flow summary

```
Browser ──(operator token)──> Broker ──(proxy_token)──> Agent
         pairing with broker         stored from registration
```

Three distinct tokens, three distinct trust relationships. No ambiguity.

---

## Agent Registry Model

**Decision:** New `AgentRegistry` struct, NOT reuse of `NodeRegistry`.

Rationale: `NodeRegistry` (`nodes.rs`) is for ephemeral capability nodes connected via `/ws/nodes`. Different trust model, different lifecycle, different semantics. Mixing them would be dangerous.

### AgentRegistry fields

| Field | Type | Source |
|-------|------|--------|
| `agent_id` | String | From IPC TokenMetadata |
| `gateway_url` | String | From registration |
| `proxy_token` | String | From registration (encrypted at rest) |
| `trust_level` | u8 | From IPC TokenMetadata |
| `role` | String | From IPC TokenMetadata |
| `model` | String | From agent `/api/status` poll |
| `status` | enum(online/offline/error) | From health poll |
| `last_seen` | i64 | Updated on health poll |
| `uptime_seconds` | u64 | From agent `/api/status` poll |
| `channels` | Vec\<String\> | From agent `/api/status` poll |

### Storage

- **Primary:** `IpcDb` table `agent_gateways` (persisted, survives broker restart)
- **Enriched:** in-memory cache updated by health polls (model, status, uptime, channels)

This means after broker restart, the registry seed comes from DB (gateway_url + proxy_token), and live metadata refreshes within one poll cycle (30s).

### Registration & discovery flow

1. Agent daemon starts → pairs with broker (existing flow, gets `broker_token`)
2. Agent generates `proxy_token` if not yet in config (one-time, saved encrypted)
3. Agent enters **registration loop** (two phases):
   - **Phase A (fast retry):** POST `{gateway_url, proxy_token}` to broker's `/api/ipc/register-gateway` with exponential backoff (1s, 2s, 4s, 8s, max 30s). Retries indefinitely until first success. This covers "agent starts before broker" — no 5-minute wait.
   - **Phase B (periodic refresh):** after first successful registration, switch to 5-minute interval re-POST. Covers broker restart / DB loss. If a refresh fails, immediately fall back to Phase A (fast retry).
4. Broker stores in `agent_gateways` table (persistent) + in-memory `AgentRegistry`
5. Broker polls agent `/health` every 30s; also fetches `/api/status` for metadata
6. If agent unreachable for 3 consecutive polls → status = `offline`

**Hard rule:** agent never waits 5 minutes for initial registration. Fast retry is always first. 5-minute interval is only a refresh cadence after proven connectivity.

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
  │                          │   Sec-WebSocket-Protocol:   │
  │                          │   bearer.<proxy_token>      │
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
- Broker looks up agent's `gateway_url` + `proxy_token` in registry
- Broker opens WS to agent's `/ws/chat` with `Sec-WebSocket-Protocol: zeroclaw.v1, bearer.<proxy_token>` (not query param — avoids token leaking in logs/diagnostics)
- All frames are forwarded bidirectionally (no parsing, no transformation)
- If agent disconnects, broker sends error frame to browser and closes

### Non-chat API proxy

For `/api/status`, `/api/nodes` on a specific agent:
- Browser calls broker `GET /api/agents/{agent_id}/status`
- Broker proxies HTTP GET to agent's `{gateway_url}/api/status` with `Authorization: Bearer <proxy_token>`
- Returns response to browser

---

## Session Ownership Model

**v1 limitation (explicit):** All operator sessions on a given agent share one session namespace (keyed by broker's `proxy_token` hash prefix). This means:
- Single-operator lab: perfectly fine, operator sees all their sessions
- Multi-operator: all operators share one session namespace per agent

This is acceptable for v1 (lab/family use). Future enhancement path: broker injects operator identity into a session namespace prefix, giving each operator isolated sessions.

---

## Failure & Restart Model

| Scenario | Behavior |
|----------|----------|
| **Agent goes offline** | Broker health poll marks `offline` in registry. Browser shows "Agent offline" in selector. Sessions persist in agent's DB, resume when agent returns. |
| **Agent restarts** | Agent re-registers (immediate POST + periodic 5min). Broker updates registry. If browser had it selected, proxy WS reconnects automatically. Sessions resume from agent's chat DB. |
| **Broker restarts** | Registry seed loaded from `agent_gateways` DB table. Live metadata refreshes within 30s poll. Agents detect failed periodic refresh → fall back to Phase A fast retry (backoff from 1s). Browser reconnects via existing WS auto-reconnect. No session data loss (sessions on agents). |
| **Machine reboot** | systemd/launchd starts all enabled services. Start order does not matter — agents use fast retry with backoff until broker accepts registration. No manual intervention needed. |
| **One agent restart** | Other agents unaffected. Restarted agent re-registers. Browser can switch to working agents during downtime. |
| **Config change** | Restart specific instance: `systemctl --user restart zeroclaw@opus`. No impact on other instances. |
| **Browser refresh** | Reconnects to broker WS. Agent selector restores from localStorage. Sessions loaded from agent via proxy. |

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

### Step 1: Multi-instance service model
- Add `--instance <name>` flag to `daemon` and `service` commands
- Instance resolves config dir: `~/.zeroclaw/agents/<name>/`
- Default (no flag) = `~/.zeroclaw/` (broker)
- Templated systemd unit `zeroclaw@.service`
- Per-instance launchd plist `com.zeroclaw.agent-<name>.plist`
- Per-instance Windows task `ZeroClaw Agent (<name>)`

### Step 2: Proxy token generation + config
- Add `proxy_token: Option<String>` to `[agents_ipc]` config
- Auto-generate on first daemon start if not set (save encrypted to config)
- Agent adds `proxy_token` to its own `paired_tokens` for gateway auth

### Step 3: Agent gateway registration endpoint
- `POST /api/ipc/register-gateway` on broker
- Accepts `{gateway_url, proxy_token}` from authenticated agent
- Creates `agent_gateways` table in IpcDb (agent_id, gateway_url, proxy_token, registered_at)
- Stores in in-memory `AgentRegistry`

### Step 4: Agent auto-registration + periodic re-registration
- Agent daemon: after IPC pairing, POST registration to broker
- **Initial registration:** fast retry with exponential backoff (1s→2s→4s→...→30s max) until first success. Covers "agent starts before broker" without 5-min wait.
- **After first success:** periodic re-registration every 5 minutes as refresh (covers broker restart, DB loss)
- Config: `gateway_url` auto-detected from `gateway.host:gateway.port` if not set

### Step 5: Broker health polling + AgentRegistry
- New `AgentRegistry` struct (not NodeRegistry)
- Broker polls each registered agent's `/health` + `/api/status` every 30s
- Updates registry with status, model, channels, uptime
- 3 missed polls → status = `offline`

### Step 6: Broker `/api/agents` endpoint
- `GET /api/agents` — returns list of registered agents with live status
- Used by browser for agent selector dropdown

### Step 7: WS chat proxy on broker
- New WS endpoint: `/ws/chat/proxy?agent=<agent_id>`
- Broker looks up `gateway_url` + `proxy_token` from AgentRegistry
- Opens upstream WS to agent's `/ws/chat` with `Sec-WebSocket-Protocol: bearer.<proxy_token>` (subprotocol auth, not query param)
- Bidirectional frame relay (transparent, no parsing)
- Error handling: agent offline → error frame → close

### Step 8: HTTP API proxy for per-agent calls
- `GET /api/agents/{agent_id}/status` → proxies to agent with proxy_token
- `GET /api/agents/{agent_id}/health` → proxies to agent
- Generic pattern for future per-agent API calls

### Step 9: Browser agent selector UI
- Dropdown in chat sidebar (above session list)
- Fetches `/api/agents` from broker
- On switch: close current proxy WS, open new one with `?agent=<id>`
- Persist selected agent in localStorage

### Step 10: Agent status display in sidebar
- Sidebar info panel (from 3.7b) shows selected agent's model, uptime, status
- Fetched via proxy `/api/agents/{id}/status`

---

## Verification Checklist

### Functional

- [ ] Broker starts with `agents_ipc.enabled = true`
- [ ] Agent registers `gateway_url` + `proxy_token` with broker on startup
- [ ] Broker `/api/agents` returns list with live status
- [ ] Browser agent selector shows all agents with status indicators
- [ ] Selecting agent opens proxy WS, loads that agent's sessions
- [ ] Chat works through proxy (send, receive, tool events, abort, lifecycle events)
- [ ] Agent going offline → selector shows "offline", error in chat
- [ ] Agent restart → re-registers, proxy reconnects, sessions resume
- [ ] Broker restart → registry rebuilds from DB seed + agent re-registration
- [ ] One SSH tunnel to broker is sufficient for full operation

### Service lifecycle — restart matrix

Each scenario must pass on every supported platform (systemd, launchd, Windows):

- [ ] `service install` — broker default instance
- [ ] `service install --instance <name>` — agent instance
- [ ] Multiple instances run simultaneously on same machine
- [ ] Machine reboot → all enabled services start automatically
- [ ] Start in any order — agents fast-retry (Phase A) until broker is up
- [ ] Restart one agent → other agents and broker unaffected
- [ ] Restart broker → agents fall back to Phase A fast retry, re-register within seconds
- [ ] Broker comes up late (after agents) → agents register once broker appears
- [ ] One agent config change + restart → no impact on others

Platform-specific:
- [ ] Linux: `systemctl --user status zeroclaw@opus` shows correct status
- [ ] macOS: `launchctl list | grep zeroclaw` shows all instances
- [ ] Windows: Task Scheduler shows all agent tasks
- [ ] OpenRC: out of scope (documented)

### Auth

- [ ] Browser authenticates with broker (operator pairing)
- [ ] Broker authenticates with agent (proxy_token from registration)
- [ ] proxy_token encrypted at rest in agent config
- [ ] Agent without proxy_token auto-generates one on first start
- [ ] Invalid proxy_token → WS proxy fails cleanly, error shown in browser

---

## Risks

| Risk | Mitigation |
|------|-----------|
| WS proxy adds latency | Transparent relay (no parsing), same-machine connections are <1ms |
| Broker becomes SPOF for dashboard | Agents continue running independently. Only dashboard access lost. Broker restarts fast. |
| Three-layer auth complexity | Clear separation: operator→broker, broker→agent (proxy_token), agent→broker (broker_token). Each is a single bearer token. |
| Session namespace collision (multi-operator) | v1 limitation: single namespace per agent. Documented. Future: operator identity in namespace prefix. |
| Registry stale after broker restart | DB-persisted seed + agent Phase A fast retry (seconds, not minutes). Metadata refresh within 30s poll. |
| N+1 process management | Templated systemd/launchd units. `service install --instance` handles creation. Standard OS tooling for lifecycle. |
| Agent starts before broker | Agent retries registration with exponential backoff. No crash, no data loss. |

---

## CLI Surface

### New flag

`--instance <name>` on `daemon` and `service` subcommands:

```bash
zeroclaw daemon                          # broker (default, ~/.zeroclaw/)
zeroclaw daemon --instance opus          # agent (~/.zeroclaw/agents/opus/)
zeroclaw service install --instance opus # install agent as OS service
zeroclaw service status --instance opus  # check agent service status
```

### No new top-level subcommands

Configuration is the differentiator between broker and agent, not commands. Both run `zeroclaw daemon`.

---

## UI Agent Provisioning (Broker-Only)

Phase 3.6 introduced config generation + pairing from the web UI. Phase 3.8 adds the infrastructure to actually create and manage agent instances from the broker dashboard.

### Security model

This is a dangerous operation. Not a regular API feature.

**Principles:**
1. **Broker-only** — only the broker instance can provision agents
2. **Disabled by default** — must be explicitly enabled in config
3. **Mode-based escalation** — three levels of capability
4. **Dual auth** — requires both paired bearer token AND localhost access
5. **Fixed write paths** — only `~/.zeroclaw/agents/<instance>/`
6. **No arbitrary commands** — only predefined lifecycle actions
7. **Audited** — all operations logged as audit events
8. **Temporary arming** — runtime arm with TTL, auto-disables

### Config

```toml
[gateway.ui_provisioning]
enabled = false                    # master switch (see security rule below)
mode = "config_only"               # config_only | service_install
agents_root = "~/.zeroclaw/agents" # fixed root, no arbitrary paths
allow_blueprints = false           # Phase 3.6 fleet blueprints
```

**Security rule:** `gateway.ui_provisioning.enabled` can ONLY be changed by editing the local config file + restarting the broker. It MUST be rejected if submitted via `PUT /api/config` — the config PUT handler must strip or reject changes to this field. Rationale: `/api/config` is bearer-auth only (not localhost-gated), so allowing it to flip the provisioning master switch would be a privilege escalation path from any paired client.

### Modes

| Mode | What UI can do |
|------|---------------|
| `disabled` | Generate + download config only (Phase 3.6 behavior) |
| `config_only` | Create agent dir, write config.toml, write instructions.md, issue paircode |
| `service_install` | All above + install OS service instance + enable/start it |

### Runtime arming

Even with `enabled = true`, provisioning requires explicit runtime activation:

```
POST /admin/provisioning/arm
{ "minutes": 30 }
```

- **Localhost-only** — same gate as `/admin/ipc/*`
- **Paired admin token required** — bearer auth
- **TTL auto-expire** — after N minutes, provisioning disarmed
- **Broker restart** — always disarmed (safe default)

### Endpoints

All under `/admin/provisioning/*` (localhost + bearer auth):

| Endpoint | Mode | What it does |
|----------|------|-------------|
| `POST /admin/provisioning/arm` | any | Arm provisioning for N minutes |
| `GET /admin/provisioning/status` | any | Is it armed? Mode? TTL remaining? |
| `POST /admin/provisioning/create` | config_only+ | Create agent dir + write config.toml |
| `POST /admin/provisioning/install` | service_install | Install + enable OS service |
| `POST /admin/provisioning/start` | service_install | Start agent service |
| `POST /admin/provisioning/stop` | service_install | Stop agent service |

### Instance name validation

`^[a-z0-9][a-z0-9_-]{0,30}$` — lowercase, digits, hyphens, underscores. Max 31 chars.

Write paths are always:
- `{agents_root}/{instance}/config.toml`
- `{agents_root}/{instance}/workspace/instructions.md`

### Audit events

| Event | Fields |
|-------|--------|
| `provisioning_armed` | operator, minutes, mode |
| `provisioning_disarmed` | reason (ttl/manual/restart) |
| `agent_config_written` | instance, operator, path |
| `agent_service_installed` | instance, operator, platform |
| `agent_service_started` | instance, operator |
| `agent_service_stopped` | instance, operator |
| `provisioning_failed` | instance, operator, error |

### Implementation note

UI provisioning is **Step 11** (optional, after core 3.8 proxy works). It bridges Phase 3.6 (config generation) with Phase 3.8 (multi-instance model). Can ship as a follow-up PR after Steps 1–10.

---

## Decisions

1. **Broker is transparent relay, not application proxy** — no message transformation, no session merging, no LLM calls on behalf of agents.
2. **Sessions remain per-agent** — no cross-agent session model in v1. Single session namespace per agent (v1 limitation).
3. **One process per agent** — no multi-agent-in-one-process model.
4. **Dedicated proxy_token for broker→agent auth** — not reuse of broker_token (wrong direction). Generated by agent, stored by broker.
5. **New AgentRegistry, not NodeRegistry** — different trust, lifecycle, semantics.
6. **DB-persisted registry + periodic re-registration** — covers broker restart without manual repair.
7. **Templated multi-instance service units** — `zeroclaw@.service` on systemd, per-instance plists on launchd.
8. **WS proxy, not HTTP long-poll** — preserves streaming, tool events, lifecycle events.
9. **Agent selector in sidebar** — not a separate page, integrated into chat flow.
10. **UI provisioning is broker-only, mode-gated, arm-required** — disabled by default, three escalation modes, localhost+bearer dual auth, fixed write paths, audit trail, TTL auto-disarm.
