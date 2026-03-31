# IPC Phase 4.1H: Hexagonal Architecture Migration

Phase 4.0: modular core refactor | Phase 4.1: deterministic pipeline engine | **Phase 4.1H: hexagonal migration** | Phase 4.2: federated execution

---

## What Phase 4.1H gives

Five promises to the fork:

1. **Clean architectural layers** — every module lives in its architectural home: domain in fork_core, adapters in fork_adapters, composition root in src/.
2. **Dead code elimination** — unused upstream modules removed, reducing attack surface and build time.
3. **Discoverable structure** — new developer opens `src/fork_adapters/` and immediately sees all external integrations; opens `crates/fork_core/` and sees all business rules.
4. **Testable boundaries** — adapters can be mocked via ports; domain logic tested without external dependencies.
5. **Foundation for Phase 4.2** — federated execution requires clean adapter boundaries for remote dispatch.

---

## Why Phase 4.1H exists

Phase 4.0 created `crates/fork_core/` with domain types, ports, and application services. Phase 4.1 added pipeline engine. But the hexagonal migration stalled at ~15% — 253K LOC still lives in flat `src/` with 33 modules mixing domain, adapters, and infrastructure:

- `crates/adapters/tools/src/` (41K LOC) = adapter, but lives outside fork_adapters
- `crates/adapters/channels/src/` (40K LOC) = adapter, mixed with domain
- `crates/adapters/providers/src/` (23K LOC) = adapter, flat in src/
- `crates/adapters/security/src/` (12K LOC) = domain rules + crypto adapters, unsplit
- `crates/adapters/core/src/agent/` (11K LOC) = orchestration logic tightly coupled to upstream types

Can't tell what's domain vs infrastructure without reading every file.

---

## Design principles

### Three workspace crates = fast incremental builds

Original motivation for fork_core split: **compilation speed**. Right now fork_adapters lives inside `src/` — any adapter change recompiles the entire 253K LOC binary. Fix:

```
crates/fork_core/       ← domain (changes rarely)    → compiles once
crates/fork_adapters/   ← adapters (changes sometimes) → compiles independently
src/                    ← wiring only (tiny)          → recompiles in seconds
```

This means `fork_adapters` becomes a **separate workspace crate** in `crates/fork_adapters/`, not a module inside `src/`.

### Move, don't rewrite

Each slice moves existing code into the correct crate. No logic changes, no refactoring within modules. If it compiles and tests pass — the slice is done.

### One PR per slice

Each slice is a single, reviewable PR. Codebase compiles and all tests pass at every step.

### Adapters first, domain later

Moving adapters to `crates/fork_adapters/` is safe (no trait changes). Extracting domain into `fork_core/` may require new port definitions. Do adapters first.

### Docs updated AFTER all slices complete

`docs/fork/news.md`, `docs/fork/delta-registry.md`, `CLAUDE.md`, `docs/SUMMARY.md` — updated in a single final documentation PR after all migration slices land.

---

## Inputs from Phase 4.1

| Phase 4.1 artifact | Phase 4.1H usage |
|---------------------|-----------------|
| `fork_adapters/pipeline/` | Pattern for adapter directory structure |
| `fork_adapters/routing/` | Pattern for domain→adapter split |
| `fork_adapters/middleware/` | Pattern for port implementations |
| Shared IpcClient (daemon) | Composition root pattern for dependency injection |
| Hardware removal (#175) | Proof that large deletions are safe with proper unwiring |

---

## Non-goals

1. No new ports or domain types (unless strictly needed for a move).
2. No logic changes inside moved modules.
3. No refactoring of internal module structure during the move.
4. No Rust edition or dependency changes.
5. No changes to runtime behavior — before/after must be identical.
6. No skill porting from other programs — separate phase after code finalization.

---

## Build simplification

`channel-matrix` becomes a **default feature** — Matrix is always built. This removes `--features channel-matrix` from all build/test/clippy commands across CI, docs, and CLAUDE.md.

```toml
# Cargo.toml
[features]
default = ["channel-matrix"]
```

All verification steps use plain `cargo build` / `cargo test` / `cargo clippy`.

---

## New dependencies

None. This is purely organizational.

---

## Slices

### Slice 0 — Audit: find and remove dead code

**Scope:** Verify every `src/` module is actually imported and used. Remove dead modules.

**Suspect modules:**
- `src/hands/` (574 LOC) — hardware gesture recognition
- `src/nodes/` (238 LOC) — node abstractions, possibly unused
- `src/skillforge/` (1.1K LOC) — skill building, possibly unused
- `src/identity/` — unknown size
- `src/migration/` — unknown size
- `src/multimodal/` — check usage
- Unused individual files in `crates/adapters/tools/src/` (64 files — are all 64 registered?)

**Output:** dead code deleted, category map documented.

### Slice 1 — Create `crates/fork_adapters/` as workspace crate + move existing adapters

**Critical first step:** Promote `src/fork_adapters/` to `crates/fork_adapters/` as a proper workspace crate with its own `Cargo.toml`.

1. Create `crates/fork_adapters/Cargo.toml` — depends on `fork_core`
2. Move `src/fork_adapters/*` → `crates/fork_adapters/src/`
3. Add `fork_adapters` to workspace `Cargo.toml` members
4. Add `fork_adapters` as dependency of main `synapseclaw` crate
5. Update all `crate::fork_adapters::` imports to `fork_adapters::`
6. Move first batch of small adapters:
   - `src/auth/` (2.6K) → `crates/fork_adapters/src/auth/`
   - `src/cost/` (737) → `crates/fork_adapters/src/cost/`
   - `src/tunnel/` (1.5K) → `crates/fork_adapters/src/tunnel/`
   - `src/heartbeat/` (1.2K) → `crates/fork_adapters/src/heartbeat/`
   - `src/health/` (184) → `crates/fork_adapters/src/health/`
   - `src/integrations/` (1.3K) → `crates/fork_adapters/src/integrations/`

**Total:** ~3.7K existing + ~7.5K new = ~11K LOC in crate.

**Note:** This is the hardest slice — it changes the dependency graph. All subsequent slices are just `mv` + update imports.

### Slice 2 — Observability + hooks + cron

**Move:**
- `crates/adapters/observability/src/` (3.0K) → `crates/fork_adapters/crates/adapters/observability/src/`
- `src/hooks/` (633) → `crates/fork_adapters/src/hooks/`
- `src/cron/` (3.4K) → `crates/fork_adapters/src/cron/`
- `src/approval/` (618) → `crates/fork_adapters/src/approval/`

**Total:** ~7.7K LOC moved.

### Slice 3 — Service infrastructure

**Move:**
- `src/onboard/` (7.1K) → `crates/fork_adapters/src/onboard/`
- `src/service/` (1.5K) → `crates/fork_adapters/src/service/`
- `src/doctor/` (1.3K) → `crates/fork_adapters/src/doctor/`
- `src/daemon/` (936) → `crates/fork_adapters/src/daemon/`

**Total:** ~10.9K LOC moved.

### Slice 4 — Providers

**Move:**
- `crates/adapters/providers/src/` (23.5K) → `crates/fork_adapters/crates/adapters/providers/src/`

**Note:** Largest single adapter module. Pure LLM provider implementations (OpenAI, Anthropic, custom). No domain logic.

### Slice 5 — Tools

**Move:**
- `crates/adapters/tools/src/` (41K) → `crates/fork_adapters/crates/adapters/tools/src/`

**Note:** Largest module. 64 files, each a tool adapter. Tool trait defined in `fork_core`.

### Slice 6 — Channels

**Move:**
- `crates/adapters/channels/src/` (40K) → `crates/fork_adapters/crates/adapters/channels/src/` (merge with existing `fork_adapters/channels/`)

**Note:** Contains channel trait impls (Telegram, Discord, Slack, Matrix) + orchestration glue. May need split in Phase D but moved as-is for now.

### Slice 7 — Gateway

**Move:**
- `crates/adapters/core/src/gateway/` (18K) → `crates/fork_adapters/crates/adapters/core/src/gateway/`

**Note:** HTTP/WebSocket server. Pure infrastructure.

### Slice 8 — Security split

**Split:**
- Policy rules (allowlist, ACL evaluation) → `fork_core/src/domain/security/`
- Crypto implementations (pairing, identity, Ed25519) → `crates/fork_adapters/crates/adapters/security/src/`

**Note:** First slice that adds to fork_core. Requires careful boundary definition.

### Slice 9 — Memory split

**Split:**
- Memory model/traits → `fork_core/src/domain/memory/` (extend existing)
- SQLite/markdown/embedding backends → `crates/fork_adapters/crates/adapters/memory/src/` (extend existing)

### Slice 10 — Agent orchestration

**Split:**
- Core orchestration logic (turn execution, tool filtering) → `fork_core/src/application/services/`
- Message handling, context building → stays in src/ (composition)

**Note:** Most complex slice — agent loop is the central hub.

### Slice 11 — Config

**Split:**
- Domain config types → `fork_core/src/domain/config/` (extend existing)
- Schema parsing, TOML loading, merging → stays in src/ (composition root)

### Slice 12 — Documentation update

**Update ALL docs in one PR:**
- `docs/fork/news.md` — Phase 4.1H complete entry
- `docs/fork/delta-registry.md` — new delta count
- `CLAUDE.md` — updated repo map reflecting new structure
- `docs/SUMMARY.md` — if needed
- `docs/fork/README.md` — architecture description

---

## Target structure after all slices

```
Cargo.toml                      ← workspace: [fork_core, fork_adapters, synapseclaw]

crates/fork_core/src/           ← WHAT (domain + ports + application) — changes rarely
├── domain/
│   ├── approval.rs
│   ├── channel.rs
│   ├── config.rs               (extended: full config domain types)
│   ├── conversation.rs
│   ├── implementation.rs
│   ├── ipc.rs
│   ├── memory.rs               (extended: memory model)
│   ├── message.rs
│   ├── pipeline*.rs
│   ├── routing.rs
│   ├── run.rs
│   ├── security/               (NEW: security policies)
│   ├── spawn.rs
│   └── tool_middleware.rs
├── ports/                      (21 existing + maybe 2-3 new)
├── application/
│   ├── services/               (8 existing + agent orchestrator)
│   └── use_cases/              (12 existing)
└── bus.rs

crates/fork_adapters/src/       ← HOW (all adapters) — compiles independently
├── approval/
├── auth/
├── channels/                   (Telegram, Discord, Slack, Matrix + registry)
├── cost/
├── cron/
├── daemon/
├── doctor/
├── gateway/                    (HTTP, WebSocket, IPC endpoints)
├── health/
├── heartbeat/
├── hooks/
├── inbound/
├── integrations/
├── ipc/
├── memory/                     (SQLite, markdown, embedding backends)
├── middleware/
├── observability/
├── onboard/
├── pipeline/
├── providers/                  (OpenAI, Anthropic, custom LLMs)
├── rag/
├── routing/
├── runtime/
├── security/                   (pairing, crypto, Ed25519)
├── service/
├── storage/
├── tools/                      (shell, file, web, browser, etc.)
└── tunnel/

src/                            ← WIRING + deferred modules (recompiles fast)
├── config/                     (schema + loading)
├── skills/                     (deferred to next phase)
├── sop/                        (deferred to next phase)
├── rag/                        (deferred to next phase)
├── main.rs                     (CLI entrypoint + command routing)
└── lib.rs                      (re-exports fork_core + fork_adapters)
```

### Compilation benefit (Phase 4.1H)

| What changed | What recompiles | Time |
|-------------|-----------------|------|
| fork_core domain type | fork_core → fork_adapters → src/ | ~2-4 min |
| fork_adapters adapter | fork_adapters → src/ | ~1-2 min |
| src/ wiring only | src/ only | ~10-20 sec |
| TOML config/pipeline | nothing (runtime) | 0 sec |

### Future: granular adapter crates (Phase 4.1H+)

After 4.1H lands, large adapters can be split into independent crates for parallel compilation:

```
crates/fork_adapters_channels/  (40K — Telegram, Matrix, Discord)
crates/fork_adapters_tools/     (41K — 64 tool adapters)
crates/fork_adapters_gateway/   (18K — HTTP, WS, IPC endpoints)
crates/fork_adapters_providers/ (23K — LLM providers)
crates/fork_adapters/           (remaining small adapters)
```

Each compiles in parallel. Changing one tool doesn't recompile channels/gateway/providers. Deferred because: one crate is simpler to manage during migration, split later when compilation times actually become a bottleneck.

---

## Verification (per slice)

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`
4. `cargo build`

## Verification (final)

5. Deploy + fleet health check (6 services active)
6. `/content` pipeline test from Matrix
7. Normal chat test
