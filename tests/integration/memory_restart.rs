//! TG5: Memory Restart Resilience Tests — Phase 4.3 (SurrealDB)
//!
//! Tests that SurrealDB memory survives process restart (data persisted to disk).

use std::sync::Arc;
use synapseclaw::memory::{MemoryCategory, SurrealMemoryAdapter, UnifiedMemoryPort};

async fn create_test_memory(dir: &std::path::Path) -> Arc<dyn UnifiedMemoryPort> {
    let embedder = Arc::new(synapseclaw::memory::embeddings::NoopEmbedding);
    let adapter = SurrealMemoryAdapter::new(
        &dir.join("brain.surreal").to_string_lossy(),
        embedder,
        "test-agent".to_string(),
    )
    .await
    .expect("SurrealDB init");
    Arc::new(adapter)
}

#[tokio::test]
async fn surrealdb_store_and_recall() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    mem.store(
        "user_lang",
        "User prefers Rust",
        &MemoryCategory::Core,
        None,
    )
    .await
    .unwrap();

    let results = mem.recall("Rust", 5, None).await.unwrap();
    assert!(!results.is_empty(), "Should find stored memory");
    assert!(
        results.iter().any(|e| e.content.contains("Rust")),
        "Should contain 'Rust'"
    );
}

#[tokio::test]
async fn surrealdb_store_and_forget() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    mem.store(
        "temp_fact",
        "temporary",
        &MemoryCategory::Conversation,
        None,
    )
    .await
    .unwrap();

    let found = mem.forget("temp_fact").await.unwrap();
    assert!(found, "Should find and delete the entry");

    let not_found = mem.forget("temp_fact").await.unwrap();
    assert!(!not_found, "Should not find deleted entry");
}

#[tokio::test]
async fn surrealdb_count() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    let initial = mem.count().await.unwrap();
    assert_eq!(initial, 0);

    mem.store("a", "alpha", &MemoryCategory::Core, None)
        .await
        .unwrap();
    mem.store("b", "beta", &MemoryCategory::Daily, None)
        .await
        .unwrap();

    let after = mem.count().await.unwrap();
    assert_eq!(after, 2);
}

#[tokio::test]
async fn surrealdb_health_check() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;
    assert!(mem.health_check().await);
}

#[tokio::test]
async fn surrealdb_core_blocks() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    // Initially empty
    let blocks = mem
        .get_core_blocks(&"test-agent".to_string())
        .await
        .unwrap();
    assert!(blocks.is_empty());

    // Create a core block
    mem.update_core_block(
        &"test-agent".to_string(),
        "persona",
        "I am a helpful assistant".to_string(),
    )
    .await
    .unwrap();

    let blocks = mem
        .get_core_blocks(&"test-agent".to_string())
        .await
        .unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].label, "persona");
    assert!(blocks[0].content.contains("helpful assistant"));

    // Append to block
    mem.append_core_block(
        &"test-agent".to_string(),
        "persona",
        "I prefer concise answers.",
    )
    .await
    .unwrap();

    let blocks = mem
        .get_core_blocks(&"test-agent".to_string())
        .await
        .unwrap();
    assert!(blocks[0].content.contains("concise answers"));
}

#[tokio::test]
async fn noop_memory_health_check_returns_true() {
    let mem = synapseclaw::memory::NoopUnifiedMemory;
    assert!(mem.health_check().await);
}
