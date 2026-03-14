# IPC Phase 2: Hardened Security — Progress

Full plan: [`ipc-phase2-plan.md`](ipc-phase2-plan.md)
Phase 1 plan: [`ipc-plan.md`](ipc-plan.md) | Phase 1 progress: [`ipc-progress.md`](ipc-progress.md)
Base branch: `main`
Working branch: feature branch off `main` (e.g. `feat/ipc-phase2-*`)
Execution owner: `Opus`

## Steps Overview

| # | Step | Files | Status | Depends on |
|---|------|-------|--------|------------|
| 1 | Audit trail: IPC event types + AuditLogger wiring | security/audit.rs, gateway/ipc.rs, gateway/mod.rs | TODO | — |
| 2 | PromptGuard: broker payload scanning | gateway/ipc.rs, config/schema.rs, gateway/mod.rs | TODO | 1 |
| 3 | Structured output: trust_warning + quarantine label | gateway/ipc.rs, tools/agents_ipc.rs | TODO | — |
| 4 | Credential leak scanning | security/prompt_guard.rs | TODO | 2 |
| 5 | Replay protection: seq validation on receive | gateway/ipc.rs | TODO | — |
| 6 | Session length limits + auto-escalation | gateway/ipc.rs, config/schema.rs | TODO | — |
| 7 | Promote-to-task: quarantine → working context | gateway/ipc.rs, gateway/mod.rs | TODO | 1, 3 |
| 8 | Synchronous spawn: wait_for_result + timeout | tools/agents_ipc.rs, gateway/ipc.rs | TODO | — |
| 9 | Final validation: fmt + clippy + test + docs | — | TODO | all |

## Session Log

| Date | Session | Steps done | Notes |
|------|---------|------------|-------|
