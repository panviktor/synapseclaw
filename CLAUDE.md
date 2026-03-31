# CLAUDE.md — SynapseClaw

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Build with required features:

```bash
cargo build --release --features channel-matrix
```

Full pre-PR validation (recommended):

```bash
./dev/ci.sh all
```

Docs-only changes: run markdown lint and link-integrity checks. If touching bootstrap scripts: `bash -n install.sh`.

## Project Snapshot

SynapseClaw is a Rust-first autonomous agent runtime optimized for performance, efficiency, stability, extensibility, sustainability, and security.

**Hexagonal architecture** — pure domain core with zero infrastructure dependencies. Extend by implementing port traits and registering in adapter modules.

## Architecture

```
synapseclaw/
├── src/
│   ├── main.rs              ← composition root (CLI, DI wiring)
│   └── lib.rs               ← thin facade (re-exports for tests)
│
└── crates/
    ├── domain/              ← PURE DOMAIN (synapse_domain)
    │   └── src/               zero infra deps: serde, schemars, async-trait
    │       ├── application/   use cases, services
    │       ├── config/        config value objects (schema types, no IO)
    │       ├── domain/        entities, value objects
    │       └── ports/         trait interfaces (Provider, Memory, Runtime, etc.)
    │
    └── adapters/
        ├── core/            ← ALL INFRASTRUCTURE (synapse_adapters)
        │   ├── Cargo.toml     main adapters crate (80K LOC, 30+ modules)
        │   └── src/
        │       ├── agent/     agent loop, classifier, dispatcher
        │       ├── channels/  telegram, discord, slack, matrix, IRC, lark, nostr, ...
        │       ├── commands.rs  CLI enums (clap derives)
        │       ├── config_io.rs ConfigIO impl (load/save/encrypt)
        │       ├── gateway/   HTTP/WS gateway, IPC broker
        │       ├── memory_adapters/  LLM-driven memory (consolidation, summary)
        │       ├── tools/     shell, file_read, browser, memory, IPC, ...
        │       └── ...        28+ modules total
        ├── security/        ← synapse_security (10K)
        │   └── src/           pairing, secrets, audit, sandbox, identity
        ├── memory/          ← synapse_memory (8K)
        │   └── src/           sqlite, qdrant, embeddings, markdown, lucid
        ├── observability/   ← synapse_observability (5K)
        ├── providers/       ← synapse_providers (20K)
        │   └── src/           openai, anthropic, gemini, ollama, auth, proxy
        └── cron-store/      ← synapse_cron (3K)
```

### Dependency Rules

```
domain/             → nothing (PURE)
adapters/security/  → domain/
adapters/memory/    → domain/
adapters/providers/ → domain/, security/
adapters/cron/      → domain/, security/
adapters/core/      → domain/, security/, memory/, providers/, cron/, observability/
src/main.rs         → all crates (composition root)
```

### Key Extension Points

- `crates/adapters/core/src/providers/traits.rs` (`Provider`)
- `crates/adapters/core/src/channels/traits.rs` (`Channel`)
- `crates/adapters/core/src/tools/traits.rs` (`Tool`)
- `crates/adapters/memory/src/traits.rs` (`Memory`)
- `crates/adapters/core/src/observability/traits.rs` (`Observer`)
- `crates/domain/src/ports/runtime.rs` (`RuntimeAdapter`)

### Workspace Crates

| Crate | Package | LOC | Role |
|-------|---------|-----|------|
| `crates/domain/` | `synapse_domain` | 24K | Pure domain: types, ports, config schema |
| `crates/adapters/core/` | `synapse_adapters` | 80K | Infrastructure: channels, agent, tools, gateway |
| `crates/adapters/security/` | `synapse_security` | 10K | Security: pairing, secrets, audit, sandbox |
| `crates/adapters/memory/` | `synapse_memory` | 8K | Memory: sqlite, qdrant, embeddings, markdown |
| `crates/adapters/providers/` | `synapse_providers` | 20K | LLM providers: openai, anthropic, gemini, ollama |
| `crates/adapters/observability/` | `synapse_observability` | 5K | Prometheus, OpenTelemetry, tracing |
| `crates/adapters/cron-store/` | `synapse_cron` | 3K | Cron scheduler, job persistence |

## Risk Tiers

- **Low risk**: docs/chore/tests-only changes
- **Medium risk**: most `crates/adapters/**` behavior changes without boundary/security impact
- **High risk**: `crates/adapters/security/src/**`, `crates/adapters/core/src/runtime/**`, `crates/adapters/core/src/gateway/**`, `crates/adapters/core/src/tools/**`, `.github/workflows/**`, access-control boundaries

When uncertain, classify as higher risk.

## Workflow

1. **Read before write** — inspect existing module, factory wiring, and adjacent tests before editing.
2. **One concern per PR** — avoid mixed feature+refactor+infra patches.
3. **Implement minimal patch** — no speculative abstractions, no config keys without a concrete use case.
4. **Validate by risk tier** — docs-only: lightweight checks. Code changes: full relevant checks.
5. **Document impact** — update PR notes for behavior, risk, side effects, and rollback.
6. **Queue hygiene** — stacked PR: declare `Depends on #...`. Replacing old PR: declare `Supersedes #...`.

Branch/commit/PR rules:
- Work from a non-`master` branch. Open a PR to `master`; do not push directly.
- Use conventional commit titles. Prefer small PRs (`size: XS/S/M`).
- Follow `.github/pull_request_template.md` fully.
- Never commit secrets, personal data, or real identity information (see `@docs/contributing/pr-discipline.md`).

## Security Invariants

- **Tool allowlist boundary** (`crates/adapters/core/src/agent/loop_.rs`): When `SYNAPSECLAW_ALLOWED_TOOLS` is set (ephemeral agents), the allowlist filter is a **hard security boundary**. Any new tool injection path must either register tools **before** the filter, or be explicitly suppressed/filtered when the allowlist is active. Current correct order: built-ins → **allowlist filter + delegate filter** → MCP (suppressed if allowlist active). Violating this invariant creates a sandbox escape. See PRs #48-#49 for context.

## Anti-Patterns

- Do not add heavy dependencies for minor convenience.
- Do not silently weaken security policy or access constraints.
- Do not add speculative config/feature flags "just in case".
- Do not mix massive formatting-only changes with functional changes.
- Do not modify unrelated modules "while here".
- Do not bypass failing checks without explicit explanation.
- Do not hide behavior-changing side effects in refactor commits.
- Do not include personal identity or sensitive information in test data, examples, docs, or commits.
- Do not add tool injection paths after the `SYNAPSECLAW_ALLOWED_TOOLS` filter without explicit allowlist enforcement (see Security Invariants).

## Linked References

- `@docs/contributing/change-playbooks.md` — adding providers, channels, tools; security/gateway changes; architecture boundaries
- `@docs/contributing/pr-discipline.md` — privacy rules, superseded-PR attribution/templates, handoff template
- `@docs/contributing/docs-contract.md` — docs system contract, i18n rules, locale parity
