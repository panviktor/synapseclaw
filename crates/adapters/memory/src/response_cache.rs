//! Response cache — avoid burning tokens on repeated prompts.
//!
//! Stores LLM responses in SurrealDB keyed by a SHA-256 hash of
//! `(model, system_prompt_hash, user_prompt)`. Entries expire after a
//! configurable TTL (default: 1 hour). The cache is optional and disabled by
//! default — users opt in via `[memory] response_cache_enabled = true`.

use anyhow::Result;
use chrono::{Duration, Local};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::Surreal;

/// An in-memory hot cache entry for the two-tier response cache.
struct InMemoryEntry {
    response: String,
    token_count: u32,
    created_at: std::time::Instant,
    accessed_at: std::time::Instant,
}

/// Two-tier response cache: in-memory LRU (hot) + SurrealDB (warm).
///
/// The hot cache avoids SurrealDB round-trips for frequently repeated prompts.
/// On miss from hot cache, falls through to SurrealDB. On hit from SurrealDB,
/// the entry is promoted to the hot cache.
pub struct ResponseCache {
    db: Arc<Surreal<Db>>,
    ttl_minutes: i64,
    max_entries: usize,
    hot_cache: Mutex<HashMap<String, InMemoryEntry>>,
    hot_max_entries: usize,
}

impl ResponseCache {
    /// Create a response cache backed by a shared SurrealDB instance.
    pub fn new_with_surreal(db: Arc<Surreal<Db>>, ttl_minutes: u32, max_entries: usize) -> Self {
        Self::with_hot_cache_surreal(db, ttl_minutes, max_entries, 256)
    }

    /// Create a response cache with a custom hot cache size.
    pub fn with_hot_cache_surreal(
        db: Arc<Surreal<Db>>,
        ttl_minutes: u32,
        max_entries: usize,
        hot_max_entries: usize,
    ) -> Self {
        Self {
            db,
            ttl_minutes: i64::from(ttl_minutes),
            max_entries,
            hot_cache: Mutex::new(HashMap::new()),
            hot_max_entries,
        }
    }

    /// Build a deterministic cache key from model + system prompt + user prompt.
    pub fn cache_key(model: &str, system_prompt: Option<&str>, user_prompt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(model.as_bytes());
        hasher.update(b"|");
        if let Some(sys) = system_prompt {
            hasher.update(sys.as_bytes());
        }
        hasher.update(b"|");
        hasher.update(user_prompt.as_bytes());
        let hash = hasher.finalize();
        hex::encode(hash)
    }

    /// Check the hot cache synchronously. Returns `Some(response)` on hit,
    /// `None` on miss or expired entry. This is a separate method to avoid
    /// holding a `parking_lot::MutexGuard` across an `.await` point.
    #[allow(clippy::cast_sign_loss)]
    fn check_hot_cache(&self, key: &str) -> Option<String> {
        let mut hot = self.hot_cache.lock();
        let entry = hot.get_mut(key)?;
        let ttl = std::time::Duration::from_secs(self.ttl_minutes as u64 * 60);
        if entry.created_at.elapsed() > ttl {
            hot.remove(key);
            None
        } else {
            entry.accessed_at = std::time::Instant::now();
            Some(entry.response.clone())
        }
    }

    /// Look up a cached response. Returns `None` on miss or expired entry.
    ///
    /// Two-tier lookup: checks the in-memory hot cache first, then falls
    /// through to SurrealDB. On a SurrealDB hit the entry is promoted to hot cache.
    #[allow(clippy::cast_sign_loss)]
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        // Tier 1: hot cache (with TTL check)
        if let Some(response) = self.check_hot_cache(key) {
            // Still bump SurrealDB hit count for accurate stats
            let now_str = Local::now().to_rfc3339();
            let _ = self
                .db
                .query(
                    "UPDATE response_cache SET accessed_at = $now, hit_count = hit_count + 1 WHERE prompt_hash = $hash",
                )
                .bind(("now", now_str))
                .bind(("hash", key.to_string()))
                .await;
            return Ok(Some(response));
        }

        // Tier 2: SurrealDB (warm)
        let now = Local::now();
        let cutoff = (now - Duration::minutes(self.ttl_minutes)).to_rfc3339();

        let mut resp = self
            .db
            .query(
                "SELECT response, token_count FROM response_cache WHERE prompt_hash = $hash AND created_at > $cutoff LIMIT 1",
            )
            .bind(("hash", key.to_string()))
            .bind(("cutoff", cutoff))
            .await?;

        let rows: Vec<serde_json::Value> = resp.take(0)?;
        let result: Option<(String, u32)> = rows.first().and_then(|row| {
            let response = row.get("response")?.as_str()?.to_string();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let token_count = row.get("token_count")?.as_u64()? as u32;
            Some((response, token_count))
        });

        if result.is_some() {
            let now_str = now.to_rfc3339();
            let _ = self
                .db
                .query(
                    "UPDATE response_cache SET accessed_at = $now, hit_count = hit_count + 1 WHERE prompt_hash = $hash",
                )
                .bind(("now", now_str))
                .bind(("hash", key.to_string()))
                .await;
        }

        if let Some((ref response, token_count)) = result {
            self.promote_to_hot(key, response, token_count);
        }

        Ok(result.map(|(r, _)| r))
    }

    /// Store a response in the cache (both hot and warm tiers).
    pub async fn put(
        &self,
        key: &str,
        model: &str,
        response: &str,
        token_count: u32,
    ) -> Result<()> {
        // Write to hot cache
        self.promote_to_hot(key, response, token_count);

        // Write to SurrealDB (warm) — upsert by prompt_hash
        let now = Local::now().to_rfc3339();

        self.db
            .query(
                "IF (SELECT count() FROM response_cache WHERE prompt_hash = $hash GROUP ALL)[0].count > 0 {
                    UPDATE response_cache SET model = $model, response = $response, token_count = $tc, created_at = $now, accessed_at = $now, hit_count = 0
                    WHERE prompt_hash = $hash
                } ELSE {
                    CREATE response_cache SET prompt_hash = $hash, model = $model, response = $response, token_count = $tc, created_at = $now, accessed_at = $now, hit_count = 0
                };",
            )
            .bind(("hash", key.to_string()))
            .bind(("model", model.to_string()))
            .bind(("response", response.to_string()))
            .bind(("tc", token_count))
            .bind(("now", now))
            .await?;

        // Evict expired entries
        let cutoff = (Local::now() - Duration::minutes(self.ttl_minutes)).to_rfc3339();
        self.db
            .query("DELETE FROM response_cache WHERE created_at <= $cutoff")
            .bind(("cutoff", cutoff))
            .await?;

        // LRU eviction if over max_entries
        #[allow(clippy::cast_possible_wrap)]
        let max = self.max_entries as i64;

        // SurrealDB doesn't support DELETE ... ORDER BY ... LIMIT in a single statement
        // the same way SQLite does, so we count first then delete the oldest excess.
        let mut count_resp = self
            .db
            .query("SELECT count() FROM response_cache GROUP ALL")
            .await?;
        let count_rows: Vec<serde_json::Value> = count_resp.take(0)?;
        let total = count_rows
            .first()
            .and_then(|r| r.get("count"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);

        if total > max {
            let excess = total - max;
            self.db
                .query(
                    "LET $old = (SELECT id, accessed_at FROM response_cache ORDER BY accessed_at ASC LIMIT $excess);
                     FOR $row IN $old { DELETE response_cache WHERE id = $row.id; };",
                )
                .bind(("excess", excess))
                .await?;
        }

        Ok(())
    }

    /// Promote an entry to the in-memory hot cache, evicting the oldest if full.
    fn promote_to_hot(&self, key: &str, response: &str, token_count: u32) {
        let mut hot = self.hot_cache.lock();

        // If already present, just update (keep original created_at for TTL)
        if let Some(entry) = hot.get_mut(key) {
            entry.response = response.to_string();
            entry.token_count = token_count;
            entry.accessed_at = std::time::Instant::now();
            return;
        }

        // Evict oldest entry if at capacity
        if self.hot_max_entries > 0 && hot.len() >= self.hot_max_entries {
            if let Some(oldest_key) = hot
                .iter()
                .min_by_key(|(_, v)| v.accessed_at)
                .map(|(k, _)| k.clone())
            {
                hot.remove(&oldest_key);
            }
        }

        if self.hot_max_entries > 0 {
            let now = std::time::Instant::now();
            hot.insert(
                key.to_string(),
                InMemoryEntry {
                    response: response.to_string(),
                    token_count,
                    created_at: now,
                    accessed_at: now,
                },
            );
        }
    }

    /// Return cache statistics: (total_entries, total_hits, total_tokens_saved).
    pub async fn stats(&self) -> Result<(usize, u64, u64)> {
        let mut count_resp = self
            .db
            .query("SELECT count() FROM response_cache GROUP ALL")
            .await?;
        let count_rows: Vec<serde_json::Value> = count_resp.take(0)?;
        let count = count_rows
            .first()
            .and_then(|r| r.get("count"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);

        let mut hits_resp = self
            .db
            .query("SELECT math::sum(hit_count) AS total FROM response_cache GROUP ALL")
            .await?;
        let hits_rows: Vec<serde_json::Value> = hits_resp.take(0)?;
        let hits = hits_rows
            .first()
            .and_then(|r| r.get("total"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);

        let mut tokens_resp = self
            .db
            .query(
                "SELECT math::sum(token_count * hit_count) AS total FROM response_cache GROUP ALL",
            )
            .await?;
        let tokens_rows: Vec<serde_json::Value> = tokens_resp.take(0)?;
        let tokens_saved = tokens_rows
            .first()
            .and_then(|r| r.get("total"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        Ok((count as usize, hits as u64, tokens_saved as u64))
    }

    /// Wipe the entire cache (useful for `synapseclaw cache clear`).
    pub async fn clear(&self) -> Result<usize> {
        self.hot_cache.lock().clear();

        // Get count before delete
        let mut count_resp = self
            .db
            .query("SELECT count() FROM response_cache GROUP ALL")
            .await?;
        let count_rows: Vec<serde_json::Value> = count_resp.take(0)?;
        let count = count_rows
            .first()
            .and_then(|r| r.get("count"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0);

        self.db.query("DELETE FROM response_cache").await?;

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::engine::local::SurrealKv;
    use tempfile::TempDir;

    async fn test_db() -> (TempDir, Arc<Surreal<Db>>) {
        let tmp = TempDir::new().unwrap();
        let db = Surreal::new::<SurrealKv>(tmp.path().join("test.surreal"))
            .await
            .unwrap();
        db.use_ns("test").use_db("test").await.unwrap();
        // Apply schema
        let schema = include_str!("surrealdb_schema.surql");
        db.query(schema).await.unwrap();
        (tmp, Arc::new(db))
    }

    async fn temp_cache(ttl_minutes: u32) -> (TempDir, ResponseCache) {
        let (tmp, db) = test_db().await;
        (tmp, ResponseCache::new_with_surreal(db, ttl_minutes, 1000))
    }

    #[test]
    fn cache_key_deterministic() {
        let k1 = ResponseCache::cache_key("gpt-4", Some("sys"), "hello");
        let k2 = ResponseCache::cache_key("gpt-4", Some("sys"), "hello");
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn cache_key_varies_by_model() {
        let k1 = ResponseCache::cache_key("gpt-4", None, "hello");
        let k2 = ResponseCache::cache_key("claude-3", None, "hello");
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_varies_by_system_prompt() {
        let k1 = ResponseCache::cache_key("gpt-4", Some("You are helpful"), "hello");
        let k2 = ResponseCache::cache_key("gpt-4", Some("You are rude"), "hello");
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_varies_by_prompt() {
        let k1 = ResponseCache::cache_key("gpt-4", None, "hello");
        let k2 = ResponseCache::cache_key("gpt-4", None, "goodbye");
        assert_ne!(k1, k2);
    }

    #[tokio::test]
    async fn put_and_get() {
        let (_tmp, cache) = temp_cache(60).await;
        let key = ResponseCache::cache_key("gpt-4", None, "What is Rust?");

        cache
            .put(&key, "gpt-4", "Rust is a systems programming language.", 25)
            .await
            .unwrap();

        let result = cache.get(&key).await.unwrap();
        assert_eq!(
            result.as_deref(),
            Some("Rust is a systems programming language.")
        );
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let (_tmp, cache) = temp_cache(60).await;
        let result = cache.get("nonexistent_key").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn expired_entry_returns_none() {
        let (_tmp, cache) = temp_cache(0).await; // 0-minute TTL -> everything is instantly expired
        let key = ResponseCache::cache_key("gpt-4", None, "test");

        cache.put(&key, "gpt-4", "response", 10).await.unwrap();

        // The entry was created with created_at = now(), but TTL is 0 minutes,
        // so cutoff = now() - 0 = now(). The entry's created_at is NOT > cutoff.
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn hit_count_incremented() {
        let (_tmp, cache) = temp_cache(60).await;
        let key = ResponseCache::cache_key("gpt-4", None, "hello");

        cache.put(&key, "gpt-4", "Hi!", 5).await.unwrap();

        // 3 hits
        for _ in 0..3 {
            let _ = cache.get(&key).await.unwrap();
        }

        let (_, total_hits, _) = cache.stats().await.unwrap();
        assert_eq!(total_hits, 3);
    }

    #[tokio::test]
    async fn tokens_saved_calculated() {
        let (_tmp, cache) = temp_cache(60).await;
        let key = ResponseCache::cache_key("gpt-4", None, "explain rust");

        cache.put(&key, "gpt-4", "Rust is...", 100).await.unwrap();

        // 5 cache hits x 100 tokens = 500 tokens saved
        for _ in 0..5 {
            let _ = cache.get(&key).await.unwrap();
        }

        let (_, _, tokens_saved) = cache.stats().await.unwrap();
        assert_eq!(tokens_saved, 500);
    }

    #[tokio::test]
    async fn lru_eviction() {
        let (_tmp, db) = test_db().await;
        let cache = ResponseCache::new_with_surreal(db, 60, 3); // max 3 entries

        for i in 0..5 {
            let key = ResponseCache::cache_key("gpt-4", None, &format!("prompt {i}"));
            cache
                .put(&key, "gpt-4", &format!("response {i}"), 10)
                .await
                .unwrap();
        }

        let (count, _, _) = cache.stats().await.unwrap();
        assert!(count <= 3, "Should have at most 3 entries after eviction");
    }

    #[tokio::test]
    async fn clear_wipes_all() {
        let (_tmp, cache) = temp_cache(60).await;

        for i in 0..10 {
            let key = ResponseCache::cache_key("gpt-4", None, &format!("prompt {i}"));
            cache
                .put(&key, "gpt-4", &format!("response {i}"), 10)
                .await
                .unwrap();
        }

        let cleared = cache.clear().await.unwrap();
        assert_eq!(cleared, 10);

        let (count, _, _) = cache.stats().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn stats_empty_cache() {
        let (_tmp, cache) = temp_cache(60).await;
        let (count, hits, tokens) = cache.stats().await.unwrap();
        assert_eq!(count, 0);
        assert_eq!(hits, 0);
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn overwrite_same_key() {
        let (_tmp, cache) = temp_cache(60).await;
        let key = ResponseCache::cache_key("gpt-4", None, "question");

        cache.put(&key, "gpt-4", "answer v1", 20).await.unwrap();
        cache.put(&key, "gpt-4", "answer v2", 25).await.unwrap();

        let result = cache.get(&key).await.unwrap();
        assert_eq!(result.as_deref(), Some("answer v2"));

        let (count, _, _) = cache.stats().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn unicode_prompt_handling() {
        let (_tmp, cache) = temp_cache(60).await;
        let key = ResponseCache::cache_key("gpt-4", None, "日本語のテスト 🦀");

        cache
            .put(&key, "gpt-4", "はい、Rustは素晴らしい", 30)
            .await
            .unwrap();

        let result = cache.get(&key).await.unwrap();
        assert_eq!(result.as_deref(), Some("はい、Rustは素晴らしい"));
    }

    // -- Cache eviction under pressure tests --

    #[tokio::test]
    async fn lru_eviction_keeps_most_recent() {
        let (_tmp, db) = test_db().await;
        let cache = ResponseCache::new_with_surreal(db, 60, 3);

        // Insert 3 entries
        for i in 0..3 {
            let key = ResponseCache::cache_key("gpt-4", None, &format!("prompt {i}"));
            cache
                .put(&key, "gpt-4", &format!("response {i}"), 10)
                .await
                .unwrap();
        }

        // Access entry 0 to make it recently used
        let key0 = ResponseCache::cache_key("gpt-4", None, "prompt 0");
        let _ = cache.get(&key0).await.unwrap();

        // Insert entry 3 (triggers eviction)
        let key3 = ResponseCache::cache_key("gpt-4", None, "prompt 3");
        cache.put(&key3, "gpt-4", "response 3", 10).await.unwrap();

        let (count, _, _) = cache.stats().await.unwrap();
        assert!(count <= 3, "cache must not exceed max_entries");

        // Entry 0 was recently accessed and should survive
        let entry0 = cache.get(&key0).await.unwrap();
        assert!(
            entry0.is_some(),
            "recently accessed entry should survive LRU eviction"
        );
    }

    #[tokio::test]
    async fn cache_handles_zero_max_entries() {
        let (_tmp, db) = test_db().await;
        let cache = ResponseCache::new_with_surreal(db, 60, 0);

        let key = ResponseCache::cache_key("gpt-4", None, "test");
        // Should not panic even with max_entries=0
        cache.put(&key, "gpt-4", "response", 10).await.unwrap();

        let (count, _, _) = cache.stats().await.unwrap();
        assert_eq!(count, 0, "cache with max_entries=0 should evict everything");
    }

    #[tokio::test]
    async fn cache_concurrent_reads_no_panic() {
        let (_tmp, db) = test_db().await;
        let cache = Arc::new(ResponseCache::new_with_surreal(db, 60, 100));

        let key = ResponseCache::cache_key("gpt-4", None, "concurrent");
        cache.put(&key, "gpt-4", "response", 10).await.unwrap();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let cache = Arc::clone(&cache);
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                let _ = cache.get(&key).await.unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let (_, hits, _) = cache.stats().await.unwrap();
        assert_eq!(hits, 10, "all concurrent reads should register as hits");
    }
}
