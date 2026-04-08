//! Tool execution engine — agent turn loop, tool dispatch, parallel execution.

use super::tool_call_parsing::*;
use super::*;
use synapse_domain::application::services::loop_detection::{
    hash_args, LoopAction, LoopDetector, ToolInvocation,
};
use synapse_domain::domain::tool_fact::{OutcomeStatus, TypedToolFact};

#[derive(Debug)]
pub(crate) struct ToolLoopCancelled;

impl std::fmt::Display for ToolLoopCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tool loop cancelled")
    }
}

impl std::error::Error for ToolLoopCancelled {}

pub(crate) fn is_tool_loop_cancelled(err: &anyhow::Error) -> bool {
    err.chain().any(|source| source.is::<ToolLoopCancelled>())
}

/// Execute a single turn of the agent loop: send messages, parse tool calls,
/// execute tools, and loop until the LLM produces a final text response.
/// When `silent` is true, suppresses stdout (for channel use).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn agent_turn(
    provider: &dyn Provider,
    history: &mut Vec<ChatMessage>,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    provider_name: &str,
    model: &str,
    temperature: f64,
    silent: bool,
    channel_name: &str,
    multimodal_config: &synapse_domain::config::schema::MultimodalConfig,
    max_tool_iterations: usize,
    excluded_tools: &[String],
    dedup_exempt_tools: &[String],
    activated_tools: Option<&std::sync::Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
) -> Result<String> {
    run_tool_call_loop(
        provider,
        history,
        tools_registry,
        observer,
        provider_name,
        model,
        temperature,
        silent,
        None,
        channel_name,
        multimodal_config,
        max_tool_iterations,
        None,
        None,
        None,
        excluded_tools,
        dedup_exempt_tools,
        activated_tools,
        None,
    )
    .await
    .map(|result| result.response)
}

#[derive(Debug)]
pub(crate) struct ToolLoopResult {
    pub(crate) response: String,
    pub(crate) tool_names: Vec<String>,
    pub(crate) tool_facts: Vec<TypedToolFact>,
}

fn collect_tool_facts(
    tool_name: &str,
    status: OutcomeStatus,
    duration: Duration,
    explicit_facts: Vec<TypedToolFact>,
) -> Vec<TypedToolFact> {
    let mut facts = explicit_facts;
    let duration_ms = u64::try_from(duration.as_millis()).ok();
    facts.push(TypedToolFact::outcome(tool_name, status, duration_ms));
    facts
}

pub(crate) async fn execute_one_tool(
    call_name: &str,
    call_arguments: serde_json::Value,
    tools_registry: &[Box<dyn Tool>],
    activated_tools: Option<&std::sync::Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
    run_ctx: Option<&std::sync::Arc<crate::agent::run_context::RunContext>>,
    tool_middleware: Option<
        &std::sync::Arc<
            synapse_domain::application::services::tool_middleware_service::ToolMiddlewareChain,
        >,
    >,
) -> Result<ToolExecutionOutcome> {
    let args_summary = truncate_with_ellipsis(&call_arguments.to_string(), 300);
    observer.record_event(&ObserverEvent::ToolCallStart {
        tool: call_name.to_string(),
        arguments: Some(args_summary),
    });
    let start = Instant::now();

    let static_tool = find_tool(tools_registry, call_name);
    let activated_arc = if static_tool.is_none() {
        activated_tools.and_then(|at| at.lock().unwrap().get_resolved(call_name))
    } else {
        None
    };
    let Some(tool) = static_tool.or(activated_arc.as_deref()) else {
        let reason = format!("Unknown tool: {call_name}");
        let duration = start.elapsed();
        let tool_facts =
            collect_tool_facts(call_name, OutcomeStatus::UnknownTool, duration, Vec::new());
        observer.record_event(&ObserverEvent::ToolCall {
            tool: call_name.to_string(),
            duration,
            success: false,
        });
        observer.record_event(&ObserverEvent::ToolResult {
            tool: call_name.to_string(),
            output: reason.clone(),
            success: false,
        });
        return Ok(ToolExecutionOutcome {
            output: reason.clone(),
            success: false,
            error_reason: Some(scrub_credentials(&reason)),
            duration,
            tool_facts,
        });
    };

    // Snapshot IPC tool args before execute() consumes them (for per-session tracking).
    let ipc_args = if run_ctx.is_some() && matches!(call_name, "agents_reply" | "agents_send") {
        Some(call_arguments.clone())
    } else {
        None
    };

    // Phase 4.1: Tool middleware before() hook
    if let Some(mw) = tool_middleware {
        let mw_ctx = synapse_domain::domain::tool_middleware::ToolCallContext {
            run_id: None,
            pipeline_name: None,
            step_id: None,
            agent_id: String::new(),
            tool_name: call_name.to_string(),
            args: call_arguments.clone(),
            call_count: 0,
        };
        if let Err(block) = mw.run_before(&mw_ctx).await {
            let reason = block.to_string();
            let duration = start.elapsed();
            observer.record_event(&ObserverEvent::ToolCall {
                tool: call_name.to_string(),
                duration,
                success: false,
            });
            observer.record_event(&ObserverEvent::ToolResult {
                tool: call_name.to_string(),
                output: format!("[blocked] {reason}"),
                success: false,
            });
            let tool_facts =
                collect_tool_facts(call_name, OutcomeStatus::Blocked, duration, Vec::new());
            return Ok(ToolExecutionOutcome {
                output: format!("[blocked] {reason}"),
                success: false,
                error_reason: Some(reason),
                duration,
                tool_facts,
            });
        }
    }

    let tool_future = tool.execute_with_facts(call_arguments.clone());
    let tool_result = if let Some(token) = cancellation_token {
        tokio::select! {
            () = token.cancelled() => return Err(ToolLoopCancelled.into()),
            result = tool_future => result,
        }
    } else {
        tool_future.await
    };

    match tool_result {
        Ok(execution) => {
            let duration = start.elapsed();
            let r = execution.result;
            let tool_facts = collect_tool_facts(
                call_name,
                if r.success {
                    OutcomeStatus::Succeeded
                } else {
                    OutcomeStatus::ReportedFailure
                },
                duration,
                execution.facts,
            );
            observer.record_event(&ObserverEvent::ToolCall {
                tool: call_name.to_string(),
                duration,
                success: r.success,
            });
            if let Some(ctx) = run_ctx {
                ctx.record_tool_call(call_name, r.success, ipc_args.as_ref());
            }
            if r.success {
                let output = scrub_credentials(&r.output);
                observer.record_event(&ObserverEvent::ToolResult {
                    tool: call_name.to_string(),
                    output: truncate_with_ellipsis(&output, 500),
                    success: true,
                });
                Ok(ToolExecutionOutcome {
                    output,
                    success: true,
                    error_reason: None,
                    duration,
                    tool_facts,
                })
            } else {
                let reason = r.error.unwrap_or(r.output);
                observer.record_event(&ObserverEvent::ToolResult {
                    tool: call_name.to_string(),
                    output: truncate_with_ellipsis(&reason, 500),
                    success: false,
                });
                Ok(ToolExecutionOutcome {
                    output: format!("Error: {reason}"),
                    success: false,
                    error_reason: Some(scrub_credentials(&reason)),
                    duration,
                    tool_facts,
                })
            }
        }
        Err(e) => {
            let duration = start.elapsed();
            let tool_facts =
                collect_tool_facts(call_name, OutcomeStatus::RuntimeError, duration, Vec::new());
            observer.record_event(&ObserverEvent::ToolCall {
                tool: call_name.to_string(),
                duration,
                success: false,
            });
            if let Some(ctx) = run_ctx {
                ctx.record_tool_call(call_name, false, ipc_args.as_ref());
            }
            let reason = format!("Error executing {call_name}: {e}");
            observer.record_event(&ObserverEvent::ToolResult {
                tool: call_name.to_string(),
                output: truncate_with_ellipsis(&reason, 500),
                success: false,
            });
            Ok(ToolExecutionOutcome {
                output: reason.clone(),
                success: false,
                error_reason: Some(scrub_credentials(&reason)),
                duration,
                tool_facts,
            })
        }
    }
}

pub(crate) struct ToolExecutionOutcome {
    pub(crate) output: String,
    pub(crate) success: bool,
    pub(crate) error_reason: Option<String>,
    pub(crate) duration: Duration,
    pub(crate) tool_facts: Vec<TypedToolFact>,
}

pub(crate) fn should_execute_tools_in_parallel(
    tool_calls: &[ParsedToolCall],
    approval: Option<&dyn ApprovalPort>,
) -> bool {
    if tool_calls.len() <= 1 {
        return false;
    }

    if let Some(port) = approval {
        if tool_calls
            .iter()
            .any(|call| port.needs_approval(&call.name))
        {
            // Approval-gated calls must keep sequential handling so the caller can
            // enforce CLI prompt/deny policy consistently.
            return false;
        }
    }

    true
}

async fn execute_tools_parallel(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    activated_tools: Option<&std::sync::Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
    run_ctx: Option<&std::sync::Arc<crate::agent::run_context::RunContext>>,
    tool_middleware: Option<
        &std::sync::Arc<
            synapse_domain::application::services::tool_middleware_service::ToolMiddlewareChain,
        >,
    >,
) -> Result<Vec<ToolExecutionOutcome>> {
    let futures: Vec<_> = tool_calls
        .iter()
        .map(|call| {
            execute_one_tool(
                &call.name,
                call.arguments.clone(),
                tools_registry,
                activated_tools,
                observer,
                cancellation_token,
                run_ctx,
                tool_middleware,
            )
        })
        .collect();

    let results = futures_util::future::join_all(futures).await;
    results.into_iter().collect()
}

async fn execute_tools_sequential(
    tool_calls: &[ParsedToolCall],
    tools_registry: &[Box<dyn Tool>],
    activated_tools: Option<&std::sync::Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
    observer: &dyn Observer,
    cancellation_token: Option<&CancellationToken>,
    run_ctx: Option<&std::sync::Arc<crate::agent::run_context::RunContext>>,
    tool_middleware: Option<
        &std::sync::Arc<
            synapse_domain::application::services::tool_middleware_service::ToolMiddlewareChain,
        >,
    >,
) -> Result<Vec<ToolExecutionOutcome>> {
    let mut outcomes = Vec::with_capacity(tool_calls.len());

    for call in tool_calls {
        outcomes.push(
            execute_one_tool(
                &call.name,
                call.arguments.clone(),
                tools_registry,
                activated_tools,
                observer,
                cancellation_token,
                run_ctx,
                tool_middleware,
            )
            .await?,
        );
    }

    Ok(outcomes)
}

// ── Agent Tool-Call Loop ──────────────────────────────────────────────────
// Core agentic iteration: send conversation to the LLM, parse any tool
// calls from the response, execute them, append results to history, and
// repeat until the LLM produces a final text-only answer.
//
// Loop invariant: at the start of each iteration, `history` contains the
// full conversation so far (system prompt + user messages + prior tool
// results). The loop exits when:
//   • the LLM returns no tool calls (final answer), or
//   • max_iterations is reached (runaway safety), or
//   • the cancellation token fires (external abort).

/// Execute a single turn of the agent loop: send messages, parse tool calls,
/// execute tools, and loop until the LLM produces a final text response.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_tool_call_loop(
    provider: &dyn Provider,
    history: &mut Vec<ChatMessage>,
    tools_registry: &[Box<dyn Tool>],
    observer: &dyn Observer,
    provider_name: &str,
    model: &str,
    temperature: f64,
    silent: bool,
    approval: Option<&dyn ApprovalPort>,
    channel_name: &str,
    multimodal_config: &synapse_domain::config::schema::MultimodalConfig,
    max_tool_iterations: usize,
    cancellation_token: Option<CancellationToken>,
    on_delta: Option<tokio::sync::mpsc::Sender<String>>,
    hooks: Option<&crate::hooks::HookRunner>,
    excluded_tools: &[String],
    dedup_exempt_tools: &[String],
    activated_tools: Option<&std::sync::Arc<std::sync::Mutex<crate::tools::ActivatedToolSet>>>,
    run_ctx: Option<&std::sync::Arc<crate::agent::run_context::RunContext>>,
) -> Result<ToolLoopResult> {
    let max_iterations = if max_tool_iterations == 0 {
        DEFAULT_MAX_TOOL_ITERATIONS
    } else {
        max_tool_iterations
    };

    let turn_id = Uuid::new_v4().to_string();
    let turn_start = std::time::Instant::now();
    let mut seen_tool_signatures: HashSet<(String, String)> = HashSet::new();
    let mut total_tool_calls = 0usize;
    let mut loop_detector = LoopDetector::new();
    let mut collected_tool_facts = Vec::<TypedToolFact>::new();
    let mut collected_tool_names = Vec::<String>::new();

    tracing::info!(
        model,
        channel = channel_name,
        max_iterations,
        "agent.turn.start"
    );

    for iteration in 0..max_iterations {
        if cancellation_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ToolLoopCancelled.into());
        }

        // Rebuild tool_specs each iteration so newly activated deferred tools appear.
        let mut tool_specs: Vec<crate::tools::ToolSpec> = tools_registry
            .iter()
            .filter(|tool| !excluded_tools.iter().any(|ex| ex == tool.name()))
            .map(|tool| tool.spec())
            .collect();
        if let Some(at) = activated_tools {
            for spec in at.lock().unwrap().tool_specs() {
                if !excluded_tools.iter().any(|ex| ex == &spec.name) {
                    tool_specs.push(spec);
                }
            }
        }
        let use_native_tools = provider.supports_native_tools() && !tool_specs.is_empty();

        let image_marker_count = multimodal::count_image_markers(history);
        if image_marker_count > 0 && !provider.supports_vision() {
            return Err(ProviderCapabilityError {
                provider: provider_name.to_string(),
                capability: "vision".to_string(),
                message: format!(
                    "received {image_marker_count} image marker(s), but this provider does not support vision input"
                ),
            }
            .into());
        }

        let prepared_messages =
            multimodal::prepare_messages_for_provider(history, multimodal_config).await?;

        // ── Progress: LLM thinking ────────────────────────────
        if let Some(ref tx) = on_delta {
            let phase = if iteration == 0 {
                "\u{1f914} Thinking...\n".to_string()
            } else {
                format!("\u{1f914} Thinking (round {})...\n", iteration + 1)
            };
            let _ = tx.send(phase).await;
        }

        observer.record_event(&ObserverEvent::LlmRequest {
            provider: provider_name.to_string(),
            model: model.to_string(),
            messages_count: history.len(),
        });
        runtime_trace::record_event(
            "llm_request",
            Some(channel_name),
            Some(provider_name),
            Some(model),
            Some(&turn_id),
            None,
            None,
            serde_json::json!({
                "iteration": iteration + 1,
                "messages_count": history.len(),
            }),
        );

        let llm_started_at = Instant::now();

        // Fire void hook before LLM call
        if let Some(hooks) = hooks {
            hooks.fire_llm_input(history, model).await;
        }

        // Unified path via Provider::chat so provider-specific native tool logic
        // (OpenAI/Anthropic/OpenRouter/compatible adapters) is honored.
        let request_tools = if use_native_tools {
            Some(tool_specs.as_slice())
        } else {
            None
        };

        let chat_future = provider.chat(
            ChatRequest {
                messages: &prepared_messages.messages,
                tools: request_tools,
            },
            model,
            temperature,
        );

        let chat_result = if let Some(token) = cancellation_token.as_ref() {
            tokio::select! {
                () = token.cancelled() => return Err(ToolLoopCancelled.into()),
                result = chat_future => result,
            }
        } else {
            chat_future.await
        };

        let (response_text, parsed_text, tool_calls, assistant_history_content, native_tool_calls) =
            match chat_result {
                Ok(resp) => {
                    let (resp_input_tokens, resp_output_tokens) = resp
                        .usage
                        .as_ref()
                        .map(|u| (u.input_tokens, u.output_tokens))
                        .unwrap_or((None, None));

                    observer.record_event(&ObserverEvent::LlmResponse {
                        provider: provider_name.to_string(),
                        model: model.to_string(),
                        duration: llm_started_at.elapsed(),
                        success: true,
                        error_message: None,
                        input_tokens: resp_input_tokens,
                        output_tokens: resp_output_tokens,
                    });

                    let response_text = resp.text_or_empty().to_string();
                    // First try native structured tool calls (OpenAI-format).
                    // Fall back to text-based parsing (XML tags, markdown blocks,
                    // GLM format) only if the provider returned no native calls —
                    // this ensures we support both native and prompt-guided models.
                    let mut calls = parse_structured_tool_calls(&resp.tool_calls);
                    let mut parsed_text = String::new();

                    if calls.is_empty() {
                        let (fallback_text, fallback_calls) = parse_tool_calls(&response_text);
                        if !fallback_text.is_empty() {
                            parsed_text = fallback_text;
                        }
                        calls = fallback_calls;
                    }

                    if let Some(parse_issue) = detect_tool_call_parse_issue(&response_text, &calls)
                    {
                        runtime_trace::record_event(
                            "tool_call_parse_issue",
                            Some(channel_name),
                            Some(provider_name),
                            Some(model),
                            Some(&turn_id),
                            Some(false),
                            Some(&parse_issue),
                            serde_json::json!({
                                "iteration": iteration + 1,
                                "response_excerpt": truncate_with_ellipsis(
                                    &scrub_credentials(&response_text),
                                    600
                                ),
                            }),
                        );
                    }

                    runtime_trace::record_event(
                        "llm_response",
                        Some(channel_name),
                        Some(provider_name),
                        Some(model),
                        Some(&turn_id),
                        Some(true),
                        None,
                        serde_json::json!({
                            "iteration": iteration + 1,
                            "duration_ms": llm_started_at.elapsed().as_millis(),
                            "input_tokens": resp_input_tokens,
                            "output_tokens": resp_output_tokens,
                            "raw_response": scrub_credentials(&response_text),
                            "native_tool_calls": resp.tool_calls.len(),
                            "parsed_tool_calls": calls.len(),
                        }),
                    );

                    // Preserve native tool call IDs in assistant history so role=tool
                    // follow-up messages can reference the exact call id.
                    let reasoning_content = resp.reasoning_content.clone();
                    let assistant_history_content = if resp.tool_calls.is_empty() {
                        if use_native_tools {
                            build_native_assistant_history_from_parsed_calls(
                                &response_text,
                                &calls,
                                reasoning_content.as_deref(),
                            )
                            .unwrap_or_else(|| response_text.clone())
                        } else {
                            response_text.clone()
                        }
                    } else {
                        build_native_assistant_history(
                            &response_text,
                            &resp.tool_calls,
                            reasoning_content.as_deref(),
                        )
                    };

                    let native_calls = resp.tool_calls;
                    (
                        response_text,
                        parsed_text,
                        calls,
                        assistant_history_content,
                        native_calls,
                    )
                }
                Err(e) => {
                    let safe_error = synapse_providers::sanitize_api_error(&e.to_string());
                    observer.record_event(&ObserverEvent::LlmResponse {
                        provider: provider_name.to_string(),
                        model: model.to_string(),
                        duration: llm_started_at.elapsed(),
                        success: false,
                        error_message: Some(safe_error.clone()),
                        input_tokens: None,
                        output_tokens: None,
                    });
                    runtime_trace::record_event(
                        "llm_response",
                        Some(channel_name),
                        Some(provider_name),
                        Some(model),
                        Some(&turn_id),
                        Some(false),
                        Some(&safe_error),
                        serde_json::json!({
                            "iteration": iteration + 1,
                            "duration_ms": llm_started_at.elapsed().as_millis(),
                        }),
                    );
                    return Err(e);
                }
            };

        let display_text =
            resolve_display_text(&response_text, &parsed_text, !tool_calls.is_empty());
        let display_text = strip_tool_result_blocks(&display_text);

        // ── Progress: LLM responded ─────────────────────────────
        if let Some(ref tx) = on_delta {
            let llm_secs = llm_started_at.elapsed().as_secs();
            if !tool_calls.is_empty() {
                let _ = tx
                    .send(format!(
                        "\u{1f4ac} Got {} tool call(s) ({llm_secs}s)\n",
                        tool_calls.len()
                    ))
                    .await;
            }
        }

        if tool_calls.is_empty() {
            runtime_trace::record_event(
                "turn_final_response",
                Some(channel_name),
                Some(provider_name),
                Some(model),
                Some(&turn_id),
                Some(true),
                None,
                serde_json::json!({
                    "iteration": iteration + 1,
                    "text": scrub_credentials(&display_text),
                }),
            );
            // No tool calls — this is the final response.
            // If a streaming sender is provided, relay the text in small chunks
            // so the channel can progressively update the draft message.
            if let Some(ref tx) = on_delta {
                // Clear accumulated progress lines before streaming the final answer.
                let _ = tx.send(DRAFT_CLEAR_SENTINEL.to_string()).await;
                // Split on whitespace boundaries, accumulating chunks of at least
                // STREAM_CHUNK_MIN_CHARS characters for progressive draft updates.
                let mut chunk = String::new();
                for word in display_text.split_inclusive(char::is_whitespace) {
                    if cancellation_token
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                    {
                        return Err(ToolLoopCancelled.into());
                    }
                    chunk.push_str(word);
                    if chunk.len() >= STREAM_CHUNK_MIN_CHARS
                        && tx.send(std::mem::take(&mut chunk)).await.is_err()
                    {
                        break; // receiver dropped
                    }
                }
                if !chunk.is_empty() {
                    let _ = tx.send(chunk).await;
                }
            }
            history.push(ChatMessage::assistant(response_text.clone()));
            tracing::info!(
                model,
                iterations = iteration + 1,
                tool_calls = total_tool_calls,
                duration_ms = turn_start.elapsed().as_millis() as u64,
                response_len = display_text.len(),
                "agent.turn.complete"
            );
            return Ok(ToolLoopResult {
                response: display_text,
                tool_names: collected_tool_names,
                tool_facts: collected_tool_facts,
            });
        }

        // Print any text the LLM produced alongside tool calls (unless silent)
        if !silent && !display_text.is_empty() {
            print!("{display_text}");
            let _ = std::io::stdout().flush();
        }

        // Execute tool calls and build results. `individual_results` tracks per-call output so
        // native-mode history can emit one role=tool message per tool call with the correct ID.
        //
        // When multiple tool calls are present and interactive CLI approval is not needed, run
        // tool executions concurrently for lower wall-clock latency.
        tracing::info!(
            iteration,
            tool_count = tool_calls.len(),
            tools = %tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(","),
            "agent.turn.tool_calls"
        );
        total_tool_calls += tool_calls.len();

        let mut tool_results = String::new();
        let mut individual_results: Vec<(Option<String>, String)> = Vec::new();
        let mut ordered_results: Vec<Option<(String, Option<String>, ToolExecutionOutcome)>> =
            (0..tool_calls.len()).map(|_| None).collect();
        let mut loop_action = LoopAction::Continue;
        let allow_parallel_execution = should_execute_tools_in_parallel(&tool_calls, approval);
        let mut executable_indices: Vec<usize> = Vec::new();
        let mut executable_calls: Vec<ParsedToolCall> = Vec::new();

        for (idx, call) in tool_calls.iter().enumerate() {
            // ── Hook: before_tool_call (modifying) ──────────
            let mut tool_name = call.name.clone();
            let mut tool_args = call.arguments.clone();
            if let Some(hooks) = hooks {
                match hooks
                    .run_before_tool_call(tool_name.clone(), tool_args.clone())
                    .await
                {
                    crate::hooks::HookResult::Cancel(reason) => {
                        tracing::info!(tool = %call.name, %reason, "tool call cancelled by hook");
                        let cancelled = format!("Cancelled by hook: {reason}");
                        runtime_trace::record_event(
                            "tool_call_result",
                            Some(channel_name),
                            Some(provider_name),
                            Some(model),
                            Some(&turn_id),
                            Some(false),
                            Some(&cancelled),
                            serde_json::json!({
                                "iteration": iteration + 1,
                                "tool": call.name,
                                "arguments": scrub_credentials(&tool_args.to_string()),
                            }),
                        );
                        if let Some(ref tx) = on_delta {
                            let _ = tx
                                .send(format!(
                                    "\u{274c} {}: {}\n",
                                    call.name,
                                    truncate_with_ellipsis(&scrub_credentials(&cancelled), 200)
                                ))
                                .await;
                        }
                        ordered_results[idx] = Some((
                            call.name.clone(),
                            call.tool_call_id.clone(),
                            ToolExecutionOutcome {
                                output: cancelled,
                                success: false,
                                error_reason: Some(scrub_credentials(&reason)),
                                duration: Duration::ZERO,
                                tool_facts: collect_tool_facts(
                                    &call.name,
                                    OutcomeStatus::Blocked,
                                    Duration::ZERO,
                                    Vec::new(),
                                ),
                            },
                        ));
                        continue;
                    }
                    crate::hooks::HookResult::Continue((name, args)) => {
                        tool_name = name;
                        tool_args = args;
                    }
                }
            }

            // ── Approval hook (Phase 4.0: via ApprovalPort) ──
            if let Some(port) = approval {
                if port.needs_approval(&tool_name) {
                    let args_str = tool_args.to_string();
                    let decision = match port.request_approval(&tool_name, &args_str).await {
                        Ok(resp) => resp,
                        Err(_) => synapse_domain::domain::approval::ApprovalResponse::No,
                    };

                    let audit = synapse_domain::domain::approval::ApprovalDecision {
                        request_id: tool_name.clone(),
                        response: decision,
                        decided_by: "system".into(),
                        channel: channel_name.to_string(),
                        timestamp: chrono::Utc::now().timestamp().cast_unsigned(),
                    };
                    port.record_decision(&audit);

                    if decision == synapse_domain::domain::approval::ApprovalResponse::No {
                        let denied = "Denied by user.".to_string();
                        runtime_trace::record_event(
                            "tool_call_result",
                            Some(channel_name),
                            Some(provider_name),
                            Some(model),
                            Some(&turn_id),
                            Some(false),
                            Some(&denied),
                            serde_json::json!({
                                "iteration": iteration + 1,
                                "tool": tool_name.clone(),
                                "arguments": scrub_credentials(&tool_args.to_string()),
                            }),
                        );
                        if let Some(ref tx) = on_delta {
                            let _ = tx
                                .send(format!("\u{274c} {}: {}\n", tool_name, denied))
                                .await;
                        }
                        ordered_results[idx] = Some((
                            tool_name.clone(),
                            call.tool_call_id.clone(),
                            ToolExecutionOutcome {
                                output: denied.clone(),
                                success: false,
                                error_reason: Some(denied),
                                duration: Duration::ZERO,
                                tool_facts: collect_tool_facts(
                                    &tool_name,
                                    OutcomeStatus::Blocked,
                                    Duration::ZERO,
                                    Vec::new(),
                                ),
                            },
                        ));
                        continue;
                    }
                }
            }

            let signature = tool_call_signature(&tool_name, &tool_args);
            let dedup_exempt = dedup_exempt_tools.iter().any(|e| e == &tool_name);
            if !dedup_exempt && !seen_tool_signatures.insert(signature) {
                let duplicate = format!(
                    "Skipped duplicate tool call '{tool_name}' with identical arguments in this turn."
                );
                runtime_trace::record_event(
                    "tool_call_result",
                    Some(channel_name),
                    Some(provider_name),
                    Some(model),
                    Some(&turn_id),
                    Some(false),
                    Some(&duplicate),
                    serde_json::json!({
                        "iteration": iteration + 1,
                        "tool": tool_name.clone(),
                        "arguments": scrub_credentials(&tool_args.to_string()),
                        "deduplicated": true,
                    }),
                );
                if let Some(ref tx) = on_delta {
                    let _ = tx
                        .send(format!("\u{274c} {}: {}\n", tool_name, duplicate))
                        .await;
                }
                ordered_results[idx] = Some((
                    tool_name.clone(),
                    call.tool_call_id.clone(),
                    ToolExecutionOutcome {
                        output: duplicate.clone(),
                        success: false,
                        error_reason: Some(duplicate),
                        duration: Duration::ZERO,
                        tool_facts: collect_tool_facts(
                            &tool_name,
                            OutcomeStatus::Blocked,
                            Duration::ZERO,
                            Vec::new(),
                        ),
                    },
                ));
                continue;
            }

            runtime_trace::record_event(
                "tool_call_start",
                Some(channel_name),
                Some(provider_name),
                Some(model),
                Some(&turn_id),
                None,
                None,
                serde_json::json!({
                    "iteration": iteration + 1,
                    "tool": tool_name.clone(),
                    "arguments": scrub_credentials(&tool_args.to_string()),
                }),
            );

            // ── Progress: tool start ────────────────────────────
            if let Some(ref tx) = on_delta {
                let hint = truncate_tool_args_for_progress(&tool_name, &tool_args, 60);
                let progress = if hint.is_empty() {
                    format!("\u{23f3} {}\n", tool_name)
                } else {
                    format!("\u{23f3} {}: {hint}\n", tool_name)
                };
                tracing::debug!(tool = %tool_name, "Sending progress start to draft");
                let _ = tx.send(progress).await;
            }

            executable_indices.push(idx);
            executable_calls.push(ParsedToolCall {
                name: tool_name,
                arguments: tool_args,
                tool_call_id: call.tool_call_id.clone(),
            });
        }

        // Phase 4.1: tool_middleware is threaded through but currently None
        // at this call site. Full wiring (from ChannelRuntimeContext) is done
        // when [pipelines] is enabled and middleware is configured.
        let tool_mw: Option<
            &std::sync::Arc<
                synapse_domain::application::services::tool_middleware_service::ToolMiddlewareChain,
            >,
        > = None;

        let executed_outcomes = if allow_parallel_execution && executable_calls.len() > 1 {
            execute_tools_parallel(
                &executable_calls,
                tools_registry,
                activated_tools,
                observer,
                cancellation_token.as_ref(),
                run_ctx,
                tool_mw,
            )
            .await?
        } else {
            execute_tools_sequential(
                &executable_calls,
                tools_registry,
                activated_tools,
                observer,
                cancellation_token.as_ref(),
                run_ctx,
                tool_mw,
            )
            .await?
        };

        for ((idx, call), outcome) in executable_indices
            .iter()
            .zip(executable_calls.iter())
            .zip(executed_outcomes.into_iter())
        {
            let detector_action = loop_detector.record(ToolInvocation {
                tool_name: call.name.clone(),
                args_hash: hash_args(&call.arguments),
                success: outcome.success,
            });
            loop_action = match (loop_action, detector_action) {
                (LoopAction::ForceStop, _) | (_, LoopAction::ForceStop) => LoopAction::ForceStop,
                (LoopAction::SuggestClarify, _) | (_, LoopAction::SuggestClarify) => {
                    LoopAction::SuggestClarify
                }
                _ => LoopAction::Continue,
            };

            runtime_trace::record_event(
                "tool_call_result",
                Some(channel_name),
                Some(provider_name),
                Some(model),
                Some(&turn_id),
                Some(outcome.success),
                outcome.error_reason.as_deref(),
                serde_json::json!({
                    "iteration": iteration + 1,
                    "tool": call.name.clone(),
                    "duration_ms": outcome.duration.as_millis(),
                    "output": scrub_credentials(&outcome.output),
                }),
            );

            // ── Hook: after_tool_call (void) ─────────────────
            if let Some(hooks) = hooks {
                let tool_result_obj = crate::tools::ToolResult {
                    success: outcome.success,
                    output: outcome.output.clone(),
                    error: None,
                };
                hooks
                    .fire_after_tool_call(&call.name, &tool_result_obj, outcome.duration)
                    .await;
            }

            // ── Progress: tool completion ───────────────────────
            if let Some(ref tx) = on_delta {
                let secs = outcome.duration.as_secs();
                let progress_msg = if outcome.success {
                    format!("\u{2705} {} ({secs}s)\n", call.name)
                } else if let Some(ref reason) = outcome.error_reason {
                    format!(
                        "\u{274c} {} ({secs}s): {}\n",
                        call.name,
                        truncate_with_ellipsis(reason, 200)
                    )
                } else {
                    format!("\u{274c} {} ({secs}s)\n", call.name)
                };
                tracing::debug!(tool = %call.name, secs, "Sending progress complete to draft");
                let _ = tx.send(progress_msg).await;
            }

            collected_tool_facts.extend(outcome.tool_facts.clone());
            if !collected_tool_names
                .iter()
                .any(|existing| existing == &call.name)
            {
                collected_tool_names.push(call.name.clone());
            }
            ordered_results[*idx] = Some((call.name.clone(), call.tool_call_id.clone(), outcome));
        }

        for (tool_name, tool_call_id, outcome) in ordered_results.into_iter().flatten() {
            individual_results.push((tool_call_id, outcome.output.clone()));
            let _ = writeln!(
                tool_results,
                "<tool_result name=\"{}\">\n{}\n</tool_result>",
                tool_name, outcome.output
            );
        }

        // Add assistant message with tool calls + tool results to history.
        // Native mode: use JSON-structured messages so convert_messages() can
        // reconstruct proper OpenAI-format tool_calls and tool result messages.
        // Prompt mode: use XML-based text format as before.
        history.push(ChatMessage::assistant(assistant_history_content));
        if native_tool_calls.is_empty() {
            let all_results_have_ids = use_native_tools
                && !individual_results.is_empty()
                && individual_results
                    .iter()
                    .all(|(tool_call_id, _)| tool_call_id.is_some());
            if all_results_have_ids {
                for (tool_call_id, result) in &individual_results {
                    let tool_msg = serde_json::json!({
                        "tool_call_id": tool_call_id,
                        "content": result,
                    });
                    history.push(ChatMessage::tool(tool_msg.to_string()));
                }
            } else {
                history.push(ChatMessage::user(format!("[Tool results]\n{tool_results}")));
            }
        } else {
            for (native_call, (_, result)) in
                native_tool_calls.iter().zip(individual_results.iter())
            {
                let tool_msg = serde_json::json!({
                    "tool_call_id": native_call.id,
                    "content": result,
                });
                history.push(ChatMessage::tool(tool_msg.to_string()));
            }
        }

        match loop_action {
            LoopAction::Continue => {}
            LoopAction::SuggestClarify => {
                let clarify = "I’m repeating the same tool steps without making progress. Please clarify the exact target or desired outcome.";
                history.push(ChatMessage::assistant(clarify));
                tracing::warn!(
                    iterations = iteration + 1,
                    tool_calls = total_tool_calls,
                    "agent.turn.loop_detected_clarify"
                );
                return Ok(ToolLoopResult {
                    response: clarify.to_string(),
                    tool_names: collected_tool_names,
                    tool_facts: collected_tool_facts,
                });
            }
            LoopAction::ForceStop => {
                let stop = "I hit too many tool steps without reaching a stable result. Please narrow the request or specify the exact target.";
                history.push(ChatMessage::assistant(stop));
                tracing::warn!(
                    iterations = iteration + 1,
                    tool_calls = total_tool_calls,
                    "agent.turn.loop_detected_stop"
                );
                return Ok(ToolLoopResult {
                    response: stop.to_string(),
                    tool_names: collected_tool_names,
                    tool_facts: collected_tool_facts,
                });
            }
        }
    }

    runtime_trace::record_event(
        "tool_loop_exhausted",
        Some(channel_name),
        Some(provider_name),
        Some(model),
        Some(&turn_id),
        Some(false),
        Some("agent exceeded maximum tool iterations"),
        serde_json::json!({
            "max_iterations": max_iterations,
        }),
    );
    anyhow::bail!("Agent exceeded maximum tool iterations ({max_iterations})")
}

// ── CLI Entrypoint ───────────────────────────────────────────────────────
// Wires up all subsystems (observer, runtime, security, memory, tools,
// provider) and enters either single-shot or interactive REPL mode.
// The interactive loop manages history compaction and hard trimming to
// keep the context window bounded.
