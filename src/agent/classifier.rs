//! Query classifier — re-exported from fork_core.
//!
//! The canonical implementation now lives in
//! `synapse_core::domain::query_classification`. This module re-exports
//! the public API so that existing import paths continue to compile.

pub use synapse_core::domain::query_classification::*;
