---
name: ipc-context
description: "Load full fork project context for a new session. Reads delta registry, plans, progress, architectural decisions, and current state so Claude understands the multi-agent IPC system, web UI, security extensions, and fork strategy. Use at the start of any session that will touch IPC code, fork plans, sync, or reviews. Trigger on: 'загрузи контекст', 'что мы делаем', 'контекст IPC', 'catch me up', 'where are we', 'new session', 'контекст форка'."
user-invocable: true
---

# Fork Project Context Loader

Load the full project context so this session understands the fork: IPC system, web UI, security extensions, sync strategy, and what's next.

## Step 1: Read core documents

Read these files in parallel:

- `docs/fork/README.md` — doc index and branch model
- `docs/fork/delta-registry.md` — **all** fork deltas (44+ entries across 11 categories), shared hotspots, fork-owned paths
- `docs/fork/sync-strategy.md` — merge-based sync, cadence, branch model

## Step 2: Read current phase context

Read these files to understand the current work:

- `docs/fork/ipc-phase4_0-plan.md` — current phase plan (modular core refactor)
- `docs/fork/ipc-phase4_0-progress.md` — current phase progress
- `docs/fork/ipc-phase3_8-progress.md` — previous phase (completed)

## Step 3: Read key code files

Read these key files (first 50 lines each is enough for orientation):

- `crates/adapters/core/src/gateway/ipc.rs` — IPC broker (handlers, IpcDb, ACL validation, audit events)
- `crates/adapters/core/src/gateway/agent_registry.rs` — agent registry + health polling
- `crates/adapters/core/src/gateway/chat_db.rs` — chat session SQLite persistence
- `crates/adapters/core/src/gateway/ws.rs` — WS handler with operator isolation (op: prefix folding)
- `crates/adapters/tools/src/agents_ipc.rs` — IPC tools (agents_spawn, send, inbox, reply, state)
- `crates/adapters/security/src/pairing.rs` — token auth, TokenMetadata
- `crates/adapters/security/src/execution.rs` — execution profiles, fail-closed sandbox
- `crates/adapters/security/src/identity.rs` — Ed25519 agent identity
- `crates/adapters/security/src/audit.rs` — dual-chain audit (Merkle + HMAC), IPC event types
- `crates/domain/src/config/schema.rs` — AgentsIpcConfig, IpcPromptGuardConfig, SandboxConfig

## Step 4: Check git state

Run:
```bash
git log --oneline -10
git status
git branch --show-current
git rev-list --count origin/main..upstream/master  # check upstream drift
```

## Step 5: Present summary

Output a concise summary in this format:

```
## Fork Project Context

### Architecture
- Broker-mediated HTTP IPC between agents with trust levels L0-L4
- 5 ACL rules, quarantine lane for L4, promote-to-task workflow
- PromptGuard + LeakDetector + sequence integrity + session limits
- Ed25519 signed messages, fail-closed execution profiles
- Dual-chain audit trail: Merkle (keyless) + HMAC-SHA256 (key-based)
- Web dashboard: Fleet, Audit, Quarantine, Sessions, Spawns, Agent provisioning
- Multi-agent dashboard with WS proxy + per-operator session isolation (Phase 3.8)
- Response cache (two-tier SQLite + hot LRU), session-scoped memory

### Upstream Sync Status
- Last full sync: 2026-03-17 (PRs #119, #120, #121)
- Synced with: upstream v0.4.3 + 18 post-release commits
- Upstream drift: {N} commits behind (from rev-list)
- rerere: all known conflict patterns recorded

### Fork Delta (from delta-registry.md)
- 44+ total entries: 31 fork-only, 12 candidate-upstream, 1 temporary-backport
- 11 categories: IPC Core, Security, Gateway, Agent, Cron, Config, Web UI, Web Infra, Channels, Other, Infra/CI

### Phase Status
- Phase 1 (brokered coordination): DONE — PRs #5-#21
- Phase 2 (broker-side safety): DONE — PRs #26-#34
- Phase 3A/3B (trusted execution): DONE — PRs #35-#55
- Phase 3.5 (control plane UI): DONE
- Phase 3.6 (agent provisioning): DONE
- Phase 3.7/3.7b (chat sessions): DONE
- Phase 3.8 (multi-agent dashboard): DONE — findings fixed in #120
- Phase 4.0 (modular core refactor): IN PROGRESS

### Current branch: {branch}
### Recent commits: {last 3}
### Uncommitted changes: {yes/no}
```

## Step 6: Ask what to do

After presenting context, ask:

> Контекст загружен. Что делаем?

## Arguments

- No args: full context load
- `brief`: skip code reading, just docs + git state
- `code`: skip docs, focus on current code state + git
- `sync`: focus on sync-related context (delta registry, strategy, upstream divergence)
