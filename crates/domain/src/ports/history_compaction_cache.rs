//! Port: shared history compaction cache.
//!
//! The cache is scoped by the adapter implementation, usually to one
//! workspace/agent pair, so web and channel runtimes can share live cache
//! telemetry without depending on each other's concrete runtime structs.

use crate::config::schema::ContextCompressionConfig;
use crate::ports::route_selection::ContextCacheStats;
use async_trait::async_trait;

#[async_trait]
pub trait HistoryCompactionCachePort: Send + Sync {
    /// Ensure persistent cache entries are loaded for the effective policy.
    async fn load(&self, compression: &ContextCompressionConfig) -> anyhow::Result<()>;

    /// Return a summary and record a cache hit when present.
    async fn get_summary(
        &self,
        compression: &ContextCompressionConfig,
        cache_key: &str,
    ) -> anyhow::Result<Option<String>>;

    /// Store or replace a summary, applying the effective policy's eviction cap.
    async fn remember_summary(
        &self,
        compression: &ContextCompressionConfig,
        cache_key: String,
        summary: String,
    ) -> anyhow::Result<()>;

    /// Return the current in-memory cache stats for the effective policy.
    fn stats(&self, compression: &ContextCompressionConfig) -> ContextCacheStats;
}
