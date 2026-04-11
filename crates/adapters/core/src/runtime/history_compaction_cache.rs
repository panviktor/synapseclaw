//! Shared runtime implementation for history compaction cache.
//!
//! This owns the live cache map. Web `Agent` sessions and channel runtime
//! inspection share it through the domain port instead of reaching into each
//! other's concrete runtime state.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use synapse_domain::config::schema::ContextCompressionConfig;
use synapse_domain::ports::history_compaction_cache::HistoryCompactionCachePort;
use synapse_domain::ports::route_selection::ContextCacheStats;

const HISTORY_COMPACTION_CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct HistoryCompactionCacheState {
    version: u32,
    entries: Vec<HistoryCompactionCacheEntry>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct HistoryCompactionCacheEntry {
    key: String,
    summary: String,
    created_at_unix: u64,
    last_used_at_unix: u64,
    hits: u64,
}

#[derive(Debug, Default)]
struct HistoryCompactionCacheInner {
    entries: HashMap<String, HistoryCompactionCacheEntry>,
    loaded: bool,
}

#[derive(Debug)]
pub struct FileHistoryCompactionCache {
    path: PathBuf,
    inner: Mutex<HistoryCompactionCacheInner>,
}

impl FileHistoryCompactionCache {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            inner: Mutex::new(HistoryCompactionCacheInner::default()),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn snapshot_entries(
        &self,
        compression: &ContextCompressionConfig,
    ) -> Vec<HistoryCompactionCacheEntry> {
        let mut entries = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .values()
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at_unix));
        entries.truncate(compression.cache_max_entries.max(1));
        entries
    }

    async fn persist_snapshot(&self, compression: &ContextCompressionConfig) -> anyhow::Result<()> {
        let entries = self.snapshot_entries(compression);
        if entries.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.path().parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let state = HistoryCompactionCacheState {
            version: HISTORY_COMPACTION_CACHE_VERSION,
            entries,
        };
        let json = serde_json::to_vec_pretty(&state)?;
        tokio::fs::write(self.path(), json).await?;
        Ok(())
    }
}

#[async_trait]
impl HistoryCompactionCachePort for FileHistoryCompactionCache {
    async fn load(&self, compression: &ContextCompressionConfig) -> anyhow::Result<()> {
        {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if inner.loaded {
                return Ok(());
            }
            inner.loaded = true;
        }

        let raw = match tokio::fs::read_to_string(self.path()).await {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                tracing::debug!(
                    %error,
                    path = %self.path().display(),
                    "Failed to read history compaction cache"
                );
                return Ok(());
            }
        };

        let state = match serde_json::from_str::<HistoryCompactionCacheState>(&raw) {
            Ok(state) if state.version == HISTORY_COMPACTION_CACHE_VERSION => state,
            Ok(_) => return Ok(()),
            Err(error) => {
                tracing::debug!(
                    %error,
                    path = %self.path().display(),
                    "Failed to parse history compaction cache"
                );
                return Ok(());
            }
        };

        let now = now_unix_secs();
        let ttl_secs = compression.cache_ttl_secs;
        let max_entries = compression.cache_max_entries.max(1);
        let mut entries = state
            .entries
            .into_iter()
            .filter(|entry| !entry.key.trim().is_empty() && !entry.summary.trim().is_empty())
            .filter(|entry| {
                ttl_secs == 0 || now.saturating_sub(entry.last_used_at_unix) <= ttl_secs
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_at_unix));
        entries.truncate(max_entries);

        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.entries = entries
            .into_iter()
            .map(|entry| (entry.key.clone(), entry))
            .collect();
        Ok(())
    }

    async fn get_summary(
        &self,
        compression: &ContextCompressionConfig,
        cache_key: &str,
    ) -> anyhow::Result<Option<String>> {
        self.load(compression).await?;
        let summary = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            let Some(entry) = inner.entries.get_mut(cache_key) else {
                return Ok(None);
            };
            entry.last_used_at_unix = now_unix_secs();
            entry.hits = entry.hits.saturating_add(1);
            entry.summary.clone()
        };
        self.persist_snapshot(compression).await?;
        Ok(Some(summary))
    }

    async fn remember_summary(
        &self,
        compression: &ContextCompressionConfig,
        cache_key: String,
        summary: String,
    ) -> anyhow::Result<()> {
        self.load(compression).await?;
        let now = now_unix_secs();
        {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(entry) = inner.entries.get_mut(&cache_key) {
                entry.summary = summary;
                entry.last_used_at_unix = now;
                entry.hits = entry.hits.saturating_add(1);
            } else {
                if inner.entries.len() >= compression.cache_max_entries.max(1) {
                    let evicted_key = inner
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_used_at_unix)
                        .map(|(key, _)| key.clone());
                    if let Some(evicted_key) = evicted_key {
                        inner.entries.remove(&evicted_key);
                    }
                }
                inner.entries.insert(
                    cache_key.clone(),
                    HistoryCompactionCacheEntry {
                        key: cache_key,
                        summary,
                        created_at_unix: now,
                        last_used_at_unix: now,
                        hits: 0,
                    },
                );
            }
        }
        self.persist_snapshot(compression).await
    }

    fn stats(&self, compression: &ContextCompressionConfig) -> ContextCacheStats {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let hits = inner.entries.values().map(|entry| entry.hits).sum();
        ContextCacheStats::from_compression_config(
            compression,
            inner.entries.len(),
            hits,
            inner.loaded,
        )
    }
}

pub fn shared_history_compaction_cache(
    workspace_dir: impl AsRef<Path>,
    agent_id: &str,
) -> Arc<dyn HistoryCompactionCachePort> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<FileHistoryCompactionCache>>>> =
        OnceLock::new();

    let path = history_compaction_cache_path(workspace_dir.as_ref(), agent_id);
    let registry = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(existing) = registry.get(&path) {
        let existing: Arc<dyn HistoryCompactionCachePort> = existing.clone();
        return existing;
    }

    let cache = Arc::new(FileHistoryCompactionCache::new(path.clone()));
    registry.insert(path, cache.clone());
    cache
}

fn history_compaction_cache_path(workspace_dir: &Path, agent_id: &str) -> PathBuf {
    let cache_id = history_compaction_agent_cache_id(agent_id);
    workspace_dir
        .join("state")
        .join("history_compaction_cache")
        .join(format!("{cache_id}.json"))
}

fn history_compaction_agent_cache_id(agent_id: &str) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(agent_id.as_bytes()))
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_cache_reuses_live_stats_for_workspace_agent() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let compression = ContextCompressionConfig {
            cache_max_entries: 4,
            ..Default::default()
        };
        let left = shared_history_compaction_cache(tmp.path(), "agent");
        let right = shared_history_compaction_cache(tmp.path(), "agent");

        left.remember_summary(&compression, "k".to_string(), "summary".to_string())
            .await
            .expect("remember");
        let summary = right
            .get_summary(&compression, "k")
            .await
            .expect("get")
            .expect("summary");

        assert_eq!(summary, "summary");
        let stats = right.stats(&compression);
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.hits, 1);
        assert!(stats.loaded);
    }
}
