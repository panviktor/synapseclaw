//! CLI memory commands — Phase 4.3 stub.
//!
//! The old factory functions (`create_memory_for_migration`, `classify_memory_backend`,
//! etc.) are removed.  CLI memory management will be re-implemented against
//! `SurrealMemoryAdapter` / `UnifiedMemoryPort`.

use anyhow::{bail, Result};
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
    }
}

/// Create a memory backend for CLI management operations.
///
/// TODO(phase4.3): replace with SurrealMemoryAdapter::new()
fn create_cli_memory(_config: &Config) -> Result<Box<dyn UnifiedMemoryPort>> {
    bail!("Phase 4.3: CLI memory commands are being migrated to SurrealDB. Not yet available.");
}

async fn handle_list(
    config: &Config,
    category: Option<String>,
    session: Option<String>,
    limit: usize,
    offset: usize,
) -> Result<()> {
    let mem = create_cli_memory(config)?;
    let cat = category.as_deref().map(parse_category);
    let entries = mem
        .recall(
            cat.as_ref()
                .map(|c| c.to_string())
                .unwrap_or_default()
                .as_str(),
            limit + offset,
            session.as_deref(),
        )
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
    let mem = create_cli_memory(config)?;

    // Use recall with the key as query to find matching entries.
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
            println!("\nSpecify a longer prefix to narrow the match.");
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
    let mem = create_cli_memory(config)?;
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
    let mem = create_cli_memory(config)?;

    // Single-key deletion (exact or prefix match).
    if let Some(key) = key {
        return handle_clear_key(&*mem, &key, yes).await;
    }

    // TODO(phase4.3): batch deletion by category via SurrealDB
    println!("Phase 4.3: batch clear not yet migrated to SurrealDB.");
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
