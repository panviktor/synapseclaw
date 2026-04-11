use anyhow::Result;
use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use synapse_domain::ports::scoped_instruction_context::{
    ScopedInstructionContextPort, ScopedInstructionRequest, ScopedInstructionSnippet,
};

const SCOPED_INSTRUCTION_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

#[derive(Debug, Clone)]
struct CachedSnippet {
    modified_at: Option<SystemTime>,
    snippet: ScopedInstructionSnippet,
}

pub struct FilesystemScopedInstructionContext {
    workspace_dir: PathBuf,
    cache: RwLock<HashMap<String, CachedSnippet>>,
}

impl FilesystemScopedInstructionContext {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    fn discover_instruction_files(&self, hints: &[String], max_files: usize) -> Vec<PathBuf> {
        let mut discovered = Vec::new();
        let mut seen = HashSet::new();

        for hint in hints {
            let Some(found) = self.find_nearest_instruction_file(hint) else {
                continue;
            };
            if seen.insert(found.clone()) {
                discovered.push(found);
                if discovered.len() >= max_files.max(1) {
                    break;
                }
            }
        }

        discovered
    }

    fn find_nearest_instruction_file(&self, hint: &str) -> Option<PathBuf> {
        let hint_path = self.resolve_hint_path(hint);
        let mut dir = start_dir_for_hint(&hint_path);

        loop {
            for file_name in SCOPED_INSTRUCTION_FILES {
                let candidate = dir.join(file_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            if dir == self.workspace_dir {
                break;
            }
            if !dir.pop() || !dir.starts_with(&self.workspace_dir) {
                break;
            }
        }

        None
    }

    fn resolve_hint_path(&self, hint: &str) -> PathBuf {
        let path = PathBuf::from(hint);
        if path.is_absolute() {
            path
        } else {
            self.workspace_dir.join(path)
        }
    }

    fn cache_key(session_id: Option<&str>, file_path: &Path) -> String {
        format!("{}::{}", session_id.unwrap_or("_"), file_path.display())
    }

    fn read_cached_snippet(
        &self,
        session_id: Option<&str>,
        file_path: &Path,
    ) -> Option<ScopedInstructionSnippet> {
        let cache_key = Self::cache_key(session_id, file_path);
        let modified_at = file_path
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok());
        let cache = self.cache.read();
        let cached = cache.get(&cache_key)?;
        if cached.modified_at == modified_at {
            let mut snippet = cached.snippet.clone();
            snippet.cache_hit = true;
            Some(snippet)
        } else {
            None
        }
    }

    fn write_cached_snippet(
        &self,
        session_id: Option<&str>,
        file_path: &Path,
        snippet: &ScopedInstructionSnippet,
    ) {
        let cache_key = Self::cache_key(session_id, file_path);
        let modified_at = file_path
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok());
        self.cache.write().insert(
            cache_key,
            CachedSnippet {
                modified_at,
                snippet: snippet.clone(),
            },
        );
    }
}

#[async_trait]
impl ScopedInstructionContextPort for FilesystemScopedInstructionContext {
    async fn load_scoped_instructions(
        &self,
        request: ScopedInstructionRequest,
    ) -> Result<Vec<ScopedInstructionSnippet>> {
        let mut snippets = Vec::new();
        let mut remaining_chars = request.max_total_chars.max(1);
        let files = self.discover_instruction_files(&request.path_hints, request.max_files);

        for file_path in files {
            if let Some(snippet) =
                self.read_cached_snippet(request.session_id.as_deref(), &file_path)
            {
                let chars = snippet.content.chars().count();
                if chars <= remaining_chars {
                    remaining_chars = remaining_chars.saturating_sub(chars);
                    snippets.push(snippet);
                }
                continue;
            }

            let Ok(content) = std::fs::read_to_string(&file_path) else {
                continue;
            };
            let trimmed = truncate_chars(content.trim(), remaining_chars);
            if trimmed.is_empty() {
                continue;
            }
            let snippet = ScopedInstructionSnippet {
                scope_root: relative_display(
                    file_path.parent().unwrap_or(&self.workspace_dir),
                    &self.workspace_dir,
                ),
                source_file: relative_display(&file_path, &self.workspace_dir),
                content: trimmed.clone(),
                cache_hit: false,
            };
            remaining_chars = remaining_chars.saturating_sub(trimmed.chars().count());
            self.write_cached_snippet(request.session_id.as_deref(), &file_path, &snippet);
            snippets.push(snippet);
            if remaining_chars == 0 {
                break;
            }
        }

        Ok(snippets)
    }
}

fn start_dir_for_hint(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }

    if path.is_file() {
        return path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf());
    }

    if path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains('.'))
    {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn relative_display(path: &Path, workspace_dir: &Path) -> String {
    path.strip_prefix(workspace_dir)
        .ok()
        .and_then(|relative| {
            if relative.as_os_str().is_empty() {
                Some(".".to_string())
            } else {
                Some(relative.display().to_string())
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    value.chars().take(max_chars).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finds_nearest_scoped_instruction_file() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path();
        std::fs::create_dir_all(workspace.join("crates/domain/src")).unwrap();
        std::fs::write(workspace.join("AGENTS.md"), "root rules").unwrap();
        std::fs::write(workspace.join("crates/domain/CLAUDE.md"), "domain rules").unwrap();

        let loader = FilesystemScopedInstructionContext::new(workspace.to_path_buf());
        let snippets = loader
            .load_scoped_instructions(ScopedInstructionRequest {
                session_id: Some("scope-test".into()),
                path_hints: vec!["crates/domain/src/lib.rs".into()],
                max_files: 2,
                max_total_chars: 500,
            })
            .await
            .unwrap();

        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0].source_file, "crates/domain/CLAUDE.md");
        assert!(snippets[0].content.contains("domain rules"));
    }

    #[tokio::test]
    async fn returns_cache_hit_on_repeated_session_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path();
        std::fs::create_dir_all(workspace.join("docs/fork")).unwrap();
        std::fs::write(workspace.join("docs/AGENTS.md"), "docs rules").unwrap();

        let loader = FilesystemScopedInstructionContext::new(workspace.to_path_buf());
        let request = ScopedInstructionRequest {
            session_id: Some("cache-test".into()),
            path_hints: vec!["docs/fork/ipc-phase4_10-plan.md".into()],
            max_files: 1,
            max_total_chars: 200,
        };

        let first = loader
            .load_scoped_instructions(request.clone())
            .await
            .unwrap();
        let second = loader.load_scoped_instructions(request).await.unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert!(!first[0].cache_hit);
        assert!(second[0].cache_hit);
    }
}
