//! Shared tool-notification mapping for runtime adapters.
//!
//! Web and channel transports intentionally render different payloads, but the
//! observer-event interpretation and preview shaping should not fork per adapter.

use serde_json::json;
use synapse_domain::domain::util::truncate_with_ellipsis;
use synapse_observability::traits::ObserverEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeToolNotification {
    CallStart {
        tool: String,
        arguments: Option<String>,
    },
    Result {
        tool: String,
        output: String,
        success: bool,
    },
}

impl RuntimeToolNotification {
    pub(crate) fn from_observer_event(event: &ObserverEvent) -> Option<Self> {
        match event {
            ObserverEvent::ToolCallStart { tool, arguments } => Some(Self::CallStart {
                tool: tool.clone(),
                arguments: arguments.clone(),
            }),
            ObserverEvent::ToolResult {
                tool,
                output,
                success,
            } => Some(Self::Result {
                tool: tool.clone(),
                output: output.clone(),
                success: *success,
            }),
            _ => None,
        }
    }

    pub(crate) fn marks_tool_used(&self) -> bool {
        matches!(self, Self::CallStart { .. })
    }

    pub(crate) fn channel_text(&self) -> String {
        match self {
            Self::CallStart { tool, arguments } => {
                let detail = channel_argument_detail(arguments.as_deref());
                format!("\u{1F527} `{tool}`{detail}")
            }
            Self::Result {
                tool,
                output,
                success,
            } => {
                let status = if *success { "\u{2705}" } else { "\u{274C}" };
                let preview = truncate_with_ellipsis(output, 200);
                format!("{status} `{tool}`: {preview}")
            }
        }
    }

    pub(crate) fn web_dedupe_key(&self) -> String {
        match self {
            Self::CallStart { .. } => self.web_content(),
            Self::Result { tool, .. } => format!("{tool}:{}", self.web_content()),
        }
    }

    pub(crate) fn web_json(&self, session_key: &str, timestamp: i64) -> String {
        match self {
            Self::CallStart { tool, .. } => json!({
                "type": "tool_call",
                "session_key": session_key,
                "tool_name": tool,
                "content": self.web_content(),
                "timestamp": timestamp,
            })
            .to_string(),
            Self::Result { .. } => json!({
                "type": "tool_result",
                "session_key": session_key,
                "content": self.web_content(),
                "timestamp": timestamp,
            })
            .to_string(),
        }
    }

    fn web_content(&self) -> String {
        match self {
            Self::CallStart { tool, arguments } => match arguments {
                Some(args) => format!("{tool}({args})"),
                None => tool.clone(),
            },
            Self::Result {
                output, success, ..
            } => {
                if *success {
                    truncate_web_tool_result(output, 500)
                } else {
                    format!("Error: {}", truncate_web_tool_result(output, 500))
                }
            }
        }
    }
}

fn channel_argument_detail(arguments: Option<&str>) -> String {
    let Some(args) = arguments.filter(|args| !args.is_empty()) else {
        return String::new();
    };
    // User-facing preview only; this must never drive routing or tool decisions.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(command) = value.get("command").and_then(|value| value.as_str()) {
            format!(": `{}`", truncate_with_ellipsis(command, 200))
        } else if let Some(query) = value.get("query").and_then(|value| value.as_str()) {
            format!(": {}", truncate_with_ellipsis(query, 200))
        } else if let Some(path) = value.get("path").and_then(|value| value.as_str()) {
            format!(": {path}")
        } else if let Some(url) = value.get("url").and_then(|value| value.as_str()) {
            format!(": {url}")
        } else {
            format!(": {}", truncate_with_ellipsis(args, 120))
        }
    } else {
        format!(": {}", truncate_with_ellipsis(args, 120))
    }
}

fn truncate_web_tool_result(value: &str, max_wire_len: usize) -> String {
    if value.len() <= max_wire_len {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_wire_len.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_text_preserves_command_preview() {
        let event = ObserverEvent::ToolCallStart {
            tool: "shell".to_string(),
            arguments: Some(serde_json::json!({"command": "printf ok"}).to_string()),
        };
        let notification = RuntimeToolNotification::from_observer_event(&event).unwrap();

        assert!(notification.marks_tool_used());
        assert_eq!(
            notification.channel_text(),
            "\u{1F527} `shell`: `printf ok`"
        );
    }

    #[test]
    fn channel_text_truncates_utf8_arguments_safely() {
        let payload = (0..300)
            .map(|n| serde_json::json!({ "content": format!("{}置tail", "a".repeat(n)) }))
            .map(|value| value.to_string())
            .find(|raw| raw.len() > 120 && !raw.is_char_boundary(120))
            .expect("should produce non-char-boundary data at byte index 120");
        let notification = RuntimeToolNotification::CallStart {
            tool: "file_write".to_string(),
            arguments: Some(payload),
        };

        let emitted = notification.channel_text();

        assert!(emitted.contains("`file_write`"));
        assert!(emitted.is_char_boundary(emitted.len()));
    }

    #[test]
    fn web_json_preserves_existing_tool_call_shape() {
        let notification = RuntimeToolNotification::CallStart {
            tool: "search".to_string(),
            arguments: Some(serde_json::json!({"query": "context budget"}).to_string()),
        };

        let value: serde_json::Value =
            serde_json::from_str(&notification.web_json("session-1", 42)).unwrap();

        assert_eq!(value["type"], "tool_call");
        assert_eq!(value["session_key"], "session-1");
        assert_eq!(value["tool_name"], "search");
        assert_eq!(value["timestamp"], 42);
        assert_eq!(value["content"], "search({\"query\":\"context budget\"})");
    }

    #[test]
    fn web_json_preserves_existing_tool_result_shape() {
        let notification = RuntimeToolNotification::Result {
            tool: "shell".to_string(),
            output: "boom".to_string(),
            success: false,
        };

        let value: serde_json::Value =
            serde_json::from_str(&notification.web_json("session-1", 42)).unwrap();

        assert_eq!(value["type"], "tool_result");
        assert_eq!(value["session_key"], "session-1");
        assert_eq!(value["timestamp"], 42);
        assert_eq!(value["content"], "Error: boom");
        assert_eq!(notification.web_dedupe_key(), "shell:Error: boom");
    }
}
