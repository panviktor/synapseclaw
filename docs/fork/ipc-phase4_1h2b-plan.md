# Phase 4.1H2B — Pure Hexagonal Architecture (Ports & Adapters)

> Implementing [Hexagonal Architecture](https://en.wikipedia.org/wiki/Hexagonal_architecture_(software))
> (also known as **Ports & Adapters**, coined by Alistair Cockburn).
>
> The core idea: the application's business logic knows nothing about the
> outside world. It communicates through **ports** (abstract interfaces/traits)
> which are implemented by **adapters** (concrete technology bindings).
> Dependencies always point **inward** — from infrastructure to domain, never
> the reverse. The outermost layer is a thin **composition root** that wires
> concrete adapters into abstract ports.
>
> **Goal**: `src/` contains ONLY `main.rs` (composition root) + thin `lib.rs`
> facade. All business logic, port definitions, and adapter implementations
> live in workspace crates with strictly enforced dependency direction.

## Context

Phase 4.2 + Phase 5 (PRs #181-#193) reduced `crate::` refs from 1,255 to 49
and restructured into 2 workspace crates. But `src/` still contains 186K LOC:
adapters, agent, memory, config IO. Additionally, `main.rs` duplicates the
lib.rs module tree — everything compiles **twice**.

This plan completes the hexagonal migration: all code moves to workspace
crates, `src/` becomes a pure composition root.

---

## Target Architecture

```
crates/
  synapse_core/         ← DOMAIN: pure types, ports (no reqwest/tokio-fs)
  synapse_security/     ← SECURITY: implementations
  synapse_config/       ← CONFIG IO: load/save/encrypt + workspace + proxy
  synapse_memory/       ← MEMORY: backends (sqlite, postgres, qdrant, etc)
  synapse_adapters/     ← ADAPTERS: channels, providers, tools, gateway + agent

src/
  main.rs               ← COMPOSITION ROOT: CLI parsing, DI wiring
  lib.rs                ← THIN FACADE: re-exports for integration tests
```

### Dependency Rules (strictly enforced, all point inward)

```
synapse_core       ← nothing (pure domain, zero infra deps)
synapse_security   ← core
synapse_config     ← core, security
synapse_memory     ← core, config
synapse_adapters   ← core, security, config, memory
main.rs            ← all crates (composition root)
```

---

## Phases

### Phase 0 — Fix dual compilation
Remove duplicate `mod` declarations from `main.rs`. Use `synapseclaw::*`
(lib.rs) instead. Halves build time immediately.

### Phase 1 — Extract `synapse_config` crate
- ConfigIO trait impl (load/save/validate/encrypt)
- WorkspaceManager + WorkspaceProfile
- Proxy builder functions (reqwest-dependent)
- workspace_boundary.rs
- **Purify synapse_core**: remove reqwest/directories deps

### Phase 2 — Extract `synapse_memory` crate
- All backends: sqlite, postgres, qdrant, lucid, markdown, none
- Factory: `create_memory()`, embeddings, chunker, knowledge_graph
- 18 files, 7K LOC

### Phase 3 — Extract `synapse_adapters` crate (THE BIG MOVE)
- All 28 adapter modules (152K LOC) + agent (15K LOC)
- Agent merges with adapters (it adapts AI providers → application use case)
- Move CLI sub-enums to `synapse_core::commands` (without clap)
- Move `scrub_credentials()` to `synapse_core::domain::util`
- Split `Agent::from_config` → `synapse_adapters::agent_factory`

### Phase 4 — Slim `src/` to composition root
- `lib.rs`: ~20 lines of `pub use synapse_*::*` re-exports
- `main.rs`: CLI parsing + DI wiring only
- Delete all `src/` directories and stubs

---

## Final Architecture Description

### Hexagonal Layers (Ports & Adapters)

In hexagonal architecture, the application is structured as concentric rings:

- **Domain (innermost)** — business rules, domain types, port trait definitions.
  Has zero dependencies on frameworks, databases, HTTP, or any I/O.
  If you delete every adapter, the domain still compiles.

- **Ports** — traits (interfaces) that the domain EXPOSES for inbound use
  (driving ports: `AgentRunnerPort`, `PipelineExecutorPort`) and REQUIRES
  from outbound services (driven ports: `Memory`, `Provider`, `Tool`,
  `Observer`, `RuntimeAdapter`, `ConversationStore`).

- **Adapters (outermost)** — concrete implementations of ports.
  *Inbound adapters* (gateway HTTP, Telegram webhook, CLI) drive the
  application by calling port traits. *Outbound adapters* (OpenAI client,
  SQLite backend, Docker runtime) implement port traits the domain needs.

- **Composition Root** — `main.rs` creates concrete adapters, injects them
  into ports, and starts the application. This is the ONLY place that knows
  about ALL concrete types.

SynapseClaw maps this to Rust workspace crates:

```
┌─────────────────────────────────────────────────────┐
│                  main.rs (Composition Root)          │
│          CLI parsing, dependency injection           │
├─────────────────────────────────────────────────────┤
│                                                     │
│  ┌─────────────────────────────────────────────┐    │
│  │         synapse_adapters (Adapters)          │    │
│  │                                             │    │
│  │  Inbound:   gateway, channels (telegram,    │    │
│  │             discord, slack, matrix, ...)     │    │
│  │  Outbound:  providers (openai, anthropic,   │    │
│  │             gemini, ollama, ...)             │    │
│  │  Tools:     shell, browser, file_read, ...  │    │
│  │  Agent:     loop, classifier, dispatcher    │    │
│  │  Infra:     cron, daemon, heartbeat,        │    │
│  │             observability, hooks, tunnel     │    │
│  ├─────────────────────────────────────────────┤    │
│  │                                             │    │
│  │  ┌──────────────┐  ┌──────────────────┐     │    │
│  │  │synapse_memory│  │ synapse_config   │     │    │
│  │  │              │  │                  │     │    │
│  │  │ sqlite       │  │ load/save/encrypt│     │    │
│  │  │ postgres     │  │ workspace mgmt   │     │    │
│  │  │ qdrant       │  │ proxy builders   │     │    │
│  │  │ embeddings   │  │                  │     │    │
│  │  └──────┬───────┘  └────────┬─────────┘     │    │
│  │         │                   │               │    │
│  │  ┌──────┴───────────────────┴─────────┐     │    │
│  │  │        synapse_security            │     │    │
│  │  │  pairing, secrets, audit, sandbox  │     │    │
│  │  │  identity, prompt guard, estop     │     │    │
│  │  └──────────────┬─────────────────────┘     │    │
│  │                 │                           │    │
│  └─────────────────┼───────────────────────────┘    │
│                    │                                │
│  ┌─────────────────┴───────────────────────────┐    │
│  │           synapse_core (Domain)              │    │
│  │                                             │    │
│  │  Domain types:  Config, SecurityPolicy,     │    │
│  │    ChatMessage, MemoryEntry, Pipeline, ...  │    │
│  │  Port traits:   Provider, Tool, Memory,     │    │
│  │    Observer, RuntimeAdapter, Channel, ...   │    │
│  │  App services:  DeliveryService,            │    │
│  │    ConversationService, PipelineService     │    │
│  │  Value objects:  AutonomyLevel, RunContext,  │    │
│  │    ToolOperation, QueryClassification       │    │
│  │                                             │    │
│  │  ZERO external dependencies (pure Rust +    │    │
│  │  serde/async-trait only)                    │    │
│  └─────────────────────────────────────────────┘    │
│                                                     │
└─────────────────────────────────────────────────────┘
```

### Hexagonal Principles Applied

| Hexagonal Concept | SynapseClaw Implementation |
|---|---|
| **Domain (Application Core)** | `synapse_core` — zero deps on HTTP, DB, LLM APIs. Compiles standalone. |
| **Driving Ports** (inbound) | `AgentRunnerPort`, `PipelineExecutorPort`, `ApprovalPort` in `synapse_core::ports` |
| **Driven Ports** (outbound) | `Memory`, `Provider`, `Tool`, `Observer`, `RuntimeAdapter`, `ConversationStore` in `synapse_core::ports` |
| **Inbound Adapters** | Gateway (HTTP/WS), Telegram webhook, Discord bot, CLI handler — in `synapse_adapters` |
| **Outbound Adapters** | OpenAI client, SQLite backend, Docker runtime, Prometheus exporter — in `synapse_adapters` |
| **Composition Root** | `main.rs` — creates adapters, injects into ports, starts app |
| **Dependency Rule** | All `use` paths point inward: adapters → core, never core → adapters |
| **Testability** | Any port can be mocked: `NoopRunner`, `TestMemory`, `MockProvider` |
| **Independent Compilation** | `cargo build -p synapse_core` — no adapters needed |

### Crate Responsibilities

| Crate | LOC | Layer | Responsibility |
|-------|-----|-------|---------------|
| `synapse_core` | 22K | Domain | Types, port traits, application services, value objects |
| `synapse_security` | 10K | Infrastructure | Encryption, pairing, sandbox, audit, identity |
| `synapse_config` | 6K | Infrastructure | Config file IO, workspace management, proxy HTTP builders |
| `synapse_memory` | 7K | Infrastructure | Memory backends (SQLite, Postgres, Qdrant, embeddings) |
| `synapse_adapters` | 167K | Infrastructure | All adapters: 28 modules + agent loop |
| `main.rs` | 2K | Composition | CLI parsing, dependency injection, application bootstrap |

### Why Agent Lives in Adapters

The agent loop (`agent/loop_.rs`) orchestrates concrete providers, tools,
and channels. It calls `provider.chat()`, `tool.execute()`, `channel.send()`.
These are all adapter-layer operations. Extracting agent to its own crate
would require full trait inversion of every provider/tool/channel call —
premature abstraction. Agent IS an adapter: it adapts AI provider APIs into
the application's conversational use case.

---

## Verification

```bash
# Only main.rs + lib.rs remain in src/
find src/ -name "*.rs" | sort
# → src/lib.rs src/main.rs

# All crates build independently
cargo build -p synapse_core
cargo build -p synapse_security
cargo build -p synapse_config
cargo build -p synapse_memory
cargo build -p synapse_adapters

# Full test suite passes
cargo test
```
