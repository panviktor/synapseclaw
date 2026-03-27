//! TOML-backed message router adapter.
//!
//! Phase 4.1 Slice 6: loads routing rules from a TOML file, implements
//! MessageRouterPort with file-based reload.

use async_trait::async_trait;
use fork_core::domain::routing::{RoutingInput, RoutingResult, RoutingTable, RoutingToml};
use fork_core::ports::message_router::MessageRouterPort;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Adapter: loads routing rules from a TOML file.
pub struct TomlMessageRouter {
    /// Path to the routing TOML file.
    path: PathBuf,
    /// Current routing table.
    table: RwLock<RoutingTable>,
}

impl TomlMessageRouter {
    /// Create a router from a TOML file.
    /// Falls back to a default table if the file doesn't exist.
    pub fn load(path: impl Into<PathBuf>, default_fallback: &str) -> Self {
        let path = path.into();
        let table = match Self::parse_file(&path) {
            Ok(t) => {
                info!(
                    path = %path.display(),
                    routes = t.routes.len(),
                    fallback = %t.fallback,
                    "routing table loaded"
                );
                t
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "routing TOML not loaded, using default fallback"
                );
                RoutingTable {
                    routes: vec![],
                    fallback: default_fallback.into(),
                }
            }
        };

        Self {
            path,
            table: RwLock::new(table),
        }
    }

    fn parse_file(path: &std::path::Path) -> Result<RoutingTable, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let toml_val: RoutingToml =
            toml::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(toml_val.into_table())
    }
}

#[async_trait]
impl MessageRouterPort for TomlMessageRouter {
    async fn route(&self, input: &RoutingInput) -> RoutingResult {
        self.table.read().await.resolve(input)
    }

    async fn reload(&self) -> anyhow::Result<()> {
        let new_table = Self::parse_file(&self.path)
            .map_err(|e| anyhow::anyhow!("routing reload failed: {e}"))?;
        info!(
            path = %self.path.display(),
            routes = new_table.routes.len(),
            "routing table reloaded"
        );
        *self.table.write().await = new_table;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;

    const ROUTING_TOML: &str = r#"
fallback = "marketing-lead"

[[routes]]
name = "research"
target = "news-reader"
priority = 10

[routes.rule]
command = "/research"

[[routes]]
name = "deploy"
target = "devops"
priority = 20

[routes.rule]
keywords = ["deploy", "restart", "server"]
"#;

    #[tokio::test]
    async fn load_and_route() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("routing.toml");
        fs::write(&path, ROUTING_TOML).unwrap();

        let router = TomlMessageRouter::load(&path, "default");

        let r = router
            .route(&RoutingInput {
                content: "/research AI trends".into(),
                source_kind: "channel".into(),
                metadata: HashMap::default(),
            })
            .await;
        assert_eq!(r.target, "news-reader");

        let r2 = router
            .route(&RoutingInput {
                content: "please deploy the new version".into(),
                source_kind: "channel".into(),
                metadata: HashMap::default(),
            })
            .await;
        assert_eq!(r2.target, "devops");

        let r3 = router
            .route(&RoutingInput {
                content: "hello there".into(),
                source_kind: "channel".into(),
                metadata: HashMap::default(),
            })
            .await;
        assert_eq!(r3.target, "marketing-lead");
        assert!(r3.is_fallback);
    }

    #[tokio::test]
    async fn load_missing_file_uses_default() {
        let router = TomlMessageRouter::load("/nonexistent/routing.toml", "fallback-agent");

        let r = router
            .route(&RoutingInput {
                content: "test".into(),
                source_kind: "web".into(),
                metadata: HashMap::default(),
            })
            .await;
        assert_eq!(r.target, "fallback-agent");
    }

    #[tokio::test]
    async fn reload_picks_up_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("routing.toml");
        fs::write(&path, ROUTING_TOML).unwrap();

        let router = TomlMessageRouter::load(&path, "default");

        // Change fallback
        let new_toml = ROUTING_TOML.replace("marketing-lead", "new-default");
        fs::write(&path, new_toml).unwrap();

        router.reload().await.unwrap();

        let r = router
            .route(&RoutingInput {
                content: "unmatched".into(),
                source_kind: "channel".into(),
                metadata: HashMap::default(),
            })
            .await;
        assert_eq!(r.target, "new-default");
    }
}
