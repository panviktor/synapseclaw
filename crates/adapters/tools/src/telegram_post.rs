use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use synapse_domain::domain::security_policy::SecurityPolicy;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org/bot";
const TELEGRAM_REQUEST_TIMEOUT_SECS: u64 = 15;

/// Tool for proactively posting messages to Telegram chats and channels.
///
/// Credentials are resolved in order:
/// 1. `TELEGRAM_BOT_TOKEN` environment variable (set via systemd env file)
/// 2. `TELEGRAM_BOT_TOKEN` in workspace `.env` file
pub struct TelegramPostTool {
    security: Arc<SecurityPolicy>,
    workspace_dir: PathBuf,
}

impl TelegramPostTool {
    pub fn new(security: Arc<SecurityPolicy>, workspace_dir: PathBuf) -> Self {
        Self {
            security,
            workspace_dir,
        }
    }

    fn parse_env_value(raw: &str) -> String {
        let raw = raw.trim();
        let unquoted = if raw.len() >= 2
            && ((raw.starts_with('"') && raw.ends_with('"'))
                || (raw.starts_with('\'') && raw.ends_with('\'')))
        {
            &raw[1..raw.len() - 1]
        } else {
            raw
        };
        unquoted.split_once(" #").map_or_else(
            || unquoted.trim().to_string(),
            |(v, _)| v.trim().to_string(),
        )
    }

    async fn get_bot_token(&self) -> anyhow::Result<String> {
        // 1. Try process environment (from systemd EnvironmentFile)
        if let Ok(token) = std::env::var("TELEGRAM_BOT_TOKEN") {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }

        // 2. Fall back to workspace .env file
        let env_path = self.workspace_dir.join(".env");
        let content = tokio::fs::read_to_string(&env_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", env_path.display(), e))?;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let line = line.strip_prefix("export ").map(str::trim).unwrap_or(line);
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().eq_ignore_ascii_case("TELEGRAM_BOT_TOKEN") {
                    let token = Self::parse_env_value(value);
                    if !token.is_empty() {
                        return Ok(token);
                    }
                }
            }
        }

        anyhow::bail!("TELEGRAM_BOT_TOKEN not found in environment or .env file")
    }
}

#[async_trait]
impl Tool for TelegramPostTool {
    fn name(&self) -> &str {
        "telegram_post"
    }

    fn description(&self) -> &str {
        "Send a message to a Telegram chat or channel. \
         Use this to publish posts, announcements, or notifications. \
         Requires TELEGRAM_BOT_TOKEN in environment. \
         The bot must be an admin of the target chat/channel."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "chat_id": {
                    "type": "string",
                    "description": "Target chat ID or channel username (e.g., '@synapseclaw' or '-1001234567890')"
                },
                "text": {
                    "type": "string",
                    "description": "Message text to send"
                },
                "parse_mode": {
                    "type": "string",
                    "enum": ["HTML", "Markdown", "MarkdownV2"],
                    "description": "Text formatting mode (default: Markdown)"
                },
                "disable_web_page_preview": {
                    "type": "boolean",
                    "description": "Disable link previews (default: false)"
                }
            },
            "required": ["chat_id", "text"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.security.can_act() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: autonomy is read-only".into()),
            });
        }

        if !self.security.record_action() {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("Action blocked: rate limit exceeded".into()),
            });
        }

        let chat_id = args
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'chat_id' parameter"))?;

        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;

        let parse_mode = args
            .get("parse_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("Markdown");

        let disable_preview = args
            .get("disable_web_page_preview")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let token = self.get_bot_token().await?;
        let api_url = format!("{}{}/sendMessage", TELEGRAM_API_BASE, token);

        let mut body = json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": parse_mode,
        });

        if disable_preview {
            body["disable_web_page_preview"] = json!(true);
        }

        tracing::info!("Telegram post to {}", chat_id);

        let client = synapse_providers::proxy::build_runtime_proxy_client_with_timeouts(
            "tool.telegram_post",
            TELEGRAM_REQUEST_TIMEOUT_SECS,
            10,
        );
        let response = client.post(&api_url).json(&body).send().await?;

        let status = response.status();
        let response_body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Ok(ToolResult {
                success: false,
                output: response_body,
                error: Some(format!("Telegram API returned status {}", status)),
            });
        }

        let api_ok = serde_json::from_str::<serde_json::Value>(&response_body)
            .ok()
            .and_then(|json| json.get("ok").and_then(|v| v.as_bool()))
            .unwrap_or(false);

        if api_ok {
            let message_id = serde_json::from_str::<serde_json::Value>(&response_body)
                .ok()
                .and_then(|json| {
                    json.get("result")
                        .and_then(|r| r.get("message_id"))
                        .and_then(|v| v.as_i64())
                });
            let output = match message_id {
                Some(id) => format!("Message sent to {} (message_id: {})", chat_id, id),
                None => format!("Message sent to {}", chat_id),
            };
            Ok(ToolResult {
                success: true,
                output,
                error: None,
            })
        } else {
            Ok(ToolResult {
                success: false,
                output: response_body,
                error: Some("Telegram API returned an error".into()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use synapse_domain::domain::config::AutonomyLevel;
    use tempfile::TempDir;

    fn test_security(level: AutonomyLevel, max_actions_per_hour: u32) -> Arc<SecurityPolicy> {
        Arc::new(SecurityPolicy {
            autonomy: level,
            max_actions_per_hour,
            workspace_dir: std::env::temp_dir(),
            ..SecurityPolicy::default()
        })
    }

    #[test]
    fn tool_name() {
        let tool = TelegramPostTool::new(
            test_security(AutonomyLevel::Full, 100),
            PathBuf::from("/tmp"),
        );
        assert_eq!(tool.name(), "telegram_post");
    }

    #[test]
    fn tool_has_parameters_schema() {
        let tool = TelegramPostTool::new(
            test_security(AutonomyLevel::Full, 100),
            PathBuf::from("/tmp"),
        );
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].get("chat_id").is_some());
        assert!(schema["properties"].get("text").is_some());
    }

    #[tokio::test]
    async fn token_from_env_file() {
        let tmp = TempDir::new().unwrap();
        let env_path = tmp.path().join(".env");
        fs::write(&env_path, "TELEGRAM_BOT_TOKEN=test123\n").unwrap();

        // Clear process env to force .env fallback
        std::env::remove_var("TELEGRAM_BOT_TOKEN");

        let tool = TelegramPostTool::new(
            test_security(AutonomyLevel::Full, 100),
            tmp.path().to_path_buf(),
        );
        let token = tool.get_bot_token().await.unwrap();
        assert_eq!(token, "test123");
    }

    #[tokio::test]
    async fn token_fails_without_config() {
        let tmp = TempDir::new().unwrap();
        std::env::remove_var("TELEGRAM_BOT_TOKEN");

        let tool = TelegramPostTool::new(
            test_security(AutonomyLevel::Full, 100),
            tmp.path().to_path_buf(),
        );
        assert!(tool.get_bot_token().await.is_err());
    }

    #[tokio::test]
    async fn execute_blocks_readonly() {
        let tool = TelegramPostTool::new(
            test_security(AutonomyLevel::ReadOnly, 100),
            PathBuf::from("/tmp"),
        );
        let result = tool
            .execute(json!({"chat_id": "@test", "text": "hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("read-only"));
    }

    #[tokio::test]
    async fn execute_blocks_rate_limit() {
        let tool =
            TelegramPostTool::new(test_security(AutonomyLevel::Full, 0), PathBuf::from("/tmp"));
        let result = tool
            .execute(json!({"chat_id": "@test", "text": "hello"}))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("rate limit"));
    }

    #[tokio::test]
    async fn token_supports_quoted_values() {
        let tmp = TempDir::new().unwrap();
        let env_path = tmp.path().join(".env");
        fs::write(
            &env_path,
            "export TELEGRAM_BOT_TOKEN=\"quoted-token-123\"\n",
        )
        .unwrap();

        std::env::remove_var("TELEGRAM_BOT_TOKEN");

        let tool = TelegramPostTool::new(
            test_security(AutonomyLevel::Full, 100),
            tmp.path().to_path_buf(),
        );
        let token = tool.get_bot_token().await.unwrap();
        assert_eq!(token, "quoted-token-123");
    }
}
