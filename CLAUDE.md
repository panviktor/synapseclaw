# CLAUDE.md — SynapseClaw

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Full pre-PR validation (recommended):

```bash
./dev/ci.sh all
```

Docs-only changes: run markdown lint and link-integrity checks. If touching bootstrap scripts: `bash -n install.sh`.

## Project Snapshot

SynapseClaw is a Rust-first autonomous agent runtime optimized for performance, efficiency, stability, extensibility, sustainability, and security.

Core architecture is trait-driven and modular. Extend by implementing traits and registering in factory modules.

Key extension points:

- `crates/infra/adapters/src/providers/traits.rs` (`Provider`)
- `crates/infra/adapters/src/channels/traits.rs` (`Channel`)
- `crates/infra/adapters/src/tools/traits.rs` (`Tool`)
- `crates/infra/memory/src/traits.rs` (`Memory`)
- `crates/infra/adapters/src/observability/traits.rs` (`Observer`)
- `crates/domain/src/ports/runtime.rs` (`RuntimeAdapter`)

## Repository Map

- `src/main.rs` — CLI entrypoint and command routing
- `src/lib.rs` — module exports and shared command enums
- `crates/domain/src/config/` — schema + config loading/merging
- `crates/infra/adapters/src/agent/` — orchestration loop (classifier, dispatcher, run_context re-export from fork_core)
- `crates/infra/security/src/` — policy, pairing, secret store (AutonomyLevel/ToolOperation re-export from fork_core)
- `crates/infra/memory/src/` — memory trait + backends (MemoryCategory/MemoryEntry re-export from fork_core)
- `crates/infra/adapters/src/runtime/` — runtime adapters (currently native)
- `crates/domain/` — hexagonal domain core: domain types, ports, application services
- `crates/infra/adapters/src/` — fork-specific adapter implementations (26 modules):
  - `channels/` — Telegram/Discord/Slack/Matrix/IRC/Lark/Nostr/etc
  - `providers/` — model providers and resilient wrapper
  - `tools/` — tool execution surface (shell, file, memory, browser, IPC)
  - `gateway/` — webhook/gateway server
  - `observability/` — logging, tracing, metrics
  - `hooks/` — lifecycle hooks
  - `pipeline/` — deterministic pipeline engine
  - `ipc/` — inter-agent IPC broker
  - `daemon/`, `service/`, `cron/`, `approval/`, `auth/`, `cost/`, `health/`, `heartbeat/`, `tunnel/`, etc.
- `docs/` — topic-based documentation (setup-guides, reference, ops, security, contributing, maintainers)
- `.github/` — CI, templates, automation workflows

## Risk Tiers

- **Low risk**: docs/chore/tests-only changes
- **Medium risk**: most `src/**` behavior changes without boundary/security impact
- **High risk**: `crates/infra/security/src/**`, `crates/infra/adapters/src/runtime/**`, `src/gateway/**`, `src/tools/**`, `.github/workflows/**`, access-control boundaries

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

- **Tool allowlist boundary** (`crates/infra/adapters/src/agent/loop_.rs`): When `SYNAPSECLAW_ALLOWED_TOOLS` is set (ephemeral agents), the allowlist filter is a **hard security boundary**. Any new tool injection path must either register tools **before** the filter, or be explicitly suppressed/filtered when the allowlist is active. Current correct order: built-ins → **allowlist filter + delegate filter** → MCP (suppressed if allowlist active). Violating this invariant creates a sandbox escape. See PRs #48-#49 for context.

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
