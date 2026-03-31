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

**Hexagonal architecture** — pure domain core with zero infrastructure dependencies. 12 workspace crates compile in parallel waves. Extend by implementing port traits in adapter crates.

## Architecture

```
synapseclaw/
├── src/
│   ├── main.rs              ← composition root (CLI, DI wiring)
│   └── lib.rs               ← thin facade (re-exports for tests)
│
└── crates/
    ├── domain/              ← PURE DOMAIN (synapse_domain, 24K)
    │   └── src/               zero infra deps: serde, schemars, async-trait
    │       ├── application/   use cases, services
    │       ├── config/        config value objects (schema types, no IO)
    │       ├── domain/        entities, value objects
    │       └── ports/         trait interfaces (Channel, Tool, Provider, Memory,
    │                          IpcClientPort, RuntimeAdapter, etc.)
    │
    └── adapters/
        ├── core/            ← COMPOSITION ROOT (synapse_adapters, 55K)
        │   └── src/
        │       ├── agent/     agent loop, classifier, dispatcher
        │       ├── channels/  re-exports from synapse_channels + orchestration
        │       ├── gateway/   HTTP/WS gateway, IPC broker
        │       ├── tools/     re-exports from synapse_tools + delegate, agents_ipc
        │       ├── daemon/    multi-agent daemon
        │       ├── hooks/     lifecycle hooks
        │       └── ...        pipeline, routing, runtime, storage, tunnel, etc.
        ├── channels/        ← synapse_channels (34K)
        │   └── src/           telegram, discord, slack, matrix, IRC, lark, nostr,
        │                      session management, registry, inbound adapters
        ├── tools/           ← synapse_tools (37K)
        │   └── src/           shell, file_read, browser, memory, cron, MCP wrappers,
        │                      composio, google_workspace, linkedin, http, etc.
        ├── security/        ← synapse_security (10K)
        │   └── src/           pairing, secrets, audit, sandbox, identity
        ├── memory/          ← synapse_memory (8K)
        │   └── src/           sqlite, qdrant, embeddings, markdown, lucid
        ├── providers/       ← synapse_providers (20K)
        │   └── src/           openai, anthropic, gemini, ollama, auth, proxy
        ├── observability/   ← synapse_observability (5K)
        │   └── src/           prometheus, opentelemetry, tracing, runtime_trace
        ├── cron-store/      ← synapse_cron (3K)
        │   └── src/           scheduler, job persistence, commands
        ├── mcp/             ← synapse_mcp (3K)
        │   └── src/           MCP protocol, transport, client, tool wrappers
        ├── infra/           ← synapse_infra (5K)
        │   └── src/           config_io, identity, approval, workspace, runtime
        └── onboard/         ← synapse_onboard (7K)
            └── src/           setup wizard, model management
```

### Compilation Waves (parallel build)

```
Wave 1: domain (24K)
Wave 2: security (10K) | observability (5K) | memory (8K)
Wave 3: providers (20K) | cron (3K)
Wave 4: infra (5K) | mcp (3K)
Wave 5: channels (34K) | tools (37K) | onboard (7K) | core (55K)  ← 4 parallel
Wave 6: synapseclaw binary
```

### Dependency Rules

```
domain/              → nothing (PURE)
adapters/security/   → domain/
adapters/memory/     → domain/
adapters/observability/ → domain/
adapters/mcp/        → domain/
adapters/providers/  → domain/, security/
adapters/cron/       → domain/, security/
adapters/infra/      → domain/, security/, providers/
adapters/channels/   → domain/, security/, providers/, infra/, mcp/, observability/, memory/
adapters/tools/      → domain/, security/, providers/, infra/, mcp/, cron/, memory/
adapters/onboard/    → domain/, infra/, providers/, memory/
adapters/core/       → ALL above crates (composition root)
src/main.rs          → all crates (binary composition root)
```

### Key Extension Points (Domain Ports)

All cross-crate communication goes through domain port traits:

- `crates/domain/src/ports/channel.rs` (`Channel`)
- `crates/domain/src/ports/tool.rs` (`Tool`, `ToolSpec`, `ToolResult`, `ArcToolRef`)
- `crates/domain/src/ports/provider.rs` (`Provider`)
- `crates/domain/src/ports/memory_backend.rs` (`Memory`)
- `crates/domain/src/ports/ipc_client.rs` (`IpcClientPort`)
- `crates/domain/src/ports/runtime.rs` (`RuntimeAdapter`)
- `crates/adapters/observability/src/traits.rs` (`Observer`)

### Workspace Crates

| Crate | Package | LOC | Role |
|-------|---------|-----|------|
| `crates/domain/` | `synapse_domain` | 24K | Pure domain: types, ports, config schema |
| `crates/adapters/core/` | `synapse_adapters` | 55K | Composition root: agent, gateway, daemon, hooks |
| `crates/adapters/channels/` | `synapse_channels` | 34K | Channel implementations (30+ platforms) |
| `crates/adapters/tools/` | `synapse_tools` | 37K | Tool implementations (49 tools) |
| `crates/adapters/security/` | `synapse_security` | 10K | Security: pairing, secrets, audit, sandbox |
| `crates/adapters/memory/` | `synapse_memory` | 8K | Memory: sqlite, qdrant, embeddings, markdown |
| `crates/adapters/providers/` | `synapse_providers` | 20K | LLM providers: openai, anthropic, gemini, ollama |
| `crates/adapters/observability/` | `synapse_observability` | 5K | Prometheus, OpenTelemetry, tracing |
| `crates/adapters/cron-store/` | `synapse_cron` | 3K | Cron scheduler, job persistence |
| `crates/adapters/mcp/` | `synapse_mcp` | 3K | MCP protocol client stack |
| `crates/adapters/infra/` | `synapse_infra` | 5K | Shared infra: config_io, identity, approval |
| `crates/adapters/onboard/` | `synapse_onboard` | 7K | Onboarding wizard, model management |

## Risk Tiers

- **Low risk**: docs/chore/tests-only changes
- **Medium risk**: most `crates/adapters/**` behavior changes without boundary/security impact
- **High risk**: `crates/adapters/security/src/**`, `crates/adapters/core/src/runtime/**`, `crates/adapters/core/src/gateway/**`, `crates/adapters/tools/src/**`, `.github/workflows/**`, access-control boundaries

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
- Do not use `use synapse_X as Y` aliases in extracted crates — always use full crate names.

## Linked References

- `@docs/contributing/change-playbooks.md` — adding providers, channels, tools; security/gateway changes; architecture boundaries
- `@docs/contributing/pr-discipline.md` — privacy rules, superseded-PR attribution/templates, handoff template
- `@docs/contributing/docs-contract.md` — docs system contract, i18n rules, locale parity
