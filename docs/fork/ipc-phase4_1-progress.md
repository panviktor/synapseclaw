# IPC Phase 4.1 Progress

**Status**: ALL 10 SLICES COMPLETE — Phase 4.1 engine ready for wiring

Phase 4.0: modular core refactor | **Phase 4.1: deterministic pipeline engine** | Phase 4.2: federated execution

---

## Goal

Build a deterministic pipeline engine that orchestrates multi-agent workflows with typed contracts, tool call safety, and resilient execution — all on top of Phase 4.0's port/adapter architecture.

---

## Checklist

### New dependencies

| Item | Status | Notes |
|------|--------|-------|
| `notify` crate added to workspace | **DONE** | Filesystem watcher for hot-reload |
| `jsonschema` crate added to workspace | **DONE** | JSON Schema validation for step contracts |
| `serde` with `derive` added to fork_core | **DONE** | Pipeline domain type serialization |

### Phase 4.0 extensions (backwards-compatible)

| Item | Status | Notes |
|------|--------|-------|
| `RunOrigin::Pipeline` variant added | **DONE** | + `from_str_lossy()` |
| `RunStorePort::list_by_state()` default method | **DONE** | For pipeline recovery on startup |

### Domain types (fork_core)

| Item | Status | PRs |
|------|--------|-----|
| `domain/pipeline.rs` — PipelineDefinition, PipelineStep, StepTransition | **DONE** | Slice 1 |
| `domain/pipeline_context.rs` — PipelineContext, PipelineState, StepRecord | **DONE** | Slice 1 |
| `domain/pipeline_validation.rs` — JSON Schema validation helper | **DONE** | Slice 2 |
| `domain/tool_middleware.rs` — ToolBlock, ToolCallContext | **DONE** | Slice 3 |
| `domain/routing.rs` — RoutingTable, Route, RoutingRule | **DONE** | Slice 6 |
| ConditionalBranch, Operator enum | **DONE** | In pipeline.rs |
| FanOutSpec, FanOutBranch | **DONE** | In pipeline.rs |
| `domain/pipeline_event.rs` — PipelineEvent enum | **DONE** | Slice 10 |

### Ports (fork_core)

| Item | Status | PRs |
|------|--------|-----|
| `ports/pipeline_store.rs` — PipelineStorePort | **DONE** | Slice 1 |
| `ports/pipeline_executor.rs` — PipelineExecutorPort | **DONE** | Slice 2 |
| `ports/tool_middleware.rs` — ToolMiddlewarePort | **DONE** | Slice 3 |
| `ports/message_router.rs` — MessageRouterPort | **DONE** | Slice 6 |
| `ports/pipeline_observer.rs` — PipelineObserverPort | **DONE** | Slice 10 |

### Application services and use cases (fork_core)

| Item | Status | PRs |
|------|--------|-----|
| `services/pipeline_service.rs` — PipelineRunner | **DONE** | Slice 2 |
| `services/tool_middleware_service.rs` — middleware chain | **DONE** | Slice 3 |
| `services/routing_service.rs` — rule evaluation | TODO | |
| `use_cases/start_pipeline.rs` | **DONE** | Slice 2 |
| `use_cases/resume_pipeline.rs` | **DONE** | Slice 5 |
| `use_cases/cancel_pipeline.rs` | **DONE** | Slice 5 |
| `use_cases/route_inbound.rs` | TODO | |

### Adapters (fork_adapters)

| Item | Status | PRs |
|------|--------|-----|
| `pipeline/toml_loader.rs` — TOML → PipelineDefinition | **DONE** | Slice 1 |
| `pipeline/hot_reload.rs` — notify watcher | **DONE** | Slice 8 |
| `pipeline/ipc_step_executor.rs` — step via IPC broker | **DONE** | Slice 2 |
| `pipeline/schema_validator.rs` — jsonschema validation | **DONE** | Slice 1 |
| `middleware/rate_limit.rs` | **DONE** | Slice 3 |
| `middleware/validation.rs` | **DONE** | Slice 3 |
| `middleware/approval_gate.rs` | **DONE** | Slice 3 |
| `routing/rule_chain.rs` — TomlMessageRouter | **DONE** | Slice 6 |

### Integration points (existing code modifications)

| Item | Status | PRs |
|------|--------|-----|
| `src/agent/loop_.rs` → `execute_one_tool` middleware hook | TODO | |
| `HandleInboundMessage` → MessageRouter pre-routing | TODO | |
| Daemon startup → pipeline recovery from RunStorePort | TODO | |
| Daemon startup → hot-reload watcher | TODO | |
| Pipeline TOML directory in workspace config | TODO | |

### Slices (implementation order)

| Slice | Description | Status | PRs |
|-------|-------------|--------|-----|
| 1 | Pipeline core — domain types + TOML loading + schema validation | **DONE** | |
| 2 | IPC bridge — step execution through broker + checkpointing | **DONE** | |
| 3 | ToolMiddleware — before/after hooks on tool calls | **DONE** | |
| 4 | FanOut + Join — parallel step execution | **DONE** | |
| 5 | Checkpointing — resume after crash | **DONE** | |
| 6 | MessageRouter — deterministic routing | **DONE** | |
| 7 | WaitForApproval — human-in-the-loop via ApprovalPort | **DONE** | |
| 8 | Hot-reload — notify filesystem watcher | **DONE** | |
| 9 | Nested pipelines — pipeline as step | **DONE** | |
| 10 | Observability — pipeline events via Observer | **DONE** | |

---

## Critical acceptance criteria

| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | Multi-step workflow executes deterministically | TODO | |
| 2 | Invalid step output rejected by JSON Schema validation | TODO | |
| 3 | Tool calls rate-limited; feedback loops impossible | TODO | |
| 4 | Inbound messages routed by deterministic rules | TODO | |
| 5 | Pipeline resumes from checkpoint after daemon restart | TODO | |
| 6 | TOML hot-reload: new runs use new def, running runs use old | TODO | |
| 7 | FanOut parallel branches + join with merged results | TODO | |
| 8 | Sub-pipeline execution with depth limit | TODO | |
| 9 | Pipeline events visible in journalctl | TODO | |
| 10 | Zero new storage dependencies | TODO | |

---

## Review checkpoints

### Checkpoint A — foundation

After slices 1-2:
- Domain types stable
- TOML loading works
- Can run a sequential pipeline through IPC

### Checkpoint B — safety

After slices 3-5:
- Tool calls intercepted
- Parallel execution works
- Crash recovery proven

### Checkpoint C — routing and UX

After slices 6-8:
- Deterministic routing active
- Approval gates work
- Hot-reload operational

### Checkpoint D — full engine

After slices 9-10:
- Nested pipelines work
- Full observability
- All acceptance criteria met

---

## Deferred to Phase 4.2

| Item | Reason |
|------|--------|
| Multi-host pipeline execution | Requires federated execution substrate |
| Pipeline web UI / visual editor | UX phase, not engine phase |
| LLM-assisted routing (as non-fallback) | Deterministic routing must prove itself first |
| Pipeline marketplace / sharing | Requires stable format, too early |
| CodingWorkerPort concrete adapter | Blocked on pipeline engine (this phase provides the runner) |
