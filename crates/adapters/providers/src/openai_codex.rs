use crate::auth::openai_oauth::{extract_account_id_from_jwt, refresh_access_token};
use crate::auth::AuthService;
use crate::multimodal;
use crate::traits::{
    ChatMessage, ChatRequest as ProviderChatRequest, ChatResponse as ProviderChatResponse,
    Provider, ProviderCapabilities, TokenUsage, ToolCall as ProviderToolCall, ToolSpec,
};
use crate::ProviderRuntimeOptions;
use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, TimeZone, Utc};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const CODEX_RESPONSES_URL_ENV: &str = "SYNAPSECLAW_CODEX_RESPONSES_URL";
const CODEX_BASE_URL_ENV: &str = "SYNAPSECLAW_CODEX_BASE_URL";
const CODEX_ACCESS_TOKEN_ENV: &str = "SYNAPSECLAW_OPENAI_CODEX_ACCESS_TOKEN";
const CODEX_REFRESH_TOKEN_ENV: &str = "SYNAPSECLAW_OPENAI_CODEX_REFRESH_TOKEN";
const CODEX_ACCOUNT_ID_ENV: &str = "SYNAPSECLAW_OPENAI_CODEX_ACCOUNT_ID";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const DEFAULT_CODEX_INSTRUCTIONS: &str =
    "You are SynapseClaw, a concise and helpful coding assistant.";
const CODEX_TOKEN_REFRESH_SKEW_SECS: i64 = 90;

pub struct OpenAiCodexProvider {
    auth: AuthService,
    auth_profile_override: Option<String>,
    responses_url: String,
    custom_endpoint: bool,
    gateway_api_key: Option<String>,
    reasoning_effort: Option<String>,
    client: Client,
}

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<Value>,
    instructions: String,
    store: bool,
    stream: bool,
    text: ResponsesTextOptions,
    reasoning: ResponsesReasoningOptions,
    include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    parallel_tool_calls: bool,
}

#[derive(Debug, Serialize)]
struct ResponsesInputContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ResponsesTextOptions {
    verbosity: String,
}

#[derive(Debug, Serialize)]
struct ResponsesReasoningOptions {
    effort: String,
    summary: String,
}

#[derive(Debug, Serialize)]
struct ResponsesToolSpec {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    description: String,
    parameters: Value,
    strict: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    output: Vec<ResponsesOutput>,
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesOutput {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    encrypted_content: Option<String>,
    #[serde(default)]
    content: Vec<ResponsesContent>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesContent {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    input_tokens_details: Option<ResponsesInputTokensDetails>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesInputTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
struct EnvCodexAuth {
    access_token: Option<String>,
    refresh_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexCliAuthFile {
    #[serde(default)]
    auth_mode: Option<String>,
    #[serde(default)]
    tokens: Option<CodexCliAuthTokens>,
}

#[derive(Debug, Deserialize)]
struct CodexCliAuthTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

impl OpenAiCodexProvider {
    pub fn new(
        options: &ProviderRuntimeOptions,
        gateway_api_key: Option<&str>,
    ) -> anyhow::Result<Self> {
        let state_dir = options
            .synapseclaw_dir
            .clone()
            .unwrap_or_else(default_synapseclaw_dir);
        let auth = AuthService::new(&state_dir, options.secrets_encrypt);
        let responses_url = resolve_responses_url(options)?;

        Ok(Self {
            auth,
            auth_profile_override: options.auth_profile_override.clone(),
            custom_endpoint: !is_default_responses_url(&responses_url),
            responses_url,
            gateway_api_key: gateway_api_key.map(ToString::to_string),
            reasoning_effort: options.reasoning_effort.clone(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        })
    }
}

fn default_synapseclaw_dir() -> PathBuf {
    directories::UserDirs::new().map_or_else(
        || PathBuf::from(".synapseclaw"),
        |dirs| dirs.home_dir().join(".synapseclaw"),
    )
}

fn build_responses_url(base_or_endpoint: &str) -> anyhow::Result<String> {
    let candidate = base_or_endpoint.trim();
    if candidate.is_empty() {
        anyhow::bail!("OpenAI Codex endpoint override cannot be empty");
    }

    let mut parsed = reqwest::Url::parse(candidate)
        .map_err(|_| anyhow::anyhow!("OpenAI Codex endpoint override must be a valid URL"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        _ => anyhow::bail!("OpenAI Codex endpoint override must use http:// or https://"),
    }

    let path = parsed.path().trim_end_matches('/');
    if !path.ends_with("/responses") {
        let with_suffix = if path.is_empty() || path == "/" {
            "/responses".to_string()
        } else {
            format!("{path}/responses")
        };
        parsed.set_path(&with_suffix);
    }

    parsed.set_query(None);
    parsed.set_fragment(None);

    Ok(parsed.to_string())
}

fn resolve_responses_url(options: &ProviderRuntimeOptions) -> anyhow::Result<String> {
    if let Some(endpoint) = std::env::var(CODEX_RESPONSES_URL_ENV)
        .ok()
        .and_then(|value| first_nonempty(Some(&value)))
    {
        return build_responses_url(&endpoint);
    }

    if let Some(base_url) = std::env::var(CODEX_BASE_URL_ENV)
        .ok()
        .and_then(|value| first_nonempty(Some(&value)))
    {
        return build_responses_url(&base_url);
    }

    if let Some(api_url) = options
        .provider_api_url
        .as_deref()
        .and_then(|value| first_nonempty(Some(value)))
    {
        return build_responses_url(&api_url);
    }

    Ok(DEFAULT_CODEX_RESPONSES_URL.to_string())
}

fn canonical_endpoint(url: &str) -> Option<(String, String, u16, String)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default()?;
    let path = parsed.path().trim_end_matches('/').to_string();
    Some((parsed.scheme().to_ascii_lowercase(), host, port, path))
}

fn is_default_responses_url(url: &str) -> bool {
    canonical_endpoint(url) == canonical_endpoint(DEFAULT_CODEX_RESPONSES_URL)
}

fn first_nonempty(text: Option<&str>) -> Option<String> {
    text.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn resolve_instructions(system_prompt: Option<&str>) -> String {
    first_nonempty(system_prompt).unwrap_or_else(|| DEFAULT_CODEX_INSTRUCTIONS.to_string())
}

fn normalize_model_id(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

fn read_env_codex_auth() -> Option<EnvCodexAuth> {
    let access_token = std::env::var(CODEX_ACCESS_TOKEN_ENV)
        .ok()
        .and_then(|value| first_nonempty(Some(&value)));
    let refresh_token = std::env::var(CODEX_REFRESH_TOKEN_ENV)
        .ok()
        .and_then(|value| first_nonempty(Some(&value)));
    let account_id = std::env::var(CODEX_ACCOUNT_ID_ENV)
        .ok()
        .and_then(|value| first_nonempty(Some(&value)));

    if access_token.is_none() && refresh_token.is_none() {
        None
    } else {
        Some(EnvCodexAuth {
            access_token,
            refresh_token,
            account_id,
        })
    }
}

fn resolve_codex_cli_home() -> Option<PathBuf> {
    match std::env::var(CODEX_HOME_ENV)
        .ok()
        .and_then(|value| first_nonempty(Some(&value)))
    {
        Some(configured) if configured == "~" => {
            directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
        }
        Some(configured) if configured.starts_with("~/") => directories::UserDirs::new()
            .map(|dirs| dirs.home_dir().join(configured.trim_start_matches("~/"))),
        Some(configured) => Some(PathBuf::from(configured)),
        None => directories::UserDirs::new().map(|dirs| dirs.home_dir().join(".codex")),
    }
}

fn read_codex_cli_auth() -> Option<EnvCodexAuth> {
    let auth_path = resolve_codex_cli_home()?.join("auth.json");
    let raw = fs::read_to_string(auth_path).ok()?;
    let parsed: CodexCliAuthFile = serde_json::from_str(&raw).ok()?;
    if parsed.auth_mode.as_deref() != Some("chatgpt") {
        return None;
    }

    let tokens = parsed.tokens?;
    let access_token = tokens
        .access_token
        .as_deref()
        .and_then(|value| first_nonempty(Some(value)));
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .and_then(|value| first_nonempty(Some(value)));
    let account_id = tokens
        .account_id
        .as_deref()
        .and_then(|value| first_nonempty(Some(value)));

    if access_token.is_none() && refresh_token.is_none() {
        None
    } else {
        Some(EnvCodexAuth {
            access_token,
            refresh_token,
            account_id,
        })
    }
}

fn extract_expiry_from_jwt(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    Utc.timestamp_opt(exp, 0).single()
}

fn token_is_expiring(token: &str, skew_secs: i64) -> bool {
    match extract_expiry_from_jwt(token) {
        Some(expiry) => expiry <= Utc::now() + chrono::Duration::seconds(skew_secs),
        None => false,
    }
}

async fn resolve_external_access_token(
    client: &Client,
) -> anyhow::Result<Option<(String, Option<String>)>> {
    let Some(external_auth) = read_env_codex_auth().or_else(read_codex_cli_auth) else {
        return Ok(None);
    };

    let access_token = match external_auth.access_token {
        Some(token) if !token_is_expiring(&token, CODEX_TOKEN_REFRESH_SKEW_SECS) => token,
        Some(token) => match external_auth.refresh_token.as_deref() {
            Some(refresh_token) => refresh_access_token(client, refresh_token)
                .await
                .map(|token_set| token_set.access_token)?,
            None => token,
        },
        None => match external_auth.refresh_token.as_deref() {
            Some(refresh_token) => refresh_access_token(client, refresh_token)
                .await
                .map(|token_set| token_set.access_token)?,
            None => return Ok(None),
        },
    };

    let account_id = external_auth
        .account_id
        .or_else(|| extract_account_id_from_jwt(&access_token));

    Ok(Some((access_token, account_id)))
}

fn build_responses_input(messages: &[ChatMessage]) -> (String, Vec<Value>) {
    let mut system_parts: Vec<&str> = Vec::new();
    let mut input: Vec<Value> = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => system_parts.push(&msg.content),
            "user" => {
                let (cleaned_text, image_refs) = multimodal::parse_image_markers(&msg.content);

                let mut content_items = Vec::new();

                // Add text if present
                if !cleaned_text.trim().is_empty() {
                    content_items.push(ResponsesInputContent {
                        kind: "input_text".to_string(),
                        text: Some(cleaned_text),
                        image_url: None,
                    });
                }

                // Add images
                for image_ref in image_refs {
                    content_items.push(ResponsesInputContent {
                        kind: "input_image".to_string(),
                        text: None,
                        image_url: Some(image_ref),
                    });
                }

                // If no content at all, add empty text
                if content_items.is_empty() {
                    content_items.push(ResponsesInputContent {
                        kind: "input_text".to_string(),
                        text: Some(String::new()),
                        image_url: None,
                    });
                }

                input.push(serde_json::json!({
                    "role": "user",
                    "content": content_items,
                }));
            }
            "assistant" => {
                append_assistant_input(&mut input, &msg.content);
            }
            "tool" => {
                append_tool_result_input(&mut input, &msg.content);
            }
            _ => {}
        }
    }

    let instructions = if system_parts.is_empty() {
        DEFAULT_CODEX_INSTRUCTIONS.to_string()
    } else {
        system_parts.join("\n\n")
    };

    (instructions, input)
}

fn append_assistant_input(input: &mut Vec<Value>, content: &str) {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        let text = value
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToString::to_string);

        if let Some(text) = text {
            input.push(serde_json::json!({
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": text,
                }],
            }));
        }

        if let Some(tool_calls) = value.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                let call_id = tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty());
                let name = tool_call
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty());
                let arguments = tool_call
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "{}".to_string());
                if let (Some(call_id), Some(name)) = (call_id, name) {
                    input.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments,
                    }));
                }
            }
            return;
        }
    }

    if !content.trim().is_empty() {
        input.push(serde_json::json!({
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": content,
            }],
        }));
    }
}

fn append_tool_result_input(input: &mut Vec<Value>, content: &str) {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        let call_id = value
            .get("tool_call_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let output = value
            .get("content")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| content.to_string());
        if let Some(call_id) = call_id {
            input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            return;
        }
    }

    input.push(serde_json::json!({
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": format!("[Tool result]\\n{content}"),
        }],
    }));
}

fn normalize_strict_tool_schema(schema: Value) -> Value {
    match schema {
        Value::Object(mut obj) => {
            if let Some(Value::Object(properties)) = obj.get_mut("properties") {
                let normalized = properties
                    .iter_mut()
                    .map(|(key, value)| (key.clone(), normalize_strict_tool_schema(value.take())))
                    .collect();
                *properties = normalized;
            }

            if let Some(Value::Object(defs)) = obj.get_mut("$defs") {
                let normalized = defs
                    .iter_mut()
                    .map(|(key, value)| (key.clone(), normalize_strict_tool_schema(value.take())))
                    .collect();
                *defs = normalized;
            }

            if let Some(Value::Object(defs)) = obj.get_mut("definitions") {
                let normalized = defs
                    .iter_mut()
                    .map(|(key, value)| (key.clone(), normalize_strict_tool_schema(value.take())))
                    .collect();
                *defs = normalized;
            }

            for key in ["items", "additionalProperties", "contains"] {
                if let Some(value) = obj.get_mut(key) {
                    *value = normalize_strict_tool_schema(value.take());
                }
            }

            for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                if let Some(Value::Array(items)) = obj.get_mut(key) {
                    for item in items {
                        *item = normalize_strict_tool_schema(item.take());
                    }
                }
            }

            let should_close_object = matches!(obj.get("type"), Some(Value::String(t)) if t == "object")
                && (obj.contains_key("properties") || obj.contains_key("required"));
            if should_close_object {
                obj.insert("additionalProperties".to_string(), Value::Bool(false));
            }

            Value::Object(obj)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(normalize_strict_tool_schema)
                .collect(),
        ),
        other => other,
    }
}

fn convert_tools(tools: Option<&[ToolSpec]>) -> Option<Vec<ResponsesToolSpec>> {
    tools.map(|items| {
        items
            .iter()
            .map(|tool| ResponsesToolSpec {
                kind: "function".to_string(),
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: normalize_strict_tool_schema(tool.parameters.clone()),
                strict: true,
            })
            .collect()
    })
}

fn extract_responses_tool_calls(response: &ResponsesResponse) -> Vec<ProviderToolCall> {
    response
        .output
        .iter()
        .filter(|item| item.kind.as_deref() == Some("function_call"))
        .filter_map(|item| {
            let call_id = item
                .call_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())?;
            let name = item
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())?;
            Some(ProviderToolCall {
                id: call_id.to_string(),
                name: name.to_string(),
                arguments: item.arguments.clone().unwrap_or_else(|| "{}".to_string()),
            })
        })
        .collect()
}

fn extract_responses_reasoning(response: &ResponsesResponse) -> Option<String> {
    response
        .output
        .iter()
        .find(|item| item.kind.as_deref() == Some("reasoning"))
        .and_then(|item| item.encrypted_content.clone())
}

fn parse_responses_chat_response(response: ResponsesResponse) -> ProviderChatResponse {
    let usage = response.usage.as_ref().map(|usage| TokenUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
    });

    ProviderChatResponse {
        text: extract_responses_text(&response),
        tool_calls: extract_responses_tool_calls(&response),
        usage,
        reasoning_content: extract_responses_reasoning(&response),
    }
}

fn clamp_reasoning_effort(model: &str, effort: &str) -> String {
    let id = normalize_model_id(model);
    if matches!(id, "gpt-5.4" | "gpt-5.4-mini" | "gpt-5-codex") {
        return match effort {
            "low" | "medium" | "high" => effort.to_string(),
            "minimal" => "low".to_string(),
            _ => "high".to_string(),
        };
    }
    if (id.starts_with("gpt-5.2") || id.starts_with("gpt-5.3") || id.starts_with("gpt-5.4"))
        && effort == "minimal"
    {
        return "low".to_string();
    }
    if (id.starts_with("gpt-5.4") || id.starts_with("gpt-5-codex")) && effort == "xhigh" {
        return "high".to_string();
    }
    if id == "gpt-5.1" && effort == "xhigh" {
        return "high".to_string();
    }
    if id == "gpt-5.1-codex-mini" {
        return if effort == "high" || effort == "xhigh" {
            "high".to_string()
        } else {
            "medium".to_string()
        };
    }
    effort.to_string()
}

fn resolve_reasoning_effort(model_id: &str, configured: Option<&str>) -> String {
    let raw = configured
        .map(ToString::to_string)
        .or_else(|| std::env::var("SYNAPSECLAW_CODEX_REASONING_EFFORT").ok())
        .and_then(|value| first_nonempty(Some(&value)))
        .unwrap_or_else(|| "xhigh".to_string())
        .to_ascii_lowercase();
    clamp_reasoning_effort(model_id, &raw)
}

fn nonempty_preserve(text: Option<&str>) -> Option<String> {
    text.and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

fn extract_responses_text(response: &ResponsesResponse) -> Option<String> {
    if let Some(text) = first_nonempty(response.output_text.as_deref()) {
        return Some(text);
    }

    for item in &response.output {
        for content in &item.content {
            if content.kind.as_deref() == Some("output_text") {
                if let Some(text) = first_nonempty(content.text.as_deref()) {
                    return Some(text);
                }
            }
        }
    }

    for item in &response.output {
        for content in &item.content {
            if let Some(text) = first_nonempty(content.text.as_deref()) {
                return Some(text);
            }
        }
    }

    None
}

fn extract_stream_event_text(event: &Value, saw_delta: bool) -> Option<String> {
    let event_type = event.get("type").and_then(Value::as_str);
    match event_type {
        Some("response.output_text.delta") => {
            nonempty_preserve(event.get("delta").and_then(Value::as_str))
        }
        Some("response.output_text.done") if !saw_delta => {
            nonempty_preserve(event.get("text").and_then(Value::as_str))
        }
        Some("response.completed" | "response.done") => event
            .get("response")
            .and_then(|value| serde_json::from_value::<ResponsesResponse>(value.clone()).ok())
            .and_then(|response| extract_responses_text(&response)),
        _ => None,
    }
}

fn parse_sse_text(body: &str) -> anyhow::Result<Option<String>> {
    let mut saw_delta = false;
    let mut delta_accumulator = String::new();
    let mut fallback_text = None;
    let mut buffer = body.to_string();

    let mut process_event = |event: Value| -> anyhow::Result<()> {
        if let Some(message) = extract_stream_error_message(&event) {
            return Err(anyhow::anyhow!("OpenAI Codex stream error: {message}"));
        }
        if let Some(text) = extract_stream_event_text(&event, saw_delta) {
            let event_type = event.get("type").and_then(Value::as_str);
            if event_type == Some("response.output_text.delta") {
                saw_delta = true;
                delta_accumulator.push_str(&text);
            } else if fallback_text.is_none() {
                fallback_text = Some(text);
            }
        }
        Ok(())
    };

    let mut process_chunk = |chunk: &str| -> anyhow::Result<()> {
        let data_lines: Vec<String> = chunk
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.trim().to_string())
            .collect();
        if data_lines.is_empty() {
            return Ok(());
        }

        let joined = data_lines.join("\n");
        let trimmed = joined.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return Ok(());
        }

        if let Ok(event) = serde_json::from_str::<Value>(trimmed) {
            return process_event(event);
        }

        for line in data_lines {
            let line = line.trim();
            if line.is_empty() || line == "[DONE]" {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<Value>(line) {
                process_event(event)?;
            }
        }

        Ok(())
    };

    loop {
        let Some(idx) = buffer.find("\n\n") else {
            break;
        };

        let chunk = buffer[..idx].to_string();
        buffer = buffer[idx + 2..].to_string();
        process_chunk(&chunk)?;
    }

    if !buffer.trim().is_empty() {
        process_chunk(&buffer)?;
    }

    if saw_delta {
        return Ok(nonempty_preserve(Some(&delta_accumulator)));
    }

    Ok(fallback_text)
}

fn parse_sse_response(body: &str) -> anyhow::Result<Option<ResponsesResponse>> {
    let mut completed_response = None;
    let mut buffer = body.to_string();

    let mut process_event = |event: Value| -> anyhow::Result<()> {
        if let Some(message) = extract_stream_error_message(&event) {
            return Err(anyhow::anyhow!("OpenAI Codex stream error: {message}"));
        }
        let event_type = event.get("type").and_then(Value::as_str);
        if matches!(event_type, Some("response.completed" | "response.done")) {
            if let Some(response) = event
                .get("response")
                .and_then(|value| serde_json::from_value::<ResponsesResponse>(value.clone()).ok())
            {
                completed_response = Some(response);
            }
        }
        Ok(())
    };

    let mut process_chunk = |chunk: &str| -> anyhow::Result<()> {
        let data_lines: Vec<String> = chunk
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.trim().to_string())
            .collect();
        if data_lines.is_empty() {
            return Ok(());
        }

        let joined = data_lines.join("\n");
        let trimmed = joined.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return Ok(());
        }

        if let Ok(event) = serde_json::from_str::<Value>(trimmed) {
            return process_event(event);
        }

        for line in data_lines {
            let line = line.trim();
            if line.is_empty() || line == "[DONE]" {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<Value>(line) {
                process_event(event)?;
            }
        }

        Ok(())
    };

    loop {
        let Some(idx) = buffer.find("\n\n") else {
            break;
        };
        let chunk = buffer[..idx].to_string();
        buffer = buffer[idx + 2..].to_string();
        process_chunk(&chunk)?;
    }

    if !buffer.trim().is_empty() {
        process_chunk(&buffer)?;
    }

    Ok(completed_response)
}

fn extract_stream_error_message(event: &Value) -> Option<String> {
    let event_type = event.get("type").and_then(Value::as_str);

    if event_type == Some("error") {
        return first_nonempty(
            event
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| event.get("code").and_then(Value::as_str))
                .or_else(|| {
                    event
                        .get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(Value::as_str)
                }),
        );
    }

    if event_type == Some("response.failed") {
        return first_nonempty(
            event
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str),
        );
    }

    None
}

fn append_utf8_stream_chunk(
    body: &mut String,
    pending: &mut Vec<u8>,
    chunk: &[u8],
) -> anyhow::Result<()> {
    if pending.is_empty() {
        if let Ok(text) = std::str::from_utf8(chunk) {
            body.push_str(text);
            return Ok(());
        }
    }

    if !chunk.is_empty() {
        pending.extend_from_slice(chunk);
    }
    if pending.is_empty() {
        return Ok(());
    }

    match std::str::from_utf8(pending) {
        Ok(text) => {
            body.push_str(text);
            pending.clear();
            Ok(())
        }
        Err(err) => {
            let valid_up_to = err.valid_up_to();
            if valid_up_to > 0 {
                // SAFETY: `valid_up_to` always points to the end of a valid UTF-8 prefix.
                let prefix = std::str::from_utf8(&pending[..valid_up_to])
                    .expect("valid UTF-8 prefix from Utf8Error::valid_up_to");
                body.push_str(prefix);
                pending.drain(..valid_up_to);
            }

            if err.error_len().is_some() {
                return Err(anyhow::anyhow!(
                    "OpenAI Codex response contained invalid UTF-8: {err}"
                ));
            }

            // `error_len == None` means we have a valid prefix and an incomplete
            // multi-byte sequence at the end; keep it buffered until next chunk.
            Ok(())
        }
    }
}

fn decode_utf8_stream_chunks<'a, I>(chunks: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut body = String::new();
    let mut pending = Vec::new();

    for chunk in chunks {
        append_utf8_stream_chunk(&mut body, &mut pending, chunk)?;
    }

    if !pending.is_empty() {
        let err = std::str::from_utf8(&pending).expect_err("pending bytes should be invalid UTF-8");
        return Err(anyhow::anyhow!(
            "OpenAI Codex response ended with incomplete UTF-8: {err}"
        ));
    }

    Ok(body)
}

/// Read the response body incrementally via `bytes_stream()` to avoid
/// buffering the entire SSE payload in memory.  The previous implementation
/// used `response.text().await?` which holds the HTTP connection open until
/// every byte has arrived — on high-latency links the long-lived connection
/// often drops mid-read, producing the "error decoding response body" failure
/// reported in #3544.
async fn decode_responses_body(response: reqwest::Response) -> anyhow::Result<String> {
    let mut body = String::new();
    let mut pending_utf8 = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk
            .map_err(|err| anyhow::anyhow!("error reading OpenAI Codex response stream: {err}"))?;
        append_utf8_stream_chunk(&mut body, &mut pending_utf8, &bytes)?;
    }

    if !pending_utf8.is_empty() {
        let err = std::str::from_utf8(&pending_utf8)
            .expect_err("pending bytes should be invalid UTF-8 at end of stream");
        return Err(anyhow::anyhow!(
            "OpenAI Codex response ended with incomplete UTF-8: {err}"
        ));
    }

    if let Some(text) = parse_sse_text(&body)? {
        return Ok(text);
    }

    let body_trimmed = body.trim_start();
    let looks_like_sse = body_trimmed.starts_with("event:") || body_trimmed.starts_with("data:");
    if looks_like_sse {
        return Err(anyhow::anyhow!(
            "No response from OpenAI Codex stream payload: {}",
            super::sanitize_api_error(&body)
        ));
    }

    let parsed: ResponsesResponse = serde_json::from_str(&body).map_err(|err| {
        anyhow::anyhow!(
            "OpenAI Codex JSON parse failed: {err}. Payload: {}",
            super::sanitize_api_error(&body)
        )
    })?;
    extract_responses_text(&parsed).ok_or_else(|| anyhow::anyhow!("No response from OpenAI Codex"))
}

impl OpenAiCodexProvider {
    async fn send_request(&self, request: &ResponsesRequest) -> anyhow::Result<reqwest::Response> {
        let use_gateway_api_key_auth = self.custom_endpoint && self.gateway_api_key.is_some();
        let external_auth = resolve_external_access_token(&self.client).await?;
        if external_auth.is_some() {
            tracing::info!("using external OpenAI Codex auth source");
        }
        let profile = match self
            .auth
            .get_profile("openai-codex", self.auth_profile_override.as_deref())
            .await
        {
            Ok(profile) => profile,
            Err(err) if external_auth.is_some() => {
                tracing::warn!(
                    error = %err,
                    "failed to load OpenAI Codex profile; continuing with external auth mode"
                );
                None
            }
            Err(err) if use_gateway_api_key_auth => {
                tracing::warn!(
                    error = %err,
                    "failed to load OpenAI Codex profile; continuing with custom endpoint API key mode"
                );
                None
            }
            Err(err) => return Err(err),
        };
        let oauth_access_token = match self
            .auth
            .get_valid_openai_access_token(self.auth_profile_override.as_deref())
            .await
        {
            Ok(token) => token,
            Err(err) if external_auth.is_some() => {
                tracing::warn!(
                    error = %err,
                    "failed to refresh OpenAI token from auth store; continuing with external auth mode"
                );
                None
            }
            Err(err) if use_gateway_api_key_auth => {
                tracing::warn!(
                    error = %err,
                    "failed to refresh OpenAI token; continuing with custom endpoint API key mode"
                );
                None
            }
            Err(err) => return Err(err),
        };

        let external_access_token = external_auth.as_ref().map(|(token, _)| token.clone());
        let external_account_id = external_auth
            .as_ref()
            .and_then(|(_, account_id)| account_id.clone());
        let access_token_for_account = oauth_access_token
            .as_deref()
            .or(external_access_token.as_deref());
        let account_id = profile
            .and_then(|profile| profile.account_id)
            .or_else(|| access_token_for_account.and_then(extract_account_id_from_jwt))
            .or(external_account_id);
        let access_token = if use_gateway_api_key_auth {
            oauth_access_token.or(external_access_token)
        } else {
            Some(oauth_access_token.or(external_access_token).ok_or_else(|| {
                anyhow::anyhow!(
                    "OpenAI Codex auth profile not found. Run `synapseclaw auth login --provider openai-codex`, set SYNAPSECLAW_OPENAI_CODEX_ACCESS_TOKEN, or provide ~/.codex/auth.json."
                )
            })?)
        };
        let account_id = if use_gateway_api_key_auth {
            account_id
        } else {
            Some(account_id.ok_or_else(|| {
                anyhow::anyhow!(
                    "OpenAI Codex account id not found in auth profile/token. Run `synapseclaw auth login --provider openai-codex` again or set SYNAPSECLAW_OPENAI_CODEX_ACCOUNT_ID."
                )
            })?)
        };
        let bearer_token = if use_gateway_api_key_auth {
            self.gateway_api_key.as_deref().unwrap_or_default()
        } else {
            access_token.as_deref().unwrap_or_default()
        };

        let mut request_builder = self
            .client
            .post(&self.responses_url)
            .header("Authorization", format!("Bearer {bearer_token}"))
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "pi")
            .header(
                "accept",
                if request.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .header("Content-Type", "application/json");

        if let Some(account_id) = account_id.as_deref() {
            request_builder = request_builder.header("chatgpt-account-id", account_id);
        }

        if use_gateway_api_key_auth {
            if let Some(access_token) = access_token.as_deref() {
                request_builder = request_builder.header("x-openai-access-token", access_token);
            }
            if let Some(account_id) = account_id.as_deref() {
                request_builder = request_builder.header("x-openai-account-id", account_id);
            }
        }

        let response = request_builder.json(&request).send().await?;

        if !response.status().is_success() {
            return Err(super::api_error("OpenAI Codex", response).await);
        }

        Ok(response)
    }

    async fn send_responses_text_request(
        &self,
        input: Vec<Value>,
        instructions: String,
        model: &str,
    ) -> anyhow::Result<String> {
        let normalized_model = normalize_model_id(model);
        let request = ResponsesRequest {
            model: normalized_model.to_string(),
            input,
            instructions,
            store: false,
            stream: true,
            text: ResponsesTextOptions {
                verbosity: "medium".to_string(),
            },
            reasoning: ResponsesReasoningOptions {
                effort: resolve_reasoning_effort(
                    normalized_model,
                    self.reasoning_effort.as_deref(),
                ),
                summary: "auto".to_string(),
            },
            include: vec!["reasoning.encrypted_content".to_string()],
            tools: None,
            tool_choice: None,
            parallel_tool_calls: false,
        };

        decode_responses_body(self.send_request(&request).await?).await
    }

    async fn send_responses_chat_request(
        &self,
        input: Vec<Value>,
        instructions: String,
        tools: Option<Vec<ResponsesToolSpec>>,
        model: &str,
    ) -> anyhow::Result<ProviderChatResponse> {
        let normalized_model = normalize_model_id(model);
        let request = ResponsesRequest {
            model: normalized_model.to_string(),
            input,
            instructions,
            store: false,
            stream: true,
            text: ResponsesTextOptions {
                verbosity: "medium".to_string(),
            },
            reasoning: ResponsesReasoningOptions {
                effort: resolve_reasoning_effort(
                    normalized_model,
                    self.reasoning_effort.as_deref(),
                ),
                summary: "auto".to_string(),
            },
            include: vec!["reasoning.encrypted_content".to_string()],
            tool_choice: tools.as_ref().map(|_| "auto".to_string()),
            tools,
            parallel_tool_calls: true,
        };

        let response = self.send_request(&request).await?;
        let body = response
            .text()
            .await
            .map_err(|err| anyhow::anyhow!("error reading OpenAI Codex response body: {err}"))?;
        let parsed = if let Some(parsed) = parse_sse_response(&body)? {
            parsed
        } else {
            serde_json::from_str(&body).map_err(|err| {
                anyhow::anyhow!(
                    "OpenAI Codex JSON parse failed: {err}. Payload: {}",
                    super::sanitize_api_error(&body)
                )
            })?
        };
        Ok(parse_responses_chat_response(parsed))
    }
}

#[async_trait]
impl Provider for OpenAiCodexProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            native_tool_calling: true,
            vision: true,
            prompt_caching: false,
        }
    }

    async fn chat(
        &self,
        request: ProviderChatRequest<'_>,
        model: &str,
        _temperature: f64,
    ) -> anyhow::Result<ProviderChatResponse> {
        let config = synapse_domain::config::schema::MultimodalConfig::default();
        let prepared =
            crate::multimodal::prepare_messages_for_provider(request.messages, &config).await?;
        let (instructions, input) = build_responses_input(&prepared.messages);
        let tools = convert_tools(request.tools);
        self.send_responses_chat_request(input, instructions, tools, model)
            .await
    }

    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        // Build temporary messages array
        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(ChatMessage::system(sys));
        }
        messages.push(ChatMessage::user(message));

        // Normalize images: convert file paths to data URIs
        let config = synapse_domain::config::schema::MultimodalConfig::default();
        let prepared = crate::multimodal::prepare_messages_for_provider(&messages, &config).await?;

        let (instructions, input) = build_responses_input(&prepared.messages);
        self.send_responses_text_request(input, instructions, model)
            .await
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        // Normalize image markers: convert file paths to data URIs
        let config = synapse_domain::config::schema::MultimodalConfig::default();
        let prepared = crate::multimodal::prepare_messages_for_provider(messages, &config).await?;

        let (instructions, input) = build_responses_input(&prepared.messages);
        self.send_responses_text_request(input, instructions, model)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var(key).ok();
            match value {
                Some(next) => std::env::set_var(key, next),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(original) = self.original.as_deref() {
                std::env::set_var(self.key, original);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn extracts_output_text_first() {
        let response = ResponsesResponse {
            output: vec![],
            output_text: Some("hello".into()),
            ..ResponsesResponse::default()
        };
        assert_eq!(extract_responses_text(&response).as_deref(), Some("hello"));
    }

    #[test]
    fn extracts_nested_output_text() {
        let response = ResponsesResponse {
            output: vec![ResponsesOutput {
                kind: Some("message".into()),
                content: vec![ResponsesContent {
                    kind: Some("output_text".into()),
                    text: Some("nested".into()),
                    ..ResponsesContent::default()
                }],
                ..ResponsesOutput::default()
            }],
            output_text: None,
            ..ResponsesResponse::default()
        };
        assert_eq!(extract_responses_text(&response).as_deref(), Some("nested"));
    }

    #[test]
    fn default_state_dir_is_non_empty() {
        let path = default_synapseclaw_dir();
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn build_responses_url_appends_suffix_for_base_url() {
        assert_eq!(
            build_responses_url("https://api.tonsof.blue/v1").unwrap(),
            "https://api.tonsof.blue/v1/responses"
        );
    }

    #[test]
    fn build_responses_url_keeps_existing_responses_endpoint() {
        assert_eq!(
            build_responses_url("https://api.tonsof.blue/v1/responses").unwrap(),
            "https://api.tonsof.blue/v1/responses"
        );
    }

    #[test]
    fn resolve_responses_url_prefers_explicit_endpoint_env() {
        let _endpoint_guard = EnvGuard::set(
            CODEX_RESPONSES_URL_ENV,
            Some("https://env.example.com/v1/responses"),
        );
        let _base_guard = EnvGuard::set(CODEX_BASE_URL_ENV, Some("https://base.example.com/v1"));

        let options = ProviderRuntimeOptions::default();
        assert_eq!(
            resolve_responses_url(&options).unwrap(),
            "https://env.example.com/v1/responses"
        );
    }

    #[test]
    fn resolve_responses_url_uses_provider_api_url_override() {
        let _endpoint_guard = EnvGuard::set(CODEX_RESPONSES_URL_ENV, None);
        let _base_guard = EnvGuard::set(CODEX_BASE_URL_ENV, None);

        let options = ProviderRuntimeOptions {
            provider_api_url: Some("https://proxy.example.com/v1".to_string()),
            ..ProviderRuntimeOptions::default()
        };

        assert_eq!(
            resolve_responses_url(&options).unwrap(),
            "https://proxy.example.com/v1/responses"
        );
    }

    #[test]
    fn default_responses_url_detector_handles_equivalent_urls() {
        assert!(is_default_responses_url(DEFAULT_CODEX_RESPONSES_URL));
        assert!(is_default_responses_url(
            "https://chatgpt.com/backend-api/codex/responses/"
        ));
        assert!(!is_default_responses_url(
            "https://api.tonsof.blue/v1/responses"
        ));
    }

    #[test]
    fn constructor_enables_custom_endpoint_key_mode() {
        let options = ProviderRuntimeOptions {
            provider_api_url: Some("https://api.tonsof.blue/v1".to_string()),
            ..ProviderRuntimeOptions::default()
        };

        let provider = OpenAiCodexProvider::new(&options, Some("test-key")).unwrap();
        assert!(provider.custom_endpoint);
        assert_eq!(provider.gateway_api_key.as_deref(), Some("test-key"));
    }

    #[test]
    fn resolve_instructions_uses_default_when_missing() {
        assert_eq!(
            resolve_instructions(None),
            DEFAULT_CODEX_INSTRUCTIONS.to_string()
        );
    }

    #[test]
    fn resolve_instructions_uses_default_when_blank() {
        assert_eq!(
            resolve_instructions(Some("   ")),
            DEFAULT_CODEX_INSTRUCTIONS.to_string()
        );
    }

    #[test]
    fn resolve_instructions_uses_system_prompt_when_present() {
        assert_eq!(
            resolve_instructions(Some("Be strict")),
            "Be strict".to_string()
        );
    }

    #[test]
    fn clamp_reasoning_effort_adjusts_known_models() {
        assert_eq!(
            clamp_reasoning_effort("gpt-5-codex", "xhigh"),
            "high".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5-codex", "minimal"),
            "low".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5-codex", "medium"),
            "medium".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5.3-codex", "minimal"),
            "low".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5.1", "xhigh"),
            "high".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5-codex", "xhigh"),
            "high".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5.1-codex-mini", "low"),
            "medium".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5.1-codex-mini", "xhigh"),
            "high".to_string()
        );
        assert_eq!(
            clamp_reasoning_effort("gpt-5.3-codex", "xhigh"),
            "xhigh".to_string()
        );
    }

    #[test]
    fn resolve_reasoning_effort_prefers_configured_override() {
        let _guard = EnvGuard::set("SYNAPSECLAW_CODEX_REASONING_EFFORT", Some("low"));
        assert_eq!(
            resolve_reasoning_effort("gpt-5-codex", Some("high")),
            "high".to_string()
        );
    }

    #[test]
    fn resolve_reasoning_effort_uses_legacy_env_when_unconfigured() {
        let _guard = EnvGuard::set("SYNAPSECLAW_CODEX_REASONING_EFFORT", Some("minimal"));
        assert_eq!(
            resolve_reasoning_effort("gpt-5-codex", None),
            "low".to_string()
        );
    }

    #[test]
    fn parse_sse_text_reads_output_text_delta() {
        let payload = r#"data: {"type":"response.created","response":{"id":"resp_123"}}

data: {"type":"response.output_text.delta","delta":"Hello"}
data: {"type":"response.output_text.delta","delta":" world"}
data: {"type":"response.completed","response":{"output_text":"Hello world"}}
data: [DONE]
"#;

        assert_eq!(
            parse_sse_text(payload).unwrap().as_deref(),
            Some("Hello world")
        );
    }

    #[test]
    fn parse_sse_text_falls_back_to_completed_response() {
        let payload = r#"data: {"type":"response.completed","response":{"output_text":"Done"}}
data: [DONE]
"#;

        assert_eq!(parse_sse_text(payload).unwrap().as_deref(), Some("Done"));
    }

    #[test]
    fn decode_utf8_stream_chunks_handles_multibyte_split_across_chunks() {
        let payload =
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello 世\"}\n\ndata: [DONE]\n";
        let bytes = payload.as_bytes();
        let split_at = payload.find('世').unwrap() + 1;

        let decoded = decode_utf8_stream_chunks([&bytes[..split_at], &bytes[split_at..]]).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(
            parse_sse_text(&decoded).unwrap().as_deref(),
            Some("Hello 世")
        );
    }

    #[test]
    fn build_responses_input_maps_content_types_by_role() {
        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: "You are helpful.".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Hi".into(),
            },
            ChatMessage {
                role: "assistant".into(),
                content: "Hello!".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Thanks".into(),
            },
        ];
        let (instructions, input) = build_responses_input(&messages);
        assert_eq!(instructions, "You are helpful.");
        assert_eq!(input.len(), 3);

        let json: Vec<Value> = input
            .iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect();
        assert_eq!(json[0]["role"], "user");
        assert_eq!(json[0]["content"][0]["type"], "input_text");
        assert_eq!(json[1]["role"], "assistant");
        assert_eq!(json[1]["content"][0]["type"], "output_text");
        assert_eq!(json[2]["role"], "user");
        assert_eq!(json[2]["content"][0]["type"], "input_text");
    }

    #[test]
    fn build_responses_input_uses_default_instructions_without_system() {
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: "Hello".into(),
        }];
        let (instructions, input) = build_responses_input(&messages);
        assert_eq!(instructions, DEFAULT_CODEX_INSTRUCTIONS);
        assert_eq!(input.len(), 1);
    }

    #[test]
    fn build_responses_input_ignores_unknown_roles() {
        let messages = vec![
            ChatMessage {
                role: "moderator".into(),
                content: "result".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Go".into(),
            },
        ];
        let (instructions, input) = build_responses_input(&messages);
        assert_eq!(instructions, DEFAULT_CODEX_INSTRUCTIONS);
        assert_eq!(input.len(), 1);
        let json = serde_json::to_value(&input[0]).unwrap();
        assert_eq!(json["role"], "user");
    }

    #[test]
    fn build_responses_input_handles_image_markers() {
        let messages = vec![ChatMessage::user(
            "Describe this\n\n[IMAGE:data:image/png;base64,abc]",
        )];
        let (_, input) = build_responses_input(&messages);

        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        let json = input[0]["content"].as_array().unwrap();
        assert_eq!(json.len(), 2);

        // First content = text
        assert_eq!(json[0]["type"], "input_text");
        assert!(json[0]["text"].as_str().unwrap().contains("Describe this"));

        // Second content = image
        assert_eq!(json[1]["type"], "input_image");
        assert_eq!(json[1]["image_url"], "data:image/png;base64,abc");
    }

    #[test]
    fn build_responses_input_preserves_text_only_messages() {
        let messages = vec![ChatMessage::user("Hello without images")];
        let (_, input) = build_responses_input(&messages);

        assert_eq!(input.len(), 1);
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);

        let json = &content[0];
        assert_eq!(json["type"], "input_text");
        assert_eq!(json["text"], "Hello without images");
    }

    #[test]
    fn build_responses_input_handles_multiple_images() {
        let messages = vec![ChatMessage::user(
            "Compare these: [IMAGE:data:image/png;base64,img1] and [IMAGE:data:image/jpeg;base64,img2]",
        )];
        let (_, input) = build_responses_input(&messages);

        assert_eq!(input.len(), 1);
        let json = input[0]["content"].as_array().unwrap();
        assert_eq!(json.len(), 3); // text + 2 images

        assert_eq!(json[0]["type"], "input_text");
        assert_eq!(json[1]["type"], "input_image");
        assert_eq!(json[2]["type"], "input_image");
    }

    #[test]
    fn build_responses_input_replays_native_tool_history() {
        let messages = vec![
            ChatMessage::assistant(
                serde_json::json!({
                    "content": "Checking now",
                    "tool_calls": [{
                        "id": "call_123",
                        "name": "shell",
                        "arguments": "{\"command\":\"uptime\"}",
                    }],
                })
                .to_string(),
            ),
            ChatMessage::tool(
                serde_json::json!({
                    "tool_call_id": "call_123",
                    "content": "load average: 0.00",
                })
                .to_string(),
            ),
        ];

        let (_, input) = build_responses_input(&messages);
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_123");
        assert_eq!(input[1]["name"], "shell");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_123");
        assert_eq!(input[2]["output"], "load average: 0.00");
    }

    #[test]
    fn parse_responses_chat_response_extracts_native_tool_calls() {
        let response = ResponsesResponse {
            output: vec![ResponsesOutput {
                kind: Some("function_call".into()),
                call_id: Some("call_abc".into()),
                name: Some("shell".into()),
                arguments: Some("{\"command\":\"date\"}".into()),
                ..ResponsesOutput::default()
            }],
            usage: Some(ResponsesUsage {
                input_tokens: Some(10),
                output_tokens: Some(4),
                input_tokens_details: Some(ResponsesInputTokensDetails {
                    cached_tokens: Some(3),
                }),
            }),
            ..ResponsesResponse::default()
        };

        let parsed = parse_responses_chat_response(response);
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "call_abc");
        assert_eq!(parsed.tool_calls[0].name, "shell");
        assert_eq!(parsed.tool_calls[0].arguments, "{\"command\":\"date\"}");
        assert_eq!(parsed.usage.as_ref().and_then(|u| u.input_tokens), Some(10));
        assert_eq!(
            parsed.usage.as_ref().and_then(|u| u.cached_input_tokens),
            Some(3)
        );
    }

    #[test]
    fn normalize_strict_tool_schema_closes_object_nodes() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "options": {
                    "type": "object",
                    "properties": {
                        "cwd": { "type": "string" }
                    }
                }
            },
            "required": ["command"]
        });

        let normalized = normalize_strict_tool_schema(schema);
        assert_eq!(normalized["additionalProperties"], Value::Bool(false));
        assert_eq!(
            normalized["properties"]["options"]["additionalProperties"],
            Value::Bool(false)
        );
    }

    #[test]
    fn convert_tools_normalizes_parameters_for_strict_mode() {
        let tools = vec![ToolSpec {
            name: "shell".to_string(),
            description: "Run a shell command".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }),
        }];

        let converted = convert_tools(Some(&tools)).expect("tool specs");
        assert_eq!(converted.len(), 1);
        assert_eq!(
            converted[0].parameters["additionalProperties"],
            Value::Bool(false)
        );
    }

    #[test]
    fn capabilities_includes_vision() {
        let options = ProviderRuntimeOptions {
            provider_api_url: None,
            synapseclaw_dir: None,
            secrets_encrypt: false,
            auth_profile_override: None,
            reasoning_enabled: None,
            reasoning_effort: None,
            provider_timeout_secs: None,
            extra_headers: std::collections::HashMap::new(),
            api_path: None,
            prompt_caching: false,
        };
        let provider =
            OpenAiCodexProvider::new(&options, None).expect("provider should initialize");
        let caps = provider.capabilities();

        assert!(caps.native_tool_calling);
        assert!(caps.vision);
    }
}
