//! Query classifier — re-exported from synapse_domain.
//!
//! The canonical implementation now lives in
//! `synapse_domain::domain::query_classification`. This module re-exports
//! the public API so that existing import paths continue to compile.

pub use synapse_domain::domain::query_classification::*;
