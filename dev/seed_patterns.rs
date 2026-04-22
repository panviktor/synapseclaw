//! One-shot seed script: inserts default learning signal patterns into all agent DBs.
//! Run with: cargo run --example seed_patterns

use anyhow::Context;
use std::path::{Path, PathBuf};
use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dbs = configured_db_paths()?;
    let patterns = build_patterns();
    println!(
        "Seeding {} patterns into {} databases...",
        patterns.len(),
        dbs.len()
    );

    for db_path in &dbs {
        print!("  {} ... ", db_path.display());
        match seed_one(db_path, &patterns).await {
            Ok(n) => println!("seeded {n} patterns"),
            Err(e) => println!("ERROR: {e}"),
        }
    }

    println!("Done.");
    Ok(())
}

fn configured_db_paths() -> anyhow::Result<Vec<PathBuf>> {
    let explicit: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if !explicit.is_empty() {
        return Ok(explicit);
    }

    let config_dir = std::env::var_os("SYNAPSECLAW_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".synapseclaw")))
        .context("set SYNAPSECLAW_CONFIG_DIR or HOME, or pass database paths as arguments")?;

    let mut paths = vec![config_dir.join("workspace/memory/brain.surreal")];
    let agents_dir = config_dir.join("agents");
    if let Ok(entries) = std::fs::read_dir(&agents_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                paths.push(entry.path().join("workspace/memory/brain.surreal"));
            }
        }
    }
    paths.sort();
    Ok(paths)
}

async fn seed_one(path: &Path, patterns: &[(&str, &str, &str, &str)]) -> anyhow::Result<usize> {
    let db_path = path.to_string_lossy().to_string();
    let db = Surreal::new::<SurrealKv>(db_path.as_str()).await?;
    db.use_ns("synapseclaw").use_db("memory").await?;

    // Apply schema for the new table
    db.query(
        "DEFINE TABLE IF NOT EXISTS learning_signal_pattern SCHEMALESS;
         DEFINE FIELD IF NOT EXISTS signal_type ON learning_signal_pattern TYPE option<string>;
         DEFINE FIELD IF NOT EXISTS pattern     ON learning_signal_pattern TYPE option<string>;
         DEFINE FIELD IF NOT EXISTS match_mode  ON learning_signal_pattern TYPE option<string>;
         DEFINE FIELD IF NOT EXISTS language    ON learning_signal_pattern TYPE option<string>;
         DEFINE FIELD IF NOT EXISTS enabled     ON learning_signal_pattern TYPE option<bool> DEFAULT true;
         DEFINE FIELD IF NOT EXISTS created_at  ON learning_signal_pattern TYPE option<datetime>;
         DEFINE INDEX IF NOT EXISTS idx_lsp_type ON learning_signal_pattern FIELDS signal_type;"
    ).await?;

    // Check if already seeded
    let mut resp = db
        .query("SELECT count() AS total FROM learning_signal_pattern GROUP ALL")
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;
    let existing = rows
        .first()
        .and_then(|v| v.get("total"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if existing > 0 {
        println!("already has {existing} patterns, skipping");
        return Ok(0);
    }

    let mut count = 0;
    for (signal_type, pattern, match_mode, language) in patterns {
        db.query(
            "CREATE learning_signal_pattern SET \
             signal_type = $st, pattern = $pat, match_mode = $mm, \
             language = $lang, enabled = true, created_at = time::now()",
        )
        .bind(("st", signal_type.to_string()))
        .bind(("pat", pattern.to_string()))
        .bind(("mm", match_mode.to_string()))
        .bind(("lang", language.to_string()))
        .await?;
        count += 1;
    }

    Ok(count)
}

fn build_patterns() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    let mut p = Vec::new();

    // Corrections (starts_with)
    for pat in &[
        "actually,",
        "actually ",
        "correction:",
        "correction ",
        "that's wrong",
        "that's not right",
        "that's incorrect",
        "not quite,",
        "no, ",
        "wrong,",
        "wrong.",
        "let me correct",
        "i was wrong",
        "i meant ",
        "i misspoke",
        "you misunderstood",
    ] {
        p.push(("correction", *pat, "starts_with", "en"));
    }
    // Corrections (contains)
    for pat in &[
        "that's wrong",
        "that's not right",
        "that's incorrect",
        "let me correct",
    ] {
        p.push(("correction", *pat, "contains", "en"));
    }

    // Memory (starts_with)
    for pat in &[
        "remember ",
        "remember:",
        "remember,",
        "store:",
        "save this",
        "save that",
        "note:",
        "note that",
        "take note",
        "jot down",
        "keep in mind",
        "don't forget",
        "important:",
        "important,",
    ] {
        p.push(("memory", *pat, "starts_with", "en"));
    }
    for pat in &["запомни", "запомни,"] {
        p.push(("memory", *pat, "starts_with", "ru"));
    }

    // Instructions (starts_with)
    for pat in &[
        "i prefer ",
        "i like ",
        "i don't like ",
        "i dislike ",
        "i hate ",
        "i want ",
        "i need ",
        "i always ",
        "i never ",
        "always ",
        "never ",
        "from now on",
        "going forward",
        "in the future",
        "my preference is",
        "my style is",
    ] {
        p.push(("instruction", *pat, "starts_with", "en"));
    }
    for pat in &["я предпочитаю", "мне нравится"] {
        p.push(("instruction", *pat, "starts_with", "ru"));
    }

    p
}
