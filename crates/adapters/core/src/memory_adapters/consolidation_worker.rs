//! Background memory consolidation worker.
//!
//! Phase 4.3 Slice 5: runs as a tokio task in the daemon, periodically
//! processing unprocessed episodes and maintaining memory health.

use std::sync::Arc;
use std::time::Duration;
use synapse_domain::ports::memory::UnifiedMemoryPort;

/// Configuration for the consolidation worker.
#[derive(Debug, Clone)]
pub struct ConsolidationWorkerConfig {
    /// Interval between consolidation cycles.
    pub interval: Duration,
    /// Importance decay threshold (entries below this after decay may be GC'd).
    pub gc_importance_threshold: f32,
    /// Max age in days for GC candidates.
    pub gc_max_age_days: u32,
}

impl Default for ConsolidationWorkerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3600), // 1 hour
            gc_importance_threshold: 0.05,
            gc_max_age_days: 30,
        }
    }
}

/// Spawn the consolidation worker as a background tokio task.
///
/// The worker runs indefinitely, performing periodic maintenance:
/// 1. Importance decay on old episodes
/// 2. Garbage collection of low-importance old entries
///
/// Entity extraction from unprocessed episodes is handled by the
/// consolidation pipeline (consolidation.rs + entity_extractor.rs)
/// which runs fire-and-forget after each conversation turn.
pub fn spawn_consolidation_worker(
    memory: Arc<dyn UnifiedMemoryPort>,
    config: ConsolidationWorkerConfig,
    agent_id: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.interval);
        // Skip the first tick (fires immediately)
        interval.tick().await;

        loop {
            interval.tick().await;
            tracing::debug!("Memory consolidation cycle starting");

            // 1. Importance decay
            match memory.recalculate_importance(&agent_id).await {
                Ok(_) => tracing::debug!("Importance decay applied"),
                Err(e) => tracing::debug!("Importance decay failed: {e}"),
            }

            // 2. Garbage collection
            match memory
                .gc_low_importance(config.gc_importance_threshold, config.gc_max_age_days)
                .await
            {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!("Memory GC: {count} entries removed");
                    }
                }
                Err(e) => tracing::debug!("Memory GC failed: {e}"),
            }

            tracing::debug!("Memory consolidation cycle complete");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = ConsolidationWorkerConfig::default();
        assert_eq!(config.interval, Duration::from_secs(3600));
        assert!((config.gc_importance_threshold - 0.05).abs() < 0.001);
        assert_eq!(config.gc_max_age_days, 30);
    }
}
