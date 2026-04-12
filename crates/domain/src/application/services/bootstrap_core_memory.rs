//! Bootstrap core memory blocks from workspace files.
//!
//! On agent startup, ensures `user_knowledge` and other critical core blocks
//! are seeded from USER.md / SOUL.md if they exist but blocks are empty.
//! Idempotent: skips if blocks already have content.

use crate::ports::memory::UnifiedMemoryPort;

/// Labels that can be bootstrapped from workspace files.
const BOOTSTRAP_LABELS: &[(&str, &str)] = &[("user_knowledge", "USER.md"), ("persona", "SOUL.md")];

/// Default placeholder when no workspace file exists.
const USER_KNOWLEDGE_PLACEHOLDER: &str =
    "No user information recorded yet. The agent should learn and store \
     durable user facts as arbitrary memory/profile facts as they become known; \
     do not assume a fixed user-profile field schema.";

/// Ensure critical core blocks are seeded from workspace files.
///
/// For each label in `BOOTSTRAP_LABELS`:
/// - If the block exists and has content → skip
/// - If workspace file content is provided → seed from it
/// - Otherwise → seed with placeholder
///
/// Called on every agent startup (all 3 paths: CLI, gateway, channels).
pub async fn ensure_core_blocks_seeded(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    workspace_files: &[(&str, Option<&str>)],
) {
    let existing = mem
        .get_core_blocks(&agent_id.to_string())
        .await
        .unwrap_or_default();

    for (label, _default_file) in BOOTSTRAP_LABELS {
        // Check if block already has content
        let has_content = existing
            .iter()
            .any(|b| b.label == *label && !b.content.trim().is_empty());

        if has_content {
            tracing::debug!(
                target: "bootstrap",
                label,
                "Core block already populated, skipping"
            );
            continue;
        }

        // Find content from workspace files
        let content = workspace_files
            .iter()
            .find(|(file, _)| {
                BOOTSTRAP_LABELS
                    .iter()
                    .any(|(l, f)| l == label && *f == *file)
            })
            .and_then(|(_, content)| *content);

        let seed_content = if let Some(text) = content {
            if text.trim().is_empty() {
                if *label == "user_knowledge" {
                    USER_KNOWLEDGE_PLACEHOLDER
                } else {
                    continue; // No content and no placeholder for this label
                }
            } else {
                text
            }
        } else if *label == "user_knowledge" {
            USER_KNOWLEDGE_PLACEHOLDER
        } else {
            continue;
        };

        // Truncate to reasonable size for core blocks
        let truncated = if seed_content.chars().count() > 2000 {
            let t: String = seed_content.chars().take(2000).collect();
            format!("{t}...")
        } else {
            seed_content.to_string()
        };

        if let Err(e) = mem
            .update_core_block(&agent_id.to_string(), label, truncated.clone())
            .await
        {
            tracing::warn!(
                target: "bootstrap",
                label,
                error = %e,
                "Failed to seed core block"
            );
        } else {
            tracing::info!(
                target: "bootstrap",
                label,
                chars = truncated.chars().count(),
                "Core block seeded"
            );
        }
    }
}

/// Helper: read a file from workspace dir, return content or None.
pub fn read_workspace_file(workspace_dir: &std::path::Path, filename: &str) -> Option<String> {
    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => Some(content),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_non_empty() {
        assert!(!USER_KNOWLEDGE_PLACEHOLDER.is_empty());
        assert!(USER_KNOWLEDGE_PLACEHOLDER.len() < 500);
    }

    #[test]
    fn bootstrap_labels_have_files() {
        for (label, file) in BOOTSTRAP_LABELS {
            assert!(!label.is_empty());
            assert!(file.ends_with(".md"));
        }
    }
}
