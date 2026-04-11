use super::{
    autosave_memory_key, load_interactive_session_history, save_interactive_session_history,
    InteractiveSessionState,
};
use synapse_providers::ChatMessage;
use tempfile::tempdir;

#[test]
fn interactive_session_state_round_trips_history() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.json");
    let history = vec![
        ChatMessage::system("system"),
        ChatMessage::user("hello"),
        ChatMessage::assistant("hi"),
    ];

    save_interactive_session_history(&path, &history).unwrap();
    let restored = load_interactive_session_history(&path, "fallback").unwrap();

    assert_eq!(restored.len(), 3);
    assert_eq!(restored[0].role, "system");
    assert_eq!(restored[1].content, "hello");
    assert_eq!(restored[2].content, "hi");
}

#[test]
fn interactive_session_state_adds_missing_system_prompt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.json");
    let payload = serde_json::to_string_pretty(&InteractiveSessionState {
        version: 1,
        history: vec![ChatMessage::user("orphan")],
    })
    .unwrap();
    std::fs::write(&path, payload).unwrap();

    let restored = load_interactive_session_history(&path, "fallback system").unwrap();

    assert_eq!(restored[0].role, "system");
    assert_eq!(restored[0].content, "fallback system");
    assert_eq!(restored[1].content, "orphan");
}

use super::*;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn scrub_credentials_redacts_bearer_token() {
    let input = "API_KEY=sk-1234567890abcdef; token: 1234567890; password=\"secret123456\"";
    let scrubbed = scrub_credentials(input);
    assert!(scrubbed.contains("API_KEY=sk-1*[REDACTED]"));
    assert!(scrubbed.contains("token: 1234*[REDACTED]"));
    assert!(scrubbed.contains("password=\"secr*[REDACTED]\""));
    assert!(!scrubbed.contains("abcdef"));
    assert!(!scrubbed.contains("secret123456"));
}

#[test]
fn scrub_credentials_redacts_json_api_key() {
    let input = r#"{"api_key": "sk-1234567890", "other": "public"}"#;
    let scrubbed = scrub_credentials(input);
    assert!(scrubbed.contains("\"api_key\": \"sk-1*[REDACTED]\""));
    assert!(scrubbed.contains("public"));
}

#[tokio::test]
async fn execute_one_tool_does_not_panic_on_utf8_boundary() {
    let call_arguments = (0..600)
        .map(|n| serde_json::json!({ "content": format!("{}：tail", "a".repeat(n)) }))
        .find(|args| {
            let raw = args.to_string();
            raw.len() > 300 && !raw.is_char_boundary(300)
        })
        .expect("should produce a sample whose byte index 300 is not a char boundary");

    let observer = NoopObserver;
    let result = execute_one_tool(
        "unknown_tool",
        call_arguments,
        &[],
        None,
        &observer,
        None,
        None,
        None, // tool_middleware
    )
    .await;
    assert!(result.is_ok(), "execute_one_tool should not panic or error");

    let outcome = result.unwrap();
    assert!(!outcome.success);
    assert!(outcome.output.contains("Unknown tool: unknown_tool"));
}

#[tokio::test]
async fn execute_one_tool_resolves_unique_activated_tool_suffix() {
    let observer = NoopObserver;
    let invocations = Arc::new(AtomicUsize::new(0));
    let activated = Arc::new(std::sync::Mutex::new(crate::tools::ActivatedToolSet::new()));
    let activated_tool: Arc<dyn Tool> = Arc::new(CountingTool::new(
        "docker-mcp__extract_text",
        Arc::clone(&invocations),
    ));
    activated
        .lock()
        .unwrap()
        .activate("docker-mcp__extract_text".into(), activated_tool);

    let outcome = execute_one_tool(
        "extract_text",
        serde_json::json!({ "value": "ok" }),
        &[],
        Some(&activated),
        &observer,
        None,
        None,
        None, // tool_middleware
    )
    .await
    .expect("suffix alias should execute the unique activated tool");

    assert!(outcome.success);
    assert_eq!(outcome.output, "counted:ok");
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

use synapse_observability::NoopObserver;
use synapse_providers::traits::ProviderCapabilities;
use synapse_providers::ChatResponse;
use tempfile::TempDir;

struct NonVisionProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for NonVisionProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("ok".to_string())
    }
}

struct VisionProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for VisionProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            native_tool_calling: false,
            vision: true,
            prompt_caching: false,
        }
    }

    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("ok".to_string())
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let marker_count = synapse_providers::multimodal::count_image_markers(request.messages);
        if marker_count == 0 {
            anyhow::bail!("expected image markers in request messages");
        }

        if request.tools.is_some() {
            anyhow::bail!("no tools should be attached for this test");
        }

        Ok(ChatResponse {
            text: Some("vision-ok".to_string()),
            tool_calls: Vec::new(),
            usage: None,
            reasoning_content: None,
        })
    }
}

struct ScriptedProvider {
    responses: Arc<Mutex<VecDeque<ChatResponse>>>,
    capabilities: ProviderCapabilities,
}

impl ScriptedProvider {
    fn from_chat_responses(responses: Vec<ChatResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            capabilities: ProviderCapabilities::default(),
        }
    }

    fn from_text_responses(responses: Vec<&str>) -> Self {
        let scripted = responses
            .into_iter()
            .map(|text| ChatResponse {
                text: Some(text.to_string()),
                tool_calls: Vec::new(),
                usage: None,
                reasoning_content: None,
            })
            .collect();
        Self {
            responses: Arc::new(Mutex::new(scripted)),
            capabilities: ProviderCapabilities::default(),
        }
    }

    fn with_native_tool_support(mut self) -> Self {
        self.capabilities.native_tool_calling = true;
        self
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        anyhow::bail!("chat_with_system should not be used in scripted provider tests");
    }

    async fn chat(
        &self,
        _request: ChatRequest<'_>,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        let mut responses = self
            .responses
            .lock()
            .expect("responses lock should be valid");
        responses
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("scripted provider exhausted responses"))
    }
}

struct CountingTool {
    name: String,
    invocations: Arc<AtomicUsize>,
}

impl CountingTool {
    fn new(name: &str, invocations: Arc<AtomicUsize>) -> Self {
        Self {
            name: name.to_string(),
            invocations,
        }
    }
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Counts executions for loop-stability tests"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let value = args
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        Ok(crate::tools::ToolResult {
            success: true,
            output: format!("counted:{value}"),
            error: None,
        })
    }
}

struct DelayTool {
    name: String,
    delay_ms: u64,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

impl DelayTool {
    fn new(
        name: &str,
        delay_ms: u64,
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            name: name.to_string(),
            delay_ms,
            active,
            max_active,
        }
    }
}

#[async_trait]
impl Tool for DelayTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Delay tool for testing parallel tool execution"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            },
            "required": ["value"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);

        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;

        self.active.fetch_sub(1, Ordering::SeqCst);

        let value = args
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(crate::tools::ToolResult {
            success: true,
            output: format!("ok:{value}"),
            error: None,
        })
    }
}

/// A tool that always returns a failure with a given error reason.
struct FailingTool {
    tool_name: String,
    error_reason: String,
}

impl FailingTool {
    fn new(name: &str, error_reason: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            error_reason: error_reason.to_string(),
        }
    }
}

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        "A tool that always fails for testing failure surfacing"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            }
        })
    }

    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
        Ok(crate::tools::ToolResult {
            success: false,
            output: String::new(),
            error: Some(self.error_reason.clone()),
        })
    }
}

#[tokio::test]
async fn run_tool_call_loop_returns_structured_error_for_non_vision_provider() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = NonVisionProvider {
        calls: Arc::clone(&calls),
    };

    let mut history = vec![ChatMessage::user(
        "please inspect [IMAGE:data:image/png;base64,iVBORw0KGgo=]".to_string(),
    )];
    let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
    let observer = NoopObserver;

    let err = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        3,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect_err("provider without vision support should fail");

    assert!(err.to_string().contains("provider_capability_error"));
    assert!(err.to_string().contains("capability=vision"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn run_tool_call_loop_rejects_oversized_image_payload() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = VisionProvider {
        calls: Arc::clone(&calls),
    };

    let oversized_payload = STANDARD.encode(vec![0_u8; (1024 * 1024) + 1]);
    let mut history = vec![ChatMessage::user(format!(
        "[IMAGE:data:image/png;base64,{oversized_payload}]"
    ))];

    let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
    let observer = NoopObserver;
    let multimodal = synapse_domain::config::schema::MultimodalConfig {
        max_images: 4,
        max_image_size_mb: 1,
        allow_remote_fetch: false,
    };

    let err = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &multimodal,
        3,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect_err("oversized payload must fail");

    assert!(err
        .to_string()
        .contains("multimodal image size limit exceeded"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn run_tool_call_loop_accepts_valid_multimodal_request_flow() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = VisionProvider {
        calls: Arc::clone(&calls),
    };

    let mut history = vec![ChatMessage::user(
        "Analyze this [IMAGE:data:image/png;base64,iVBORw0KGgo=]".to_string(),
    )];
    let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
    let observer = NoopObserver;

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        3,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect("valid multimodal payload should pass");

    assert_eq!(result.response, "vision-ok");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn should_execute_tools_in_parallel_returns_false_for_single_call() {
    let calls = vec![ParsedToolCall {
        name: "file_read".to_string(),
        arguments: serde_json::json!({"path": "a.txt"}),
        tool_call_id: None,
    }];

    assert!(!should_execute_tools_in_parallel(&calls, None));
}

#[test]
fn should_execute_tools_in_parallel_returns_false_when_approval_is_required() {
    let calls = vec![
        ParsedToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "pwd"}),
            tool_call_id: None,
        },
        ParsedToolCall {
            name: "http_request".to_string(),
            arguments: serde_json::json!({"url": "https://example.com"}),
            tool_call_id: None,
        },
    ];
    let approval_cfg = synapse_domain::config::schema::AutonomyConfig::default();
    let approval_mgr = ApprovalManager::from_config(&approval_cfg);

    assert!(!should_execute_tools_in_parallel(
        &calls,
        Some(&approval_mgr)
    ));
}

#[test]
fn should_execute_tools_in_parallel_returns_true_when_cli_has_no_interactive_approvals() {
    let calls = vec![
        ParsedToolCall {
            name: "shell".to_string(),
            arguments: serde_json::json!({"command": "pwd"}),
            tool_call_id: None,
        },
        ParsedToolCall {
            name: "http_request".to_string(),
            arguments: serde_json::json!({"url": "https://example.com"}),
            tool_call_id: None,
        },
    ];
    let approval_cfg = synapse_domain::config::schema::AutonomyConfig {
        level: synapse_security::AutonomyLevel::Full,
        ..synapse_domain::config::schema::AutonomyConfig::default()
    };
    let approval_mgr = ApprovalManager::from_config(&approval_cfg);

    assert!(should_execute_tools_in_parallel(
        &calls,
        Some(&approval_mgr)
    ));
}

#[tokio::test]
async fn run_tool_call_loop_executes_multiple_tools_with_ordered_results() {
    let provider = ScriptedProvider::from_text_responses(vec![
        r#"<tool_call>
{"name":"delay_a","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"delay_b","arguments":{"value":"B"}}
</tool_call>"#,
        "done",
    ]);

    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let tools_registry: Vec<Box<dyn Tool>> = vec![
        Box::new(DelayTool::new(
            "delay_a",
            200,
            Arc::clone(&active),
            Arc::clone(&max_active),
        )),
        Box::new(DelayTool::new(
            "delay_b",
            200,
            Arc::clone(&active),
            Arc::clone(&max_active),
        )),
    ];

    let approval_cfg = synapse_domain::config::schema::AutonomyConfig {
        level: synapse_security::AutonomyLevel::Full,
        ..synapse_domain::config::schema::AutonomyConfig::default()
    };
    let approval_mgr = ApprovalManager::from_config(&approval_cfg);

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("run tool calls"),
    ];
    let observer = NoopObserver;

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        Some(&approval_mgr),
        "telegram",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect("parallel execution should complete");

    assert_eq!(result.response, "done");
    assert!(
        max_active.load(Ordering::SeqCst) >= 1,
        "tools should execute successfully"
    );

    let tool_results_message = history
        .iter()
        .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
        .expect("tool results message should be present");
    let idx_a = tool_results_message
        .content
        .find("name=\"delay_a\"")
        .expect("delay_a result should be present");
    let idx_b = tool_results_message
        .content
        .find("name=\"delay_b\"")
        .expect("delay_b result should be present");
    assert!(
        idx_a < idx_b,
        "tool results should preserve input order for tool call mapping"
    );
}

#[tokio::test]
async fn run_tool_call_loop_deduplicates_repeated_tool_calls() {
    let provider = ScriptedProvider::from_text_responses(vec![
        r#"<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>"#,
        "done",
    ]);

    let invocations = Arc::new(AtomicUsize::new(0));
    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(CountingTool::new(
        "count_tool",
        Arc::clone(&invocations),
    ))];

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("run tool calls"),
    ];
    let observer = NoopObserver;

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect("loop should finish after deduplicating repeated calls");

    assert_eq!(result.response, "done");
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        1,
        "duplicate tool call with same args should not execute twice"
    );

    let tool_results = history
        .iter()
        .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
        .expect("prompt-mode tool result payload should be present");
    assert!(tool_results.content.contains("counted:A"));
    assert!(tool_results.content.contains("Skipped duplicate tool call"));
}

#[tokio::test]
async fn run_tool_call_loop_allows_low_risk_shell_in_non_interactive_mode() {
    let provider = ScriptedProvider::from_text_responses(vec![
        r#"<tool_call>
{"name":"shell","arguments":{"command":"echo hello"}}
</tool_call>"#,
        "done",
    ]);

    let tmp = TempDir::new().expect("temp dir");
    let security = Arc::new(synapse_security::SecurityPolicy {
        autonomy: synapse_security::AutonomyLevel::Supervised,
        workspace_dir: tmp.path().to_path_buf(),
        ..synapse_security::SecurityPolicy::default()
    });
    let runtime: Arc<dyn synapse_domain::ports::runtime::RuntimeAdapter> =
        Arc::new(crate::runtime::native::NativeRuntime::new());
    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(crate::tools::shell::ShellTool::new(
        security, runtime,
    ))];

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("run shell"),
    ];
    let observer = NoopObserver;
    let approval_mgr = ApprovalManager::for_non_interactive(
        &synapse_domain::config::schema::AutonomyConfig::default(),
    );

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        Some(&approval_mgr),
        "telegram",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect("non-interactive shell should succeed for low-risk command");

    assert_eq!(result.response, "done");

    let tool_results = history
        .iter()
        .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
        .expect("tool results message should be present");
    assert!(tool_results.content.contains("hello"));
    assert!(!tool_results.content.contains("Denied by user."));
}

#[tokio::test]
async fn run_tool_call_loop_dedup_exempt_allows_repeated_calls() {
    let provider = ScriptedProvider::from_text_responses(vec![
        r#"<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>"#,
        "done",
    ]);

    let invocations = Arc::new(AtomicUsize::new(0));
    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(CountingTool::new(
        "count_tool",
        Arc::clone(&invocations),
    ))];

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("run tool calls"),
    ];
    let observer = NoopObserver;
    let exempt = vec!["count_tool".to_string()];

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        None,
        None,
        &[],
        &exempt,
        None,
        None,
    )
    .await
    .expect("loop should finish with exempt tool executing twice");

    assert_eq!(result.response, "done");
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        2,
        "exempt tool should execute both duplicate calls"
    );

    let tool_results = history
        .iter()
        .find(|msg| msg.role == "user" && msg.content.starts_with("[Tool results]"))
        .expect("prompt-mode tool result payload should be present");
    assert!(
        !tool_results.content.contains("Skipped duplicate tool call"),
        "exempt tool calls should not be suppressed"
    );
}

#[tokio::test]
async fn run_tool_call_loop_dedup_exempt_only_affects_listed_tools() {
    let provider = ScriptedProvider::from_text_responses(vec![
        r#"<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"count_tool","arguments":{"value":"A"}}
</tool_call>
<tool_call>
{"name":"other_tool","arguments":{"value":"B"}}
</tool_call>
<tool_call>
{"name":"other_tool","arguments":{"value":"B"}}
</tool_call>"#,
        "done",
    ]);

    let count_invocations = Arc::new(AtomicUsize::new(0));
    let other_invocations = Arc::new(AtomicUsize::new(0));
    let tools_registry: Vec<Box<dyn Tool>> = vec![
        Box::new(CountingTool::new(
            "count_tool",
            Arc::clone(&count_invocations),
        )),
        Box::new(CountingTool::new(
            "other_tool",
            Arc::clone(&other_invocations),
        )),
    ];

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("run tool calls"),
    ];
    let observer = NoopObserver;
    let exempt = vec!["count_tool".to_string()];

    let _result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        None,
        None,
        &[],
        &exempt,
        None,
        None,
    )
    .await
    .expect("loop should complete");

    assert_eq!(
        count_invocations.load(Ordering::SeqCst),
        2,
        "exempt tool should execute both calls"
    );
    assert_eq!(
        other_invocations.load(Ordering::SeqCst),
        1,
        "non-exempt tool should still be deduped"
    );
}

#[tokio::test]
async fn run_tool_call_loop_native_mode_preserves_structured_tool_call_ids() {
    let provider = ScriptedProvider::from_chat_responses(vec![
        ChatResponse {
            text: Some("Need to call tool".into()),
            tool_calls: vec![ToolCall {
                id: "call_abc".into(),
                name: "count_tool".into(),
                arguments: r#"{"value":"X"}"#.into(),
            }],
            usage: None,
            reasoning_content: None,
        },
        ChatResponse {
            text: Some("done".into()),
            tool_calls: Vec::new(),
            usage: None,
            reasoning_content: None,
        },
    ])
    .with_native_tool_support();

    let invocations = Arc::new(AtomicUsize::new(0));
    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(CountingTool::new(
        "count_tool",
        Arc::clone(&invocations),
    ))];

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("run tool calls"),
    ];
    let observer = NoopObserver;

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "cli",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        None,
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect("native tool-call id flow should complete");

    assert_eq!(result.response, "done");
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert!(
        history.iter().any(|msg| {
            msg.role == "tool" && msg.content.contains("\"tool_call_id\":\"call_abc\"")
        }),
        "tool result should preserve parsed fallback tool_call_id in native mode"
    );
    assert!(
        history
            .iter()
            .all(|msg| !(msg.role == "user" && msg.content.starts_with("[Tool results]"))),
        "native mode should use role=tool history instead of prompt fallback wrapper"
    );
}

#[test]
fn agent_turn_executes_activated_tool_from_wrapper() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");

    runtime.block_on(async {
        let provider = ScriptedProvider::from_text_responses(vec![
            r#"<tool_call>
{"name":"pixel__get_api_health","arguments":{"value":"ok"}}
</tool_call>"#,
            "done",
        ]);

        let invocations = Arc::new(AtomicUsize::new(0));
        let activated = Arc::new(std::sync::Mutex::new(crate::tools::ActivatedToolSet::new()));
        let activated_tool: Arc<dyn Tool> = Arc::new(CountingTool::new(
            "pixel__get_api_health",
            Arc::clone(&invocations),
        ));
        activated
            .lock()
            .unwrap()
            .activate("pixel__get_api_health".into(), activated_tool);

        let tools_registry: Vec<Box<dyn Tool>> = Vec::new();
        let mut history = vec![
            ChatMessage::system("test-system"),
            ChatMessage::user("use the activated MCP tool"),
        ];
        let observer = NoopObserver;

        let result = agent_turn(
            &provider,
            &mut history,
            &tools_registry,
            &observer,
            "mock-provider",
            "mock-model",
            0.0,
            true,
            "daemon",
            &synapse_domain::config::schema::MultimodalConfig::default(),
            4,
            &[],
            &[],
            Some(&activated),
        )
        .await
        .expect("wrapper path should execute activated tools");

        assert_eq!(result, "done");
        assert_eq!(invocations.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn resolve_display_text_hides_raw_payload_for_tool_only_turns() {
    let display = resolve_display_text(
        "<tool_call>{\"name\":\"memory_store\"}</tool_call>",
        "",
        true,
    );
    assert!(display.is_empty());
}

#[test]
fn resolve_display_text_keeps_plain_text_for_tool_turns() {
    let display = resolve_display_text(
        "<tool_call>{\"name\":\"shell\"}</tool_call>",
        "Let me check that.",
        true,
    );
    assert_eq!(display, "Let me check that.");
}

#[test]
fn resolve_display_text_uses_response_text_for_final_turns() {
    let display = resolve_display_text("Final answer", "", false);
    assert_eq!(display, "Final answer");
}

#[test]
fn parse_tool_calls_extracts_single_call() {
    let response = r#"Let me check that.
<tool_call>
{"name": "shell", "arguments": {"command": "ls -la"}}
</tool_call>"#;

    let (text, calls) = parse_tool_calls(response);
    assert_eq!(text, "Let me check that.");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "shell");
    assert_eq!(
        calls[0].arguments.get("command").unwrap().as_str().unwrap(),
        "ls -la"
    );
}

#[test]
fn parse_tool_calls_extracts_multiple_calls() {
    let response = r#"<tool_call>
{"name": "file_read", "arguments": {"path": "a.txt"}}
</tool_call>
<tool_call>
{"name": "file_read", "arguments": {"path": "b.txt"}}
</tool_call>"#;

    let (_, calls) = parse_tool_calls(response);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "file_read");
    assert_eq!(calls[1].name, "file_read");
}

#[test]
fn parse_tool_calls_returns_text_only_when_no_calls() {
    let response = "Just a normal response with no tools.";
    let (text, calls) = parse_tool_calls(response);
    assert_eq!(text, "Just a normal response with no tools.");
    assert!(calls.is_empty());
}

#[test]
fn parse_tool_calls_handles_malformed_json() {
    let response = r#"<tool_call>
not valid json
</tool_call>
Some text after."#;

    let (text, calls) = parse_tool_calls(response);
    assert!(calls.is_empty());
    assert!(text.contains("Some text after."));
}

#[test]
fn parse_tool_calls_text_before_and_after() {
    let response = r#"Before text.
<tool_call>
{"name": "shell", "arguments": {"command": "echo hi"}}
</tool_call>
After text."#;

    let (text, calls) = parse_tool_calls(response);
    assert!(text.contains("Before text."));
    assert!(text.contains("After text."));
    assert_eq!(calls.len(), 1);
}

#[test]
fn parse_tool_calls_rejects_raw_openai_format_without_tags() {
    // Provider adapters must expose native tool calls through LlmResponse.tool_calls.
    // The shared text fallback intentionally rejects bare OpenAI-shaped JSON.
    let response = r#"{"content": "Let me check that for you.", "tool_calls": [{"type": "function", "function": {"name": "shell", "arguments": "{\"command\": \"ls -la\"}"}}]}"#;

    let (text, calls) = parse_tool_calls(response);
    assert_eq!(text, response);
    assert!(calls.is_empty());
}

#[test]
fn parse_tool_calls_rejects_raw_openai_format_multiple_calls_without_tags() {
    let response = r#"{"tool_calls": [{"type": "function", "function": {"name": "file_read", "arguments": "{\"path\": \"a.txt\"}"}}, {"type": "function", "function": {"name": "file_read", "arguments": "{\"path\": \"b.txt\"}"}}]}"#;

    let (text, calls) = parse_tool_calls(response);
    assert_eq!(text, response);
    assert!(calls.is_empty());
}

#[test]
fn parse_tool_calls_rejects_raw_canonical_format_without_tags() {
    let response = r#"{"tool_calls": [{"name": "memory_recall", "arguments": "{}"}]}"#;

    let (text, calls) = parse_tool_calls(response);
    assert_eq!(text, response);
    assert!(calls.is_empty());
}

#[test]
fn parse_tool_calls_preserves_canonical_tool_call_ids_inside_tags() {
    let response = r#"<tool_call>{"tool_calls":[{"id":"call_42","name":"shell","arguments":"{\"command\":\"pwd\"}"}]}</tool_call>"#;
    let (_, calls) = parse_tool_calls(response);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tool_call_id.as_deref(), Some("call_42"));
}

#[test]
fn parse_tool_calls_handles_markdown_json_inside_tool_call_tag() {
    let response = r#"<tool_call>
```json
{"name": "file_write", "arguments": {"path": "test.py", "content": "print('ok')"}}
```
</tool_call>"#;

    let (text, calls) = parse_tool_calls(response);
    assert!(text.is_empty());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "file_write");
    assert_eq!(
        calls[0].arguments.get("path").unwrap().as_str().unwrap(),
        "test.py"
    );
}

#[test]
fn parse_tool_calls_handles_noisy_tool_call_tag_body() {
    let response = r#"<tool_call>
I will now call the tool with this payload:
{"name": "shell", "arguments": {"command": "pwd"}}
</tool_call>"#;

    let (text, calls) = parse_tool_calls(response);
    assert!(text.is_empty());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "shell");
    assert_eq!(
        calls[0].arguments.get("command").unwrap().as_str().unwrap(),
        "pwd"
    );
}

#[test]
fn parse_tool_calls_rejects_raw_tool_json_without_tags() {
    // SECURITY: Raw JSON without explicit wrappers should NOT be parsed
    // This prevents prompt injection attacks where malicious content
    // could include JSON that mimics a tool call.
    let response = r#"Sure, creating the file now.
{"name": "file_write", "arguments": {"path": "hello.py", "content": "print('hello')"}}"#;

    let (text, calls) = parse_tool_calls(response);
    assert!(text.contains("Sure, creating the file now."));
    assert_eq!(
        calls.len(),
        0,
        "Raw JSON without wrappers should not be parsed"
    );
}

#[test]
fn build_tool_instructions_includes_all_tools() {
    use synapse_security::security_policy_from_config;
    let security = Arc::new(security_policy_from_config(
        &synapse_domain::config::schema::AutonomyConfig::default(),
        std::path::Path::new("/tmp"),
    ));
    let tools = tools::default_tools(security);
    let instructions = build_tool_instructions(&tools);

    assert!(instructions.contains("## Tool Use Protocol"));
    assert!(instructions.contains("<tool_call>"));
    assert!(instructions.contains("shell"));
    assert!(instructions.contains("file_read"));
    assert!(instructions.contains("file_write"));
}

#[test]
fn tools_to_openai_format_produces_valid_schema() {
    use synapse_security::security_policy_from_config;
    let security = Arc::new(security_policy_from_config(
        &synapse_domain::config::schema::AutonomyConfig::default(),
        std::path::Path::new("/tmp"),
    ));
    let tools = tools::default_tools(security);
    let formatted = tools_to_openai_format(&tools);

    assert!(!formatted.is_empty());
    for tool_json in &formatted {
        assert_eq!(tool_json["type"], "function");
        assert!(tool_json["function"]["name"].is_string());
        assert!(tool_json["function"]["description"].is_string());
        assert!(!tool_json["function"]["name"].as_str().unwrap().is_empty());
    }
    // Verify known tools are present
    let names: Vec<&str> = formatted
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"shell"));
    assert!(names.contains(&"file_read"));
}

#[test]
fn trim_history_preserves_system_prompt() {
    let mut history = vec![ChatMessage::system("system prompt")];
    for i in 0..DEFAULT_MAX_HISTORY_MESSAGES + 20 {
        history.push(ChatMessage::user(format!("msg {i}")));
    }
    let original_len = history.len();
    assert!(original_len > DEFAULT_MAX_HISTORY_MESSAGES + 1);

    trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);

    // System prompt preserved
    assert_eq!(history[0].role, "system");
    assert_eq!(history[0].content, "system prompt");
    // Trimmed to limit
    assert_eq!(history.len(), DEFAULT_MAX_HISTORY_MESSAGES + 1); // +1 for system
                                                                 // Most recent messages preserved
    let last = &history[history.len() - 1];
    assert_eq!(
        last.content,
        format!("msg {}", DEFAULT_MAX_HISTORY_MESSAGES + 19)
    );
}

#[test]
fn trim_history_noop_when_within_limit() {
    let mut history = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("hello"),
        ChatMessage::assistant("hi"),
    ];
    trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
    assert_eq!(history.len(), 3);
}

// NOTE: build_compaction_transcript and apply_compaction tests moved to
// synapse_domain::application::services::history_compaction::tests (Phase 6D).

#[test]
fn autosave_memory_key_has_prefix_and_uniqueness() {
    let key1 = autosave_memory_key("user_msg");
    let key2 = autosave_memory_key("user_msg");

    assert!(key1.starts_with("user_msg_"));
    assert!(key2.starts_with("user_msg_"));
    assert_ne!(key1, key2);
}

// Unit tests use NoopUnifiedMemory; real store/recall tested in tests/integration/memory_restart.rs.

#[tokio::test]
async fn autosave_memory_keys_preserve_multiple_turns() {
    // Verify unique keys are generated per call
    let key1 = autosave_memory_key("user_msg");
    let key2 = autosave_memory_key("user_msg");
    assert_ne!(key1, key2, "autosave keys must be unique per call");
}

#[tokio::test]
async fn build_context_ignores_legacy_assistant_autosave_entries() {
    let mem = synapse_memory::NoopUnifiedMemory;
    // NoopUnifiedMemory returns empty recall, so context should be empty
    let context = build_context(&mem, "status updates", 0.0, None, "default").await;
    assert!(context.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// Recovery Tests - Tool Call Parsing Edge Cases
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn parse_tool_calls_handles_empty_tool_result() {
    // Recovery: Empty tool_result tag should be handled gracefully
    let response = r#"I'll run that command.
<tool_result name="shell">

</tool_result>
Done."#;
    let (text, calls) = parse_tool_calls(response);
    assert!(text.contains("Done."));
    assert!(calls.is_empty());
}

#[test]
fn strip_tool_result_blocks_removes_single_block() {
    let input = r#"<tool_result name="memory_recall" status="ok">
{"matches":["hello"]}
</tool_result>
Here is my answer."#;
    assert_eq!(strip_tool_result_blocks(input), "Here is my answer.");
}

#[test]
fn strip_tool_result_blocks_removes_multiple_blocks() {
    let input = r#"<tool_result name="memory_recall" status="ok">
{"matches":[]}
</tool_result>
<tool_result name="shell" status="ok">
done
</tool_result>
Final answer."#;
    assert_eq!(strip_tool_result_blocks(input), "Final answer.");
}

#[test]
fn strip_tool_result_blocks_removes_prefix() {
    let input =
        "[Tool results]\n<tool_result name=\"shell\" status=\"ok\">\nok\n</tool_result>\nDone.";
    assert_eq!(strip_tool_result_blocks(input), "Done.");
}

#[test]
fn strip_tool_result_blocks_removes_thinking() {
    let input = "<thinking>\nLet me think...\n</thinking>\nHere is the answer.";
    assert_eq!(strip_tool_result_blocks(input), "Here is the answer.");
}

#[test]
fn strip_tool_result_blocks_removes_think_tags() {
    let input = "<think>\nLet me reason...\n</think>\nHere is the answer.";
    assert_eq!(strip_tool_result_blocks(input), "Here is the answer.");
}

#[test]
fn strip_think_tags_removes_single_block() {
    assert_eq!(strip_think_tags("<think>reasoning</think>Hello"), "Hello");
}

#[test]
fn strip_think_tags_removes_multiple_blocks() {
    assert_eq!(strip_think_tags("<think>a</think>X<think>b</think>Y"), "XY");
}

#[test]
fn strip_think_tags_handles_unclosed_block() {
    assert_eq!(strip_think_tags("visible<think>hidden"), "visible");
}

#[test]
fn strip_think_tags_preserves_text_without_tags() {
    assert_eq!(strip_think_tags("plain text"), "plain text");
}

#[test]
fn parse_tool_calls_strips_think_before_tool_call() {
    // Qwen regression: <think> tags before <tool_call> tags should be
    // stripped, allowing the tool call to be parsed correctly.
    let response = "<think>I need to list files to understand the project</think>\n<tool_call>\n{\"name\":\"shell\",\"arguments\":{\"command\":\"ls\"}}\n</tool_call>";
    let (text, calls) = parse_tool_calls(response);
    assert_eq!(
        calls.len(),
        1,
        "should parse tool call after stripping think tags"
    );
    assert_eq!(calls[0].name, "shell");
    assert_eq!(
        calls[0].arguments.get("command").unwrap().as_str().unwrap(),
        "ls"
    );
    assert!(text.is_empty(), "think content should not appear as text");
}

#[test]
fn parse_tool_calls_strips_think_only_returns_empty() {
    // When response is only <think> tags with no tool calls, should
    // return empty text and no calls.
    let response = "<think>Just thinking, no action needed</think>";
    let (text, calls) = parse_tool_calls(response);
    assert!(calls.is_empty());
    assert!(text.is_empty());
}

#[test]
fn parse_tool_calls_handles_qwen_think_with_multiple_tool_calls() {
    let response = "<think>I need to check two things</think>\n<tool_call>\n{\"name\":\"shell\",\"arguments\":{\"command\":\"date\"}}\n</tool_call>\n<tool_call>\n{\"name\":\"shell\",\"arguments\":{\"command\":\"pwd\"}}\n</tool_call>";
    let (_, calls) = parse_tool_calls(response);
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].arguments.get("command").unwrap().as_str().unwrap(),
        "date"
    );
    assert_eq!(
        calls[1].arguments.get("command").unwrap().as_str().unwrap(),
        "pwd"
    );
}

#[test]
fn strip_tool_result_blocks_preserves_clean_text() {
    let input = "Hello, this is a normal response.";
    assert_eq!(strip_tool_result_blocks(input), input);
}

#[test]
fn strip_tool_result_blocks_returns_empty_for_only_tags() {
    let input = "<tool_result name=\"memory_recall\" status=\"ok\">\n{}\n</tool_result>";
    assert_eq!(strip_tool_result_blocks(input), "");
}

#[test]
fn parse_arguments_value_handles_null() {
    // Recovery: null arguments are returned as-is (Value::Null)
    let value = serde_json::json!(null);
    let result = parse_arguments_value(Some(&value));
    assert!(result.is_null());
}

#[test]
fn parse_tool_calls_handles_empty_tool_calls_array() {
    // Recovery: Empty tool_calls array returns original response (no tool parsing)
    let response = r#"{"content": "Hello", "tool_calls": []}"#;
    let (text, calls) = parse_tool_calls(response);
    // When tool_calls is empty, the entire JSON is returned as text
    assert!(text.contains("Hello"));
    assert!(calls.is_empty());
}

#[test]
fn detect_tool_call_parse_issue_flags_malformed_payloads() {
    let response = "<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"pwd\"}</tool_call>";
    let issue = detect_tool_call_parse_issue(response, &[]);
    assert!(
        issue.is_some(),
        "malformed tool payload should be flagged for diagnostics"
    );
}

#[test]
fn detect_tool_call_parse_issue_ignores_normal_text() {
    let issue = detect_tool_call_parse_issue("Thanks, done.", &[]);
    assert!(issue.is_none());
}

#[test]
fn parse_tool_calls_handles_whitespace_only_name() {
    // Recovery: Whitespace-only tool name should return None
    let value = serde_json::json!({"function": {"name": "   ", "arguments": {}}});
    let result = parse_tool_call_value(&value);
    assert!(result.is_none());
}

#[test]
fn parse_tool_calls_handles_empty_string_arguments() {
    // Recovery: Empty string arguments should be handled
    let value = serde_json::json!({"name": "test", "arguments": ""});
    let result = parse_tool_call_value(&value);
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "test");
}

// ═══════════════════════════════════════════════════════════════════════
// Recovery Tests - History Management
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn trim_history_with_no_system_prompt() {
    // Recovery: History without system prompt should trim correctly
    let mut history = vec![];
    for i in 0..DEFAULT_MAX_HISTORY_MESSAGES + 20 {
        history.push(ChatMessage::user(format!("msg {i}")));
    }
    trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
    assert_eq!(history.len(), DEFAULT_MAX_HISTORY_MESSAGES);
}

#[test]
fn trim_history_preserves_role_ordering() {
    // Recovery: After trimming, role ordering should remain consistent
    let mut history = vec![ChatMessage::system("system")];
    for i in 0..DEFAULT_MAX_HISTORY_MESSAGES + 10 {
        history.push(ChatMessage::user(format!("user {i}")));
        history.push(ChatMessage::assistant(format!("assistant {i}")));
    }
    trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[history.len() - 1].role, "assistant");
}

#[test]
fn trim_history_with_only_system_prompt() {
    // Recovery: Only system prompt should not be trimmed
    let mut history = vec![ChatMessage::system("system prompt")];
    trim_history(&mut history, DEFAULT_MAX_HISTORY_MESSAGES);
    assert_eq!(history.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// Recovery Tests - Arguments Parsing
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn parse_arguments_value_handles_invalid_json_string() {
    // Recovery: Invalid JSON string should return empty object
    let value = serde_json::Value::String("not valid json".to_string());
    let result = parse_arguments_value(Some(&value));
    assert!(result.is_object());
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn parse_arguments_value_handles_none() {
    // Recovery: None arguments should return empty object
    let result = parse_arguments_value(None);
    assert!(result.is_object());
    assert!(result.as_object().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// Recovery Tests - JSON Extraction
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn extract_json_values_handles_empty_string() {
    // Recovery: Empty input should return empty vec
    let result = extract_json_values("");
    assert!(result.is_empty());
}

#[test]
fn extract_json_values_handles_whitespace_only() {
    // Recovery: Whitespace only should return empty vec
    let result = extract_json_values("   \n\t  ");
    assert!(result.is_empty());
}

#[test]
fn extract_json_values_handles_multiple_objects() {
    // Recovery: Multiple JSON objects should all be extracted
    let input = r#"{"a": 1}{"b": 2}{"c": 3}"#;
    let result = extract_json_values(input);
    assert_eq!(result.len(), 3);
}

#[test]
fn extract_json_values_handles_arrays() {
    // Recovery: JSON arrays should be extracted
    let input = r#"[1, 2, 3]{"key": "value"}"#;
    let result = extract_json_values(input);
    assert_eq!(result.len(), 2);
}

// ═══════════════════════════════════════════════════════════════════════
// Recovery Tests - Constants Validation
// ═══════════════════════════════════════════════════════════════════════

const _: () = {
    assert!(DEFAULT_MAX_TOOL_ITERATIONS > 0);
    assert!(DEFAULT_MAX_TOOL_ITERATIONS <= 100);
    assert!(DEFAULT_MAX_HISTORY_MESSAGES > 0);
    assert!(DEFAULT_MAX_HISTORY_MESSAGES <= 1000);
};

#[test]
fn constants_bounds_are_compile_time_checked() {
    // Bounds are enforced by the const assertions above.
}

// ═══════════════════════════════════════════════════════════════════════
// Recovery Tests - Tool Call Value Parsing
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn parse_tool_call_value_handles_missing_name_field() {
    // Recovery: Missing name field should return None
    let value = serde_json::json!({"function": {"arguments": {}}});
    let result = parse_tool_call_value(&value);
    assert!(result.is_none());
}

#[test]
fn parse_tool_call_value_handles_top_level_name() {
    // Recovery: Tool call with name at top level (non-OpenAI format)
    let value = serde_json::json!({"name": "test_tool", "arguments": {}});
    let result = parse_tool_call_value(&value);
    assert!(result.is_some());
    assert_eq!(result.unwrap().name, "test_tool");
}

#[test]
fn parse_tool_calls_from_json_value_handles_empty_array() {
    // Recovery: Empty tool_calls array should return empty vec
    let value = serde_json::json!({"tool_calls": []});
    let result = parse_tool_calls_from_json_value(&value);
    assert!(result.is_empty());
}

#[test]
fn parse_tool_calls_from_json_value_handles_missing_tool_calls() {
    // Recovery: Missing tool_calls field should fall through
    let value = serde_json::json!({"name": "test", "arguments": {}});
    let result = parse_tool_calls_from_json_value(&value);
    assert_eq!(result.len(), 1);
}

#[test]
fn parse_tool_calls_from_json_value_handles_top_level_array() {
    // Recovery: Top-level array of tool calls
    let value = serde_json::json!([
        {"name": "tool_a", "arguments": {}},
        {"name": "tool_b", "arguments": {}}
    ]);
    let result = parse_tool_calls_from_json_value(&value);
    assert_eq!(result.len(), 2);
}

// ─────────────────────────────────────────────────────────────────────
// TG4 (inline): parse_tool_calls robustness — malformed/edge-case inputs
// Prevents: Pattern 4 issues #746, #418, #777, #848
// ─────────────────────────────────────────────────────────────────────

#[test]
fn parse_tool_calls_empty_input_returns_empty() {
    let (text, calls) = parse_tool_calls("");
    assert!(calls.is_empty(), "empty input should produce no tool calls");
    assert!(text.is_empty(), "empty input should produce no text");
}

#[test]
fn parse_tool_calls_whitespace_only_returns_empty_calls() {
    let (text, calls) = parse_tool_calls("   \n\t  ");
    assert!(calls.is_empty());
    assert!(text.is_empty() || text.trim().is_empty());
}

#[test]
fn parse_tool_calls_nested_xml_tags_handled() {
    // Double-wrapped tool call should still parse the inner call
    let response =
        r#"<tool_call><tool_call>{"name":"echo","arguments":{"msg":"hi"}}</tool_call></tool_call>"#;
    let (_text, calls) = parse_tool_calls(response);
    // Should find at least one tool call
    assert!(
        !calls.is_empty(),
        "nested XML tags should still yield at least one tool call"
    );
}

#[test]
fn parse_tool_calls_truncated_json_no_panic() {
    // Incomplete JSON inside tool_call tags
    let response = r#"<tool_call>{"name":"shell","arguments":{"command":"ls"</tool_call>"#;
    let (_text, _calls) = parse_tool_calls(response);
    // Should not panic — graceful handling of truncated JSON
}

#[test]
fn parse_tool_calls_empty_json_object_in_tag() {
    let response = "<tool_call>{}</tool_call>";
    let (_text, calls) = parse_tool_calls(response);
    // Empty JSON object has no name field — should not produce valid tool call
    assert!(
        calls.is_empty(),
        "empty JSON object should not produce a tool call"
    );
}

#[test]
fn parse_tool_calls_closing_tag_only_returns_text() {
    let response = "Some text </tool_call> more text";
    let (text, calls) = parse_tool_calls(response);
    assert!(
        calls.is_empty(),
        "closing tag only should not produce calls"
    );
    assert!(
        !text.is_empty(),
        "text around orphaned closing tag should be preserved"
    );
}

#[test]
fn parse_tool_calls_very_large_arguments_no_panic() {
    let large_arg = "x".repeat(100_000);
    let response = format!(
        r#"<tool_call>{{"name":"echo","arguments":{{"message":"{}"}}}}</tool_call>"#,
        large_arg
    );
    let (_text, calls) = parse_tool_calls(&response);
    assert_eq!(calls.len(), 1, "large arguments should still parse");
    assert_eq!(calls[0].name, "echo");
}

#[test]
fn parse_tool_calls_special_characters_in_arguments() {
    let response = r#"<tool_call>{"name":"echo","arguments":{"message":"hello \"world\" <>&'\n\t"}}</tool_call>"#;
    let (_text, calls) = parse_tool_calls(response);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "echo");
}

#[test]
fn parse_tool_calls_text_with_embedded_json_not_extracted() {
    // Raw JSON without any tags should NOT be extracted as a tool call
    let response = r#"Here is some data: {"name":"echo","arguments":{"message":"hi"}} end."#;
    let (_text, calls) = parse_tool_calls(response);
    assert!(
        calls.is_empty(),
        "raw JSON in text without tags should not be extracted"
    );
}

#[test]
fn parse_tool_calls_multiple_formats_mixed() {
    // Mix of text and properly tagged tool call
    let response = r#"I'll help you with that.

<tool_call>
{"name":"shell","arguments":{"command":"echo hello"}}
</tool_call>

Let me check the result."#;
    let (text, calls) = parse_tool_calls(response);
    assert_eq!(
        calls.len(),
        1,
        "should extract one tool call from mixed content"
    );
    assert_eq!(calls[0].name, "shell");
    assert!(
        text.contains("help you"),
        "text before tool call should be preserved"
    );
}

// ─────────────────────────────────────────────────────────────────────
// TG4 (inline): scrub_credentials edge cases
// ─────────────────────────────────────────────────────────────────────

#[test]
fn scrub_credentials_empty_input() {
    let result = scrub_credentials("");
    assert_eq!(result, "");
}

#[test]
fn scrub_credentials_no_sensitive_data() {
    let input = "normal text without any secrets";
    let result = scrub_credentials(input);
    assert_eq!(
        result, input,
        "non-sensitive text should pass through unchanged"
    );
}

#[test]
fn scrub_credentials_multibyte_chars_no_panic() {
    // Regression test for #3024: byte index 4 is not a char boundary
    // when the captured value contains multi-byte UTF-8 characters.
    // The regex only matches quoted values for non-ASCII content, since
    // capture group 4 is restricted to [a-zA-Z0-9_\-\.].
    let input = "password=\"\u{4f60}\u{7684}WiFi\u{5bc6}\u{7801}ab\"";
    let result = scrub_credentials(input);
    assert!(
        result.contains("[REDACTED]"),
        "multi-byte quoted value should be redacted without panic, got: {result}"
    );
}

#[test]
fn scrub_credentials_short_values_not_redacted() {
    // Values shorter than 8 chars should not be redacted
    let input = r#"api_key="short""#;
    let result = scrub_credentials(input);
    assert_eq!(result, input, "short values should not be redacted");
}

// ─────────────────────────────────────────────────────────────────────
// TG4 (inline): trim_history edge cases
// ─────────────────────────────────────────────────────────────────────

#[test]
fn trim_history_empty_history() {
    let mut history: Vec<synapse_providers::ChatMessage> = vec![];
    trim_history(&mut history, 10);
    assert!(history.is_empty());
}

#[test]
fn trim_history_system_only() {
    let mut history = vec![synapse_providers::ChatMessage::system("system prompt")];
    trim_history(&mut history, 10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, "system");
}

#[test]
fn trim_history_exactly_at_limit() {
    let mut history = vec![
        synapse_providers::ChatMessage::system("system"),
        synapse_providers::ChatMessage::user("msg 1"),
        synapse_providers::ChatMessage::assistant("reply 1"),
    ];
    trim_history(&mut history, 2); // 2 non-system messages = exactly at limit
    assert_eq!(history.len(), 3, "should not trim when exactly at limit");
}

#[test]
fn trim_history_removes_oldest_non_system() {
    let mut history = vec![
        synapse_providers::ChatMessage::system("system"),
        synapse_providers::ChatMessage::user("old msg"),
        synapse_providers::ChatMessage::assistant("old reply"),
        synapse_providers::ChatMessage::user("new msg"),
        synapse_providers::ChatMessage::assistant("new reply"),
    ];
    trim_history(&mut history, 2);
    assert_eq!(history.len(), 3); // system + 2 kept
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].content, "new msg");
}

/// When `build_system_prompt_with_mode` is called with `native_tools = true`,
/// the output must contain ZERO tool-call envelope artifacts. In the native path
/// `build_tool_instructions` is never called, so the system prompt alone
/// must be clean of fallback envelope protocol.
#[test]
fn native_tools_system_prompt_contains_zero_xml() {
    use crate::channels::build_system_prompt_with_mode;

    let tool_summaries: Vec<(&str, &str)> = vec![
        ("shell", "Execute shell commands"),
        ("file_read", "Read files"),
    ];

    let system_prompt = build_system_prompt_with_mode(
        std::path::Path::new("/tmp"),
        "test-model",
        &tool_summaries,
        &[],  // no skills
        None, // no identity config
        None, // no bootstrap_max_chars
        true, // native_tools
        synapse_domain::config::schema::SkillsPromptInjectionMode::Full,
    );

    // Must contain zero XML protocol artifacts
    assert!(
        !system_prompt.contains("<tool_call>"),
        "Native prompt must not contain <tool_call>"
    );
    assert!(
        !system_prompt.contains("</tool_call>"),
        "Native prompt must not contain </tool_call>"
    );
    assert!(
        !system_prompt.contains("<tool_result>"),
        "Native prompt must not contain <tool_result>"
    );
    assert!(
        !system_prompt.contains("</tool_result>"),
        "Native prompt must not contain </tool_result>"
    );
    assert!(
        !system_prompt.contains("## Tool Use Protocol"),
        "Native prompt must not contain XML protocol header"
    );

    // Positive: native prompt should still list tools and contain task instructions
    assert!(
        system_prompt.contains("shell"),
        "Native prompt must list tool names"
    );
    assert!(
        system_prompt.contains("## Your Task"),
        "Native prompt should contain task instructions"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// reasoning_content pass-through tests for history builders
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn build_native_assistant_history_includes_reasoning_content() {
    let calls = vec![ToolCall {
        id: "call_1".into(),
        name: "shell".into(),
        arguments: "{}".into(),
    }];
    let result = build_native_assistant_history("answer", &calls, Some("thinking step"));
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["content"].as_str(), Some("answer"));
    assert_eq!(parsed["reasoning_content"].as_str(), Some("thinking step"));
    assert!(parsed["tool_calls"].is_array());
}

#[test]
fn build_native_assistant_history_omits_reasoning_content_when_none() {
    let calls = vec![ToolCall {
        id: "call_1".into(),
        name: "shell".into(),
        arguments: "{}".into(),
    }];
    let result = build_native_assistant_history("answer", &calls, None);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["content"].as_str(), Some("answer"));
    assert!(parsed.get("reasoning_content").is_none());
}

#[test]
fn build_native_assistant_history_from_parsed_calls_includes_reasoning_content() {
    let calls = vec![ParsedToolCall {
        name: "shell".into(),
        arguments: serde_json::json!({"command": "pwd"}),
        tool_call_id: Some("call_2".into()),
    }];
    let result =
        build_native_assistant_history_from_parsed_calls("answer", &calls, Some("deep thought"));
    assert!(result.is_some());
    let parsed: serde_json::Value = serde_json::from_str(result.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["content"].as_str(), Some("answer"));
    assert_eq!(parsed["reasoning_content"].as_str(), Some("deep thought"));
    assert!(parsed["tool_calls"].is_array());
}

#[test]
fn build_native_assistant_history_from_parsed_calls_omits_reasoning_content_when_none() {
    let calls = vec![ParsedToolCall {
        name: "shell".into(),
        arguments: serde_json::json!({"command": "pwd"}),
        tool_call_id: Some("call_2".into()),
    }];
    let result = build_native_assistant_history_from_parsed_calls("answer", &calls, None);
    assert!(result.is_some());
    let parsed: serde_json::Value = serde_json::from_str(result.as_deref().unwrap()).unwrap();
    assert_eq!(parsed["content"].as_str(), Some("answer"));
    assert!(parsed.get("reasoning_content").is_none());
}

// ── glob_match tests ──────────────────────────────────────────────────────

#[test]
fn glob_match_exact_no_wildcard() {
    assert!(glob_match("mcp_browser_navigate", "mcp_browser_navigate"));
    assert!(!glob_match("mcp_browser_navigate", "mcp_browser_click"));
}

#[test]
fn glob_match_prefix_wildcard() {
    // Suffix pattern: mcp_browser_*
    assert!(glob_match("mcp_browser_*", "mcp_browser_navigate"));
    assert!(glob_match("mcp_browser_*", "mcp_browser_click"));
    assert!(!glob_match("mcp_browser_*", "mcp_filesystem_read"));

    // Prefix pattern: *_read
    assert!(glob_match("*_read", "mcp_filesystem_read"));
    assert!(!glob_match("*_read", "mcp_filesystem_write"));

    // Infix: mcp_*_navigate
    assert!(glob_match("mcp_*_navigate", "mcp_browser_navigate"));
    assert!(!glob_match("mcp_*_navigate", "mcp_browser_click"));
}

#[test]
fn glob_match_star_matches_everything() {
    assert!(glob_match("*", "anything_at_all"));
    assert!(glob_match("*", ""));
}

// ── filter_tool_specs_for_turn tests ──────────────────────────────────────

fn make_spec(name: &str) -> crate::tools::ToolSpec {
    crate::tools::ToolSpec {
        name: name.to_string(),
        description: String::new(),
        parameters: serde_json::json!({}),
        runtime_role: None,
    }
}

#[test]
fn filter_tool_specs_no_groups_returns_all() {
    let specs = vec![
        make_spec("shell_exec"),
        make_spec("mcp_browser_navigate"),
        make_spec("mcp_filesystem_read"),
    ];
    let result = filter_tool_specs_for_turn(specs, &[], "hello");
    assert_eq!(result.len(), 3);
}

#[test]
fn filter_tool_specs_always_group_includes_matching_mcp_tool() {
    use synapse_domain::config::schema::{ToolFilterGroup, ToolFilterGroupMode};

    let specs = vec![
        make_spec("shell_exec"),
        make_spec("mcp_browser_navigate"),
        make_spec("mcp_filesystem_read"),
    ];
    let groups = vec![ToolFilterGroup {
        mode: ToolFilterGroupMode::Always,
        tools: vec!["mcp_filesystem_*".into()],
        keywords: vec![],
    }];
    let result = filter_tool_specs_for_turn(specs, &groups, "anything");
    let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
    // Built-in passes through, matched MCP passes, unmatched MCP excluded.
    assert!(names.contains(&"shell_exec"));
    assert!(names.contains(&"mcp_filesystem_read"));
    assert!(!names.contains(&"mcp_browser_navigate"));
}

#[test]
fn filter_tool_specs_dynamic_group_included_on_keyword_match() {
    use synapse_domain::config::schema::{ToolFilterGroup, ToolFilterGroupMode};

    let specs = vec![make_spec("shell_exec"), make_spec("mcp_browser_navigate")];
    let groups = vec![ToolFilterGroup {
        mode: ToolFilterGroupMode::Dynamic,
        tools: vec!["mcp_browser_*".into()],
        keywords: vec!["browse".into(), "website".into()],
    }];
    let result = filter_tool_specs_for_turn(specs, &groups, "please browse this page");
    let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"shell_exec"));
    assert!(names.contains(&"mcp_browser_navigate"));
}

#[test]
fn filter_tool_specs_dynamic_group_excluded_on_no_keyword_match() {
    use synapse_domain::config::schema::{ToolFilterGroup, ToolFilterGroupMode};

    let specs = vec![make_spec("shell_exec"), make_spec("mcp_browser_navigate")];
    let groups = vec![ToolFilterGroup {
        mode: ToolFilterGroupMode::Dynamic,
        tools: vec!["mcp_browser_*".into()],
        keywords: vec!["browse".into(), "website".into()],
    }];
    let result = filter_tool_specs_for_turn(specs, &groups, "read the file /etc/hosts");
    let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"shell_exec"));
    assert!(!names.contains(&"mcp_browser_navigate"));
}

#[test]
fn filter_tool_specs_dynamic_keyword_match_is_case_insensitive() {
    use synapse_domain::config::schema::{ToolFilterGroup, ToolFilterGroupMode};

    let specs = vec![make_spec("mcp_browser_navigate")];
    let groups = vec![ToolFilterGroup {
        mode: ToolFilterGroupMode::Dynamic,
        tools: vec!["mcp_browser_*".into()],
        keywords: vec!["Browse".into()],
    }];
    let result = filter_tool_specs_for_turn(specs, &groups, "BROWSE the site");
    assert_eq!(result.len(), 1);
}

// ── Token-based compaction tests ──────────────────────────

#[test]
fn estimate_history_tokens_empty() {
    assert_eq!(super::estimate_history_tokens(&[]), 0);
}

#[test]
fn estimate_history_tokens_single_message() {
    let history = vec![ChatMessage::user("hello world")]; // 11 chars
    let tokens = super::estimate_history_tokens(&history);
    // 11.div_ceil(4) + 4 = 3 + 4 = 7
    assert_eq!(tokens, 7);
}

#[test]
fn estimate_history_tokens_multiple_messages() {
    let history = vec![
        ChatMessage::system("You are helpful."), // 16 chars → 4 + 4 = 8
        ChatMessage::user("What is Rust?"),      // 13 chars → 4 + 4 = 8
        ChatMessage::assistant("A language."),   // 11 chars → 3 + 4 = 7
    ];
    let tokens = super::estimate_history_tokens(&history);
    assert_eq!(tokens, 23);
}

#[tokio::test]
async fn run_tool_call_loop_surfaces_tool_failure_reason_in_on_delta() {
    let provider = ScriptedProvider::from_text_responses(vec![
        r#"<tool_call>
{"name":"failing_shell","arguments":{"command":"rm -rf /"}}
</tool_call>"#,
        "I could not execute that command.",
    ]);

    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(FailingTool::new(
        "failing_shell",
        "Command not allowed by security policy: rm -rf /",
    ))];

    let mut history = vec![
        ChatMessage::system("test-system"),
        ChatMessage::user("delete everything"),
    ];
    let observer = NoopObserver;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);

    let result = run_tool_call_loop(
        &provider,
        &mut history,
        &tools_registry,
        &observer,
        "mock-provider",
        "mock-model",
        0.0,
        true,
        None,
        "telegram",
        &synapse_domain::config::schema::MultimodalConfig::default(),
        4,
        None,
        Some(tx),
        None,
        &[],
        &[],
        None,
        None,
    )
    .await
    .expect("tool loop should complete");

    // Collect all messages sent to the on_delta channel.
    let mut deltas = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        deltas.push(msg);
    }

    let all_deltas = deltas.join("");

    // The failure reason should appear in the progress messages.
    assert!(
        all_deltas.contains("Command not allowed by security policy"),
        "on_delta messages should include the tool failure reason, got: {all_deltas}"
    );

    // Should also contain the cross mark (❌) icon to indicate failure.
    assert!(
        all_deltas.contains('\u{274c}'),
        "on_delta messages should include ❌ for failed tool calls, got: {all_deltas}"
    );

    assert_eq!(result.response, "I could not execute that command.");
}

// ── filter_by_allowed_tools tests ─────────────────────────────────────

#[test]
fn filter_by_allowed_tools_none_passes_all() {
    let specs = vec![
        make_spec("shell"),
        make_spec("memory_store"),
        make_spec("file_read"),
    ];
    let result = filter_by_allowed_tools(specs, None);
    assert_eq!(result.len(), 3);
}

#[test]
fn filter_by_allowed_tools_some_restricts_to_listed() {
    let specs = vec![
        make_spec("shell"),
        make_spec("memory_store"),
        make_spec("file_read"),
    ];
    let allowed = vec!["shell".to_string(), "memory_store".to_string()];
    let result = filter_by_allowed_tools(specs, Some(&allowed));
    let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"shell"));
    assert!(names.contains(&"memory_store"));
    assert!(!names.contains(&"file_read"));
}

#[test]
fn filter_by_allowed_tools_unknown_names_silently_ignored() {
    let specs = vec![make_spec("shell"), make_spec("file_read")];
    let allowed = vec![
        "shell".to_string(),
        "nonexistent_tool".to_string(),
        "another_missing".to_string(),
    ];
    let result = filter_by_allowed_tools(specs, Some(&allowed));
    let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names.len(), 1);
    assert!(names.contains(&"shell"));
}

#[test]
fn filter_by_allowed_tools_empty_list_excludes_all() {
    let specs = vec![make_spec("shell"), make_spec("file_read")];
    let allowed: Vec<String> = vec![];
    let result = filter_by_allowed_tools(specs, Some(&allowed));
    assert!(result.is_empty());
}
