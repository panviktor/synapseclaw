# IPC Phase 4.3 Progress

**Status**: NOT STARTED

Phase 4.1H2B: pure hexagonal architecture | **Phase 4.3: memory architecture (SurrealDB)** | Phase 4.4: TBD

---

## Goal

Replace the current flat Memory backend (SQLite key-value + optional embeddings) with a full-featured agent memory system built on **SurrealDB 3.0 embedded**: episodic memory, knowledge graph with bitemporal facts, skill learning, core memory blocks (MemGPT pattern), reflection/self-improvement, and cross-agent memory sharing via IPC.

---

## Checklist

### New dependencies

| Item | Status | Notes |
|------|--------|-------|
| `surrealdb` crate (`kv-surrealkv` feature) added to workspace | TODO | Pure-Rust LSM engine, single-binary friendly |
| Embedding model (nomic-embed-text via llama.cpp) deployed | TODO | Separate from main Qwen3.5 instance |

### Domain types (crates/domain)

| Item | Status | PRs |
|------|--------|-----|
| `domain/memory.rs` — MemoryEntry, MemoryType, Visibility, Entity, TemporalFact | TODO | |
| `domain/memory.rs` — Skill, CoreMemoryBlock, Reflection, ReflectionOutcome | TODO | |
| `domain/memory.rs` — MemoryQuery, SearchResult, SearchSource, HybridSearchResult | TODO | |
| `domain/memory.rs` — MemoryError (Storage, AccessDenied, NotFound, Embedding) | TODO | |
| `domain/memory.rs` — SkillUpdate, ConsolidationReport | TODO | |

### Ports (crates/domain)

| Item | Status | PRs |
|------|--------|-----|
| `ports/memory.rs` — `WorkingMemoryPort` (core blocks: get, update, append) | TODO | |
| `ports/memory.rs` — `EpisodicMemoryPort` (store, get_recent, get_session, search) | TODO | |
| `ports/memory.rs` — `SemanticMemoryPort` (entities, facts, traverse, search) | TODO | |
| `ports/memory.rs` — `SkillMemoryPort` (store, find, update, get) | TODO | |
| `ports/memory.rs` — `ReflectionPort` (store, get_relevant, get_failure_patterns) | TODO | |
| `ports/memory.rs` — `ConsolidationPort` (run, recalculate_importance, gc) | TODO | |
| `ports/memory.rs` — `UnifiedMemoryPort` (facade: hybrid_search + embed) | TODO | |

### SurrealDB adapter (crates/adapters/memory)

| Item | Status | PRs |
|------|--------|-----|
| `MemoryEngine::new()` — SurrealKV init + schema application | TODO | |
| SurrealQL schema — 7 tables: agent, episode, entity, fact, skill, core_memory, reflection | TODO | |
| HNSW vector indexes (DIMENSION 1024, COSINE) on episode, entity, fact, skill, reflection | TODO | |
| BM25 full-text search indexes on episode, entity, skill | TODO | |
| `WorkingMemoryPort` impl — core_memory CRUD | TODO | |
| `EpisodicMemoryPort` impl — episode storage + hybrid search | TODO | |
| `SemanticMemoryPort` impl — entity resolution + temporal facts + graph traversal | TODO | |
| `SkillMemoryPort` impl — skill CRUD + vector search | TODO | |
| `ReflectionPort` impl — reflection storage + pattern retrieval | TODO | |
| `ConsolidationPort` impl — importance decay + GC | TODO | |
| `UnifiedMemoryPort` impl — RRF fusion (vector + BM25 + graph) | TODO | |
| Entity resolution: exact match + embedding similarity (>0.85 threshold) | TODO | |
| Bitemporal fact management: conflict detection + invalidation | TODO | |

### Memory tools (crates/adapters/tools)

| Item | Status | PRs |
|------|--------|-----|
| `memory_store` — rewrite to use UnifiedMemoryPort + embedding | TODO | |
| `memory_search` — hybrid search (vector + BM25 + graph + skills) | TODO | |
| `core_memory_update` — new tool: replace/append core blocks (MemGPT pattern) | TODO | |
| `memory_forget` — rewrite to use UnifiedMemoryPort | TODO | |
| Tool registration in `crates/adapters/core/src/tools/mod.rs` | TODO | |

### Context injection (crates/adapters/core)

| Item | Status | PRs |
|------|--------|-----|
| Core memory blocks injected into every agent prompt (system message) | TODO | |
| Relevant memories injected as `<relevant_memories>` section | TODO | |
| Relevant skills injected when task is received | TODO | |
| Memory loader rewrite: `DefaultMemoryLoader` → UnifiedMemoryPort | TODO | |

### Knowledge graph + entity extraction

| Item | Status | PRs |
|------|--------|-----|
| `EntityExtractor` — LLM-driven extraction (entities + relationships) | TODO | |
| Entity resolution in SurrealDB (exact + fuzzy embedding match) | TODO | |
| Temporal fact updates with conflict detection | TODO | |
| Graph traversal via SurrealQL `->fact->entity` | TODO | |

### Skill learning + reflection

| Item | Status | PRs |
|------|--------|-----|
| `SkillLearner.reflect_on_run()` — hook into PipelineEngine | TODO | |
| Reflection prompt: what_worked, what_failed, lesson, should_create_skill | TODO | |
| Skill creation/update from reflection output | TODO | |
| Success/fail counter tracking per skill | TODO | |

### Consolidation worker

| Item | Status | PRs |
|------|--------|-----|
| `ConsolidationWorker` — background tokio task in daemon | TODO | |
| Entity extraction from unprocessed episodes (batch 50) | TODO | |
| Importance decay: `importance *= 0.95` for entries >7d old | TODO | |
| GC: delete entries with importance <0.05 and age >30d | TODO | |
| Consolidation interval configurable (default: 1h) | TODO | |

### Memory sharing via IPC

| Item | Status | PRs |
|------|--------|-----|
| `MemoryEvent` IPC message kind (EntityDiscovered, FactEstablished, etc.) | TODO | |
| ACL checks: Private / SharedWith / Global visibility | TODO | |
| Cross-agent read queries with ACL enforcement | TODO | |

### Embeddings

| Item | Status | PRs |
|------|--------|-----|
| `LlamaCppEmbedder` — HTTP client for local llama.cpp server | TODO | |
| `EmbeddingProvider` trait integration with SurrealDB adapter | TODO | |
| Embedding cache (LRU, configurable size) | TODO | |

### Migration

| Item | Status | PRs |
|------|--------|-----|
| Data migration: existing SQLite brain.db → SurrealDB | TODO | |
| Markdown snapshot import (MEMORY_SNAPSHOT.md → SurrealDB) | TODO | |
| Config migration: MemoryConfig backend="surrealdb" option | TODO | |

---

## Slices (implementation order)

| Slice | Description | Status | PRs |
|-------|-------------|--------|-----|
| 1 | SurrealDB embedded + schema + WorkingMemoryPort + EpisodicMemoryPort | TODO | |
| 2 | Memory tools rewrite + core_memory_update + context injection | TODO | |
| 3 | Knowledge graph: SemanticMemoryPort + entity extraction + temporal facts | TODO | |
| 4 | Skill learning: SkillMemoryPort + ReflectionPort + PipelineEngine hook | TODO | |
| 5 | Consolidation worker + importance decay + GC | TODO | |
| 6 | Memory sharing via IPC + ACL + MemoryEvent | TODO | |
| 7 | Hybrid search: RRF fusion + UnifiedMemoryPort facade | TODO | |
| 8 | Local embeddings (llama.cpp) + embedding cache | TODO | |
| 9 | Migration: SQLite → SurrealDB + config + snapshot import | TODO | |

---

## Critical acceptance criteria

| # | Criterion | Status | Notes |
|---|-----------|--------|-------|
| 1 | Core memory blocks present in every agent prompt | TODO | |
| 2 | Hybrid search returns vector + BM25 + graph results with RRF fusion | TODO | |
| 3 | Entity extraction creates knowledge graph from conversations | TODO | |
| 4 | Bitemporal facts: old facts invalidated when contradicted | TODO | |
| 5 | Skills created from pipeline reflections, success/fail tracked | TODO | |
| 6 | Consolidation worker runs without blocking agent loops | TODO | |
| 7 | Cross-agent memory sharing respects ACL (Private/SharedWith/Global) | TODO | |
| 8 | Data migration from existing SQLite preserves all memories | TODO | |
| 9 | 6 concurrent agents read/write without deadlocks | TODO | |
| 10 | Embedding latency <50ms for local model | TODO | |

---

## Review checkpoints

### Checkpoint A — foundation (after slices 1-2)

- SurrealDB initialized with full schema
- Core memory blocks work (get/update/append)
- Episode storage + retrieval works
- Memory tools functional with new backend

### Checkpoint B — knowledge (after slices 3-4)

- Entities extracted from conversations
- Facts stored with bitemporal semantics
- Skills created from pipeline reflections
- Graph traversal returns related entities

### Checkpoint C — intelligence (after slices 5-7)

- Consolidation worker running in background
- Memory sharing via IPC operational
- Hybrid search with RRF fusion works across all memory types

### Checkpoint D — production (after slices 8-9)

- Local embeddings deployed
- Migration from SQLite complete
- All acceptance criteria met

---

## Deferred

| Item | Reason |
|------|--------|
| Multi-host memory federation | Requires distributed SurrealDB (not embedded) |
| Memory visualization in web UI | UX phase, not engine phase |
| Automatic skill versioning with A/B testing | Needs stable skill pipeline first |
| Memory compression / distillation | Optimization, not MVP |
