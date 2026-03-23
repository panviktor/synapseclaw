# Fork Delta Registry

## Purpose

This file records **all intentional delta of the fork** relative to upstream — not just IPC, but every area where the fork diverges.

It serves three purposes:
- understand exactly what we maintain ourselves
- simplify sync with upstream (conflict classification, resolution strategy per file)
- separate fork-only policy from code that can eventually be extracted upstream

Related documents:
- [`sync-strategy.md`](sync-strategy.md) — fork sync strategy
- [`sync-review-rubric.md`](sync-review-rubric.md) — review rules for sync PRs
- [`ipc-plan.md`](ipc-plan.md) — IPC design and phases

## Statuses

- `fork-only` — product logic that is not planned to go upstream as a whole
- `candidate-upstream` — a neutral primitive / extension point
- `temporary-backport` — a temporarily backported upstream fix that should disappear after sync

## Merge Risk

- `low` — isolated, conflicts are rare
- `medium` — periodic conflicts in shared files
- `high` — security/gateway/config hotspots; manual review is mandatory

---

## IPC Core (IPC-001 .. IPC-015)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| IPC-001 | Broker-mediated IPC endpoints + SQLite store | `candidate-upstream` | `high` | `src/gateway/ipc.rs`, `src/gateway/mod.rs` | Base substrate can be split into neutral PRs |
| IPC-002 | Token metadata, IPC eligibility, revoke/disable/downgrade hooks | `candidate-upstream` | `high` | `src/security/pairing.rs`, `src/config/schema.rs`, `src/gateway/ipc.rs` | Strong upstream candidate as primitives |
| IPC-003 | Correlated `result` only + session validation | `candidate-upstream` | `medium` | `src/gateway/ipc.rs` | Useful as generic safety rule |
| IPC-004 | L0-L4 trust hierarchy and directional ACL matrix | `fork-only` | `high` | `src/gateway/ipc.rs`, `src/config/schema.rs` | Tightly coupled to product model |
| IPC-005 | L4 quarantine lane (read-only for execution) | `fork-only` | `high` | `src/gateway/ipc.rs`, tools/inbox, audit events | Unlikely to go upstream as-is |
| IPC-006 | Sparse mesh lateral policy (`L2↔L2`, `L3↔L3`, allowlisted FYI text) | `fork-only` | `medium` | `src/gateway/ipc.rs`, config allowlists | Policy-specific |
| IPC-007 | Logical destinations for L4 (`supervisor`, `escalation`) | `fork-only` | `medium` | `src/gateway/ipc.rs`, config schema | Routing tied to low-trust model |
| IPC-008 | Approval broker via Opus / control plane / `#approvals` | `fork-only` | `high` | orchestration policy, channel integrations, audit | Authority boundary |
| IPC-009 | Structured IPC tracing events | `candidate-upstream` | `medium` | `src/gateway/ipc.rs`, tracing | Neutral observability layer |
| IPC-010 | Agent IPC tools (`agents_list/send/inbox/reply/state/spawn`) | `candidate-upstream` | `medium` | `src/tools/agents_ipc.rs`, `src/tools/mod.rs` | Policy surface is not upstreamable |
| IPC-011 | Subprocess spawn with broker-backed identity | `fork-only` | `high` | `src/tools/agents_ipc.rs`, `src/cron/*` | Phase 3A: subprocess execution, ephemeral identity |
| IPC-012 | Config masking/encryption for IPC secrets | `candidate-upstream` | `medium` | `src/gateway/api.rs`, `src/config/schema.rs` | Good generic hardening |
| IPC-013 | Ephemeral identity provisioning + spawn_runs table | `fork-only` | `high` | `src/gateway/ipc.rs`, `src/security/pairing.rs` | Phase 3A: runtime-only tokens, auto-revoke |
| IPC-014 | Child process IPC bootstrap via env vars | `candidate-upstream` | `low` | `src/config/schema.rs`, `src/agent/prompt.rs` | Env-based IPC auto-config |
| IPC-015 | Fail-closed execution profiles + workload profiles | `fork-only` | `high` | `src/security/execution.rs`, `src/config/schema.rs` | Phase 3A: trust-derived sandbox enforcement |

## Security (SEC-001 .. SEC-003)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| SEC-001 | Ed25519 agent identity + register-key endpoint | `candidate-upstream` | `medium` | `src/security/identity.rs`, `src/security/mod.rs` | Signed messages, broker verifies keys |
| SEC-002 | PromptGuard integration for IPC payloads | `fork-only` | `medium` | `src/security/prompt_guard.rs`, `src/gateway/ipc.rs` | Scans payloads before insert; exempt levels configurable |
| SEC-003 | Execution profiles (fail-closed sandbox, workload profiles) | `fork-only` | `high` | `src/security/execution.rs`, `src/config/schema.rs` | L2+ refuse to start without sandbox; same as IPC-015 |

## Gateway (GW-001 .. GW-005)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| GW-001 | IPC route registration + AppState extensions | `candidate-upstream` | `high` | `src/gateway/mod.rs`, `src/gateway/api.rs` | All IPC endpoints mounted here |
| GW-002 | Agent registry + broker health polling | `fork-only` | `medium` | `src/gateway/agent_registry.rs` | New file, fork-owned |
| GW-003 | Chat session SQLite persistence | `candidate-upstream` | `medium` | `src/gateway/chat_db.rs` | New file; neutral session store |
| GW-004 | Agent provisioning (add from UI, presets, config gen) | `fork-only` | `medium` | `src/gateway/provisioning.rs` | New file, fork-owned |
| GW-005 | WS chat proxy + HTTP API proxy for multi-agent dashboard | `fork-only` | `high` | `src/gateway/ws.rs`, `src/gateway/mod.rs` | Phase 3.8: broker proxies to per-agent gateways |

## Agent / Loop (AGT-001 .. AGT-002)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| AGT-001 | IPC bootstrap prompt injection via env vars | `candidate-upstream` | `low` | `src/agent/prompt.rs` | Appends IPC instructions when `SYNAPSECLAW_IPC_*` env vars set |
| AGT-002 | `agent::run()` + `process_message()` signature extensions | `fork-only` | `high` | `src/agent/agent.rs`, `src/agent/loop_.rs` | Added IPC-related params; conflicts on every upstream signature change |

## Cron / Scheduler (CRON-001 .. CRON-002)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| CRON-001 | `allowed_tools` field on CronJob | `candidate-upstream` | `low` | `src/cron/types.rs`, `src/cron/store.rs` | Capability-based tool access per job |
| CRON-002 | Cron-triggered `agents_spawn` integration | `fork-only` | `medium` | `src/cron/scheduler.rs`, `src/cron/mod.rs` | Scheduler passes allowed_tools to agent::run |

## Config (CFG-001 .. CFG-002)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| CFG-001 | `AgentsIpcConfig`, `IpcPromptGuardConfig`, `TokenMetadata` structs | `fork-only` | `high` | `src/config/schema.rs`, `src/config/mod.rs` | Added to Config struct; conflicts when upstream adds adjacent fields |
| CFG-002 | Execution profile + workload config fields | `fork-only` | `medium` | `src/config/schema.rs` | `SandboxConfig` extensions, `ExecutionProfileConfig` |

## Web UI (UI-001 .. UI-006)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| UI-001 | IPC dashboard pages (Fleet, Audit, Quarantine, Sessions, Spawns, AgentDetail) | `fork-only` | `low` | `web/src/pages/ipc/*` | 6 new pages, ~1200 lines total |
| UI-002 | IPC UI components (badges, dialogs, detail views) | `fork-only` | `low` | `web/src/components/ipc/*` | ~10 new components |
| UI-003 | IPC API client + types | `fork-only` | `low` | `web/src/lib/ipc-api.ts`, `web/src/types/ipc.ts` | Isolated from upstream web code |
| UI-004 | Agent provisioning UI (presets, config gen, deploy blueprint) | `fork-only` | `low` | `web/src/lib/ipc-presets.ts`, `ipc-config-gen.ts`, `ipc-providers.ts`, `ipc-channels.ts` | New files, fork-owned |
| UI-005 | Chat session sidebar + multi-session store | `fork-only` | `medium` | `web/src/pages/AgentChat.tsx`, `web/src/hooks/useChatStore.ts`, `web/src/components/chat/SessionSidebar.tsx` | Extends upstream AgentChat page |
| UI-006 | App routing + sidebar navigation for IPC | `fork-only` | `medium` | `web/src/App.tsx`, `web/src/components/layout/Sidebar.tsx` | Adds IPC routes; conflicts when upstream adds own routes |

## Web Infra (WEB-001 .. WEB-003)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| WEB-001 | WS client refactor (chat store, session support) | `fork-only` | `medium` | `web/src/lib/ws.ts`, removed `web/src/hooks/useWebSocket.ts` | Replaced upstream hook with session-aware store |
| WEB-002 | i18n stub | `fork-only` | `low` | `web/src/lib/i18n.ts` | New file |
| WEB-003 | API types extensions | `fork-only` | `medium` | `web/src/types/api.ts`, `web/src/lib/api.ts` | Extended upstream types with IPC fields |

## Channels (CH-001)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| CH-001 | Matrix channel fixes (media, E2EE, dedup) | `temporary-backport` | `medium` | `src/channels/matrix.rs` | May be resolved by upstream; review on every sync |

## Fork Core (CORE-001)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| CORE-001 | Fork-owned application core: OutboundIntent, ChannelRegistryPort, CachedChannelRegistry, bus, push relay | `fork-only` | `low` | `src/fork_core/*`, `src/fork_adapters/*`, `src/daemon/mod.rs` | Phase 4.0 Steps 1-2; new files, fork-owned |

## Other (MISC-001 .. MISC-003)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| MISC-001 | Integration registry wiring for IPC | `fork-only` | `low` | `src/integrations/registry.rs` | IPC service registration |
| MISC-002 | Multimodal extensions | `fork-only` | `low` | `src/multimodal.rs` | Minor fork-specific additions |
| MISC-003 | Claude Code skills (ipc-context, ipc-review, ipc-smoke, pre-pr, sync-upstream) | `fork-only` | `low` | `.claude/skills/*` | Not in upstream; no conflict risk |

## Infra / CI (INFRA-001 .. INFRA-002)

| ID | Change | Status | Merge risk | Main files | Notes |
|----|--------|--------|------------|------------|-------|
| INFRA-001 | Fork-invariants CI job | `fork-only` | `low` | `.github/workflows/checks-on-pr.yml` | Tests IPC, pairing, agents_ipc |
| INFRA-002 | Sync automation (scripts, workflow, templates) | `fork-only` | `low` | `scripts/sync-upstream.sh`, `scripts/report-sync-conflicts.sh`, `scripts/render-sync-pr-body.sh`, `.github/workflows/upstream-sync.yml` | Fork-owned |

---

## Shared Hotspots

These files should automatically go onto the manual review list for every sync PR:

**Config:**
- `src/config/schema.rs`
- `src/config/mod.rs`

**Gateway:**
- `src/gateway/mod.rs`
- `src/gateway/api.rs`
- `src/gateway/ws.rs`

**Security:**
- `src/security/pairing.rs`
- `src/security/audit.rs`

**Agent:**
- `src/agent/agent.rs`
- `src/agent/loop_.rs`

**Other:**
- `src/tools/mod.rs`
- `src/onboard/wizard.rs`
- `src/cron/scheduler.rs`
- `src/daemon/mod.rs`
- `src/main.rs`
- `src/service/mod.rs`

**Web:**
- `web/src/App.tsx`
- `web/src/components/layout/Sidebar.tsx`
- `web/src/lib/ws.ts`
- `web/src/types/api.ts`

## Fork-Owned Paths (ours-first on merge)

These are new files created by the fork. Accept fork version on conflict:
- `src/gateway/ipc.rs`
- `src/gateway/agent_registry.rs`
- `src/gateway/chat_db.rs`
- `src/gateway/provisioning.rs`
- `src/security/execution.rs`
- `src/security/identity.rs`
- `src/security/prompt_guard.rs`
- `src/tools/agents_ipc.rs`
- `web/src/pages/ipc/*`
- `web/src/components/ipc/*`
- `web/src/lib/ipc-*.ts`
- `web/src/types/ipc.ts`
- `web/src/hooks/useChatStore.ts`
- `web/src/components/chat/SessionSidebar.tsx`
- `src/fork_core/*`
- `src/fork_adapters/*`
- `docs/fork/*`
- `.claude/skills/*`
- `scripts/sync-upstream.sh`, `scripts/report-sync-conflicts.sh`, `scripts/render-sync-pr-body.sh`

## Review Rules

### For `candidate-upstream`

On every sync and every noticeable redesign, ask:
- can this be separated from our trust/policy model?
- can it be extracted into a separate hook, trait, helper, or neutral API?
- can we prepare a small upstream PR instead of growing the fork further?

### For `fork-only`

On every sync, verify:
- has the logic spread further across shared-hotspot files?
- can it be moved behind an overlay/module boundary?
- has a hidden dependency on upstream internals appeared that will make the next merge harder?

### For `temporary-backport`

On every sync, check:
- has upstream merged the fix? If yes, remove the backport entry.

## Updating This File

Update this registry when:
- a new fork-owned file or shared hotspot is added
- the `fork-only` ↔ `candidate-upstream` status changes
- a temporary backport appears or is resolved
- the architectural boundary shifts

## Delta Summary

| Category | fork-only | candidate-upstream | temporary-backport | Total |
|----------|-----------|-------------------|-------------------|-------|
| IPC Core | 8 | 7 | 0 | 15 |
| Security | 2 | 1 | 0 | 3 |
| Gateway | 3 | 2 | 0 | 5 |
| Agent/Loop | 1 | 1 | 0 | 2 |
| Cron | 1 | 1 | 0 | 2 |
| Config | 2 | 0 | 0 | 2 |
| Web UI | 6 | 0 | 0 | 6 |
| Web Infra | 3 | 0 | 0 | 3 |
| Channels | 0 | 0 | 1 | 1 |
| Fork Core | 1 | 0 | 0 | 1 |
| Other | 3 | 0 | 0 | 3 |
| Infra/CI | 2 | 0 | 0 | 2 |
| **Total** | **32** | **12** | **1** | **45** |

## Current Conclusion

The main maintenance task is not just “merge more often,” but **systematically reduce the volume of intentional delta**.
Everything neutral should gradually be extracted into upstream primitives. Everything policy-specific should be tightly isolated and explicitly marked as fork-only.

The Web UI delta (UI-001..006, WEB-001..003) is low-conflict because most files are fork-owned additions. The highest-risk areas remain `src/config/schema.rs`, `src/agent/loop_.rs`, and `src/gateway/mod.rs` — these are shared hotspots where both fork and upstream actively add code.
