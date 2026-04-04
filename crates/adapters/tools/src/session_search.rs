//! Session search tool — search past conversation sessions.
//!
//! Enables "did we talk about this last week?" without relying on
//! long-term episodic memory alone. Searches session labels, summaries,
//! and recent messages via ConversationStorePort.

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::ports::conversation_store::ConversationStorePort;
use synapse_domain::ports::tool::{Tool, ToolResult};

pub struct SessionSearchTool {
    store: Arc<dyn ConversationStorePort>,
}

impl SessionSearchTool {
    pub fn new(store: Arc<dyn ConversationStorePort>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search past conversation sessions by keyword. Returns matching sessions \
         with labels, summaries, and timestamps. Use this when the user references \
         past discussions, previous decisions, or 'what we talked about before'."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default 5, max 10)"
                },
                "kind": {
                    "type": "string",
                    "enum": ["web", "channel", "ipc"],
                    "description": "Filter by session kind (optional)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(10) as usize;
        let kind_filter = args.get("kind").and_then(|v| v.as_str());

        if query.trim().is_empty() {
            return Ok(ToolResult { output: "Query cannot be empty".into(), success: false, error: None });
        }

        let query_lower = query.to_lowercase();
        let keywords: Vec<&str> = query_lower.split_whitespace().collect();

        // Get all sessions
        let sessions = self.store.list_sessions(None).await;

        // Score and filter
        let mut scored: Vec<(f64, &synapse_domain::domain::conversation::ConversationSession)> = sessions
            .iter()
            .filter(|s| {
                if let Some(kind) = kind_filter {
                    let session_kind = format!("{:?}", s.kind).to_lowercase();
                    session_kind.contains(kind)
                } else {
                    true
                }
            })
            .filter_map(|s| {
                let mut score = 0.0;
                let label = s.label.as_deref().unwrap_or("").to_lowercase();
                let summary = s.summary.as_deref().unwrap_or("").to_lowercase();
                let key = s.key.to_lowercase();

                for kw in &keywords {
                    if label.contains(kw) {
                        score += 3.0;
                    }
                    if summary.contains(kw) {
                        score += 2.0;
                    }
                    if key.contains(kw) {
                        score += 1.0;
                    }
                }

                if score > 0.0 {
                    Some((score, s))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        if scored.is_empty() {
            return Ok(ToolResult { output: format!("No sessions found matching '{query}'"), success: true, error: None });
        }

        let mut output = format!("Found {} session(s) matching '{query}':\n\n", scored.len());
        for (i, (_score, s)) in scored.iter().enumerate() {
            let label = s.label.as_deref().unwrap_or("(untitled)");
            let summary = s
                .summary
                .as_deref()
                .map(|s| {
                    if s.chars().count() > 150 {
                        let t: String = s.chars().take(150).collect();
                        format!("{t}...")
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_else(|| "(no summary)".into());
            let kind = format!("{:?}", s.kind).to_lowercase();
            let msgs = s.message_count;

            output.push_str(&format!(
                "{}. **{}** ({})\n   {} messages | {}\n\n",
                i + 1,
                label,
                kind,
                msgs,
                summary
            ));
        }

        Ok(ToolResult {
            output,
            success: true,
            error: None,
        })
    }
}
