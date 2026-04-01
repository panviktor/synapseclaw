# IPC Phase 4.3 Progress

**Status**: IN PROGRESS — Slices 1-5+7 complete (PR #217), Slices 6+8+9 deferred to next pass

Phase 4.1H2B: pure hexagonal architecture | **Phase 4.3: memory architecture (SurrealDB)** | Phase 4.4: TBD

---

## Goal

Replace the current flat Memory backend (SQLite key-value + optional embeddings) with a full-featured agent memory system built on **SurrealDB 3.0 embedded**: episodic memory, knowledge graph with bitemporal facts, skill learning, core memory blocks (MemGPT pattern), reflection/self-improvement, and cross-agent memory sharing via IPC.

---

## Checklist

### New dependencies

| Item | Status | Notes |
|------|--------|-------|
| `surrealdb` crate (`kv-surrealkv`) added to workspace | **DONE** | Non-optional dependency in synapse_memory, pure Rust |
| Embedding model (nomic-embed-text via llama.cpp) deployed | TODO | Separate from main Qwen3.5 instance |

### Domain types (crates/domain)

| Item | Status | PRs |
|------|--------|-----|
| `domain/memory.rs` — MemoryEntry, Visibility, Entity, TemporalFact | **DONE** | #217 Slice 1 |
| `domain/memory.rs` — Skill, CoreMemoryBlock, Reflection, ReflectionOutcome | **DONE** | #217 Slice 1 |
| `domain/memory.rs` — MemoryQuery, SearchResult, SearchSource, HybridSearchResult | **DONE** | #217 Slice 1 |
| `domain/memory.rs` — MemoryError (Storage, AccessDenied, NotFound, Embedding) | **DONE** | #217 Slice 1 |
| `domain/memory.rs` — SkillUpdate, ConsolidationReport | **DONE** | #217 Slice 1 |

### Ports (crates/domain)

| Item | Status | PRs |
|------|--------|-----|
| `ports/memory.rs` — `WorkingMemoryPort` (core blocks: get, update, append) | **DONE** | #217 Slice 1 |
| `ports/memory.rs` — `EpisodicMemoryPort` (store, get_recent, get_session, search) | **DONE** | #217 Slice 1 |
| `ports/memory.rs` — `SemanticMemoryPort` (entities, facts, traverse, search) | **DONE** | #217 Slice 1 |
| `ports/memory.rs` — `SkillMemoryPort` (store, find, update, get) | **DONE** | #217 Slice 1 |
| `ports/memory.rs` — `ReflectionPort` (store, get_relevant, get_failure_patterns) | **DONE** | #217 Slice 1 |
| `ports/memory.rs` — `ConsolidationPort` (run, recalculate_importance, gc) | **DONE** | #217 Slice 1 |
| `ports/memory.rs` — `UnifiedMemoryPort` (facade: hybrid_search + embed + convenience) | **DONE** | #217 Slice 1 |

### SurrealDB adapter (crates/adapters/memory)

| Item | Status | PRs |
|------|--------|-----|
| `SurrealMemoryAdapter::new()` — SurrealKV init + schema application | **DONE** | #217 Slice 1 |
| SurrealQL schema — 6 tables: episode, core_memory, entity, fact, skill, reflection | **DONE** | #217 Slice 1 |
| HNSW vector indexes on tables | DEFERRED | Requires embedding pipeline first (Slice 8) |
| BM25 full-text search indexes on episode content | **DONE** | #217 Slice 1 |
| `WorkingMemoryPort` impl — core_memory CRUD + upsert | **DONE** | #217 Slice 1 |
| `EpisodicMemoryPort` impl — episode storage + BM25 search | **DONE** | #217 Slice 1 |
| `SemanticMemoryPort` impl — entity resolution + temporal facts + 1-hop traversal | **DONE** | #217 Slice 1 |
| `SkillMemoryPort` impl — skill CRUD + text search | **DONE** | #217 Slice 1 |
| `ReflectionPort` impl — reflection storage + pattern retrieval | **DONE** | #217 Slice 1 |
| `ConsolidationPort` impl — importance decay + GC (stubs for run_consolidation) | **DONE** | #217 Slice 1 |
| `UnifiedMemoryPort` impl — cross-tier search + convenience ops | **DONE** | #217 Slice 1 |
| Entity resolution: exact case-insensitive match | **DONE** | #217 Slice 1 |
| Entity resolution: embedding similarity (>0.85 threshold) | DEFERRED | Requires vector indexes (Slice 8) |
| Bitemporal fact management: invalidation on update | **DONE** | #217 Slice 1 |
| Bitemporal fact management: automatic conflict detection | DEFERRED | Needs consolidation worker (Slice 5) |
| `NoopUnifiedMemory` for tests and `backend=none` | **DONE** | #217 Slice 1 |
| `create_memory()` factory — creates SurrealDB or Noop from config | **DONE** | #217 Slice 2 |
| Embedding provider factory with env key resolution | **DONE** | #217 Slice 2 |

### Deleted code (old backends)

| Item | Status | Notes |
|------|--------|-------|
| Removed sqlite.rs, postgres.rs, qdrant.rs, markdown.rs, lucid.rs, none.rs | **DONE** | ~4900 LOC removed |
| Removed backend.rs, traits.rs, snapshot.rs, hygiene.rs, knowledge_graph.rs | **DONE** | Backend classification, SQLite maintenance |
| Removed `Memory` trait (ports/memory_backend.rs) | **DONE** | Replaced by 7 specialized ports |
| Removed `MemoryTiersPort` | **DONE** | Replaced by UnifiedMemoryPort |
| Migrated ~30 consumer files to new ports | **DONE** | agent, gateway, channels, tools, infra, onboard |

### Memory tools (crates/adapters/tools)

| Item | Status | PRs |
|------|--------|-----|
| `memory_store` — rewrite to use UnifiedMemoryPort | **DONE** | #217 Slice 1 |
| `memory_recall` — rewrite to use UnifiedMemoryPort | **DONE** | #217 Slice 1 |
| `memory_forget` — rewrite to use UnifiedMemoryPort | **DONE** | #217 Slice 1 |
| `core_memory_update` — new tool: replace/append core blocks (MemGPT pattern) | **DONE** | #217 Slice 2 |
| `knowledge` tool — stub (entity search via SemanticMemoryPort) | **DONE** | #217 Slice 1 |
| Tool registration in `crates/adapters/core/src/tools/mod.rs` | **DONE** | #217 Slice 2 |
| `memory_search` — full hybrid search (vector + BM25 + graph + skills) | DEFERRED | Needs RRF fusion (Slice 7) |

### Context injection (crates/adapters/core)

| Item | Status | PRs |
|------|--------|-----|
| Memory loader rewrite: `DefaultMemoryLoader` → UnifiedMemoryPort | **DONE** | #217 Slice 1 |
| `load_core_blocks()` — XML-tagged core blocks for system prompt | **DONE** | #217 Slice 2 |
| Core memory blocks injected into every agent prompt (system message) | PARTIAL | load_core_blocks() exists, not yet wired into prompt builder |
| Relevant memories injected via recall on each turn | **DONE** | Existing recall path works with new ports |
| Relevant skills injected when task is received | DEFERRED | Needs skill learning (Slice 4) |

### Knowledge graph + entity extraction

| Item | Status | PRs |
|------|--------|-----|
| `EntityExtractor` — LLM-driven extraction (entities + relationships) | **DONE** | #217 Slice 3 |
| `store_extraction()` — persists to SemanticMemoryPort | **DONE** | #217 Slice 3 |
| Entity extraction wired into consolidation pipeline (Phase 3 block) | **DONE** | #217 Slice 3 |
| Entity resolution in SurrealDB (case-insensitive name match) | **DONE** | #217 Slice 1 |
| Entity resolution: fuzzy embedding similarity | DEFERRED | Needs vector indexes (Slice 8) |
| Temporal fact updates with conflict detection | DEFERRED | Needs consolidation worker (Slice 5) |
| Graph traversal: 1-hop via SurrealQL | **DONE** | #217 Slice 1 |
| Graph traversal: multi-hop (N hops) | DEFERRED | Needs SurrealQL RELATE (future slice) |

### Skill learning + reflection

| Item | Status | PRs |
|------|--------|-----|
| `skill_learner::reflect_on_run()` — LLM-driven post-run analysis | **DONE** | #217 Slice 4 |
| Reflection prompt: what_worked, what_failed, lesson, should_create_skill | **DONE** | #217 Slice 4 |
| Skill creation/update from reflection output | **DONE** | #217 Slice 4 |
| Success/fail counter tracking per skill | **DONE** | #217 Slice 4 |
| PipelineEngine hook (wire reflect_on_run into pipeline runner) | DEFERRED | Needs pipeline runner access |

### Consolidation worker

| Item | Status | PRs |
|------|--------|-----|
| `spawn_consolidation_worker()` — background tokio task | **DONE** | #217 Slice 5 |
| Entity extraction from unprocessed episodes (batch 50) | DEFERRED | Needs batch processing logic |
| Importance decay: `importance *= 0.95` for entries >7d old | **DONE** | #217 Slice 1+5 |
| GC: delete entries with importance <0.05 and age >30d | **DONE** | #217 Slice 1+5 |
| Consolidation interval configurable (default: 1h) | **DONE** | #217 Slice 5 |

### Memory sharing via IPC

| Item | Status | PRs |
|------|--------|-----|
| `MemoryEvent` IPC message kind (EntityDiscovered, FactEstablished, etc.) | TODO | Slice 6 |
| ACL checks: Private / SharedWith / Global visibility | TODO | Slice 6 |
| Cross-agent read queries with ACL enforcement | TODO | Slice 6 |

### Embeddings

| Item | Status | PRs |
|------|--------|-----|
| `EmbeddingProvider` trait + `OpenAiEmbedding` + `NoopEmbedding` | **DONE** | Preserved from old codebase |
| `default_base_url_for_provider()` helper | **DONE** | #217 Slice 2 |
| `create_embedding_provider()` factory with env key resolution | **DONE** | #217 Slice 2 |
| `LlamaCppEmbedder` — HTTP client for local llama.cpp server | TODO | Slice 8 |
| HNSW vector indexes in SurrealDB schema | TODO | Slice 8 |
| Embedding cache (LRU, configurable size) | TODO | Slice 8 |

### Migration

| Item | Status | PRs |
|------|--------|-----|
| Data migration: existing SQLite brain.db → SurrealDB | TODO | Slice 9 |
| Markdown snapshot import (MEMORY_SNAPSHOT.md → SurrealDB) | TODO | Slice 9 |
| Config migration: MemoryConfig backend="surrealdb" option | N/A | SurrealDB is now the only backend |

---

## Slices (implementation order)

| Slice | Description | Status | PRs |
|-------|-------------|--------|-----|
| 1 | SurrealDB embedded + schema + 7 ports + adapter + 30 consumer migrations | **DONE** | #217 |
| 2 | SurrealDB wiring + core_memory_update tool + memory_loader core blocks | **DONE** | #217 |
| 3 | Knowledge graph: EntityExtractor + consolidation pipeline integration | **DONE** | #217 |
| 4 | Skill learning: SkillLearner + reflect_on_run() + skill CRUD | **DONE** | #217 |
| 5 | Consolidation worker: background tokio task + importance decay + GC | **DONE** | #217 |
| 6 | Memory sharing via IPC + ACL + MemoryEvent | DEFERRED | Depends on IPC broker |
| 7 | Hybrid search: RRF fusion + weighted merge | **DONE** | #217 |
| 8 | Embeddings: HNSW indexes + llama.cpp local + embedding cache | DEFERRED | Infra (next pass) |
| 9 | Migration: SQLite → SurrealDB + snapshot import | DEFERRED | Infra (next pass) |

---

## Critical acceptance criteria

| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | Core memory blocks present in every agent prompt | PARTIAL | load_core_blocks() done, prompt wiring pending |
| 2 | Hybrid search returns vector + BM25 + graph results with RRF fusion | PARTIAL | BM25 works, vector + RRF deferred to Slice 7-8 |
| 3 | Entity extraction creates knowledge graph from conversations | **DONE** | Wired into consolidation pipeline |
| 4 | Bitemporal facts: old facts invalidated when contradicted | PARTIAL | Manual invalidation works, auto-detection deferred |
| 5 | Skills created from pipeline reflections, success/fail tracked | **DONE** | SkillLearner + reflect_on_run() |
| 6 | Consolidation worker runs without blocking agent loops | **DONE** | spawn_consolidation_worker() |
| 7 | Cross-agent memory sharing respects ACL (Private/SharedWith/Global) | TODO | Slice 6 |
| 8 | Data migration from existing SQLite preserves all memories | TODO | Slice 9 |
| 9 | 6 concurrent agents read/write without deadlocks | EXPECTED | SurrealKV MVCC, not yet load-tested |
| 10 | Embedding latency <50ms for local model | TODO | Slice 8 |

---

## Deferred items (next pass)

| Item | Reason | Depends on |
|------|--------|------------|
| HNSW vector indexes in SurrealDB | Needs embedding pipeline first | Slice 8 |
| Entity resolution via embedding similarity (>0.85) | Needs vector indexes | Slice 8 |
| Multi-hop graph traversal via SurrealQL RELATE | Needs RELATE syntax validation | Future |
| Automatic fact conflict detection | Needs consolidation worker | Slice 5 |
| `memory_search` tool with full RRF hybrid search | RRF function done, tool needs vector source | Slice 8 |
| Core blocks wiring into prompt builder | Needs build_context() refactor | Follow-up |
| `consolidate_turn()` calling LLM from UnifiedMemoryPort | Architecture constraint: memory crate has no Provider dependency; consolidation runs from adapter layer | Follow-up |
| Skill injection into agent context on task receive | Needs skill learning pipeline | Slice 4 |
| Multi-host memory federation | Requires distributed SurrealDB (not embedded) | Out of scope |
| Memory visualization in web UI | UX phase, not engine phase | Out of scope |
| Automatic skill versioning with A/B testing | Needs stable skill pipeline first | Out of scope |
| Memory compression / distillation | Optimization, not MVP | Out of scope |

---

## Architecture notes

### Consolidation flow
`handle_inbound_message` → `mem.consolidate_turn()` → stub in SurrealDB adapter. Real LLM consolidation runs via `consolidation::consolidate_turn(provider, model, mem, ...)` from adapter layer (channels/gateway). Entity extraction is Phase 3 of that pipeline. The port method is a thin hook; actual LLM work happens in the adapter layer where Provider is available.

### SurrealDB deserialization
SurrealDB 3.x uses `SurrealValue` trait instead of `serde::Deserialize` for `.take()`. We use `serde_json::Value` as escape hatch (it has a built-in `SurrealValue` impl), then convert to domain types via helper functions (`row_to_entry`, `row_to_entity`, etc.).

---

## Review checkpoints

### Checkpoint A — foundation (after slices 1-2) ✅

- [x] SurrealDB initialized with full schema
- [x] Core memory blocks work (get/update/append)
- [x] Episode storage + retrieval works
- [x] Memory tools functional with new backend

### Checkpoint B — knowledge (after slices 3-4)

- [x] Entities extracted from conversations
- [x] Facts stored with bitemporal semantics
- [ ] Skills created from pipeline reflections
- [x] Graph traversal returns related entities

### Checkpoint C — intelligence (after slices 5-7)

- [ ] Consolidation worker running in background
- [ ] Memory sharing via IPC operational
- [ ] Hybrid search with RRF fusion works across all memory types

### Checkpoint D — production (after slices 8-9)

- [ ] Local embeddings deployed
- [ ] Migration from SQLite complete
- [ ] All acceptance criteria met
