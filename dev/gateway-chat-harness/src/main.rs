use anyhow::{bail, Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use reqwest::{header::HeaderMap, Client, Url};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use synapse_domain::config::schema::Config;
use synapse_infra::config_io::ConfigIO;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message, MaybeTlsStream};
use uuid::Uuid;

type GatewaySocket = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Parser)]
#[command(
    name = "gateway-chat-harness",
    about = "Drive live SynapseClaw chat sessions over the gateway WebSocket"
)]
struct Args {
    /// Override config dir before loading config.toml.
    #[arg(long)]
    config_dir: Option<PathBuf>,

    /// Gateway base URL (defaults to config gateway host/port).
    #[arg(long)]
    gateway_url: Option<String>,

    /// Route through broker proxy to a specific helper agent.
    #[arg(long)]
    agent: Option<String>,

    /// Stable logical session id. Reuse across invocations for multi-turn tests.
    #[arg(long)]
    session: Option<String>,

    /// Prime the live session onto a runtime route before sending test messages.
    /// Use `gpt-5.4` for OpenAI-specific continuation/history experiments.
    #[arg(long, default_value = "cheap")]
    route: String,

    /// User message to send. Repeat for multi-turn scenarios in the same session.
    #[arg(long = "message", short = 'm', required = true)]
    messages: Vec<String>,

    /// Per-turn timeout in seconds.
    #[arg(long, default_value_t = 180)]
    timeout_secs: u64,

    /// Fetch chat.history after the last turn.
    #[arg(long)]
    history: bool,

    /// Limit for chat.history when --history is used.
    #[arg(long, default_value_t = 50)]
    history_limit: usize,

    /// Emit a single JSON report instead of human-readable trace lines.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct HarnessReport {
    gateway_url: String,
    agent: Option<String>,
    session: String,
    primed_route: Option<String>,
    history: Option<Value>,
    turns: Vec<TurnReport>,
}

#[derive(Debug, Serialize)]
struct TurnReport {
    index: usize,
    user_message: String,
    run_id: Option<String>,
    rpc_result: Option<Value>,
    rpc_error: Option<String>,
    events: Vec<Value>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(config_dir) = args.config_dir.as_ref() {
        std::env::set_var("SYNAPSECLAW_CONFIG_DIR", config_dir);
    }

    let config = Config::load_or_init().await?;
    let gateway_url = args
        .gateway_url
        .clone()
        .unwrap_or_else(|| default_gateway_url(&config));
    let token = resolve_gateway_token(&config, &gateway_url).await?;
    let session = args
        .session
        .clone()
        .unwrap_or_else(|| format!("harness-{}", Uuid::new_v4()));
    let ws_url = build_ws_url(&gateway_url, &token, &session, args.agent.as_deref())?;
    let (mut socket, _) = connect_async(ws_url.as_str())
        .await
        .with_context(|| format!("Failed to connect to gateway WebSocket at {gateway_url}"))?;

    let mut report = HarnessReport {
        gateway_url,
        agent: args.agent.clone(),
        session: session.clone(),
        primed_route: None,
        history: None,
        turns: Vec::new(),
    };

    let route = args.route.trim();
    if !route.is_empty() {
        if !args.json {
            println!("Priming route> {route}");
        }
        let route_turn = run_turn(
            &mut socket,
            &session,
            0,
            &format!("/model {route}"),
            Duration::from_secs(args.timeout_secs),
            args.json,
        )
        .await?;
        report.primed_route = Some(route.to_string());
        report.turns.push(route_turn);
    }

    for (index, user_message) in args.messages.iter().enumerate() {
        if !args.json {
            println!("Turn {} user> {}", index + 1, user_message);
        }
        let turn = run_turn(
            &mut socket,
            &session,
            index + 1,
            user_message,
            Duration::from_secs(args.timeout_secs),
            args.json,
        )
        .await?;
        report.turns.push(turn);
    }

    if args.history {
        report.history = Some(
            fetch_history(
                &mut socket,
                &session,
                args.history_limit,
                Duration::from_secs(args.timeout_secs),
            )
            .await?,
        );
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if let Some(history) = report.history.as_ref() {
        print_history_summary(history);
    }

    let _ = socket.close(None).await;
    Ok(())
}

async fn run_turn(
    socket: &mut GatewaySocket,
    _session: &str,
    index: usize,
    user_message: &str,
    timeout_duration: Duration,
    json_mode: bool,
) -> Result<TurnReport> {
    let rpc_id = format!("turn-{index}-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "type": "rpc",
        "id": rpc_id,
        "method": "chat.send",
        "params": {
            "message": user_message,
        }
    });
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .context("Failed to send chat.send RPC")?;

    let deadline = Instant::now() + timeout_duration;
    let mut events = Vec::new();
    let (rpc_result, rpc_error, run_id) = loop {
        let msg = recv_json(socket, deadline).await?;
        if msg.get("type").and_then(Value::as_str) == Some("rpc_response")
            && msg.get("id").and_then(Value::as_str) == Some(rpc_id.as_str())
        {
            let rpc_result = msg.get("result").cloned();
            let rpc_error = msg
                .get("error")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let run_id = rpc_result
                .as_ref()
                .and_then(|value| value.get("run_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if !json_mode {
                match rpc_error.as_deref() {
                    Some(error) => println!("Turn {index} rpc_error> {error}"),
                    None => println!(
                        "Turn {index} done> run_id={}",
                        run_id.as_deref().unwrap_or("unknown")
                    ),
                }
            }
            break (rpc_result, rpc_error, run_id);
        }

        if !json_mode {
            print_event(index, &msg);
        }
        events.push(msg);
    };

    while let Some(msg) = recv_json_quiet(socket, Duration::from_millis(250)).await? {
        if !json_mode {
            print_event(index, &msg);
        }
        events.push(msg);
    }

    Ok(TurnReport {
        index,
        user_message: user_message.to_string(),
        run_id,
        rpc_result,
        rpc_error,
        events,
    })
}

async fn fetch_history(
    socket: &mut GatewaySocket,
    _session: &str,
    limit: usize,
    timeout_duration: Duration,
) -> Result<Value> {
    let rpc_id = format!("history-{}", Uuid::new_v4());
    let payload = serde_json::json!({
        "type": "rpc",
        "id": rpc_id,
        "method": "chat.history",
        "params": {
            "limit": limit,
        }
    });
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .context("Failed to send chat.history RPC")?;

    let deadline = Instant::now() + timeout_duration;
    loop {
        let msg = recv_json(socket, deadline).await?;
        if msg.get("type").and_then(Value::as_str) != Some("rpc_response") {
            continue;
        }
        if msg.get("id").and_then(Value::as_str) != Some(rpc_id.as_str()) {
            continue;
        }
        if let Some(error) = msg.get("error").and_then(Value::as_str) {
            bail!("chat.history failed: {error}");
        }
        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
    }
}

async fn recv_json(socket: &mut GatewaySocket, deadline: Instant) -> Result<Value> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            bail!("Timed out waiting for gateway WebSocket message");
        }
        let next = timeout(deadline.saturating_duration_since(now), socket.next())
            .await
            .context("Timed out waiting for gateway WebSocket message")?;
        let Some(frame) = next else {
            bail!("Gateway WebSocket closed");
        };
        let frame = frame.context("Gateway WebSocket read failed")?;
        match frame {
            Message::Text(text) => {
                return serde_json::from_str(&text).context("Failed to parse gateway JSON frame");
            }
            Message::Ping(payload) => {
                socket
                    .send(Message::Pong(payload))
                    .await
                    .context("Failed to reply to gateway ping")?;
            }
            Message::Pong(_) | Message::Binary(_) => {}
            Message::Close(_) => bail!("Gateway WebSocket closed"),
            Message::Frame(_) => {}
        }
    }
}

async fn recv_json_quiet(
    socket: &mut GatewaySocket,
    quiet_window: Duration,
) -> Result<Option<Value>> {
    let next = timeout(quiet_window, socket.next()).await;
    let Ok(next) = next else {
        return Ok(None);
    };
    let Some(frame) = next else {
        return Ok(None);
    };
    let frame = frame.context("Gateway WebSocket read failed")?;
    match frame {
        Message::Text(text) => Ok(Some(
            serde_json::from_str(&text).context("Failed to parse gateway JSON frame")?,
        )),
        Message::Ping(payload) => {
            socket
                .send(Message::Pong(payload))
                .await
                .context("Failed to reply to gateway ping")?;
            Ok(None)
        }
        Message::Pong(_) | Message::Binary(_) | Message::Frame(_) | Message::Close(_) => Ok(None),
    }
}

fn default_gateway_url(config: &Config) -> String {
    let host = match config.gateway.host.as_str() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        other => other,
    };
    format!("http://{host}:{}", config.gateway.port)
}

async fn resolve_gateway_token(config: &Config, gateway_url: &str) -> Result<String> {
    if !config.gateway.require_pairing {
        return Ok(String::new());
    }

    if let Some(raw_token) = config
        .gateway
        .paired_tokens
        .iter()
        .find(|token| !is_token_hash(token))
        .cloned()
    {
        return Ok(raw_token);
    }

    pair_local_token(gateway_url).await
}

async fn pair_local_token(gateway_url: &str) -> Result<String> {
    let client = Client::builder()
        .build()
        .context("Failed to build HTTP client for local gateway pairing")?;

    let paircode_url = format!("{}/admin/paircode/new", gateway_url.trim_end_matches('/'));
    let paircode_resp = client
        .post(&paircode_url)
        .send()
        .await
        .with_context(|| format!("Failed to request local pairing code from {paircode_url}"))?;
    let paircode_status = paircode_resp.status();
    let paircode_body: Value = paircode_resp
        .json()
        .await
        .context("Failed to parse /admin/paircode/new response")?;
    if !paircode_status.is_success() {
        bail!(
            "Local pairing code request failed ({}): {}",
            paircode_status,
            paircode_body
        );
    }
    let pairing_code = paircode_body
        .get("pairing_code")
        .and_then(Value::as_str)
        .filter(|code| !code.is_empty())
        .context("Gateway returned no pairing_code from /admin/paircode/new")?;

    let pair_url = format!("{}/pair", gateway_url.trim_end_matches('/'));
    let mut headers = HeaderMap::new();
    headers.insert("X-Pairing-Code", pairing_code.parse()?);
    let pair_resp = client
        .post(&pair_url)
        .headers(headers)
        .send()
        .await
        .with_context(|| format!("Failed to exchange pairing code at {pair_url}"))?;
    let pair_status = pair_resp.status();
    let pair_body: Value = pair_resp
        .json()
        .await
        .context("Failed to parse /pair response")?;
    if !pair_status.is_success() {
        bail!("Gateway pairing failed ({}): {}", pair_status, pair_body);
    }

    pair_body
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .context("Gateway /pair response did not include a bearer token")
}

fn build_ws_url(base_url: &str, token: &str, session: &str, agent: Option<&str>) -> Result<Url> {
    let normalized = if base_url.starts_with("ws://") || base_url.starts_with("wss://") {
        base_url.to_string()
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = base_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        format!("ws://{base_url}")
    };

    let mut url =
        Url::parse(&normalized).with_context(|| format!("Invalid gateway URL: {base_url}"))?;
    url.set_path(if agent.is_some() {
        "/ws/chat/proxy"
    } else {
        "/ws/chat"
    });
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("token", token);
        pairs.append_pair("session_id", session);
        if let Some(agent) = agent {
            pairs.append_pair("agent", agent);
        }
    }
    Ok(url)
}

fn is_token_hash(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn print_event(index: usize, event: &Value) {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match event_type {
        "tool_call" => println!(
            "Turn {index} tool_call> {}",
            truncate(
                event.get("content").and_then(Value::as_str).unwrap_or(""),
                300
            )
        ),
        "tool_result" => println!(
            "Turn {index} tool_result> {}",
            truncate(
                event.get("content").and_then(Value::as_str).unwrap_or(""),
                300
            )
        ),
        "assistant" => println!(
            "Turn {index} assistant> {}",
            truncate(
                event.get("content").and_then(Value::as_str).unwrap_or(""),
                600
            )
        ),
        "error" => println!(
            "Turn {index} error> {}",
            event.get("message").and_then(Value::as_str).unwrap_or("")
        ),
        "session.run_started" | "session.run_finished" | "session.run_interrupted" => {
            println!(
                "Turn {index} {event_type}> {}",
                event.get("run_id").and_then(Value::as_str).unwrap_or("")
            )
        }
        _ => println!(
            "Turn {index} {event_type}> {}",
            truncate(&event.to_string(), 300)
        ),
    }
}

fn print_history_summary(history: &Value) {
    let count = history
        .get("messages")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let label = history
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("unknown session");
    println!("History> {count} messages ({label})");
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let shortened: String = trimmed.chars().take(limit).collect();
    format!("{shortened}...")
}

#[cfg(test)]
mod tests {
    use super::{build_ws_url, default_gateway_url, is_token_hash};
    use synapse_domain::config::schema::Config;

    #[test]
    fn default_gateway_url_uses_loopback_for_public_bind() {
        let mut config = Config::default();
        config.gateway.host = "0.0.0.0".into();
        config.gateway.port = 42617;
        assert_eq!(default_gateway_url(&config), "http://127.0.0.1:42617");
    }

    #[test]
    fn ws_url_builder_targets_proxy_when_agent_set() {
        let url = build_ws_url(
            "http://127.0.0.1:42617",
            "tok",
            "sess-1",
            Some("copywriter"),
        )
        .unwrap();
        assert_eq!(url.path(), "/ws/chat/proxy");
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "agent").unwrap().1,
            "copywriter"
        );
    }

    #[test]
    fn token_hash_detector_matches_persisted_pairing_hashes() {
        assert!(is_token_hash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_token_hash("zc_plaintext_token"));
    }
}
