//! Fork-owned application core (Phase 4.0).
//!
//! This module owns business semantics that are specific to the fork:
//! routing decisions, delivery policy, capability-driven channel behavior,
//! and the outbound intent bus that connects gateway to channels.
//!
//! Design rule: `fork_core` owns *what* happens; adapters own *how*.

pub mod bus;
pub mod domain;
pub mod ports;
