//! Configuration domain types — pure value objects for the config schema.
//!
//! These types define the shape of SynapseClaw's configuration.
//! IO operations (load/save) live in the binary crate.
//! The full schema (with proxy infrastructure) lives in `crate::config::schema`.

pub mod adapter_configs;
pub mod channel_traits;
pub mod model_catalog;
pub mod provider_aliases;
pub mod schema;
pub mod workload;
