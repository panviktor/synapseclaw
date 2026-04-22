# Phase 3.8 Progress: Broker-Centered Multi-Agent Dashboard

## Status: Complete (Steps 1-11 done, 2 audit rounds)

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
| 11 | UI agent provisioning | done | #111 | Broker-only, mode-gated, arm-required. 6 endpoints, full audit trail. |

## Audit Rounds

| Round | PR | Findings fixed |
|-------|-----|---------------|
| Audit 1 | #112 | 9 findings: broker_token decrypt (critical), instance name validation, systemd quoting, proxy_token validation, save ordering, Phase B dedup, audit gaps, warnings |
| Audit 2 | #113 | 3 findings: frontend proxy wiring (high), proxy_token persistence on restart (high), session namespace confirmed v1 limitation |
| Audit 3 | #114 | 2 findings: trust/role restore on broker restart, 12 new unit tests |

## Known v1 Limitations

- **Shared session namespace**: all operators through one broker share one session namespace per agent (keyed by proxy_token hash). Documented in plan. Per-operator isolation deferred.

---

## Verification

### Functional
- [x] Broker starts, agents register with gateway_url + proxy_token
- [x] `/api/agents` returns correct list with live status
- [x] Agent selector works in browser (dropdown, localStorage)
- [x] Frontend connects to `/ws/chat/proxy?agent=<id>` when agent selected
- [x] Agent status fetched via proxy `GET /api/agents/{id}/status`
- [x] Offline agent handled gracefully (503 on proxy connect)
- [x] Broker restart → registry seeded from DB with trust/role from IPC agents table
- [ ] E2E: full broker + remote agent + chat through proxy + restart scenarios

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

- [x] OpenRC: out of scope (documented)

### Auth
- [x] proxy_token generated, encrypted on save, decrypted on load
- [x] proxy_token reconciled into paired_tokens on every daemon start
- [x] proxy_token not exposed in `/api/agents` response (serde skip)
- [x] proxy_token validated on register-gateway (non-empty, <=256 chars)
- [x] Three-layer auth: operator→broker, broker→agent (subprotocol), agent→broker (bearer)
- [ ] Invalid proxy_token → clean error in browser (needs manual test)

### UI Provisioning (Step 11)
- [x] Disabled by default
- [x] Entire subtree immutable via PUT /api/config
- [x] Mode gating: config_only vs service_install
- [x] Runtime arm/disarm with TTL
- [x] Localhost + human operator token (L0/L1) dual auth
- [x] Agent tokens (L2+) rejected even from localhost
- [x] Writes only under agents_root
- [x] Instance name validated (`^[a-z0-9][a-z0-9_-]{0,30}$`)
- [x] All operations generate audit events (7 types)
- [x] Broker restart → provisioning disarmed

### Tests (12 new)
- [x] AgentRegistry: upsert_and_get
- [x] AgentRegistry: upsert_resets_status_and_missed_polls
- [x] AgentRegistry: offline_after_three_failures
- [x] AgentRegistry: update_metadata_resets_missed_polls
- [x] AgentRegistry: set_trust_info
- [x] AgentRegistry: list_and_remove
- [x] AgentRegistry: get_nonexistent_returns_none
- [x] IpcDb: agent_gateway_upsert_and_list
- [x] IpcDb: agent_gateway_get
- [x] IpcDb: agent_gateway_upsert_updates_existing
- [x] IpcDb: agent_gateway_remove
- [x] IpcDb: agent_gateway_seed_with_trust_info
