# Phase 4.1H Progress: Hexagonal Architecture Migration

See [ipc-phase4_1h-plan.md](ipc-phase4_1h-plan.md) for full plan.

## Slice Status

| Slice | Description | Status | PR | LOC moved |
|-------|-------------|--------|-----|-----------|
| 0 | Audit + dead code removal | **done** | #176 | −15K |
| 1 | Move adapters to `src/fork_adapters/` | **done** | #177 | ~122K moved |
| 2 | Observability + hooks + cron | **done** | #177 | (included in Slice 1) |
| 3 | Service infrastructure | **done** | #177 | (included in Slice 1) |
| 4 | Providers | **done** | #177 | (included in Slice 1) |
| 5 | Tools | **done** | #177 | (included in Slice 1) |
| 6 | Channels | **done** | #177 | (included in Slice 1) |
| 7 | Gateway | **done** | #177 | (included in Slice 1) |
| 8 | Security domain → fork_core | **done** | #178 | ~60 lines |
| 9 | Memory split → fork_core | **done** | #178 | ~130 lines |
| 10 | Agent orchestration → fork_core | **done** | #179 | ~490 lines |
| 11 | Config types → fork_core | **done** | #179 | (QueryClassificationConfig included in Slice 10) |
| 12 | Promote `src/fork_adapters/` → `crates/fork_adapters/` | **deferred** | — | blocked: 55+ `crate::security`, 46+ `crate::config` refs |
| 13 | Documentation update | **done** | #180 | — |

## Notes

- `channel-matrix` is now a default feature (Slice 0).
- Skills/SOP/RAG deferred to Phase 4.2.
- Phase 4.1H+ (granular adapter crates) deferred until compilation times warrant it.
- Crate promotion (Slice 12) blocked: fork_adapters has 55+ `crate::security` and 46+ `crate::config` refs — requires extracting full config/security/memory/runtime to fork_core first. Deferred to Phase 4.2.
- Slices 1-7 collapsed into single PR #177 — all adapter modules now live in `src/fork_adapters/`.
