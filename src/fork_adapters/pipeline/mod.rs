//! Pipeline adapters — TOML loading, schema validation, IPC execution, hot-reload.
//!
//! Phase 4.1:
//! - Slice 1: `TomlPipelineLoader` and `SchemaValidator`
//! - Slice 2: `IpcStepExecutor`

pub mod ipc_step_executor;
pub mod schema_validator;
pub mod toml_loader;
