use async_trait::async_trait;
use std::fmt::Write;
use synapse_domain::domain::memory::MemoryQuery;
use synapse_memory::UnifiedMemoryPort;

#[async_trait]
pub trait MemoryLoader: Send + Sync {
    async fn load_context(
        &self,
        memory: &dyn UnifiedMemoryPort,
        user_message: &str,
        session_id: Option<&str>,
        agent_id: &str,
    ) -> anyhow::Result<String>;

    /// Load core memory blocks for system prompt injection.
    async fn load_core_blocks(
        &self,
        memory: &dyn UnifiedMemoryPort,
        agent_id: &str,
    ) -> anyhow::Result<String>;
}

pub struct DefaultMemoryLoader {
    limit: usize,
    min_relevance_score: f64,
}

impl Default for DefaultMemoryLoader {
    fn default() -> Self {
        Self {
            limit: 5,
            min_relevance_score: 0.4,
        }
    }
}

impl DefaultMemoryLoader {
    pub fn new(limit: usize, min_relevance_score: f64) -> Self {
        Self {
            limit: limit.max(1),
            min_relevance_score,
        }
    }
}

#[async_trait]
impl MemoryLoader for DefaultMemoryLoader {
    async fn load_context(
        &self,
        memory: &dyn UnifiedMemoryPort,
        user_message: &str,
        session_id: Option<&str>,
        agent_id: &str,
    ) -> anyhow::Result<String> {
        let entries = memory.recall(user_message, self.limit, session_id).await?;
        let mut context = String::new();
        let mut has_recall = false;

        if !entries.is_empty() {
            context.push_str("[Memory context]\n");
            for entry in &entries {
                if synapse_memory::is_assistant_autosave_key(&entry.key) {
                    continue;
                }
                if synapse_domain::domain::util::should_skip_autosave_content(&entry.content) {
                    continue;
                }
                // Skip entries containing tool_result blocks — they can leak
                // stale tool output from previous sessions.
                if entry.content.contains("<tool_result") {
                    continue;
                }
                if let Some(score) = entry.score {
                    if score < self.min_relevance_score {
                        continue;
                    }
                }
                let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
                has_recall = true;
            }

            // If all entries were filtered, clear the header
            if !has_recall {
                context.clear();
            } else {
                context.push('\n');
            }
        }

        // ── Skills + entities (only if recall found relevant memories) ──
        if has_recall {
            let query = MemoryQuery {
                text: user_message.to_string(),
                embedding: None,
                agent_id: agent_id.to_string(),
                include_shared: false,
                time_range: None,
                limit: 3,
            };

            if let Ok(skills) = memory.find_skills(&query).await {
                for skill in &skills {
                    if !skill.content.trim().is_empty() {
                        let _ = writeln!(context, "<skill name=\"{}\">", skill.name);
                        let _ = writeln!(context, "{}", skill.content.trim());
                        let _ = writeln!(context, "</skill>");
                    }
                }
            }

            if let Ok(entities) = memory.search_entities(&query).await {
                for entity in &entities {
                    if let Some(ref summary) = entity.summary {
                        let _ = writeln!(
                            context,
                            "<entity name=\"{}\" type=\"{}\">",
                            entity.name, entity.entity_type
                        );
                        let _ = writeln!(context, "{}", summary);
                        let _ = writeln!(context, "</entity>");
                    }
                }
            }
        }

        Ok(context)
    }

    /// Load core memory blocks (MemGPT pattern) for system prompt injection.
    ///
    /// Returns XML-tagged blocks, e.g.:
    /// ```text
    /// <persona>
    /// I am a helpful assistant...
    /// </persona>
    /// <user_knowledge>
    /// The user prefers Rust...
    /// </user_knowledge>
    /// ```
    async fn load_core_blocks(
        &self,
        memory: &dyn UnifiedMemoryPort,
        agent_id: &str,
    ) -> anyhow::Result<String> {
        let blocks = memory.get_core_blocks(&agent_id.to_string()).await;

        let blocks = match blocks {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("Failed to load core memory blocks: {e}");
                return Ok(String::new());
            }
        };

        if blocks.is_empty() {
            return Ok(String::new());
        }

        let mut output = String::new();
        for block in &blocks {
            if block.content.trim().is_empty() {
                continue;
            }
            let _ = writeln!(output, "<{}>", block.label);
            let _ = writeln!(output, "{}", block.content.trim());
            let _ = writeln!(output, "</{}>", block.label);
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_loader_returns_empty_when_no_entries() {
        let loader = DefaultMemoryLoader::default();
        let mem = synapse_memory::NoopUnifiedMemory;
        let context = loader
            .load_context(&mem, "hello", None, "test")
            .await
            .unwrap();
        assert!(context.is_empty());
    }

    #[tokio::test]
    async fn default_loader_core_blocks_empty_for_noop() {
        let loader = DefaultMemoryLoader::default();
        let mem = synapse_memory::NoopUnifiedMemory;
        let blocks = loader.load_core_blocks(&mem, "test-agent").await.unwrap();
        assert!(blocks.is_empty());
    }

    #[tokio::test]
    async fn default_loader_respects_relevance_threshold() {
        let loader = DefaultMemoryLoader::new(5, 0.8);
        let mem = synapse_memory::NoopUnifiedMemory;
        let context = loader
            .load_context(&mem, "answer style", None, "test")
            .await
            .unwrap();
        assert!(context.is_empty());
    }
}
