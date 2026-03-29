//! Workload profile types — used in IPC config and security execution.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Workload profile for child agent spawning.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct WorkloadProfile {
    /// LLM model override for the child.
    pub model: Option<String>,
    /// System prompt prefix/template.
    pub prompt_template: Option<String>,
    /// Tool subset available to the child.
    pub allowed_tools: Option<Vec<String>>,
    /// Maximum output tokens for the child.
    pub max_output_tokens: Option<u32>,
}
