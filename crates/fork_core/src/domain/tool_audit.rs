//! Run context — shared metadata for a single agent run invocation.
//!
//! Tracks tool execution events during the agent loop for audit,
//! safety-net decisions, and per-session IPC reply tracking.

use std::sync::Mutex;
use std::time::Instant;

/// Maximum number of tool events stored per run (defensive cap).
/// With `MAX_TOOL_ITERATIONS = 10` and ~3 tools per iteration, normal runs
/// produce ≤30 events.  256 is generous headroom without unbounded growth.
const MAX_TOOL_EVENTS: usize = 256;

/// Tools whose arguments are stored for per-session IPC reply tracking.
/// All other tools get `args: None` to avoid memory bloat.
const IPC_TOOLS: &[&str] = &["agents_reply", "agents_send"];

/// A recorded tool execution event.
#[derive(Debug, Clone)]
pub struct ToolEvent {
    pub tool_name: String,
    pub success: bool,
    pub timestamp: Instant,
    /// Arguments snapshot — only populated for IPC tools (see `IPC_TOOLS`).
    pub args: Option<serde_json::Value>,
}

/// Shared run context that accumulates signals during `agent::run()`.
///
/// Thread-safe: fields behind `Mutex` so tool executors on any task/thread
/// can record events concurrently.  If the mutex is poisoned (panic in
/// another thread), we recover the inner data rather than silently losing
/// events — a lost recording could cause a spurious auto-reply.
#[derive(Debug)]
pub struct RunContext {
    tool_events: Mutex<Vec<ToolEvent>>,
}

/// Recover a poisoned mutex instead of silently failing.
fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("RunContext mutex was poisoned, recovering");
        poisoned.into_inner()
    })
}

impl RunContext {
    /// Create a new empty run context.
    pub fn new() -> Self {
        Self {
            tool_events: Mutex::new(Vec::new()),
        }
    }

    /// Record that a tool was executed.
    ///
    /// `args` should be `Some(&call_arguments)` for IPC tools
    /// (`agents_reply`, `agents_send`) so the safety net can track
    /// per-session replies.  Pass `None` for all other tools.
    pub fn record_tool_call(
        &self,
        tool_name: &str,
        success: bool,
        args: Option<&serde_json::Value>,
    ) {
        let mut events = lock_or_recover(&self.tool_events);
        if events.len() >= MAX_TOOL_EVENTS {
            return; // defensive cap — should never be hit in practice
        }
        let stored_args = if IPC_TOOLS.contains(&tool_name) {
            args.cloned()
        } else {
            None
        };
        events.push(ToolEvent {
            tool_name: tool_name.to_string(),
            success,
            timestamp: Instant::now(),
            args: stored_args,
        });
    }

    /// Check whether a specific tool was called (regardless of success).
    pub fn was_tool_called(&self, tool_name: &str) -> bool {
        let events = lock_or_recover(&self.tool_events);
        events.iter().any(|e| e.tool_name == tool_name)
    }

    /// Check whether a specific tool was called **and succeeded**.
    pub fn was_tool_called_successfully(&self, tool_name: &str) -> bool {
        let events = lock_or_recover(&self.tool_events);
        events.iter().any(|e| e.tool_name == tool_name && e.success)
    }

    /// Check whether an IPC result was sent for a specific `session_id`.
    ///
    /// Returns `true` if:
    /// - `agents_reply` was called successfully with matching `session_id`, OR
    /// - `agents_send` was called successfully with `kind=result` and matching
    ///   `session_id`.
    pub fn was_ipc_reply_sent_for_session(&self, session_id: &str) -> bool {
        let events = lock_or_recover(&self.tool_events);
        events.iter().any(|e| {
            if !e.success {
                return false;
            }
            let args = match &e.args {
                Some(a) => a,
                None => return false,
            };
            if e.tool_name == "agents_reply" {
                args["session_id"].as_str() == Some(session_id)
            } else if e.tool_name == "agents_send" {
                args["kind"].as_str() == Some("result")
                    && args["session_id"].as_str() == Some(session_id)
            } else {
                false
            }
        })
    }

    /// Return a snapshot of all recorded tool events.
    pub fn tool_events(&self) -> Vec<ToolEvent> {
        lock_or_recover(&self.tool_events).clone()
    }
}

impl Default for RunContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_and_query_tool_calls() {
        let ctx = RunContext::new();

        assert!(!ctx.was_tool_called("agents_reply"));
        assert!(!ctx.was_tool_called_successfully("agents_reply"));

        ctx.record_tool_call("agents_send", true, Some(&json!({"to": "x"})));
        ctx.record_tool_call("agents_reply", false, Some(&json!({"session_id": "s1"})));

        assert!(ctx.was_tool_called("agents_reply"));
        assert!(!ctx.was_tool_called_successfully("agents_reply"));

        ctx.record_tool_call(
            "agents_reply",
            true,
            Some(&json!({"session_id": "s1", "to": "peer", "payload": "done"})),
        );
        assert!(ctx.was_tool_called_successfully("agents_reply"));

        assert_eq!(ctx.tool_events().len(), 3);
    }

    #[test]
    fn default_is_empty() {
        let ctx = RunContext::default();
        assert!(!ctx.was_tool_called("anything"));
        assert!(ctx.tool_events().is_empty());
    }

    #[test]
    fn per_session_reply_via_agents_reply() {
        let ctx = RunContext::new();
        ctx.record_tool_call(
            "agents_reply",
            true,
            Some(&json!({"to": "opus", "session_id": "s1", "payload": "result"})),
        );
        assert!(ctx.was_ipc_reply_sent_for_session("s1"));
        assert!(!ctx.was_ipc_reply_sent_for_session("s2"));
    }

    #[test]
    fn per_session_reply_via_agents_send_result() {
        let ctx = RunContext::new();
        ctx.record_tool_call(
            "agents_send",
            true,
            Some(&json!({"to": "opus", "kind": "result", "session_id": "s1", "payload": "done"})),
        );
        assert!(ctx.was_ipc_reply_sent_for_session("s1"));
        assert!(!ctx.was_ipc_reply_sent_for_session("s2"));
    }

    #[test]
    fn agents_send_text_not_counted_as_reply() {
        let ctx = RunContext::new();
        ctx.record_tool_call(
            "agents_send",
            true,
            Some(&json!({"to": "opus", "kind": "text", "session_id": "s1", "payload": "hi"})),
        );
        assert!(!ctx.was_ipc_reply_sent_for_session("s1"));
    }

    #[test]
    fn failed_reply_not_counted() {
        let ctx = RunContext::new();
        ctx.record_tool_call(
            "agents_reply",
            false,
            Some(&json!({"to": "opus", "session_id": "s1", "payload": "result"})),
        );
        assert!(!ctx.was_ipc_reply_sent_for_session("s1"));
    }

    #[test]
    fn non_ipc_tools_dont_store_args() {
        let ctx = RunContext::new();
        ctx.record_tool_call("shell", true, Some(&json!({"command": "ls"})));
        let events = ctx.tool_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].args.is_none());
    }

    #[test]
    fn cap_prevents_unbounded_growth() {
        let ctx = RunContext::new();
        for i in 0..MAX_TOOL_EVENTS + 50 {
            ctx.record_tool_call(&format!("tool_{i}"), true, None);
        }
        assert_eq!(ctx.tool_events().len(), MAX_TOOL_EVENTS);
    }
}
