# Memory Architecture — How It Actually Works

Phase 4.3 implementation. This document describes the real data flows, not aspirations.

---

## Overview

SynapseClaw uses **SurrealDB 3.0 embedded** as the single memory backend. Five memory subsystems:

| Subsystem | What it stores | SurrealDB table | Always in prompt? |
|-----------|---------------|-----------------|-------------------|
| **Working Memory** | Core blocks (persona, user_knowledge, task_state, domain) | `core_memory` | Yes |
| **Episodic Memory** | Conversation turns, tool calls, autosaved messages | `episode` | Via recall |
| **Semantic Memory** | Knowledge graph: entities + bitemporal facts | `entity`, `fact` | No |
| **Procedural Memory** | Learned skills from pipeline reflections | `skill` | Via injection |
| **Reflective Memory** | Post-run analyses (what_worked, what_failed, lesson) | `reflection` | No |

Data directory: `workspace_dir/memory/brain.surreal`

---

## Flow 1: Message Arrives

```
User sends message via Telegram/Discord/CLI/WebSocket
       |
       v
handle_inbound_message.rs
       |
       +-- #5: Autosave (if auto_save=true AND content >= 20 chars)
       |        |
       |        v  mem.store(key, content, Conversation, session_id)
       |        |
       |        v  SurrealDB: CREATE episode + generate embedding
       |
       +-- #8: Memory context injection (before LLM call)
       |        |
       |        v  build_context() → core blocks + recall + skills
       |
       +-- #14: LLM generates response
       |
       +-- #18: Consolidation (fire-and-forget via tokio::spawn)
                |
                v  ConsolidatingMemory.consolidate_turn()
                |
                +-- Phase 1: LLM extracts history_entry + memory_update
                |   |  history → Daily category
                |   |  facts   → Core category
                |
                +-- Phase 3: Entity extraction (best-effort)
                    |  LLM extracts entities + relationships
                    |  upsert_entity() → entity table
                    |  add_fact() → fact table (bitemporal)
```

### Autosave details

- Key format: `channel:{conversation_key}:user`
- Skipped if: content < 20 chars, starts with `[cron:`, contains `DISTILLED_INDEX_SIG:`, or `should_skip_autosave()` returns true
- Embedding generated inline if embedding provider configured (dimensions > 0)

### Consolidation details

- LLM call with `temperature=0.1` for deterministic extraction
- Turn text truncated to 4000 chars (UTF-8 boundary safe)
- JSON output: `{"history_entry": "...", "memory_update": "..." | null}`
- Fallback on malformed JSON: raw turn text as history_entry
- Entity extraction prompt: extracts `{entities: [...], relationships: [...]}` with confidence scores

---

## Flow 2: Agent Needs Context

```
build_context(mem, user_msg, min_relevance, session_id)
       |
       +-- 1. Core memory blocks (MemGPT pattern)
       |       |
       |       v  mem.get_core_blocks("default")
       |       |
       |       v  Output: <persona>...\n</persona>\n<user_knowledge>...\n</user_knowledge>
       |
       +-- 2. Relevant memories (recall)
       |       |
       |       v  mem.recall(user_msg, 5, session_id)
       |       |
       |       v  SurrealDB: delegates to search_episodes() (BM25 + HNSW vector + RRF fusion)
       |       |
       |       v  Filter: score >= min_relevance_score (default 0.4)
       |       |  Filter: skip assistant_autosave keys, cron noise, tool_result blocks
       |       |
       |       v  Output: [Memory context]\n- key: content\n...
       |
       +-- 3. Relevant skills
               |
               v  mem.find_skills(MemoryQuery { text: user_msg, limit: 3 })
               |
               v  SurrealDB: CONTAINS text search on skill name/description
               |
               v  Output: <skill name="...">...\n</skill>
```

### Search pipeline

When embedding provider is configured:

1. **BM25 search**: `SELECT *, search::score(1) AS bm25_score FROM episode WHERE content @1@ $text`
2. **Vector search**: Embed query → `SELECT *, vector::similarity::cosine(embedding, $emb) AS vec_score FROM episode WHERE embedding <|K,64|> $emb`
3. **RRF fusion**: `score = 1/(60 + rank_bm25) + 1/(60 + rank_vector)` — deduplicates, sorts by fused score

When no embedding provider: BM25 only.

---

## Flow 3: Agent Stores Memory (Tool)

```
Agent calls memory_store(key="user_lang", content="Prefers Rust", category="core")
       |
       v  SecurityPolicy check (ToolOperation::Act)
       |
       v  mem.store(key, content, &Core, None)
       |
       v  SurrealDB: CREATE episode SET ... embedding = embed_one(content)
```

Categories: `core` (permanent), `daily` (session notes), `conversation` (chat), custom.

---

## Flow 4: Agent Updates Core Blocks (Tool)

```
Agent calls core_memory_update(label="user_knowledge", action="append", content="Likes Rust")
       |
       v  SecurityPolicy check
       |
       v  mem.update_core_block("default", "user_knowledge", content)
       |   OR mem.append_core_block("default", "user_knowledge", text)
       |
       v  SurrealDB: conditional CREATE/UPDATE on core_memory
       |   WHERE agent_id = "default" AND label = "user_knowledge"
```

Labels: `persona`, `user_knowledge`, `task_state`, `domain`. Max 2000 tokens each.

---

## Flow 5: Background Consolidation Worker

```
Daemon startup → spawn_consolidation_worker(mem, config, "default")
       |
       v  tokio::spawn — runs forever
       |
       every 1 hour:
       |
       +-- recalculate_importance()
       |       SELECT count WHERE created_at < now() - 7d AND importance > 0.1
       |       UPDATE episode SET importance *= 0.95 ...
       |
       +-- gc_low_importance(threshold=0.05, max_age=30d)
               SELECT count WHERE importance < 0.05 AND created_at < now() - 30d
               DELETE FROM episode WHERE ...
```

Worker is wrapped in `ConsolidatingMemory` in daemon — has access to provider for LLM calls.

---

## Flow 6: Knowledge Graph

### Automatic (via consolidation)

Every conversation turn → entity extraction → `store_extraction()`:
- Entities: `{name, type, summary}` → `upsert_entity()` (case-insensitive merge)
- Relationships: `{subject, predicate, object, confidence}` → `add_fact()` (bitemporal)

### Manual (via knowledge tool)

| Action | What happens |
|--------|-------------|
| `search` | `find_entity(name)` + `get_current_facts(entity_id)` |
| `add_entity` | `upsert_entity()` — CREATE or UPDATE by name |
| `add_fact` | Resolve both entities by name → `add_fact()` |
| `get_facts` | `find_entity()` → `get_current_facts()` WHERE valid_to IS NONE |

Fact invalidation: when a contradicting fact arrives, old one gets `valid_to = now()`.

---

## Flow 7: Embedding Pipeline

### What gets embedded

| Event | Content embedded | Where stored |
|-------|-----------------|--------------|
| `store_episode()` | Episode content | `episode.embedding` |
| `search_episodes()` | Query text | Not stored (query-time) |

### When embeddings are generated

- **On store**: If `embedder.dimensions() > 0`, `embed_one(content)` is called inline (async)
- **On search**: If no `query.embedding` provided, `embed_one(query.text)` called
- **Cached**: `CachedEmbeddingProvider` wraps provider with 10K-entry LRU in-memory cache

### Provider configuration (`config.toml`)

```toml
[memory]
embedding_provider = "openrouter"           # or "openai", "llama.cpp", "custom:URL", "none"
embedding_model = "openai/text-embedding-3-small"
embedding_dimensions = 1536
```

| Provider | Config value | API key env var | Notes |
|----------|-------------|-----------------|-------|
| OpenAI | `"openai"` | `OPENAI_API_KEY` | text-embedding-3-small (1536d) |
| OpenRouter | `"openrouter"` | `OPENROUTER_API_KEY` | Same models via OpenRouter |
| llama.cpp | `"llama.cpp"` | None needed | Local server on :8081 |
| llama.cpp (custom) | `"llama.cpp:http://host:port"` | None | Remote llama-server |
| Custom | `"custom:https://api.example.com"` | Caller API key | Any OpenAI-compatible |
| Disabled | `"none"` | — | BM25 keyword search only |

### HNSW indexes

SurrealDB tables with vector indexes (DIMENSION 768, COSINE):
- `episode.embedding`
- `entity.embedding`
- `skill.embedding`
- `reflection.embedding`

---

## Config Reference

```toml
[memory]
backend = "surrealdb"            # "surrealdb" or "none"
auto_save = true                 # autosave conversation messages

# Embeddings
embedding_provider = "none"      # see provider table above
embedding_model = "text-embedding-3-small"
embedding_dimensions = 1536

# Search tuning
vector_weight = 0.7              # weight for vector similarity in hybrid merge
keyword_weight = 0.3             # weight for BM25 keyword score
min_relevance_score = 0.4        # threshold for including in context

# Response cache (optional, separate from memory)
response_cache_enabled = false
response_cache_ttl_minutes = 60
response_cache_max_entries = 5000
response_cache_hot_entries = 256
```

---

## SurrealDB Schema (6 tables)

| Table | Key indexes | Rows per agent |
|-------|------------|----------------|
| `episode` | agent_id, session_id, key, BM25(content), HNSW(embedding) | Thousands |
| `core_memory` | UNIQUE(agent_id, label) | 4 (persona, user_knowledge, task_state, domain) |
| `entity` | name, entity_type, HNSW(embedding) | Hundreds |
| `fact` | subject, object, predicate | Hundreds |
| `skill` | name, HNSW(embedding) | Tens |
| `reflection` | agent_id, HNSW(embedding) | Tens |

---

## Port Architecture (7 traits)

```
UnifiedMemoryPort (facade)
  ├── WorkingMemoryPort     → core_memory table
  ├── EpisodicMemoryPort    → episode table
  ├── SemanticMemoryPort    → entity + fact tables
  ├── SkillMemoryPort       → skill table
  ├── ReflectionPort        → reflection table
  └── ConsolidationPort     → importance decay + GC
```

`ConsolidatingMemory` wrapper adds LLM consolidation on top of any `UnifiedMemoryPort` impl. Used in production paths (channels, gateway, agent, daemon).

---

## Agent Tools (5 memory tools)

| Tool | Port | What it does |
|------|------|-------------|
| `memory_store` | UnifiedMemoryPort | Store key-value fact with category |
| `memory_recall` | UnifiedMemoryPort | Search memories by query |
| `memory_forget` | UnifiedMemoryPort | Delete memory by key |
| `core_memory_update` | WorkingMemoryPort | Edit always-in-prompt blocks (MemGPT) |
| `knowledge` | SemanticMemoryPort | Search/add entities and facts |

---

## CLI Commands

```bash
synapseclaw memory list [--category core] [--limit 20]
synapseclaw memory get <key>
synapseclaw memory stats
synapseclaw memory clear --key <key> [--yes]
```
