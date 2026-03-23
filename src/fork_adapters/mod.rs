//! Fork-owned adapters — infrastructure implementations of `fork_core` ports.
//!
//! Design rule: `fork_core` owns *what* happens; `fork_adapters` owns *how*.

pub mod channels;
