//! Fork-owned application core (Phase 4.0).
//!
//! This is a pure business-logic crate — no dependencies on upstream
//! transport, provider, or infrastructure modules.
//!
//! Design rule: `synapse_domain` owns *what* happens; adapters own *how*.

pub mod application;
pub mod bus;
pub mod config;
pub mod domain;
pub mod ports;
