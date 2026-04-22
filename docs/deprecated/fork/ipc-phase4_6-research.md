# Phase 4.6 Research: Agent Product Intelligence

Companion to `ipc-phase4_6-plan.md`. Contains competitive research, root cause analysis, and implementation guidance.

---

## Part 1 — Why "What's the weather?" Fails

### Root Cause: Empty Core Blocks + Semantic Recall Gap

SynapseClaw has the right architecture (Letta-style core memory blocks) but the wrong initialization.

**What happens today:**

1. User: "What's the weather?"
2. `assemble_turn_context()` → `get_core_blocks(agent_id)` → **empty** (nobody wrote `user_knowledge`)
3. `recall("what's the weather?")` → BM25 on episodic table → **miss** ("weather" ≠ "lives in Moscow")
4. SOUL.md has "Location: Moscow" but it's in the **static system prompt**, not in `core_memory` table
5. Agent has no context → asks "Which city?"

**What should happen:**

1. On first boot: SOUL.md/USER.md facts → written to `core_memory.user_knowledge` block
2. `get_core_blocks()` returns `user_knowledge = "Location: Moscow, Timezone: UTC+3"`
3. Agent sees location in system prompt every turn → queries weather for Moscow directly

### The Three Missing Components

| # | Component | What it does | Fix effort |
|---|-----------|-------------|------------|
| 1 | **Bootstrap Loader** | On agent start: parse SOUL.md → extract facts → write to `user_knowledge` core block | 1 PR |
| 2 | **Consolidation→Core Bridge** | When consolidation extracts "user lives in X" → update `user_knowledge` block | 1 PR |
| 3 | **Dialogue State** (Slice 6) | Session-scoped working memory for follow-ups, references, comparison sets | Slice 6 |

### Similar Failure Modes

| User says | Agent should know | Source |
|-----------|-------------------|--------|
| "What's the weather?" | City from user_knowledge | Core block |
| "Translate to my language" | Language preference | Core block |
| "Remind me tomorrow" | Local timezone | Core block |
| "Send it here" | Current room/thread | Conversation target (Slice 1) |
| "And the second one?" | Previous comparison set | Dialogue state (Slice 6) |
| "What did we discuss last week?" | Session history | session_search (Slice 2) |
| "Do it like last time" | Procedural memory | Skills + session_search |
| "Restart that service" | Focus entity from tool output | Dialogue state (Slice 6) |
| "Is it still failing?" | Last tool result subject | Dialogue state (Slice 6) |

---

## Part 2 — Competitive Analysis

### Letta / MemGPT

**Architecture**: Three-tier virtual memory (OS-inspired).

| Tier | Always in prompt? | Agent edits? | Size |
|------|-------------------|-------------|------|
| Core Memory (RAM) | Yes, every turn | Yes, via `core_memory_append/replace` | ~2000 tokens |
| Recall Memory (cache) | No, searched | No | Conversation history |
| Archival Memory (disk) | No, searched | Yes, via `archival_memory_insert/search` | Unlimited |

**Core blocks**:
- `persona` — agent identity, behavioral guidelines
- `human` — user name, preferences, city, timezone, response style
- Each block has label, content, and **character limit** (prevents bloat)
- Agent autonomously writes to blocks when learning new facts
- Read-only blocks possible for policies/config

**Why "weather" works**: City is in `human` block → always in system prompt → no retrieval needed.

**Key lesson**: Separate "always-available" (core) from "search-on-demand" (archival/recall). Core blocks must be curated, bounded, and self-edited by the agent.

### Hermes Agent

**Architecture**: Two-file bounded memory + session search.

| Component | Size | Loaded when? | Editable? |
|-----------|------|-------------|-----------|
| `MEMORY.md` | 2,200 chars | Session start (frozen snapshot) | Yes, via `memory` tool |
| `USER.md` | 1,375 chars | Session start (frozen snapshot) | Yes, via `memory` tool |
| Session DB | Unlimited | On-demand via `session_search` | No |

**USER.md** contains: name, role, timezone, response preferences, pet peeves, technical level.
**MEMORY.md** contains: environment facts, project conventions, discovered workarounds.

Both injected into system prompt at session start as frozen snapshot. Changes persist to disk but only appear at **next** session start (preserves LLM prefix caching).

**Orchestration tools**:
- `todo` — session-scoped task list (add/list/update/complete)
- `clarify` — structured question with optional choices (max 4)
- `send_message` — 13+ platforms, discovers targets via `list` action first
- `session_search` — FTS5 search over past sessions, LLM summarization
- `skill_manage` — auto-create skills after complex tasks (5+ tool calls)

**Memory curation**: Bounded limits force consolidation. When approaching 80% capacity, agent merges related entries. Security scanning blocks prompt injection attempts in memory writes.

**Key lesson**: Bounded memory forces quality. `clarify` tool replaces free-form guessing. `session_search` replaces "I don't remember."

### Mem0

**Architecture**: Pure retrieval-based memory layer.

Per turn:
1. `memory.search(query=message, user_id=...)` → semantic vector search
2. Results injected as context
3. `memory.add(messages, user_id=...)` → LLM extracts facts, classifies AUDN

**Three-prompt pipeline**:
1. **Fact Extraction**: Extract facts from user messages → 7 categories (preferences, personal, plans, etc.)
2. **Memory Update**: Compare new vs existing → ADD / UPDATE / DELETE / NONE
3. **Response Generation**: User prompt + retrieved memories

**Proposed but rejected**: `get_profile()` — compact 200-400 token user summary, always injected. Team closed this (issue #3528) — stayed retrieval-only.

**Key limitation**: Semantic search can miss tangential facts. "What's the weather?" has low embedding similarity to "lives in Moscow." Silent miss → agent asks.

**Key lesson**: Pure retrieval fails for tangential queries. Need an always-available tier for critical user facts.

### OpenClaw

**Architecture**: Session-first with channel-scoped context.

- Sessions are first-class (session tools: `sessions_list`, `sessions_history`, `sessions_send`)
- Channel metadata (room, user, thread) automatically available
- Gateway-owned proactive flows (restart reports, heartbeat, system events)
- Loop detection as configurable guardrail

**Key lesson**: Current conversation/session should be a first-class runtime object. The agent should never need to discover what room it's in.

### Rasa / LangGraph

**Dialogue state management**:
- Rasa: explicit slot-based dialogue state (`entities`, `slots`, `active_form`)
- LangGraph: explicit short-term thread state (typed dict, updated per node)

**Key lesson**: Short follow-ups ("and the second one?", "restart it") are **dialogue state** problems, not memory problems. Need a deterministic ephemeral state layer.

---

## Part 3 — Architecture Mapping to SynapseClaw

### What SynapseClaw already has (correct architecture)

| Letta concept | SynapseClaw equivalent | Status |
|--------------|----------------------|--------|
| Core Memory blocks | `core_memory` table (persona, user_knowledge, task_state, domain) | ✅ Schema exists, injection works (but blocks typically empty) |
| `core_memory_append/replace` | `core_memory_update` tool | ✅ Tool exists (agent must explicitly call it) |
| Archival Memory | `episode` table + vector search | ⚠️ Implemented, known recall parser gaps on some queries |
| Recall Memory | Conversation history + `recall()` | ⚠️ Implemented, misses tangential queries (no dialogue state) |
| Memory blocks API | `get_core_blocks()` / `update_core_block()` ports | ✅ Working |

| Hermes concept | SynapseClaw equivalent | Status |
|---------------|----------------------|--------|
| MEMORY.md | `core_memory.domain` + `core_memory.task_state` blocks | ✅ Available but empty |
| USER.md | `core_memory.user_knowledge` block | ❌ **Never populated** |
| `clarify` tool | — | ❌ Missing |
| `todo` tool | — | ❌ Missing |
| `session_search` | — | ❌ Missing |
| `send_message` (with current target) | — | ❌ Missing |
| Bounded limits | `max_tokens` field on core_memory | ✅ Field exists, not enforced |

### What's missing (gaps to fill)

| Layer | Gap | Fix |
|-------|-----|-----|
| **Always-available context** | `user_knowledge` core block empty | Bootstrap from SOUL.md + consolidation bridge |
| **Dialogue state** | No session-scoped working memory | Slice 6: `DialogueState` |
| **Current conversation target** | No `DeliveryTarget::CurrentConversation` | Slice 1 |
| **Orchestration tools** | No `clarify`, `todo`, `session_search`, `message_send` | Slice 2 |
| **Standing orders** | No product-native proactive flows | Slice 3 |
| **Planner guardrails** | No prerequisite validation, loop detection | Slice 4 |
| **Side questions** | No ephemeral tangent mode | Slice 5 |

---

## Part 4 — Execution Strategy

### Pre-Phase 4.6: Core Block Bootstrap (1 PR)

**Must happen before anything else.** Without populated core blocks, all slices underperform.

1. On agent startup: read SOUL.md/USER.md from workspace
2. Extract key facts: name, location, local timezone, language preference, preferences
3. Write to `core_memory.user_knowledge` via `update_core_block()`
4. Idempotent: skip if block already has content
5. Consolidation bridge: when AUDN extracts user-facing facts → append to `user_knowledge`

### Phase 4.6 Execution Order

```
Pre-req: Bootstrap user_knowledge core block
   ↓
Slice 1: CurrentConversation delivery target
   ↓
Slice 2: Orchestration tools (todo, clarify, message_send, session_search)
   ↓
Slice 6: Dialogue state + referential resolution
   ↓
Slice 4: Planner guardrails + channel tool profiles
   ↓
Slice 3: Standing orders + restart reports
   ↓
Slice 5: Side questions
```

### Memory Layer Stack (target state)

```
┌─────────────────────────────────────────────┐
│  Dialogue State (ephemeral, session-scoped)  │  Slice 6
│  focus_entities, slots, comparison_set       │
├─────────────────────────────────────────────┤
│  Core Memory Blocks (always in prompt)       │  Pre-req bootstrap
│  persona, user_knowledge, task_state, domain │
├─────────────────────────────────────────────┤
│  Episodic Recall (search on relevance)       │  Existing
│  BM25 + vector + retention scoring           │
├─────────────────────────────────────────────┤
│  Session Search (on-demand, LLM-summarized)  │  Slice 2
│  FTS over past session transcripts           │
├─────────────────────────────────────────────┤
│  Archival / Knowledge Graph                  │  Existing
│  entities, facts, skills, reflections        │
└─────────────────────────────────────────────┘
```

---

## Part 5 — Key Decisions

### 1. Core blocks should be bounded (Hermes pattern)

Enforce `max_tokens` on core blocks. Without limits, `user_knowledge` grows unbounded and consumes the context window. Hermes caps USER.md at 1,375 chars. We should cap `user_knowledge` at ~2,000 chars.

### 2. Dialogue state is ephemeral (Rasa/LangGraph pattern)

`DialogueState` lives in session memory, NOT in `core_memory` or `episode` tables. It is not promoted to long-term memory by default. It dies with the session.

### 3. `clarify` is a tool, not a prompt trick (Hermes pattern)

The agent calls `clarify` with structured options. This is better than free-form "which city?" because:
- It limits choices
- It structures the response
- It can be logged/tracked as a distinct event
- UI can render it as buttons/options

### 4. Bootstrap is idempotent (Letta pattern)

On every agent start, check if `user_knowledge` is empty. If so, populate from files. If not, skip. This handles first boot AND recovery after memory wipe.

### 5. Consolidation→core is selective (Hermes curation pattern)

Not every fact goes to core blocks. Only high-confidence user-facing facts: name, location, local timezone, language preference, stated preferences, explicit instructions. Everything else stays in episodic/archival.

---

## Sources

- Letta/MemGPT core memory: https://docs.letta.com/guides/agents/memory-blocks/
- Letta memory concepts: https://docs.letta.com/concepts/memgpt/
- MemGPT paper: https://arxiv.org/abs/2310.08560
- Hermes Agent: https://github.com/NousResearch/hermes-agent
- Hermes memory: https://hermes-agent.nousresearch.com/docs/user-guide/features/memory/
- Hermes tools: https://hermes-agent.nousresearch.com/docs/user-guide/features/tools/
- Mem0 architecture: https://github.com/mem0ai/mem0
- Mem0 prompts: https://github.com/mem0ai/mem0/blob/main/mem0/configs/prompts.py
- Mem0 profile proposal (rejected): https://github.com/mem0ai/mem0/issues/3528
- OpenClaw sessions: https://docs.openclaw.ai/session
- OpenClaw session tools: https://docs.openclaw.ai/concepts/session-tool
- Rasa dialogue management: https://rasa.com/docs/rasa/dialogue-elements/
- LangGraph state management: https://langchain-ai.github.io/langgraph/concepts/low_level/
