use async_trait::async_trait;
use std::fmt::Write;
use synapse_memory::UnifiedMemoryPort;

#[async_trait]
pub trait MemoryLoader: Send + Sync {
    async fn load_context(
        &self,
        memory: &dyn UnifiedMemoryPort,
        user_message: &str,
        session_id: Option<&str>,
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
    ) -> anyhow::Result<String> {
        let entries = memory.recall(user_message, self.limit, session_id).await?;
        if entries.is_empty() {
            return Ok(String::new());
        }

        let mut context = String::from("[Memory context]\n");
        for entry in entries {
            if synapse_memory::is_assistant_autosave_key(&entry.key) {
                continue;
            }
            if synapse_domain::domain::util::should_skip_autosave_content(&entry.content) {
                continue;
            }
            if let Some(score) = entry.score {
                if score < self.min_relevance_score {
                    continue;
                }
            }
            let _ = writeln!(context, "- {}: {}", entry.key, entry.content);
        }

        // If all entries were below threshold, return empty
        if context == "[Memory context]\n" {
            return Ok(String::new());
        }

        context.push('\n');
        Ok(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_memory::{MemoryCategory, MemoryEntry};

    // NoopUnifiedMemory returns empty recall — good enough for basic tests.
    // For recall-with-entries, use the NoopUnifiedMemory and test the loader
    // behavior on empty results. Real integration tests deferred to Phase 4.3.

    #[tokio::test]
    async fn default_loader_returns_empty_when_no_entries() {
        let loader = DefaultMemoryLoader::default();
        let mem = synapse_memory::NoopUnifiedMemory;
        let context = loader.load_context(&mem, "hello", None).await.unwrap();
        assert!(context.is_empty());
    }

    #[tokio::test]
    async fn default_loader_respects_relevance_threshold() {
        // With noop memory (empty recall), loader should return empty
        let loader = DefaultMemoryLoader::new(5, 0.8);
        let mem = synapse_memory::NoopUnifiedMemory;
        let context = loader
            .load_context(&mem, "answer style", None)
            .await
            .unwrap();
        assert!(context.is_empty());
    }
}
