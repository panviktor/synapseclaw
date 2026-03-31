//! SynapseClaw — thin facade crate.
//!
//! All real code lives in workspace crates. This lib re-exports
//! the public API for integration tests and external consumers.

// Re-export workspace crates.
pub use synapse_adapters as adapters;
pub use synapse_adapters::agent;
pub use synapse_domain;
pub use synapse_memory;
pub use synapse_security;

// Config facade.
pub mod config {
    pub use synapse_infra::config_io::ConfigIO;
    pub use synapse_infra::workspace;
    pub use synapse_infra::workspace_io;
    pub use synapse_domain::config::schema;
    pub use synapse_domain::config::schema::Config;
}

// CLI command enums.
pub use synapse_adapters::commands::{
    ChannelCommands, CronCommands, GatewayCommands, IntegrationCommands, MemoryCommands,
    ServiceCommands, SkillCommands,
};

// Memory.
pub use synapse_memory as memory;
