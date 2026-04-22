# Documentation Discrepancies: Fork Docs vs Current Code

This document tracks known discrepancies between the fork documentation (this directory) and the actual current codebase. It is created as a working artifact during the documentation refresh of April 2026.

## Crate Naming

| Fork docs say | Code actually |
|---|---|
| `fork_core` | `synapse_domain` |
| `fork_security` | `synapse_security` |
| `fork_adapters` | `synapse_adapters` (in `crates/adapters/core/`) |
| `fork_config` | Not a separate crate — config types live in `synapse_domain` |
| `crates/fork_core/` | `crates/domain/` |
| `crates/fork_security/` | `crates/adapters/security/` |
| `src/fork_adapters/` | No longer exists — all adapters are in `crates/adapters/` |

**Impact**: The delta-registry.md still references `fork_core` and `fork_adapters` paths extensively. The `CORE-*` entries in the delta registry are especially affected — they describe files in `src/fork_core/` and `src/fork_adapters/` which no longer exist.

## Architecture Diagram in README

The root README.md architecture block still shows:
```
fork_core (workspace crate)    fork_adapters (main crate)
├── domain/                    ├── channels/registry
...
```

The actual structure is:
```
crates/domain/                 crates/adapters/
  synapse_domain                 core/ (synapse_adapters)
                                   channels/ (synapse_channels)
                                   tools/ (synapse_tools)
                                   security/ (synapse_security)
                                   memory/ (synapse_memory)
                                   providers/ (synapse_providers)
                                   ...
```

## Delta Registry Staleness

`delta-registry.md` references paths that no longer exist:
- `src/fork_core/*` → moved to `crates/domain/`
- `src/fork_adapters/*` → moved to `crates/adapters/`
- `crates/fork_core/` → `crates/domain/`
- `src/fork_adapters/pipeline/` → `crates/adapters/core/src/pipeline/`
- `src/fork_adapters/middleware/` → `crates/adapters/core/src/middleware/`

Many deltas marked as `fork-only` or `candidate-upstream` reference old file paths.
The table entries need path updates but the *substance* of the deltas is still valid.

## IPC Quickstart

The `ipc-quickstart.md` is largely accurate for the IPC protocol/API but:
- References `POST /admin/paircode/new` — actual endpoint path should be verified
- References `GET /api/ipc/agents` — actual endpoint should be verified
- The spawn workflow with `workload` profiles is described but implementation status unclear

## Phase Progress Docs

Phase progress documents (`ipc-phase*-progress.md`) are largely accurate but some reference:
- `fork_core` tests count — now `synapse_domain` tests
- Old LOC counts — may differ after crate extraction

## News / Changelog

`news.md` entries reference PRs with `fork_core` and `fork_adapters` in descriptions.
The substance is correct but crate names are outdated.

## Install / Setup Docs

The root README.md and setup docs are mostly current but:
- Some references to `fork_core`/`fork_adapters` in architecture diagrams
- Homebrew install (`brew install synapseclaw`) — verify this is actually published
- Some CLI command references may be outdated

## Security Isolation (PR #242)

The news entry about memory isolation (PR #242) describes port trait changes
requiring `agent_id`. This is implemented but not reflected in reference docs
(`channels-reference.md`, `config-reference.md`, etc.).

## Web UI Delta References

Delta registry references `web/src/pages/ipc/*` and `web/src/components/ipc/*` —
these paths should be verified against the actual web directory structure.

---

*Last updated: April 12, 2026. This document should be pruned as discrepancies are resolved.*
