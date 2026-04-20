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
        candidate_count = summary_route.candidates.len(),
        "Inbound session summary auxiliary lane selected"
    );

    let provider = {
        let provider_name = summary_route.selected.provider.as_str();
        let api_key = summary_route
            .selected
            .api_key_env
            .as_deref()
            .and_then(|env| std::env::var(env).ok())
            .or_else(|| summary_route.selected.api_key.clone());
        match synapse_providers::create_provider_with_options(
            provider_name,
            api_key.as_deref(),
            input.provider_runtime_options,
        ) {
            Ok(provider) => Arc::from(provider),
            Err(error) => {
                tracing::warn!(
                    %error,
                    transport = input.transport_label,
                    summary_provider = provider_name,
                    compaction_model = summary_route.selected.model.as_str(),
                    "Summary provider init failed"
                );
                return Err(error);
            }
        }
    };

    let generator =
        crate::memory_adapters::summary_generator_adapter::ProviderSummaryGenerator::new(
            provider,
            summary_route.selected.model,
            input.config.summary.temperature,
        );

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
