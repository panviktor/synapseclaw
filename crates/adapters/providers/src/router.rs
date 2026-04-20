use super::reliable::classify_provider_error;
use super::traits::{ChatMessage, ChatRequest, ChatResponse};
use super::Provider;
use anyhow::Context;
use async_trait::async_trait;
use std::collections::HashMap;

/// A single route: maps a task hint to a provider + model combo.
#[derive(Debug, Clone)]
pub struct Route {
    pub provider_name: String,
    pub model: String,
}

/// Multi-model router — routes requests to different provider+model combos
/// based on a task hint encoded in the model parameter.
///
/// The model parameter can be:
/// - A regular model name (e.g. "anthropic/claude-sonnet-4") → uses default provider
/// - A hint-prefixed string (e.g. "hint:reasoning") → resolves via route table
///
/// This wraps multiple pre-created providers and selects the right one per request.
pub struct RouterProvider {
    routes: HashMap<String, (usize, String)>, // hint → (provider_index, model)
    route_chains: HashMap<String, Vec<(usize, String)>>, // hint → ordered failover chain
    default_chain: Vec<(usize, String)>,
    providers: Vec<(String, Box<dyn Provider>)>,
    default_index: usize,
    default_model: String,
}

impl RouterProvider {
    /// Create a new router with a default provider and optional routes.
    ///
    /// `providers` is a list of (name, provider) pairs. The first one is the default.
    /// `routes` maps hint names to Route structs containing provider_name and model.
    pub fn new(
        providers: Vec<(String, Box<dyn Provider>)>,
        routes: Vec<(String, Route)>,
        default_model: String,
    ) -> Self {
        Self::new_with_chains(providers, routes, Vec::new(), default_model)
    }

    pub fn new_with_chains(
        providers: Vec<(String, Box<dyn Provider>)>,
        routes: Vec<(String, Route)>,
        route_chains: Vec<(String, Vec<Route>)>,
        default_model: String,
    ) -> Self {
        // Build provider name → index lookup
        let name_to_index: HashMap<&str, usize> = providers
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (name.as_str(), i))
            .collect();

        // Resolve routes to provider indices
        let resolved_routes: HashMap<String, (usize, String)> = routes
            .into_iter()
            .filter_map(|(hint, route)| {
                let index = name_to_index.get(route.provider_name.as_str()).copied();
                match index {
                    Some(i) => Some((hint, (i, route.model))),
                    None => {
                        tracing::warn!(
                            hint = hint,
                            provider = route.provider_name,
                            "Route references unknown provider, skipping"
                        );
                        None
                    }
                }
            })
            .collect();

        let resolved_chains: HashMap<String, Vec<(usize, String)>> = route_chains
            .into_iter()
            .filter_map(|(hint, chain)| {
                let mut resolved = Vec::new();
                for route in chain {
                    let Some(index) = name_to_index.get(route.provider_name.as_str()).copied()
                    else {
                        tracing::warn!(
                            hint = hint.as_str(),
                            provider = route.provider_name.as_str(),
                            "Route chain references unknown provider, skipping candidate"
                        );
                        continue;
                    };
                    resolved.push((index, route.model));
                }
                (!resolved.is_empty()).then_some((hint, resolved))
            })
            .collect();

        let default_chain = resolved_chains
            .get("reasoning")
            .filter(|chain| {
                chain
                    .first()
                    .is_some_and(|(idx, model)| *idx == 0 && model == &default_model)
            })
            .cloned()
            .unwrap_or_else(|| vec![(0, default_model.clone())]);

        Self {
            routes: resolved_routes,
            route_chains: resolved_chains,
            default_chain,
            providers,
            default_index: 0,
            default_model,
        }
    }

    /// Resolve a model parameter to a (provider, actual_model) pair.
    ///
    /// If the model starts with "hint:", look up the hint in the route table.
    /// Otherwise, use the default provider with the given model name.
    fn resolve(&self, model: &str) -> anyhow::Result<(usize, String)> {
        if let Some(hint) = model.strip_prefix("hint:") {
            if let Some((idx, resolved_model)) = self.routes.get(hint) {
                return Ok((*idx, resolved_model.clone()));
            }
            anyhow::bail!("Unknown route hint: {hint}");
        }

        Ok((self.default_index, model.to_string()))
    }

    fn resolve_chain(&self, model: &str) -> anyhow::Result<Vec<(usize, String)>> {
        if let Some(hint) = model.strip_prefix("hint:") {
            if let Some(chain) = self
                .route_chains
                .get(hint)
                .filter(|chain| !chain.is_empty())
            {
                return Ok(chain.clone());
            }
            return self.resolve(model).map(|route| vec![route]);
        }

        if model == self.default_model {
            return Ok(self.default_chain.clone());
        }

        self.resolve(model).map(|route| vec![route])
    }

    fn push_failure(
        failures: &mut Vec<String>,
        provider_name: &str,
        model: &str,
        error: &anyhow::Error,
    ) -> bool {
        let class = classify_provider_error(error);
        failures.push(format!(
            "provider={provider_name} model={model} kind={} error={}",
            class.kind.as_str(),
            class.detail
        ));
        class.failover_candidate
    }

    fn failover_error_context(failures: &[String]) -> String {
        format!(
            "Router provider/model attempts failed. Attempts:\n{}",
            failures.join("\n")
        )
    }
}

#[async_trait]
impl Provider for RouterProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let chain = self.resolve_chain(model)?;
        let mut failures = Vec::new();

        for (position, (provider_idx, resolved_model)) in chain.iter().enumerate() {
            let (provider_name, provider) = &self.providers[*provider_idx];
            tracing::info!(
                provider = provider_name.as_str(),
                model = resolved_model.as_str(),
                route_position = position,
                route_candidate_count = chain.len(),
                "Router dispatching request"
            );

            match provider
                .chat_with_system(system_prompt, message, resolved_model, temperature)
                .await
            {
                Ok(response) => {
                    if position > 0 {
                        tracing::info!(
                            provider = provider_name.as_str(),
                            model = resolved_model.as_str(),
                            failed_candidates = position,
                            "Router recovered via candidate failover"
                        );
                    }
                    return Ok(response);
                }
                Err(error) => {
                    let failover = Self::push_failure(
                        &mut failures,
                        provider_name.as_str(),
                        resolved_model,
                        &error,
                    );
                    if !failover || position + 1 == chain.len() {
                        return Err(error).context(Self::failover_error_context(&failures));
                    }
                    tracing::warn!(
                        provider = provider_name.as_str(),
                        model = resolved_model.as_str(),
                        "Router candidate failed; trying next candidate"
                    );
                }
            }
        }

        anyhow::bail!("{}", Self::failover_error_context(&failures))
    }

    async fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let chain = self.resolve_chain(model)?;
        let mut failures = Vec::new();

        for (position, (provider_idx, resolved_model)) in chain.iter().enumerate() {
            let (provider_name, provider) = &self.providers[*provider_idx];
            match provider
                .chat_with_history(messages, resolved_model, temperature)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let failover = Self::push_failure(
                        &mut failures,
                        provider_name.as_str(),
                        resolved_model,
                        &error,
                    );
                    if !failover || position + 1 == chain.len() {
                        return Err(error).context(Self::failover_error_context(&failures));
                    }
                    tracing::warn!(
                        provider = provider_name.as_str(),
                        model = resolved_model.as_str(),
                        "Router history candidate failed; trying next candidate"
                    );
                }
            }
        }

        anyhow::bail!("{}", Self::failover_error_context(&failures))
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        let chain = self.resolve_chain(model)?;
        let mut failures = Vec::new();

        for (position, (provider_idx, resolved_model)) in chain.iter().enumerate() {
            let (provider_name, provider) = &self.providers[*provider_idx];
            let candidate_request = ChatRequest {
                messages: request.messages,
                tools: request.tools,
            };
            match provider
                .chat(candidate_request, resolved_model, temperature)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let failover = Self::push_failure(
                        &mut failures,
                        provider_name.as_str(),
                        resolved_model,
                        &error,
                    );
                    if !failover || position + 1 == chain.len() {
                        return Err(error).context(Self::failover_error_context(&failures));
                    }
                    tracing::warn!(
                        provider = provider_name.as_str(),
                        model = resolved_model.as_str(),
                        "Router chat candidate failed; trying next candidate"
                    );
                }
            }
        }

        anyhow::bail!("{}", Self::failover_error_context(&failures))
    }

    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[serde_json::Value],
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<ChatResponse> {
        let chain = self.resolve_chain(model)?;
        let mut failures = Vec::new();

        for (position, (provider_idx, resolved_model)) in chain.iter().enumerate() {
            let (provider_name, provider) = &self.providers[*provider_idx];
            match provider
                .chat_with_tools(messages, tools, resolved_model, temperature)
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let failover = Self::push_failure(
                        &mut failures,
                        provider_name.as_str(),
                        resolved_model,
                        &error,
                    );
                    if !failover || position + 1 == chain.len() {
                        return Err(error).context(Self::failover_error_context(&failures));
                    }
                    tracing::warn!(
                        provider = provider_name.as_str(),
                        model = resolved_model.as_str(),
                        "Router tool candidate failed; trying next candidate"
                    );
                }
            }
        }

        anyhow::bail!("{}", Self::failover_error_context(&failures))
    }

    fn supports_native_tools(&self) -> bool {
        self.providers
            .get(self.default_index)
            .map(|(_, p)| p.supports_native_tools())
            .unwrap_or(false)
    }

    fn supports_vision(&self) -> bool {
        self.providers
            .iter()
            .any(|(_, provider)| provider.supports_vision())
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        for (name, provider) in &self.providers {
            tracing::info!(provider = name, "Warming up routed provider");
            if let Err(e) = provider.warmup().await {
                tracing::warn!(provider = name, "Warmup failed (non-fatal): {e}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct MockProvider {
        calls: Arc<AtomicUsize>,
        response: &'static str,
        last_model: parking_lot::Mutex<String>,
    }

    impl MockProvider {
        fn new(response: &'static str) -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                response,
                last_model: parking_lot::Mutex::new(String::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn last_model(&self) -> String {
            self.last_model.lock().clone()
        }
    }

    struct FailingProvider {
        calls: Arc<AtomicUsize>,
        error: &'static str,
        last_model: parking_lot::Mutex<String>,
    }

    impl FailingProvider {
        fn new(error: &'static str) -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                error,
                last_model: parking_lot::Mutex::new(String::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn last_model(&self) -> String {
            self.last_model.lock().clone()
        }
    }

    #[async_trait]
    impl Provider for FailingProvider {
        fn supports_native_tools(&self) -> bool {
            true
        }

        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_model.lock() = model.to_string();
            anyhow::bail!("{}", self.error)
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn supports_native_tools(&self) -> bool {
            true
        }

        async fn chat_with_system(
            &self,
            _system_prompt: Option<&str>,
            _message: &str,
            model: &str,
            _temperature: f64,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_model.lock() = model.to_string();
            Ok(self.response.to_string())
        }

        async fn chat_with_tools(
            &self,
            messages: &[ChatMessage],
            _tools: &[serde_json::Value],
            model: &str,
            temperature: f64,
        ) -> anyhow::Result<ChatResponse> {
            let text = self.chat_with_history(messages, model, temperature).await?;
            Ok(ChatResponse {
                text: Some(text),
                tool_calls: Vec::new(),
                usage: None,
                reasoning_content: None,
                media_artifacts: Vec::new(),
            })
        }
    }

    fn make_router(
        providers: Vec<(&'static str, &'static str)>,
        routes: Vec<(&str, &str, &str)>,
    ) -> (RouterProvider, Vec<Arc<MockProvider>>) {
        let mocks: Vec<Arc<MockProvider>> = providers
            .iter()
            .map(|(_, response)| Arc::new(MockProvider::new(response)))
            .collect();

        let provider_list: Vec<(String, Box<dyn Provider>)> = providers
            .iter()
            .zip(mocks.iter())
            .map(|((name, _), mock)| {
                (
                    name.to_string(),
                    Box::new(Arc::clone(mock)) as Box<dyn Provider>,
                )
            })
            .collect();

        let route_list: Vec<(String, Route)> = routes
            .iter()
            .map(|(hint, provider_name, model)| {
                (
                    hint.to_string(),
                    Route {
                        provider_name: provider_name.to_string(),
                        model: model.to_string(),
                    },
                )
            })
            .collect();

        let router = RouterProvider::new(provider_list, route_list, "default-model".to_string());

        (router, mocks)
    }

    // Arc<MockProvider> should also be a Provider
    #[async_trait]
    impl Provider for Arc<MockProvider> {
        fn supports_native_tools(&self) -> bool {
            self.as_ref().supports_native_tools()
        }

        async fn chat_with_system(
            &self,
            system_prompt: Option<&str>,
            message: &str,
            model: &str,
            temperature: f64,
        ) -> anyhow::Result<String> {
            self.as_ref()
                .chat_with_system(system_prompt, message, model, temperature)
                .await
        }

        async fn chat_with_tools(
            &self,
            messages: &[ChatMessage],
            tools: &[serde_json::Value],
            model: &str,
            temperature: f64,
        ) -> anyhow::Result<ChatResponse> {
            self.as_ref()
                .chat_with_tools(messages, tools, model, temperature)
                .await
        }
    }

    #[async_trait]
    impl Provider for Arc<FailingProvider> {
        fn supports_native_tools(&self) -> bool {
            self.as_ref().supports_native_tools()
        }

        async fn chat_with_system(
            &self,
            system_prompt: Option<&str>,
            message: &str,
            model: &str,
            temperature: f64,
        ) -> anyhow::Result<String> {
            self.as_ref()
                .chat_with_system(system_prompt, message, model, temperature)
                .await
        }
    }

    #[tokio::test]
    async fn routes_hint_to_correct_provider() {
        let (router, mocks) = make_router(
            vec![("fast", "fast-response"), ("smart", "smart-response")],
            vec![
                ("fast", "fast", "llama-3-70b"),
                ("reasoning", "smart", "claude-opus"),
            ],
        );

        let result = router
            .simple_chat("hello", "hint:reasoning", 0.5)
            .await
            .unwrap();
        assert_eq!(result, "smart-response");
        assert_eq!(mocks[1].call_count(), 1);
        assert_eq!(mocks[1].last_model(), "claude-opus");
        assert_eq!(mocks[0].call_count(), 0);
    }

    #[tokio::test]
    async fn routes_fast_hint() {
        let (router, mocks) = make_router(
            vec![("fast", "fast-response"), ("smart", "smart-response")],
            vec![("fast", "fast", "llama-3-70b")],
        );

        let result = router.simple_chat("hello", "hint:fast", 0.5).await.unwrap();
        assert_eq!(result, "fast-response");
        assert_eq!(mocks[0].call_count(), 1);
        assert_eq!(mocks[0].last_model(), "llama-3-70b");
    }

    #[tokio::test]
    async fn unknown_hint_is_rejected_instead_of_using_default_provider() {
        let (router, mocks) = make_router(
            vec![("default", "default-response"), ("other", "other-response")],
            vec![],
        );

        let error = router
            .simple_chat("hello", "hint:nonexistent", 0.5)
            .await
            .expect_err("unknown hint should fail before provider dispatch");
        assert!(error.to_string().contains("Unknown route hint"));
        assert_eq!(mocks[0].call_count(), 0);
        assert_eq!(mocks[1].call_count(), 0);
    }

    #[tokio::test]
    async fn non_hint_model_uses_default_provider() {
        let (router, mocks) = make_router(
            vec![
                ("primary", "primary-response"),
                ("secondary", "secondary-response"),
            ],
            vec![("code", "secondary", "codellama")],
        );

        let result = router
            .simple_chat("hello", "anthropic/claude-sonnet-4-20250514", 0.5)
            .await
            .unwrap();
        assert_eq!(result, "primary-response");
        assert_eq!(mocks[0].call_count(), 1);
        assert_eq!(mocks[0].last_model(), "anthropic/claude-sonnet-4-20250514");
    }

    #[test]
    fn resolve_preserves_model_for_non_hints() {
        let (router, _) = make_router(vec![("default", "ok")], vec![]);

        let (idx, model) = router.resolve("gpt-4o").expect("regular model id");
        assert_eq!(idx, 0);
        assert_eq!(model, "gpt-4o");
    }

    #[test]
    fn resolve_strips_hint_prefix() {
        let (router, _) = make_router(
            vec![("fast", "ok"), ("smart", "ok")],
            vec![("reasoning", "smart", "claude-opus")],
        );

        let (idx, model) = router.resolve("hint:reasoning").expect("known route hint");
        assert_eq!(idx, 1);
        assert_eq!(model, "claude-opus");
    }

    #[test]
    fn skips_routes_with_unknown_provider() {
        let (router, _) = make_router(
            vec![("default", "ok")],
            vec![("broken", "nonexistent", "model")],
        );

        // Route should not exist
        assert!(!router.routes.contains_key("broken"));
    }

    #[tokio::test]
    async fn warmup_calls_all_providers() {
        let (router, _) = make_router(vec![("a", "ok"), ("b", "ok")], vec![]);

        // Warmup should not error
        assert!(router.warmup().await.is_ok());
    }

    #[tokio::test]
    async fn chat_with_system_passes_system_prompt() {
        let mock = Arc::new(MockProvider::new("response"));
        let router = RouterProvider::new(
            vec![(
                "default".into(),
                Box::new(Arc::clone(&mock)) as Box<dyn Provider>,
            )],
            vec![],
            "model".into(),
        );

        let result = router
            .chat_with_system(Some("system"), "hello", "model", 0.5)
            .await
            .unwrap();
        assert_eq!(result, "response");
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test]
    async fn chat_with_tools_delegates_to_resolved_provider() {
        let mock = Arc::new(MockProvider::new("tool-response"));
        let router = RouterProvider::new(
            vec![(
                "default".into(),
                Box::new(Arc::clone(&mock)) as Box<dyn Provider>,
            )],
            vec![],
            "model".into(),
        );

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "use tools".to_string(),
        }];
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "Run shell command",
                "parameters": {}
            }
        })];

        // chat_with_tools should delegate through the router to the mock.
        let result = router
            .chat_with_tools(&messages, &tools, "model", 0.7)
            .await
            .unwrap();
        assert_eq!(result.text.as_deref(), Some("tool-response"));
        assert_eq!(mock.call_count(), 1);
        assert_eq!(mock.last_model(), "model");
    }

    #[tokio::test]
    async fn default_model_fails_over_across_reasoning_chain_on_quota() {
        let primary = Arc::new(FailingProvider::new(
            "API error (429 Too Many Requests): insufficient quota",
        ));
        let secondary = Arc::new(MockProvider::new("secondary-response"));
        let router = RouterProvider::new_with_chains(
            vec![
                (
                    "primary".to_string(),
                    Box::new(Arc::clone(&primary)) as Box<dyn Provider>,
                ),
                (
                    "secondary".to_string(),
                    Box::new(Arc::clone(&secondary)) as Box<dyn Provider>,
                ),
            ],
            vec![],
            vec![(
                "reasoning".to_string(),
                vec![
                    Route {
                        provider_name: "primary".to_string(),
                        model: "gpt-5.4".to_string(),
                    },
                    Route {
                        provider_name: "secondary".to_string(),
                        model: "claude-sonnet-4-6".to_string(),
                    },
                ],
            )],
            "gpt-5.4".to_string(),
        );

        let result = router.simple_chat("hello", "gpt-5.4", 0.5).await.unwrap();
        assert_eq!(result, "secondary-response");
        assert_eq!(primary.call_count(), 1);
        assert_eq!(primary.last_model(), "gpt-5.4");
        assert_eq!(secondary.call_count(), 1);
        assert_eq!(secondary.last_model(), "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn default_model_does_not_fail_over_on_context_window_error() {
        let primary = Arc::new(FailingProvider::new(
            "input exceeds the context window of this model",
        ));
        let secondary = Arc::new(MockProvider::new("secondary-response"));
        let router = RouterProvider::new_with_chains(
            vec![
                (
                    "primary".to_string(),
                    Box::new(Arc::clone(&primary)) as Box<dyn Provider>,
                ),
                (
                    "secondary".to_string(),
                    Box::new(Arc::clone(&secondary)) as Box<dyn Provider>,
                ),
            ],
            vec![],
            vec![(
                "reasoning".to_string(),
                vec![
                    Route {
                        provider_name: "primary".to_string(),
                        model: "gpt-5.4".to_string(),
                    },
                    Route {
                        provider_name: "secondary".to_string(),
                        model: "claude-sonnet-4-6".to_string(),
                    },
                ],
            )],
            "gpt-5.4".to_string(),
        );

        let error = router
            .simple_chat("hello", "gpt-5.4", 0.5)
            .await
            .expect_err("context-window overflow should stay on the selected model");
        assert!(error.to_string().contains("context window"));
        assert_eq!(primary.call_count(), 1);
        assert_eq!(primary.last_model(), "gpt-5.4");
        assert_eq!(secondary.call_count(), 0);
    }

    #[tokio::test]
    async fn chat_with_tools_routes_hint_correctly() {
        let (router, mocks) = make_router(
            vec![("fast", "fast-tool"), ("smart", "smart-tool")],
            vec![("reasoning", "smart", "claude-opus")],
        );

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "reason about this".to_string(),
        }];
        let tools = vec![serde_json::json!({"type": "function", "function": {"name": "test"}})];

        let result = router
            .chat_with_tools(&messages, &tools, "hint:reasoning", 0.5)
            .await
            .unwrap();
        assert_eq!(result.text.as_deref(), Some("smart-tool"));
        assert_eq!(mocks[1].call_count(), 1);
        assert_eq!(mocks[1].last_model(), "claude-opus");
        assert_eq!(mocks[0].call_count(), 0);
    }
}
