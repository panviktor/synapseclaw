# Phase 4.1H2 — Hexagonal Migration Completion

## Context

Phase 4.1H (PRs #176-#180) moved 26 adapter modules to `src/fork_adapters/` and extracted domain types to `crates/fork_core/` (16.3K LOC, 397 tests, 21 domain modules, 24 ports). But the **main architectural goal was not met**: fork_adapters is still a module inside `src/`, not a workspace crate. Any change to fork_adapters recompiles the entire 225K LOC binary (~7 min). The blocker: 414 refs to `crate::config::Config` from fork_adapters.

Phase 4.1H2 completes the migration by:
1. Creating `crates/fork_config/` to hold Config types (breaking the circular dependency)
2. Moving remaining deferred modules into fork_adapters
3. Promoting `src/fork_adapters/` → `crates/fork_adapters/` as a workspace crate
4. Updating all documentation

---

## Architecture: three-crate split → four-crate split

```
crates/fork_core/       ← domain + ports + services (16K, rarely changes)
crates/fork_config/     ← NEW: Config struct + all sub-config types (12K, rarely changes)
crates/fork_adapters/   ← NEW: all adapters (152K, compiles independently)
src/                    ← wiring only (~25K: agent, config loader, main)
```

Dependency graph:
```
fork_config  →  fork_core (for AutonomyLevel, QueryClassificationConfig)
fork_adapters  →  fork_config + fork_core
synapseclaw  →  all three
```

---

## Slices

### Slice 1 — Dead code removal (nodes, rag)
Delete `src/nodes/` (238 LOC) and `src/rag/` (395 LOC) — both unused.
- Files: `src/nodes/`, `src/rag/`, `src/lib.rs`
- Risk: **low** | LOC: −633

### Slice 2 — Scaffold fork_config + move adapter-owned config types
Create `crates/fork_config/Cargo.toml`. Move into it the config types that live in adapter modules but are referenced by Config struct:
- `EmailConfig` (from channels/email_channel.rs)
- `ClawdTalkConfig` (from channels/clawdtalk.rs)
- `BrowserDelegateConfig` (from tools/browser_delegate.rs)
- `DomainMatcher` (from security/domain_matcher.rs, 259 LOC)
- `is_glm_alias`/`is_zai_alias` (from providers/mod.rs)
- Files: new `crates/fork_config/`, adapter sources become re-exports
- Risk: **medium** | LOC: +400 scaffold

### Slice 3 — Move config schema types to fork_config
Move all 123 struct/enum type definitions from `crates/domain/src/config/schema.rs` → `crates/fork_config/src/schema.rs`. What stays in `src/config/`:
- `Config::load()`, `Config::save()` (use SecretStore — infra)
- `config/workspace.rs` (filesystem ops)
- `config/traits.rs` (ChannelConfig trait)
- `crates/domain/src/config/schema.rs` becomes `pub use fork_config::schema::*;` + SecretStore methods

All existing `crate::config::XxxConfig` paths keep working via re-export.
- Risk: **high** | LOC: net 0 (12K moved)
- Depends on: Slice 2

### Slice 4 — Move multimodal.rs → fork_adapters
Move `src/multimodal.rs` (659 LOC) → `src/fork_adapters/multimodal/`. Update 15 import paths. `src/lib.rs` re-exports.
- Risk: **low** | LOC: net 0

### Slice 5 — Move identity.rs → fork_adapters
Move `src/identity.rs` (1,488 LOC) → `src/fork_adapters/identity.rs`. Only 2 consumers.
- Risk: **low** | LOC: net 0

### Slice 6 — Move skills/ → fork_adapters
Move `src/skills/` (2,577 LOC) → `src/fork_adapters/skills/`. ~18 consumer refs.
- Risk: **low** | LOC: net 0

### Slice 7 — Move sop/ → fork_adapters
Move `src/sop/` (6,615 LOC) → `src/fork_adapters/sop/`. Investigate `SopConfig` (may be behind dead feature gate). 4 refs from mqtt channel.
- Risk: **medium** (SopConfig resolution needed)
- Depends on: Slice 3 (config types resolved)

### Slice 8 — Move runtime/ impls → fork_adapters
Move `crates/adapters/core/src/runtime/native.rs`, `docker.rs` → `src/fork_adapters/runtime/`. Traits already in fork_core. `crates/adapters/core/src/runtime/` becomes thin re-export.
- Risk: **medium** (runtime is high-risk area per CLAUDE.md, but pure move)
- LOC: net 0

### Slice 9 — Promote fork_adapters to workspace crate
The main goal. Three sub-slices:

**9a** — Define agent-related ports in fork_core:
- `AgentRunnerPort`: `async fn run(config, message) -> Result<String>`
- `ToolCallRunnerPort`: for run_tool_call_loop
- fork_adapters modules (gateway, daemon, channels, cron, tools/delegate) receive `Arc<dyn AgentRunnerPort>` instead of calling `crate::agent::*` directly

**9b** — Refactor fork_adapters refs (while still in src/):
- Replace `crate::config::*` → `fork_config::*` (414 refs, ~92 files)
- Replace `crate::agent::*` → port calls (24 refs)
- Replace `crate::security::*` → `fork_core::*` where possible, port for rest (5 refs)
- Replace `crate::util::*` → `fork_core::domain::util::*` (13 refs)

**9c** — Physical promotion:
- Create `crates/fork_adapters/Cargo.toml`
- `mv src/fork_adapters/* crates/fork_adapters/src/`
- Add to workspace members
- Add as dependency of synapseclaw
- Replace `crate::fork_adapters::` → `fork_adapters::` everywhere
- `src/lib.rs`: `pub use fork_adapters;`

- Risk: **high** | LOC: +500 (ports, Cargo.toml)
- Depends on: all prior slices

### Slice 10 — Documentation
Update all stale docs in one PR:
- `CLAUDE.md` — fix repo map (fork_config, corrected trait paths)
- `docs/SUMMARY.md` — refresh (38 days stale)
- `docs/fork/README.md` — document four-crate architecture
- `docs/fork/ipc-phase4_1h2-plan.md` — create (this plan)
- `docs/fork/ipc-phase4_1h2-progress.md` — create tracker
- `docs/fork/news.md` — Phase 4.1H2 entries
- `docs/fork/delta-registry.md` — update crate structure
- Risk: **low** | LOC: +300

---

## Dependency Graph

```
Slice 1 (dead code)          ──────────────┐
Slice 2 (fork_config scaffold) ──┐         │
Slice 4 (multimodal)         ────┤         │
Slice 5 (identity)           ────┤         │
Slice 6 (skills)             ────┤         │
Slice 8 (runtime)            ────┤         │
                                 ↓         │
Slice 3 (config → fork_config) ─┤         │
                                 ↓         │
Slice 7 (sop)                ────┤         │
                                 ↓         ↓
Slice 9a (agent ports)       ────┤
Slice 9b (ref replacement)   ────┤
Slice 9c (physical move)     ────┤
                                 ↓
Slice 10 (docs)              ────┘
```

Slices 1, 2, 4, 5, 6, 8 are independent — can parallelize.

---

## Critical Files

| File | Role | Slice |
|------|------|-------|
| `crates/domain/src/config/schema.rs` (12K) | Config types — splits into fork_config | 3 |
| `src/fork_adapters/mod.rs` | Adapter registry → becomes crate lib.rs | 9c |
| `crates/fork_core/src/ports/` | New agent ports | 9a |
| `src/lib.rs` | Module wiring, updated every slice | all |
| `Cargo.toml` | Workspace members + deps | 2, 9c |

---

## Verification

Per slice:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Final (after Slice 9c):
```bash
cargo build                    # 3-crate workspace builds
cargo test                     # all tests pass
cargo test -p fork_core        # 397+ tests
cargo test -p fork_config      # config type tests
cargo test -p fork_adapters    # adapter tests compile independently
```

Deploy + fleet health check (6 services), `/content` pipeline test, normal chat test.

---

## Summary

| Slice | Description | Risk | Est. PRs |
|-------|-------------|------|----------|
| 1 | Dead code (nodes, rag) | Low | 1 |
| 2 | fork_config scaffold | Medium | 1 |
| 3 | Config schema → fork_config | High | 1-2 |
| 4 | multimodal → fork_adapters | Low | 1 |
| 5 | identity → fork_adapters | Low | 1 |
| 6 | skills → fork_adapters | Low | 1 |
| 7 | sop → fork_adapters | Medium | 1 |
| 8 | runtime → fork_adapters | Medium | 1 |
| 9 | Crate promotion (3 sub-slices) | High | 2-3 |
| 10 | Documentation | Low | 1 |
| **Total** | | | **11-13** |
