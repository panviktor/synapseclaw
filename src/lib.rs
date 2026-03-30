//! SynapseClaw — thin facade crate.
//!
//! All real code lives in workspace crates. This lib re-exports
//! the public API for integration tests and external consumers.

// Composition root modules (kept locally — config IO + memory factory).
pub mod commands;
pub mod config;
pub mod memory;

// Re-export workspace crates.
pub use synapse_adapters as adapters;
pub use synapse_adapters::agent;
pub use synapse_adapters::{channels, gateway, hooks, observability, providers, tools};
pub use synapse_core;
pub use synapse_memory;
pub use synapse_security;

// Convenience re-exports.
pub use commands::{
    ChannelCommands, CronCommands, GatewayCommands, IntegrationCommands, MemoryCommands,
    ServiceCommands, SkillCommands,
};
pub use config::Config;
