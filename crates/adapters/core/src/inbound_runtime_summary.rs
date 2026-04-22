//! Shared inbound summary orchestration for web and channel transports.
//!
//! Domain owns the summary policy and prompt. Adapter-core owns concrete
//! provider selection and `SummaryGeneratorPort` wiring.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use synapse_domain::application::services::auxiliary_model_resolution::{
    resolve_auxiliary_model, AuxiliaryLane, AuxiliaryModelResolutionError,
};
use synapse_domain::application::services::conversation_service;
use synapse_domain::config::schema::Config;
use synapse_domain::ports::conversation_store::ConversationStorePort;
use synapse_providers::{Provider, ProviderRuntimeOptions};

static INFLIGHT_SUMMARIES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct InflightSummaryGuard(String);

impl InflightSummaryGuard {
    fn acquire(session_key: &str) -> Option<Self> {
        let mut inflight = INFLIGHT_SUMMARIES.lock().unwrap_or_else(|e| e.into_inner());
        inflight
            .insert(session_key.to_string())
            .then(|| Self(session_key.to_string()))
    }
}

impl Drop for InflightSummaryGuard {
    fn drop(&mut self) {
        if let Ok(mut inflight) = INFLIGHT_SUMMARIES.lock() {
            inflight.remove(&self.0);
        }
    }
}

pub(crate) struct InboundRuntimeSummaryInput<'a> {
    pub store: &'a dyn ConversationStorePort,
    pub current_provider: Arc<dyn Provider>,
    pub config: &'a Config,
    pub current_model: &'a str,
    pub provider_runtime_options: &'a ProviderRuntimeOptions,
    pub session_key: &'a str,
    pub message_count: usize,
    pub last_summary_count: usize,
    pub previous_summary: Option<&'a str>,
    pub interval: usize,
    pub transport_label: &'static str,
}

pub(crate) async fn summarize_session_if_needed(
    input: InboundRuntimeSummaryInput<'_>,
) -> anyhow::Result<Option<String>> {
    if !conversation_service::needs_summary(
        input.message_count,
        input.last_summary_count,
        input.interval,
    ) {
        return Ok(None);
    }

    let Some(_guard) = InflightSummaryGuard::acquire(input.session_key) else {
        return Ok(None);
    };

    let summary_route = match resolve_auxiliary_model(input.config, AuxiliaryLane::Compaction, None)
    {
        Ok(route) => route,
        Err(AuxiliaryModelResolutionError::LaneNotConfigured { lane }) => {
            tracing::warn!(
                session_key = input.session_key,
                transport = input.transport_label,
                auxiliary_lane = lane.as_str(),
                "Inbound session summary skipped: auxiliary lane is not configured"
            );
            return Ok(None);
        }
        Err(error) => {
            tracing::warn!(
                session_key = input.session_key,
                transport = input.transport_label,
                %error,
                "Inbound session summary skipped: no supported auxiliary candidate"
            );
            return Ok(None);
        }
    };
    tracing::info!(
        session_key = input.session_key,
        transport = input.transport_label,
        auxiliary_lane = summary_route.lane.as_str(),
        summary_provider = summary_route.selected.provider.as_str(),
        compaction_model = summary_route.selected.model.as_str(),
        selected_candidate_index = summary_route.selected_index,
        supported_candidate_count = summary_route.supported_candidates.len(),
        candidate_count = summary_route.candidates.len(),
        "Inbound session summary auxiliary lane selected"
    );

    let generator =
        crate::memory_adapters::summary_generator_adapter::FailoverSummaryGenerator::from_auxiliary_resolution(
            &summary_route,
            input.provider_runtime_options,
            input.config.summary.temperature,
        )?;

    conversation_service::generate_session_summary(
        input.store,
        &generator,
        input.session_key,
        input.message_count,
        input.last_summary_count,
        input.previous_summary,
        input.interval,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use synapse_domain::domain::conversation::{ConversationEvent, ConversationSession};
    use synapse_providers::Provider;

    struct CountingStore {
        event_reads: AtomicUsize,
    }

    impl CountingStore {
        fn new() -> Self {
            Self {
                event_reads: AtomicUsize::new(0),
            }
        }

        fn event_reads(&self) -> usize {
            self.event_reads.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ConversationStorePort for CountingStore {
        async fn get_session(&self, _key: &str) -> Option<ConversationSession> {
            None
        }

        async fn list_sessions(&self, _prefix: Option<&str>) -> Vec<ConversationSession> {
            Vec::new()
        }

        async fn upsert_session(&self, _session: &ConversationSession) -> anyhow::Result<()> {
            Ok(())
        }

        async fn delete_session(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        async fn touch_session(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn append_event(
            &self,
            _session_key: &str,
            _event: &ConversationEvent,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn get_events(&self, _session_key: &str, _limit: usize) -> Vec<ConversationEvent> {
            self.event_reads.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        }

        async fn clear_events(&self, _session_key: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn update_label(&self, _key: &str, _label: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn update_goal(&self, _key: &str, _goal: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn increment_message_count(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }

        async fn add_token_usage(
            &self,
            _key: &str,
            _input: i64,
            _output: i64,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn get_summary(&self, _key: &str) -> Option<String> {
            None
        }

        async fn set_summary(&self, _key: &str, _summary: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct PanicProvider;

    #[async_trait]
    impl Provider for PanicProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            panic!("summary tests should not call provider");
        }
    }

    fn input<'a>(
        store: &'a CountingStore,
        config: &'a Config,
        provider_runtime_options: &'a ProviderRuntimeOptions,
        message_count: usize,
        interval: usize,
    ) -> InboundRuntimeSummaryInput<'a> {
        InboundRuntimeSummaryInput {
            store,
            current_provider: Arc::new(PanicProvider),
            config,
            current_model: "primary-model",
            provider_runtime_options,
            session_key: "web:test",
            message_count,
            last_summary_count: 0,
            previous_summary: None,
            interval,
            transport_label: "web",
        }
    }

    #[tokio::test]
    async fn summary_not_due_does_not_require_compaction_lane() {
        let store = CountingStore::new();
        let config = Config::default();
        let provider_runtime_options = ProviderRuntimeOptions::default();

        let result =
            summarize_session_if_needed(input(&store, &config, &provider_runtime_options, 2, 10))
                .await
                .expect("summary check should not fail");

        assert!(result.is_none());
        assert_eq!(store.event_reads(), 0);
    }

    #[tokio::test]
    async fn due_summary_without_compaction_lane_skips_without_default_fallback() {
        let store = CountingStore::new();
        let mut config = Config::default();
        config.default_provider = Some("primary-provider".into());
        config.default_model = Some("primary-model".into());
        let provider_runtime_options = ProviderRuntimeOptions::default();

        let result =
            summarize_session_if_needed(input(&store, &config, &provider_runtime_options, 10, 10))
                .await
                .expect("missing lane should degrade to skipped summary");

        assert!(result.is_none());
        assert_eq!(store.event_reads(), 0);
    }
}
