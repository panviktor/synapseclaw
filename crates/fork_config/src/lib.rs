//! Fork-owned configuration types — shared across all workspace crates.
//!
//! This crate holds config structs, channel/provider config types, and
//! pure helper functions that both fork_adapters and the main binary need.
//! It breaks the circular dependency: fork_adapters can depend on fork_config
//! instead of the main synapseclaw crate.

pub mod adapter_configs;
pub mod channel_traits;
pub mod provider_aliases;
