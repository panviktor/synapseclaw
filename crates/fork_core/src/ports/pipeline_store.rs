//! Port: pipeline definition store.
//!
//! Phase 4.1 Slice 1: loads, caches, and watches pipeline definitions.
//! Implementations: `TomlPipelineLoader` (reads TOML files from a directory).

use crate::domain::pipeline::PipelineDefinition;
use async_trait::async_trait;

/// Port for loading and managing pipeline definitions.
///
/// The pipeline engine uses this port to resolve pipeline names to
/// definitions.  Implementations handle format-specific loading (TOML)
/// and optional hot-reload.
#[async_trait]
pub trait PipelineStorePort: Send + Sync {
    /// Load a pipeline definition by name.
    /// Returns `None` if the pipeline does not exist.
    async fn get(&self, name: &str) -> Option<PipelineDefinition>;

    /// List all available pipeline names.
    async fn list(&self) -> Vec<String>;

    /// Reload definitions from source (e.g. re-read TOML directory).
    /// Called by the hot-reload watcher or manually.
    async fn reload(&self) -> anyhow::Result<Vec<ReloadEvent>>;
}

/// Events emitted during a reload cycle.
#[derive(Debug, Clone)]
pub enum ReloadEvent {
    /// A pipeline definition was added or updated.
    Updated {
        name: String,
        old_version: Option<String>,
        new_version: String,
    },
    /// A pipeline definition was removed (TOML file deleted).
    Removed { name: String },
    /// A TOML file failed to parse or validate.
    Failed { file: String, error: String },
}
