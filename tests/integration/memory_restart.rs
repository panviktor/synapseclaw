//! TG5: Memory Restart Resilience Tests
//!
//! Phase 4.3: These tests previously tested SqliteMemory directly.
//! They will be rewritten against SurrealMemoryAdapter in Phase 4.3.
//!
//! TODO(phase4.3): Rewrite with SurrealMemoryAdapter::new()

use synapseclaw::memory::UnifiedMemoryPort;

#[tokio::test]
async fn noop_memory_health_check_returns_true() {
    let mem = synapseclaw::memory::NoopUnifiedMemory;
    assert!(mem.health_check().await);
}

#[tokio::test]
async fn noop_memory_count_returns_zero() {
    let mem = synapseclaw::memory::NoopUnifiedMemory;
    assert_eq!(mem.count().await.unwrap(), 0);
}

#[tokio::test]
async fn noop_memory_recall_returns_empty() {
    let mem = synapseclaw::memory::NoopUnifiedMemory;
    let results = mem.recall("anything", 10, None).await.unwrap();
    assert!(results.is_empty());
}
