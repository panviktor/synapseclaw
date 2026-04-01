//! Head-to-head comparison: SQLite vs Markdown memory backends
//!
//! Phase 4.3: Both SQLite and Markdown backends have been replaced by SurrealDB.
//! This comparison test is no longer applicable.
//!
//! TODO(phase4.3): Replace with SurrealDB performance benchmarks if needed.

use synapseclaw::memory::UnifiedMemoryPort;

#[tokio::test]
async fn noop_memory_name_is_noop() {
    let mem = synapseclaw::memory::NoopUnifiedMemory;
    assert_eq!(mem.name(), "noop");
}
