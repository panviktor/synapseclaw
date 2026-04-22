# Unified Memory / Prompt-Assembly Architecture

## Context

Memory enrichment and post-turn learning are fragmented across three code paths (web, channels, websocket), each with its own assembly logic, budget defaults, session scoping, and learning gates. This causes: web storing enriched (bloated) messages in history, skills/entities gated on episodic recall, web reflection ignoring tool usage, continuation turns losing all memory, and inconsistent budget policies. The fix unifies these into a single domain-level mechanism.

## Scope

This plan covers **web (gateway/ws)** and **channel (handle_inbound_message)** prompt assembly and post-turn learning. Explicitly **out of scope** for now:
- **CLI token-compaction** (`crates/adapters/core/src/agent/loop_/cli_run.rs:640`) — stays as-is, CLI is a different interaction model with its own compaction loop.
- **Daemon background optimizer** (`crates/adapters/core/src/daemon/mod.rs:364`) — daemon-only auto-improve remains separate; it operates on already-consolidated memory, not on turn assembly.
- **CLI prompt assembly** — CLI uses `Agent::turn()` too, so it benefits from Phase 1c/2e automatically, but CLI-specific compaction and interactive flow are untouched.

Future work may unify CLI compaction and daemon optimizer under the same `PromptBudget`, but that requires a separate plan.

---

## Phase 1 — Fix three worst bugs (3 independent PRs)

### 1a. WebSocket reflection: pass collected tool_history

**File**: `crates/adapters/core/src/gateway/ws.rs`
- At ~line 1063, `reflect_on_turn(user_msg, response, &[])` passes empty tools.
- Extract tool names from `tool_history` (collected at ~line 969) the same way channels do from `[Used tools: ...]`.
- Add `fn extract_tool_names(history: &[ConversationMessage]) -> Vec<String>` helper.
- Pass result to `reflect_on_turn()`.

### 1b. Decouple skills/entities from episodic recall

**File**: `crates/adapters/core/src/agent/memory_loader.rs`
- Remove `if has_recall {` guard at ~line 91. Always query skills/entities from user_message.

**File**: `crates/domain/src/application/use_cases/handle_inbound_message.rs`
- At ~line 342, change `if !recall_ctx.is_empty()` to unconditionally load skills/entities.

### 1c. Raw user message in web history

**File**: `crates/adapters/core/src/agent/agent.rs`
- At ~lines 697-705, stop storing enriched (recall_context + timestamp + message) in history.
- Store raw user message in `self.history`.
- Recall context stays as **ephemeral user-prefix** for the current provider call only (not system message — system slot is reserved for core blocks which have highest priority). After `to_provider_messages()` produces the final payload, the prefix is not persisted to history.
- Implementation: build `provider_user_message = format!("{recall_context}\n{user_message}")` for the provider call, but push only `user_message` to `self.history`.

---

## Phase 2 — Unified `TurnContextAssembler` (single PR, main change)

### Layer separation (hexagonal boundary)

**Domain layer** (`crates/domain/`) owns:
- `TurnMemoryContext` — structured data (core blocks, entries, skills, entities)
- `PromptBudget` — budget value object
- `assemble_turn_context()` — pure data assembly, no formatting
- `ContinuationPolicy` — decides what to load on continuation turns

**Adapter layer** (`crates/adapters/core/`) owns:
- `TurnContextFormatter` — converts `TurnMemoryContext` → prompt strings (system segments, user prefix)
- XML/markdown formatting, string truncation, prompt-specific layout

This keeps domain free of prompt/formatting concerns.

### 2a. New domain types

**New file**: `crates/domain/src/application/services/turn_context.rs`

```rust
/// Structured memory context for a single LLM turn.
/// Pure data — no formatting, no prompt strings.
pub struct TurnMemoryContext {
    pub core_blocks: Vec<CoreMemoryBlock>,
    pub recalled_entries: Vec<MemoryEntry>,
    pub skills: Vec<Skill>,
    pub entities: Vec<Entity>,
}

/// Token/char budget for turn context assembly.
pub struct PromptBudget {
    pub recall_max_entries: usize,          // default: 5
    pub recall_entry_max_chars: usize,      // default: 800
    pub recall_total_max_chars: usize,      // default: 4_000
    pub recall_min_relevance: f64,          // default: 0.4
    pub skills_max_count: usize,            // default: 3
    pub skills_total_max_chars: usize,      // default: 2_000
    pub entities_max_count: usize,          // default: 3
    pub entities_total_max_chars: usize,    // default: 1_500
    pub enrichment_total_max_chars: usize,  // default: 8_000 — hard cap on all enrichment combined
}

/// What to load on continuation turns (turn N>1 in a session).
pub enum ContinuationPolicy {
    /// Core blocks only — cheapest, no recall/skills/entities.
    CoreOnly,
    /// Core blocks + lightweight recall (reduced budget).
    CorePlusRecall { recall_max_entries: usize },
    /// Full context — same as first turn.
    Full,
}
```

### 2b. Assembler function

```rust
pub async fn assemble_turn_context(
    mem: &dyn UnifiedMemoryPort,
    user_message: &str,
    agent_id: &str,
    session_id: Option<&str>,
    budget: &PromptBudget,
    continuation: Option<&ContinuationPolicy>,
) -> TurnMemoryContext
```

- Core blocks: always loaded.
- If `continuation == Some(CoreOnly)`: return early with only core blocks.
- If `continuation == Some(CorePlusRecall { n })`: recall with reduced limit `n`, skip skills/entities.
- Otherwise (first turn or `Full`): full recall + skills + entities, all independent queries.
- Skills: `mem.find_skills(user_message, budget.skills_max_count)` — independent of recall.
- Entities: `mem.search_entities(user_message, budget.entities_max_count)` — independent of recall.

### 2c. Adapter-layer formatter

**New file**: `crates/adapters/core/src/agent/turn_context_fmt.rs`

```rust
pub struct FormattedTurnContext {
    pub core_blocks_system: String,   // for system prompt (highest priority)
    pub enrichment_prefix: String,    // ephemeral user-prefix for provider call only
}

/// Format TurnMemoryContext into prompt-injectable strings.
/// Applies char budgets, XML wrapping, truncation.
pub fn format_turn_context(
    ctx: &TurnMemoryContext,
    budget: &PromptBudget,
) -> FormattedTurnContext
```

Consolidates XML formatting from `memory_loader.rs:103-124` and `handle_inbound_message.rs:351-371`.

### 2d. PromptBudget in config

**File**: `crates/domain/src/config/schema.rs`
- Add `PromptBudgetConfig` sub-struct to `MemoryConfig`.
- All fields have defaults matching current hardcoded values. Backward-compatible.
- Add `continuation_policy` field: `"core_only"` | `"core_plus_recall"` | `"full"` (default: `"core_plus_recall"`).

### 2e. Migrate callers

**`crates/adapters/core/src/agent/agent.rs`**:
- Replace `self.memory_loader.load_context()` with `assemble_turn_context()` + `format_turn_context()`.
- Remove `memory_loader` field. Core blocks also handled by assembler.
- `turn()`: assemble → format → push raw user msg to history → use `enrichment_prefix` for provider call only.

**`crates/domain/src/application/use_cases/handle_inbound_message.rs`**:
- Replace `MemoryContext` branch (lines 303-401) with `assemble_turn_context()`.
- Replace `CoreBlocksOnly` branch (lines 403-421) with `assemble_turn_context(..., Some(&ContinuationPolicy))` using the configured `continuation_policy`. This is NOT "always full context" — the default `CorePlusRecall` loads core blocks + lightweight recall (2 entries) without skills/entities, keeping continuation turns cheap. Operators can tune via config.

**`crates/adapters/core/src/agent/memory_loader.rs`**:
- Mark `MemoryLoader` trait and `DefaultMemoryLoader` as `#[deprecated]`.

---

## Phase 3 — Session scoping + unified post-turn (2 independent PRs)

### 3a. Web memory_session_id bound to session_key

**File**: `crates/adapters/core/src/gateway/ws.rs`
- Set `memory_session_id` every time agent is materialized for a session — both in `ensure_session()` (create path, ~line 466) and in the restore/resume path.
- The setter at `crates/adapters/core/src/agent/agent.rs:313` already exists; add callsites in both create and restore branches.
- Episodic recall becomes session-scoped; core/skills/entities remain agent-scoped.

### 3b. Unified post-turn learning policy

**New file**: `crates/domain/src/application/services/post_turn.rs`

```rust
pub struct PostTurnPolicy {
    pub should_consolidate: bool,
    pub should_reflect: bool,
    pub tools_used: Vec<String>,
}

pub fn decide_post_turn(
    auto_save_enabled: bool,
    user_message: &str,
    assistant_response: &str,
    tools_used: &[String],
) -> PostTurnPolicy
```

Consolidates duplicated gates from `ws.rs:1043-1065` and `handle_inbound_message.rs:592-633`.

**Migrate**: both `ws.rs` and `handle_inbound_message.rs` call `decide_post_turn()` then execute.

---

## Phase 4 — Cleanup

- Replace `HistoryEnrichment::CoreBlocksOnly` with delegation to `ContinuationPolicy` (the enum variant may stay but its handling is now a single `assemble_turn_context()` call with policy).
- Delete deprecated `memory_loader.rs`.
- Consolidate `RecallConfig` into `PromptBudget` (add `impl From<&PromptBudget> for RecallConfig` or replace).
- Update `MemoryService::recall_context()` / `format_recall_context()` to use `PromptBudget`.

---

## Dependency Graph

```
1a (fix empty tools)     ─────────────────────────────► 3b (post-turn policy)
1b (decouple skills)     ──► 2 (TurnContextAssembler) ──► 4 (cleanup)
1c (raw user in history) ──┘
3a (session scoping)     ─────────────────────────────────┘
```

Phases 1a, 1b, 1c, 3a are independently shippable.

## Critical Files

| File | Phases |
|------|--------|
| `crates/adapters/core/src/gateway/ws.rs` | 1a, 3a, 3b |
| `crates/adapters/core/src/agent/agent.rs` | 1c, 2e |
| `crates/adapters/core/src/agent/memory_loader.rs` | 1b, 2e, 4 |
| `crates/domain/src/application/use_cases/handle_inbound_message.rs` | 1b, 2e, 3b |
| `crates/domain/src/application/services/inbound_message_service.rs` | 4 |
| `crates/domain/src/application/services/memory_service.rs` | 4 |
| `crates/domain/src/config/schema.rs` | 2d |
| `crates/domain/src/domain/memory.rs` | 4 |
| `crates/adapters/core/src/agent/turn_context_fmt.rs` | 2c (new) |
| `crates/domain/src/application/services/turn_context.rs` | 2a, 2b (new) |
| `crates/domain/src/application/services/post_turn.rs` | 3b (new) |

## Reuse

- `MemoryService::format_recall_context()` — move formatting logic to adapter-layer `turn_context_fmt.rs`
- `RecallConfig` defaults — mirror in `PromptBudget` defaults
- `CORE_MEMORY_MARKER` pattern in `agent.rs` — reuse for core block injection
- `should_autosave()` / `should_consolidate()` from `MemoryService` — reuse in `PostTurnPolicy`

## Verification

1. `cargo test -q -p synapse_domain --lib` — domain tests pass (includes inbound_message_service tests)
2. `cargo test -p synapse_adapters --lib` — adapter tests pass (after fixing existing test-harness issues)
3. `cargo clippy --all-targets -- -D warnings` — no warnings
4. Manual: send messages via web dashboard and Telegram, verify:
   - Web history shows raw user messages (not enriched)
   - Skills load even when recall is empty
   - Reflection logs show tool names (not empty)
   - Continuation turns in Telegram get core blocks + lightweight recall (not full context)
   - Long Telegram sessions don't bloat prompt (check token counts in logs)
5. `./dev/ci.sh all` — full CI passes
