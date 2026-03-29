//! Port: Runtime adapter — abstracts platform-specific execution.
//!
//! Implement this trait to port the agent to a new execution environment.

use std::path::{Path, PathBuf};

/// Runtime adapter that abstracts platform differences for the agent.
///
/// Implementations must be `Send + Sync` because the adapter is shared
/// across async tasks on the Tokio runtime.
pub trait RuntimeAdapter: Send + Sync {
    /// Return the human-readable name of this runtime environment.
    fn name(&self) -> &str;

    /// Report whether this runtime supports shell command execution.
    fn has_shell_access(&self) -> bool;

    /// Report whether this runtime supports filesystem read/write.
    fn has_filesystem_access(&self) -> bool;

    /// Return the base directory for persistent storage on this runtime.
    fn storage_path(&self) -> PathBuf;

    /// Report whether this runtime supports long-running background processes.
    fn supports_long_running(&self) -> bool;

    /// Return the maximum memory budget in bytes for this runtime.
    /// A value of `0` (the default) indicates no limit.
    fn memory_budget(&self) -> u64 {
        0
    }

    /// Build a shell command process configured for this runtime.
    fn build_shell_command(
        &self,
        command: &str,
        workspace_dir: &Path,
    ) -> anyhow::Result<tokio::process::Command>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyRuntime;

    impl RuntimeAdapter for DummyRuntime {
        fn name(&self) -> &str {
            "dummy-runtime"
        }
        fn has_shell_access(&self) -> bool {
            true
        }
        fn has_filesystem_access(&self) -> bool {
            true
        }
        fn storage_path(&self) -> PathBuf {
            PathBuf::from("/tmp/dummy-runtime")
        }
        fn supports_long_running(&self) -> bool {
            true
        }
        fn build_shell_command(
            &self,
            command: &str,
            workspace_dir: &Path,
        ) -> anyhow::Result<tokio::process::Command> {
            let mut cmd = tokio::process::Command::new("echo");
            cmd.arg(command);
            cmd.current_dir(workspace_dir);
            Ok(cmd)
        }
    }

    #[test]
    fn default_memory_budget_is_zero() {
        let runtime = DummyRuntime;
        assert_eq!(runtime.memory_budget(), 0);
    }

    #[test]
    fn runtime_reports_capabilities() {
        let runtime = DummyRuntime;
        assert_eq!(runtime.name(), "dummy-runtime");
        assert!(runtime.has_shell_access());
        assert!(runtime.has_filesystem_access());
        assert!(runtime.supports_long_running());
        assert_eq!(runtime.storage_path(), PathBuf::from("/tmp/dummy-runtime"));
    }
}
