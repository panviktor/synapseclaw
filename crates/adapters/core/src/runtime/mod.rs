//! Runtime adapters — agent execution, hook lifecycle, and platform backends.

pub mod agent_runtime_adapter;
pub mod docker;
pub mod history_compaction_cache;
pub mod hooks_adapter;
pub mod native;
pub(crate) mod runtime_error_classification;

use synapse_domain::config::schema::RuntimeConfig;
pub use synapse_domain::ports::runtime::RuntimeAdapter;

/// Factory: create the right runtime from config
pub fn create_runtime(config: &RuntimeConfig) -> anyhow::Result<Box<dyn RuntimeAdapter>> {
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
