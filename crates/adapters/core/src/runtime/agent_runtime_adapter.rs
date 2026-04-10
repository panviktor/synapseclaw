//! Adapter: wraps `run_tool_call_loop` as AgentRuntimePort.
//!
//! Since `synapse_providers::ChatMessage` is now a re-export of
//! `synapse_domain::domain::message::ChatMessage`, no conversions are needed.

use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use synapse_domain::config::schema::ReliabilityConfig;
use synapse_domain::ports::agent_runtime::{
    AgentRuntimeError, AgentRuntimeErrorKind, AgentRuntimePort, AgentTurnResult,
};
use synapse_domain::ports::model_profile_catalog::ModelProfileCatalogPort;
use synapse_domain::ports::provider::ProviderCapabilities;
use synapse_infra::approval::ApprovalManager;
use synapse_providers::{ChatMessage, Provider, ProviderCapabilityError, ProviderRuntimeOptions};
use synapse_security::scrub_credentials;

/// Wraps the existing agent loop infrastructure behind `AgentRuntimePort`.
pub struct ChannelAgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub default_provider_name: String,
    pub default_api_key: Option<String>,
    pub default_api_url: Option<String>,
    pub provider_cache: Arc<Mutex<HashMap<String, Arc<dyn Provider>>>>,
    pub reliability: ReliabilityConfig,
    pub provider_runtime_options: ProviderRuntimeOptions,
    pub model_profile_catalog: Option<Arc<dyn ModelProfileCatalogPort>>,
    pub tools_registry: Arc<Vec<Box<dyn Tool>>>,
    pub observer: Arc<dyn synapse_observability::Observer>,
    pub approval_manager: Arc<ApprovalManager>,
    pub channel_name: String,
    pub multimodal: synapse_domain::config::schema::MultimodalConfig,
    pub excluded_tools: Arc<Vec<String>>,
    pub dedup_exempt_tools: Arc<Vec<String>>,
    pub hooks: Option<Arc<crate::hooks::HookRunner>>,
    pub activated_tools: Option<Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
    pub message_timeout_secs: u64,
    pub max_tool_iterations: usize,
}

impl ChannelAgentRuntime {
    async fn get_or_create_provider(&self, provider_name: &str) -> Result<Arc<dyn Provider>> {
        if provider_name.is_empty() || provider_name == self.default_provider_name {
            return Ok(Arc::clone(&self.provider));
        }

        if let Some(existing) = self
            .provider_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(provider_name)
            .cloned()
        {
            return Ok(existing);
        }

        let provider_name_owned = provider_name.to_string();
        let api_key = if provider_name == self.default_provider_name {
            self.default_api_key.clone()
        } else {
            None
        };
        let api_url = if provider_name == self.default_provider_name {
            self.default_api_url.clone()
        } else {
            None
        };
        let reliability = self.reliability.clone();
        let runtime_options = self.provider_runtime_options.clone();

        let provider = tokio::task::spawn_blocking(move || {
            synapse_providers::create_resilient_provider_with_options(
                &provider_name_owned,
                api_key.as_deref(),
                api_url.as_deref(),
                &reliability,
                &runtime_options,
            )
        })
        .await
        .map_err(|error| {
            anyhow::anyhow!("failed to join provider initialization task: {error}")
        })??;
        let provider: Arc<dyn Provider> = Arc::from(provider);

        if let Err(err) = provider.warmup().await {
            tracing::warn!(provider = provider_name, "Provider warmup failed: {err}");
        }

        let mut cache = self
            .provider_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cached = cache
            .entry(provider_name.to_string())
            .or_insert_with(|| Arc::clone(&provider));
        Ok(Arc::clone(cached))
    }
}

#[async_trait]
impl AgentRuntimePort for ChannelAgentRuntime {
    async fn execute_turn(
        &self,
        mut history: Vec<ChatMessage>,
        provider_name: &str,
        model: &str,
        temperature: f64,
        max_iterations: usize,
        timeout_secs: u64,
        on_delta: Option<tokio::sync::mpsc::Sender<String>>,
    ) -> std::result::Result<AgentTurnResult, AgentRuntimeError> {
        // Compute timeout budget (scale by max iterations)
        let iterations = max_iterations.max(1) as u64;
        let scale = iterations.min(5);
        let budget_secs = if timeout_secs > 0 {
            timeout_secs.saturating_mul(scale)
        } else {
            0
        };
        let provider = self
            .get_or_create_provider(provider_name)
            .await
            .map_err(classify_agent_runtime_error)?;

        let fut = Box::pin(crate::agent::run_tool_call_loop(
            provider.as_ref(),
            &mut history,
            &self.tools_registry,
            self.observer.as_ref(),
            provider_name,
            model,
            temperature,
            true, // silent (channel mode)
            Some(&*self.approval_manager as &dyn synapse_domain::ports::approval::ApprovalPort),
            &self.channel_name,
            &self.multimodal,
            max_iterations,
            None,     // cancellation_token
            on_delta, // streaming deltas
            self.hooks.as_deref(),
            &self.excluded_tools,
            &self.dedup_exempt_tools,
            self.activated_tools.as_ref(),
            None, // run_ctx
        ));

        // Apply timeout if configured
        let loop_result = if budget_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(budget_secs), fut).await {
                Ok(result) => result.map_err(classify_agent_runtime_error),
                Err(_) => {
                    return Err(AgentRuntimeError::new(
                        AgentRuntimeErrorKind::Timeout,
                        format!("agent execution timed out after {budget_secs}s"),
                    ));
                }
            }
        } else {
            fut.await.map_err(classify_agent_runtime_error)
        };
        let loop_result = loop_result?;

        let response = loop_result.response;
        let tool_names = loop_result.tool_names;
        let tool_facts = loop_result.tool_facts;
        let last_tool_repair = loop_result.last_tool_repair;
        let tool_repairs = loop_result.tool_repairs;
        let tools_used = !tool_names.is_empty();
        let tool_summary = format_tool_summary(&tool_names);

        Ok(AgentTurnResult {
            response,
            history,
            tools_used,
            tool_names,
            tool_facts,
            tool_summary,
            last_tool_repair,
            tool_repairs,
        })
    }

    fn capabilities_for(&self, provider_name: &str) -> ProviderCapabilities {
        if provider_name.is_empty() || provider_name == self.default_provider_name {
            return self.provider.capabilities();
        }

        self.provider_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(provider_name)
            .map(|provider| provider.capabilities())
            .unwrap_or_default()
    }

    fn supports_vision_for_route(&self, provider_name: &str, model: &str) -> bool {
        if self.supports_vision_for(provider_name) {
            return true;
        }

        self.model_profile_catalog
            .as_ref()
            .and_then(|catalog| catalog.lookup_model_profile(provider_name, model))
            .is_some_and(|profile| {
                profile
                    .features
                    .contains(&synapse_domain::config::schema::ModelFeature::Vision)
                    || profile.features.contains(
                        &synapse_domain::config::schema::ModelFeature::MultimodalUnderstanding,
                    )
            })
    }

    fn supports_vision(&self) -> bool {
        self.provider.supports_vision()
    }
}

fn format_tool_summary(tool_names: &[String]) -> String {
    if tool_names.is_empty() {
        String::new()
    } else {
        format!("[Used tools: {}]", tool_names.join(", "))
    }
}

fn classify_agent_runtime_error(err: anyhow::Error) -> AgentRuntimeError {
    if err.downcast_ref::<ProviderCapabilityError>().is_some() {
        return AgentRuntimeError::new(
            AgentRuntimeErrorKind::CapabilityMismatch,
            scrub_credentials(&err.to_string()),
        );
    }

    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        if let Some(status) = reqwest_err.status() {
            return match status.as_u16() {
                401 | 403 => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::AuthFailure,
                    scrub_credentials(&err.to_string()),
                ),
                413 => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::ContextLimitExceeded,
                    scrub_credentials(&err.to_string()),
                ),
                _ => AgentRuntimeError::new(
                    AgentRuntimeErrorKind::RuntimeFailure,
                    scrub_credentials(&err.to_string()),
                ),
            };
        }
    }

    let detail = scrub_credentials(&err.to_string());
    let lower = detail.to_lowercase();
    let context_hints = [
        "exceeds the context window",
        "context window of this model",
        "maximum context length",
        "context length exceeded",
        "too many tokens",
        "token limit exceeded",
        "prompt is too long",
        "input is too long",
    ];
    if context_hints.iter().any(|hint| lower.contains(hint)) {
        return AgentRuntimeError::new(AgentRuntimeErrorKind::ContextLimitExceeded, detail);
    }

    AgentRuntimeError::new(AgentRuntimeErrorKind::RuntimeFailure, detail)
}
