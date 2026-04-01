# Phase 4.3 — Скоп замены системы памяти

## Context

Текущая система памяти — плоский key-value store с 6 бэкендами (SQLite, Postgres, Qdrant, Markdown, Lucid, None), двумя портами (`Memory` + `MemoryTiersPort`) и 3 инструментами агента. Phase 4.3 заменяет её на SurrealDB embedded с knowledge graph, skill learning, MemGPT core blocks и cross-agent sharing.

Задача этого плана — зафиксировать **что удаляется, что заменяется, что остаётся** перед началом реализации.

---

## УДАЛЯЕТСЯ (безвозвратно) — ~4900 LOC

### Бэкенды (все 6 реализаций `Memory` trait):

| Файл | LOC | Причина удаления |
|------|-----|------------------|
| `crates/adapters/memory/src/sqlite.rs` | ~1000 | SurrealDB заменяет SQLite: hybrid search, FTS, WAL — всё SQLite-specific |
| `crates/adapters/memory/src/postgres.rs` | 394 | Внешний Postgres не нужен при embedded SurrealDB |
| `crates/adapters/memory/src/qdrant.rs` | 643 | SurrealDB имеет native HNSW vector index |
| `crates/adapters/memory/src/markdown.rs` | 356 | Fallback для юзеров без SQLite — не нужен с embedded DB |
| `crates/adapters/memory/src/lucid.rs` | 676 | Экспериментальный CLI bridge, SurrealDB поглощает |
| `crates/adapters/memory/src/none.rs` | 88 | Перепишется под новые порты (см. ЗАМЕНЯЕТСЯ) |

### Инфраструктура (привязана к старым бэкендам):

| Файл | LOC | Причина удаления |
|------|-----|------------------|
| `crates/adapters/memory/src/backend.rs` | 175 | `MemoryBackendKind` enum для 6 бэкендов — мёртвый код при одном бэкенде |
| `crates/adapters/memory/src/snapshot.rs` | ~300 | Прямые SQL-запросы к `rusqlite` для export/hydrate |
| `crates/adapters/memory/src/hygiene.rs` | ~300 | Архивация/purge через `rusqlite` + filesystem — SurrealDB получит свой retention |
| `crates/adapters/memory/src/knowledge_graph.rs` | ~200 | SQLite knowledge graph — заменяется native SurrealDB graph |

### Domain:

| Файл | LOC | Причина удаления |
|------|-----|------------------|
| `crates/domain/src/ports/memory_backend.rs` | 51 | Старый flat `Memory` trait — заменяется 7 новыми портами |

### Factory (частично):

| Файл | Что удаляется |
|------|---------------|
| `crates/adapters/memory/src/lib.rs` | Функции `create_memory()`, `create_memory_with_storage()`, `create_memory_with_storage_and_routes()`, `create_memory_for_migration()` и вся логика выбора бэкенда (~400 LOC из 716) |

### Тесты:

| Файл | Причина |
|------|---------|
| `tests/integration/memory_restart.rs` | Тесты SqliteMemory dedup |
| `tests/integration/memory_comparison.rs` | SQLite vs Markdown сравнение |

---

## ЗАМЕНЯЕТСЯ (та же концепция, новая реализация)

### Domain порты:

| Файл | Было | Станет |
|------|------|--------|
| `crates/domain/src/ports/memory.rs` | `MemoryTiersPort` (1 trait, session+long-term+consolidation) | 7 портов: `WorkingMemoryPort`, `EpisodicMemoryPort`, `SemanticMemoryPort`, `SkillMemoryPort`, `ReflectionPort`, `ConsolidationPort`, `UnifiedMemoryPort` |

### Core adapter:

| Файл | Было | Станет |
|------|------|--------|
| `crates/adapters/core/src/memory_adapters/memory_adapter.rs` (151 LOC) | `MemoryTiersAdapter` wraps `dyn Memory` + `ConversationStorePort` | `SurrealMemoryAdapter` implements all 7 ports, backed by single SurrealDB connection |
| `crates/adapters/core/src/memory_adapters/consolidation.rs` (179 LOC) | 2-phase LLM extraction (history + facts) | Enhanced: + entity extraction + skill identification + entity resolution |
| `crates/adapters/core/src/memory_adapters/cli.rs` (364 LOC) | CLI через `dyn Memory` | CLI через новые порты + команды для core blocks и skills |
| `crates/adapters/core/src/agent/memory_loader.rs` (244 LOC) | `DefaultMemoryLoader` вызывает `Memory::recall()` | Вызывает `UnifiedMemoryPort` + инжектит core memory blocks (MemGPT) |

### Инструменты агентов:

| Файл | Было | Станет |
|------|------|--------|
| `crates/adapters/tools/src/memory_store.rs` (226 LOC) | `memory_store` через `dyn Memory` | `memory_store` через `SemanticMemoryPort` |
| `crates/adapters/tools/src/memory_recall.rs` (168 LOC) | `memory_recall` — keyword search | `memory_search` — hybrid search через `UnifiedMemoryPort` |
| `crates/adapters/tools/src/memory_forget.rs` (181 LOC) | `memory_forget` через `dyn Memory` | `memory_forget` через `SemanticMemoryPort` |
| *(новый)* | — | `core_memory_update` — update/append core blocks (MemGPT) |

### Factory:

| Файл | Было | Станет |
|------|------|--------|
| `crates/adapters/memory/src/lib.rs` | Multi-backend factory с 6 ветвями | Single-backend factory: `create_surrealdb_memory()` + `create_noop_memory()` |

### Конфигурация:

| Файл | Что меняется |
|------|-------------|
| `crates/domain/src/config/schema.rs` (`MemoryConfig`) | **Удаляются**: `backend` (enum→bool), `sqlite_open_timeout_secs`, `snapshot_*`, `hygiene_*`, `archive_*`, `purge_*`, `conversation_retention_days`, `qdrant` section. **Добавляются**: `surrealdb_path`, `core_memory_enabled`, `skill_learning_enabled`, `retention_ttl_days` |

### Потребители (~20 файлов — смена типа `dyn Memory` → новые порты):

| Файл | Изменение |
|------|-----------|
| `core/gateway/mod.rs` | `AppState.mem: Arc<dyn Memory>` → `Arc<dyn UnifiedMemoryPort>` |
| `core/gateway/api.rs` | `/api/memory` endpoints → новые порты |
| `core/channels/mod.rs` | Создание памяти + wiring MemoryTiersAdapter |
| `core/agent/agent.rs` | `Agent.memory: Arc<dyn Memory>` → новые порты |
| `core/agent/loop_/cli_run.rs` | Factory вызов |
| `core/tools/mod.rs` | Регистрация: 3 старых → 4 новых |
| `domain/use_cases/handle_inbound_message.rs` | `MemoryTiersPort` → новые порты |
| `domain/services/memory_service.rs` | Сигнатуры port types |
| Тесты: `agent/tests.rs`, `tools/mod.rs`, `support/helpers.rs` | Mock backends |

---

## ОСТАЁТСЯ (as-is или минимальные изменения)

| Файл | LOC | Почему остаётся |
|------|-----|----------------|
| `crates/adapters/memory/src/embeddings.rs` | ~300 | `EmbeddingProvider` trait, `OpenAiEmbedding` — backend-agnostic, SurrealDB тоже нужны embeddings |
| `crates/adapters/memory/src/vector.rs` | 403 | Чистая математика: `cosine_similarity`, `hybrid_merge`, `vec_to_bytes` — полезна для re-ranking |
| `crates/adapters/memory/src/chunker.rs` | 378 | Markdown chunking — пригодится для ingestion в SemanticMemory |
| `crates/adapters/memory/src/response_cache.rs` | 527 | Независим от memory backend, свой SQLite, потребляется Agent напрямую |
| `crates/adapters/memory/src/traits.rs` | 7 | Re-export — обновить на новые порты |
| `crates/domain/src/domain/memory.rs` | 137 | Domain types — расширяются (новые категории, temporal fields), не удаляются |
| `crates/domain/src/application/services/memory_service.rs` | 263 | Бизнес-логика autosave/recall/consolidation — backend-agnostic |
| `crates/adapters/core/src/memory_adapters/summary_generator_adapter.rs` | 33 | Wraps Provider as SummaryGeneratorPort — не связан с memory backend |

**Важно**: `rusqlite` остаётся в зависимостях `synapse_memory` — его использует `response_cache.rs`.

---

## Зависимости Cargo.toml

**synapse_memory Cargo.toml changes:**
- **Добавить**: `surrealdb = { version = "3", features = ["kv-surrealkv"] }`
- **Удалить**: `postgres` (optional), `tokio-postgres` (optional)
- **Оставить**: `rusqlite` (для response_cache), `reqwest` (для embeddings), `sha2` (для response_cache), остальное
- **Удалить feature**: `memory-postgres`

---

## Миграция данных

Существующие юзеры имеют `brain.db` (SQLite). Нужен one-time migration tool:
1. Читает все строки из SQLite `memories` table
2. Вставляет в SurrealDB с маппингом схемы
3. Мигрирует knowledge graph nodes/edges если есть
4. Fallback: `MEMORY_SNAPSHOT.md` → hydrate в SurrealDB

---

## Итого

| Категория | Файлов | ~LOC |
|-----------|--------|------|
| Удаляется | 12 | ~4900 |
| Заменяется | 10 | ~1900 |
| Остаётся | 8 | ~2050 |
| Новый код (Phase 4.3) | ~15 | ~4000-6000 (estimate) |
| Потребители (обновить типы) | ~20 | Точечные правки |

---

## Verification

После реализации:
1. `cargo fmt --all -- --check` + `cargo clippy --all-targets -- -D warnings`
2. `cargo test` — все существующие тесты (кроме удалённых memory_restart/memory_comparison) проходят
3. Новые тесты: SurrealDB CRUD, hybrid search, entity extraction, core blocks, skill learning
4. `./dev/ci.sh all` — full pre-PR validation
5. Manual: проверить что агенты стартуют, memory tools работают через Telegram
