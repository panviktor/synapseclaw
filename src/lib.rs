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
    pub use synapse_domain::config::schema;
    pub use synapse_domain::config::schema::*;
    pub use synapse_infra::config_io::ConfigIO;
    pub use synapse_infra::workspace;
    pub use synapse_infra::workspace_io;
}

// CLI command enums.
pub use synapse_adapters::commands::{
    ChannelCommands, CronCommands, GatewayCommands, IntegrationCommands, MemoryCommands,
    PipelineCommands, ServiceCommands, SkillCommands,
};

// Memory.
pub use synapse_memory as memory;

// Facade re-exports for integration tests.
pub use synapse_adapters::channels;
pub use synapse_adapters::gateway;
pub use synapse_adapters::hooks;
pub use synapse_adapters::tools;
pub use synapse_observability as observability;
pub use synapse_providers as providers;
