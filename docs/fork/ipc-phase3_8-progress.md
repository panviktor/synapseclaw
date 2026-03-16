# Phase 3.8 Progress: Broker-Centered Multi-Agent Dashboard

## Status: Not Started

---

## Steps

| # | Step | Status | PR | Notes |
|---|------|--------|----|-------|
| 1 | Agent gateway registration endpoint | todo | — | `POST /api/ipc/register-gateway` on broker |
| 2 | Broker health polling loop | todo | — | Poll agent `/health` + `/api/status` every 30s |
| 3 | Broker `/api/agents` endpoint | todo | — | List registered agents with live status |
| 4 | WS chat proxy on broker | todo | — | `/ws/chat/proxy?agent=<id>`, bidirectional relay |
| 5 | HTTP API proxy for per-agent calls | todo | — | `GET /api/agents/{id}/status` etc. |
| 6 | Agent auto-registration on startup | todo | — | Agent daemon POSTs gateway_url after pairing |
| 7 | Browser agent selector UI | todo | — | Dropdown in sidebar, localStorage persistence |
| 8 | Agent status in sidebar info panel | todo | — | Extend 3.7b panel with selected agent info |

---

## Verification

- [ ] Broker starts, agents register
- [ ] `/api/agents` returns correct list
- [ ] Agent selector works in browser
- [ ] Chat through proxy works end-to-end
- [ ] Offline agent handled gracefully
- [ ] Restart scenarios pass
- [ ] One SSH tunnel sufficient
