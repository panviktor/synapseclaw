//! Port: summary generation — generate text summaries via LLM.
//!
//! Abstracts the LLM call for summary generation so the conversation
//! service can orchestrate without depending on provider infrastructure.

use anyhow::Result;
use async_trait::async_trait;

/// Port for generating text summaries.
#[async_trait]
pub trait SummaryGeneratorPort: Send + Sync {
    /// Generate a summary from the given prompt.
    async fn generate_summary(&self, prompt: &str) -> Result<String>;
}
