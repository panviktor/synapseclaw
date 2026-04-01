//! SurrealDB memory integration tests — Phase 4.3.
//!
//! Tests entity operations, session scoping, and backend identity.

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
async fn surrealdb_entity_upsert_and_find() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    let entity = synapseclaw::synapse_domain::domain::memory::Entity {
        id: String::new(),
        name: "Rust".to_string(),
        entity_type: "concept".to_string(),
        properties: serde_json::json!({}),
        summary: Some("Systems programming language".to_string()),
        created_by: "test".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mem.upsert_entity(entity).await.unwrap();

    let found = mem.find_entity("Rust").await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().entity_type, "concept");

    // Case-insensitive
    let found_lower = mem.find_entity("rust").await.unwrap();
    assert!(found_lower.is_some());
}

#[tokio::test]
async fn surrealdb_multiple_stores_recall() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    for i in 0..5 {
        mem.store(
            &format!("fact_{i}"),
            &format!("Interesting fact number {i} about Rust programming"),
            &MemoryCategory::Core,
            None,
        )
        .await
        .unwrap();
    }

    let results = mem.recall("Rust programming", 3, None).await.unwrap();
    assert!(
        results.len() <= 3,
        "Should respect limit: got {}",
        results.len()
    );
}

#[tokio::test]
async fn surrealdb_session_scoped_store() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;

    mem.store(
        "session_note",
        "This is session-specific",
        &MemoryCategory::Conversation,
        Some("session-123"),
    )
    .await
    .unwrap();

    mem.store("global_note", "This is global", &MemoryCategory::Core, None)
        .await
        .unwrap();

    let count = mem.count().await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn surrealdb_name_is_surrealdb() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mem = create_test_memory(tmp.path()).await;
    assert_eq!(mem.name(), "surrealdb");
}
