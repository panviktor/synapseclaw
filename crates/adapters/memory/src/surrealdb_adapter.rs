//! SurrealDB embedded adapter — implements all Phase 4.3 memory ports.
//!
//! Single SurrealDB instance backs: working memory (core blocks), episodic memory,
//! semantic memory (knowledge graph), skill memory, reflections, and consolidation.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use surrealdb::engine::local::{Db, SurrealKv};
use surrealdb::Surreal;

use synapse_domain::domain::memory::{
    AgentId, ConsolidationReport, CoreMemoryBlock, Entity, HybridSearchResult, MemoryCategory,
    MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, ReflectionOutcome, SearchResult,
    SessionId, Skill, SkillUpdate, TemporalFact,
};
use synapse_domain::ports::memory::{
    ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
    UnifiedMemoryPort, WorkingMemoryPort,
};

use crate::embeddings::EmbeddingProvider;

/// Log memory operation latency after an async block completes.
fn log_latency(op: &str, start: Instant) {
    let ms = start.elapsed().as_millis() as u64;
    if ms > 50 {
        tracing::info!(op, latency_ms = ms, "memory.slow_op");
    } else {
        tracing::debug!(op, latency_ms = ms, "memory.op");
    }
}

/// SurrealDB-backed memory adapter.
pub struct SurrealMemoryAdapter {
    db: Arc<Surreal<Db>>,
    embedder: Arc<dyn EmbeddingProvider>,
    agent_id: String,
}

impl SurrealMemoryAdapter {
    /// Create a new adapter, initializing the DB and applying the schema.
    pub async fn new(
        data_dir: &str,
        embedder: Arc<dyn EmbeddingProvider>,
        agent_id: String,
    ) -> Result<Self, MemoryError> {
        let db = Surreal::new::<SurrealKv>(data_dir)
            .await
            .map_err(|e| MemoryError::Storage(format!("SurrealDB init: {e}")))?;

        db.use_ns("synapseclaw")
            .use_db("memory")
            .await
            .map_err(|e| MemoryError::Storage(format!("SurrealDB use ns/db: {e}")))?;

        let adapter = Self {
            db: Arc::new(db),
            embedder,
            agent_id,
        };

        adapter.apply_schema().await?;

        Ok(adapter)
    }

    async fn apply_schema(&self) -> Result<(), MemoryError> {
        // Apply base schema (tables, fields, standard indexes).
        let schema = include_str!("surrealdb_schema.surql");
        self.db
            .query(schema)
            .await
            .map_err(|e| MemoryError::Storage(format!("Schema apply: {e}")))?;

        // Apply BM25 full-text search index (separate query — syntax incompatible with IF NOT EXISTS).
        if let Err(e) = self
            .db
            .query("DEFINE INDEX idx_ep_content ON episode FIELDS content SEARCH ANALYZER simple_analyzer BM25")
            .await
        {
            tracing::debug!("BM25 index creation (may already exist): {e}");
        }

        // Apply HNSW vector indexes (best-effort — requires embedding data).
        for (table, idx) in [
            ("episode", "idx_ep_vector"),
            ("entity", "idx_ent_vector"),
            ("skill", "idx_skill_vector"),
            ("reflection", "idx_refl_vector"),
        ] {
            let q = format!(
                "DEFINE INDEX {idx} ON {table} FIELDS embedding HNSW DIMENSION 768 DIST COSINE"
            );
            if let Err(e) = self.db.query(&q).await {
                tracing::debug!("HNSW index {idx} (may already exist): {e}");
            }
        }

        tracing::info!("SurrealDB memory schema applied");
        Ok(())
    }

    fn me(&self) -> &str {
        &self.agent_id
    }

    /// Helper: take rows as serde_json::Value and convert.
    fn rows_to_entries(rows: Vec<serde_json::Value>) -> Vec<MemoryEntry> {
        rows.into_iter().filter_map(|v| row_to_entry(&v)).collect()
    }
}

// ── JSON → domain type helpers ───────────────────────────────────

fn row_to_entry(v: &serde_json::Value) -> Option<MemoryEntry> {
    Some(MemoryEntry {
        id: json_str(v, "id"),
        key: json_str(v, "key"),
        content: json_str(v, "content"),
        category: MemoryCategory::from_str_lossy(&json_str(v, "category")),
        timestamp: json_str(v, "created_at"),
        session_id: v
            .get("session_id")
            .and_then(|s| s.as_str())
            .map(String::from),
        score: v.get("bm25_score").and_then(|s| s.as_f64()),
    })
}

fn row_to_core_block(v: &serde_json::Value) -> Option<CoreMemoryBlock> {
    Some(CoreMemoryBlock {
        agent_id: json_str(v, "agent_id"),
        label: json_str(v, "label"),
        content: json_str(v, "content"),
        max_tokens: v.get("max_tokens").and_then(|n| n.as_u64()).unwrap_or(2000) as usize,
        updated_at: chrono::Utc::now(), // SurrealDB datetime → chrono would need parsing; approximate
    })
}

fn row_to_entity(v: &serde_json::Value) -> Option<Entity> {
    Some(Entity {
        id: json_str(v, "id"),
        name: json_str(v, "name"),
        entity_type: json_str(v, "entity_type"),
        properties: v
            .get("properties")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default())),
        summary: v.get("summary").and_then(|s| s.as_str()).map(String::from),
        created_by: json_str(v, "created_by"),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    })
}

fn row_to_fact(v: &serde_json::Value) -> Option<TemporalFact> {
    Some(TemporalFact {
        id: json_str(v, "id"),
        subject: json_str(v, "subject"),
        predicate: json_str(v, "predicate"),
        object: json_str(v, "object"),
        confidence: v.get("confidence").and_then(|n| n.as_f64()).unwrap_or(0.8) as f32,
        valid_from: chrono::Utc::now(),
        valid_to: None,
        recorded_at: chrono::Utc::now(),
        source_episode: v
            .get("source_episode")
            .and_then(|s| s.as_str())
            .map(String::from),
        created_by: json_str(v, "created_by"),
    })
}

fn row_to_skill(v: &serde_json::Value) -> Option<Skill> {
    Some(Skill {
        id: json_str(v, "id"),
        name: json_str(v, "name"),
        description: json_str(v, "description"),
        content: json_str(v, "content"),
        tags: v
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        success_count: v.get("success_count").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
        fail_count: v.get("fail_count").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
        version: v.get("version").and_then(|n| n.as_u64()).unwrap_or(1) as u32,
        created_by: json_str(v, "created_by"),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    })
}

fn row_to_reflection(v: &serde_json::Value) -> Option<Reflection> {
    Some(Reflection {
        id: json_str(v, "id"),
        agent_id: json_str(v, "agent_id"),
        pipeline_run: v
            .get("pipeline_run")
            .and_then(|s| s.as_str())
            .map(String::from),
        task_summary: json_str(v, "task_summary"),
        outcome: match json_str(v, "outcome").as_str() {
            "success" => ReflectionOutcome::Success,
            "partial" => ReflectionOutcome::Partial,
            _ => ReflectionOutcome::Failure,
        },
        what_worked: json_str(v, "what_worked"),
        what_failed: json_str(v, "what_failed"),
        lesson: json_str(v, "lesson"),
        created_at: chrono::Utc::now(),
    })
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .map(|val| {
            val.as_str()
                .map(String::from)
                // SurrealDB record IDs are objects like {"tb":"episode","id":{"String":"..."}}
                .unwrap_or_else(|| val.to_string())
        })
        .unwrap_or_default()
}

/// Helper macro: take Vec<serde_json::Value> from query response.
macro_rules! take_json {
    ($resp:expr, $idx:expr) => {{
        let rows: Vec<serde_json::Value> = $resp
            .take($idx)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        rows
    }};
}

// ── WorkingMemoryPort ────────────────────────────────────────────

#[async_trait]
impl WorkingMemoryPort for SurrealMemoryAdapter {
    async fn get_core_blocks(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT * FROM core_memory WHERE agent_id = $agent ORDER BY label ASC")
            .bind(("agent", agent_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.iter().filter_map(row_to_core_block).collect())
    }

    async fn update_core_block(
        &self,
        agent_id: &AgentId,
        label: &str,
        content: String,
    ) -> Result<(), MemoryError> {
        // Try update first, then create if no match
        self.db
            .query(
                "IF (SELECT count() FROM core_memory WHERE agent_id = $agent AND label = $label GROUP ALL)[0].count > 0 {
                    UPDATE core_memory SET content = $content, updated_at = time::now()
                        WHERE agent_id = $agent AND label = $label;
                } ELSE {
                    CREATE core_memory SET agent_id = $agent, label = $label, content = $content, max_tokens = 2000, updated_at = time::now();
                };",
            )
            .bind(("agent", agent_id.clone()))
            .bind(("label", label.to_string()))
            .bind(("content", content))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn append_core_block(
        &self,
        agent_id: &AgentId,
        label: &str,
        text: &str,
    ) -> Result<(), MemoryError> {
        self.db
            .query(
                "UPDATE core_memory SET
                    content = string::concat(content, '\n', $text),
                    updated_at = time::now()
                 WHERE agent_id = $agent AND label = $label",
            )
            .bind(("agent", agent_id.clone()))
            .bind(("label", label.to_string()))
            .bind(("text", text.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }
}

// ── EpisodicMemoryPort ───────────────────────────────────────────

#[async_trait]
impl EpisodicMemoryPort for SurrealMemoryAdapter {
    async fn store_episode(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError> {
        let id = if entry.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            entry.id.clone()
        };

        // Generate embedding if provider is available (best-effort).
        let embedding: Option<Vec<f32>> = if self.embedder.dimensions() > 0 {
            self.embedder.embed_one(&entry.content).await.ok()
        } else {
            None
        };

        self.db
            .query(
                "CREATE episode SET
                    agent_id = $agent,
                    key = $key,
                    content = $content,
                    category = $category,
                    session_id = $session_id,
                    importance = 0.5,
                    created_at = time::now(),
                    visibility = 'private',
                    embedding = $embedding",
            )
            .bind(("agent", self.me().to_string()))
            .bind(("key", entry.key))
            .bind(("content", entry.content))
            .bind(("category", entry.category.to_string()))
            .bind(("session_id", entry.session_id))
            .bind(("embedding", embedding))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(id)
    }

    async fn get_recent(
        &self,
        agent_id: &AgentId,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM episode WHERE agent_id = $agent
                 ORDER BY created_at DESC LIMIT $limit",
            )
            .bind(("agent", agent_id.clone()))
            .bind(("limit", limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(SurrealMemoryAdapter::rows_to_entries(rows))
    }

    async fn get_session(&self, session_id: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM episode WHERE session_id = $sid
                 ORDER BY created_at ASC",
            )
            .bind(("sid", session_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(SurrealMemoryAdapter::rows_to_entries(rows))
    }

    async fn search_episodes(&self, query: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
        // BM25 keyword search
        let mut bm25_resp = self
            .db
            .query(
                "SELECT *, search::score(1) AS bm25_score FROM episode
                 WHERE content @1@ $text
                 AND (agent_id = $agent OR visibility = 'global' OR $agent INSIDE shared_with)
                 ORDER BY bm25_score DESC
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("agent", query.agent_id.clone()))
            .bind(("limit", query.limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let bm25_rows = take_json!(bm25_resp, 0);

        // Vector search (if embedding provider is available)
        let query_embedding = if self.embedder.dimensions() > 0 {
            match &query.embedding {
                Some(emb) => Some(emb.clone()),
                None => self.embedder.embed_one(&query.text).await.ok(),
            }
        } else {
            None
        };

        let vec_rows = if let Some(ref emb) = query_embedding {
            let mut vec_resp = self
                .db
                .query(
                    "SELECT *,
                        vector::similarity::cosine(embedding, $emb) AS vec_score
                     FROM episode
                     WHERE embedding <|$limit,64|> $emb
                     AND (agent_id = $agent OR visibility = 'global' OR $agent INSIDE shared_with)
                     ORDER BY vec_score DESC
                     LIMIT $limit",
                )
                .bind(("emb", emb.clone()))
                .bind(("agent", query.agent_id.clone()))
                .bind(("limit", query.limit))
                .await
                .map_err(|e| MemoryError::Storage(e.to_string()))?;

            take_json!(vec_resp, 0)
        } else {
            vec![]
        };

        // Merge results: if we have both BM25 and vector, use RRF fusion.
        // Otherwise return BM25 results.
        if vec_rows.is_empty() {
            // BM25 only
            return Ok(bm25_rows
                .iter()
                .filter_map(|v| {
                    let entry = row_to_entry(v)?;
                    let score = v.get("bm25_score").and_then(|s| s.as_f64()).unwrap_or(0.0);
                    Some(SearchResult {
                        entry,
                        score: score as f32,
                        source: synapse_domain::domain::memory::SearchSource::BM25,
                    })
                })
                .collect());
        }

        // RRF fusion: combine BM25 + vector results.
        let bm25_list: Vec<(String, f32)> = bm25_rows
            .iter()
            .filter_map(|v| {
                let id = json_str(v, "id");
                let score = v.get("bm25_score").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                Some((id, score))
            })
            .collect();

        let vec_list: Vec<(String, f32)> = vec_rows
            .iter()
            .filter_map(|v| {
                let id = json_str(v, "id");
                let score = v.get("vec_score").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                Some((id, score))
            })
            .collect();

        let fused = crate::vector::rrf_fusion(&[bm25_list, vec_list], 60.0, query.limit);

        // Build a lookup map for row data.
        let mut row_map: std::collections::HashMap<String, &serde_json::Value> =
            std::collections::HashMap::new();
        for row in bm25_rows.iter().chain(vec_rows.iter()) {
            let id = json_str(row, "id");
            row_map.entry(id).or_insert(row);
        }

        Ok(fused
            .into_iter()
            .filter_map(|scored| {
                let row = row_map.get(&scored.id)?;
                let entry = row_to_entry(row)?;
                Some(SearchResult {
                    entry,
                    score: scored.final_score,
                    source: synapse_domain::domain::memory::SearchSource::Hybrid,
                })
            })
            .collect())
    }
}

impl SurrealMemoryAdapter {
    async fn find_entity_by_id(&self, id: &str) -> Result<Option<Entity>, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT * FROM entity WHERE id = $id LIMIT 1")
            .bind(("id", id.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.first().and_then(row_to_entity))
    }
}

// ── SemanticMemoryPort ───────────────────────────────────────────

#[async_trait]
impl SemanticMemoryPort for SurrealMemoryAdapter {
    async fn upsert_entity(&self, entity: Entity) -> Result<MemoryId, MemoryError> {
        let id = if entity.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            entity.id.clone()
        };

        // Generate embedding for entity name + summary (best-effort).
        let embed_text = format!(
            "{} {}",
            entity.name,
            entity.summary.as_deref().unwrap_or("")
        );
        let embedding: Option<Vec<f32>> = if self.embedder.dimensions() > 0 {
            self.embedder.embed_one(&embed_text).await.ok()
        } else {
            None
        };

        self.db
            .query(
                "IF (SELECT count() FROM entity WHERE string::lowercase(name) = string::lowercase($name) GROUP ALL)[0].count > 0 {
                    UPDATE entity SET
                        entity_type = $etype,
                        properties = $props,
                        summary = $summary,
                        embedding = $embedding,
                        updated_at = time::now()
                    WHERE string::lowercase(name) = string::lowercase($name);
                } ELSE {
                    CREATE entity SET
                        name = $name,
                        entity_type = $etype,
                        properties = $props,
                        summary = $summary,
                        embedding = $embedding,
                        created_by = $agent,
                        created_at = time::now(),
                        updated_at = time::now();
                };",
            )
            .bind(("name", entity.name))
            .bind(("etype", entity.entity_type))
            .bind(("props", entity.properties))
            .bind(("summary", entity.summary))
            .bind(("embedding", embedding))
            .bind(("agent", entity.created_by))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(id)
    }

    async fn find_entity(&self, name: &str) -> Result<Option<Entity>, MemoryError> {
        // 1. Exact case-insensitive name match
        let mut resp = self
            .db
            .query(
                "SELECT * FROM entity
                 WHERE string::lowercase(name) = string::lowercase($name)
                 LIMIT 1",
            )
            .bind(("name", name.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        if let Some(entity) = rows.first().and_then(row_to_entity) {
            return Ok(Some(entity));
        }

        // 2. Embedding similarity fallback (>0.85 threshold)
        if self.embedder.dimensions() > 0 {
            if let Ok(emb) = self.embedder.embed_one(name).await {
                let mut vec_resp = self
                    .db
                    .query(
                        "SELECT *,
                            vector::similarity::cosine(embedding, $emb) AS sim
                         FROM entity
                         WHERE embedding <|3,32|> $emb
                         ORDER BY sim DESC
                         LIMIT 1",
                    )
                    .bind(("emb", emb))
                    .await
                    .map_err(|e| MemoryError::Storage(e.to_string()))?;

                let vec_rows = take_json!(vec_resp, 0);
                if let Some(row) = vec_rows.first() {
                    let sim = row.get("sim").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if sim > 0.85 {
                        return Ok(row_to_entity(row));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn add_fact(&self, fact: TemporalFact) -> Result<MemoryId, MemoryError> {
        let id = if fact.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            fact.id.clone()
        };

        self.db
            .query(
                "CREATE fact SET
                    subject = $subj,
                    predicate = $pred,
                    object = $obj,
                    confidence = $conf,
                    valid_from = time::now(),
                    recorded_at = time::now(),
                    source_episode = $source,
                    created_by = $agent",
            )
            .bind(("subj", fact.subject))
            .bind(("pred", fact.predicate))
            .bind(("obj", fact.object))
            .bind(("conf", fact.confidence))
            .bind(("source", fact.source_episode))
            .bind(("agent", fact.created_by))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(id)
    }

    async fn invalidate_fact(&self, fact_id: &MemoryId) -> Result<(), MemoryError> {
        self.db
            .query("UPDATE fact SET valid_to = time::now() WHERE id = $id")
            .bind(("id", fact_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_current_facts(
        &self,
        entity_id: &MemoryId,
    ) -> Result<Vec<TemporalFact>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM fact
                 WHERE subject = $eid AND valid_to IS NONE
                 ORDER BY recorded_at DESC",
            )
            .bind(("eid", entity_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.iter().filter_map(row_to_fact).collect())
    }

    async fn traverse(
        &self,
        entity_id: &MemoryId,
        hops: usize,
    ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
        let max_hops = hops.min(5); // safety cap
        let mut results = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut frontier = vec![entity_id.clone()];

        for _depth in 0..max_hops {
            let mut next_frontier = Vec::new();
            for eid in &frontier {
                if !visited.insert(eid.clone()) {
                    continue;
                }
                let facts = self.get_current_facts(eid).await?;
                for fact in facts {
                    if let Some(entity) = self.find_entity_by_id(&fact.object).await? {
                        if !visited.contains(&entity.id) {
                            next_frontier.push(entity.id.clone());
                        }
                        results.push((entity, fact));
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }
        Ok(results)
    }

    async fn search_entities(&self, query: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM entity
                 WHERE name CONTAINS $text OR summary CONTAINS $text
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("limit", query.limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.iter().filter_map(row_to_entity).collect())
    }
}

// ── SkillMemoryPort ──────────────────────────────────────────────

#[async_trait]
impl SkillMemoryPort for SurrealMemoryAdapter {
    async fn store_skill(&self, skill: Skill) -> Result<MemoryId, MemoryError> {
        let id = if skill.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            skill.id.clone()
        };

        self.db
            .query(
                "CREATE skill SET
                    name = $name,
                    description = $desc,
                    content = $content,
                    tags = $tags,
                    success_count = $sc,
                    fail_count = $fc,
                    version = $ver,
                    created_by = $agent,
                    created_at = time::now(),
                    updated_at = time::now()",
            )
            .bind(("name", skill.name))
            .bind(("desc", skill.description))
            .bind(("content", skill.content))
            .bind(("tags", skill.tags))
            .bind(("sc", skill.success_count as i64))
            .bind(("fc", skill.fail_count as i64))
            .bind(("ver", skill.version as i64))
            .bind(("agent", skill.created_by))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(id)
    }

    async fn find_skills(&self, query: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM skill
                 WHERE name CONTAINS $text OR description CONTAINS $text
                 ORDER BY success_count DESC
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("limit", query.limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.iter().filter_map(row_to_skill).collect())
    }

    async fn update_skill(
        &self,
        skill_id: &MemoryId,
        update: SkillUpdate,
    ) -> Result<(), MemoryError> {
        let mut parts = vec!["updated_at = time::now()".to_string()];
        if update.increment_success {
            parts.push("success_count += 1".to_string());
        }
        if update.increment_fail {
            parts.push("fail_count += 1".to_string());
        }
        if update.new_content.is_some() {
            parts.push("content = $content".to_string());
            parts.push("version += 1".to_string());
        }

        let q = format!("UPDATE skill SET {} WHERE id = $id", parts.join(", "));

        let mut query = self.db.query(&q).bind(("id", skill_id.clone()));
        if let Some(ref content) = update.new_content {
            query = query.bind(("content", content.clone()));
        }

        query
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_skill(&self, name: &str) -> Result<Option<Skill>, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT * FROM skill WHERE name = $name LIMIT 1")
            .bind(("name", name.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.first().and_then(row_to_skill))
    }
}

// ── ReflectionPort ───────────────────────────────────────────────

#[async_trait]
impl ReflectionPort for SurrealMemoryAdapter {
    async fn store_reflection(&self, reflection: Reflection) -> Result<MemoryId, MemoryError> {
        let id = if reflection.id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            reflection.id.clone()
        };

        self.db
            .query(
                "CREATE reflection SET
                    agent_id = $agent,
                    pipeline_run = $run,
                    task_summary = $task,
                    outcome = $outcome,
                    what_worked = $worked,
                    what_failed = $failed,
                    lesson = $lesson,
                    created_at = time::now()",
            )
            .bind(("agent", reflection.agent_id))
            .bind(("run", reflection.pipeline_run))
            .bind(("task", reflection.task_summary))
            .bind(("outcome", reflection.outcome.to_string()))
            .bind(("worked", reflection.what_worked))
            .bind(("failed", reflection.what_failed))
            .bind(("lesson", reflection.lesson))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(id)
    }

    async fn get_relevant_reflections(
        &self,
        query: &MemoryQuery,
    ) -> Result<Vec<Reflection>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM reflection
                 WHERE lesson CONTAINS $text OR task_summary CONTAINS $text
                 ORDER BY created_at DESC
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("limit", query.limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.iter().filter_map(row_to_reflection).collect())
    }

    async fn get_failure_patterns(
        &self,
        agent_id: &AgentId,
        limit: usize,
    ) -> Result<Vec<Reflection>, MemoryError> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM reflection
                 WHERE agent_id = $agent AND outcome = 'failure'
                 ORDER BY created_at DESC
                 LIMIT $limit",
            )
            .bind(("agent", agent_id.clone()))
            .bind(("limit", limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.iter().filter_map(row_to_reflection).collect())
    }
}

// ── ConsolidationPort ────────────────────────────────────────────

#[async_trait]
impl ConsolidationPort for SurrealMemoryAdapter {
    async fn run_consolidation(
        &self,
        agent_id: &AgentId,
    ) -> Result<ConsolidationReport, MemoryError> {
        let decayed = self.recalculate_importance(agent_id).await.unwrap_or(0);
        let gc_count = self.gc_low_importance(0.05, 30).await.unwrap_or(0);

        // Count current totals for the report.
        let mut resp = self
            .db
            .query(
                "SELECT
                    (SELECT count() FROM episode GROUP ALL)[0].count AS episodes,
                    (SELECT count() FROM entity GROUP ALL)[0].count AS entities,
                    (SELECT count() FROM fact WHERE valid_to IS NONE GROUP ALL)[0].count AS facts,
                    (SELECT count() FROM skill GROUP ALL)[0].count AS skills",
            )
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let r = rows.first();
        Ok(ConsolidationReport {
            episodes_processed: r
                .and_then(|v| v.get("episodes"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            entities_extracted: r
                .and_then(|v| v.get("entities"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            facts_created: r
                .and_then(|v| v.get("facts"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            facts_invalidated: 0,
            skills_generated: r
                .and_then(|v| v.get("skills"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            entries_garbage_collected: gc_count + decayed,
        })
    }

    async fn recalculate_importance(&self, _agent_id: &AgentId) -> Result<u32, MemoryError> {
        // Count affected before update.
        let mut count_resp = self
            .db
            .query(
                "SELECT count() AS total FROM episode
                 WHERE created_at < time::now() - 7d AND importance > 0.1 GROUP ALL",
            )
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows: Vec<serde_json::Value> = count_resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        let affected = rows
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        self.db
            .query(
                "UPDATE episode SET importance = importance * 0.95
                 WHERE created_at < time::now() - 7d AND importance > 0.1",
            )
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(affected)
    }

    async fn gc_low_importance(
        &self,
        threshold: f32,
        max_age_days: u32,
    ) -> Result<u32, MemoryError> {
        // Count before delete.
        let count_q = format!(
            "SELECT count() AS total FROM episode
             WHERE importance < $threshold AND created_at < time::now() - {max_age_days}d GROUP ALL"
        );
        let mut count_resp = self
            .db
            .query(&count_q)
            .bind(("threshold", threshold))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows: Vec<serde_json::Value> = count_resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        let affected = rows
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let q = format!(
            "DELETE FROM episode WHERE importance < $threshold AND created_at < time::now() - {max_age_days}d"
        );
        self.db
            .query(q)
            .bind(("threshold", threshold))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        Ok(affected)
    }
}

// ── UnifiedMemoryPort ────────────────────────────────────────────

#[async_trait]
impl UnifiedMemoryPort for SurrealMemoryAdapter {
    async fn hybrid_search(&self, query: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
        let episodes = self.search_episodes(query).await?;
        let entities = self.search_entities(query).await?;
        let skills = self.find_skills(query).await?;
        let reflections = self.get_relevant_reflections(query).await?;

        Ok(HybridSearchResult {
            episodes,
            entities,
            facts: vec![],
            skills,
            reflections,
        })
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embedder
            .embed_one(text)
            .await
            .map_err(|e| MemoryError::Embedding(e.to_string()))
    }

    async fn store(
        &self,
        key: &str,
        content: &str,
        category: &MemoryCategory,
        session_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        let entry = MemoryEntry {
            id: String::new(),
            key: key.to_string(),
            content: content.to_string(),
            category: category.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_id: session_id.map(String::from),
            score: None,
        };
        self.store_episode(entry).await?;
        Ok(())
    }

    async fn recall(
        &self,
        query: &str,
        limit: usize,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let session_filter = if session_id.is_some() {
            "AND session_id = $sid"
        } else {
            ""
        };

        let q = format!(
            "SELECT *, search::score(1) AS bm25_score FROM episode
             WHERE content @1@ $text
             AND (agent_id = $agent OR visibility = 'global' OR $agent INSIDE shared_with)
             {session_filter}
             ORDER BY bm25_score DESC
             LIMIT $limit"
        );

        let mut resp = self
            .db
            .query(&q)
            .bind(("text", query.to_string()))
            .bind(("agent", self.me().to_string()))
            .bind(("sid", session_id.unwrap_or("").to_string()))
            .bind(("limit", limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows
            .iter()
            .filter_map(|v| {
                let mut entry = row_to_entry(v)?;
                entry.score = v.get("bm25_score").and_then(|s| s.as_f64());
                Some(entry)
            })
            .collect())
    }

    async fn forget(&self, key: &str) -> Result<bool, MemoryError> {
        let mut resp = self
            .db
            .query("DELETE FROM episode WHERE key = $key RETURN BEFORE")
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(!rows.is_empty())
    }

    async fn get(&self, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT * FROM episode WHERE key = $key LIMIT 1")
            .bind(("key", key.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows.first().and_then(row_to_entry))
    }

    async fn list(
        &self,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let mut conditions = vec![
            "(agent_id = $agent OR visibility = 'global' OR $agent INSIDE shared_with)".to_string(),
        ];
        if category.is_some() {
            conditions.push("category = $cat".to_string());
        }
        if session_id.is_some() {
            conditions.push("session_id = $sid".to_string());
        }

        let q = format!(
            "SELECT * FROM episode WHERE {} ORDER BY created_at DESC LIMIT $limit",
            conditions.join(" AND ")
        );

        let mut resp = self
            .db
            .query(&q)
            .bind(("agent", self.me().to_string()))
            .bind(("cat", category.map(|c| c.to_string()).unwrap_or_default()))
            .bind(("sid", session_id.unwrap_or("").to_string()))
            .bind(("limit", limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(SurrealMemoryAdapter::rows_to_entries(rows))
    }

    async fn consolidate_turn(
        &self,
        _user_message: &str,
        _assistant_response: &str,
    ) -> Result<(), MemoryError> {
        // LLM consolidation runs via ConsolidatingMemory wrapper, not here.
        // This stub exists for when SurrealMemoryAdapter is used unwrapped.
        Ok(())
    }

    fn should_skip_autosave(&self, content: &str) -> bool {
        let trimmed = content.trim();
        trimmed.is_empty()
            || trimmed.len() < 5
            || trimmed.starts_with("user_msg_")
            || trimmed.starts_with("assistant_autosave_")
    }

    async fn count(&self) -> Result<usize, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT count() AS total FROM episode GROUP ALL")
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let count = rows
            .first()
            .and_then(|v| v.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Ok(count as usize)
    }

    fn name(&self) -> &str {
        "surrealdb"
    }

    async fn health_check(&self) -> bool {
        self.db.health().await.is_ok()
    }
}
