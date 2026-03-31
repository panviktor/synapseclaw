# SynapseClaw News & Changelog

## 2026-03-31

### Phase 3.12: Thread Context Seeding (PR #215)

- SQLite summary persistence (`session_summaries` table)
- Thread seeding: parent summary + last 3 turns (2000 char budget) + root message (Matrix)
- Discord thread detection via `message_reference`/`thread` fields
- `smart_truncate_parent_turns` — domain-pure, per-message char cap
- All 17 IPC phases now DONE

### Phases 8-12: Full Crate Extraction (PRs #209-#211)

**12 workspace crates** — bottleneck 146K → 55K LOC (2.7x), 4 crates compile in parallel on Wave 5.

- **Phase 8**: Rename `crates/infra/` → `crates/adapters/`, `adapters/` → `adapters/core/`
- **Phase 9a-9c**: Extract `synapse_mcp` (3K), `synapse_infra` (5K), promote Channel/Tool ports to domain
- **Phase 10**: Extract `synapse_channels` (34K) — 30+ channel implementations
- **Phase 11**: Extract `synapse_tools` (37K) — 49 tools, NO crate aliases
- **Phase 12**: Extract `synapse_onboard` (7K), remove ALL `pub use X as Y` re-exports
- **IpcClientPort**: Full trait migration — `broker_get`/`broker_post` in domain, all 7 IPC tools on port
- **Bug fix**: Channel interrupt cancellation token was created but never checked — added `tokio::select!`
- **Test fixes**: 4417 tests pass, 0 failures across all 12 crates

```
Wave 1: domain (24K)
Wave 2: security (10K) | observability (5K) | memory (8K)
Wave 3: providers (20K) | cron (3K)
Wave 4: infra (5K) | mcp (3K)
Wave 5: channels (34K) | tools (37K) | onboard (7K) | core (55K)
Wave 6: synapseclaw binary
```

## 2026-03-30

### Phase 5: Complete Hexagonal Architecture (20 PRs: #181-#200)

**Full hexagonal restructuring** — domain is pure, src/ contains only main.rs + lib.rs, all infrastructure in workspace crates.

**Phase 4.2 — Decouple adapters (PRs #181-#190):**
- AgentRunnerPort breaks agent↔adapters circular dependency (#185)
- Config types (6.2K LOC) extracted to fork_config (#186)
- fork_security crate (10K LOC) extracted (#189)
- `crate::` refs in adapters: 1,255 → 49 (96% reduction)

**Phase 5A — Crate promotion (PRs #191-#196):**
- Rename: fork_core→synapse_core, fork_security→synapse_security (#191)
- Dissolve synapse_config into synapse_core (#193)
- Extract synapse_memory crate (8K LOC) (#195)
- **THE BIG MOVE**: promote adapters + agent to synapse_adapters crate (55K LOC (core) + 155K total across 12 crates) (#196)
- Delete src/adapters/, crates/adapters/core/src/agent/, crates/adapters/security/src/, crates/adapters/core/src/runtime/ — src/ = 2 files

**Phase 5B — Purify domain (PRs #197-#200):**
- Hexagonal directory layout: `crates/domain/` + `crates/adapters/core/` (#197)
- Feature flag propagation fix, CLAUDE.md update (#198)
- Remove reqwest from domain — extract proxy to adapters (#199)
- Move security + memory sub-crates inside adapters (#200)

**Phase 5 architecture (before Phase 8-12 extraction):**
```
crates/
  domain/                    ← PURE DOMAIN (zero infra deps, 24K)
  adapters/core/             ← composition root (55K)
  adapters/channels/         ← 30+ platforms (34K)
  adapters/tools/            ← 49 tools (37K)
  adapters/security/         ← pairing, secrets, audit (10K)
  adapters/memory/           ← sqlite, qdrant (8K)
  adapters/providers/        ← LLM providers (20K)
  adapters/observability/    ← prometheus, otel (5K)
  adapters/cron-store/       ← scheduler (3K)
  adapters/mcp/              ← MCP protocol (3K)
  adapters/infra/            ← config_io, identity (5K)
  adapters/onboard/          ← setup wizard (7K)
```

Domain dependencies: serde, schemars, async-trait, chrono, uuid, url, anyhow, parking_lot — **zero HTTP, zero filesystem, zero CLI framework**.

## 2026-03-29

## 2026-03-28

### Phase 4.1H: Hexagonal Architecture Migration
- **Slice 0** — audit + dead code removal: −15K lines (#176)
- **Slices 1-7** — moved 26 adapter modules → `src/fork_adapters/` (#177): auth, cost, tunnel, heartbeat, health, integrations, channels, providers, tools, gateway, observability, hooks, cron, approval, daemon, doctor, onboard, service, middleware, pipeline, routing, storage, inbound, ipc, memory
- **Slice 8** — extracted `AutonomyLevel`/`ToolOperation`/`CommandRiskLevel` to `fork_core::domain::config` (#178)
- **Slice 9** — extracted `MemoryCategory`/`MemoryEntry` to `fork_core::domain::memory` (#178)
- **Slices 10-11** — extracted `RunContext`/`ToolEvent` to `fork_core::domain::tool_audit`, `QueryClassificationConfig`/`ClassificationRule`/`classify()` to `fork_core::domain::query_classification` (#179)
- **Slice 12** — extracted SecurityPolicy (2.7K LOC + 113 tests), Memory/Runtime/Sandbox traits, util → fork_core; crate promotion deferred (Config struct too large to move)
- **Slice 13** — documentation update (this entry)

## 2026-03-27

### Phase 4.1: Shared IpcClient + routing cleanup
- **Shared IpcClient** — single `Arc<IpcClient>` in daemon, eliminates `replay_rejected`
- **Rename openclaw → broker** in config, token_metadata, seq file
- **Command-only routing** — removed keyword routes, `/content` triggers pipeline deterministically
- **Pipeline response** — human-readable summary instead of raw JSON dump
- **Clippy cleanup** — all warnings fixed in fork_core + synapseclaw

## 2026-03-26

### Phase 4.1 Slices 1-3: Deterministic Pipeline Engine foundation
- **Slice 1 — Pipeline domain types + TOML loading + schema validation**
  - `domain/pipeline.rs`: PipelineDefinition, PipelineStep, StepTransition, ConditionalBranch, Operator, FanOutSpec
  - `domain/pipeline_context.rs`: PipelineContext, PipelineState, StepRecord for run tracking
  - `ports/pipeline_store.rs`: PipelineStorePort trait + ReloadEvent
  - `fork_adapters/pipeline/toml_loader.rs`: TomlPipelineLoader (directory scan, validation, reload diffing)
  - `fork_adapters/pipeline/schema_validator.rs`: JSON Schema validation for step contracts
  - Phase 4.0 extensions: `RunOrigin::Pipeline`, `RunStorePort::list_by_state()`
  - Fixture: `content-creation.toml` (4-step marketing pipeline)
- **Slice 2 — PipelineRunner + IPC bridge + checkpointing**
  - `services/pipeline_service.rs`: full execution loop — sequential + conditional branches, retry with backoff, global/per-step timeouts, checkpointing after each step
  - `use_cases/start_pipeline.rs`: entry point for triggering pipeline runs
  - `ports/pipeline_executor.rs`: PipelineExecutorPort trait (mockable step dispatch)
  - `domain/pipeline_validation.rs`: JSON Schema validation helper for fork_core
  - `fork_adapters/pipeline/ipc_step_executor.rs`: IPC adapter (task dispatch via DispatchIpcMessage, poll for result)
  - Safety-net timeouts: 30min default per step, 2h default per pipeline
- **Slice 3 — ToolMiddleware: rate limit, validation, approval gate**
  - `domain/tool_middleware.rs`: ToolBlock enum + ToolCallContext
  - `ports/tool_middleware.rs`: ToolMiddlewarePort trait (before/after hooks)
  - `services/tool_middleware_service.rs`: ToolMiddlewareChain (ordered execution)
  - `fork_adapters/middleware/rate_limit.rs`: per-tool per-run call limits
  - `fork_adapters/middleware/validation.rs`: JSON Schema on tool arguments
  - `fork_adapters/middleware/approval_gate.rs`: human-in-the-loop for dangerous tools
- New dependencies: `jsonschema` (step contracts), `notify` (hot-reload)
- **Audit**: 2 rounds, all CRITICAL/MODERATE/MINOR findings fixed
  - Parallel FanOut via Arc+JoinSet, flat struct ComplexTransition (TOML fix),
    cancel pipeline works, context 10MB cap, step history cap 500,
    approval per-step, rate limit eviction, safe timestamps
- **Wiring**: PipelineEngineConfig, gateway AppState integration,
  IPC endpoints (POST /api/pipelines/start, GET /api/pipelines/list),
  ToolMiddleware hook in execute_one_tool, pipeline recovery on startup
- **Agent integration**: pipeline-aware inbox processing — detects pipeline_step
  in IPC payload, enforces JSON response via prompt, auto-reply JSON extraction
  (markdown code blocks, brace extraction, fallback wrapping)
- Example pipelines: content-creation.toml, parallel-research.toml, routing.toml
  (matched to real fleet: news-reader/copywriter/marketing-lead/publisher/trend-aggregator)
- 315 tests (252 fork_core + 63 adapters), 0 failures
- Merged to master and deployed

## 2026-03-24 (2)

### Project independence: upstream detachment, i18n cleanup, README rewrite
- **Removed 29 non-EN/RU README translations** + 29 docs hub translations
- **Deleted Vietnamese docs tree** (`docs/vi/`, ~40 files) and **Chinese docs tree** (`docs/i18n/zh-CN/`, ~60 files)
- **README.md completely rewritten** — removed upstream donation links, social media badges, Special Thanks, benchmark table, Star History, contributor badges; updated project description to reflect Phase 4.0 architecture
- **README.ru.md rewritten** — mirrors new EN README in Russian
- **NOTICE updated** — minimal ZeroClaw attribution (MIT/Apache requirement)
- **Upstream sync infrastructure removed** — deleted `upstream-sync.yml` workflow, sync scripts, sync PR/issue templates
- **CONTRIBUTING.md cleaned** — removed Branch Migration Notice (upstream artifact)
- **docs/fork/README.md updated** — project is now independent, not a fork extension; removed sync automation references
- **docs/fork/sync-strategy.md archived** — kept for historical reference with archive header

## 2026-03-24

### Phase 4.0 workspace crate + all 10 use cases + full restructuring
- **fork_core extracted as workspace crate** (`crates/fork_core/`) — 0 upstream deps, compiles standalone
- Core-owned types: `ChatMessage`, `AutonomyLevel`, `HeartbeatConfig`, `CronDeliveryConfig`, `AutoDetectCandidate`
- `ChannelRegistryPort::resolve()` → `has_channel()` — removed `Channel` trait dependency
- `InboundEnvelope::from_channel_message()` moved to fork_adapters (adapter concern)
- Old `src/fork_core/` directory deleted
- **10 of 10 use cases now implemented:**
  - `SpawnChildAgent` — provision ephemeral identity, create Run(Spawn), return child token (5 tests)
  - `ResumeConversation` — load session + rebuild transcript from ConversationStorePort (4 tests)
  - `AbortConversationRun` — cancel running execution with terminal state guard (4 tests)
  - `DispatchIpcMessage` — resolve → limit → ACL → send (5 tests)
- New domain: `domain/spawn.rs` (SpawnRequest, EphemeralAgent, SpawnStatus)
- New port: `ports/spawn_broker.rs` (SpawnBrokerPort)
- **fork_adapters restructured** — `inbound/` split into `runtime/`, `memory/`, `ipc/`
- New adapters: `IpcBusAdapter`, `QuarantineAdapter` (wraps IpcDb behind ports)
- ResumeConversation **wired into ws.rs** ensure_session
- Updated progress.md, delta-registry.md, news.md
- 180 fork_core tests + 22 fork_adapters tests
- 170+ total fork_core tests
