//! Adapter: wraps provider for summary generation as SummaryGeneratorPort.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::application::services::auxiliary_model_resolution::AuxiliaryModelResolution;
use synapse_domain::ports::summary::SummaryGeneratorPort;
use synapse_providers::{
    reliable::{classify_provider_error, ProviderErrorClassification},
    Provider, ProviderRuntimeOptions,
};

/// Wraps a Provider for summary generation.
pub struct ProviderSummaryGenerator {
    provider: Arc<dyn Provider>,
    model: String,
    temperature: f64,
}

impl ProviderSummaryGenerator {
    pub fn new(provider: Arc<dyn Provider>, model: String, temperature: f64) -> Self {
        Self {
            provider,
            model,
            temperature,
        }
    }
}

#[async_trait]
impl SummaryGeneratorPort for ProviderSummaryGenerator {
    async fn generate_summary(&self, prompt: &str) -> Result<String> {
        self.provider
            .chat_with_system(None, prompt, &self.model, self.temperature)
            .await
    }
}

struct SummaryCandidate {
    candidate_index: usize,
    provider_name: String,
    provider: Arc<dyn Provider>,
    model: String,
}

pub struct FailoverSummaryGenerator {
    lane: &'static str,
    candidates: Vec<SummaryCandidate>,
    temperature: f64,
}

impl FailoverSummaryGenerator {
    pub fn from_auxiliary_resolution(
        resolution: &AuxiliaryModelResolution,
        provider_runtime_options: &ProviderRuntimeOptions,
        temperature: f64,
    ) -> Result<Self> {
        let mut candidates = Vec::new();
        let mut init_failures = Vec::new();

        for supported in &resolution.supported_candidates {
            let candidate = &supported.candidate;
            let provider_name = candidate.provider.as_str();
            let api_key = candidate
                .api_key_env
                .as_deref()
                .and_then(|env| std::env::var(env).ok())
                .or_else(|| candidate.api_key.clone());

            match synapse_providers::create_provider_with_options(
                provider_name,
                api_key.as_deref(),
                provider_runtime_options,
            ) {
                Ok(provider) => candidates.push(SummaryCandidate {
                    candidate_index: supported.index,
                    provider_name: candidate.provider.clone(),
                    provider: Arc::from(provider),
                    model: candidate.model.clone(),
                }),
                Err(error) => {
                    let class = classify_provider_error(&error);
                    init_failures.push(format!(
                        "candidate={} provider={} model={} kind={} error={}",
                        supported.index,
                        candidate.provider,
                        candidate.model,
                        class.kind.as_str(),
                        class.detail
                    ));
                    tracing::warn!(
                        %error,
                        failure_kind = class.kind.as_str(),
                        auxiliary_lane = resolution.lane.as_str(),
                        candidate_index = supported.index,
                        provider = candidate.provider.as_str(),
                        model = candidate.model.as_str(),
                        "Summary provider candidate initialization failed"
                    );
                }
            }
        }

        if candidates.is_empty() {
            anyhow::bail!(
                "No summary provider candidates initialized for auxiliary lane '{}'. Init failures: {}",
                resolution.lane.as_str(),
                init_failures.join(" | ")
            );
        }

        Ok(Self {
            lane: resolution.lane.as_str(),
            candidates,
            temperature,
        })
    }

    #[cfg(test)]
    fn from_test_candidates(
        lane: &'static str,
        candidates: Vec<(usize, &str, &str, Arc<dyn Provider>)>,
    ) -> Self {
        Self {
            lane,
            candidates: candidates
                .into_iter()
                .map(
                    |(candidate_index, provider_name, model, provider)| SummaryCandidate {
                        candidate_index,
                        provider_name: provider_name.to_string(),
                        provider,
                        model: model.to_string(),
                    },
                )
                .collect(),
            temperature: 0.0,
        }
    }
}

#[async_trait]
impl SummaryGeneratorPort for FailoverSummaryGenerator {
    async fn generate_summary(&self, prompt: &str) -> Result<String> {
        let mut failures = Vec::new();

        for (position, candidate) in self.candidates.iter().enumerate() {
            match candidate
                .provider
                .chat_with_system(None, prompt, &candidate.model, self.temperature)
                .await
            {
                Ok(summary) => {
                    if position > 0 {
                        tracing::info!(
                            auxiliary_lane = self.lane,
                            candidate_index = candidate.candidate_index,
                            provider = candidate.provider_name.as_str(),
                            model = candidate.model.as_str(),
                            failed_candidates = position,
                            "Auxiliary summary candidate recovered via failover"
                        );
                    }
                    return Ok(summary);
                }
                Err(error) => {
                    let class = classify_provider_error(&error);
                    push_summary_failure(&mut failures, candidate, &class);
                    tracing::warn!(
                        %error,
                        failure_kind = class.kind.as_str(),
                        failover_candidate = class.failover_candidate,
                        auxiliary_lane = self.lane,
                        candidate_index = candidate.candidate_index,
                        provider = candidate.provider_name.as_str(),
                        model = candidate.model.as_str(),
                        "Auxiliary summary provider candidate failed"
                    );

                    let has_next = position + 1 < self.candidates.len();
                    if class.failover_candidate && has_next {
                        continue;
                    }

                    return Err(error).with_context(|| {
                        format!(
                            "Auxiliary summary lane '{}' failed. Attempts: {}",
                            self.lane,
                            failures.join(" | ")
                        )
                    });
                }
            }
        }

        anyhow::bail!(
            "Auxiliary summary lane '{}' exhausted all candidates. Attempts: {}",
            self.lane,
            failures.join(" | ")
        )
    }
}

fn push_summary_failure(
    failures: &mut Vec<String>,
    candidate: &SummaryCandidate,
    class: &ProviderErrorClassification,
) {
    failures.push(format!(
        "candidate={} provider={} model={} kind={} error={}",
        candidate.candidate_index,
        candidate.provider_name,
        candidate.model,
        class.kind.as_str(),
        class.detail
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use synapse_providers::ChatMessage;

    struct TestProvider {
        response: std::result::Result<String, &'static str>,
    }

    #[async_trait]
    impl Provider for TestProvider {
        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            _model: &str,
            _temperature: f64,
        ) -> Result<String> {
            match &self.response {
                Ok(response) => Ok(response.clone()),
                Err(error) => anyhow::bail!("{error}"),
            }
        }

        async fn chat_with_history(
            &self,
            _messages: &[ChatMessage],
            _model: &str,
            _temperature: f64,
        ) -> Result<String> {
            self.chat_with_system(None, "", "", 0.0).await
        }
    }

    #[tokio::test]
    async fn summary_generator_fails_over_after_quota_error() {
        let generator = FailoverSummaryGenerator::from_test_candidates(
            "compaction",
            vec![
                (
                    0,
                    "provider-a",
                    "model-a",
                    Arc::new(TestProvider {
                        response: Err("API error 429: insufficient quota"),
                    }),
                ),
                (
                    1,
                    "provider-b",
                    "model-b",
                    Arc::new(TestProvider {
                        response: Ok("summary from b".into()),
                    }),
                ),
            ],
        );

        let summary = generator.generate_summary("summarize").await.unwrap();

        assert_eq!(summary, "summary from b");
    }

    #[tokio::test]
    async fn summary_generator_does_not_fail_over_context_window_error() {
        let generator = FailoverSummaryGenerator::from_test_candidates(
            "compaction",
            vec![
                (
                    0,
                    "provider-a",
                    "model-a",
                    Arc::new(TestProvider {
                        response: Err("input exceeds the context window"),
                    }),
                ),
                (
                    1,
                    "provider-b",
                    "model-b",
                    Arc::new(TestProvider {
                        response: Ok("summary from b".into()),
                    }),
                ),
            ],
        );

        let error = generator.generate_summary("summarize").await.unwrap_err();

        assert!(error.to_string().contains("context window"));
    }
}
