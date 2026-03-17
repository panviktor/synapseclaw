# Phase 3.8 Progress: Broker-Centered Multi-Agent Dashboard

## Status: In Progress (Steps 1-10 done, Step 11 optional)

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | Multi-instance service model | done | #103 | `--instance <name>`, templated systemd/launchd units, config dir layout |
| 2 | Proxy token generation + config | done | #104 | `proxy_token` in `[agents_ipc]`, auto-generate, encrypt at rest |
| 3 | Agent gateway registration endpoint | done | #105 | `POST /api/ipc/register-gateway`, `agent_gateways` table in IpcDb |
| 4 | Agent auto-registration + re-registration | done | #106 | Phase A: fast retry with backoff. Phase B: 5min refresh after first success. |
| 5 | Broker health polling + AgentRegistry | done | #107 | New `AgentRegistry` struct, 30s poll, offline detection |
| 6 | Broker `/api/agents` endpoint | done | #108 | List agents with live status for browser selector |
| 7 | WS chat proxy on broker | done | #109 | `/ws/chat/proxy?agent=<id>`, bidirectional relay, subprotocol auth |
| 8 | HTTP API proxy for per-agent calls | done | #110 | `GET /api/agents/{id}/status` proxy to agent |
| 9 | Browser agent selector UI | done | #110 | Dropdown in sidebar, localStorage persistence |
| 10 | Agent status display in sidebar | done | #110 | Info panel shows selected agent's model/uptime via proxy |
| 11 | UI agent provisioning (optional) | todo | — | Broker-only, mode-gated, arm-required. Bridges 3.6→3.8. |

---

## Verification

### Functional
- [ ] Broker starts, agents register with gateway_url + proxy_token
- [ ] `/api/agents` returns correct list with live status
- [ ] Agent selector works in browser
- [ ] Chat through proxy works end-to-end (send, receive, tool events, abort)
- [ ] Offline agent handled gracefully
- [ ] Agent restart → re-registers (Phase A fast retry), proxy reconnects
- [ ] Broker restart → registry from DB seed, agents fast-retry re-register
- [ ] One SSH tunnel sufficient

### Service lifecycle — restart matrix

Each scenario on every supported platform:

| Scenario | systemd | launchd | Windows |
|----------|---------|---------|---------|
| Install broker (default) | [ ] | [ ] | [ ] |
| Install agent instance | [ ] | [ ] | [ ] |
| Multiple instances run concurrently | [ ] | [ ] | [ ] |
| Machine reboot → all start | [ ] | [ ] | [ ] |
| Start in any order (agents before broker) | [ ] | [ ] | [ ] |
| Restart one agent → others unaffected | [ ] | [ ] | [ ] |
| Restart broker → agents re-register (seconds) | [ ] | [ ] | [ ] |
| Broker comes up late → agents register | [ ] | [ ] | [ ] |
| Agent config change + restart → no cascade | [ ] | [ ] | [ ] |

- [ ] OpenRC: out of scope (documented)

### Auth
- [ ] proxy_token generated, encrypted, used for broker→agent via subprotocol
- [ ] Invalid proxy_token → clean error in browser
- [ ] Three-layer auth works end-to-end (operator→broker→agent)
- [ ] proxy_token never appears in URL query strings or logs

### UI Provisioning (Step 11, if implemented)
- [ ] Disabled by default, requires config change + restart to enable
- [ ] Mode gating: config_only vs service_install
- [ ] Runtime arm/disarm with TTL
- [ ] Localhost + bearer dual auth required
- [ ] Writes only under agents_root
- [ ] Instance name validated (`^[a-z0-9][a-z0-9_-]{0,30}$`)
- [ ] All operations generate audit events
- [ ] Broker restart → provisioning disarmed
