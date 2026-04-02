---
name: ipc-context
description: "Load full project context for a new session. Reads delta registry, plans, progress, memory architecture, and current state. Use at the start of any session that will touch IPC, memory, fork plans, or reviews. Trigger on: 'загрузи контекст', 'что мы делаем', 'контекст', 'catch me up', 'where are we', 'new session'."
user-invocable: true
---

# Project Context Loader

Load the full project context so this session understands SynapseClaw: IPC system, memory architecture, security, and what's next.

## Step 1: Read core documents

Read these files in parallel:

- `docs/fork/README.md` — doc index and branch model
- `docs/fork/delta-registry.md` — all fork deltas (56+ entries), shared hotspots
- `docs/fork/news.md` — changelog with latest features

## Step 2: Read current phase context

Read these files to understand the current work:

- `docs/fork/ipc-phase4_3-progress.md` — Phase 4.3 Memory Architecture (COMPLETE)
- `docs/fork/memory-architecture.md` — how memory actually works (data flows, SurrealDB schema, tools)
- `docs/fork/ipc-phase4_3-scope.md` — what was deleted/replaced/kept

## Step 3: Read key code files

Read these key files (first 50 lines each is enough for orientation):

- `crates/domain/src/ports/memory.rs` — 7 memory ports (UnifiedMemoryPort facade)
- `crates/adapters/memory/src/surrealdb_adapter.rs` — SurrealDB embedded backend
- `crates/adapters/core/src/memory_adapters/memory_adapter.rs` — ConsolidatingMemory wrapper
- `crates/adapters/core/src/memory_adapters/entity_extractor.rs` — LLM entity extraction
- `crates/adapters/core/src/memory_adapters/skill_learner.rs` — Skill learning from reflections
- `crates/adapters/core/src/agent/loop_/mod.rs` — build_context() with core blocks + recall + skills + entities
- `crates/adapters/core/src/gateway/ipc/mod.rs` — IPC broker (handlers, ACL, audit)
- `crates/domain/src/config/schema.rs` — MemoryConfig, AgentsIpcConfig

## Step 4: Check git state

Run:
```bash
git log --oneline -10
git status
git branch --show-current
```

## Step 5: Present summary

Output a concise summary:

```
## Project Context

### Architecture
- Hexagonal: 12 workspace crates, pure domain (zero infra deps)
- SurrealDB 3.0 embedded: single memory backend (replaced 6 old backends)
- 7 memory ports: Working, Episodic, Semantic, Skill, Reflection, Consolidation, Unified
- Self-improvement: entity extraction → knowledge graph → prompt enrichment
- Skill learning: reflect_on_run → create/update skills → inject in context
- MemGPT pattern: core blocks (persona, user_knowledge, task_state, domain) always in prompt
- IPC broker with L0-L4 trust, Ed25519 signing, quarantine, ACL
- 6 agents: main broker + marketing-lead + copywriter + news-reader + publisher + trend-aggregator

### Memory System
- Backend: SurrealDB embedded (kv-surrealkv, pure Rust)
- Embeddings: OpenRouter Qwen3 Embedding 8B (4096 dims)
- Search: BM25 + HNSW vector → RRF fusion
- Consolidation: LLM extraction (history + facts + entities) fire-and-forget
- Tools: memory_store, memory_recall, memory_forget, core_memory_update, knowledge
- Monitoring: InstrumentedMemory wrapper (latency tracking)

### Phase Status
- Phases 1-3.12 (IPC): DONE
- Phase 4.0 (modular core): DONE
- Phase 4.1 (pipeline engine): DONE
- Phase 4.1H (hexagonal extraction): DONE
- Phase 4.3 (memory architecture): DONE — PRs #217-#221

### Current branch: {branch}
### Recent commits: {last 3}
### Uncommitted changes: {yes/no}
```

## Step 6: Ask what to do

After presenting context, ask:

> Контекст загружен. Что делаем?

## Arguments

- No args: full context load
- `brief`: skip code reading, just docs + git state
- `code`: skip docs, focus on current code state + git
