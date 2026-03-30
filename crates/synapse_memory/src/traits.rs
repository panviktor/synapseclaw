//! Memory trait — re-exported from fork_core.
//!
//! The canonical definition now lives in `synapse_core::ports::memory_backend`.

pub use synapse_core::domain::memory::{MemoryCategory, MemoryEntry};
pub use synapse_core::ports::memory_backend::Memory;
