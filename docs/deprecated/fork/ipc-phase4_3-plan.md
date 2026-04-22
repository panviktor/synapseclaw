# Phase 4.3: Полная архитектура памяти SynapseClaw

## Практическое руководство: SurrealDB embedded + Rust traits + Self-improving Memory

**Контекст:** SynapseClaw — Rust multi-agent runtime, Phase 4.1 (Pipeline Engine) завершён. 6 равноправных агентов через IPC-брокер, Contabo VPS (48GB RAM, 18 vCPU), Qwen3.5-35B-A3B локально + API-каскад.

---

## 1. SurrealDB 3.0 Embedded — подключение и схема

### 1.1 Cargo.toml

```toml
[dependencies]
surrealdb = { version = "3", features = ["kv-surrealkv"] }
# Альтернатива: features = ["kv-rocksdb"] — более зрелый движок, но тяжелее
```

`kv-surrealkv` — собственный LSM-движок SurrealDB на чистом Rust, оптимизирован для embedded. `kv-rocksdb` — классика, тяжелее по зависимостям но проверен временем. Для SynapseClaw рекомендую начать с `surrealkv` — меньше зависимостей, лучше вписывается в single-binary философию.

### 1.2 Инициализация embedded

```rust
use surrealdb::Surreal;
use surrealdb::engine::local::SurrealKv;

pub struct MemoryEngine {
    db: Surreal<surrealdb::engine::local::Db>,
}

impl MemoryEngine {
    pub async fn new(data_dir: &str) -> anyhow::Result<Self> {
        let db = Surreal::new::<SurrealKv>(data_dir).await?;
        db.use_ns("synapseclaw").use_db("memory").await?;

        // Применяем схему при старте
        Self::apply_schema(&db).await?;

        Ok(Self { db })
    }
}
```

### 1.3 Полная SurrealQL-схема для агентной памяти

```sql
-- ═══════════════════════════════════════════
-- АГЕНТЫ И ПРАВА ДОСТУПА
-- ═══════════════════════════════════════════

DEFINE TABLE agent SCHEMAFULL;
DEFINE FIELD name       ON agent TYPE string;
DEFINE FIELD role       ON agent TYPE string;       -- "news-reader", "copywriter", "marketing-lead"
DEFINE FIELD trust_level ON agent TYPE int DEFAULT 50;
DEFINE FIELD created_at ON agent TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_agent_name ON agent FIELDS name UNIQUE;

-- ═══════════════════════════════════════════
-- ЭПИЗОДИЧЕСКАЯ ПАМЯТЬ (сырые взаимодействия)
-- ═══════════════════════════════════════════

DEFINE TABLE episode SCHEMAFULL;
DEFINE FIELD agent_id    ON episode TYPE record<agent>;
DEFINE FIELD session_id  ON episode TYPE string;
DEFINE FIELD content     ON episode TYPE string;       -- полный текст взаимодействия
DEFINE FIELD summary     ON episode TYPE option<string>;
DEFINE FIELD role        ON episode TYPE string;       -- "user" | "assistant" | "system" | "tool"
DEFINE FIELD tool_calls  ON episode TYPE option<array>; -- [{name, args, result}]
DEFINE FIELD importance  ON episode TYPE float DEFAULT 0.5;
DEFINE FIELD created_at  ON episode TYPE datetime DEFAULT time::now();
DEFINE FIELD visibility  ON episode TYPE string DEFAULT "private";
   -- "private" | "shared" | "global"
DEFINE FIELD shared_with ON episode TYPE option<array<record<agent>>>;
DEFINE FIELD embedding   ON episode TYPE option<array<float>>;

-- Индексы
DEFINE INDEX idx_ep_agent   ON episode FIELDS agent_id;
DEFINE INDEX idx_ep_session ON episode FIELDS session_id;
DEFINE INDEX idx_ep_time    ON episode FIELDS created_at;
DEFINE INDEX idx_ep_vector  ON episode FIELDS embedding
    HNSW DIMENSION 1024 DIST COSINE;
    -- 1024 для Qwen3.5 embeddings; если используешь text-embedding-3-small = 1536
DEFINE INDEX idx_ep_content ON episode FIELDS content
    SEARCH ANALYZER simple BM25;

-- ═══════════════════════════════════════════
-- СУЩНОСТИ (Knowledge Graph узлы)
-- ═══════════════════════════════════════════

DEFINE TABLE entity SCHEMAFULL;
DEFINE FIELD name        ON entity TYPE string;
DEFINE FIELD entity_type ON entity TYPE string;     -- "person", "company", "concept", "tool", "channel"
DEFINE FIELD properties  ON entity TYPE object DEFAULT {};
DEFINE FIELD summary     ON entity TYPE option<string>;
DEFINE FIELD created_at  ON entity TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at  ON entity TYPE datetime DEFAULT time::now();
DEFINE FIELD created_by  ON entity TYPE record<agent>;
DEFINE FIELD embedding   ON entity TYPE option<array<float>>;

DEFINE INDEX idx_ent_name ON entity FIELDS name;
DEFINE INDEX idx_ent_type ON entity FIELDS entity_type;
DEFINE INDEX idx_ent_vec  ON entity FIELDS embedding
    HNSW DIMENSION 1024 DIST COSINE;
DEFINE INDEX idx_ent_search ON entity FIELDS name, summary
    SEARCH ANALYZER simple BM25;

-- ═══════════════════════════════════════════
-- ФАКТЫ / РЁБРА ГРАФА (Knowledge Graph связи)
-- Битемпоральная модель: valid_from/valid_to + recorded_at
-- ═══════════════════════════════════════════

DEFINE TABLE fact SCHEMAFULL;
DEFINE FIELD subject      ON fact TYPE record<entity>;
DEFINE FIELD predicate    ON fact TYPE string;     -- "works_at", "prefers", "knows", "created_by"
DEFINE FIELD object       ON fact TYPE record<entity>;
DEFINE FIELD confidence   ON fact TYPE float DEFAULT 0.8;
-- Битемпоральность
DEFINE FIELD valid_from   ON fact TYPE datetime DEFAULT time::now();
DEFINE FIELD valid_to     ON fact TYPE option<datetime>;  -- None = текущий факт
DEFINE FIELD recorded_at  ON fact TYPE datetime DEFAULT time::now();
DEFINE FIELD invalidated_at ON fact TYPE option<datetime>;
-- Провенанс
DEFINE FIELD source_episode ON fact TYPE option<record<episode>>;
DEFINE FIELD created_by    ON fact TYPE record<agent>;
DEFINE FIELD embedding     ON fact TYPE option<array<float>>;

DEFINE INDEX idx_fact_subj ON fact FIELDS subject;
DEFINE INDEX idx_fact_obj  ON fact FIELDS object;
DEFINE INDEX idx_fact_pred ON fact FIELDS predicate;
DEFINE INDEX idx_fact_time ON fact FIELDS valid_from, valid_to;
DEFINE INDEX idx_fact_vec  ON fact FIELDS embedding
    HNSW DIMENSION 1024 DIST COSINE;

-- ═══════════════════════════════════════════
-- НАВЫКИ (Skill Learning — процедурная память)
-- ═══════════════════════════════════════════

DEFINE TABLE skill SCHEMAFULL;
DEFINE FIELD name         ON skill TYPE string;
DEFINE FIELD description  ON skill TYPE string;
DEFINE FIELD content      ON skill TYPE string;     -- Markdown с процедурой
DEFINE FIELD tags         ON skill TYPE array<string> DEFAULT [];
DEFINE FIELD success_count ON skill TYPE int DEFAULT 0;
DEFINE FIELD fail_count    ON skill TYPE int DEFAULT 0;
DEFINE FIELD version       ON skill TYPE int DEFAULT 1;
DEFINE FIELD created_by    ON skill TYPE record<agent>;
DEFINE FIELD created_at    ON skill TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at    ON skill TYPE datetime DEFAULT time::now();
DEFINE FIELD embedding     ON skill TYPE option<array<float>>;

DEFINE INDEX idx_skill_name ON skill FIELDS name;
DEFINE INDEX idx_skill_tags ON skill FIELDS tags;
DEFINE INDEX idx_skill_vec  ON skill FIELDS embedding
    HNSW DIMENSION 1024 DIST COSINE;
DEFINE INDEX idx_skill_search ON skill FIELDS name, description, content
    SEARCH ANALYZER simple BM25;

-- ═══════════════════════════════════════════
-- CORE MEMORY BLOCKS (MemGPT-паттерн)
-- Именованные блоки текста, всегда в контексте агента
-- ═══════════════════════════════════════════

DEFINE TABLE core_memory SCHEMAFULL;
DEFINE FIELD agent_id   ON core_memory TYPE record<agent>;
DEFINE FIELD label      ON core_memory TYPE string;   -- "persona", "user_knowledge", "task_state", "domain"
DEFINE FIELD content    ON core_memory TYPE string;
DEFINE FIELD max_tokens ON core_memory TYPE int DEFAULT 2000;
DEFINE FIELD updated_at ON core_memory TYPE datetime DEFAULT time::now();

DEFINE INDEX idx_cm_agent ON core_memory FIELDS agent_id, label UNIQUE;

-- ═══════════════════════════════════════════
-- РЕФЛЕКСИИ (self-improvement записи)
-- ═══════════════════════════════════════════

DEFINE TABLE reflection SCHEMAFULL;
DEFINE FIELD agent_id     ON reflection TYPE record<agent>;
DEFINE FIELD pipeline_run ON reflection TYPE option<string>;
DEFINE FIELD task_summary ON reflection TYPE string;
DEFINE FIELD outcome      ON reflection TYPE string;  -- "success" | "partial" | "failure"
DEFINE FIELD what_worked  ON reflection TYPE string;
DEFINE FIELD what_failed  ON reflection TYPE string;
DEFINE FIELD lesson       ON reflection TYPE string;
DEFINE FIELD created_at   ON reflection TYPE datetime DEFAULT time::now();
DEFINE FIELD embedding    ON reflection TYPE option<array<float>>;

DEFINE INDEX idx_refl_agent ON reflection FIELDS agent_id;
DEFINE INDEX idx_refl_vec   ON reflection FIELDS embedding
    HNSW DIMENSION 1024 DIST COSINE;
```

### 1.4 Гибридный поиск в одном SurrealQL-запросе

```sql
-- Находим релевантные воспоминания: vector + BM25 + graph traversal
-- за один запрос к SurrealDB

LET $query_emb = $embedding;  -- передаётся из Rust
LET $query_text = $text;

-- 1. Векторный поиск по эпизодам
LET $vec_results = (
    SELECT id, content, importance, created_at,
           vector::similarity::cosine(embedding, $query_emb) AS vec_score
    FROM episode
    WHERE embedding <|10,64|> $query_emb  -- top-10, ef_search=64
    AND agent_id = $agent OR visibility = "global"
    ORDER BY vec_score DESC
    LIMIT 20
);

-- 2. BM25 поиск по эпизодам
LET $bm25_results = (
    SELECT id, content, importance, created_at,
           search::score(1) AS bm25_score
    FROM episode
    WHERE content @1@ $query_text
    AND (agent_id = $agent OR visibility = "global")
    ORDER BY bm25_score DESC
    LIMIT 20
);

-- 3. Графовый обход: найти связанные сущности
LET $related_entities = (
    SELECT ->fact->entity AS connected
    FROM entity
    WHERE name @@ $query_text OR embedding <|5,32|> $query_emb
    FETCH connected
);

-- 4. Навыки по тегам и embeddings
LET $relevant_skills = (
    SELECT id, name, description, content, success_count, fail_count
    FROM skill
    WHERE embedding <|5,32|> $query_emb
    OR tags CONTAINSANY $tags
    ORDER BY success_count DESC
    LIMIT 5
);

-- Возвращаем всё вместе
RETURN {
    episodes_vector: $vec_results,
    episodes_bm25: $bm25_results,
    related_entities: $related_entities,
    skills: $relevant_skills
};
```

### 1.5 Известные ограничения SurrealDB 3.0

**Issue #6800 (nested JSON performance):** на 3+ уровнях вложенности — до 22x медленнее. Решение: хранить `properties` как плоский object, не вкладывать объекты глубоко. Для SynapseClaw это не проблема — memory entries по природе плоские.

**Concurrent access:** SurrealDB embedded использует MVCC (multi-version concurrency control). 6 агентов могут читать параллельно без блокировок. Запись — serializable через SurrealKV WAL. Для 6 агентов bottleneck маловероятен — write throughput SurrealKV на NVMe порядка 10K ops/sec.

**Нет native RRF (Reciprocal Rank Fusion):** гибридное объединение vector + BM25 делается в application layer (Rust), не в SurrealQL. Это нормально — fusion-логика 20 строк Rust.

---

## 2. Rust Trait Design для fork_core

### 2.1 Основные типы

```rust
// crates/fork_core/src/domain/memory.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type AgentId = String;
pub type MemoryId = String;
pub type SessionId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: MemoryId,
    pub agent_id: AgentId,
    pub content: String,
    pub entry_type: MemoryType,
    pub importance: f32,
    pub created_at: DateTime<Utc>,
    pub visibility: Visibility,
    pub embedding: Option<Vec<f32>>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryType {
    Episode,
    Fact,
    Skill,
    Reflection,
    CoreBlock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Visibility {
    Private,
    SharedWith(Vec<AgentId>),
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: MemoryId,
    pub name: String,
    pub entity_type: String,
    pub properties: serde_json::Value,
    pub summary: Option<String>,
    pub created_by: AgentId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalFact {
    pub id: MemoryId,
    pub subject: MemoryId,
    pub predicate: String,
    pub object: MemoryId,
    pub confidence: f32,
    pub valid_from: DateTime<Utc>,
    pub valid_to: Option<DateTime<Utc>>,  // None = current
    pub recorded_at: DateTime<Utc>,
    pub source_episode: Option<MemoryId>,
    pub created_by: AgentId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: MemoryId,
    pub name: String,
    pub description: String,
    pub content: String,  // Markdown
    pub tags: Vec<String>,
    pub success_count: u32,
    pub fail_count: u32,
    pub version: u32,
    pub created_by: AgentId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreMemoryBlock {
    pub agent_id: AgentId,
    pub label: String,  // "persona", "user_knowledge", "task_state", "domain"
    pub content: String,
    pub max_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub agent_id: AgentId,
    pub pipeline_run: Option<String>,
    pub task_summary: String,
    pub outcome: ReflectionOutcome,
    pub what_worked: String,
    pub what_failed: String,
    pub lesson: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReflectionOutcome {
    Success,
    Partial,
    Failure,
}

#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub text: String,
    pub embedding: Option<Vec<f32>>,
    pub agent_id: AgentId,
    pub include_shared: bool,
    pub time_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub entry: MemoryEntry,
    pub score: f32,        // fusion score
    pub source: SearchSource,
}

#[derive(Debug, Clone)]
pub enum SearchSource {
    Vector,
    BM25,
    Graph,
    Hybrid,
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Access denied: agent {agent} cannot {action} on {resource}")]
    AccessDenied { agent: AgentId, action: String, resource: String },
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
```

### 2.2 Port Traits

```rust
// crates/fork_core/src/ports/memory.rs

use async_trait::async_trait;

/// Рабочая память: контекст текущей задачи, всегда в промпте агента
#[async_trait]
pub trait WorkingMemoryPort: Send + Sync {
    /// Получить все core memory blocks агента (всегда включаются в контекст)
    async fn get_core_blocks(&self, agent_id: &AgentId)
        -> Result<Vec<CoreMemoryBlock>, MemoryError>;

    /// Обновить конкретный блок (агент редактирует сам через tool call)
    async fn update_core_block(&self, agent_id: &AgentId, label: &str, content: String)
        -> Result<(), MemoryError>;

    /// Добавить текст к блоку (append, не replace)
    async fn append_core_block(&self, agent_id: &AgentId, label: &str, text: &str)
        -> Result<(), MemoryError>;
}

/// Эпизодическая память: история взаимодействий
#[async_trait]
pub trait EpisodicMemoryPort: Send + Sync {
    /// Сохранить новый эпизод
    async fn store_episode(&self, entry: MemoryEntry)
        -> Result<MemoryId, MemoryError>;

    /// Последние N эпизодов агента
    async fn get_recent(&self, agent_id: &AgentId, limit: usize)
        -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Эпизоды конкретной сессии
    async fn get_session(&self, session_id: &SessionId)
        -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Гибридный поиск по эпизодам (vector + BM25)
    async fn search_episodes(&self, query: &MemoryQuery)
        -> Result<Vec<SearchResult>, MemoryError>;
}

/// Семантическая память: графовая с битемпоральными фактами
#[async_trait]
pub trait SemanticMemoryPort: Send + Sync {
    /// Создать или обновить сущность (entity resolution)
    async fn upsert_entity(&self, entity: Entity) -> Result<MemoryId, MemoryError>;

    /// Найти сущность по имени (с fuzzy matching)
    async fn find_entity(&self, name: &str) -> Result<Option<Entity>, MemoryError>;

    /// Добавить факт (ребро графа)
    async fn add_fact(&self, fact: TemporalFact) -> Result<MemoryId, MemoryError>;

    /// Инвалидировать факт (пометить valid_to = now)
    async fn invalidate_fact(&self, fact_id: &MemoryId) -> Result<(), MemoryError>;

    /// Получить текущие факты о сущности (valid_to IS NONE)
    async fn get_current_facts(&self, entity_id: &MemoryId)
        -> Result<Vec<TemporalFact>, MemoryError>;

    /// Графовый обход от сущности на N шагов
    async fn traverse(&self, entity_id: &MemoryId, hops: usize)
        -> Result<Vec<(Entity, TemporalFact)>, MemoryError>;

    /// Поиск сущностей (vector + BM25)
    async fn search_entities(&self, query: &MemoryQuery)
        -> Result<Vec<Entity>, MemoryError>;
}

/// Процедурная память: навыки
#[async_trait]
pub trait SkillMemoryPort: Send + Sync {
    /// Сохранить новый навык
    async fn store_skill(&self, skill: Skill) -> Result<MemoryId, MemoryError>;

    /// Найти релевантные навыки для задачи
    async fn find_skills(&self, query: &MemoryQuery)
        -> Result<Vec<Skill>, MemoryError>;

    /// Обновить skill после использования (success/fail counters, новая версия)
    async fn update_skill(&self, skill_id: &MemoryId, update: SkillUpdate)
        -> Result<(), MemoryError>;

    /// Получить навык по имени
    async fn get_skill(&self, name: &str) -> Result<Option<Skill>, MemoryError>;
}

pub struct SkillUpdate {
    pub increment_success: bool,
    pub increment_fail: bool,
    pub new_content: Option<String>,  // обновлённая процедура
}

/// Рефлексия и самоулучшение
#[async_trait]
pub trait ReflectionPort: Send + Sync {
    async fn store_reflection(&self, reflection: Reflection)
        -> Result<MemoryId, MemoryError>;

    async fn get_relevant_reflections(&self, query: &MemoryQuery)
        -> Result<Vec<Reflection>, MemoryError>;

    async fn get_failure_patterns(&self, agent_id: &AgentId, limit: usize)
        -> Result<Vec<Reflection>, MemoryError>;
}

/// Консолидация: фоновая обработка памяти
#[async_trait]
pub trait ConsolidationPort: Send + Sync {
    /// Запустить цикл консолидации
    async fn run_consolidation(&self, agent_id: &AgentId) -> Result<ConsolidationReport, MemoryError>;

    /// Пересчитать importance scores
    async fn recalculate_importance(&self, agent_id: &AgentId) -> Result<u32, MemoryError>;

    /// Удалить записи ниже порога importance
    async fn gc_low_importance(&self, threshold: f32, max_age_days: u32) -> Result<u32, MemoryError>;
}

pub struct ConsolidationReport {
    pub episodes_processed: u32,
    pub entities_extracted: u32,
    pub facts_created: u32,
    pub facts_invalidated: u32,
    pub skills_generated: u32,
    pub entries_garbage_collected: u32,
}

/// Unified Memory Port — фасад для всех типов памяти
/// Агенты используют этот единый интерфейс
#[async_trait]
pub trait UnifiedMemoryPort:
    WorkingMemoryPort
    + EpisodicMemoryPort
    + SemanticMemoryPort
    + SkillMemoryPort
    + ReflectionPort
    + Send + Sync
{
    /// Гибридный поиск по всем типам памяти с RRF fusion
    async fn hybrid_search(&self, query: &MemoryQuery)
        -> Result<HybridSearchResult, MemoryError>;

    /// Генерация embedding (через локальную модель или API)
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError>;
}

pub struct HybridSearchResult {
    pub episodes: Vec<SearchResult>,
    pub entities: Vec<Entity>,
    pub facts: Vec<TemporalFact>,
    pub skills: Vec<Skill>,
    pub reflections: Vec<Reflection>,
}
```

### 2.3 SurrealDB Adapter (реализует все порты)

```rust
// src/adapters/memory/surrealdb_adapter.rs

use crate::ports::memory::*;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use std::sync::Arc;

pub struct SurrealMemoryAdapter {
    db: Arc<Surreal<Db>>,
    embedder: Arc<dyn EmbeddingProvider>,  // Qwen3.5 local или API
}

impl SurrealMemoryAdapter {
    pub fn new(db: Arc<Surreal<Db>>, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { db, embedder }
    }

    /// RRF fusion двух списков результатов
    fn rrf_fusion(&self, lists: Vec<Vec<(MemoryId, f32)>>, k: f32) -> Vec<(MemoryId, f32)> {
        let mut scores: std::collections::HashMap<MemoryId, f32> = Default::default();
        for list in &lists {
            for (rank, (id, _original_score)) in list.iter().enumerate() {
                *scores.entry(id.clone()).or_default() += 1.0 / (k + rank as f32 + 1.0);
            }
        }
        let mut result: Vec<_> = scores.into_iter().collect();
        result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        result
    }
}

#[async_trait]
impl WorkingMemoryPort for SurrealMemoryAdapter {
    async fn get_core_blocks(&self, agent_id: &AgentId)
        -> Result<Vec<CoreMemoryBlock>, MemoryError>
    {
        let blocks: Vec<CoreMemoryBlock> = self.db
            .query("SELECT * FROM core_memory WHERE agent_id = type::thing('agent', $id)")
            .bind(("id", agent_id))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(blocks)
    }

    async fn update_core_block(&self, agent_id: &AgentId, label: &str, content: String)
        -> Result<(), MemoryError>
    {
        self.db.query(
            "UPDATE core_memory SET content = $content, updated_at = time::now()
             WHERE agent_id = type::thing('agent', $agent) AND label = $label"
        )
        .bind(("content", &content))
        .bind(("agent", agent_id))
        .bind(("label", label))
        .await
        .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn append_core_block(&self, agent_id: &AgentId, label: &str, text: &str)
        -> Result<(), MemoryError>
    {
        self.db.query(
            "UPDATE core_memory
             SET content = string::concat(content, '\n', $text),
                 updated_at = time::now()
             WHERE agent_id = type::thing('agent', $agent) AND label = $label"
        )
        .bind(("text", text))
        .bind(("agent", agent_id))
        .bind(("label", label))
        .await
        .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }
}

// ... аналогично для остальных портов
```

---

## 3. Self-Editing Memory: агент управляет памятью через Tool Calls

### 3.1 Memory Tools для существующего Tool trait

```rust
// src/tools/memory_tools.rs

use crate::domain::tool::{Tool, ToolResult, ToolError};
use crate::ports::memory::UnifiedMemoryPort;
use serde_json::Value;
use std::sync::Arc;

/// Агент вызывает этот инструмент чтобы запомнить что-то важное
pub struct MemoryStoreTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    agent_id: AgentId,
}

#[async_trait]
impl Tool for MemoryStoreTool {
    fn name(&self) -> &str { "memory_store" }

    fn description(&self) -> &str {
        "Store important information in long-term memory. Use this when you learn \
         something worth remembering across sessions: user preferences, project details, \
         decisions made, important facts."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "What to remember (be specific and concise)"
                },
                "category": {
                    "type": "string",
                    "enum": ["fact", "preference", "decision", "context", "lesson"],
                    "description": "Type of information"
                },
                "importance": {
                    "type": "number",
                    "minimum": 0.0,
                    "maximum": 1.0,
                    "description": "How important (0.0 = trivial, 1.0 = critical)"
                },
                "visibility": {
                    "type": "string",
                    "enum": ["private", "global"],
                    "description": "private = only you, global = all agents can see"
                }
            },
            "required": ["content", "category"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult, ToolError> {
        let content = args["content"].as_str()
            .ok_or(ToolError::InvalidArgs("content required".into()))?;
        let importance = args["importance"].as_f64().unwrap_or(0.5) as f32;
        let visibility = match args["visibility"].as_str() {
            Some("global") => Visibility::Global,
            _ => Visibility::Private,
        };

        let embedding = self.memory.embed(content).await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let entry = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: self.agent_id.clone(),
            content: content.to_string(),
            entry_type: MemoryType::Episode,
            importance,
            created_at: chrono::Utc::now(),
            visibility,
            embedding: Some(embedding),
            metadata: serde_json::json!({
                "category": args["category"],
            }),
        };

        let id = self.memory.store_episode(entry).await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        Ok(ToolResult::text(format!("Stored in memory (id: {})", id)))
    }
}

/// Поиск по памяти — агент ищет что помнит
pub struct MemorySearchTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    agent_id: AgentId,
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str { "memory_search" }

    fn description(&self) -> &str {
        "Search your memory for relevant information. Use this when you need to recall \
         past conversations, user preferences, project context, or lessons learned."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to search for"
                },
                "limit": {
                    "type": "integer",
                    "default": 5,
                    "description": "Max results"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult, ToolError> {
        let query_text = args["query"].as_str()
            .ok_or(ToolError::InvalidArgs("query required".into()))?;
        let limit = args["limit"].as_u64().unwrap_or(5) as usize;

        let embedding = self.memory.embed(query_text).await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let query = MemoryQuery {
            text: query_text.to_string(),
            embedding: Some(embedding),
            agent_id: self.agent_id.clone(),
            include_shared: true,
            time_range: None,
            limit,
        };

        let results = self.memory.hybrid_search(&query).await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        // Форматируем для LLM
        let mut output = String::new();
        for (i, r) in results.episodes.iter().enumerate() {
            output.push_str(&format!(
                "{}. [score: {:.2}] {}\n   ({})\n\n",
                i + 1, r.score, r.entry.content,
                r.entry.created_at.format("%Y-%m-%d %H:%M")
            ));
        }
        if !results.skills.is_empty() {
            output.push_str("Relevant skills:\n");
            for s in &results.skills {
                output.push_str(&format!("- {} (success: {}, fail: {})\n",
                    s.name, s.success_count, s.fail_count));
            }
        }

        Ok(ToolResult::text(if output.is_empty() {
            "No relevant memories found.".into()
        } else {
            output
        }))
    }
}

/// Редактирование core memory — агент обновляет свой контекст
pub struct CoreMemoryUpdateTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    agent_id: AgentId,
}

#[async_trait]
impl Tool for CoreMemoryUpdateTool {
    fn name(&self) -> &str { "core_memory_update" }

    fn description(&self) -> &str {
        "Update your core memory blocks. These blocks are ALWAYS present in your context. \
         Use 'persona' for your identity/behavior, 'user_knowledge' for what you know about \
         the user, 'task_state' for current task context, 'domain' for domain expertise."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "label": {
                    "type": "string",
                    "enum": ["persona", "user_knowledge", "task_state", "domain"]
                },
                "action": {
                    "type": "string",
                    "enum": ["replace", "append"]
                },
                "content": { "type": "string" }
            },
            "required": ["label", "action", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult, ToolError> {
        let label = args["label"].as_str().unwrap();
        let action = args["action"].as_str().unwrap();
        let content = args["content"].as_str().unwrap();

        match action {
            "replace" => {
                self.memory.update_core_block(&self.agent_id, label, content.to_string())
                    .await.map_err(|e| ToolError::Execution(e.to_string()))?;
            }
            "append" => {
                self.memory.append_core_block(&self.agent_id, label, content)
                    .await.map_err(|e| ToolError::Execution(e.to_string()))?;
            }
            _ => return Err(ToolError::InvalidArgs("action must be replace or append".into())),
        }

        Ok(ToolResult::text(format!("Core memory '{}' updated", label)))
    }
}

/// Забывание — агент удаляет ненужное
pub struct MemoryForgetTool {
    memory: Arc<dyn UnifiedMemoryPort>,
    agent_id: AgentId,
}

// Аналогичная реализация — memory.invalidate() или delete по ID
```

### 3.2 Инъекция core memory в каждый промпт агента

```rust
// В runtime/agent.rs — при формировании контекста для LLM

async fn build_context(&self, message: &str) -> Vec<Message> {
    let mut messages = vec![];

    // 1. System prompt с core memory blocks
    let blocks = self.memory.get_core_blocks(&self.agent_id).await.unwrap_or_default();
    let mut system = self.base_system_prompt.clone();
    for block in &blocks {
        system.push_str(&format!(
            "\n\n<{}>\n{}\n</{}>",
            block.label, block.content, block.label
        ));
    }
    messages.push(Message::system(&system));

    // 2. Релевантные воспоминания для текущего сообщения
    if let Ok(embedding) = self.memory.embed(message).await {
        let query = MemoryQuery {
            text: message.to_string(),
            embedding: Some(embedding),
            agent_id: self.agent_id.clone(),
            include_shared: true,
            time_range: None,
            limit: 5,
        };
        if let Ok(results) = self.memory.hybrid_search(&query).await {
            if !results.episodes.is_empty() || !results.skills.is_empty() {
                let context = format_memory_context(&results);
                messages.push(Message::system(&format!(
                    "<relevant_memories>\n{}\n</relevant_memories>", context
                )));
            }
        }
    }

    // 3. Последние сообщения сессии (conversation history)
    // 4. Текущее сообщение пользователя
    messages.push(Message::user(message));

    messages
}
```

---

## 4. Knowledge Graph: инкрементальное извлечение сущностей

### 4.1 Extraction через локальную Qwen3.5

```rust
// src/memory/entity_extractor.rs

const EXTRACTION_PROMPT: &str = r#"
Extract entities and relationships from this conversation.
Return ONLY valid JSON, no other text.

Format:
{
  "entities": [
    {"name": "exact name", "type": "person|company|concept|tool|place", "properties": {}}
  ],
  "relationships": [
    {"subject": "entity name", "predicate": "verb phrase", "object": "entity name", "confidence": 0.9}
  ]
}

Rules:
- Merge variations: "Виктор", "Victor", "the user" → one entity "Victor"
- predicate should be lowercase verb phrase: "works_at", "prefers", "knows_about"
- confidence 0.0-1.0 based on how explicit the statement is
- Only extract what is clearly stated, don't infer

Conversation:
{conversation}
"#;

pub struct EntityExtractor {
    llm: Arc<dyn LlmProvider>,  // Qwen3.5 local или API
}

impl EntityExtractor {
    /// Извлечь сущности и связи из нового эпизода
    pub async fn extract(&self, episode_content: &str) -> Result<ExtractionResult, MemoryError> {
        let prompt = EXTRACTION_PROMPT.replace("{conversation}", episode_content);
        let response = self.llm.complete(&prompt).await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        // Парсим JSON (с защитой от мусора вокруг)
        let json_str = extract_json_from_response(&response);
        let result: ExtractionResult = serde_json::from_str(&json_str)?;
        Ok(result)
    }
}

#[derive(Deserialize)]
pub struct ExtractionResult {
    pub entities: Vec<ExtractedEntity>,
    pub relationships: Vec<ExtractedRelationship>,
}
```

### 4.2 Entity Resolution в SurrealDB

```rust
/// Найти существующую сущность или создать новую
async fn resolve_entity(
    &self,
    extracted: &ExtractedEntity,
    agent_id: &AgentId,
) -> Result<MemoryId, MemoryError> {
    // 1. Точный поиск по имени
    let existing: Option<Entity> = self.db.query(
        "SELECT * FROM entity WHERE string::lowercase(name) = string::lowercase($name) LIMIT 1"
    ).bind(("name", &extracted.name))
    .await?.take(0)?;

    if let Some(e) = existing {
        // Обновляем properties если есть новые
        if !extracted.properties.is_null() {
            self.db.query(
                "UPDATE $id SET properties = object::merge(properties, $props), updated_at = time::now()"
            ).bind(("id", &e.id)).bind(("props", &extracted.properties)).await?;
        }
        return Ok(e.id);
    }

    // 2. Fuzzy поиск по embedding (может быть другое написание)
    let embedding = self.embedder.embed(&extracted.name).await?;
    let similar: Vec<Entity> = self.db.query(
        "SELECT *, vector::similarity::cosine(embedding, $emb) AS sim FROM entity
         WHERE embedding <|3,32|> $emb AND sim > 0.85 LIMIT 3"
    ).bind(("emb", &embedding)).await?.take(0)?;

    if let Some(best) = similar.first() {
        // Высокая схожесть — скорее всего та же сущность
        return Ok(best.id.clone());
    }

    // 3. Создаём новую сущность
    let entity = Entity {
        id: uuid::Uuid::new_v4().to_string(),
        name: extracted.name.clone(),
        entity_type: extracted.entity_type.clone(),
        properties: extracted.properties.clone(),
        summary: None,
        created_by: agent_id.clone(),
    };
    // INSERT в SurrealDB...
    Ok(entity.id)
}
```

### 4.3 Темпоральное обновление фактов

```rust
/// Добавить новый факт с проверкой на конфликт с существующими
async fn add_or_update_fact(
    &self,
    subject_id: &MemoryId,
    predicate: &str,
    object_id: &MemoryId,
    confidence: f32,
    agent_id: &AgentId,
    source_episode: Option<&MemoryId>,
) -> Result<MemoryId, MemoryError> {
    // Проверяем: есть ли текущий (valid_to IS NONE) факт с тем же subject+predicate?
    let existing: Vec<TemporalFact> = self.db.query(
        "SELECT * FROM fact
         WHERE subject = $subj AND predicate = $pred AND valid_to IS NONE"
    )
    .bind(("subj", subject_id))
    .bind(("pred", predicate))
    .await?.take(0)?;

    // Если есть конфликтующий факт — инвалидируем его
    for old_fact in &existing {
        if old_fact.object != *object_id {
            self.db.query(
                "UPDATE $id SET valid_to = time::now(), invalidated_at = time::now()"
            ).bind(("id", &old_fact.id)).await?;
        } else {
            // Тот же факт уже существует, обновляем confidence
            self.db.query(
                "UPDATE $id SET confidence = math::max(confidence, $conf)"
            ).bind(("id", &old_fact.id)).bind(("conf", confidence)).await?;
            return Ok(old_fact.id.clone());
        }
    }

    // Создаём новый факт
    let fact = TemporalFact {
        id: uuid::Uuid::new_v4().to_string(),
        subject: subject_id.clone(),
        predicate: predicate.to_string(),
        object: object_id.clone(),
        confidence,
        valid_from: chrono::Utc::now(),
        valid_to: None,
        recorded_at: chrono::Utc::now(),
        source_episode: source_episode.map(|s| s.to_string()),
        created_by: agent_id.clone(),
    };
    // INSERT...
    Ok(fact.id)
}
```

---

## 5. Skill Learning: навыки из успехов и ошибок

### 5.1 Reflection pipeline (после каждого pipeline run)

```rust
// src/memory/skill_learner.rs

const REFLECTION_PROMPT: &str = r#"
You just completed a task. Analyze the outcome and extract lessons.

Task: {task_summary}
Outcome: {outcome}
Steps taken: {steps}
Errors encountered: {errors}

Respond in JSON:
{
  "what_worked": "specific techniques that helped",
  "what_failed": "specific mistakes or blockers",
  "lesson": "one concise takeaway for future similar tasks",
  "should_create_skill": true/false,
  "skill_name": "optional: name for reusable procedure",
  "skill_content": "optional: step-by-step procedure in markdown"
}
"#;

pub struct SkillLearner {
    llm: Arc<dyn LlmProvider>,
    memory: Arc<dyn UnifiedMemoryPort>,
}

impl SkillLearner {
    /// Вызывается PipelineEngine после завершения run
    pub async fn reflect_on_run(
        &self,
        agent_id: &AgentId,
        run_summary: &PipelineRunSummary,
    ) -> Result<(), MemoryError> {
        let prompt = REFLECTION_PROMPT
            .replace("{task_summary}", &run_summary.task)
            .replace("{outcome}", &run_summary.outcome.to_string())
            .replace("{steps}", &run_summary.steps_description())
            .replace("{errors}", &run_summary.errors_description());

        let response = self.llm.complete(&prompt).await?;
        let analysis: ReflectionAnalysis = serde_json::from_str(&extract_json(&response))?;

        // Сохраняем рефлексию
        let embedding = self.memory.embed(&analysis.lesson).await?;
        self.memory.store_reflection(Reflection {
            agent_id: agent_id.clone(),
            pipeline_run: Some(run_summary.run_id.clone()),
            task_summary: run_summary.task.clone(),
            outcome: run_summary.outcome.clone(),
            what_worked: analysis.what_worked,
            what_failed: analysis.what_failed,
            lesson: analysis.lesson,
        }).await?;

        // Создаём или обновляем навык если нужно
        if analysis.should_create_skill {
            if let (Some(name), Some(content)) = (&analysis.skill_name, &analysis.skill_content) {
                let existing = self.memory.get_skill(name).await?;
                match existing {
                    Some(skill) => {
                        // Навык уже есть — обновляем
                        self.memory.update_skill(&skill.id, SkillUpdate {
                            increment_success: run_summary.outcome == ReflectionOutcome::Success,
                            increment_fail: run_summary.outcome == ReflectionOutcome::Failure,
                            new_content: Some(content.clone()),
                        }).await?;
                    }
                    None => {
                        let emb = self.memory.embed(content).await?;
                        self.memory.store_skill(Skill {
                            id: uuid::Uuid::new_v4().to_string(),
                            name: name.clone(),
                            description: analysis.lesson.clone(),
                            content: content.clone(),
                            tags: vec![],
                            success_count: if run_summary.outcome == ReflectionOutcome::Success { 1 } else { 0 },
                            fail_count: if run_summary.outcome == ReflectionOutcome::Failure { 1 } else { 0 },
                            version: 1,
                            created_by: agent_id.clone(),
                        }).await?;
                    }
                }
            }
        }

        Ok(())
    }
}
```

---

## 6. Memory Consolidation: фоновый воркер

```rust
// src/memory/consolidation_worker.rs

pub struct ConsolidationWorker {
    memory: Arc<dyn UnifiedMemoryPort>,
    extractor: Arc<EntityExtractor>,
    interval: Duration,
}

impl ConsolidationWorker {
    /// Запускается как фоновая tokio-задача при старте daemon
    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.interval);
        loop {
            interval.tick().await;
            if let Err(e) = self.consolidate_cycle().await {
                tracing::error!("Consolidation error: {}", e);
            }
        }
    }

    async fn consolidate_cycle(&self) -> Result<(), MemoryError> {
        tracing::info!("Starting memory consolidation cycle");

        // 1. Извлечение сущностей из необработанных эпизодов
        let unprocessed: Vec<MemoryEntry> = self.memory.db.query(
            "SELECT * FROM episode WHERE metadata.consolidated IS NOT true
             ORDER BY created_at ASC LIMIT 50"
        ).await?.take(0)?;

        for episode in &unprocessed {
            let extracted = self.extractor.extract(&episode.content).await?;
            for entity in &extracted.entities {
                let entity_id = self.resolve_entity(entity, &episode.agent_id).await?;
                // создаём facts...
            }
            // Помечаем обработанным
            self.memory.db.query(
                "UPDATE $id SET metadata.consolidated = true"
            ).bind(("id", &episode.id)).await?;
        }

        // 2. Importance decay: снижаем score старых записей
        self.memory.db.query(
            "UPDATE episode SET importance = importance * 0.95
             WHERE created_at < time::now() - 7d AND importance > 0.1"
        ).await?;

        // 3. GC: удаляем совсем старые неважные записи
        let deleted: Vec<MemoryEntry> = self.memory.db.query(
            "DELETE FROM episode WHERE importance < 0.05
             AND created_at < time::now() - 30d
             RETURN BEFORE"
        ).await?.take(0)?;

        tracing::info!(
            "Consolidation: {} episodes processed, {} entries GC'd",
            unprocessed.len(), deleted.len()
        );

        Ok(())
    }
}
```

---

## 7. Memory Sharing через IPC-брокер

### 7.1 MemoryEvent в IPC

```rust
// Расширение существующего IPC message kind

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcMessageKind {
    Task,
    Result,
    Done,
    Report,
    // Новые виды для памяти:
    MemoryEvent(MemoryEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub event_type: MemoryEventType,
    pub source_agent: AgentId,
    pub entry_id: MemoryId,
    pub summary: String,  // краткое описание для других агентов
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryEventType {
    EntityDiscovered,     // новая сущность обнаружена
    FactEstablished,      // новый факт подтверждён
    FactInvalidated,      // факт устарел
    SkillLearned,         // новый навык создан
    SkillUpdated,         // навык обновлён
    InsightGenerated,     // рефлексия создана
}
```

### 7.2 ACL-проверка в адаптере

```rust
fn check_read_access(
    &self,
    requesting_agent: &AgentId,
    entry: &MemoryEntry,
) -> bool {
    match &entry.visibility {
        Visibility::Global => true,
        Visibility::Private => entry.agent_id == *requesting_agent,
        Visibility::SharedWith(agents) => agents.contains(requesting_agent),
    }
}
```

ACL проверяется в каждом read-запросе. Write — агент всегда пишет только в свою память. Shared visibility устанавливается при создании записи.

---

## 8. Генерация Embeddings локально

### 8.1 Trait для embedding provider

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError>;
    fn dimensions(&self) -> usize;
}

/// Через HTTP к llama.cpp server с embedding-моделью
pub struct LlamaCppEmbedder {
    client: reqwest::Client,
    url: String,  // "http://127.0.0.1:8081/v1/embeddings"
    dimensions: usize,
}

#[async_trait]
impl EmbeddingProvider for LlamaCppEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        let resp = self.client.post(&self.url)
            .json(&serde_json::json!({
                "input": text,
                "model": "qwen3.5"  // или dedicated embedding model
            }))
            .send().await
            .map_err(|e| MemoryError::Embedding(e.to_string()))?;

        let body: serde_json::Value = resp.json().await
            .map_err(|e| MemoryError::Embedding(e.to_string()))?;

        let embedding: Vec<f32> = body["data"][0]["embedding"]
            .as_array()
            .ok_or(MemoryError::Embedding("invalid response".into()))?
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();

        Ok(embedding)
    }

    fn dimensions(&self) -> usize { self.dimensions }
}
```

Рекомендация: для embeddings на 48GB VPS лучше запустить отдельный инстанс llama.cpp с маленькой embedding-моделью (nomic-embed-text, 137M параметров, ~300MB RAM, 1000+ tok/s) отдельно от основной Qwen3.5. Два процесса llama-server на разных портах.

---

## 9. Поэтапный план внедрения

### Шаг 1 (2-3 дня): SurrealDB embedded + базовые порты

- Добавить `surrealdb` в Cargo.toml
- Создать `MemoryEngine::new()` с инициализацией схемы
- Реализовать `WorkingMemoryPort` и `EpisodicMemoryPort`
- Core memory blocks для всех 6 агентов
- Тесты: запись/чтение эпизодов, обновление core blocks

### Шаг 2 (2-3 дня): Memory tools + инъекция в контекст

- Реализовать `MemoryStoreTool`, `MemorySearchTool`, `CoreMemoryUpdateTool`
- Зарегистрировать tools в runtime для каждого агента
- Build context: core blocks + relevant memories в каждом промпте
- Запустить embedding model (nomic-embed через llama.cpp)

### Шаг 3 (3-5 дней): Knowledge Graph + Entity Extraction

- Реализовать `SemanticMemoryPort` (entities + temporal facts)
- `EntityExtractor` через локальную Qwen3.5
- Entity resolution (exact match + embedding similarity)
- Битемпоральное обновление фактов
- Графовый обход через SurrealQL

### Шаг 4 (2-3 дня): Skill Learning + Reflection

- `SkillLearner.reflect_on_run()` — хук в PipelineEngine
- `SkillMemoryPort` с vector search по навыкам
- Инъекция релевантных skills в контекст при получении задачи

### Шаг 5 (2-3 дня): Consolidation + Memory Sharing

- `ConsolidationWorker` как фоновая tokio task
- Importance decay + GC
- `MemoryEvent` в IPC-брокере
- ACL-проверки при кросс-агентном доступе

### Шаг 6 (1-2 дня): Гибридный поиск + RRF

- Параллельный vector + BM25 retrieval
- RRF fusion в Rust
- `UnifiedMemoryPort.hybrid_search()`
- Мониторинг: логируем query latency, hit rate
