use super::traits::{Tool, ToolResult};
use crate::tool_fact_helpers::is_low_signal_bootstrap_path;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use synapse_domain::domain::security_policy::SecurityPolicy;
use synapse_domain::domain::tool_fact::{
    ResourceFact, ResourceKind, ResourceMetadata, ResourceOperation, ToolFactPayload, TypedToolFact,
};

const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Read file contents with path sandboxing
pub struct FileReadTool {
    security: Arc<SecurityPolicy>,
}

impl FileReadTool {
    pub fn new(security: Arc<SecurityPolicy>) -> Self {
        Self { security }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read file contents with line numbers. Supports partial reading via offset and limit. Extracts text from PDF; other binary files are read with lossy UTF-8 conversion."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file. Relative paths resolve from workspace; outside paths require policy allowlist."
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting line number (1-based, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return (default: all)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;

        if self.security.is_rate_limited() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: too many actions in the last hour".into()),
            });
        }

        // Security check: validate path is within workspace
        if !self.security.is_path_allowed(path) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Path not allowed by security policy: {path}")),
            });
        }

        // Record action BEFORE canonicalization so that every non-trivially-rejected
        // request consumes rate limit budget. This prevents attackers from probing
        // path existence (via canonicalize errors) without rate limit cost.
        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Rate limit exceeded: action budget exhausted".into()),
            });
        }

        let full_path = self.security.resolve_tool_path(path);

        // Resolve path before reading to block symlink escapes.
        let resolved_path = match tokio::fs::canonicalize(&full_path).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to resolve file path: {e}")),
                });
            }
        };

        if !self.security.is_resolved_path_allowed(&resolved_path) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    self.security
                        .resolved_path_violation_message(&resolved_path),
                ),
            });
        }

        // Check file size AFTER canonicalization to prevent TOCTOU symlink bypass
        match tokio::fs::metadata(&resolved_path).await {
            Ok(meta) => {
                if meta.len() > MAX_FILE_SIZE_BYTES {
                    return Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!(
                            "File too large: {} bytes (limit: {MAX_FILE_SIZE_BYTES} bytes)",
                            meta.len()
                        )),
                    });
                }
            }
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to read file metadata: {e}")),
                });
            }
        }

        match tokio::fs::read_to_string(&resolved_path).await {
            Ok(contents) => {
                let lines: Vec<&str> = contents.lines().collect();
                let total = lines.len();

                if total == 0 {
                    return Ok(ToolResult {
                        success: true,
                        output: String::new(),
                        error: None,
                    });
                }

                let offset = args
                    .get("offset")
                    .and_then(|v| v.as_u64())
                    .map(|v| {
                        usize::try_from(v.max(1))
                            .unwrap_or(usize::MAX)
                            .saturating_sub(1)
                    })
                    .unwrap_or(0);
                let start = offset.min(total);

                let end = match args.get("limit").and_then(|v| v.as_u64()) {
                    Some(l) => {
                        let limit = usize::try_from(l).unwrap_or(usize::MAX);
                        (start.saturating_add(limit)).min(total)
                    }
                    None => total,
                };

                if start >= end {
                    return Ok(ToolResult {
                        success: true,
                        output: format!("[No lines in range, file has {total} lines]"),
                        error: None,
                    });
                }

                let numbered: String = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {}", start + i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                let partial = start > 0 || end < total;
                let summary = if partial {
                    format!("\n[Lines {}-{} of {total}]", start + 1, end)
                } else {
                    format!("\n[{total} lines total]")
                };

                Ok(ToolResult {
                    success: true,
                    output: format!("{numbered}{summary}"),
                    error: None,
                })
            }
            Err(_) => {
                // Not valid UTF-8 — read raw bytes and try to extract text
                let bytes = tokio::fs::read(&resolved_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to read file: {e}"))?;

                if let Some(text) = try_extract_pdf_text(&bytes) {
                    return Ok(ToolResult {
                        success: true,
                        output: text,
                        error: None,
                    });
                }

                // Lossy fallback — replaces invalid bytes with U+FFFD
                let lossy = String::from_utf8_lossy(&bytes).into_owned();
                Ok(ToolResult {
                    success: true,
                    output: lossy,
                    error: None,
                })
            }
        }
    }

    fn extract_facts(
        &self,
        args: &serde_json::Value,
        result: Option<&ToolResult>,
    ) -> Vec<TypedToolFact> {
        if matches!(result, Some(result) if !result.success) {
            return Vec::new();
        }

        let path = match args.get("path").and_then(|value| value.as_str()) {
            Some(path) if !path.trim().is_empty() => path,
            _ => return Vec::new(),
        };

        if is_low_signal_bootstrap_path(path) {
            return Vec::new();
        }

        vec![TypedToolFact {
            tool_id: self.name().to_string(),
            payload: ToolFactPayload::Resource(ResourceFact {
                kind: ResourceKind::File,
                operation: ResourceOperation::Read,
                locator: path.to_string(),
                host: None,
                metadata: ResourceMetadata::default(),
            }),
        }]
    }
}

#[cfg(feature = "rag-pdf")]
fn try_extract_pdf_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 5 || &bytes[..5] != b"%PDF-" {
        return None;
    }
    let text = pdf_extract::extract_text_from_mem(bytes).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

#[cfg(not(feature = "rag-pdf"))]
fn try_extract_pdf_text(_bytes: &[u8]) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::domain::config::AutonomyLevel;
    use synapse_domain::domain::security_policy::SecurityPolicy;

    fn test_security(workspace: std::path::PathBuf) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: workspace,
            ..SecurityPolicy::default()
        })
    }

    fn test_security_with(
        workspace: std::path::PathBuf,
        autonomy: AutonomyLevel,
        max_actions_per_hour: u32,
    ) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy,
            workspace_dir: workspace,
            max_actions_per_hour,
            ..SecurityPolicy::default()
        })
    }

    #[test]
    fn file_read_name() {
        let tool = FileReadTool::new(test_security(std::env::temp_dir()));
        assert_eq!(tool.name(), "file_read");
    }

    #[test]
    fn file_read_schema_has_path() {
        let tool = FileReadTool::new(test_security(std::env::temp_dir()));
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["path"].is_object());
        assert!(schema["properties"]["offset"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("path")));
        // offset and limit are optional
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("offset")));
    }

    #[tokio::test]
    async fn file_read_existing_file() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("test.txt"), "hello world")
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "test.txt"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("1: hello world"));
        assert!(result.output.contains("[1 lines total]"));
        assert!(result.error.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn extract_facts_emits_file_resource() {
        let tool = FileReadTool::new(test_security(std::env::temp_dir()));
        let facts = tool.extract_facts(
            &json!({"path": "notes/todo.txt", "offset": 10, "limit": 20}),
            Some(&ToolResult {
                success: true,
                output: String::new(),
                error: None,
            }),
        );

        assert_eq!(facts.len(), 1);
        let projected = facts[0].projected_focus_entities();
        assert_eq!(projected[0].kind, "file_resource");
        assert_eq!(projected[0].name, "notes/todo.txt");
        assert_eq!(projected[0].metadata.as_deref(), Some("read"));
        assert!(facts[0]
            .projected_subjects()
            .iter()
            .any(|subject| subject == "notes/todo.txt"));
    }

    #[test]
    fn extract_facts_skips_low_signal_bootstrap_files() {
        let tool = FileReadTool::new(test_security(std::env::temp_dir()));
        let facts = tool.extract_facts(
            &json!({"path": "SOUL.md"}),
            Some(&ToolResult {
                success: true,
                output: String::new(),
                error: None,
            }),
        );

        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn file_read_nonexistent_file() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_missing");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "nope.txt"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Failed to resolve"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_blocks_path_traversal() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_traversal");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "../../../etc/passwd"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("not allowed"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_blocks_absolute_path() {
        let tool = FileReadTool::new(test_security(std::env::temp_dir()));
        let result = tool.execute(json!({"path": "/etc/passwd"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("not allowed"));
    }

    #[tokio::test]
    async fn file_read_blocks_when_rate_limited() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_rate_limited");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("test.txt"), "hello world")
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security_with(
            dir.clone(),
            AutonomyLevel::Supervised,
            0,
        ));
        let result = tool.execute(json!({"path": "test.txt"})).await.unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Rate limit exceeded"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_allows_readonly_mode() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_readonly");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("test.txt"), "readonly ok")
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security_with(dir.clone(), AutonomyLevel::ReadOnly, 20));
        let result = tool.execute(json!({"path": "test.txt"})).await.unwrap();

        assert!(result.success);
        assert!(result.output.contains("1: readonly ok"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_missing_path_param() {
        let tool = FileReadTool::new(test_security(std::env::temp_dir()));
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_read_empty_file() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_empty");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("empty.txt"), "").await.unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "empty.txt"})).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_nested_path() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_nested");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("sub/dir"))
            .await
            .unwrap();
        tokio::fs::write(dir.join("sub/dir/deep.txt"), "deep content")
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "sub/dir/deep.txt"}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("1: deep content"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_read_blocks_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join("synapseclaw_test_file_read_symlink_escape");
        let workspace = root.join("workspace");
        let outside = root.join("outside");

        let _ = tokio::fs::remove_dir_all(&root).await;
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();

        tokio::fs::write(outside.join("secret.txt"), "outside workspace")
            .await
            .unwrap();

        symlink(outside.join("secret.txt"), workspace.join("escape.txt")).unwrap();

        let tool = FileReadTool::new(test_security(workspace.clone()));
        let result = tool.execute(json!({"path": "escape.txt"})).await.unwrap();

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("escapes workspace"));

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn file_read_outside_workspace_allowed_when_workspace_only_disabled() {
        let root = std::env::temp_dir().join("synapseclaw_test_file_read_allowed_roots_hint");
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        let outside_file = outside.join("notes.txt");

        let _ = tokio::fs::remove_dir_all(&root).await;
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        tokio::fs::write(&outside_file, "outside").await.unwrap();

        let security = Arc::new(SecurityPolicy {
            autonomy: AutonomyLevel::Supervised,
            workspace_dir: workspace,
            workspace_only: false,
            forbidden_paths: vec![],
            ..SecurityPolicy::default()
        });
        let tool = FileReadTool::new(security);

        let result = tool
            .execute(json!({"path": outside_file.to_string_lossy().to_string()}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.error.is_none());
        assert!(result.output.contains("outside"));

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn file_read_nonexistent_consumes_rate_limit_budget() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_probe");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Allow only 2 actions total
        let tool = FileReadTool::new(test_security_with(
            dir.clone(),
            AutonomyLevel::Supervised,
            2,
        ));

        // Both reads fail (file doesn't exist) but should consume budget
        let r1 = tool.execute(json!({"path": "nope1.txt"})).await.unwrap();
        assert!(!r1.success);
        assert!(r1.error.as_ref().unwrap().contains("Failed to resolve"));

        let r2 = tool.execute(json!({"path": "nope2.txt"})).await.unwrap();
        assert!(!r2.success);
        assert!(r2.error.as_ref().unwrap().contains("Failed to resolve"));

        // Third attempt should be rate limited even though file doesn't exist
        let r3 = tool.execute(json!({"path": "nope3.txt"})).await.unwrap();
        assert!(!r3.success);
        assert!(
            r3.error.as_ref().unwrap().contains("Rate limit"),
            "Expected rate limit error, got: {:?}",
            r3.error
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_with_offset_and_limit() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_offset");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("lines.txt"), "aaa\nbbb\nccc\nddd\neee")
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));

        // Read lines 2-3
        let result = tool
            .execute(json!({"path": "lines.txt", "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("2: bbb"));
        assert!(result.output.contains("3: ccc"));
        assert!(!result.output.contains("1: aaa"));
        assert!(!result.output.contains("4: ddd"));
        assert!(result.output.contains("[Lines 2-3 of 5]"));

        // Read from offset 4 to end
        let result = tool
            .execute(json!({"path": "lines.txt", "offset": 4}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("4: ddd"));
        assert!(result.output.contains("5: eee"));
        assert!(result.output.contains("[Lines 4-5 of 5]"));

        // Limit only (first 2 lines)
        let result = tool
            .execute(json!({"path": "lines.txt", "limit": 2}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("1: aaa"));
        assert!(result.output.contains("2: bbb"));
        assert!(!result.output.contains("3: ccc"));
        assert!(result.output.contains("[Lines 1-2 of 5]"));

        // Full read (no offset/limit) shows all lines
        let result = tool.execute(json!({"path": "lines.txt"})).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("1: aaa"));
        assert!(result.output.contains("5: eee"));
        assert!(result.output.contains("[5 lines total]"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_offset_beyond_end() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_offset_end");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("short.txt"), "one\ntwo")
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool
            .execute(json!({"path": "short.txt", "offset": 100}))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result
            .output
            .contains("[No lines in range, file has 2 lines]"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn file_read_rejects_oversized_file() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_large");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Create a file just over 10 MB
        let big = vec![b'x'; 10 * 1024 * 1024 + 1];
        tokio::fs::write(dir.join("huge.bin"), &big).await.unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "huge.bin"})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("File too large"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// PDF files should be readable via pdf-extract text extraction.
    #[tokio::test]
    async fn file_read_extracts_pdf_text() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_pdf");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/test_document.pdf");
        tokio::fs::copy(&fixture, dir.join("report.pdf"))
            .await
            .expect("copy PDF fixture");

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "report.pdf"})).await.unwrap();

        assert!(
            result.success,
            "PDF read must succeed, error: {:?}",
            result.error
        );
        assert!(
            result.output.contains("Hello"),
            "extracted text must contain 'Hello', got: {}",
            result.output
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// Non-UTF-8 binary files should be read with lossy conversion.
    #[tokio::test]
    async fn file_read_lossy_reads_binary_file() {
        let dir = std::env::temp_dir().join("synapseclaw_test_file_read_lossy");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();

        // Write bytes that are not valid UTF-8 and not a PDF
        let binary_data: Vec<u8> = vec![0x00, 0x80, 0xFF, 0xFE, b'h', b'i', 0x80];
        tokio::fs::write(dir.join("data.bin"), &binary_data)
            .await
            .unwrap();

        let tool = FileReadTool::new(test_security(dir.clone()));
        let result = tool.execute(json!({"path": "data.bin"})).await.unwrap();

        assert!(
            result.success,
            "lossy read must succeed, error: {:?}",
            result.error
        );
        assert!(
            result.output.contains('\u{FFFD}'),
            "lossy output must contain replacement character, got: {:?}",
            result.output
        );
        assert!(
            result.output.contains("hi"),
            "lossy output must preserve valid ASCII, got: {:?}",
            result.output
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
