//! Background memory consolidation worker.
//!
//! Runs as a tokio task in the daemon, periodically:
//! 1. Importance decay on old episodes
//! 2. Garbage collection of low-importance entries
//! 3. Prompt optimization (Phase 4.4) — analyzes reflections, improves instructions

use std::sync::Arc;
use std::time::Duration;
use synapse_domain::ports::memory::UnifiedMemoryPort;

/// Configuration for the consolidation worker.
#[derive(Debug, Clone)]
pub struct ConsolidationWorkerConfig {
    /// Interval between consolidation cycles (decay + GC).
    pub interval: Duration,
    /// Importance decay threshold.
    pub gc_importance_threshold: f32,
    /// Max age in days for GC candidates.
    pub gc_max_age_days: u32,
    /// Interval between prompt optimization cycles.
    pub optimization_interval: Duration,
    /// Minimum reflections needed before optimization runs.
    pub min_reflections_for_optimization: usize,
}

impl Default for ConsolidationWorkerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3600), // 1 hour
            gc_importance_threshold: 0.05,
            gc_max_age_days: 30,
            optimization_interval: Duration::from_secs(21600), // 6 hours
            min_reflections_for_optimization: 3,
        }
    }
}

/// Spawn the consolidation worker as a background tokio task.
///
/// When `provider` is Some, prompt optimization runs every `optimization_interval`.
pub fn spawn_consolidation_worker(
    memory: Arc<dyn UnifiedMemoryPort>,
    config: ConsolidationWorkerConfig,
    agent_id: String,
    provider: Option<(Arc<dyn synapse_providers::traits::Provider>, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.interval);
        interval.tick().await; // skip first immediate tick

        let mut last_optimization = std::time::Instant::now();

        loop {
            interval.tick().await;
            tracing::debug!("Memory consolidation cycle starting");

            // Phase 1: Importance decay
            match memory.recalculate_importance(&agent_id).await {
                Ok(_) => tracing::debug!("Importance decay applied"),
                Err(e) => tracing::debug!("Importance decay failed: {e}"),
            }

            // Phase 2: Garbage collection
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

            // Phase 3: Prompt optimization (if provider available and interval elapsed)
            if let Some((ref prov, ref model)) = provider {
                if last_optimization.elapsed() >= config.optimization_interval {
                    match super::prompt_optimizer::optimize_prompt(
                        prov.as_ref(),
                        model,
                        memory.as_ref(),
                        &agent_id,
                        config.min_reflections_for_optimization,
                    )
                    .await
                    {
                        Ok(Some(opt)) => {
                            tracing::info!(
                                changes = opt.changes.len(),
                                reflections = opt.reflections_analyzed,
                                "prompt.optimization.cycle_complete"
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!("prompt.optimization.failed: {e}");
                        }
                    }
                    last_optimization = std::time::Instant::now();
                }
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
        assert_eq!(config.optimization_interval, Duration::from_secs(21600));
        assert_eq!(config.min_reflections_for_optimization, 3);
    }
}
