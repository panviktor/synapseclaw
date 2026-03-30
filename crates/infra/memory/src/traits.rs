//! Memory trait — re-exported from fork_core.
//!
//! The canonical definition now lives in `synapse_domain::ports::memory_backend`.

pub use synapse_domain::domain::memory::{MemoryCategory, MemoryEntry};
pub use synapse_domain::ports::memory_backend::Memory;
