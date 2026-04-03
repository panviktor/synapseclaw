//! One-shot seed script: inserts default learning signal patterns into all agent DBs.
//! Run with: cargo run --example seed_patterns

use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dbs = vec![
        "/home/protosik00/.synapseclaw/workspace/memory/brain.surreal",
        "/home/protosik00/.synapseclaw/agents/copywriter/workspace/memory/brain.surreal",
        "/home/protosik00/.synapseclaw/agents/marketing-lead/workspace/memory/brain.surreal",
        "/home/protosik00/.synapseclaw/agents/news-reader/workspace/memory/brain.surreal",
        "/home/protosik00/.synapseclaw/agents/publisher/workspace/memory/brain.surreal",
        "/home/protosik00/.synapseclaw/agents/trend-aggregator/workspace/memory/brain.surreal",
    ];

    let patterns = build_patterns();
    println!("Seeding {} patterns into {} databases...", patterns.len(), dbs.len());

    for db_path in &dbs {
        print!("  {db_path} ... ");
        match seed_one(db_path, &patterns).await {
            Ok(n) => println!("seeded {n} patterns"),
            Err(e) => println!("ERROR: {e}"),
        }
    }

    println!("Done.");
    Ok(())
}

async fn seed_one(path: &str, patterns: &[(& str, &str, &str, &str)]) -> anyhow::Result<usize> {
    let db = Surreal::new::<SurrealKv>(path).await?;
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
    let mut resp = db.query("SELECT count() AS total FROM learning_signal_pattern GROUP ALL").await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;
    let existing = rows.first()
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
             language = $lang, enabled = true, created_at = time::now()"
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
        "actually,", "actually ", "correction:", "correction ",
        "that's wrong", "that's not right", "that's incorrect",
        "not quite,", "no, ", "wrong,", "wrong.",
        "let me correct", "i was wrong", "i meant ", "i misspoke",
        "you misunderstood",
    ] {
        p.push(("correction", *pat, "starts_with", "en"));
    }
    // Corrections (contains)
    for pat in &["that's wrong", "that's not right", "that's incorrect", "let me correct"] {
        p.push(("correction", *pat, "contains", "en"));
    }

    // Memory (starts_with)
    for pat in &[
        "remember ", "remember:", "remember,", "store:", "save this",
        "save that", "note:", "note that", "take note", "jot down",
        "keep in mind", "don't forget", "important:", "important,",
    ] {
        p.push(("memory", *pat, "starts_with", "en"));
    }
    for pat in &["запомни", "запомни,"] {
        p.push(("memory", *pat, "starts_with", "ru"));
    }

    // Instructions (starts_with)
    for pat in &[
        "i prefer ", "i like ", "i don't like ", "i dislike ", "i hate ",
        "i want ", "i need ", "i always ", "i never ",
        "always ", "never ", "from now on", "going forward",
        "in the future", "my preference is", "my style is",
    ] {
        p.push(("instruction", *pat, "starts_with", "en"));
    }
    for pat in &["я предпочитаю", "мне нравится"] {
        p.push(("instruction", *pat, "starts_with", "ru"));
    }

    p
}
