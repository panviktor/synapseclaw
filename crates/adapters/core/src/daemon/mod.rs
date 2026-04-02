use anyhow::Result;
use chrono::Utc;
use std::future::Future;
use std::path::PathBuf;
use synapse_domain::config::schema::Config;
use synapse_domain::domain::config::{AutoDetectCandidate, HeartbeatConfig};
use tokio::task::JoinHandle;
use tokio::time::Duration;

const STATUS_FLUSH_SECONDS: u64 = 5;

/// Bridges the adapters `health` module to `synapse_cron::scheduler::HealthReporter`.
pub struct CronHealthReporter;

impl synapse_cron::scheduler::HealthReporter for CronHealthReporter {
    fn mark_ok(&self, component: &str) {
        crate::health::mark_component_ok(component);
    }
    fn mark_error(&self, component: &str, error: String) {
        crate::health::mark_component_error(component, error);
    }
    fn snapshot_json(&self) -> serde_json::Value {
        crate::health::snapshot_json()
    }
}

/// Bridges `DeliveryService` to `synapse_cron::scheduler::CronDeliveryPort`.
pub struct CronDeliveryAdapter {
    pub delivery_service:
        std::sync::Arc<synapse_domain::application::services::delivery_service::DeliveryService>,
}

#[async_trait::async_trait]
impl synapse_cron::scheduler::CronDeliveryPort for CronDeliveryAdapter {
    async fn deliver_cron_output(
        &self,
        delivery: &synapse_domain::domain::config::CronDeliveryConfig,
        output: &str,
    ) -> anyhow::Result<()> {
        self.delivery_service
            .deliver_cron_output(delivery, output)
            .await
    }
}

/// Wait for shutdown signal (SIGINT or SIGTERM).
/// SIGHUP is explicitly ignored so the daemon survives terminal/SSH disconnects.
async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sighup = signal(SignalKind::hangup())?;

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    tracing::info!("Received SIGINT, shutting down...");
                    break;
                }
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM, shutting down...");
                    break;
                }
                _ = sighup.recv() => {
                    tracing::info!("Received SIGHUP, ignoring (daemon stays running)");
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        tracing::info!("Received Ctrl+C, shutting down...");
    }

    Ok(())
}

pub async fn run(
    config: Config,
    host: String,
    port: u16,
    agent_runner: std::sync::Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
) -> Result<()> {
    let initial_backoff = config.reliability.channel_initial_backoff_secs.max(1);
    let max_backoff = config
        .reliability
        .channel_max_backoff_secs
        .max(initial_backoff);

    crate::health::mark_component_ok("daemon");

    if config.heartbeat.enabled {
        let _ =
            crate::heartbeat::engine::HeartbeatEngine::ensure_heartbeat_file(&config.workspace_dir)
                .await;
    }

    let mut handles: Vec<JoinHandle<()>> = vec![spawn_state_writer(config.clone())];

    // ── Phase 4.1: Shared IpcClient for pipeline executor + agent tools ──
    // One Arc<IpcClient> with a single AtomicI64 seq counter shared across
    // gateway, channels, and tools. Prevents replay_rejected from duplicate
    // seq numbers when multiple components send signed IPC messages.
    let shared_ipc_client: Option<
        std::sync::Arc<dyn synapse_domain::ports::ipc_client::IpcClientPort>,
    > = if config.agents_ipc.enabled {
        if let Some(ref token) = config.agents_ipc.broker_token {
            let mut client = crate::tools::agents_ipc::IpcClient::new(
                &config.agents_ipc.broker_url,
                token,
                config.agents_ipc.request_timeout_secs,
            );
            let key_path = config
                .config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("agent.key");
            match synapse_security::identity::AgentIdentity::load_or_generate(&key_path) {
                Ok(identity) => {
                    let agent_id = config
                        .agents_ipc
                        .agent_id
                        .clone()
                        .unwrap_or_else(|| config.agents_ipc.role.clone());
                    tracing::info!(
                        agent_id = %agent_id,
                        "daemon: shared IpcClient with Ed25519 identity"
                    );
                    client = client.with_identity(identity, agent_id);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "daemon: Ed25519 identity unavailable");
                }
            }
            let client = std::sync::Arc::new(client);
            // Fire-and-forget key registration with background retry.
            // Gateway listener may not be up yet — retries handle that.
            {
                let c = client.clone();
                tokio::spawn(async move {
                    {
                        let _ = c.register_public_key().await;
                    }
                });
            }
            Some(client)
        } else {
            None
        }
    } else {
        None
    };

    // ── Phase 4.0: ChannelRegistryPort ─────────────────────────────
    // Always available in daemon mode.  Shared between relay, heartbeat,
    // scheduler, delivery service, and gateway REST API.
    let channel_registry: std::sync::Arc<
        dyn synapse_domain::ports::channel_registry::ChannelRegistryPort,
    > = std::sync::Arc::new(crate::channels::registry::CachedChannelRegistry::new(
        config.clone(),
        std::sync::Arc::new(crate::channels::build_channel_by_id),
    ));

    // Phase 4.0 Slice 1: DeliveryService owns delivery policy.
    let delivery_service = std::sync::Arc::new(
        synapse_domain::application::services::delivery_service::DeliveryService::new(
            channel_registry.clone(),
        ),
    );

    // OutboundIntent bus: gateway emits intents, relay delivers via registry.
    let outbound_tx = if config.agents_ipc.push_relay_channel.is_some()
        && config.agents_ipc.push_relay_recipient.is_some()
    {
        let (tx, rx) = synapse_domain::bus::outbound_intent_bus();
        let relay_registry = channel_registry.clone();
        handles.push(tokio::spawn(async move {
            Box::pin(outbound_intent_relay(relay_registry, rx)).await;
        }));
        Some(tx)
    } else {
        None
    };

    // ── Phase 4.3: Shared memory — create ONCE, share across all components ──
    // SurrealKV locks the DB file; creating multiple adapters from different
    // components causes contention. Create the raw adapter here and pass it
    // to gateway, channels, and the consolidation worker.
    let daemon_agent_id = crate::agent::loop_::resolve_agent_id(&config);
    let mem_backend = match synapse_memory::create_memory(
        &config.memory,
        &config.workspace_dir,
        &daemon_agent_id,
        config.api_key.as_deref(),
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Memory init failed in daemon: {e} — using noop memory");
            let noop = std::sync::Arc::new(synapse_memory::NoopUnifiedMemory);
            synapse_memory::MemoryBackend {
                memory: noop.clone(),
                dead_letter: noop,
                surreal: None,
            }
        }
    };
    let shared_raw_mem = mem_backend.memory;
    let shared_dead_letter = mem_backend.dead_letter;
    let shared_surreal = mem_backend.surreal;

    // Replace AgentRunner with one that shares memory (avoids SurrealKV LOCK conflicts)
    let agent_runner: std::sync::Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort> = {
        let config_for_runner = std::sync::Arc::new(std::sync::Mutex::new(config.clone()));
        std::sync::Arc::new(
            crate::agent::runner_adapter::AgentRunner::with_shared_memory(
                config_for_runner,
                shared_raw_mem.clone(),
            ),
        )
    };

    {
        let gateway_cfg = config.clone();
        let gateway_host = host.clone();
        let gw_outbound_tx = outbound_tx.clone();
        let gw_registry = Some(channel_registry.clone());
        let gw_ipc = shared_ipc_client.clone();
        let gw_runner = agent_runner.clone();
        let gw_mem = shared_raw_mem.clone();
        let gw_dlq = shared_dead_letter.clone();
        let gw_surreal = shared_surreal.clone();
        handles.push(spawn_component_supervisor(
            "gateway",
            initial_backoff,
            max_backoff,
            move || {
                let cfg = gateway_cfg.clone();
                let host = gateway_host.clone();
                let otx = gw_outbound_tx.clone();
                let reg = gw_registry.clone();
                let ipc = gw_ipc.clone();
                let ar = gw_runner.clone();
                let mem = gw_mem.clone();
                let dlq = gw_dlq.clone();
                let surreal = gw_surreal.clone();
                async move {
                    Box::pin(crate::gateway::run_gateway(
                        &host,
                        port,
                        cfg,
                        otx,
                        reg,
                        ipc,
                        ar,
                        Some(mem),
                        Some(dlq),
                        surreal,
                    ))
                    .await
                }
            },
        ));
    }

    {
        if has_supervised_channels(&config) {
            let channels_cfg = config.clone();
            let ch_ipc = shared_ipc_client.clone();
            let ch_mem = shared_raw_mem.clone();
            let ch_surreal = shared_surreal.clone();
            handles.push(spawn_component_supervisor(
                "channels",
                initial_backoff,
                max_backoff,
                move || {
                    let cfg = channels_cfg.clone();
                    let ipc = ch_ipc.clone();
                    let mem = ch_mem.clone();
                    let surreal = ch_surreal.clone();
                    async move {
                        Box::pin(crate::channels::start_channels(
                            cfg,
                            ipc,
                            Some(mem),
                            surreal,
                        ))
                        .await
                    }
                },
            ));
        } else {
            crate::health::mark_component_ok("channels");
            tracing::info!("No real-time channels configured; channel supervisor disabled");
        }
    }

    if config.heartbeat.enabled {
        let heartbeat_cfg = config.clone();
        let hb_delivery = delivery_service.clone();
        let heartbeat_runner = agent_runner.clone();
        let hb_surreal = shared_surreal.clone();
        handles.push(spawn_component_supervisor(
            "heartbeat",
            initial_backoff,
            max_backoff,
            move || {
                let cfg = heartbeat_cfg.clone();
                let ds = hb_delivery.clone();
                let ar = heartbeat_runner.clone();
                let surreal = hb_surreal.clone();
                async move { Box::pin(run_heartbeat_worker(cfg, ds, ar, surreal)).await }
            },
        ));
    }

    if config.cron.enabled {
        let scheduler_cfg = config.clone();
        let sched_delivery = delivery_service.clone();
        let sched_surreal = shared_surreal.clone();
        handles.push(spawn_component_supervisor(
            "scheduler",
            initial_backoff,
            max_backoff,
            move || {
                let cfg = scheduler_cfg.clone();
                let db = sched_surreal.clone();
                let ds: std::sync::Arc<dyn synapse_cron::scheduler::CronDeliveryPort> =
                    std::sync::Arc::new(CronDeliveryAdapter {
                        delivery_service: sched_delivery.clone(),
                    });
                let ar = agent_runner.clone();
                async move {
                    let health = std::sync::Arc::new(CronHealthReporter);
                    let db =
                        db.ok_or_else(|| anyhow::anyhow!("SurrealDB not available for scheduler"))?;
                    Box::pin(synapse_cron::scheduler::run(cfg, db, ds, ar, health)).await
                }
            },
        ));
    } else {
        crate::health::mark_component_ok("scheduler");
        tracing::info!("Cron disabled; scheduler supervisor not started");
    }

    // Phase 3.8: agent auto-registration with broker
    if config.agents_ipc.enabled
        && config.agents_ipc.broker_token.is_some()
        && config.agents_ipc.gateway_url.is_some()
        && config.agents_ipc.proxy_token.is_some()
    {
        let reg_cfg = config.clone();
        handles.push(tokio::spawn(async move {
            Box::pin(broker_registration_loop(reg_cfg)).await;
        }));
    }

    // ── Phase 4.3: Memory consolidation worker ────────────────────
    {
        // Wrap with ConsolidatingMemory if provider available.
        let consolidation_model = config
            .default_model
            .clone()
            .unwrap_or_else(|| "anthropic/claude-sonnet-4".into());
        let provider_for_worker: Option<(
            std::sync::Arc<dyn synapse_providers::traits::Provider>,
            String,
        )>;
        let mem: std::sync::Arc<dyn synapse_memory::UnifiedMemoryPort> =
            match synapse_providers::create_resilient_provider(
                config.default_provider.as_deref().unwrap_or("openrouter"),
                config.api_key.as_deref(),
                config.api_url.as_deref(),
                &config.reliability,
            ) {
                Ok(p) => {
                    let prov_arc: std::sync::Arc<dyn synapse_providers::traits::Provider> =
                        std::sync::Arc::from(p);
                    provider_for_worker = Some((prov_arc.clone(), consolidation_model.clone()));
                    std::sync::Arc::new(
                        crate::memory_adapters::instrumented::InstrumentedMemory::new(
                            std::sync::Arc::new(
                                crate::memory_adapters::memory_adapter::ConsolidatingMemory::new(
                                    shared_raw_mem.clone(),
                                    prov_arc,
                                    consolidation_model,
                                    daemon_agent_id.clone(),
                                    shared_ipc_client.clone(),
                                ),
                            ),
                        ),
                    )
                }
                Err(e) => {
                    tracing::warn!("Consolidation provider unavailable: {e}");
                    provider_for_worker = None;
                    shared_raw_mem.clone()
                }
            };

        let worker_handle =
            crate::memory_adapters::consolidation_worker::spawn_consolidation_worker(
                mem,
                crate::memory_adapters::consolidation_worker::ConsolidationWorkerConfig::default(),
                daemon_agent_id.clone(),
                provider_for_worker,
            );
        handles.push(worker_handle);
    }

    println!("🧠 SynapseClaw daemon started");
    println!("   Gateway:  http://{host}:{port}");
    println!("   Components: gateway, channels, heartbeat, scheduler, memory");
    if config.gateway.require_pairing {
        println!("   Pairing:    enabled (code appears in gateway output above)");
    }
    println!("   Ctrl+C or SIGTERM to stop");

    // Wait for shutdown signal (SIGINT or SIGTERM)
    wait_for_shutdown_signal().await?;
    crate::health::mark_component_error("daemon", "shutdown requested");

    for handle in &handles {
        handle.abort();
    }
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

pub fn state_file_path(config: &Config) -> PathBuf {
    config
        .config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join("daemon_state.json")
}

fn spawn_state_writer(config: Config) -> JoinHandle<()> {
    tokio::spawn(async move {
        let path = state_file_path(&config);
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let mut interval = tokio::time::interval(Duration::from_secs(STATUS_FLUSH_SECONDS));
        loop {
            interval.tick().await;
            let mut json = crate::health::snapshot_json();
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "written_at".into(),
                    serde_json::json!(Utc::now().to_rfc3339()),
                );
            }
            let data = serde_json::to_vec_pretty(&json).unwrap_or_else(|_| b"{}".to_vec());
            let _ = tokio::fs::write(&path, data).await;
        }
    })
}

fn spawn_component_supervisor<F, Fut>(
    name: &'static str,
    initial_backoff_secs: u64,
    max_backoff_secs: u64,
    mut run_component: F,
) -> JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut backoff = initial_backoff_secs.max(1);
        let max_backoff = max_backoff_secs.max(backoff);

        loop {
            crate::health::mark_component_ok(name);
            match run_component().await {
                Ok(()) => {
                    crate::health::mark_component_error(name, "component exited unexpectedly");
                    tracing::warn!("Daemon component '{name}' exited unexpectedly");
                    // Clean exit — reset backoff since the component ran successfully
                    backoff = initial_backoff_secs.max(1);
                }
                Err(e) => {
                    crate::health::mark_component_error(name, e.to_string());
                    tracing::error!("Daemon component '{name}' failed: {e}");
                }
            }

            crate::health::bump_component_restart(name);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            // Double backoff AFTER sleeping so first error uses initial_backoff
            backoff = backoff.saturating_mul(2).min(max_backoff);
        }
    })
}

// ── Config projection helpers for synapse_domain types ───────────────
//
// These convert from the upstream `Config` to the lean domain types
// that `DeliveryService` expects, keeping synapse_domain free from
// upstream config dependencies.

fn heartbeat_config_from(config: &Config) -> HeartbeatConfig {
    HeartbeatConfig {
        target: config.heartbeat.target.clone(),
        to: config.heartbeat.to.clone(),
        deadman_channel: config.heartbeat.deadman_channel.clone(),
        deadman_to: config.heartbeat.deadman_to.clone(),
    }
}

fn auto_detect_candidates(config: &Config) -> Vec<AutoDetectCandidate> {
    let mut candidates = Vec::new();

    // Priority order mirrors the old auto_detect_heartbeat_channel:
    // matrix > telegram (channels where allowed_users[0] works as recipient).
    if let Some(mx) = &config.channels_config.matrix {
        candidates.push(AutoDetectCandidate {
            channel_name: "matrix".into(),
            recipient: mx.allowed_users.first().cloned().filter(|u| !u.is_empty()),
        });
    }
    if let Some(tg) = &config.channels_config.telegram {
        candidates.push(AutoDetectCandidate {
            channel_name: "telegram".into(),
            recipient: tg.allowed_users.first().cloned().filter(|u| !u.is_empty()),
        });
    }

    candidates
}

async fn run_heartbeat_worker(
    config: Config,
    delivery_service: std::sync::Arc<
        synapse_domain::application::services::delivery_service::DeliveryService,
    >,
    agent_runner: std::sync::Arc<dyn synapse_domain::ports::agent_runner::AgentRunnerPort>,
    surreal: Option<std::sync::Arc<synapse_memory::Surreal<synapse_memory::SurrealDb>>>,
) -> Result<()> {
    use crate::heartbeat::engine::{
        compute_adaptive_interval, HeartbeatEngine, HeartbeatTask, TaskPriority, TaskStatus,
    };
    use std::sync::Arc;

    let observer: std::sync::Arc<dyn synapse_observability::Observer> = std::sync::Arc::from(
        synapse_observability::create_observer(&config.observability),
    );
    let engine = HeartbeatEngine::new(
        config.heartbeat.clone(),
        config.workspace_dir.clone(),
        observer,
    );
    let metrics = engine.metrics();
    let hb_config = heartbeat_config_from(&config);
    let candidates = auto_detect_candidates(&config);
    let delivery = delivery_service.resolve_heartbeat_target(&hb_config, &candidates)?;
    let two_phase = config.heartbeat.two_phase;
    let adaptive = config.heartbeat.adaptive;
    let start_time = std::time::Instant::now();

    // ── Deadman watcher ──────────────────────────────────────────
    let deadman_timeout = config.heartbeat.deadman_timeout_minutes;
    if deadman_timeout > 0 {
        let dm_metrics = Arc::clone(&metrics);
        let dm_target = delivery_service.resolve_deadman_target(&hb_config, &delivery);
        let dm_delivery_svc = delivery_service.clone();
        tokio::spawn(async move {
            let check_interval = Duration::from_secs(60);
            let timeout = chrono::Duration::minutes(i64::from(deadman_timeout));
            loop {
                tokio::time::sleep(check_interval).await;
                let last_tick = dm_metrics.lock().last_tick_at;
                if let Some(last) = last_tick {
                    if chrono::Utc::now() - last > timeout {
                        let alert = format!(
                            "⚠️ Heartbeat dead-man's switch: no tick in {deadman_timeout} minutes"
                        );
                        if let Some(target) = &dm_target {
                            let _ = dm_delivery_svc.deliver(target, &alert).await;
                        }
                    }
                }
            }
        });
    }

    let base_interval = config.heartbeat.interval_minutes.max(5);
    let mut sleep_mins = base_interval;

    loop {
        tokio::time::sleep(Duration::from_secs(u64::from(sleep_mins) * 60)).await;

        // Update uptime
        {
            let mut m = metrics.lock();
            m.uptime_secs = start_time.elapsed().as_secs();
        }

        let tick_start = std::time::Instant::now();

        // Collect runnable tasks (active only, sorted by priority)
        let mut tasks = engine.collect_runnable_tasks().await?;
        let has_high_priority = tasks.iter().any(|t| t.priority == TaskPriority::High);

        if tasks.is_empty() {
            if let Some(fallback) = config
                .heartbeat
                .message
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
            {
                tasks.push(HeartbeatTask {
                    text: fallback.to_string(),
                    priority: TaskPriority::Medium,
                    status: TaskStatus::Active,
                });
            } else {
                #[allow(clippy::cast_precision_loss)]
                let elapsed = tick_start.elapsed().as_millis() as f64;
                metrics.lock().record_success(elapsed);
                continue;
            }
        }

        // ── Phase 1: LLM decision (two-phase mode) ──────────────
        let tasks_to_run = if two_phase {
            let decision_prompt = HeartbeatEngine::build_decision_prompt(&tasks);
            match agent_runner
                .run(
                    Some(decision_prompt),
                    None,
                    None,
                    0.0,
                    false,
                    None,
                    None,
                    None,
                )
                .await
            {
                Ok(response) => {
                    let indices = HeartbeatEngine::parse_decision_response(&response, tasks.len());
                    if indices.is_empty() {
                        tracing::info!("💓 Heartbeat Phase 1: skip (nothing to do)");
                        crate::health::mark_component_ok("heartbeat");
                        #[allow(clippy::cast_precision_loss)]
                        let elapsed = tick_start.elapsed().as_millis() as f64;
                        metrics.lock().record_success(elapsed);
                        continue;
                    }
                    tracing::info!(
                        "💓 Heartbeat Phase 1: run {} of {} tasks",
                        indices.len(),
                        tasks.len()
                    );
                    indices
                        .into_iter()
                        .filter_map(|i| tasks.get(i).cloned())
                        .collect()
                }
                Err(e) => {
                    tracing::warn!("💓 Heartbeat Phase 1 failed, running all tasks: {e}");
                    tasks
                }
            }
        } else {
            tasks
        };

        // ── Phase 2: Execute selected tasks ─────────────────────
        let mut tick_had_error = false;
        for task in &tasks_to_run {
            let task_start = std::time::Instant::now();
            let prompt = format!("[Heartbeat Task | {}] {}", task.priority, task.text);
            let temp = config.default_temperature;
            match agent_runner
                .run(Some(prompt), None, None, temp, false, None, None, None)
                .await
            {
                Ok(output) => {
                    crate::health::mark_component_ok("heartbeat");
                    #[allow(clippy::cast_possible_truncation)]
                    let duration_ms = task_start.elapsed().as_millis() as i64;
                    let now = chrono::Utc::now();
                    if let Some(ref db) = surreal {
                        let _ = crate::heartbeat::store::record_run(
                            db,
                            &task.text,
                            &task.priority.to_string(),
                            now - chrono::Duration::milliseconds(duration_ms),
                            now,
                            "ok",
                            Some(output.as_str()),
                            duration_ms,
                            config.heartbeat.max_run_history,
                        )
                        .await;
                    }
                    let announcement = if output.trim().is_empty() {
                        format!("💓 heartbeat task completed: {}", task.text)
                    } else {
                        output
                    };
                    if let Some(target) = &delivery {
                        if let Err(e) = delivery_service.deliver(target, &announcement).await {
                            crate::health::mark_component_error(
                                "heartbeat",
                                format!("delivery failed: {e}"),
                            );
                            tracing::warn!("Heartbeat delivery failed: {e}");
                        }
                    }
                }
                Err(e) => {
                    tick_had_error = true;
                    #[allow(clippy::cast_possible_truncation)]
                    let duration_ms = task_start.elapsed().as_millis() as i64;
                    let now = chrono::Utc::now();
                    if let Some(ref db) = surreal {
                        let _ = crate::heartbeat::store::record_run(
                            db,
                            &task.text,
                            &task.priority.to_string(),
                            now - chrono::Duration::milliseconds(duration_ms),
                            now,
                            "error",
                            Some(&e.to_string()),
                            duration_ms,
                            config.heartbeat.max_run_history,
                        )
                        .await;
                    }
                    crate::health::mark_component_error("heartbeat", e.to_string());
                    tracing::warn!("Heartbeat task failed: {e}");
                }
            }
        }

        // Update metrics
        #[allow(clippy::cast_precision_loss)]
        let tick_elapsed = tick_start.elapsed().as_millis() as f64;
        {
            let mut m = metrics.lock();
            if tick_had_error {
                m.record_failure(tick_elapsed);
            } else {
                m.record_success(tick_elapsed);
            }
        }

        // Compute next sleep interval
        if adaptive {
            let failures = metrics.lock().consecutive_failures;
            sleep_mins = compute_adaptive_interval(
                base_interval,
                config.heartbeat.min_interval_minutes,
                config.heartbeat.max_interval_minutes,
                failures,
                has_high_priority,
            );
        } else {
            sleep_mins = base_interval;
        }
    }
}

// ── Phase 4.0: OutboundIntent relay ──────────────────────────────
//
// Consumes outbound intents emitted by the gateway push inbox processor
// and delivers them via CachedChannelRegistry (ChannelRegistryPort).
// Long-lived adapters: stateful channels (Matrix) keep their authenticated
// SDK client alive across deliveries.
async fn outbound_intent_relay(
    registry: std::sync::Arc<dyn synapse_domain::ports::channel_registry::ChannelRegistryPort>,
    mut rx: synapse_domain::bus::OutboundIntentReceiver,
) {
    tracing::info!("OutboundIntent relay started (CachedChannelRegistry)");
    while let Some(intent) = rx.recv().await {
        let channel = intent.target_channel.clone();
        let recipient = intent.target_recipient.clone();
        let kind = intent.intent_kind.to_string();
        match registry.deliver(&intent).await {
            Ok(()) => {
                tracing::info!(
                    channel = %channel,
                    recipient = %recipient,
                    kind = %kind,
                    "OutboundIntent delivered"
                );
            }
            Err(e) => {
                tracing::warn!(
                    channel = %channel,
                    recipient = %recipient,
                    "OutboundIntent delivery failed: {e}"
                );
            }
        }
    }
    tracing::info!("OutboundIntent relay stopped (bus closed)");
}

fn has_supervised_channels(config: &Config) -> bool {
    config
        .channels_config
        .channels_except_webhook()
        .iter()
        .any(|(_, ok)| *ok)
}

// ── Broker registration loop (Phase 3.8) ────────────────────────

/// Two-phase registration: fast retry until first success, then periodic refresh.
async fn broker_registration_loop(config: Config) {
    let broker_url = config.agents_ipc.broker_url.clone();
    let broker_token = match &config.agents_ipc.broker_token {
        Some(t) => t.clone(),
        None => {
            tracing::warn!("IPC enabled but broker_token not set — skipping broker registration");
            return;
        }
    };
    let gateway_url = match &config.agents_ipc.gateway_url {
        Some(u) => u.clone(),
        None => {
            tracing::warn!("IPC enabled but gateway_url not set — skipping broker registration");
            return;
        }
    };
    let proxy_token = match &config.agents_ipc.proxy_token {
        Some(t) => t.clone(),
        None => {
            tracing::warn!("IPC enabled but proxy_token not set — skipping broker registration");
            return;
        }
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{broker_url}/api/ipc/register-gateway");
    let body = serde_json::json!({
        "gateway_url": gateway_url,
        "proxy_token": proxy_token,
    });

    // Phase A: fast retry with exponential backoff until first success
    let mut delay = Duration::from_secs(1);
    let max_delay = Duration::from_secs(30);
    loop {
        match client
            .post(&url)
            .bearer_auth(&broker_token)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("Registered gateway with broker ({gateway_url})");
                break;
            }
            Ok(resp) => {
                tracing::warn!(
                    "Gateway registration failed (HTTP {}), retrying in {:?}",
                    resp.status(),
                    delay
                );
            }
            Err(e) => {
                tracing::debug!("Gateway registration failed ({e}), retrying in {:?}", delay);
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(max_delay);
    }

    // Phase B: periodic refresh every 5 minutes
    let refresh_interval = Duration::from_secs(300);

    // Shared retry logic for Phase A fallback
    let fast_retry =
        |client: &reqwest::Client, url: &str, broker_token: &str, body: &serde_json::Value| {
            let client = client.clone();
            let url = url.to_string();
            let token = broker_token.to_string();
            let body = body.clone();
            async move {
                let mut retry_delay = Duration::from_secs(1);
                loop {
                    tokio::time::sleep(retry_delay).await;
                    match client
                        .post(&url)
                        .bearer_auth(&token)
                        .json(&body)
                        .send()
                        .await
                    {
                        Ok(r) if r.status().is_success() => {
                            tracing::info!("Gateway re-registered after broker recovery");
                            break;
                        }
                        _ => {
                            retry_delay = (retry_delay * 2).min(max_delay);
                        }
                    }
                }
            }
        };

    loop {
        tokio::time::sleep(refresh_interval).await;
        match client
            .post(&url)
            .bearer_auth(&broker_token)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!("Gateway registration refreshed");
            }
            Ok(resp) => {
                tracing::warn!(
                    "Gateway refresh failed (HTTP {}), switching to fast retry",
                    resp.status()
                );
                fast_retry(&client, &url, &broker_token, &body).await;
            }
            Err(e) => {
                tracing::warn!("Gateway refresh failed ({e}), switching to fast retry");
                fast_retry(&client, &url, &broker_token, &body).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(tmp: &TempDir) -> Config {
        let config = Config {
            workspace_dir: tmp.path().join("workspace"),
            config_path: tmp.path().join("config.toml"),
            ..Config::default()
        };
        std::fs::create_dir_all(&config.workspace_dir).unwrap();
        config
    }

    #[test]
    fn state_file_path_uses_config_directory() {
        let tmp = TempDir::new().unwrap();
        let config = test_config(&tmp);

        let path = state_file_path(&config);
        assert_eq!(path, tmp.path().join("daemon_state.json"));
    }

    #[tokio::test]
    async fn supervisor_marks_error_and_restart_on_failure() {
        let handle = spawn_component_supervisor("daemon-test-fail", 1, 1, || async {
            anyhow::bail!("boom")
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-fail"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(component["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("boom"));
    }

    #[tokio::test]
    async fn supervisor_marks_unexpected_exit_as_error() {
        let handle = spawn_component_supervisor("daemon-test-exit", 1, 1, || async { Ok(()) });

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;

        let snapshot = crate::health::snapshot_json();
        let component = &snapshot["components"]["daemon-test-exit"];
        assert_eq!(component["status"], "error");
        assert!(component["restart_count"].as_u64().unwrap_or(0) >= 1);
        assert!(component["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("component exited unexpectedly"));
    }

    #[test]
    fn detects_no_supervised_channels() {
        let config = Config::default();
        assert!(!has_supervised_channels(&config));
    }

    #[test]
    fn detects_supervised_channels_present() {
        let mut config = Config::default();
        config.channels_config.telegram = Some(synapse_domain::config::schema::TelegramConfig {
            bot_token: "token".into(),
            allowed_users: vec![],
            stream_mode: synapse_domain::config::schema::StreamMode::default(),
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_dingtalk_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.dingtalk = Some(synapse_domain::config::schema::DingTalkConfig {
            client_id: "client_id".into(),
            client_secret: "client_secret".into(),
            allowed_users: vec!["*".into()],
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_mattermost_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.mattermost =
            Some(synapse_domain::config::schema::MattermostConfig {
                url: "https://mattermost.example.com".into(),
                bot_token: "token".into(),
                channel_id: Some("channel-id".into()),
                allowed_users: vec!["*".into()],
                thread_replies: Some(true),
                mention_only: Some(false),
            });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_qq_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.qq = Some(synapse_domain::config::schema::QQConfig {
            app_id: "app-id".into(),
            app_secret: "app-secret".into(),
            allowed_users: vec!["*".into()],
        });
        assert!(has_supervised_channels(&config));
    }

    #[test]
    fn detects_nextcloud_talk_as_supervised_channel() {
        let mut config = Config::default();
        config.channels_config.nextcloud_talk =
            Some(synapse_domain::config::schema::NextcloudTalkConfig {
                base_url: "https://cloud.example.com".into(),
                app_token: "app-token".into(),
                webhook_secret: None,
                allowed_users: vec!["*".into()],
            });
        assert!(has_supervised_channels(&config));
    }

    // Heartbeat delivery target resolution tests are now in
    // synapse_domain::application::services::delivery_service::tests.
    // The old daemon-local functions have been replaced by DeliveryService.

    /// Verify that SIGHUP does not cause shutdown — the daemon should ignore it
    /// and only terminate on SIGINT or SIGTERM.
    #[cfg(unix)]
    #[tokio::test]
    async fn sighup_does_not_shut_down_daemon() {
        use libc;
        use tokio::time::{timeout, Duration};

        let handle = tokio::spawn(wait_for_shutdown_signal());

        // Give the signal handler time to register
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send SIGHUP to ourselves — should be ignored by the handler
        unsafe { libc::raise(libc::SIGHUP) };

        // The future should NOT complete within a short window
        let result = timeout(Duration::from_millis(200), handle).await;
        assert!(
            result.is_err(),
            "wait_for_shutdown_signal should not return after SIGHUP"
        );
    }
}
