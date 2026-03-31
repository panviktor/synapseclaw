//! Shared infrastructure adapters used by multiple higher-level crates
//! (channels, gateway, tools, onboard).
//!
//! Contains config I/O, identity management, approval flow, and runtime backends.

pub mod approval;
pub mod config_io;
pub mod docker;
pub mod identity;
pub mod native;
pub mod workspace;
pub mod workspace_io;

pub use synapse_domain::ports::runtime::RuntimeAdapter;

/// Factory: create the right runtime backend from config.
pub fn create_runtime(
    config: &synapse_domain::config::schema::RuntimeConfig,
) -> anyhow::Result<Box<dyn RuntimeAdapter>> {
    match config.kind.as_str() {
        "native" => Ok(Box::new(native::NativeRuntime::new())),
        "docker" => Ok(Box::new(docker::DockerRuntime::new(config.docker.clone()))),
        "cloudflare" => anyhow::bail!(
            "runtime.kind='cloudflare' is not implemented yet. Use runtime.kind='native' for now."
        ),
        other if other.trim().is_empty() => {
            anyhow::bail!("runtime.kind cannot be empty. Supported values: native, docker")
        }
        other => anyhow::bail!("Unknown runtime kind '{other}'. Supported values: native, docker"),
    }
}
