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
    AgentId, ConsolidationReport, CoreMemoryBlock, EmbeddingProfile, Entity, HybridSearchResult,
    MemoryCategory, MemoryEntry, MemoryError, MemoryId, MemoryQuery, Reflection, ReflectionOutcome,
    SearchResult, SessionId, Skill, SkillUpdate, TemporalFact,
};
use synapse_domain::ports::memory::{
    ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
    UnifiedMemoryPort, WorkingMemoryPort,
};

use crate::embeddings::EmbeddingProvider;

/// Log memory operation latency after an async block completes.
#[allow(dead_code)]
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
    fn active_embedding_profile_id(&self) -> Option<String> {
        let profile = self.embedder.profile();
        (profile.dimensions > 0 && profile.provider_family != "none").then_some(profile.profile_id)
    }

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

    /// Wrap an existing SurrealDB handle (no schema init, no new connection).
    /// Used by API handlers that share the gateway's DB handle.
    pub fn from_existing(db: Arc<Surreal<Db>>, agent_id: String) -> Self {
        Self {
            db,
            embedder: Arc::new(crate::embeddings::NoopEmbedding),
            agent_id,
        }
    }

    /// Get a shared handle to the underlying SurrealDB instance.
    /// Used by other components (IPC, cron, chat, etc.) to share the same DB.
    pub fn db(&self) -> Arc<Surreal<Db>> {
        Arc::clone(&self.db)
    }

    async fn clear_vector_fields(&self, tables: &[&str]) {
        for table in tables {
            let query = format!(
                "UPDATE {table} SET embedding = NONE, embedding_profile_id = NONE WHERE embedding IS NOT NONE"
            );
            if let Err(e) = self.db.query(&query).await {
                tracing::warn!(
                    table,
                    "memory: failed to clear stale embeddings before reindex: {e}"
                );
            }
        }
    }

    async fn reindex_profile_bound_vectors(&self) -> Result<(), MemoryError> {
        let Some(profile_id) = self.active_embedding_profile_id() else {
            return Ok(());
        };

        let mut reindexed = 0usize;

        let mut episode_resp = self
            .db
            .query(
                "SELECT id, content FROM episode
                 WHERE content != NONE
                 AND (embedding_profile_id != $profile OR embedding_profile_id IS NONE)",
            )
            .bind(("profile", profile_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        let episode_rows: Vec<serde_json::Value> = episode_resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        for row in episode_rows {
            let id = json_str(&row, "id");
            let content = json_str(&row, "content");
            if content.trim().is_empty() {
                continue;
            }
            if let Ok(embedding) = self.embedder.embed_document(&content).await {
                self.db
                    .query(
                        "UPDATE type::thing('episode', $id) SET
                            embedding = $embedding,
                            embedding_profile_id = $profile",
                    )
                    .bind(("id", id))
                    .bind(("embedding", embedding))
                    .bind(("profile", profile_id.clone()))
                    .await
                    .map_err(|e| MemoryError::Storage(e.to_string()))?;
                reindexed += 1;
            }
        }

        let mut entity_resp = self
            .db
            .query(
                "SELECT id, name, summary FROM entity
                 WHERE name != NONE
                 AND (embedding_profile_id != $profile OR embedding_profile_id IS NONE)",
            )
            .bind(("profile", profile_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        let entity_rows: Vec<serde_json::Value> = entity_resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        for row in entity_rows {
            let id = json_str(&row, "id");
            let name = json_str(&row, "name");
            let summary = row.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            let text = format!("{name} {summary}");
            if text.trim().is_empty() {
                continue;
            }
            if let Ok(embedding) = self.embedder.embed_document(&text).await {
                self.db
                    .query(
                        "UPDATE type::thing('entity', $id) SET
                            embedding = $embedding,
                            embedding_profile_id = $profile",
                    )
                    .bind(("id", id))
                    .bind(("embedding", embedding))
                    .bind(("profile", profile_id.clone()))
                    .await
                    .map_err(|e| MemoryError::Storage(e.to_string()))?;
                reindexed += 1;
            }
        }

        let mut fact_resp = self
            .db
            .query(
                "SELECT id, subject, predicate, object FROM fact
                 WHERE (embedding_profile_id != $profile OR embedding_profile_id IS NONE)",
            )
            .bind(("profile", profile_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        let fact_rows: Vec<serde_json::Value> = fact_resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        for row in fact_rows {
            let id = json_str(&row, "id");
            let subject = json_str(&row, "subject");
            let predicate = json_str(&row, "predicate");
            let object = json_str(&row, "object");
            let text = format!("{subject} {predicate} {object}");
            if text.trim().is_empty() {
                continue;
            }
            if let Ok(embedding) = self.embedder.embed_document(&text).await {
                self.db
                    .query(
                        "UPDATE type::thing('fact', $id) SET
                            embedding = $embedding,
                            embedding_profile_id = $profile",
                    )
                    .bind(("id", id))
                    .bind(("embedding", embedding))
                    .bind(("profile", profile_id.clone()))
                    .await
                    .map_err(|e| MemoryError::Storage(e.to_string()))?;
                reindexed += 1;
            }
        }

        tracing::info!(profile = %profile_id, count = reindexed, "memory: embedding profile reindex complete");
        Ok(())
    }

    async fn apply_schema(&self) -> Result<(), MemoryError> {
        // Apply base schema (tables, fields, standard indexes).
        let schema = include_str!("surrealdb_schema.surql");
        self.db
            .query(schema)
            .await
            .map_err(|e| MemoryError::Storage(format!("Schema apply: {e}")))?;

        // Apply BM25 full-text search index.
        // SurrealDB 3.0 changed syntax: SEARCH ANALYZER → FULLTEXT ANALYZER.
        // We REMOVE first (idempotent) then re-DEFINE on every startup.
        // Without this index, recall() returns empty because @1@ requires it.
        let _ = self
            .db
            .query("REMOVE INDEX IF EXISTS idx_ep_content ON episode")
            .await;
        if let Err(e) = self
            .db
            .query("DEFINE INDEX idx_ep_content ON episode FIELDS content FULLTEXT ANALYZER simple_analyzer BM25")
            .await
        {
            tracing::error!("BM25 FULLTEXT index creation failed: {e} — recall() will return empty results!");
        } else {
            tracing::info!("BM25 FULLTEXT index created on episode.content");
        }

        // Apply HNSW vector indexes — auto-detect dimension mismatch and recreate.
        let dim = self.embedder.dimensions();
        if dim > 0 {
            // Probe existing index dimension from INFO FOR TABLE on the first table.
            let need_recreate = match self.db.query("INFO FOR TABLE episode").await {
                Ok(mut r) => {
                    let info: Option<serde_json::Value> = r.take(0).unwrap_or(None);
                    let existing_dim = info
                        .as_ref()
                        .and_then(|v| v.get("indexes"))
                        .and_then(|v| v.get("idx_ep_vector"))
                        .and_then(|v| {
                            let s = v.as_str().unwrap_or("");
                            // Parse "DIMENSION <N>" from the index definition string.
                            s.find("DIMENSION ")
                                .map(|pos| &s[pos + 10..])
                                .and_then(|rest| rest.split_whitespace().next())
                                .and_then(|n| n.parse::<usize>().ok())
                        });
                    match existing_dim {
                        Some(d) if d == dim => false, // dimensions match, no action needed
                        Some(d) => {
                            tracing::warn!(
                                old_dim = d,
                                new_dim = dim,
                                "HNSW dimension mismatch detected — recreating vector indexes"
                            );
                            true
                        }
                        None => true, // no index exists yet
                    }
                }
                Err(_) => true, // can't probe, recreate to be safe
            };

            if need_recreate {
                self.clear_vector_fields(&["episode", "entity", "fact", "skill", "reflection"])
                    .await;
            }

            for (table, idx) in [
                ("episode", "idx_ep_vector"),
                ("entity", "idx_ent_vector"),
                ("fact", "idx_fact_vector"),
                ("skill", "idx_skill_vector"),
                ("reflection", "idx_refl_vector"),
            ] {
                let q = if need_recreate {
                    format!(
                        "REMOVE INDEX IF EXISTS {idx} ON {table}; \
                         DEFINE INDEX {idx} ON {table} FIELDS embedding HNSW DIMENSION {dim} DIST COSINE"
                    )
                } else {
                    format!(
                        "DEFINE INDEX IF NOT EXISTS {idx} ON {table} FIELDS embedding HNSW DIMENSION {dim} DIST COSINE"
                    )
                };
                if let Err(e) = self.db.query(&q).await {
                    tracing::warn!("HNSW index {idx} failed: {e}");
                }
            }
            if need_recreate {
                tracing::info!(dim, "HNSW vector indexes recreated (dimension changed)");
            } else {
                tracing::debug!(dim, "HNSW vector indexes verified");
            }

            self.reindex_profile_bound_vectors().await?;
        }

        // Migrate stale agent_ids to the current agent_id.
        // Before PR #230, CLI and daemon modes could use different IDs ("cli", "default")
        // for the same logical agent. Reassign orphaned episodes so recall() can find them.
        let stale_ids = ["cli", "default"];
        for stale in &stale_ids {
            if *stale == self.me() {
                continue; // current agent already uses this ID
            }
            match self
                .db
                .query("UPDATE episode SET agent_id = $new WHERE agent_id = $old RETURN BEFORE")
                .bind(("new", self.me().to_string()))
                .bind(("old", stale.to_string()))
                .await
            {
                Ok(mut r) => {
                    let migrated: Vec<serde_json::Value> = r.take(0).unwrap_or_default();
                    if !migrated.is_empty() {
                        tracing::info!(
                            from = *stale,
                            to = self.me(),
                            count = migrated.len(),
                            "memory: migrated stale agent_id episodes"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(from = *stale, "memory: agent_id migration failed: {e}");
                }
            }
            // Also migrate core_memory blocks.
            if let Err(e) = self
                .db
                .query("UPDATE core_memory SET agent_id = $new WHERE agent_id = $old")
                .bind(("new", self.me().to_string()))
                .bind(("old", stale.to_string()))
                .await
            {
                tracing::warn!(from = *stale, "memory: core_memory migration failed: {e}");
            }
            // Migrate skill, reflection, and entity tables for consistent agent scoping.
            for (table, col) in [
                ("skill", "created_by"),
                ("reflection", "agent_id"),
                ("entity", "created_by"),
            ] {
                let q = format!("UPDATE {table} SET {col} = $new WHERE {col} = $old");
                if let Err(e) = self
                    .db
                    .query(&q)
                    .bind(("new", self.me().to_string()))
                    .bind(("old", stale.to_string()))
                    .await
                {
                    tracing::warn!(
                        from = *stale,
                        table,
                        "memory: {table} agent_id migration failed: {e}"
                    );
                }
            }
        }

        // Diagnostic: report episode count and agent_id distribution.
        match self.db.query(
            "SELECT count() AS total FROM episode GROUP ALL;
             SELECT agent_id, count() AS cnt FROM episode GROUP BY agent_id ORDER BY cnt DESC LIMIT 10"
        ).await {
            Ok(mut r) => {
                let total_rows: Vec<serde_json::Value> = r.take(0).unwrap_or_default();
                let count = total_rows.first()
                    .and_then(|v| v.get("total"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agent_rows: Vec<serde_json::Value> = r.take(1).unwrap_or_default();
                let agents: Vec<String> = agent_rows.iter().map(|v| {
                    let aid = v.get("agent_id").and_then(|a| a.as_str()).unwrap_or("NULL");
                    let cnt = v.get("cnt").and_then(|c| c.as_u64()).unwrap_or(0);
                    format!("{}={}", aid, cnt)
                }).collect();
                tracing::info!(
                    episode_count = count,
                    my_agent_id = self.me(),
                    agent_ids = %agents.join(", "),
                    "SurrealDB memory schema applied"
                );
            }
            Err(e) => {
                tracing::warn!("SurrealDB schema applied but episode count failed: {e}");
            }
        }
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
        embedding: None,
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
                "IF (SELECT count() FROM core_memory WHERE agent_id = $agent AND label = $label GROUP ALL)[0].count > 0 {
                    UPDATE core_memory SET content = string::concat(content, '\n', $text), updated_at = time::now()
                        WHERE agent_id = $agent AND label = $label;
                } ELSE {
                    CREATE core_memory SET agent_id = $agent, label = $label, content = $text, max_tokens = 2000, updated_at = time::now();
                };",
            )
            .bind(("agent", agent_id.clone()))
            .bind(("label", label.to_string()))
            .bind(("text", text.to_string()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        // Enforce max size: trim oldest content if block exceeds 2000 chars.
        const MAX_BLOCK_CHARS: usize = 2000;
        if let Ok(blocks) = self.get_core_blocks(agent_id).await {
            if let Some(block) = blocks.iter().find(|b| b.label == label) {
                if block.content.len() > MAX_BLOCK_CHARS {
                    let trimmed = &block.content[block.content.len() - MAX_BLOCK_CHARS..];
                    let start = trimmed.find('\n').map(|i| i + 1).unwrap_or(0);
                    let final_content = trimmed[start..].to_string();
                    let _ = self
                        .db
                        .query(
                            "UPDATE core_memory SET content = $content, updated_at = time::now()
                             WHERE agent_id = $agent AND label = $label",
                        )
                        .bind(("content", final_content))
                        .bind(("agent", agent_id.clone()))
                        .bind(("label", label.to_string()))
                        .await;
                    tracing::info!(label, "memory.core_block.trimmed");
                }
            }
        }

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
        let embedding_profile_id = self.active_embedding_profile_id();

        // Generate embedding if provider is available (best-effort).
        let embedding: Option<Vec<f32>> = if self.embedder.dimensions() > 0 {
            match self.embedder.embed_document(&entry.content).await {
                Ok(emb) => {
                    tracing::info!(dims = emb.len(), "memory.embedding.stored");
                    Some(emb)
                }
                Err(e) => {
                    tracing::warn!(op = "store_episode", "Embedding failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        let mut resp = self
            .db
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
                    embedding = $embedding,
                    embedding_profile_id = $embedding_profile_id",
            )
            .bind(("agent", self.me().to_string()))
            .bind(("key", entry.key))
            .bind(("content", entry.content))
            .bind(("category", entry.category.to_string()))
            .bind(("session_id", entry.session_id))
            .bind(("embedding", embedding))
            .bind(("embedding_profile_id", embedding_profile_id))
            .await
            .map_err(|e| MemoryError::Storage(format!("store_episode transport: {e}")))?;

        // Check for query-level errors (not just transport errors).
        let created: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| MemoryError::Storage(format!("store_episode query: {e}")))?;

        if created.is_empty() {
            tracing::warn!(
                agent = self.me(),
                "store_episode: CREATE returned 0 rows — data may not have been persisted"
            );
        } else {
            tracing::debug!(
                agent = self.me(),
                rows = created.len(),
                "store_episode: created"
            );
        }

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
                None => match self.embedder.embed_query(&query.text).await {
                    Ok(emb) => Some(emb),
                    Err(e) => {
                        tracing::warn!(op = "search_episodes", "Embedding failed: {e}");
                        None
                    }
                },
            }
        } else {
            None
        };

        let vec_rows = if let Some(ref emb) = query_embedding {
            let embedding_profile_id = self.active_embedding_profile_id();
            let mut vec_resp = self
                .db
                .query(
                    "SELECT *,
                        vector::similarity::cosine(embedding, $emb) AS vec_score
                     FROM episode
                     WHERE embedding <|$limit,64|> $emb
                     AND embedding_profile_id = $profile
                     AND (agent_id = $agent OR visibility = 'global' OR $agent INSIDE shared_with)
                     ORDER BY vec_score DESC
                     LIMIT $limit",
                )
                .bind(("emb", emb.clone()))
                .bind(("profile", embedding_profile_id))
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
            .map(|v| {
                let id = json_str(v, "id");
                let score = v.get("bm25_score").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                (id, score)
            })
            .collect();

        let vec_list: Vec<(String, f32)> = vec_rows
            .iter()
            .map(|v| {
                let id = json_str(v, "id");
                let score = v.get("vec_score").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                (id, score)
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

        let mut results: Vec<SearchResult> = fused
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
            .collect();

        // Apply full retention scoring: relevance + recency + category importance + frequency.
        {
            use synapse_domain::application::services::retention::{
                compute_retention_score, RetentionPolicy, RetentionWeights,
            };
            let policy = RetentionPolicy::default();
            let weights = RetentionWeights::default();
            for r in &mut results {
                let age_hours =
                    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&r.entry.timestamp) {
                        (chrono::Utc::now() - ts.with_timezone(&chrono::Utc))
                            .num_hours()
                            .max(0) as f64
                    } else {
                        0.0
                    };
                // Extract access_count from the search row (stored in episode table).
                let access_count = row_map
                    .get(&r.entry.id)
                    .and_then(|row| row.get("access_count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let retention = compute_retention_score(
                    r.score as f64,
                    age_hours,
                    access_count,
                    &r.entry.category,
                    &policy,
                    &weights,
                );
                r.score = retention.total as f32;
            }
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        tracing::info!(
            bm25_results = bm25_rows.len(),
            vector_results = vec_rows.len(),
            fused = results.len(),
            "memory.search.hybrid"
        );

        Ok(results)
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
        let embedding_profile_id = self.active_embedding_profile_id();

        // Generate embedding for entity name + summary (best-effort).
        let embed_text = format!(
            "{} {}",
            entity.name,
            entity.summary.as_deref().unwrap_or("")
        );
        let embedding: Option<Vec<f32>> = if self.embedder.dimensions() > 0 {
            match self.embedder.embed_document(&embed_text).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!(op = "upsert_entity", "Embedding failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        self.db
            .query(
                "IF (SELECT count() FROM entity WHERE string::lowercase(name) = string::lowercase($name) AND created_by = $agent GROUP ALL)[0].count > 0 {
                    UPDATE entity SET
                        entity_type = $etype,
                        properties = $props,
                        summary = $summary,
                        embedding = $embedding,
                        embedding_profile_id = $embedding_profile_id,
                        updated_at = time::now()
                    WHERE string::lowercase(name) = string::lowercase($name) AND created_by = $agent;
                } ELSE {
                    CREATE entity SET
                        name = $name,
                        entity_type = $etype,
                        properties = $props,
                        summary = $summary,
                        embedding = $embedding,
                        embedding_profile_id = $embedding_profile_id,
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
            .bind(("embedding_profile_id", embedding_profile_id))
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
            let emb_result = self.embedder.embed_document(name).await;
            if let Err(ref e) = emb_result {
                tracing::warn!(op = "find_entity", "Embedding failed: {e}");
            }
            if let Ok(emb) = emb_result {
                let embedding_profile_id = self.active_embedding_profile_id();
                let mut vec_resp = self
                    .db
                    .query(
                        "SELECT *,
                            vector::similarity::cosine(embedding, $emb) AS sim
                         FROM entity
                         WHERE embedding <|3,32|> $emb
                         AND embedding_profile_id = $profile
                         ORDER BY sim DESC
                         LIMIT 1",
                    )
                    .bind(("emb", emb))
                    .bind(("profile", embedding_profile_id))
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
        let embedding_profile_id = self.active_embedding_profile_id();

        // Use pre-computed embedding if provided (from AUDN in entity_extractor).
        // Fallback: embed using predicate text (for knowledge_tool and other callers).
        let embedding: Option<Vec<f32>> = if fact.embedding.is_some() {
            fact.embedding.clone()
        } else if self.embedder.dimensions() > 0 {
            let fact_text = format!("{} {} {}", fact.subject, fact.predicate, fact.object);
            match self.embedder.embed_document(&fact_text).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!(op = "add_fact", "Embedding failed: {e}");
                    None
                }
            }
        } else {
            None
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
                    created_by = $agent,
                    embedding = $embedding,
                    embedding_profile_id = $embedding_profile_id",
            )
            .bind(("subj", fact.subject))
            .bind(("pred", fact.predicate))
            .bind(("obj", fact.object))
            .bind(("conf", fact.confidence))
            .bind(("source", fact.source_episode))
            .bind(("agent", fact.created_by))
            .bind(("embedding", embedding))
            .bind(("embedding_profile_id", embedding_profile_id))
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
                 WHERE (name CONTAINS $text OR summary CONTAINS $text)
                 AND created_by = $agent
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("agent", query.agent_id.clone()))
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
                 WHERE (name CONTAINS $text OR description CONTAINS $text)
                 AND created_by = $agent
                 ORDER BY success_count DESC
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("agent", query.agent_id.clone()))
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
        agent_id: &AgentId,
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

        let q = format!(
            "UPDATE skill SET {} WHERE id = $id AND created_by = $agent",
            parts.join(", ")
        );

        let mut query = self
            .db
            .query(&q)
            .bind(("id", skill_id.clone()))
            .bind(("agent", agent_id.clone()));
        if let Some(ref content) = update.new_content {
            query = query.bind(("content", content.clone()));
        }

        query
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_skill(
        &self,
        name: &str,
        agent_id: &AgentId,
    ) -> Result<Option<Skill>, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT * FROM skill WHERE name = $name AND created_by = $agent LIMIT 1")
            .bind(("name", name.to_string()))
            .bind(("agent", agent_id.clone()))
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
                 WHERE (lesson CONTAINS $text OR task_summary CONTAINS $text)
                 AND agent_id = $agent
                 ORDER BY created_at DESC
                 LIMIT $limit",
            )
            .bind(("text", query.text.clone()))
            .bind(("agent", query.agent_id.clone()))
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

        // Category-aware decay: each category has its own decay rate
        // derived from half-life. Decay per hourly cycle = 0.5^(1/half_life_hours).
        use synapse_domain::application::services::retention::RetentionPolicy;
        let policy = RetentionPolicy::default();
        let categories = [
            ("conversation", policy.conversation_half_life_hours),
            ("daily", policy.daily_half_life_hours),
            ("reflection", policy.reflection_half_life_hours),
            ("core", policy.core_half_life_hours),
            ("skill", policy.skill_half_life_hours),
            ("entity", policy.core_half_life_hours),
        ];
        for (cat, half_life) in &categories {
            let decay_factor = (0.5_f64).powf(1.0 / half_life);
            let q = format!(
                "UPDATE episode SET importance = importance * {decay_factor}
                 WHERE category = '{cat}' AND created_at < time::now() - 1d AND importance > 0.05"
            );
            let _ = self
                .db
                .query(&q)
                .await
                .map_err(|e| MemoryError::Storage(e.to_string()));
        }

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

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embedder
            .embed_query(text)
            .await
            .map_err(|e| MemoryError::Embedding(e.to_string()))
    }

    async fn embed_document(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embedder
            .embed_document(text)
            .await
            .map_err(|e| MemoryError::Embedding(e.to_string()))
    }

    fn embedding_profile(&self) -> EmbeddingProfile {
        self.embedder.profile()
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
        let mq = MemoryQuery {
            text: query.to_string(),
            embedding: None,
            agent_id: self.me().to_string(),
            include_shared: true,
            time_range: None,
            limit,
        };

        let results = self.search_episodes(&mq).await?;

        let entries: Vec<MemoryEntry> = results
            .into_iter()
            .filter(|r| session_id.map_or(true, |sid| r.entry.session_id.as_deref() == Some(sid)))
            .map(|r| {
                let mut entry = r.entry;
                entry.score = Some(r.score as f64);
                entry
            })
            .collect();

        // Bump access_count + last_accessed for returned entries (fire-and-forget).
        if !entries.is_empty() {
            let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
            let db = self.db.clone();
            tokio::spawn(async move {
                for id in ids {
                    let _ = db
                        .query(
                            "UPDATE type::thing('episode', $id) SET \
                             access_count = (access_count ?? 0) + 1, \
                             last_accessed = time::now()",
                        )
                        .bind(("id", id))
                        .await;
                }
            });
        }

        Ok(entries)
    }

    async fn forget(&self, key: &str, agent_id: &AgentId) -> Result<bool, MemoryError> {
        let mut resp = self
            .db
            .query("DELETE FROM episode WHERE key = $key AND agent_id = $agent RETURN BEFORE")
            .bind(("key", key.to_string()))
            .bind(("agent", agent_id.clone()))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| MemoryError::Storage(e.to_string()))?;
        Ok(!rows.is_empty())
    }

    async fn get(&self, key: &str, agent_id: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> {
        let mut resp = self
            .db
            .query("SELECT * FROM episode WHERE key = $key AND agent_id = $agent LIMIT 1")
            .bind(("key", key.to_string()))
            .bind(("agent", agent_id.clone()))
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

    async fn find_similar_facts(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(TemporalFact, f32)>, MemoryError> {
        if embedding.is_empty() {
            return Ok(vec![]);
        }

        let mut resp = self
            .db
            .query(
                "SELECT *,
                    vector::similarity::cosine(embedding, $emb) AS sim
                 FROM fact
                 WHERE embedding <|$limit,64|> $emb
                 AND embedding_profile_id = $profile
                 AND valid_to IS NONE
                 AND (created_by = $agent OR created_by IS NONE)
                 ORDER BY sim DESC
                 LIMIT $limit",
            )
            .bind(("emb", embedding.to_vec()))
            .bind(("profile", self.active_embedding_profile_id()))
            .bind(("agent", self.me().to_string()))
            .bind(("limit", limit))
            .await
            .map_err(|e| MemoryError::Storage(e.to_string()))?;

        let rows = take_json!(resp, 0);
        Ok(rows
            .iter()
            .filter_map(|v| {
                let fact = row_to_fact(v)?;
                let sim = v.get("sim").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
                Some((fact, sim))
            })
            .collect())
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

    async fn list_signal_patterns(
        &self,
    ) -> Result<
        Vec<synapse_domain::application::services::learning_signals::SignalPattern>,
        MemoryError,
    > {
        // Delegate to the inherent method on SurrealMemoryAdapter
        SurrealMemoryAdapter::list_signal_patterns(self).await
    }

    async fn promote_visibility(
        &self,
        entry_id: &synapse_domain::domain::memory::MemoryId,
        visibility: &synapse_domain::domain::memory::Visibility,
        shared_with: &[synapse_domain::domain::memory::AgentId],
        agent_id: &synapse_domain::domain::memory::AgentId,
    ) -> Result<(), synapse_domain::domain::memory::MemoryError> {
        let vis_str = match visibility {
            synapse_domain::domain::memory::Visibility::Private => "private",
            synapse_domain::domain::memory::Visibility::SharedWith(_) => "shared",
            synapse_domain::domain::memory::Visibility::Global => "global",
        };
        self.db
            .query(
                "UPDATE type::thing('episode', $id) SET \
                 visibility = $vis, shared_with = $agents \
                 WHERE agent_id = $owner",
            )
            .bind(("id", entry_id.clone()))
            .bind(("vis", vis_str.to_string()))
            .bind(("agents", shared_with.to_vec()))
            .bind(("owner", agent_id.clone()))
            .await
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        tracing::debug!(
            target: "memory_sharing",
            entry_id,
            visibility = vis_str,
            shared_with = ?shared_with,
            "Visibility promoted"
        );
        Ok(())
    }
}

// ── Learning Signal Patterns ─────────────────────────────────────

impl SurrealMemoryAdapter {
    /// Load all signal patterns from DB.
    pub async fn list_signal_patterns(
        &self,
    ) -> Result<
        Vec<synapse_domain::application::services::learning_signals::SignalPattern>,
        synapse_domain::domain::memory::MemoryError,
    > {
        use synapse_domain::application::services::learning_signals::SignalPattern;
        let mut resp = self
            .db
            .query("SELECT * FROM learning_signal_pattern ORDER BY signal_type, pattern")
            .await
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        Ok(rows
            .iter()
            .filter_map(|v| {
                Some(SignalPattern {
                    id: v.get("id")?.to_string().trim_matches('"').to_string(),
                    signal_type: v.get("signal_type")?.as_str()?.to_string(),
                    pattern: v.get("pattern")?.as_str()?.to_string(),
                    match_mode: v
                        .get("match_mode")?
                        .as_str()
                        .unwrap_or("starts_with")
                        .to_string(),
                    language: v.get("language")?.as_str().unwrap_or("en").to_string(),
                    enabled: v.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                })
            })
            .collect())
    }

    /// Add a new signal pattern.
    pub async fn add_signal_pattern(
        &self,
        pattern: &synapse_domain::application::services::learning_signals::SignalPattern,
    ) -> Result<String, synapse_domain::domain::memory::MemoryError> {
        let mut resp = self
            .db
            .query(
                "CREATE learning_signal_pattern SET \
                 signal_type = $signal_type, \
                 pattern = $pattern, \
                 match_mode = $match_mode, \
                 language = $language, \
                 enabled = $enabled, \
                 created_at = time::now() \
                 RETURN id",
            )
            .bind(("signal_type", pattern.signal_type.clone()))
            .bind(("pattern", pattern.pattern.clone()))
            .bind(("match_mode", pattern.match_mode.clone()))
            .bind(("language", pattern.language.clone()))
            .bind(("enabled", pattern.enabled))
            .await
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        let id = rows
            .first()
            .and_then(|v| v.get("id"))
            .map(|v| v.to_string().trim_matches('"').to_string())
            .unwrap_or_default();
        Ok(id)
    }

    /// Delete a signal pattern by ID.
    pub async fn delete_signal_pattern(
        &self,
        id: &str,
    ) -> Result<bool, synapse_domain::domain::memory::MemoryError> {
        let mut resp = self
            .db
            .query("DELETE FROM learning_signal_pattern WHERE id = type::thing('learning_signal_pattern', $id) RETURN BEFORE")
            .bind(("id", id.to_string()))
            .await
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| synapse_domain::domain::memory::MemoryError::Storage(e.to_string()))?;
        Ok(!rows.is_empty())
    }

    /// Seed default patterns if table is empty.
    pub async fn seed_default_signal_patterns(
        &self,
    ) -> Result<usize, synapse_domain::domain::memory::MemoryError> {
        let existing = self.list_signal_patterns().await?;
        if !existing.is_empty() {
            return Ok(0);
        }
        let defaults = synapse_domain::application::services::learning_signals::default_patterns();
        let count = defaults.len();
        for pat in &defaults {
            self.add_signal_pattern(pat).await?;
        }
        tracing::info!(count, "Seeded default learning signal patterns");
        Ok(count)
    }
}

// ── DeadLetterPort (Phase 4.5) ─────────────────────────────────

use synapse_domain::domain::pipeline_context::{DeadLetter, DeadLetterStatus};
use synapse_domain::ports::dead_letter::DeadLetterPort;

fn row_to_dead_letter(v: &serde_json::Value) -> Option<DeadLetter> {
    let status_str = v.get("status")?.as_str()?;
    Some(DeadLetter {
        id: json_str(v, "dl_id"),
        pipeline_run_id: json_str(v, "pipeline_run_id"),
        step_id: json_str(v, "step_id"),
        agent_id: json_str(v, "agent_id"),
        input: v.get("input").cloned().unwrap_or(serde_json::Value::Null),
        error: json_str(v, "error"),
        attempt: v.get("attempt").and_then(|a| a.as_u64()).unwrap_or(0) as u32,
        max_retries: v.get("max_retries").and_then(|a| a.as_u64()).unwrap_or(0) as u32,
        created_at: v
            .get("created_at")
            .and_then(|t| t.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(0),
        status: match status_str {
            "retried" => DeadLetterStatus::Retried,
            "dismissed" => DeadLetterStatus::Dismissed,
            _ => DeadLetterStatus::Pending,
        },
        retried_at: v
            .get("retried_at")
            .and_then(|t| t.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp()),
        dismissed_by: v
            .get("dismissed_by")
            .and_then(|s| s.as_str())
            .map(String::from),
    })
}

#[async_trait]
impl DeadLetterPort for SurrealMemoryAdapter {
    async fn enqueue(&self, letter: DeadLetter) -> anyhow::Result<()> {
        self.db
            .query(
                "CREATE dead_letter SET
                    dl_id = $dl_id,
                    pipeline_run_id = $run_id,
                    step_id = $step_id,
                    agent_id = $agent_id,
                    input = $input,
                    error = $error,
                    attempt = $attempt,
                    max_retries = $max_retries,
                    created_at = time::now(),
                    status = $status",
            )
            .bind(("dl_id", letter.id))
            .bind(("run_id", letter.pipeline_run_id))
            .bind(("step_id", letter.step_id))
            .bind(("agent_id", letter.agent_id))
            .bind(("input", letter.input))
            .bind(("error", letter.error))
            .bind(("attempt", letter.attempt as i64))
            .bind(("max_retries", letter.max_retries as i64))
            .bind(("status", letter.status.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("DLQ enqueue failed: {e}"))?;
        Ok(())
    }

    async fn list_pending(&self, limit: usize) -> anyhow::Result<Vec<DeadLetter>> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM dead_letter
                 WHERE status = 'pending'
                 ORDER BY created_at DESC
                 LIMIT $limit",
            )
            .bind(("limit", limit as i64))
            .await
            .map_err(|e| anyhow::anyhow!("DLQ list_pending failed: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("DLQ response parse: {e}"))?;
        Ok(rows.iter().filter_map(row_to_dead_letter).collect())
    }

    async fn list_all(&self, limit: usize) -> anyhow::Result<Vec<DeadLetter>> {
        let mut resp = self
            .db
            .query(
                "SELECT * FROM dead_letter
                 ORDER BY created_at DESC
                 LIMIT $limit",
            )
            .bind(("limit", limit as i64))
            .await
            .map_err(|e| anyhow::anyhow!("DLQ list_all failed: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("DLQ response parse: {e}"))?;
        Ok(rows.iter().filter_map(row_to_dead_letter).collect())
    }

    async fn mark_retried(&self, id: &str) -> anyhow::Result<()> {
        let mut resp = self
            .db
            .query(
                "UPDATE dead_letter SET status = 'retried', retried_at = time::now()
                 WHERE dl_id = $dl_id AND status = 'pending'
                 RETURN AFTER",
            )
            .bind(("dl_id", id.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("DLQ mark_retried failed: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("DLQ response parse: {e}"))?;
        if rows.is_empty() {
            anyhow::bail!("dead letter '{id}' not found or not pending");
        }
        Ok(())
    }

    async fn dismiss(&self, id: &str, by: &str) -> anyhow::Result<()> {
        let mut resp = self
            .db
            .query(
                "UPDATE dead_letter SET status = 'dismissed', dismissed_by = $by
                 WHERE dl_id = $dl_id AND status = 'pending'
                 RETURN AFTER",
            )
            .bind(("dl_id", id.to_string()))
            .bind(("by", by.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("DLQ dismiss failed: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("DLQ response parse: {e}"))?;
        if rows.is_empty() {
            anyhow::bail!("dead letter '{id}' not found or not pending");
        }
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<DeadLetter>> {
        let mut resp = self
            .db
            .query("SELECT * FROM dead_letter WHERE dl_id = $dl_id LIMIT 1")
            .bind(("dl_id", id.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("DLQ get failed: {e}"))?;
        let rows: Vec<serde_json::Value> = resp
            .take(0)
            .map_err(|e| anyhow::anyhow!("DLQ response parse: {e}"))?;
        Ok(rows.first().and_then(row_to_dead_letter))
    }
}
