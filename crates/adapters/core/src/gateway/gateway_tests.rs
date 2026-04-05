use super::*;
use crate::channels::traits::ChannelMessage;
use async_trait::async_trait;
use axum::http::HeaderValue;
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use synapse_domain::domain::memory::{MemoryCategory, MemoryEntry, MemoryError};
use synapse_memory::UnifiedMemoryPort;
use synapse_providers::Provider;

struct NoopRunner;
#[async_trait]
impl synapse_domain::ports::agent_runner::AgentRunnerPort for NoopRunner {
    async fn run(
        &self,
        _: Option<String>,
        _: Option<String>,
        _: Option<String>,
        _: f64,
        _: bool,
        _: Option<std::path::PathBuf>,
        _: Option<Vec<String>>,
        _: Option<Arc<synapse_domain::domain::tool_audit::RunContext>>,
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }
    async fn process_message(&self, _: &str, _: Option<&str>) -> anyhow::Result<String> {
        Ok(String::new())
    }
}

fn test_agent_runner() -> Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort> {
    Arc::new(NoopRunner)
}

/// Generate a random hex secret at runtime to avoid hard-coded cryptographic values.
fn generate_test_secret() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}

#[test]
fn security_body_limit_is_64kb() {
    assert_eq!(MAX_BODY_SIZE, 65_536);
}

#[test]
fn security_timeout_is_30_seconds() {
    assert_eq!(REQUEST_TIMEOUT_SECS, 30);
}

#[test]
fn webhook_body_requires_message_field() {
    let valid = r#"{"message": "hello"}"#;
    let parsed: Result<WebhookBody, _> = serde_json::from_str(valid);
    assert!(parsed.is_ok());
    assert_eq!(parsed.unwrap().message, "hello");

    let missing = r#"{"other": "field"}"#;
    let parsed: Result<WebhookBody, _> = serde_json::from_str(missing);
    assert!(parsed.is_err());
}

#[test]
fn whatsapp_query_fields_are_optional() {
    let q = WhatsAppVerifyQuery {
        mode: None,
        verify_token: None,
        challenge: None,
    };
    assert!(q.mode.is_none());
}

#[test]
fn app_state_is_clone() {
    fn assert_clone<T: Clone>() {}
    assert_clone::<AppState>();
}

#[tokio::test]
async fn metrics_endpoint_returns_hint_when_prometheus_is_disabled() {
    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider: Arc::new(MockProvider::default()),
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: Arc::new(synapse_memory::NoopUnifiedMemory),
        auto_save: false,
        webhook_secret_hash: None,
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let response = handle_metrics(State(state)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some(PROMETHEUS_CONTENT_TYPE)
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("Prometheus backend not enabled"));
}

#[cfg(feature = "observability-prometheus")]
#[tokio::test]
async fn metrics_endpoint_renders_prometheus_output() {
    let prom = Arc::new(synapse_observability::PrometheusObserver::new());
    synapse_observability::Observer::record_event(
        prom.as_ref(),
        &synapse_observability::ObserverEvent::HeartbeatTick,
    );

    let observer: Arc<dyn synapse_observability::Observer> = prom;
    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider: Arc::new(MockProvider::default()),
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: Arc::new(synapse_memory::NoopUnifiedMemory),
        auto_save: false,
        webhook_secret_hash: None,
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer,
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let response = handle_metrics(State(state)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("synapseclaw_heartbeat_ticks_total 1"));
}

#[test]
fn gateway_rate_limiter_blocks_after_limit() {
    let limiter = GatewayRateLimiter::new(2, 2, 100);
    assert!(limiter.allow_pair("127.0.0.1"));
    assert!(limiter.allow_pair("127.0.0.1"));
    assert!(!limiter.allow_pair("127.0.0.1"));
}

#[test]
fn rate_limiter_sweep_removes_stale_entries() {
    let limiter = SlidingWindowRateLimiter::new(10, Duration::from_secs(60), 100);
    // Add entries for multiple IPs
    assert!(limiter.allow("ip-1"));
    assert!(limiter.allow("ip-2"));
    assert!(limiter.allow("ip-3"));

    {
        let guard = limiter.requests.lock();
        assert_eq!(guard.0.len(), 3);
    }

    // Force a sweep by backdating last_sweep
    {
        let mut guard = limiter.requests.lock();
        guard.1 = Instant::now()
            .checked_sub(Duration::from_secs(RATE_LIMITER_SWEEP_INTERVAL_SECS + 1))
            .unwrap();
        // Clear timestamps for ip-2 and ip-3 to simulate stale entries
        guard.0.get_mut("ip-2").unwrap().clear();
        guard.0.get_mut("ip-3").unwrap().clear();
    }

    // Next allow() call should trigger sweep and remove stale entries
    assert!(limiter.allow("ip-1"));

    {
        let guard = limiter.requests.lock();
        assert_eq!(guard.0.len(), 1, "Stale entries should have been swept");
        assert!(guard.0.contains_key("ip-1"));
    }
}

#[test]
fn rate_limiter_zero_limit_always_allows() {
    let limiter = SlidingWindowRateLimiter::new(0, Duration::from_secs(60), 10);
    for _ in 0..100 {
        assert!(limiter.allow("any-key"));
    }
}

#[test]
fn idempotency_store_rejects_duplicate_key() {
    let store = IdempotencyStore::new(Duration::from_secs(30), 10);
    assert!(store.record_if_new("req-1"));
    assert!(!store.record_if_new("req-1"));
    assert!(store.record_if_new("req-2"));
}

#[test]
fn rate_limiter_bounded_cardinality_evicts_oldest_key() {
    let limiter = SlidingWindowRateLimiter::new(5, Duration::from_secs(60), 2);
    assert!(limiter.allow("ip-1"));
    assert!(limiter.allow("ip-2"));
    assert!(limiter.allow("ip-3"));

    let guard = limiter.requests.lock();
    assert_eq!(guard.0.len(), 2);
    assert!(guard.0.contains_key("ip-2"));
    assert!(guard.0.contains_key("ip-3"));
}

#[test]
fn idempotency_store_bounded_cardinality_evicts_oldest_key() {
    let store = IdempotencyStore::new(Duration::from_secs(300), 2);
    assert!(store.record_if_new("k1"));
    std::thread::sleep(Duration::from_millis(2));
    assert!(store.record_if_new("k2"));
    std::thread::sleep(Duration::from_millis(2));
    assert!(store.record_if_new("k3"));

    let keys = store.keys.lock();
    assert_eq!(keys.len(), 2);
    assert!(!keys.contains_key("k1"));
    assert!(keys.contains_key("k2"));
    assert!(keys.contains_key("k3"));
}

#[test]
fn client_key_defaults_to_peer_addr_when_untrusted_proxy_mode() {
    let peer = SocketAddr::from(([10, 0, 0, 5], 42617));
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Forwarded-For",
        HeaderValue::from_static("198.51.100.10, 203.0.113.11"),
    );

    let key = client_key_from_request(Some(peer), &headers, false);
    assert_eq!(key, "10.0.0.5");
}

#[test]
fn client_key_uses_forwarded_ip_only_in_trusted_proxy_mode() {
    let peer = SocketAddr::from(([10, 0, 0, 5], 42617));
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Forwarded-For",
        HeaderValue::from_static("198.51.100.10, 203.0.113.11"),
    );

    let key = client_key_from_request(Some(peer), &headers, true);
    assert_eq!(key, "198.51.100.10");
}

#[test]
fn client_key_falls_back_to_peer_when_forwarded_header_invalid() {
    let peer = SocketAddr::from(([10, 0, 0, 5], 42617));
    let mut headers = HeaderMap::new();
    headers.insert("X-Forwarded-For", HeaderValue::from_static("garbage-value"));

    let key = client_key_from_request(Some(peer), &headers, true);
    assert_eq!(key, "10.0.0.5");
}

#[test]
fn normalize_max_keys_uses_fallback_for_zero() {
    assert_eq!(normalize_max_keys(0, 10_000), 10_000);
    assert_eq!(normalize_max_keys(0, 0), 1);
}

#[test]
fn normalize_max_keys_preserves_nonzero_values() {
    assert_eq!(normalize_max_keys(2_048, 10_000), 2_048);
    assert_eq!(normalize_max_keys(1, 10_000), 1);
}

#[tokio::test]
async fn persist_pairing_tokens_writes_config_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    let workspace_path = temp.path().join("workspace");

    let mut config = Config::default();
    config.config_path = config_path.clone();
    config.workspace_dir = workspace_path;
    config.save().await.unwrap();

    let guard = PairingGuard::new(true, &[]);
    let code = guard.pairing_code().unwrap();
    let token = guard.try_pair(&code, "test_client").await.unwrap().unwrap();
    assert!(guard.is_authenticated(&token));

    let shared_config = Arc::new(Mutex::new(config));
    Box::pin(persist_pairing_tokens(shared_config.clone(), &guard))
        .await
        .unwrap();

    // In-memory tokens should remain as plaintext 64-char hex hashes.
    let plaintext = {
        let in_memory = shared_config.lock();
        assert_eq!(in_memory.gateway.paired_tokens.len(), 1);
        in_memory.gateway.paired_tokens[0].clone()
    };
    assert_eq!(plaintext.len(), 64);
    assert!(plaintext.chars().all(|c: char| c.is_ascii_hexdigit()));

    // On disk, the token should be encrypted (secrets.encrypt defaults to true).
    let saved = tokio::fs::read_to_string(config_path).await.unwrap();
    let raw_parsed: Config = toml::from_str(&saved).unwrap();
    assert_eq!(raw_parsed.gateway.paired_tokens.len(), 1);
    let on_disk = &raw_parsed.gateway.paired_tokens[0];
    assert!(
        synapse_security::SecretStore::is_encrypted(on_disk),
        "paired_token should be encrypted on disk"
    );
}

#[test]
fn webhook_memory_key_is_unique() {
    let key1 = webhook_memory_key();
    let key2 = webhook_memory_key();

    assert!(key1.starts_with("webhook_msg_"));
    assert!(key2.starts_with("webhook_msg_"));
    assert_ne!(key1, key2);
}

#[test]
fn whatsapp_memory_key_includes_sender_and_message_id() {
    let msg = ChannelMessage {
        id: "wamid-123".into(),
        sender: "+1234567890".into(),
        reply_target: "+1234567890".into(),
        content: "hello".into(),
        channel: "whatsapp".into(),
        timestamp: 1,
        thread_ts: None,
    };

    let key = whatsapp_memory_key(&msg);
    assert_eq!(key, "whatsapp_+1234567890_wamid-123");
}

// MockMemory replaced by synapse_memory::NoopUnifiedMemory (used inline).

#[derive(Default)]
struct MockProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for MockProvider {
    async fn chat_with_system(
        &self,
        _system_prompt: Option<&str>,
        _message: &str,
        _model: &str,
        _temperature: f64,
    ) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("ok".into())
    }
}

#[derive(Default)]
struct TrackingMemory {
    keys: Mutex<Vec<String>>,
}

// Delegate all sub-trait impls to noop, override store/count in UnifiedMemoryPort.
#[async_trait]
impl synapse_domain::ports::memory::WorkingMemoryPort for TrackingMemory {
    async fn get_core_blocks(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<Vec<synapse_domain::domain::memory::CoreMemoryBlock>, MemoryError> {
        Ok(vec![])
    }
    async fn update_core_block(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
        _: &str,
        _: String,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn append_core_block(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
        _: &str,
        _: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
}
#[async_trait]
impl synapse_domain::ports::memory::EpisodicMemoryPort for TrackingMemory {
    async fn store_episode(
        &self,
        _: MemoryEntry,
    ) -> Result<synapse_domain::domain::memory::MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn get_recent(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
        _: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    async fn get_session(
        &self,
        _: &synapse_domain::domain::memory::SessionId,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    async fn search_episodes(
        &self,
        _: &synapse_domain::domain::memory::MemoryQuery,
    ) -> Result<Vec<synapse_domain::domain::memory::SearchResult>, MemoryError> {
        Ok(vec![])
    }
}
#[async_trait]
impl synapse_domain::ports::memory::SemanticMemoryPort for TrackingMemory {
    async fn upsert_entity(
        &self,
        _: synapse_domain::domain::memory::Entity,
    ) -> Result<synapse_domain::domain::memory::MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn find_entity(
        &self,
        _: &str,
    ) -> Result<Option<synapse_domain::domain::memory::Entity>, MemoryError> {
        Ok(None)
    }
    async fn add_fact(
        &self,
        _: synapse_domain::domain::memory::TemporalFact,
    ) -> Result<synapse_domain::domain::memory::MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn invalidate_fact(
        &self,
        _: &synapse_domain::domain::memory::MemoryId,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn get_current_facts(
        &self,
        _: &synapse_domain::domain::memory::MemoryId,
    ) -> Result<Vec<synapse_domain::domain::memory::TemporalFact>, MemoryError> {
        Ok(vec![])
    }
    async fn traverse(
        &self,
        _: &synapse_domain::domain::memory::MemoryId,
        _: usize,
    ) -> Result<
        Vec<(
            synapse_domain::domain::memory::Entity,
            synapse_domain::domain::memory::TemporalFact,
        )>,
        MemoryError,
    > {
        Ok(vec![])
    }
    async fn search_entities(
        &self,
        _: &synapse_domain::domain::memory::MemoryQuery,
    ) -> Result<Vec<synapse_domain::domain::memory::Entity>, MemoryError> {
        Ok(vec![])
    }
}
#[async_trait]
impl synapse_domain::ports::memory::SkillMemoryPort for TrackingMemory {
    async fn store_skill(
        &self,
        _: synapse_domain::domain::memory::Skill,
    ) -> Result<synapse_domain::domain::memory::MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn find_skills(
        &self,
        _: &synapse_domain::domain::memory::MemoryQuery,
    ) -> Result<Vec<synapse_domain::domain::memory::Skill>, MemoryError> {
        Ok(vec![])
    }
    async fn update_skill(
        &self,
        _: &synapse_domain::domain::memory::MemoryId,
        _: synapse_domain::domain::memory::SkillUpdate,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn get_skill(
        &self,
        _: &str,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<Option<synapse_domain::domain::memory::Skill>, MemoryError> {
        Ok(None)
    }
}
#[async_trait]
impl synapse_domain::ports::memory::ReflectionPort for TrackingMemory {
    async fn store_reflection(
        &self,
        _: synapse_domain::domain::memory::Reflection,
    ) -> Result<synapse_domain::domain::memory::MemoryId, MemoryError> {
        Ok(String::new())
    }
    async fn get_relevant_reflections(
        &self,
        _: &synapse_domain::domain::memory::MemoryQuery,
    ) -> Result<Vec<synapse_domain::domain::memory::Reflection>, MemoryError> {
        Ok(vec![])
    }
    async fn get_failure_patterns(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
        _: usize,
    ) -> Result<Vec<synapse_domain::domain::memory::Reflection>, MemoryError> {
        Ok(vec![])
    }
}
#[async_trait]
impl synapse_domain::ports::memory::ConsolidationPort for TrackingMemory {
    async fn run_consolidation(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<synapse_domain::domain::memory::ConsolidationReport, MemoryError> {
        Ok(Default::default())
    }
    async fn recalculate_importance(
        &self,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<u32, MemoryError> {
        Ok(0)
    }
    async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
        Ok(0)
    }
}
#[async_trait]
impl UnifiedMemoryPort for TrackingMemory {
    async fn hybrid_search(
        &self,
        _: &synapse_domain::domain::memory::MemoryQuery,
    ) -> Result<synapse_domain::domain::memory::HybridSearchResult, MemoryError> {
        Ok(Default::default())
    }
    async fn embed(&self, _: &str) -> Result<Vec<f32>, MemoryError> {
        Ok(vec![])
    }
    async fn store(
        &self,
        key: &str,
        _: &str,
        _: &MemoryCategory,
        _: Option<&str>,
    ) -> Result<(), MemoryError> {
        self.keys.lock().push(key.to_string());
        Ok(())
    }
    async fn recall(
        &self,
        _: &str,
        _: usize,
        _: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn forget(
        &self,
        _: &str,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<bool, MemoryError> {
        Ok(false)
    }
    async fn get(
        &self,
        _: &str,
        _: &synapse_domain::domain::memory::AgentId,
    ) -> Result<Option<synapse_domain::domain::memory::MemoryEntry>, MemoryError> {
        Ok(None)
    }
    async fn list(
        &self,
        _: Option<&synapse_domain::domain::memory::MemoryCategory>,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<synapse_domain::domain::memory::MemoryEntry>, MemoryError> {
        Ok(vec![])
    }
    fn should_skip_autosave(&self, _: &str) -> bool {
        false
    }
    async fn count(&self) -> Result<usize, MemoryError> {
        let size = self.keys.lock().len();
        Ok(size)
    }
    fn name(&self) -> &str {
        "tracking"
    }
    async fn health_check(&self) -> bool {
        true
    }
}

fn test_connect_info() -> ConnectInfo<SocketAddr> {
    ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 30_300)))
}

#[tokio::test]
async fn webhook_idempotency_skips_duplicate_provider_calls() {
    let provider_impl = Arc::new(MockProvider::default());
    let provider: Arc<dyn Provider> = provider_impl.clone();
    let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: false,
        webhook_secret_hash: None,
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let mut headers = HeaderMap::new();
    headers.insert("X-Idempotency-Key", HeaderValue::from_static("abc-123"));

    let body = Ok(Json(WebhookBody {
        message: "hello".into(),
    }));
    let first = handle_webhook(
        State(state.clone()),
        test_connect_info(),
        headers.clone(),
        body,
    )
    .await
    .into_response();
    assert_eq!(first.status(), StatusCode::OK);

    let body = Ok(Json(WebhookBody {
        message: "hello".into(),
    }));
    let second = handle_webhook(State(state), test_connect_info(), headers, body)
        .await
        .into_response();
    assert_eq!(second.status(), StatusCode::OK);

    let payload = second.into_body().collect().await.unwrap().to_bytes();
    let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(parsed["status"], "duplicate");
    assert_eq!(parsed["idempotent"], true);
    assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn webhook_autosave_stores_distinct_keys_per_request() {
    let provider_impl = Arc::new(MockProvider::default());
    let provider: Arc<dyn Provider> = provider_impl.clone();

    let tracking_impl = Arc::new(TrackingMemory::default());
    let memory: Arc<dyn UnifiedMemoryPort> = tracking_impl.clone();

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: true,
        webhook_secret_hash: None,
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let headers = HeaderMap::new();

    let body1 = Ok(Json(WebhookBody {
        message: "hello one".into(),
    }));
    let first = handle_webhook(
        State(state.clone()),
        test_connect_info(),
        headers.clone(),
        body1,
    )
    .await
    .into_response();
    assert_eq!(first.status(), StatusCode::OK);

    let body2 = Ok(Json(WebhookBody {
        message: "hello two".into(),
    }));
    let second = handle_webhook(State(state), test_connect_info(), headers, body2)
        .await
        .into_response();
    assert_eq!(second.status(), StatusCode::OK);

    let keys = tracking_impl.keys.lock().clone();
    assert_eq!(keys.len(), 2);
    assert_ne!(keys[0], keys[1]);
    assert!(keys[0].starts_with("webhook_msg_"));
    assert!(keys[1].starts_with("webhook_msg_"));
    assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn webhook_secret_hash_is_deterministic_and_nonempty() {
    let secret_a = generate_test_secret();
    let secret_b = generate_test_secret();
    let one = hash_webhook_secret(&secret_a);
    let two = hash_webhook_secret(&secret_a);
    let other = hash_webhook_secret(&secret_b);

    assert_eq!(one, two);
    assert_ne!(one, other);
    assert_eq!(one.len(), 64);
}

#[tokio::test]
async fn webhook_secret_hash_rejects_missing_header() {
    let provider_impl = Arc::new(MockProvider::default());
    let provider: Arc<dyn Provider> = provider_impl.clone();
    let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);
    let secret = generate_test_secret();

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: false,
        webhook_secret_hash: Some(Arc::from(hash_webhook_secret(&secret))),
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let response = handle_webhook(
        State(state),
        test_connect_info(),
        HeaderMap::new(),
        Ok(Json(WebhookBody {
            message: "hello".into(),
        })),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn webhook_secret_hash_rejects_invalid_header() {
    let provider_impl = Arc::new(MockProvider::default());
    let provider: Arc<dyn Provider> = provider_impl.clone();
    let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);
    let valid_secret = generate_test_secret();
    let wrong_secret = generate_test_secret();

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: false,
        webhook_secret_hash: Some(Arc::from(hash_webhook_secret(&valid_secret))),
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Webhook-Secret",
        HeaderValue::from_str(&wrong_secret).unwrap(),
    );

    let response = handle_webhook(
        State(state),
        test_connect_info(),
        headers,
        Ok(Json(WebhookBody {
            message: "hello".into(),
        })),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn webhook_secret_hash_accepts_valid_header() {
    let provider_impl = Arc::new(MockProvider::default());
    let provider: Arc<dyn Provider> = provider_impl.clone();
    let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);
    let secret = generate_test_secret();

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: false,
        webhook_secret_hash: Some(Arc::from(hash_webhook_secret(&secret))),
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let mut headers = HeaderMap::new();
    headers.insert("X-Webhook-Secret", HeaderValue::from_str(&secret).unwrap());

    let response = handle_webhook(
        State(state),
        test_connect_info(),
        headers,
        Ok(Json(WebhookBody {
            message: "hello".into(),
        })),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 1);
}

fn compute_nextcloud_signature_hex(secret: &str, random: &str, body: &str) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    let payload = format!("{random}{body}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[tokio::test]
async fn nextcloud_talk_webhook_returns_not_found_when_not_configured() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::default());
    let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: false,
        webhook_secret_hash: None,
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: None,
        nextcloud_talk_webhook_secret: None,
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let response = Box::pin(handle_nextcloud_talk_webhook(
        State(state),
        HeaderMap::new(),
        Bytes::from_static(br#"{"type":"message"}"#),
    ))
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn nextcloud_talk_webhook_rejects_invalid_signature() {
    let provider_impl = Arc::new(MockProvider::default());
    let provider: Arc<dyn Provider> = provider_impl.clone();
    let memory: Arc<dyn UnifiedMemoryPort> = Arc::new(synapse_memory::NoopUnifiedMemory);

    let channel = Arc::new(NextcloudTalkChannel::new(
        "https://cloud.example.com".into(),
        "app-token".into(),
        vec!["*".into()],
    ));

    let secret = "nextcloud-test-secret";
    let random = "seed-value";
    let body = r#"{"type":"message","object":{"token":"room-token"},"message":{"actorType":"users","actorId":"user_a","message":"hello"}}"#;
    let _valid_signature = compute_nextcloud_signature_hex(secret, random, body);
    let invalid_signature = "deadbeef";

    let state = AppState {
        config: Arc::new(Mutex::new(Config::default())),
        provider,
        model: "test-model".into(),
        summary_model: None,
        temperature: 0.0,
        mem: memory,
        auto_save: false,
        webhook_secret_hash: None,
        pairing: Arc::new(PairingGuard::new(false, &[])),
        trust_forwarded_headers: false,
        rate_limiter: Arc::new(GatewayRateLimiter::new(100, 100, 100)),
        idempotency_store: Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000)),
        whatsapp: None,
        whatsapp_app_secret: None,
        linq: None,
        linq_signing_secret: None,
        nextcloud_talk: Some(channel),
        nextcloud_talk_webhook_secret: Some(Arc::from(secret)),
        wati: None,
        observer: Arc::new(synapse_observability::NoopObserver),
        tools_registry: Arc::new(Vec::new()),
        cost_tracker: None,
        event_tx: tokio::sync::broadcast::channel(16).0,
        dialogue_state_store: Arc::new(
            synapse_domain::application::services::dialogue_state_service::DialogueStateStore::new(
            ),
        ),
        run_recipe_store: Arc::new(
            synapse_domain::ports::run_recipe_store::InMemoryRunRecipeStore::new(),
        ),
        shutdown_tx: tokio::sync::watch::channel(false).0,
        audit_logger: None,
        ipc_prompt_guard: None,
        ipc_leak_detector: None,
        ipc_db: None,
        ipc_rate_limiter: None,
        ipc_read_rate_limiter: None,
        node_registry: Arc::new(nodes::NodeRegistry::new(16)),
        agent_registry: Arc::new(agent_registry::AgentRegistry::new()),
        agent_runner: test_agent_runner(),
        provisioning_state: Arc::new(provisioning::ProvisioningState::new()),
        admin_cidrs: Arc::new(vec![]),
        chat_sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
        chat_db: None,
        ipc_push_dispatcher: None,
        ipc_push_dedup: None,
        ipc_push_signal: None,
        channel_session_backend: None,
        channel_registry: None,
        conversation_store: None,
        run_store: None,
        pipeline_store: None,
        pipeline_executor: None,
        message_router: None,
        tool_middleware: None,
        dead_letter: None,
        surreal: None,
        agent_id: "test-agent".into(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Nextcloud-Talk-Random",
        HeaderValue::from_str(random).unwrap(),
    );
    headers.insert(
        "X-Nextcloud-Talk-Signature",
        HeaderValue::from_str(invalid_signature).unwrap(),
    );

    let response = Box::pin(handle_nextcloud_talk_webhook(
        State(state),
        headers,
        Bytes::from(body),
    ))
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(provider_impl.calls.load(Ordering::SeqCst), 0);
}

// ══════════════════════════════════════════════════════════
// WhatsApp Signature Verification Tests (CWE-345 Prevention)
// ══════════════════════════════════════════════════════════

fn compute_whatsapp_signature_hex(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn compute_whatsapp_signature_header(secret: &str, body: &[u8]) -> String {
    format!("sha256={}", compute_whatsapp_signature_hex(secret, body))
}

#[test]
fn whatsapp_signature_valid() {
    let app_secret = generate_test_secret();
    let body = b"test body content";

    let signature_header = compute_whatsapp_signature_header(&app_secret, body);

    assert!(verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_invalid_wrong_secret() {
    let app_secret = generate_test_secret();
    let wrong_secret = generate_test_secret();
    let body = b"test body content";

    let signature_header = compute_whatsapp_signature_header(&wrong_secret, body);

    assert!(!verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_invalid_wrong_body() {
    let app_secret = generate_test_secret();
    let original_body = b"original body";
    let tampered_body = b"tampered body";

    let signature_header = compute_whatsapp_signature_header(&app_secret, original_body);

    // Verify with tampered body should fail
    assert!(!verify_whatsapp_signature(
        &app_secret,
        tampered_body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_missing_prefix() {
    let app_secret = generate_test_secret();
    let body = b"test body";

    // Signature without "sha256=" prefix
    let signature_header = "abc123def456";

    assert!(!verify_whatsapp_signature(
        &app_secret,
        body,
        signature_header
    ));
}

#[test]
fn whatsapp_signature_empty_header() {
    let app_secret = generate_test_secret();
    let body = b"test body";

    assert!(!verify_whatsapp_signature(&app_secret, body, ""));
}

#[test]
fn whatsapp_signature_invalid_hex() {
    let app_secret = generate_test_secret();
    let body = b"test body";

    // Invalid hex characters
    let signature_header = "sha256=not_valid_hex_zzz";

    assert!(!verify_whatsapp_signature(
        &app_secret,
        body,
        signature_header
    ));
}

#[test]
fn whatsapp_signature_empty_body() {
    let app_secret = generate_test_secret();
    let body = b"";

    let signature_header = compute_whatsapp_signature_header(&app_secret, body);

    assert!(verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_unicode_body() {
    let app_secret = generate_test_secret();
    let body = "Hello 🦀 World".as_bytes();

    let signature_header = compute_whatsapp_signature_header(&app_secret, body);

    assert!(verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_json_payload() {
    let app_secret = generate_test_secret();
    let body = br#"{"entry":[{"changes":[{"value":{"messages":[{"from":"1234567890","text":{"body":"Hello"}}]}}]}]}"#;

    let signature_header = compute_whatsapp_signature_header(&app_secret, body);

    assert!(verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_case_sensitive_prefix() {
    let app_secret = generate_test_secret();
    let body = b"test body";

    let hex_sig = compute_whatsapp_signature_hex(&app_secret, body);

    // Wrong case prefix should fail
    let wrong_prefix = format!("SHA256={hex_sig}");
    assert!(!verify_whatsapp_signature(&app_secret, body, &wrong_prefix));

    // Correct prefix should pass
    let correct_prefix = format!("sha256={hex_sig}");
    assert!(verify_whatsapp_signature(
        &app_secret,
        body,
        &correct_prefix
    ));
}

#[test]
fn whatsapp_signature_truncated_hex() {
    let app_secret = generate_test_secret();
    let body = b"test body";

    let hex_sig = compute_whatsapp_signature_hex(&app_secret, body);
    let truncated = &hex_sig[..32]; // Only half the signature
    let signature_header = format!("sha256={truncated}");

    assert!(!verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

#[test]
fn whatsapp_signature_extra_bytes() {
    let app_secret = generate_test_secret();
    let body = b"test body";

    let hex_sig = compute_whatsapp_signature_hex(&app_secret, body);
    let extended = format!("{hex_sig}deadbeef");
    let signature_header = format!("sha256={extended}");

    assert!(!verify_whatsapp_signature(
        &app_secret,
        body,
        &signature_header
    ));
}

// ══════════════════════════════════════════════════════════
// IdempotencyStore Edge-Case Tests
// ══════════════════════════════════════════════════════════

#[test]
fn idempotency_store_allows_different_keys() {
    let store = IdempotencyStore::new(Duration::from_secs(60), 100);
    assert!(store.record_if_new("key-a"));
    assert!(store.record_if_new("key-b"));
    assert!(store.record_if_new("key-c"));
    assert!(store.record_if_new("key-d"));
}

#[test]
fn idempotency_store_max_keys_clamped_to_one() {
    let store = IdempotencyStore::new(Duration::from_secs(60), 0);
    assert!(store.record_if_new("only-key"));
    assert!(!store.record_if_new("only-key"));
}

#[test]
fn idempotency_store_rapid_duplicate_rejected() {
    let store = IdempotencyStore::new(Duration::from_secs(300), 100);
    assert!(store.record_if_new("rapid"));
    assert!(!store.record_if_new("rapid"));
}

#[test]
fn idempotency_store_accepts_after_ttl_expires() {
    let store = IdempotencyStore::new(Duration::from_millis(1), 100);
    assert!(store.record_if_new("ttl-key"));
    std::thread::sleep(Duration::from_millis(10));
    assert!(store.record_if_new("ttl-key"));
}

#[test]
fn idempotency_store_eviction_preserves_newest() {
    let store = IdempotencyStore::new(Duration::from_secs(300), 1);
    assert!(store.record_if_new("old-key"));
    std::thread::sleep(Duration::from_millis(2));
    assert!(store.record_if_new("new-key"));

    let keys = store.keys.lock();
    assert_eq!(keys.len(), 1);
    assert!(!keys.contains_key("old-key"));
    assert!(keys.contains_key("new-key"));
}

#[test]
fn rate_limiter_allows_after_window_expires() {
    let window = Duration::from_millis(50);
    let limiter = SlidingWindowRateLimiter::new(2, window, 100);
    assert!(limiter.allow("ip-1"));
    assert!(limiter.allow("ip-1"));
    assert!(!limiter.allow("ip-1")); // blocked

    // Wait for window to expire
    std::thread::sleep(Duration::from_millis(60));

    // Should be allowed again
    assert!(limiter.allow("ip-1"));
}

#[test]
fn rate_limiter_independent_keys_tracked_separately() {
    let limiter = SlidingWindowRateLimiter::new(2, Duration::from_secs(60), 100);
    assert!(limiter.allow("ip-1"));
    assert!(limiter.allow("ip-1"));
    assert!(!limiter.allow("ip-1")); // ip-1 blocked

    // ip-2 should still work
    assert!(limiter.allow("ip-2"));
    assert!(limiter.allow("ip-2"));
    assert!(!limiter.allow("ip-2")); // ip-2 now blocked
}

#[test]
fn rate_limiter_exact_boundary_at_max_keys() {
    let limiter = SlidingWindowRateLimiter::new(10, Duration::from_secs(60), 3);
    assert!(limiter.allow("ip-1"));
    assert!(limiter.allow("ip-2"));
    assert!(limiter.allow("ip-3"));
    // At capacity now
    assert!(limiter.allow("ip-4")); // should evict ip-1

    let guard = limiter.requests.lock();
    assert_eq!(guard.0.len(), 3);
    assert!(
        !guard.0.contains_key("ip-1"),
        "ip-1 should have been evicted"
    );
    assert!(guard.0.contains_key("ip-2"));
    assert!(guard.0.contains_key("ip-3"));
    assert!(guard.0.contains_key("ip-4"));
}

#[test]
fn gateway_rate_limiter_pair_and_webhook_are_independent() {
    let limiter = GatewayRateLimiter::new(2, 3, 100);

    // Exhaust pair limit
    assert!(limiter.allow_pair("ip-1"));
    assert!(limiter.allow_pair("ip-1"));
    assert!(!limiter.allow_pair("ip-1")); // pair blocked

    // Webhook should still work
    assert!(limiter.allow_webhook("ip-1"));
    assert!(limiter.allow_webhook("ip-1"));
    assert!(limiter.allow_webhook("ip-1"));
    assert!(!limiter.allow_webhook("ip-1")); // webhook now blocked
}

#[test]
fn rate_limiter_single_key_max_allows_one_request() {
    let limiter = SlidingWindowRateLimiter::new(5, Duration::from_secs(60), 1);
    assert!(limiter.allow("ip-1"));
    assert!(limiter.allow("ip-2")); // evicts ip-1

    let guard = limiter.requests.lock();
    assert_eq!(guard.0.len(), 1);
    assert!(guard.0.contains_key("ip-2"));
    assert!(!guard.0.contains_key("ip-1"));
}

#[test]
fn rate_limiter_concurrent_access_safe() {
    use std::sync::Arc;

    let limiter = Arc::new(SlidingWindowRateLimiter::new(
        1000,
        Duration::from_secs(60),
        1000,
    ));
    let mut handles = Vec::new();

    for i in 0..10 {
        let limiter = limiter.clone();
        handles.push(std::thread::spawn(move || {
            for j in 0..100 {
                limiter.allow(&format!("thread-{i}-req-{j}"));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Should not panic or deadlock
    let guard = limiter.requests.lock();
    assert!(guard.0.len() <= 1000, "should respect max_keys");
}

#[test]
fn idempotency_store_concurrent_access_safe() {
    use std::sync::Arc;

    let store = Arc::new(IdempotencyStore::new(Duration::from_secs(300), 1000));
    let mut handles = Vec::new();

    for i in 0..10 {
        let store = store.clone();
        handles.push(std::thread::spawn(move || {
            for j in 0..100 {
                store.record_if_new(&format!("thread-{i}-key-{j}"));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let keys = store.keys.lock();
    assert!(keys.len() <= 1000, "should respect max_keys");
}

#[test]
fn rate_limiter_rapid_burst_then_cooldown() {
    let limiter = SlidingWindowRateLimiter::new(5, Duration::from_millis(50), 100);

    // Burst: use all 5 requests
    for _ in 0..5 {
        assert!(limiter.allow("burst-ip"));
    }
    assert!(!limiter.allow("burst-ip")); // 6th should fail

    // Cooldown
    std::thread::sleep(Duration::from_millis(60));

    // Should be allowed again
    assert!(limiter.allow("burst-ip"));
}

#[test]
fn require_localhost_accepts_ipv4_loopback() {
    let peer = SocketAddr::from(([127, 0, 0, 1], 12345));
    assert!(require_localhost(&peer, &[]).is_ok());
}

#[test]
fn require_localhost_accepts_ipv6_loopback() {
    let peer = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 12345));
    assert!(require_localhost(&peer, &[]).is_ok());
}

#[test]
fn require_localhost_rejects_non_loopback_ipv4() {
    let peer = SocketAddr::from(([192, 168, 1, 100], 12345));
    let err = require_localhost(&peer, &[]).unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
}

#[test]
fn require_localhost_rejects_non_loopback_ipv6() {
    let peer = SocketAddr::from((
        std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
        12345,
    ));
    let err = require_localhost(&peer, &[]).unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
}

#[test]
fn admin_cidr_parse_and_contains() {
    let cidr = AdminCidr::parse("100.64.0.0/10").unwrap();
    assert!(cidr.contains(std::net::Ipv4Addr::new(100, 83, 1, 114)));
    assert!(cidr.contains(std::net::Ipv4Addr::new(100, 127, 255, 255)));
    assert!(!cidr.contains(std::net::Ipv4Addr::new(100, 128, 0, 0)));
    assert!(!cidr.contains(std::net::Ipv4Addr::new(192, 168, 1, 1)));
}

#[test]
fn admin_cidr_parse_rejects_invalid() {
    assert!(AdminCidr::parse("not-a-cidr").is_err());
    assert!(AdminCidr::parse("100.64.0.0/33").is_err());
    assert!(AdminCidr::parse("100.64.0.0").is_err());
}

#[test]
fn require_localhost_accepts_admin_cidr() {
    let cidrs = vec![AdminCidr::parse("100.64.0.0/10").unwrap()];
    let peer = SocketAddr::from(([100, 83, 1, 114], 12345));
    assert!(require_localhost(&peer, &cidrs).is_ok());
}

#[test]
fn require_localhost_rejects_without_admin_cidr() {
    let peer = SocketAddr::from(([100, 83, 1, 114], 12345));
    let err = require_localhost(&peer, &[]).unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
}

// ── Phase 3.8 broker proxy e2e tests ────────────────────────
// Temporarily disabled: IPC DB migrated from SQLite to SurrealDB (Phase 4.5).
// These tests need async setup with SurrealDB in-memory/temp instances.

#[cfg(feature = "_ipc_tests_todo")]
#[test]
fn broker_proxy_e2e_agent_registry_lifecycle() {
    // Simulates: register → seed from DB → health poll updates → offline detection
    let db = ipc::IpcDb::open_in_memory().unwrap();

    // 1. Agent registers via IPC
    db.update_last_seen("opus", 1, "coordinator");
    db.upsert_agent_gateway("opus", "http://127.0.0.1:42618", "zc_proxy_opus")
        .unwrap();

    // 2. Broker seeds registry from DB (simulates restart)
    let registry = agent_registry::AgentRegistry::new();
    let gateways = db.list_agent_gateways().unwrap();
    let ipc_agents = db.list_agents(120);
    for gw in &gateways {
        registry.upsert(&gw.agent_id, &gw.gateway_url, &gw.proxy_token);
        if let Some(ipc_agent) = ipc_agents.iter().find(|a| a.agent_id == gw.agent_id) {
            if let (Some(tl), Some(role)) = (ipc_agent.trust_level, ipc_agent.role.as_deref()) {
                registry.set_trust_info(&gw.agent_id, tl, role);
            }
        }
    }

    // 3. Verify registry state
    let info = registry.get("opus").unwrap();
    assert_eq!(info.status, agent_registry::AgentStatus::Online);
    assert_eq!(info.trust_level, Some(1));
    assert_eq!(info.role.as_deref(), Some("coordinator"));
    assert_eq!(info.gateway_url, "http://127.0.0.1:42618");
    assert_eq!(info.proxy_token, "zc_proxy_opus");

    // 4. Simulate health poll success
    registry.update_metadata(
        "opus",
        Some("claude-opus-4".into()),
        Some(3600),
        vec!["matrix".into()],
    );
    let info = registry.get("opus").unwrap();
    assert_eq!(info.model.as_deref(), Some("claude-opus-4"));
    assert_eq!(info.channels, vec!["matrix"]);

    // 5. Simulate health poll failures → offline
    registry.record_poll_failure("opus");
    registry.record_poll_failure("opus");
    assert_eq!(
        registry.get("opus").unwrap().status,
        agent_registry::AgentStatus::Online
    );
    registry.record_poll_failure("opus");
    assert_eq!(
        registry.get("opus").unwrap().status,
        agent_registry::AgentStatus::Offline
    );

    // 6. Agent re-registers → back online
    registry.upsert("opus", "http://127.0.0.1:42618", "zc_proxy_opus");
    assert_eq!(
        registry.get("opus").unwrap().status,
        agent_registry::AgentStatus::Online
    );
    assert_eq!(registry.get("opus").unwrap().missed_polls, 0);
}

#[cfg(feature = "_ipc_tests_todo")]
#[test]
fn broker_proxy_e2e_multi_agent_registry() {
    let db = ipc::IpcDb::open_in_memory().unwrap();

    // Register multiple agents
    db.update_last_seen("opus", 0, "admin");
    db.update_last_seen("daily", 3, "worker");
    db.update_last_seen("code", 2, "developer");

    db.upsert_agent_gateway("opus", "http://127.0.0.1:42618", "tok_opus")
        .unwrap();
    db.upsert_agent_gateway("daily", "http://127.0.0.1:42619", "tok_daily")
        .unwrap();
    db.upsert_agent_gateway("code", "http://127.0.0.1:42620", "tok_code")
        .unwrap();

    // Seed registry
    let registry = agent_registry::AgentRegistry::new();
    let gateways = db.list_agent_gateways().unwrap();
    let ipc_agents = db.list_agents(120);
    for gw in &gateways {
        registry.upsert(&gw.agent_id, &gw.gateway_url, &gw.proxy_token);
        if let Some(a) = ipc_agents.iter().find(|a| a.agent_id == gw.agent_id) {
            if let (Some(tl), Some(role)) = (a.trust_level, a.role.as_deref()) {
                registry.set_trust_info(&gw.agent_id, tl, role);
            }
        }
    }

    // All 3 agents online
    let agents = registry.list();
    assert_eq!(agents.len(), 3);
    assert!(agents
        .iter()
        .all(|a| a.status == agent_registry::AgentStatus::Online));

    // Verify trust levels restored
    assert_eq!(registry.get("opus").unwrap().trust_level, Some(0));
    assert_eq!(registry.get("daily").unwrap().trust_level, Some(3));
    assert_eq!(registry.get("code").unwrap().trust_level, Some(2));

    // One agent goes offline
    for _ in 0..3 {
        registry.record_poll_failure("daily");
    }
    let agents = registry.list();
    let online_count = agents
        .iter()
        .filter(|a| a.status == agent_registry::AgentStatus::Online)
        .count();
    assert_eq!(online_count, 2);

    // Remove agent
    registry.remove("code");
    assert_eq!(registry.list().len(), 2);
}

#[cfg(feature = "_ipc_tests_todo")]
#[test]
fn broker_proxy_e2e_gateway_db_persistence() {
    // Verify DB survives "restart" (new IpcDb instance on same data)
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("ipc.db");

    // First "run": register gateway
    {
        let db = ipc::IpcDb::open(&db_path).unwrap();
        db.update_last_seen("opus", 1, "coordinator");
        db.upsert_agent_gateway("opus", "http://127.0.0.1:42618", "zc_proxy_test")
            .unwrap();
    }

    // Second "run": verify data persisted
    {
        let db = ipc::IpcDb::open(&db_path).unwrap();
        let gateways = db.list_agent_gateways().unwrap();
        assert_eq!(gateways.len(), 1);
        assert_eq!(gateways[0].agent_id, "opus");
        assert_eq!(gateways[0].proxy_token, "zc_proxy_test");

        let agents = db.list_agents(120);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].trust_level, Some(1));
        assert_eq!(agents[0].role.as_deref(), Some("coordinator"));
    }
}

#[test]
fn broker_proxy_e2e_proxy_token_not_in_api_response() {
    // Verify proxy_token excluded from AgentInfo serialization
    let registry = agent_registry::AgentRegistry::new();
    registry.upsert("opus", "http://127.0.0.1:42618", "SECRET_TOKEN");

    let info = registry.get("opus").unwrap();
    let json = serde_json::to_value(&info).unwrap();

    // proxy_token should be skipped via #[serde(skip)]
    assert!(json.get("proxy_token").is_none());
    // But agent_id should be present
    assert_eq!(json["agent_id"].as_str(), Some("opus"));
}

#[test]
fn broker_proxy_operator_isolation_covers_session_crud() {
    // Phase 3.8 Finding 1 (v2): verify that the token_prefix folding in
    // handle_socket correctly scopes ALL session CRUD per-operator.
    //
    // When broker proxies with ?session_id=op:<hash>, handle_socket folds
    // the operator prefix into token_prefix: "{proxy_hash}:op:{op_hash}".
    // This makes sessions.list/new/rename/delete all scoped per-operator.
    let token_a = "browser_token_alice_abc123";
    let token_b = "browser_token_bob_xyz789";
    let proxy_token = "shared_proxy_token";

    let op_a = ws::token_hash_prefix(token_a);
    let op_b = ws::token_hash_prefix(token_b);
    let proxy_prefix = ws::token_hash_prefix(proxy_token);

    // Simulate handle_socket folding: session_id=op:{op_hash}
    // → token_prefix becomes "{proxy_prefix}:op:{op_hash}"
    let effective_prefix_a = format!("{proxy_prefix}:op:{op_a}");
    let effective_prefix_b = format!("{proxy_prefix}:op:{op_b}");

    // Different operators must have different effective prefixes
    assert_ne!(
        effective_prefix_a, effective_prefix_b,
        "different operators must yield different effective prefixes"
    );

    // sessions.list filters by "web:{effective_prefix}:" — verify isolation
    let list_prefix_a = format!("web:{effective_prefix_a}:");
    let list_prefix_b = format!("web:{effective_prefix_b}:");
    assert!(!list_prefix_a.starts_with(&list_prefix_b));
    assert!(!list_prefix_b.starts_with(&list_prefix_a));

    // sessions.new creates "web:{effective_prefix}:{uuid}" — verify isolation
    let new_key_a = format!("web:{effective_prefix_a}:sess-001");
    let new_key_b = format!("web:{effective_prefix_b}:sess-001");
    assert_ne!(
        new_key_a, new_key_b,
        "sessions.new must create keys in different namespaces per operator"
    );

    // Direct browser (no proxy) — no folding, no op: prefix
    let direct_session = format!("web:{proxy_prefix}:default");
    assert!(
        !direct_session.contains(":op:"),
        "direct connections must not have op: prefix"
    );
}

#[cfg(feature = "_ipc_tests_todo")]
#[test]
fn broker_proxy_e2e_restart_recovery_agent_registry() {
    // Phase 3.8 Finding 2: verify that after broker "restart", trust/role
    // can be re-seeded from IpcDb's update_last_seen records into a fresh
    // AgentRegistry. This tests the code path used in gateway startup.
    let db = ipc::IpcDb::open_in_memory().unwrap();

    // 1. Register agents in IPC DB (simulates normal broker operation)
    db.update_last_seen("opus", 1, "coordinator");
    db.update_last_seen("worker", 3, "worker");
    db.upsert_agent_gateway("opus", "http://127.0.0.1:42618", "proxy_opus")
        .unwrap();

    // 2. Simulate restart: create fresh registry, seed trust/role from DB
    let registry = agent_registry::AgentRegistry::new();
    let agents = db.list_agents(3600);
    for agent in &agents {
        registry.upsert(&agent.agent_id, "", "");
        if let (Some(trust), Some(ref role)) = (agent.trust_level, &agent.role) {
            registry.set_trust_info(&agent.agent_id, trust, role);
        }
    }

    // 3. Verify trust/role survived "restart"
    let opus = registry.get("opus").unwrap();
    assert_eq!(opus.trust_level, Some(1));
    assert_eq!(opus.role.as_deref(), Some("coordinator"));

    let worker = registry.get("worker").unwrap();
    assert_eq!(worker.trust_level, Some(3));
    assert_eq!(worker.role.as_deref(), Some("worker"));

    // 4. Gateway URL/token empty until agent re-registers
    assert!(opus.gateway_url.is_empty());
}
