//! Run context — re-exported from fork_core.
//!
//! The canonical implementation now lives in
//! `synapse_domain::domain::tool_audit`. This module re-exports the public
//! API so that existing `use crate::agent::run_context::*` paths
//! continue to compile unchanged.

pub use synapse_domain::domain::tool_audit::*;
