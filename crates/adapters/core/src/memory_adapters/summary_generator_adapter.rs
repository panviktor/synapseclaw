//! Adapter: wraps provider for summary generation as SummaryGeneratorPort.

use synapse_providers::Provider;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use synapse_domain::ports::summary::SummaryGeneratorPort;

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
