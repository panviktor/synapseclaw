//! Port: pipeline observer.
//!
//! Phase 4.1 Slice 10: receives pipeline lifecycle events for logging,
//! metrics, alerting. Implementations wrap the existing Observer trait.

use crate::domain::pipeline_event::PipelineEvent;
use async_trait::async_trait;

/// Port for observing pipeline lifecycle events.
///
/// The pipeline runner emits events through this port at each
/// significant lifecycle point. Implementations can log, collect
/// metrics, send alerts, etc.
#[async_trait]
pub trait PipelineObserverPort: Send + Sync {
    /// Record a pipeline event.
    async fn emit(&self, event: PipelineEvent);
}

/// No-op observer for tests and when observability is disabled.
pub struct NoopPipelineObserver;

#[async_trait]
impl PipelineObserverPort for NoopPipelineObserver {
    async fn emit(&self, _event: PipelineEvent) {}
}

/// Observer that logs events via tracing.
pub struct TracingPipelineObserver;

#[async_trait]
impl PipelineObserverPort for TracingPipelineObserver {
    async fn emit(&self, event: PipelineEvent) {
        tracing::info!(
            event_type = std::any::type_name::<PipelineEvent>(),
            summary = %event.summary(),
            "pipeline event"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct CollectingObserver {
        events: Arc<Mutex<Vec<PipelineEvent>>>,
    }

    #[async_trait]
    impl PipelineObserverPort for CollectingObserver {
        async fn emit(&self, event: PipelineEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn collecting_observer_captures_events() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer = CollectingObserver {
            events: events.clone(),
        };

        observer
            .emit(PipelineEvent::PipelineStarted {
                run_id: "r1".into(),
                pipeline_name: "test".into(),
                version: "1.0".into(),
                triggered_by: "op".into(),
                depth: 0,
            })
            .await;

        observer
            .emit(PipelineEvent::PipelineCompleted {
                run_id: "r1".into(),
                pipeline_name: "test".into(),
                duration_ms: 5000,
                step_count: 3,
            })
            .await;

        assert_eq!(events.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn noop_observer_does_not_panic() {
        let observer = NoopPipelineObserver;
        observer
            .emit(PipelineEvent::PipelineFailed {
                run_id: "r".into(),
                pipeline_name: "p".into(),
                error: "e".into(),
                last_step: "s".into(),
            })
            .await;
    }
}
