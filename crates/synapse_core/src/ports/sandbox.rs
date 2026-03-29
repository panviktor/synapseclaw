//! Port: Sandbox trait for pluggable OS-level isolation.

use async_trait::async_trait;
use std::process::Command;

/// Sandbox backend for OS-level process isolation.
///
/// Implementations must be `Send + Sync` because the sandbox may be shared
/// across concurrent tool executions on the Tokio runtime.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Wrap a command with sandbox protection.
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()>;

    /// Check if this sandbox backend is available on the current platform.
    fn is_available(&self) -> bool;

    /// Return the human-readable name of this sandbox backend.
    fn name(&self) -> &str;

    /// Return a brief description of the isolation guarantees.
    fn description(&self) -> &str;
}

/// No-op sandbox that provides no additional OS-level isolation.
#[derive(Debug, Clone, Default)]
pub struct NoopSandbox;

impl Sandbox for NoopSandbox {
    fn wrap_command(&self, _cmd: &mut Command) -> std::io::Result<()> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "none"
    }

    fn description(&self) -> &str {
        "No sandboxing (application-layer security only)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sandbox_name() {
        assert_eq!(NoopSandbox.name(), "none");
    }

    #[test]
    fn noop_sandbox_is_always_available() {
        assert!(NoopSandbox.is_available());
    }

    #[test]
    fn noop_sandbox_wrap_command_is_noop() {
        let mut cmd = Command::new("echo");
        cmd.arg("test");
        let original_program = cmd.get_program().to_string_lossy().to_string();
        let original_args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        let sandbox = NoopSandbox;
        assert!(sandbox.wrap_command(&mut cmd).is_ok());

        assert_eq!(cmd.get_program().to_string_lossy(), original_program);
        assert_eq!(
            cmd.get_args()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            original_args
        );
    }
}
