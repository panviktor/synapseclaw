//! Port for loading scoped project instructions on demand.
//!
//! Domain services decide when scoped instructions are relevant.
//! Adapters discover, cache, and read the actual workspace files.

use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedInstructionRequest {
    pub session_id: Option<String>,
    pub path_hints: Vec<String>,
    pub max_files: usize,
    pub max_total_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedInstructionSnippet {
    pub scope_root: String,
    pub source_file: String,
    pub content: String,
    pub cache_hit: bool,
}

#[async_trait]
pub trait ScopedInstructionContextPort: Send + Sync {
    async fn load_scoped_instructions(
        &self,
        request: ScopedInstructionRequest,
    ) -> Result<Vec<ScopedInstructionSnippet>>;
}
