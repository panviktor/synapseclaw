//! Memory trait — re-exported from fork_core.
//!
//! The canonical definition now lives in `fork_core::ports::memory_backend`.

pub use fork_core::domain::memory::{MemoryCategory, MemoryEntry};
pub use fork_core::ports::memory_backend::Memory;
