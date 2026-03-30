//! Hot-reload watcher for pipeline TOML files.
//!
//! Phase 4.1 Slice 8: watches the pipeline directory for changes using
//! the `notify` crate, triggers reload on the PipelineStorePort.
//!
//! Running pipelines always complete on the definition version they started
//! with. New runs pick up the updated definition.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use synapse_core::ports::pipeline_store::{PipelineStorePort, ReloadEvent};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Start a background task that watches a directory for TOML changes
/// and triggers reload on the pipeline store.
///
/// Returns a handle to stop the watcher (drop the sender).
///
/// Events are debounced: multiple rapid file changes within 500ms
/// result in a single reload.
pub fn start_watcher(
    dir: PathBuf,
    store: Arc<dyn PipelineStorePort>,
) -> Result<WatcherHandle, notify::Error> {
    let (tx, mut rx) = mpsc::channel::<()>(16);

    // Create filesystem watcher
    let tx_clone = tx.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // Only react to TOML file modifications/creations/deletions
                    let dominated_by_toml = event
                        .paths
                        .iter()
                        .any(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"));
                    let is_relevant = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    );
                    if dominated_by_toml && is_relevant {
                        let _ = tx_clone.try_send(());
                    }
                }
                Err(e) => {
                    warn!(error = %e, "filesystem watcher error");
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    info!(dir = %dir.display(), "pipeline hot-reload watcher started");

    // Spawn debounced reload task
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        // Keep watcher alive
        let _watcher = watcher;

        loop {
            tokio::select! {
                Some(()) = rx.recv() => {
                    // Debounce: drain any queued events, wait 500ms for more
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    while rx.try_recv().is_ok() {}

                    // Trigger reload
                    match store.reload().await {
                        Ok(events) => {
                            for event in &events {
                                match event {
                                    ReloadEvent::Updated { name, old_version, new_version } => {
                                        info!(
                                            pipeline = %name,
                                            old_version = old_version.as_deref().unwrap_or("(new)"),
                                            new_version = %new_version,
                                            "pipeline reloaded"
                                        );
                                    }
                                    ReloadEvent::Removed { name } => {
                                        warn!(pipeline = %name, "pipeline removed");
                                    }
                                    ReloadEvent::Failed { file, error } => {
                                        error!(file = %file, error = %error, "pipeline reload failed");
                                    }
                                }
                            }
                            if events.is_empty() {
                                info!("pipeline reload: no changes detected");
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "pipeline reload failed");
                        }
                    }
                }
                _ = stop_rx.recv() => {
                    info!("pipeline hot-reload watcher stopped");
                    break;
                }
            }
        }
    });

    Ok(WatcherHandle { _stop: stop_tx })
}

/// Handle for stopping the watcher. Drop to stop.
pub struct WatcherHandle {
    _stop: mpsc::Sender<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use synapse_core::domain::pipeline::PipelineDefinition;
    use tempfile::TempDir;

    struct CountingStore {
        reload_count: AtomicU32,
    }

    #[async_trait]
    impl PipelineStorePort for CountingStore {
        async fn get(&self, _name: &str) -> Option<PipelineDefinition> {
            None
        }
        async fn list(&self) -> Vec<String> {
            vec![]
        }
        async fn reload(&self) -> anyhow::Result<Vec<ReloadEvent>> {
            self.reload_count.fetch_add(1, Ordering::Relaxed);
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn watcher_detects_toml_change() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(CountingStore {
            reload_count: AtomicU32::new(0),
        });

        let handle = start_watcher(dir.path().to_path_buf(), store.clone()).unwrap();

        // Write a TOML file — should trigger reload
        std::fs::write(dir.path().join("test.toml"), "[pipeline]\nname = \"x\"").unwrap();

        // Wait for debounce + reload
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        assert!(
            store.reload_count.load(Ordering::Relaxed) >= 1,
            "expected at least 1 reload, got {}",
            store.reload_count.load(Ordering::Relaxed)
        );

        drop(handle); // stop watcher
    }

    #[tokio::test]
    async fn watcher_ignores_non_toml() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(CountingStore {
            reload_count: AtomicU32::new(0),
        });

        let handle = start_watcher(dir.path().to_path_buf(), store.clone()).unwrap();

        // Write non-TOML file
        std::fs::write(dir.path().join("notes.md"), "# notes").unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        assert_eq!(
            store.reload_count.load(Ordering::Relaxed),
            0,
            "should not reload for non-TOML files"
        );

        drop(handle);
    }
}
