//! TOML-based pipeline definition loader.
//!
//! Reads `*.toml` files from a pipeline directory, parses them into
//! `PipelineDefinition`, validates internal consistency, and caches
//! definitions in memory.  Implements `PipelineStorePort`.

use async_trait::async_trait;
use fork_core::domain::pipeline::{PipelineDefinition, PipelineToml};
use fork_core::ports::pipeline_store::{PipelineStorePort, ReloadEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Adapter: loads pipeline definitions from TOML files in a directory.
///
/// Thread-safe (interior `RwLock`).  Call `reload()` to re-scan the
/// directory, or use the hot-reload watcher (Slice 8) to trigger
/// reloads on file changes.
pub struct TomlPipelineLoader {
    /// Directory containing `*.toml` pipeline files.
    dir: PathBuf,
    /// Cached definitions, keyed by pipeline name.
    cache: Arc<RwLock<HashMap<String, PipelineDefinition>>>,
}

impl TomlPipelineLoader {
    /// Create a new loader for the given directory.
    /// Does **not** load definitions yet — call `reload()` or `load_initial()`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all TOML files on startup.  Returns reload events.
    pub async fn load_initial(&self) -> anyhow::Result<Vec<ReloadEvent>> {
        self.reload().await
    }

    /// Read and parse a single TOML file into a `PipelineDefinition`.
    fn parse_file(path: &Path) -> Result<PipelineDefinition, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;

        let toml_val: PipelineToml =
            toml::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))?;

        let def = toml_val.into_definition();

        def.validate()
            .map_err(|e| format!("validate {}: {e}", path.display()))?;

        Ok(def)
    }

    /// Scan the directory for `*.toml` files (non-recursive).
    fn list_toml_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) => {
                warn!(dir = %self.dir.display(), error = %e, "cannot read pipeline directory");
                return files;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") && path.is_file() {
                files.push(path);
            }
        }
        files.sort();
        files
    }
}

#[async_trait]
impl PipelineStorePort for TomlPipelineLoader {
    async fn get(&self, name: &str) -> Option<PipelineDefinition> {
        self.cache.read().await.get(name).cloned()
    }

    async fn list(&self) -> Vec<String> {
        self.cache.read().await.keys().cloned().collect()
    }

    async fn reload(&self) -> anyhow::Result<Vec<ReloadEvent>> {
        let files = self.list_toml_files();
        let mut new_defs: HashMap<String, PipelineDefinition> = HashMap::new();
        let mut events = Vec::new();

        for path in &files {
            match Self::parse_file(path) {
                Ok(def) => {
                    new_defs.insert(def.name.clone(), def);
                }
                Err(error) => {
                    let file = path.display().to_string();
                    warn!(%file, %error, "pipeline TOML load failed");
                    events.push(ReloadEvent::Failed { file, error });
                }
            }
        }

        let mut cache = self.cache.write().await;

        // Detect updates and removals
        for (name, old_def) in cache.iter() {
            if let Some(new_def) = new_defs.get(name) {
                if old_def.version != new_def.version {
                    info!(
                        pipeline = %name,
                        old_version = %old_def.version,
                        new_version = %new_def.version,
                        "pipeline reloaded"
                    );
                    events.push(ReloadEvent::Updated {
                        name: name.clone(),
                        old_version: Some(old_def.version.clone()),
                        new_version: new_def.version.clone(),
                    });
                }
            } else {
                warn!(pipeline = %name, "pipeline removed (TOML file deleted)");
                events.push(ReloadEvent::Removed { name: name.clone() });
            }
        }

        // Detect new pipelines
        for (name, def) in &new_defs {
            if !cache.contains_key(name) {
                info!(pipeline = %name, version = %def.version, "pipeline loaded");
                events.push(ReloadEvent::Updated {
                    name: name.clone(),
                    old_version: None,
                    new_version: def.version.clone(),
                });
            }
        }

        *cache = new_defs;
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_toml(dir: &Path, filename: &str, content: &str) {
        fs::write(dir.join(filename), content).unwrap();
    }

    const SIMPLE_PIPELINE: &str = r#"
[pipeline]
name = "test-simple"
version = "1.0"
entry_point = "step1"

[[steps]]
id = "step1"
agent_id = "agent-a"
next = "step2"

[[steps]]
id = "step2"
agent_id = "agent-b"
next = "end"
"#;

    const UPDATED_PIPELINE: &str = r#"
[pipeline]
name = "test-simple"
version = "2.0"
entry_point = "step1"

[[steps]]
id = "step1"
agent_id = "agent-a"
next = "step2"

[[steps]]
id = "step2"
agent_id = "agent-b"
next = "end"
"#;

    #[tokio::test]
    async fn load_initial_empty_dir() {
        let dir = TempDir::new().unwrap();
        let loader = TomlPipelineLoader::new(dir.path());
        let events = loader.load_initial().await.unwrap();
        assert!(events.is_empty());
        assert!(loader.list().await.is_empty());
    }

    #[tokio::test]
    async fn load_single_pipeline() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "simple.toml", SIMPLE_PIPELINE);

        let loader = TomlPipelineLoader::new(dir.path());
        let events = loader.load_initial().await.unwrap();

        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], ReloadEvent::Updated { name, new_version, old_version }
            if name == "test-simple" && new_version == "1.0" && old_version.is_none())
        );

        let def = loader.get("test-simple").await.unwrap();
        assert_eq!(def.steps.len(), 2);
        assert_eq!(def.entry_point, "step1");
    }

    #[tokio::test]
    async fn reload_detects_version_change() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "simple.toml", SIMPLE_PIPELINE);

        let loader = TomlPipelineLoader::new(dir.path());
        loader.load_initial().await.unwrap();

        // Update the file
        write_toml(dir.path(), "simple.toml", UPDATED_PIPELINE);
        let events = loader.reload().await.unwrap();

        let updated = events
            .iter()
            .find(|e| matches!(e, ReloadEvent::Updated { name, .. } if name == "test-simple"));
        assert!(updated.is_some());
        if let Some(ReloadEvent::Updated {
            old_version,
            new_version,
            ..
        }) = updated
        {
            assert_eq!(old_version.as_deref(), Some("1.0"));
            assert_eq!(new_version, "2.0");
        }

        let def = loader.get("test-simple").await.unwrap();
        assert_eq!(def.version, "2.0");
    }

    #[tokio::test]
    async fn reload_detects_removal() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "simple.toml", SIMPLE_PIPELINE);

        let loader = TomlPipelineLoader::new(dir.path());
        loader.load_initial().await.unwrap();

        // Delete the file
        fs::remove_file(dir.path().join("simple.toml")).unwrap();
        let events = loader.reload().await.unwrap();

        let removed = events
            .iter()
            .find(|e| matches!(e, ReloadEvent::Removed { name } if name == "test-simple"));
        assert!(removed.is_some());
        assert!(loader.get("test-simple").await.is_none());
    }

    #[tokio::test]
    async fn invalid_toml_reported_as_failed() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "bad.toml", "this is not valid toml {{{}}}");

        let loader = TomlPipelineLoader::new(dir.path());
        let events = loader.load_initial().await.unwrap();

        let failed = events
            .iter()
            .find(|e| matches!(e, ReloadEvent::Failed { .. }));
        assert!(failed.is_some());
        assert!(loader.list().await.is_empty());
    }

    #[tokio::test]
    async fn invalid_pipeline_validation() {
        let bad = r#"
[pipeline]
name = "broken"
version = "1.0"
entry_point = "nonexistent"

[[steps]]
id = "step1"
agent_id = "agent-a"
next = "end"
"#;
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "broken.toml", bad);

        let loader = TomlPipelineLoader::new(dir.path());
        let events = loader.load_initial().await.unwrap();

        let failed = events
            .iter()
            .find(|e| matches!(e, ReloadEvent::Failed { .. }));
        assert!(failed.is_some());
    }

    #[tokio::test]
    async fn ignores_non_toml_files() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "simple.toml", SIMPLE_PIPELINE);
        write_toml(dir.path(), "notes.md", "# not a pipeline");
        write_toml(dir.path(), "data.json", "{}");

        let loader = TomlPipelineLoader::new(dir.path());
        loader.load_initial().await.unwrap();

        assert_eq!(loader.list().await.len(), 1);
    }

    #[tokio::test]
    async fn load_content_creation_fixture() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pipelines");
        if !dir.exists() {
            return; // skip if fixture dir absent
        }
        let loader = TomlPipelineLoader::new(&dir);
        let events = loader.load_initial().await.unwrap();
        // No failures expected
        for event in &events {
            assert!(
                !matches!(event, ReloadEvent::Failed { .. }),
                "unexpected failure: {event:?}"
            );
        }

        let def = loader.get("content-creation").await.unwrap();
        assert_eq!(def.version, "1.0");
        assert_eq!(def.entry_point, "research");
        assert_eq!(def.max_depth, 3);
        assert_eq!(def.timeout_secs, Some(3600));
        assert_eq!(def.steps.len(), 4);
        assert!(def.validate().is_ok());

        // Step details
        let research = def.step("research").unwrap();
        assert_eq!(research.agent_id, "news-reader");
        assert_eq!(
            research.tools,
            vec!["web_search", "rss_fetch", "memory_read"]
        );
        assert!(research.output_schema.is_some());

        let draft = def.step("draft").unwrap();
        assert_eq!(draft.agent_id, "copywriter");
        assert_eq!(draft.max_retries, 2);

        let publish = def.step("publish").unwrap();
        assert!(publish.next.is_end());
    }

    #[tokio::test]
    async fn schema_validation_with_fixture() {
        use crate::fork_adapters::pipeline::schema_validator::validate_against_schema;

        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pipelines");
        if !dir.exists() {
            return;
        }
        let loader = TomlPipelineLoader::new(&dir);
        loader.load_initial().await.unwrap();
        let def = loader.get("content-creation").await.unwrap();

        // Valid research output
        let research = def.step("research").unwrap();
        let good = serde_json::json!({
            "topic": "Rust async",
            "sources": ["https://tokio.rs"],
            "summary": "Overview of async patterns in Rust"
        });
        assert!(validate_against_schema(&good, research.output_schema.as_ref()).is_ok());

        // Missing required field
        let bad = serde_json::json!({"topic": "Rust"});
        assert!(validate_against_schema(&bad, research.output_schema.as_ref()).is_err());

        // Valid draft output
        let draft = def.step("draft").unwrap();
        let good_draft = serde_json::json!({
            "title": "Why Rust",
            "body": "Rust provides memory safety without GC",
            "tags": ["rust"]
        });
        assert!(validate_against_schema(&good_draft, draft.output_schema.as_ref()).is_ok());
    }
}
