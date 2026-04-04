# Fork Documentation

This directory contains the strategy, plans, and operational docs for the SynapseClaw fork.

SynapseClaw is an independent project (originally forked from ZeroClaw) with its own **multi-agent IPC system** (broker, trust model, quarantine, control plane), **web-based operator dashboard**, **security hardening** (execution profiles, Ed25519 identity, PromptGuard), and **hexagonal core** (fork_core workspace crate). See [`delta-registry.md`](delta-registry.md) for the full delta inventory.

## News & Changelog

See [`news.md`](news.md) for the latest updates and release notes.

## Documents

### Project History & Delta Registry

| Document | Purpose | Who reads it |
|----------|---------|-------------|
| [delta-registry.md](delta-registry.md) | All project deltas (IPC, security, gateway, web UI, core, infra) | Everyone |
| [sync-strategy.md](sync-strategy.md) | ARCHIVED — historical upstream sync strategy (discontinued) | Reference only |
| [sync-review-rubric.md](sync-review-rubric.md) | ARCHIVED — historical sync PR review policy | Reference only |

### IPC Plans & Progress

| Document | Purpose | Who reads it |
|----------|---------|-------------|
| [ipc-plan.md](ipc-plan.md) | Full IPC design: trust model, ACL, quarantine, approvals, phases | Everyone |
| [ipc-progress.md](ipc-progress.md) | Phase 1 execution checklist (11 steps — DONE) | Opus |
| [ipc-phase2-plan.md](ipc-phase2-plan.md) | Phase 2: Hardened Security — PromptGuard, audit, replay, session limits | Everyone |
| [ipc-phase2-progress.md](ipc-phase2-progress.md) | Phase 2 execution checklist (8 steps — DONE) | Opus |
| [ipc-quickstart.md](ipc-quickstart.md) | Minimal configs, pairing flow, smoke-test curl commands | Everyone |
| [ipc-phase3-plan.md](ipc-phase3-plan.md) | Phase 3: Trusted Execution — ephemeral agents, subprocess isolation, crypto provenance | Everyone |
| [ipc-phase3-progress.md](ipc-phase3-progress.md) | Phase 3A/3B execution checklist (all steps — DONE) | Opus |
| [ipc-phase3_5-plan.md](ipc-phase3_5-plan.md) | Phase 3.5: Human Control Plane — IPC operator UI (6 screens, 10 steps) | Everyone |
| [ipc-phase3_6-plan.md](ipc-phase3_6-plan.md) | Phase 3.6: Agent Provisioning — presets, config gen, pairing flow | Everyone |
| [ipc-phase3_7-plan.md](ipc-phase3_7-plan.md) | Phase 3.7: Chat Sessions — WS RPC, multi-session, SQLite persistence | Everyone |
| [ipc-phase3_7b-plan.md](ipc-phase3_7b-plan.md) | Phase 3.7b: Session Intelligence — rolling summaries, live tool events | Everyone |
| [ipc-phase3_8-plan.md](ipc-phase3_8-plan.md) | Phase 3.8: Multi-Agent Dashboard — one frontend shell, broker mode + local agent mode, shared agent workbench | Everyone |
| [ipc-phase3_8-progress.md](ipc-phase3_8-progress.md) | Phase 3.8 execution checklist (all steps — DONE) | Opus |
| [ipc-phase3_9-plan.md](ipc-phase3_9-plan.md) | Phase 3.9: Operator Control Plane — broker-global activity feed and fleet cron on top of the shared workbench | Everyone |
| [ipc-phase3_9-progress.md](ipc-phase3_9-progress.md) | Phase 3.9 execution checklist (Steps 1-6 done, Step 3 deferred) | Opus |
| [ipc-phase3_10-plan.md](ipc-phase3_10-plan.md) | Phase 3.10: Push Loop Prevention — kind-based filtering, per-peer counter, one-way dispatch mode | Everyone |
| [ipc-phase3_10-progress.md](ipc-phase3_10-progress.md) | Phase 3.10 execution checklist | Opus |
| [ipc-phase3_11-plan.md](ipc-phase3_11-plan.md) | Phase 3.11: Multi-Blueprint Fleet Topology — hierarchical fleet, blueprint drill-down, aggregated cross-blueprint traffic | Everyone |
| [ipc-phase3_11-progress.md](ipc-phase3_11-progress.md) | Phase 3.11 execution checklist (all steps — DONE) | Opus |
| [ipc-phase3_12-plan.md](ipc-phase3_12-plan.md) | Phase 3.12: Channel Session Intelligence — rolling summary, thread seeding, channel sessions in web UI | Everyone |
| [ipc-phase3_12-progress.md](ipc-phase3_12-progress.md) | Phase 3.12 execution checklist | Opus |
| [ipc-phase4_0-plan.md](ipc-phase4_0-plan.md) | Phase 4.0: Modular Core Refactor — capability-driven channels, conversation store | Everyone |
| [ipc-phase4_0-progress.md](ipc-phase4_0-progress.md) | Phase 4.0 execution checklist | Opus |
| [ipc-phase4_3-plan.md](ipc-phase4_3-plan.md) | Phase 4.3: Memory Architecture — SurrealDB embedded, knowledge graph, skill learning, MemGPT pattern | Everyone |
| [ipc-phase4_3-progress.md](ipc-phase4_3-progress.md) | Phase 4.3 execution checklist (COMPLETE — all 9 slices, PRs #217-#218) | Opus |
| [memory-architecture.md](memory-architecture.md) | Memory system data flows, SurrealDB schema, embedding pipeline, tools | Everyone |
| [ipc-phase4_4-plan.md](ipc-phase4_4-plan.md) | Phase 4.4: Prompt Optimizer — self-improving agent instructions (DONE, PR #226) | Everyone |
| [ipc-phase4_4-progress.md](ipc-phase4_4-progress.md) | Phase 4.4 execution checklist (DONE) | Opus |
| [ipc-phase4_5-plan.md](ipc-phase4_5-plan.md) | Phase 4.5: Pipeline Hardening — message filtering, DLQ, visualization | Everyone |
| [ipc-phase4_5-progress.md](ipc-phase4_5-progress.md) | Phase 4.5 execution checklist (4 slices — NOT STARTED) | Opus |
| [ipc-phase4_6-plan.md](ipc-phase4_6-plan.md) | Phase 4.6: Agent Product Intelligence — current-conversation targets, orchestration tools, standing orders, planner guardrails | Everyone |
| [channel-triage.md](channel-triage.md) | Channel port priority: 10 Tier 1 (port) + 17 Tier 2 (defer) | Everyone |

### Memory / UX Plans

| Document | Purpose | Who reads it |
|----------|---------|-------------|
| [memory-learning-foundation-plan.md](memory-learning-foundation-plan.md) | Backend learning contract before UI: mutation semantics, forgetting/retention, hot-path capture, namespaces, events | Everyone |
| [memory-unification-plan.md](memory-unification-plan.md) | Unify prompt assembly, post-turn learning, session scoping, and memory budgets across web + channels | Everyone |
| [multi-agent-memory-ui-plan.md](multi-agent-memory-ui-plan.md) | Multi-agent workbench UX: Agent Rail, Memory Pulse, fleet constellation, presets, motion | Everyone |
| [ipc-phase4_6-research.md](ipc-phase4_6-research.md) | Research companion: Letta/Hermes/Mem0/OpenClaw competitive analysis, root cause diagnosis, architecture mapping | Everyone |

## Reading order

**New to the fork?** Start with `delta-registry.md` → `sync-strategy.md` → `ipc-plan.md`.

**Starting IPC work?** Read the phase plans in order:
`ipc-phase2-plan.md` → `ipc-phase3-plan.md` → `ipc-phase3_5-plan.md` → `ipc-phase3_6-plan.md` → `ipc-phase3_7-plan.md` → `ipc-phase3_7b-plan.md` → `ipc-phase3_8-plan.md` → `ipc-phase3_9-plan.md` → `ipc-phase3_10-plan.md` → `ipc-phase3_11-plan.md` → `ipc-phase3_12-plan.md` → `ipc-phase4_0-plan.md` → `ipc-phase4_3-plan.md` → `ipc-phase4_5-plan.md` → `ipc-phase4_6-plan.md` → `ipc-phase4_6-research.md`.

**Starting memory/self-learning work?** Read:
`memory-learning-foundation-plan.md` → `memory-unification-plan.md` → `multi-agent-memory-ui-plan.md`.

**Setting up IPC locally?** Follow `ipc-quickstart.md` — configs, pairing, smoke tests.

**Reviewing a sync PR?** Open `sync-review-rubric.md` and `delta-registry.md`.

## Branch model

| Branch | Role |
|--------|------|
| `main` | Default branch |
| `feat/*` | Feature work, branched from `main` |

## Related

- [CLAUDE.md](../../CLAUDE.md) — project-wide coding conventions
- [docs/contributing/](../contributing/) — PR discipline, change playbooks
