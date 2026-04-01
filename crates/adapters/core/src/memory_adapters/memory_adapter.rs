//! Phase 4.3: MemoryTiersAdapter is REMOVED.
//!
//! The old `MemoryTiersPort` wrapping `dyn Memory` is superseded by
//! `UnifiedMemoryPort` implemented directly by `SurrealMemoryAdapter`.
//! This module is kept empty to avoid breaking `mod.rs` re-exports.
