# Phase 3.8 Progress: Broker-Centered Multi-Agent Dashboard

## Status: Not Started

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | Multi-instance service model | todo | — | `--instance <name>`, templated systemd/launchd units, config dir layout |
| 2 | Proxy token generation + config | todo | — | `proxy_token` in `[agents_ipc]`, auto-generate, encrypt at rest |
| 3 | Agent gateway registration endpoint | todo | — | `POST /api/ipc/register-gateway`, `agent_gateways` table in IpcDb |
| 4 | Agent auto-registration + re-registration | todo | — | Fast retry with backoff on startup, periodic 5min refresh after |
| 5 | Broker health polling + AgentRegistry | todo | — | New `AgentRegistry` struct, 30s poll, offline detection |
| 6 | Broker `/api/agents` endpoint | todo | — | List agents with live status for browser selector |
| 7 | WS chat proxy on broker | todo | — | `/ws/chat/proxy?agent=<id>`, bidirectional relay, subprotocol auth |
| 8 | HTTP API proxy for per-agent calls | todo | — | `GET /api/agents/{id}/status` etc. |
| 9 | Browser agent selector UI | todo | — | Dropdown in sidebar, localStorage persistence |
| 10 | Agent status display in sidebar | todo | — | Extend 3.7b panel with selected agent info via proxy |

---

## Verification

### Functional
- [ ] Broker starts, agents register with gateway_url + proxy_token
- [ ] `/api/agents` returns correct list with live status
- [ ] Agent selector works in browser
- [ ] Chat through proxy works end-to-end (send, receive, tool events, abort)
- [ ] Offline agent handled gracefully
- [ ] Agent restart → re-registers, proxy reconnects
- [ ] Broker restart → registry from DB seed, agents re-register
- [ ] One SSH tunnel sufficient

### Service lifecycle
- [ ] Multi-instance install works on Linux (systemd user units, `default.target`)
- [ ] Multi-instance install works on macOS (launchd)
- [ ] Multi-instance install works on Windows (scheduled tasks)
- [ ] OpenRC: explicitly out of scope (documented)
- [ ] Machine reboot → all enabled services start automatically
- [ ] Start in any order (agents fast-retry with backoff until broker up)
- [ ] Restart one agent → others and broker unaffected
- [ ] Restart broker → agents detect failure, switch to fast retry, re-register
- [ ] No manual intervention needed for any restart scenario

### Auth
- [ ] proxy_token generated, encrypted, used for broker→agent via subprotocol
- [ ] Invalid proxy_token → clean error in browser
- [ ] Three-layer auth works end-to-end (operator→broker→agent)
- [ ] proxy_token never appears in URL query strings or logs
