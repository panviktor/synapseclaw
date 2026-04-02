//! CLI memory commands — Phase 4.3 stub.
//!
//! The old factory functions (`create_memory_for_migration`, `classify_memory_backend`,
//! etc.) are removed.  CLI memory management will be re-implemented against
//! `SurrealMemoryAdapter` / `UnifiedMemoryPort`.

use anyhow::Result;
use console::style;
use synapse_domain::config::schema::Config;
use synapse_memory::{MemoryCategory, UnifiedMemoryPort};

/// Handle `synapseclaw memory <subcommand>` CLI commands.
pub async fn handle_command(
    command: crate::commands::MemoryCommands,
    config: &Config,
) -> Result<()> {
    match command {
        crate::commands::MemoryCommands::List {
            category,
            session,
            limit,
            offset,
        } => handle_list(config, category, session, limit, offset).await,
        crate::commands::MemoryCommands::Get { key } => handle_get(config, &key).await,
        crate::commands::MemoryCommands::Stats => handle_stats(config).await,
        crate::commands::MemoryCommands::Clear { key, category, yes } => {
            handle_clear(config, key, category, yes).await
        }
        crate::commands::MemoryCommands::Migrate { yes } => handle_migrate(config, yes).await,
    }
}

/// Create a memory backend for CLI management operations.
async fn create_cli_memory(config: &Config) -> Result<std::sync::Arc<dyn UnifiedMemoryPort>> {
    let backend = synapse_memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        "cli",
        config.api_key.as_deref(),
    )
    .await?;
    Ok(backend.memory)
}

async fn handle_list(
    config: &Config,
    category: Option<String>,
    session: Option<String>,
    limit: usize,
    offset: usize,
) -> Result<()> {
    let mem = create_cli_memory(config).await?;
    let cat = category.as_deref().map(parse_category);
    let entries = mem
        .list(cat.as_ref(), session.as_deref(), limit + offset)
        .await?;

    if entries.is_empty() {
        println!("No memory entries found.");
        return Ok(());
    }

    let total = entries.len();
    let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();

    if page.is_empty() {
        println!("No entries at offset {offset} (total: {total}).");
        return Ok(());
    }

    println!(
        "Memory entries ({total} total, showing {}-{}):\n",
        offset + 1,
        offset + page.len(),
    );

    for entry in &page {
        println!(
            "- {} [{}]",
            style(&entry.key).white().bold(),
            entry.category,
        );
        println!("    {}", truncate_content(&entry.content, 80));
    }

    if offset + page.len() < total {
        println!("\n  Use --offset {} to see the next page.", offset + limit);
    }

    Ok(())
}

async fn handle_get(config: &Config, key: &str) -> Result<()> {
    let mem = create_cli_memory(config).await?;

    // Direct key lookup first, then prefix search fallback.
    if let Some(entry) = mem.get(key).await? {
        print_entry(&entry);
    } else {
        // Fallback: search for prefix matches
        let entries = mem.recall(key, 10, None).await?;
        let matches: Vec<_> = entries.iter().filter(|e| e.key.starts_with(key)).collect();
        match matches.len() {
            0 => println!("No memory entry found for key: {key}"),
            1 => print_entry(matches[0]),
            n => {
                println!("Prefix '{key}' matched {n} entries:\n");
                for entry in matches {
                    println!(
                        "- {} [{}]",
                        style(&entry.key).white().bold(),
                        entry.category
                    );
                }
                println!("\nSpecify the exact key or a longer prefix.");
            }
        }
    }

    Ok(())
}

fn print_entry(entry: &synapse_memory::MemoryEntry) {
    println!("Key:       {}", style(&entry.key).white().bold());
    println!("Category:  {}", entry.category);
    println!("Timestamp: {}", entry.timestamp);
    if let Some(sid) = &entry.session_id {
        println!("Session:   {sid}");
    }
    println!("\n{}", entry.content);
}

async fn handle_stats(config: &Config) -> Result<()> {
    let mem = create_cli_memory(config).await?;
    let healthy = mem.health_check().await;
    let total = mem.count().await.unwrap_or(0);

    println!("Memory Statistics:\n");
    println!("  Backend:  {}", style(mem.name()).white().bold());
    println!(
        "  Health:   {}",
        if healthy {
            style("healthy").green().bold().to_string()
        } else {
            style("unhealthy").yellow().bold().to_string()
        }
    );
    println!("  Total:    {total}");

    Ok(())
}

async fn handle_clear(
    config: &Config,
    key: Option<String>,
    _category: Option<String>,
    yes: bool,
) -> Result<()> {
    let mem = create_cli_memory(config).await?;

    // Single-key deletion (exact or prefix match).
    if let Some(key) = key {
        return handle_clear_key(&*mem, &key, yes).await;
    }

    // Batch clear: not supported without list() — inform user.
    println!(
        "{}",
        style("Use --key <KEY> to delete individual entries.").yellow()
    );
    println!("Batch category clear will be available in a future update.");
    Ok(())
}

/// Delete a single entry by key.
async fn handle_clear_key(mem: &dyn UnifiedMemoryPort, key: &str, yes: bool) -> Result<()> {
    if !yes {
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(format!("  Delete '{key}'?"))
            .default(false)
            .interact()?;
        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }
    }

    if mem.forget(key).await? {
        println!("{} Deleted key: {key}", style("✓").green().bold());
    } else {
        println!("No memory entry found for key: {key}");
    }

    Ok(())
}

async fn handle_migrate(config: &Config, yes: bool) -> Result<()> {
    let sqlite_path = config.workspace_dir.join("memory").join("brain.db");

    if !sqlite_path.exists() {
        println!("No legacy brain.db found at {}", sqlite_path.display());
        return Ok(());
    }

    let count = {
        let conn = rusqlite::Connection::open_with_flags(
            &sqlite_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.query_row("SELECT count(*) FROM memories", [], |row| {
            row.get::<_, i64>(0)
        })?
    };

    println!(
        "Found legacy brain.db: {} entries at {}",
        style(count).white().bold(),
        sqlite_path.display()
    );

    if !yes {
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(format!("  Migrate {count} entries to SurrealDB?"))
            .default(true)
            .interact()?;
        if !confirmed {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mem = create_cli_memory(config).await?;
    let migrated =
        super::migration::migrate_sqlite_to_surrealdb(&sqlite_path, mem.as_ref()).await?;

    println!(
        "{} Migrated {migrated} entries to SurrealDB",
        style("✓").green().bold()
    );

    Ok(())
}

fn parse_category(s: &str) -> MemoryCategory {
    match s.trim().to_ascii_lowercase().as_str() {
        "core" => MemoryCategory::Core,
        "daily" => MemoryCategory::Daily,
        "conversation" => MemoryCategory::Conversation,
        other => MemoryCategory::Custom(other.to_string()),
    }
}

fn truncate_content(s: &str, max_len: usize) -> String {
    let line = s.lines().next().unwrap_or(s);
    if line.len() <= max_len {
        return line.to_string();
    }
    let truncated: String = line.chars().take(max_len.saturating_sub(3)).collect();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_category_known_variants() {
        assert_eq!(parse_category("core"), MemoryCategory::Core);
        assert_eq!(parse_category("daily"), MemoryCategory::Daily);
        assert_eq!(parse_category("conversation"), MemoryCategory::Conversation);
        assert_eq!(parse_category("CORE"), MemoryCategory::Core);
        assert_eq!(parse_category("  Daily  "), MemoryCategory::Daily);
    }

    #[test]
    fn parse_category_custom_fallback() {
        assert_eq!(
            parse_category("project_notes"),
            MemoryCategory::Custom("project_notes".into())
        );
    }

    #[test]
    fn truncate_content_short_text_unchanged() {
        assert_eq!(truncate_content("hello", 10), "hello");
    }

    #[test]
    fn truncate_content_long_text_truncated() {
        let result = truncate_content("this is a very long string", 10);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 10);
    }

    #[test]
    fn truncate_content_multiline_uses_first_line() {
        assert_eq!(truncate_content("first\nsecond", 20), "first");
    }

    #[test]
    fn truncate_content_empty_string() {
        assert_eq!(truncate_content("", 10), "");
    }
}
