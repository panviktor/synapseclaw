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
use synapse_domain::application::services::model_lane_resolution::{
    ResolvedModelProfile, ResolvedModelProfileSource,
};
use synapse_domain::config::schema::ModelFeature;
use synapse_domain::domain::memory::{Skill as MemorySkill, SkillOrigin, SkillStatus};
use synapse_domain::ports::tool::{ToolContract, ToolNonReplayableReason};

fn test_tool_contract() -> ToolContract {
    ToolContract::non_replayable(None, ToolNonReplayableReason::Other("test_tool".into()))
}

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

#[test]
fn relevant_skill_cards_keep_full_skill_body_out_of_context() {
    let skill = MemorySkill {
        id: "skill-123".into(),
        name: "Matrix release drift audit".into(),
        description:
            "Find a local self-hosted chat server checkout and compare it with upstream releases."
                .into(),
        content: "# Full body\n\nSECRET_DETAILED_PROCEDURE_SHOULD_NOT_BE_IN_PREAMBLE".into(),
        task_family: Some("release-drift-audit".into()),
        tool_pattern: vec!["repo_discovery".into(), "git_operations".into()],
        lineage_task_families: Vec::new(),
        tags: vec!["matrix".into(), "selfhosted".into()],
        success_count: 0,
        fail_count: 0,
        version: 1,
        origin: SkillOrigin::Manual,
        status: SkillStatus::Active,
        created_by: "default".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let rendered = render_relevant_skill_cards(&[skill]);

    assert!(rendered.contains("<skill_id>skill-123</skill_id>"));
    assert!(rendered.contains("skill_read"));
    assert!(rendered.contains("repo_discovery, git_operations"));
    assert!(!rendered.contains("SECRET_DETAILED_PROCEDURE"));
    assert!(rendered.contains("omitted; load on demand with skill_read"));
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

#[tokio::test]
async fn execute_one_tool_captures_contract_sanitized_replay_args() {
    let observer = NoopObserver;
    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(ReplayableLookupTool::new("lookup"))];

    let outcome = execute_one_tool(
        "lookup",
        serde_json::json!({
            "url": "https://example.invalid/status",
            "api_key": "sk-secret-value",
            "ignored": "not declared in schema"
        }),
        &tools_registry,
        None,
        &observer,
        None,
        None,
        None,
    )
    .await
    .expect("lookup should execute");

    assert!(outcome.success);
    assert_eq!(
        outcome.replay_args,
        Some(serde_json::json!({ "url": "https://example.invalid/status" }))
    );
}

#[tokio::test]
async fn execute_one_tool_never_captures_shell_command_replay_args() {
    let observer = NoopObserver;
    let tools_registry: Vec<Box<dyn Tool>> = vec![Box::new(ReplayableLookupTool::new("shell"))];

    let outcome = execute_one_tool(
        "shell",
        serde_json::json!({ "url": "git status --short" }),
        &tools_registry,
        None,
        &observer,
        None,
        None,
        None,
    )
    .await
    .expect("fake shell should execute");

    assert!(outcome.success);
    assert!(outcome.replay_args.is_none());
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
        Ok(ChatResponse {
            text: Some("route-vision-ok".to_string()),
            tool_calls: Vec::new(),
            usage: None,
            reasoning_content: None,
            media_artifacts: Vec::new(),
        })
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
            media_artifacts: Vec::new(),
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
                media_artifacts: Vec::new(),
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

fn native_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn native_tool_response(calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls: calls,
        usage: None,
        reasoning_content: None,
        media_artifacts: Vec::new(),
    }
}

fn text_chat_response(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_string()),
        tool_calls: Vec::new(),
        usage: None,
        reasoning_content: None,
        media_artifacts: Vec::new(),
    }
}

fn native_tool_result_contents(history: &[ChatMessage]) -> Vec<String> {
    history
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| {
            serde_json::from_str::<serde_json::Value>(&message.content)
                .ok()
                .and_then(|value| {
                    value
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| message.content.clone())
        })
        .collect()
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

struct ReplayableLookupTool {
    name: String,
}

impl ReplayableLookupTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for ReplayableLookupTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Replayable typed lookup test tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" },
                "api_key": { "type": "string" }
            },
            "required": ["url"]
        })
    }

    fn runtime_role(&self) -> Option<synapse_domain::ports::tool::ToolRuntimeRole> {
        Some(synapse_domain::ports::tool::ToolRuntimeRole::ExternalLookup)
    }

    fn tool_contract(&self) -> synapse_domain::ports::tool::ToolContract {
        synapse_domain::ports::tool::ToolContract::replayable(self.runtime_role()).with_arguments(
            vec![
                synapse_domain::ports::tool::ToolArgumentPolicy::replayable("url"),
                synapse_domain::ports::tool::ToolArgumentPolicy::sensitive("api_key"),
            ],
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<crate::tools::ToolResult> {
        let url = args
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        Ok(crate::tools::ToolResult {
            success: true,
            output: format!("lookup:{url}"),
            error: None,
        })
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

    fn tool_contract(&self) -> ToolContract {
        test_tool_contract()
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

    fn tool_contract(&self) -> ToolContract {
        test_tool_contract()
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

    fn tool_contract(&self) -> ToolContract {
        test_tool_contract()
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
        ToolLoopRouteCapabilities::from_provider(&provider),
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
async fn run_tool_call_loop_honors_route_vision_capability_override() {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = NonVisionProvider {
        calls: Arc::clone(&calls),
    };

    let mut history = vec![ChatMessage::user(
        "please inspect [IMAGE:data:image/png;base64,iVBORw0KGgo=]".to_string(),
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
        ToolLoopRouteCapabilities::new(
            ProviderCapabilities::default(),
            ResolvedModelProfile {
                features: vec![ModelFeature::Vision],
                features_source: ResolvedModelProfileSource::ManualConfig,
                ..Default::default()
            },
        ),
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
    .expect("route-aware vision capability should allow image input");

    assert_eq!(result.response, "route-vision-ok");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
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
        ToolLoopRouteCapabilities::from_provider(&provider),
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
        ToolLoopRouteCapabilities::from_provider(&provider),
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
    let provider = ScriptedProvider::from_chat_responses(vec![
        native_tool_response(vec![
            native_call("call_delay_a", "delay_a", serde_json::json!({"value":"A"})),
            native_call("call_delay_b", "delay_b", serde_json::json!({"value":"B"})),
        ]),
        text_chat_response("done"),
    ])
    .with_native_tool_support();

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
        ToolLoopRouteCapabilities::from_provider(&provider),
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

    let tool_results = native_tool_result_contents(&history);
    let idx_a = tool_results
        .iter()
        .position(|content| content.contains("ok:A"))
        .expect("delay_a result should be present");
    let idx_b = tool_results
        .iter()
        .position(|content| content.contains("ok:B"))
        .expect("delay_b result should be present");
    assert!(
        idx_a < idx_b,
        "tool results should preserve input order for tool call mapping"
    );
}

#[tokio::test]
async fn run_tool_call_loop_deduplicates_repeated_tool_calls() {
    let provider = ScriptedProvider::from_chat_responses(vec![
        native_tool_response(vec![
            native_call(
                "call_count_1",
                "count_tool",
                serde_json::json!({"value":"A"}),
            ),
            native_call(
                "call_count_2",
                "count_tool",
                serde_json::json!({"value":"A"}),
            ),
        ]),
        text_chat_response("done"),
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
        ToolLoopRouteCapabilities::from_provider(&provider),
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

    let tool_results = native_tool_result_contents(&history).join("\n");
    assert!(tool_results.contains("counted:A"));
    assert!(tool_results.contains("Skipped duplicate tool call"));
}

#[tokio::test]
async fn run_tool_call_loop_allows_low_risk_shell_in_non_interactive_mode() {
    let provider = ScriptedProvider::from_chat_responses(vec![
        native_tool_response(vec![native_call(
            "call_shell",
            "shell",
            serde_json::json!({"command":"echo hello"}),
        )]),
        text_chat_response("done"),
    ])
    .with_native_tool_support();

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
        ToolLoopRouteCapabilities::from_provider(&provider),
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

    let tool_results = native_tool_result_contents(&history).join("\n");
    assert!(tool_results.contains("hello"));
    assert!(!tool_results.contains("Denied by user."));
}

#[tokio::test]
async fn run_tool_call_loop_dedup_exempt_allows_repeated_calls() {
    let provider = ScriptedProvider::from_chat_responses(vec![
        native_tool_response(vec![
            native_call(
                "call_count_1",
                "count_tool",
                serde_json::json!({"value":"A"}),
            ),
            native_call(
                "call_count_2",
                "count_tool",
                serde_json::json!({"value":"A"}),
            ),
        ]),
        text_chat_response("done"),
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
        ToolLoopRouteCapabilities::from_provider(&provider),
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

    let tool_results = native_tool_result_contents(&history).join("\n");
    assert!(
        !tool_results.contains("Skipped duplicate tool call"),
        "exempt tool calls should not be suppressed"
    );
}

#[tokio::test]
async fn run_tool_call_loop_dedup_exempt_only_affects_listed_tools() {
    let provider = ScriptedProvider::from_chat_responses(vec![
        native_tool_response(vec![
            native_call(
                "call_count_1",
                "count_tool",
                serde_json::json!({"value":"A"}),
            ),
            native_call(
                "call_count_2",
                "count_tool",
                serde_json::json!({"value":"A"}),
            ),
            native_call(
                "call_other_1",
                "other_tool",
                serde_json::json!({"value":"B"}),
            ),
            native_call(
                "call_other_2",
                "other_tool",
                serde_json::json!({"value":"B"}),
            ),
        ]),
        text_chat_response("done"),
    ])
    .with_native_tool_support();

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
        ToolLoopRouteCapabilities::from_provider(&provider),
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
            media_artifacts: Vec::new(),
        },
        ChatResponse {
            text: Some("done".into()),
            tool_calls: Vec::new(),
            usage: None,
            reasoning_content: None,
            media_artifacts: Vec::new(),
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
        ToolLoopRouteCapabilities::from_provider(&provider),
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
        "tool result should preserve native tool_call_id"
    );
    assert!(
        history
            .iter()
            .all(|msg| !(msg.role == "user" && msg.content.starts_with("[Tool results]"))),
        "native mode should use role=tool history"
    );
}

#[test]
fn agent_turn_executes_activated_tool_from_wrapper() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");

    runtime.block_on(async {
        let provider = ScriptedProvider::from_chat_responses(vec![
            native_tool_response(vec![native_call(
                "call_pixel_health",
                "pixel__get_api_health",
                serde_json::json!({"value":"ok"}),
            )]),
            text_chat_response("done"),
        ])
        .with_native_tool_support();

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
// Native tool-call parsing and visible-output hygiene
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn parse_structured_tool_calls_preserves_native_id_and_arguments() {
    let calls = vec![native_call(
        "call_shell_1",
        "shell",
        serde_json::json!({"command":"pwd"}),
    )];

    let parsed = parse_structured_tool_calls(&calls).unwrap();

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].name, "shell");
    assert_eq!(parsed[0].tool_call_id.as_deref(), Some("call_shell_1"));
    assert_eq!(parsed[0].arguments["command"], "pwd");
}

#[test]
fn parse_structured_tool_calls_rejects_invalid_arguments_json() {
    let calls = vec![synapse_providers::ToolCall {
        id: "call_bad".into(),
        name: "shell".into(),
        arguments: "{not-json".into(),
    }];

    let error = parse_structured_tool_calls(&calls).unwrap_err();

    assert!(error.to_string().contains("invalid JSON arguments"));
}

#[test]
fn parse_structured_tool_calls_rejects_missing_id() {
    let calls = vec![synapse_providers::ToolCall {
        id: " ".into(),
        name: "shell".into(),
        arguments: "{}".into(),
    }];

    let error = parse_structured_tool_calls(&calls).unwrap_err();

    assert!(error.to_string().contains("missing call id"));
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
/// the output must contain ZERO tool-call envelope artifacts.
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
    )
    .unwrap();

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
    let provider = ScriptedProvider::from_chat_responses(vec![
        native_tool_response(vec![native_call(
            "call_failing_shell",
            "failing_shell",
            serde_json::json!({"command":"rm -rf /"}),
        )]),
        text_chat_response("I could not execute that command."),
    ])
    .with_native_tool_support();

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
        ToolLoopRouteCapabilities::from_provider(&provider),
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
