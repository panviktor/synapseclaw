//! Run recipe — a reusable summary of a previously successful execution style.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunRecipe {
    pub agent_id: String,
    pub task_family: String,
    pub sample_request: String,
    pub summary: String,
    pub tool_pattern: Vec<String>,
    pub success_count: u32,
    pub updated_at: u64,
}
