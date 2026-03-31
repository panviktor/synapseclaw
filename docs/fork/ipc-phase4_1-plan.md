# IPC Phase 4.1: Deterministic Pipeline Engine

Phase 4.0: modular core refactor | **Phase 4.1: deterministic pipeline engine** | Phase 4.2: federated execution

---

## What Phase 4.1 gives

Five promises to the fork:

1. **Deterministic multi-agent workflows** — agents execute pipeline steps in defined order with typed handoffs, not free-form LLM-driven routing.
2. **Anti-hallucination contracts** — each step declares expected input/output schemas; the engine validates data at every transition, agents cannot skip or invent steps.
3. **Tool call safety** — middleware layer intercepts tool calls with rate limiting, validation, and human-in-the-loop approval gates.
4. **Deterministic message routing** — rule-based routing chain replaces broadcast-to-all; LLM classifier is last-resort fallback only.
5. **Resilient execution** — checkpointing, retry with backoff, nested pipelines, and hot-reload make pipelines production-grade.

---

## Why Phase 4.1 exists

Phase 4.0 delivered modular core with ports, adapters, domain types, and 6 application services. But the fleet still has fundamental problems:

1. **Agents hallucinate workflow steps** — no mechanism forces an agent to follow a multi-step process. The LLM decides what to do next, and it often skips steps or invents new ones.
2. **IPC broker broadcasts instead of routing** — messages go to all registered agents or rely on LLM to pick the right recipient.
3. **Push notifications create feedback loops** — a result from agent A triggers agent B, which triggers agent A again. No rate limiting, no circuit breakers.
4. **No pipeline concept** — there is no way to define "news-reader researches → copywriter drafts → marketing-lead reviews → publisher publishes" as a first-class executable object.

These problems cannot be solved by better prompts. They require a deterministic execution engine.

---

## Design principles

### Dynamic over static

Everything is configurable at runtime through TOML files. Adding a new pipeline, changing step order, adjusting routing rules — none of these require recompilation. If we wanted static scripts, we would use n8n.

### Contracts are JSON Schema, not Rust enums

Step input/output contracts are `serde_json::Value` validated against JSON Schema at runtime. This trades compile-time safety for the ability to define arbitrary agent workflows without touching Rust code.

### Pipeline engine is orchestrator, agents are executors

The engine tells agents what to do via IPC. Agents do not know they are in a pipeline — they receive a message, process it, return a result. This preserves existing agent architecture.

### Zero new storage dependencies

All state persists through existing `RunStorePort` and `ConversationStorePort` (rusqlite). No new databases, no new storage engines.

### Borrowed design, zero borrowed code

API patterns inspired by graph-flow (Task/NextAction/Context/FanOut) and LangGraph (StateGraph/conditional edges). No external dependencies — implemented natively in `fork_core` on top of tokio + serde + rusqlite.

---

## Inputs from Phase 4.0

Phase 4.0 delivered the foundation that 4.1 builds on:

| Phase 4.0 artifact | Phase 4.1 usage |
|---------------------|-----------------|
| `RunStorePort` + `ChatDbRunStore` | Pipeline checkpointing, step state persistence |
| `ConversationStorePort` | Pipeline context persistence |
| `DispatchIpcMessage` use case | IPC bridge — pipeline steps dispatch work to agents |
| `IpcBusPort` | Step results received from agents |
| `ApprovalPort` + `RequestApproval` | WaitForApproval step pauses |
| `InboundEnvelope` / `OutboundIntent` | MessageRouter input/output types |
| `HandleInboundMessage` | MessageRouter integration point |
| `ChannelRegistryPort` | Delivery for pipeline notifications |
| `Observer` trait (upstream) | Pipeline event emission |

---

## Non-goals

1. No visual pipeline builder or web UI for pipeline editing.
2. No distributed execution across multiple hosts (that is Phase 4.2).
3. No new database engine or storage backend.
4. No changes to existing agent process model (1 agent = 1 process).
5. No LLM-driven step routing inside the pipeline engine (conditional edges are code/data, not LLM).
6. No breaking changes to existing IPC protocol.

---

## New dependencies

Only two new crates, both lightweight:

| Crate | Purpose | Size |
|-------|---------|------|
| `notify` | Filesystem watcher for TOML hot-reload | ~50KB |
| `jsonschema` | JSON Schema validation for step contracts | ~200KB |

Both are well-maintained, MIT-licensed, widely used in Rust ecosystem.

Additionally, `serde` with `derive` feature is added to `fork_core` (was only `serde_json` before) for Serialize/Deserialize on pipeline domain types.

### Phase 4.0 extensions required

- `RunOrigin` gains a `Pipeline` variant for pipeline-originated runs.
- `RunStorePort` gains `list_by_state(states: &[RunState], limit: usize) -> Vec<Run>` for recovery queries.
- These are backwards-compatible additions, no breaking changes.

---

## Target module layout

```text
crates/fork_core/src/
  domain/
    pipeline.rs          -- PipelineDefinition, PipelineStep, StepTransition, FanOutSpec
    pipeline_context.rs  -- PipelineContext (shared state for a pipeline run)
    tool_middleware.rs    -- ToolBlock, ToolInterception
    routing.rs           -- Route, RoutingRule, MessageRouter definition
  ports/
    pipeline_store.rs    -- PipelineStorePort (load/list/watch pipeline definitions)
    tool_middleware.rs    -- ToolMiddlewarePort (before/after hooks)
    message_router.rs    -- MessageRouterPort (deterministic routing)
    pipeline_observer.rs -- PipelineObserverPort (event emission)
  application/
    services/
      pipeline_service.rs    -- PipelineRunner, step execution, checkpointing
      tool_middleware_service.rs -- middleware chain management
      routing_service.rs     -- rule evaluation, fallback handling
    use_cases/
      start_pipeline.rs      -- trigger a pipeline run
      resume_pipeline.rs     -- resume from checkpoint after crash
      cancel_pipeline.rs     -- cancel a running pipeline
      route_inbound.rs       -- deterministic routing for inbound messages

src/fork_adapters/
  pipeline/
    toml_loader.rs       -- load PipelineDefinition from TOML files
    hot_reload.rs        -- notify-based file watcher, reload on change
    ipc_step_executor.rs -- execute pipeline step via IPC broker
    schema_validator.rs  -- jsonschema-based contract validation
  middleware/
    rate_limit.rs        -- RateLimitMiddleware
    validation.rs        -- ValidationMiddleware (JSON Schema on tool args)
    approval_gate.rs     -- ApprovalGateMiddleware (human-in-the-loop)
  routing/
    rule_chain.rs        -- ordered rule evaluation
```

---

## Domain concepts

### PipelineDefinition

A complete workflow specification loaded from TOML:

```rust
pub struct PipelineDefinition {
    pub name: String,
    pub version: String,
    pub description: String,
    pub steps: Vec<PipelineStep>,
    pub entry_point: String,
    pub max_depth: u8,              // nested pipeline limit, default 5
    pub timeout_secs: Option<u64>,  // global pipeline timeout
}
```

### PipelineStep

An atomic unit of work within a pipeline:

```rust
pub struct PipelineStep {
    pub id: String,
    pub agent_id: String,
    pub description: String,
    pub tools: Vec<String>,              // scoped tool allowlist for this step
    pub input_schema: Option<Value>,     // JSON Schema for expected input
    pub output_schema: Option<Value>,    // JSON Schema for expected output
    pub next: StepTransition,
    pub max_retries: u8,                 // default 0
    pub retry_backoff_secs: u64,         // default 5
    pub timeout_secs: Option<u64>,       // per-step timeout
}
```

### StepTransition

Deterministic flow control:

```rust
pub enum StepTransition {
    /// Go to a specific next step
    Next(String),

    /// Branch based on output data
    Conditional {
        branches: Vec<ConditionalBranch>,
        fallback: String,
    },

    /// Wait for human approval before continuing
    WaitForApproval {
        prompt: String,
        next_approved: String,
        next_denied: String,
    },

    /// Execute multiple steps in parallel, join results
    FanOut(FanOutSpec),

    /// Run another pipeline as a sub-pipeline
    SubPipeline {
        pipeline_name: String,
        next: String,
    },

    /// Pipeline ends here
    End,
}
```

### ConditionalBranch

Data-driven branching (evaluated in code, not LLM):

```rust
pub struct ConditionalBranch {
    pub field: String,       // JSON pointer into step output
    pub operator: Operator,  // eq, ne, gt, lt, contains, matches
    pub value: Value,        // expected value
    pub target: String,      // step id to jump to
}

pub enum Operator {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    Contains,
    Matches,  // regex
}
```

### FanOutSpec

Parallel execution with join:

```rust
pub struct FanOutSpec {
    pub branches: Vec<FanOutBranch>,
    pub join_step: String,           // step that receives merged results
    pub timeout_secs: Option<u64>,   // max wait for all branches
    pub require_all: bool,           // fail if any branch fails, default true
}

pub struct FanOutBranch {
    pub step_id: String,             // step to execute
    pub result_key: String,          // key in merged output: "fanout.<result_key>"
}
```

### PipelineContext

Shared state for a pipeline run, persisted through RunStorePort:

```rust
pub struct PipelineContext {
    pub run_id: String,
    pub pipeline_name: String,
    pub pipeline_version: String,  // for recovery with stale definitions
    pub current_step: String,
    pub state: PipelineState,
    pub data: Value,                 // accumulated step outputs (serde_json::Value)
    pub depth: u8,                   // current nesting depth
    pub started_at: i64,
    pub updated_at: i64,
    pub step_history: Vec<StepRecord>,
    pub error: Option<String>,
}

pub enum PipelineState {
    Running,
    WaitingForAgent(String),         // waiting for agent response
    WaitingForApproval(String),      // waiting for human
    WaitingForFanOut(Vec<String>),   // waiting for parallel branches
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

pub struct StepRecord {
    pub step_id: String,
    pub agent_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub attempt: u8,
    pub status: StepStatus,
    pub output: Option<Value>,
    pub error: Option<String>,
}

pub enum StepStatus {
    Running,
    Completed,
    Failed,
    Retrying,
    Skipped,
    TimedOut,
}
```

### ToolMiddleware

Interception layer for tool calls:

```rust
pub enum ToolBlock {
    RateLimited { tool: String, limit: u32, window_secs: u64 },
    ValidationFailed { tool: String, reason: String },
    ApprovalRequired { tool: String, prompt: String },
    Denied { tool: String, reason: String },
}

#[async_trait]
pub trait ToolMiddlewarePort: Send + Sync {
    /// Called before tool execution. Return Err to block the call.
    async fn before(&self, ctx: &ToolCallContext) -> Result<(), ToolBlock>;

    /// Called after tool execution. Can modify the result.
    async fn after(&self, ctx: &ToolCallContext, result: &mut Value) -> Result<(), ToolBlock>;
}

pub struct ToolCallContext {
    pub run_id: String,
    pub pipeline_name: Option<String>,
    pub step_id: Option<String>,
    pub agent_id: String,
    pub tool_name: String,
    pub args: Value,
    pub call_count: u32,           // how many times this tool was called in this run
}
```

### MessageRouter

Deterministic routing chain:

```rust
pub struct MessageRouter {
    pub rules: Vec<Route>,
    pub fallback: String,           // agent_id for unmatched messages
}

pub struct Route {
    pub name: String,
    pub rule: RoutingRule,
    pub target: String,             // agent_id
    pub priority: u16,              // lower = higher priority
}

pub enum RoutingRule {
    /// Exact command match: "/research" -> news-reader
    Command(String),

    /// Regex match on message content
    Regex(String),

    /// Any keyword present in message
    Keywords(Vec<String>),

    /// Field in InboundEnvelope metadata matches value
    FieldEquals { field: String, value: Value },

    /// Source kind matches (ipc, channel, web, cron)
    SourceKind(String),

    /// Always matches (useful for catch-all before fallback)
    Always,
}
```

---

## Pipeline TOML format

Example: marketing content creation pipeline.

```toml
[pipeline]
name = "content-creation"
version = "1.0"
description = "Research → Draft → Review → Publish content pipeline"
entry_point = "research"
max_depth = 3
timeout_secs = 3600

[[steps]]
id = "research"
agent_id = "news-reader"
description = "Research topic and gather sources"
tools = ["web_search", "rss_fetch", "memory_read"]
next = "draft"
timeout_secs = 300

[steps.output_schema]
type = "object"
required = ["topic", "sources", "summary"]
properties.topic = { type = "string" }
properties.sources = { type = "array", items = { type = "string" } }
properties.summary = { type = "string", minLength = 100 }

[[steps]]
id = "draft"
agent_id = "copywriter"
description = "Write content based on research"
tools = ["memory_read", "memory_write"]
max_retries = 2
retry_backoff_secs = 10
timeout_secs = 600

[steps.input_schema]
type = "object"
required = ["topic", "sources", "summary"]

[steps.output_schema]
type = "object"
required = ["title", "body", "tags"]
properties.title = { type = "string", maxLength = 120 }
properties.body = { type = "string", minLength = 200 }
properties.tags = { type = "array", items = { type = "string" } }

[steps.next]
conditional = [
    { field = "/body", operator = "ne", value = "", target = "review" },
]
fallback = "research"

[[steps]]
id = "review"
agent_id = "marketing-lead"
description = "Review draft quality and approve or request revision"
tools = ["memory_read"]
timeout_secs = 300

[steps.input_schema]
type = "object"
required = ["title", "body", "tags"]

[steps.output_schema]
type = "object"
required = ["approved", "feedback"]
properties.approved = { type = "boolean" }
properties.feedback = { type = "string" }

[steps.next]
conditional = [
    { field = "/approved", operator = "eq", value = true, target = "publish" },
]
fallback = "draft"

[[steps]]
id = "publish"
agent_id = "publisher"
description = "Publish approved content to channels"
tools = ["channel_send"]
next = "end"
timeout_secs = 120

[steps.input_schema]
type = "object"
required = ["title", "body", "tags", "approved"]
```

Example: parallel research pipeline with FanOut.

```toml
[pipeline]
name = "parallel-research"
version = "1.0"
description = "Research news and trends in parallel, then draft"
entry_point = "gather"
max_depth = 3

[[steps]]
id = "gather"
agent_id = "_fanout"
description = "Parallel research: news + trends"

[steps.next.fan_out]
join_step = "draft"
require_all = true
timeout_secs = 300

[[steps.next.fan_out.branches]]
step_id = "fetch-news"
result_key = "news"

[[steps.next.fan_out.branches]]
step_id = "fetch-trends"
result_key = "trends"

[[steps]]
id = "fetch-news"
agent_id = "news-reader"
tools = ["web_search", "rss_fetch"]
next = "_join"

[[steps]]
id = "fetch-trends"
agent_id = "trend-aggregator"
tools = ["web_search"]
next = "_join"

[[steps]]
id = "draft"
agent_id = "copywriter"
tools = ["memory_read"]
next = "end"
```

Example: pipeline with human approval gate.

```toml
[[steps]]
id = "review"
agent_id = "marketing-lead"

[steps.next.wait_for_approval]
prompt = "Marketing lead approved the draft. Publish?"
next_approved = "publish"
next_denied = "draft"
```

---

## Routing TOML format

```toml
# {workspace}/pipelines/routing.toml

[[routes]]
name = "research-commands"
rule.command = "/research"
target = "news-reader"
priority = 10

[[routes]]
name = "deploy-keywords"
rule.keywords = ["deploy", "server", "restart", "systemd"]
target = "devops"
priority = 20

[[routes]]
name = "pr-pattern"
rule.regex = "PR #\\d+"
target = "code-reviewer"
priority = 30

[[routes]]
name = "cron-source"
rule.source_kind = "cron"
target = "scheduler"
priority = 40

[[routes]]
name = "ipc-tasks"
rule.field_equals = { field = "metadata.kind", value = "task" }
target = "task-router"
priority = 50

fallback = "marketing-lead"
```

---

## Hot-reload behavior

1. `notify` watcher monitors `{workspace}/pipelines/` directory.
2. On file change: parse TOML, validate schemas, compare with current in-memory definitions.
3. If valid: replace pipeline definition for new runs. Log `INFO pipeline reloaded: {name} v{old} -> v{new}`.
4. If invalid: log `WARN pipeline reload failed: {name}: {error}`. Keep old definition.
5. **Currently running pipeline runs always complete on the definition version they started with.** New runs pick up the new definition.
6. If a pipeline TOML is deleted: no new runs can start. Running runs complete normally. Log `WARN pipeline removed: {name}`.

---

## Checkpointing and recovery

### What is persisted

After each step completion, `PipelineContext` is serialized and stored via `RunStorePort`:

- `run_id` + `pipeline_name` + `current_step`
- accumulated `data` (all step outputs so far)
- `step_history` (completed steps with timing and status)
- `state` (Running, WaitingForApproval, etc.)

### Recovery on restart

1. On daemon startup, `PipelineService` queries `RunStorePort` for runs in non-terminal state.
2. For each incomplete run: load `PipelineContext`, resolve the pipeline definition (must still exist).
3. If `state == WaitingForAgent`: re-dispatch the current step to the agent via IPC.
4. If `state == WaitingForApproval`: resume waiting (approval state is in `ApprovalPort`).
5. If `state == WaitingForFanOut`: check which branches completed, re-dispatch missing ones.
6. If `state == Running`: re-execute from `current_step`.

### What happens if pipeline definition changed during downtime

If the pipeline TOML was modified while a run was in progress:
- The run resumes with the **original** definition (version stored in context).
- Log `WARN pipeline run {run_id} resuming with stale definition v{old}, current is v{new}`.
- Operator can cancel and restart if desired.

---

## Integration with existing systems

### Tool execution (ToolMiddleware)

Single hook point in `crates/adapters/core/src/agent/loop_.rs` → `execute_one_tool`:

```
Before: tool_name + args → execute → result
After:  tool_name + args → middleware.before() → execute → middleware.after() → result
```

If `before()` returns `Err(ToolBlock)`, the tool is **not executed**. The LLM receives a structured error explaining why (rate limited, validation failed, approval required).

Middleware chain is ordered: RateLimit → Validation → ApprovalGate → (execute) → after hooks.

### IPC (pipeline steps)

Pipeline step execution flow:

```
PipelineRunner
  → build IPC message from step definition + context data
  → DispatchIpcMessage (existing use case)
  → IPC broker delivers to agent
  → agent processes, returns result
  → IpcBusPort receives result
  → PipelineRunner validates output against schema
  → if valid: advance to next step
  → if invalid: retry or fail based on max_retries
```

### Inbound routing (MessageRouter)

Inserts before `HandleInboundMessage`:

```
InboundEnvelope arrives
  → MessageRouter evaluates rules in priority order
  → first match: set target agent_id on envelope
  → no match: use fallback agent_id
  → HandleInboundMessage proceeds with routed envelope
```

### Observer integration

New event types emitted through existing `Observer` trait:

```rust
pub enum PipelineEvent {
    PipelineStarted { run_id, pipeline_name, triggered_by },
    StepStarted { run_id, step_id, agent_id, attempt },
    StepCompleted { run_id, step_id, agent_id, duration_ms },
    StepFailed { run_id, step_id, agent_id, error, will_retry },
    StepRetrying { run_id, step_id, agent_id, attempt, backoff_secs },
    FanOutStarted { run_id, step_id, branch_count },
    FanOutBranchCompleted { run_id, step_id, branch_key },
    FanOutJoined { run_id, step_id, branch_count, duration_ms },
    ApprovalRequested { run_id, step_id, prompt },
    ApprovalReceived { run_id, step_id, approved },
    PipelineCompleted { run_id, pipeline_name, duration_ms, step_count },
    PipelineFailed { run_id, pipeline_name, error, last_step },
    PipelineCancelled { run_id, pipeline_name, reason },
    PipelineReloaded { pipeline_name, old_version, new_version },
    PipelineReloadFailed { pipeline_name, error },
    ToolBlocked { run_id, tool_name, reason: ToolBlock },
    MessageRouted { envelope_id, rule_name, target_agent },
}
```

---

## Slices (implementation order)

### Slice 1: Pipeline core — domain types + TOML loading

**Scope**: PipelineDefinition, PipelineStep, StepTransition, PipelineContext, PipelineState, ConditionalBranch, FanOutSpec. TOML deserialization. PipelineStorePort trait. TomlPipelineLoader adapter. JSON Schema contract validation via `jsonschema` crate.

**Deliverables**:
- `domain/pipeline.rs`
- `domain/pipeline_context.rs`
- `ports/pipeline_store.rs`
- `fork_adapters/pipeline/toml_loader.rs`
- `fork_adapters/pipeline/schema_validator.rs`
- Unit tests: TOML parsing, schema validation, conditional branch evaluation

**Acceptance**: can load the `content-creation.toml` example, validate step contracts, evaluate conditional branches.

### Slice 2: IPC bridge — step execution through broker

**Scope**: PipelineRunner core loop. Execute a step by sending IPC message via `DispatchIpcMessage`, receive result via `IpcBusPort`. Advance pipeline state. Sequential pipeline (Next transitions only). Checkpointing after each step via `RunStorePort`.

**Deliverables**:
- `application/services/pipeline_service.rs`
- `application/use_cases/start_pipeline.rs`
- `fork_adapters/pipeline/ipc_step_executor.rs`
- Integration with `RunStorePort` for persistence

**Acceptance**: can run a 2-step sequential pipeline (agent A → agent B) through IPC broker with checkpointing.

### Slice 3: ToolMiddleware — before/after hooks

**Scope**: ToolMiddlewarePort trait. ToolCallContext. ToolBlock enum. RateLimitMiddleware, ValidationMiddleware, ApprovalGateMiddleware. Hook into `execute_one_tool` in agent loop.

**Deliverables**:
- `domain/tool_middleware.rs`
- `ports/tool_middleware.rs`
- `application/services/tool_middleware_service.rs`
- `fork_adapters/middleware/rate_limit.rs`
- `fork_adapters/middleware/validation.rs`
- `fork_adapters/middleware/approval_gate.rs`
- Modification: `crates/adapters/core/src/agent/loop_.rs` → `execute_one_tool`

**Acceptance**: rate-limited tool call returns structured error to LLM. Approval gate pauses execution.

### Slice 4: FanOut + Join — parallel step execution

**Scope**: FanOutSpec processing. tokio::JoinSet for parallel IPC dispatch. Join step that merges results under `fanout.<key>` namespace. Timeout handling. Partial failure modes (require_all flag).

**Deliverables**:
- FanOut handling in `pipeline_service.rs`
- `PipelineState::WaitingForFanOut` transitions
- Timeout and partial failure logic

**Acceptance**: can run the `parallel-research.toml` example. Both branches execute concurrently, results merge, draft step receives combined data.

### Slice 5: Checkpointing — resume after crash

**Scope**: Recovery logic on daemon startup. Query `RunStorePort` for incomplete runs. Re-dispatch current step. Handle stale pipeline definitions. Logging and warnings.

**Deliverables**:
- `application/use_cases/resume_pipeline.rs`
- Startup recovery in `pipeline_service.rs`
- `application/use_cases/cancel_pipeline.rs`

**Acceptance**: kill daemon mid-pipeline, restart, pipeline resumes from last checkpoint.

### Slice 6: MessageRouter — deterministic routing

**Scope**: MessageRouterPort trait. Route, RoutingRule types. TOML loading for routing rules. Rule evaluation chain. Integration with `HandleInboundMessage`.

**Deliverables**:
- `domain/routing.rs`
- `ports/message_router.rs`
- `application/services/routing_service.rs`
- `application/use_cases/route_inbound.rs`
- `fork_adapters/routing/rule_chain.rs`
- Routing TOML loader

**Acceptance**: inbound message with `/research` prefix routes to news-reader, not broadcast.

### Slice 7: WaitForApproval — human-in-the-loop

**Scope**: Adapt existing `ApprovalPort` + `RequestApproval` use case for pipeline context. Pipeline pauses at WaitForApproval transition. Approval webhook/IPC triggers resume. Denied → routes to denied step.

**Deliverables**:
- WaitForApproval handling in `pipeline_service.rs`
- `PipelineState::WaitingForApproval` persistence and resume
- Integration with `ApprovalPort`

**Acceptance**: pipeline pauses at approval step, operator approves via existing mechanism, pipeline continues to next step.

### Slice 8: Hot-reload — filesystem watcher

**Scope**: `notify` crate watcher on `{workspace}/pipelines/`. Parse, validate, swap definitions. Running pipelines unaffected. Error logging for invalid TOML. Reload events through Observer.

**Deliverables**:
- `fork_adapters/pipeline/hot_reload.rs`
- Watcher startup in daemon
- PipelineReloaded / PipelineReloadFailed events

**Acceptance**: edit TOML while daemon runs, new pipeline runs use updated definition, running pipeline completes on old definition.

### Slice 9: Nested pipelines — pipeline as step

**Scope**: SubPipeline transition type. Depth tracking and `max_depth` enforcement. Parent pipeline pauses while sub-pipeline runs. Sub-pipeline output becomes parent step output.

**Deliverables**:
- SubPipeline handling in `pipeline_service.rs`
- Depth tracking in `PipelineContext`
- Recursion limit enforcement

**Acceptance**: pipeline A calls pipeline B as a step, B completes, A continues with B's output. Depth > max_depth returns error.

### Slice 10: Observability — pipeline events

**Scope**: PipelineEvent enum. Emit events at each pipeline/step lifecycle point. Observer receives and logs. Structured event data for debugging.

**Deliverables**:
- `ports/pipeline_observer.rs`
- PipelineEvent integration in `pipeline_service.rs`
- Observer adapter

**Acceptance**: `journalctl` shows structured pipeline events for a complete run.

---

## Verification checklist

Phase 4.1 is successful when all are true:

1. A multi-step workflow (research → draft → review → publish) executes deterministically without LLM deciding step order.
2. Step output that does not match JSON Schema is rejected; pipeline retries or fails, agent cannot proceed with invalid data.
3. Tool calls are rate-limited per run; feedback loops between agents are impossible.
4. Inbound messages route to the correct agent by deterministic rules, not LLM classification.
5. Daemon crash mid-pipeline → restart → pipeline resumes from last completed step.
6. Editing pipeline TOML while daemon runs → new runs use new definition, running runs complete on old.
7. FanOut executes parallel branches concurrently, join merges results.
8. Pipeline can trigger sub-pipeline with depth limit.
9. Pipeline events visible in daemon logs via Observer.
10. Zero new storage dependencies; all state in existing rusqlite via RunStorePort.

---

## Risks

1. **IPC latency per step** — each step is an IPC round-trip. Mitigation: pipelines are inherently async/batch, latency is acceptable.
2. **Schema validation overhead** — `jsonschema` validation on every step transition. Mitigation: schemas are small, validation is microseconds.
3. **Agent doesn't return valid JSON** — LLM output may not match schema. Mitigation: retry with max_retries, include schema in agent prompt.
4. **Hot-reload race condition** — file partially written. Mitigation: parse, validate, only swap if valid; fsync on write.
5. **Nested pipeline depth bomb** — pipeline A calls B calls A. Mitigation: max_depth limit (default 5) + pipeline name cycle detection.
6. **ToolMiddleware bypass** — new tool injection path skips middleware. Mitigation: single hook point in `execute_one_tool`, same as SYNAPSECLAW_ALLOWED_TOOLS boundary.

---

## Relationship to the roadmap

- Phase 4.0 made the **platform architecture** modular and port-driven.
- Phase 4.1 makes **agent coordination** deterministic and contract-driven.
- Phase 4.2 can then tackle federated execution / multi-host placement on top of a reliable pipeline engine.

Phase 4.1 is the product phase that makes multi-agent workflows usable rather than aspirational.
