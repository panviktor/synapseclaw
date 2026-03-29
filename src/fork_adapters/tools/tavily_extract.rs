use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Tavily Extract tool — fetches and extracts content from URLs using the
/// Tavily Extract API. Returns clean markdown/text content from web pages.
pub struct TavilyExtractTool {
    boot_api_key: Option<String>,
    timeout_secs: u64,
    config_path: PathBuf,
    secrets_encrypt: bool,
}

impl TavilyExtractTool {
    pub fn new_with_config(
        api_key: Option<String>,
        timeout_secs: u64,
        config_path: PathBuf,
        secrets_encrypt: bool,
    ) -> Self {
        Self {
            boot_api_key: api_key,
            timeout_secs: timeout_secs.max(5),
            config_path,
            secrets_encrypt,
        }
    }

    fn resolve_api_key(&self) -> anyhow::Result<String> {
        if let Ok(key) = std::env::var("TAVILY_API_KEY") {
            if !key.is_empty() {
                return Ok(key);
            }
        }
        if let Some(ref key) = self.boot_api_key {
            if !key.is_empty() && !fork_security::SecretStore::is_encrypted(key) {
                return Ok(key.clone());
            }
        }
        let contents = std::fs::read_to_string(&self.config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read config for Tavily API key: {e}"))?;
        let config: fork_config::schema::Config = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("Failed to parse config for Tavily API key: {e}"))?;
        let raw_key = config
            .web_search
            .tavily_api_key
            .filter(|k| !k.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Tavily API key not configured"))?;
        if fork_security::SecretStore::is_encrypted(&raw_key) {
            let dir = self.config_path.parent().unwrap_or_else(|| Path::new("."));
            let store = fork_security::SecretStore::new(dir, self.secrets_encrypt);
            store.decrypt(&raw_key)
        } else {
            Ok(raw_key)
        }
    }
}

#[async_trait]
impl Tool for TavilyExtractTool {
    fn name(&self) -> &str {
        "tavily_extract"
    }

    fn description(&self) -> &str {
        "Extract clean content from web URLs using Tavily. \
         Returns page content in markdown format. Useful for reading articles, \
         documentation, or any web page content. Supports up to 20 URLs at once."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "urls": {
                    "type": "string",
                    "description": "URL or comma-separated list of URLs to extract content from (max 20)"
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "text"],
                    "description": "Output format (default: markdown)"
                }
            },
            "required": ["urls"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let urls_str = args
            .get("urls")
            .and_then(|u| u.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: urls"))?;

        let urls: Vec<&str> = urls_str
            .split(',')
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .take(20)
            .collect();

        if urls.is_empty() {
            anyhow::bail!("No valid URLs provided");
        }

        let format = args
            .get("format")
            .and_then(|f| f.as_str())
            .unwrap_or("markdown");

        let api_key = self.resolve_api_key()?;

        tracing::info!("Tavily extract: {} URL(s)", urls.len());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let body = json!({
            "urls": urls,
            "format": format,
            "extract_depth": "basic"
        });

        let response = client
            .post("https://api.tavily.com/extract")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            match status.as_u16() {
                401 => anyhow::bail!("Tavily: invalid API key"),
                429 => anyhow::bail!("Tavily: rate limit exceeded"),
                _ => anyhow::bail!("Tavily extract failed ({}): {}", status, text),
            }
        }

        let json: serde_json::Value = response.json().await?;
        let mut output = String::new();

        if let Some(results) = json.get("results").and_then(|r| r.as_array()) {
            for result in results {
                let url = result.get("url").and_then(|u| u.as_str()).unwrap_or("?");
                let content = result
                    .get("raw_content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");

                if !output.is_empty() {
                    output.push_str("\n---\n\n");
                }
                let _ = write!(output, "## {}\n\n{}", url, content);
            }
        }

        if let Some(failed) = json.get("failed_results").and_then(|f| f.as_array()) {
            for fail in failed {
                let url = fail.get("url").and_then(|u| u.as_str()).unwrap_or("?");
                let err = fail
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown");
                let _ = write!(output, "\n\n[Failed] {}: {}", url, err);
            }
        }

        if output.is_empty() {
            output = "No content extracted".to_string();
        }

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }
}
