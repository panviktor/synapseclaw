//! SQLite → SurrealDB memory migration tool.
//!
//! Reads entries from legacy `brain.db` (SQLite) and inserts them
//! into SurrealDB via UnifiedMemoryPort.

use anyhow::Result;
use std::path::Path;
use synapse_domain::domain::memory::MemoryCategory;
use synapse_memory::UnifiedMemoryPort;

/// Migrate memories from legacy SQLite `brain.db` to SurrealDB.
///
/// Reads all rows from the `memories` table and stores them via
/// `UnifiedMemoryPort::store()`. Returns count of migrated entries.
pub async fn migrate_sqlite_to_surrealdb(
    sqlite_path: &Path,
    memory: &dyn UnifiedMemoryPort,
) -> Result<u32> {
    if !sqlite_path.exists() {
        tracing::info!(
            "No legacy brain.db found at {}, skipping migration",
            sqlite_path.display()
        );
        return Ok(0);
    }

    let conn = rusqlite::Connection::open_with_flags(
        sqlite_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;

    let mut stmt = conn.prepare(
        "SELECT key, content, category, session_id FROM memories ORDER BY created_at ASC",
    )?;

    let mut count = 0u32;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;

    for row in rows {
        let (key, content, category_str, session_id) = row?;
        let category = MemoryCategory::from_str_lossy(&category_str);

        if let Err(e) = memory
            .store(&key, &content, &category, session_id.as_deref())
            .await
        {
            tracing::warn!("Migration: failed to store key '{key}': {e}");
            continue;
        }
        count += 1;

        if count % 100 == 0 {
            tracing::info!("Migration progress: {count} entries migrated");
        }
    }

    tracing::info!(
        "Migration complete: {count} entries migrated from {}",
        sqlite_path.display()
    );
    Ok(count)
}

/// Check if legacy brain.db exists and has entries.
pub fn has_legacy_sqlite(workspace_dir: &Path) -> bool {
    let db_path = workspace_dir.join("memory").join("brain.db");
    if !db_path.exists() {
        return false;
    }
    match rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => {
            conn.query_row("SELECT count(*) FROM memories", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0)
                > 0
        }
        Err(_) => false,
    }
}
