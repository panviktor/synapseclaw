use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use synapse_domain::config::schema::{DeepgramFluxConfig, TranscriptionConfig};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const DEEPGRAM_FLUX_PROVIDER: &str = "deepgram_flux";
const FLUX_STREAM_CHUNK_MS: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealtimeTurnEngineStatus {
    pub provider: String,
    pub configured: bool,
    pub ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub language_hints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Default for RealtimeTurnEngineStatus {
    fn default() -> Self {
        Self {
            provider: DEEPGRAM_FLUX_PROVIDER.into(),
            configured: false,
            ready: false,
            model: None,
            language_hints: Vec::new(),
            last_error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeepgramFluxSessionConfig {
    pub api_key: String,
    pub model: String,
    pub language_hints: Vec<String>,
    pub eot_threshold: Option<f32>,
    pub eager_eot_threshold: Option<f32>,
    pub eot_timeout_ms: Option<u32>,
    pub keyterms: Vec<String>,
}

impl DeepgramFluxSessionConfig {
    pub fn from_transcription(transcription: Option<&TranscriptionConfig>) -> Result<Self> {
        let transcription = transcription
            .filter(|config| config.enabled)
            .context("voice transcription is disabled; live calls require [transcription] enabled")?;
        let deepgram = transcription
            .deepgram
            .as_ref()
            .context("live calls require [transcription.deepgram] configuration")?;
        let flux = deepgram
            .flux
            .as_ref()
            .context("live calls require [transcription.deepgram.flux] configuration")?;
        let api_key = deepgram
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .context("live calls require transcription.deepgram.api_key")?;

        let model = flux.model.trim();
        if model.is_empty() {
            bail!("transcription.deepgram.flux.model cannot be empty");
        }

        let language_hints = normalized_language_hints(flux, deepgram.language.as_deref());
        if !language_hints.is_empty() && !model.eq_ignore_ascii_case("flux-general-multi") {
            bail!(
                "language_hints are only supported for Deepgram Flux model `flux-general-multi`"
            );
        }

        if let Some(value) = flux.eot_threshold {
            validate_threshold("eot_threshold", value, 0.5, 0.9)?;
        }
        if let Some(value) = flux.eager_eot_threshold {
            validate_threshold("eager_eot_threshold", value, 0.3, 0.9)?;
        }
        if let (Some(eager), Some(eot)) = (flux.eager_eot_threshold, flux.eot_threshold) {
            if eager > eot {
                bail!("eager_eot_threshold cannot be greater than eot_threshold");
            }
        }
        if let Some(value) = flux.eot_timeout_ms {
            if value == 0 {
                bail!("eot_timeout_ms must be greater than zero");
            }
        }

        Ok(Self {
            api_key,
            model: model.to_string(),
            language_hints,
            eot_threshold: flux.eot_threshold,
            eager_eot_threshold: flux.eager_eot_threshold,
            eot_timeout_ms: flux.eot_timeout_ms,
            keyterms: flux
                .keyterms
                .iter()
                .map(|term| term.trim())
                .filter(|term| !term.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        })
    }

    pub fn status_from_transcription(
        transcription: Option<&TranscriptionConfig>,
    ) -> RealtimeTurnEngineStatus {
        match Self::from_transcription(transcription) {
            Ok(config) => RealtimeTurnEngineStatus {
                provider: DEEPGRAM_FLUX_PROVIDER.into(),
                configured: true,
                ready: true,
                model: Some(config.model),
                language_hints: config.language_hints,
                last_error: None,
            },
            Err(error) => RealtimeTurnEngineStatus {
                provider: DEEPGRAM_FLUX_PROVIDER.into(),
                configured: transcription.is_some(),
                ready: false,
                model: transcription
                    .and_then(|config| config.deepgram.as_ref())
                    .and_then(|config| config.flux.as_ref())
                    .map(|config| config.model.clone()),
                language_hints: transcription
                    .and_then(|config| config.deepgram.as_ref())
                    .and_then(|config| config.flux.as_ref())
                    .map(|config| {
                        config
                            .language_hints
                            .iter()
                            .map(|value| value.trim())
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                last_error: Some(error.to_string()),
            },
        }
    }
}

fn validate_threshold(name: &str, value: f32, min: f32, max: f32) -> Result<()> {
    if !(min..=max).contains(&value) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(())
}

fn normalized_language_hints(
    flux: &DeepgramFluxConfig,
    fallback_language: Option<&str>,
) -> Vec<String> {
    let mut values = flux
        .language_hints
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if values.is_empty() {
        if let Some(language) = fallback_language
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("multi"))
        {
            values.push(language.to_ascii_lowercase());
        }
    }

    values.sort();
    values.dedup();
    values
}

#[async_trait::async_trait]
pub trait RealtimeTurnEngine: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn connect(
        &self,
        sample_rate: u32,
        channels: u32,
    ) -> Result<RealtimeTurnEngineSession>;
}

#[derive(Debug)]
pub struct RealtimeTurnEngineSession {
    control: RealtimeTurnEngineControl,
    event_rx: mpsc::Receiver<RealtimeTurnEvent>,
}

impl RealtimeTurnEngineSession {
    pub fn split(self) -> (RealtimeTurnEngineControl, mpsc::Receiver<RealtimeTurnEvent>) {
        (self.control, self.event_rx)
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeTurnEngineControl {
    command_tx: mpsc::Sender<RealtimeTurnEngineCommand>,
}

impl RealtimeTurnEngineControl {
    pub async fn send_audio(&self, pcm16_samples: Vec<i16>) -> Result<()> {
        self.command_tx
            .send(RealtimeTurnEngineCommand::Audio(pcm16_samples_to_bytes(
                &pcm16_samples,
            )))
            .await
            .map_err(|_| anyhow::anyhow!("realtime turn engine writer is closed"))
    }

    pub async fn configure_language_hints(&self, language_hints: Vec<String>) -> Result<()> {
        self.command_tx
            .send(RealtimeTurnEngineCommand::ConfigureLanguageHints(
                language_hints,
            ))
            .await
            .map_err(|_| anyhow::anyhow!("realtime turn engine writer is closed"))
    }

    pub async fn close(&self) -> Result<()> {
        self.command_tx
            .send(RealtimeTurnEngineCommand::Close)
            .await
            .map_err(|_| anyhow::anyhow!("realtime turn engine writer is closed"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeTurnEvent {
    Update {
        turn_index: u64,
        transcript: Option<String>,
        languages: Vec<String>,
        end_of_turn_confidence: Option<f32>,
    },
    StartOfTurn {
        turn_index: u64,
        transcript: Option<String>,
    },
    EagerEndOfTurn {
        turn_index: u64,
        transcript: String,
        languages: Vec<String>,
    },
    EndOfTurn {
        turn_index: u64,
        transcript: String,
        languages: Vec<String>,
    },
    TurnResumed {
        turn_index: u64,
        transcript: Option<String>,
    },
    Error {
        code: Option<String>,
        description: String,
    },
    Closed,
}

enum RealtimeTurnEngineCommand {
    Audio(Vec<u8>),
    ConfigureLanguageHints(Vec<String>),
    Close,
}

#[derive(Debug, Clone)]
pub struct RealtimePcm16StreamBatcher {
    channels: usize,
    chunk_samples: usize,
    buffer: Vec<i16>,
}

impl RealtimePcm16StreamBatcher {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        let safe_channels = channels.max(1) as usize;
        let samples_per_channel = ((sample_rate.max(1) as usize) * FLUX_STREAM_CHUNK_MS) / 1000;
        Self {
            channels: safe_channels,
            chunk_samples: samples_per_channel.max(1) * safe_channels,
            buffer: Vec::new(),
        }
    }

    pub fn push_frame(&mut self, frame: &[i16]) -> Vec<Vec<i16>> {
        if frame.is_empty() {
            return Vec::new();
        }
        self.buffer.extend_from_slice(frame);
        let mut chunks = Vec::new();
        while self.buffer.len() >= self.chunk_samples {
            let remainder = self.buffer.split_off(self.chunk_samples);
            chunks.push(std::mem::take(&mut self.buffer));
            self.buffer = remainder;
        }
        chunks
    }

    pub fn finish(&mut self) -> Option<Vec<i16>> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn buffered_samples(&self) -> usize {
        self.buffer.len()
    }
}

#[derive(Debug, Clone)]
pub struct DeepgramFluxTurnEngine {
    config: DeepgramFluxSessionConfig,
}

impl DeepgramFluxTurnEngine {
    pub fn new(config: DeepgramFluxSessionConfig) -> Self {
        Self { config }
    }

    fn websocket_url(&self, sample_rate: u32, channels: u32) -> String {
        let mut url =
            reqwest::Url::parse("wss://api.deepgram.com/v2/listen").expect("valid Deepgram URL");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("model", &self.config.model);
            query.append_pair("encoding", "linear16");
            query.append_pair("sample_rate", &sample_rate.to_string());
            if channels > 1 {
                query.append_pair("channels", &channels.to_string());
            }
            if let Some(value) = self.config.eot_threshold {
                query.append_pair("eot_threshold", &value.to_string());
            }
            if let Some(value) = self.config.eager_eot_threshold {
                query.append_pair("eager_eot_threshold", &value.to_string());
            }
            if let Some(value) = self.config.eot_timeout_ms {
                query.append_pair("eot_timeout_ms", &value.to_string());
            }
            for hint in &self.config.language_hints {
                query.append_pair("language_hint", hint);
            }
            for keyterm in &self.config.keyterms {
                query.append_pair("keyterm", keyterm);
            }
        }
        url.into()
    }

    fn websocket_request(
        &self,
        sample_rate: u32,
        channels: u32,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>> {
        let endpoint = self.websocket_url(sample_rate, channels);
        let uri = tokio_tungstenite::tungstenite::http::Uri::try_from(endpoint.as_str())
            .map_err(|error| anyhow::anyhow!("invalid Deepgram Flux websocket URL: {error}"))?;
        let host = uri
            .host()
            .ok_or_else(|| anyhow::anyhow!("Deepgram Flux websocket URL is missing host"))?;
        tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(endpoint)
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .header(
                "Authorization",
                format!("Token {}", self.config.api_key.trim()),
            )
            .header("User-Agent", "synapseclaw-deepgram-flux/1")
            .body(())
            .map_err(|error| anyhow::anyhow!("invalid Deepgram Flux websocket request: {error}"))
    }
}

#[async_trait::async_trait]
impl RealtimeTurnEngine for DeepgramFluxTurnEngine {
    fn provider_name(&self) -> &'static str {
        DEEPGRAM_FLUX_PROVIDER
    }

    async fn connect(
        &self,
        sample_rate: u32,
        channels: u32,
    ) -> Result<RealtimeTurnEngineSession> {
        let request = self.websocket_request(sample_rate, channels)?;
        let (mut socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("failed to connect to Deepgram Flux websocket")?;
        let (command_tx, mut command_rx) = mpsc::channel::<RealtimeTurnEngineCommand>(32);
        let (event_tx, event_rx) = mpsc::channel::<RealtimeTurnEvent>(32);

        tokio::spawn(async move {
            let mut server_ready = false;
            let mut pending_audio = VecDeque::<Vec<u8>>::new();
            loop {
                tokio::select! {
                    maybe_command = command_rx.recv() => {
                        match maybe_command {
                            Some(RealtimeTurnEngineCommand::Audio(bytes)) => {
                                if !server_ready {
                                    if pending_audio.len() >= 64 {
                                        pending_audio.pop_front();
                                    }
                                    pending_audio.push_back(bytes);
                                    continue;
                                }
                                if let Err(error) = socket.send(WsMessage::Binary(bytes.into())).await {
                                    let _ = event_tx.send(RealtimeTurnEvent::Error {
                                        code: None,
                                        description: format!("failed to send audio to Deepgram Flux: {error}"),
                                    }).await;
                                    break;
                                }
                            }
                            Some(RealtimeTurnEngineCommand::ConfigureLanguageHints(language_hints)) => {
                                let payload = serde_json::to_string(&DeepgramFluxConfigureMessage {
                                    message_type: "Configure",
                                    language_hints: Some(language_hints),
                                }).unwrap_or_else(|_| "{\"type\":\"Configure\"}".to_string());
                                if let Err(error) = socket.send(WsMessage::Text(payload.into())).await {
                                    let _ = event_tx.send(RealtimeTurnEvent::Error {
                                        code: None,
                                        description: format!("failed to send Deepgram Flux configure message: {error}"),
                                    }).await;
                                    break;
                                }
                            }
                            Some(RealtimeTurnEngineCommand::Close) | None => {
                                let _ = socket.send(WsMessage::Text("{\"type\":\"CloseStream\"}".into())).await;
                                let _ = event_tx.send(RealtimeTurnEvent::Closed).await;
                                break;
                            }
                        }
                    }
                    maybe_message = socket.next() => {
                        match maybe_message {
                            Some(Ok(WsMessage::Text(text))) => {
                                tracing::info!(
                                    summary = %summarize_flux_server_message(&text),
                                    "Deepgram Flux server message"
                                );
                                if !server_ready && flux_server_message_marks_ready(&text) {
                                    server_ready = true;
                                    while let Some(audio) = pending_audio.pop_front() {
                                        if let Err(error) = socket.send(WsMessage::Binary(audio.into())).await {
                                            let _ = event_tx.send(RealtimeTurnEvent::Error {
                                                code: None,
                                                description: format!("failed to flush buffered audio to Deepgram Flux: {error}"),
                                            }).await;
                                            break;
                                        }
                                    }
                                }
                                if let Some(event) = parse_flux_server_message(&text) {
                                    let terminal = matches!(event, RealtimeTurnEvent::Closed);
                                    if event_tx.send(event).await.is_err() {
                                        break;
                                    }
                                    if terminal {
                                        break;
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Ping(payload))) => {
                                if let Err(error) = socket.send(WsMessage::Pong(payload)).await {
                                    let _ = event_tx.send(RealtimeTurnEvent::Error {
                                        code: None,
                                        description: format!("failed to answer Deepgram Flux ping: {error}"),
                                    }).await;
                                    break;
                                }
                            }
                            Some(Ok(WsMessage::Close(_))) | None => {
                                let _ = event_tx.send(RealtimeTurnEvent::Closed).await;
                                break;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(error)) => {
                                let _ = event_tx.send(RealtimeTurnEvent::Error {
                                    code: None,
                                    description: format!("Deepgram Flux websocket error: {error}"),
                                }).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(RealtimeTurnEngineSession {
            control: RealtimeTurnEngineControl { command_tx },
            event_rx,
        })
    }
}

#[derive(Debug, Serialize)]
struct DeepgramFluxConfigureMessage {
    #[serde(rename = "type")]
    message_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    language_hints: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum DeepgramFluxServerMessage {
    Welcome {
        #[serde(default)]
        request_id: Option<String>,
    },
    Connected {},
    TurnInfo {
        event: String,
        #[serde(default)]
        turn_index: u64,
        #[serde(default)]
        transcript: Option<String>,
        #[serde(default)]
        languages: Vec<String>,
        #[serde(default)]
        end_of_turn_confidence: Option<f32>,
    },
    ConfigureSuccess {},
    ConfigureFailure {
        #[serde(default)]
        description: Option<String>,
    },
    Error {
        #[serde(default)]
        code: Option<String>,
        description: String,
    },
}

fn parse_flux_server_message(text: &str) -> Option<RealtimeTurnEvent> {
    let message = match serde_json::from_str::<DeepgramFluxServerMessage>(text) {
        Ok(message) => message,
        Err(error) => {
            return Some(RealtimeTurnEvent::Error {
                code: None,
                description: format!("invalid Deepgram Flux message: {error}"),
            });
        }
    };

    match message {
        DeepgramFluxServerMessage::Welcome { .. } => None,
        DeepgramFluxServerMessage::Connected { .. } => None,
        DeepgramFluxServerMessage::ConfigureSuccess { .. } => None,
        DeepgramFluxServerMessage::ConfigureFailure { description } => {
            Some(RealtimeTurnEvent::Error {
                code: Some("configure_failure".into()),
                description: description.unwrap_or_else(|| {
                    "Deepgram Flux rejected a configure message".into()
                }),
            })
        }
        DeepgramFluxServerMessage::Error { code, description } => {
            Some(RealtimeTurnEvent::Error { code, description })
        }
        DeepgramFluxServerMessage::TurnInfo {
            event,
            turn_index,
            transcript,
            languages,
            end_of_turn_confidence,
        } => match event.as_str() {
            "Update" => Some(RealtimeTurnEvent::Update {
                turn_index,
                transcript: normalized_transcript(transcript),
                languages,
                end_of_turn_confidence,
            }),
            "StartOfTurn" => Some(RealtimeTurnEvent::StartOfTurn {
                turn_index,
                transcript: normalized_transcript(transcript),
            }),
            "EagerEndOfTurn" => normalized_transcript(transcript).map(|transcript| {
                RealtimeTurnEvent::EagerEndOfTurn {
                    turn_index,
                    transcript,
                    languages,
                }
            }),
            "TurnResumed" => Some(RealtimeTurnEvent::TurnResumed {
                turn_index,
                transcript: normalized_transcript(transcript),
            }),
            "EndOfTurn" => normalized_transcript(transcript).map(|transcript| {
                RealtimeTurnEvent::EndOfTurn {
                    turn_index,
                    transcript,
                    languages,
                }
            }),
            _ => None,
        },
    }
}

fn flux_server_message_marks_ready(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("type").and_then(|value| value.as_str()).map(str::to_owned))
        .map(|kind| matches!(kind.as_str(), "Welcome" | "Connected" | "ConfigureSuccess" | "TurnInfo"))
        .unwrap_or(false)
}

fn summarize_flux_server_message(text: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return bounded_flux_log(text);
    };
    let message_type = value
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let event = value
        .get("event")
        .and_then(|value| value.as_str())
        .unwrap_or("-");
    let transcript = value
        .get("transcript")
        .and_then(|value| value.as_str())
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    if transcript.is_empty() {
        format!("type={message_type} event={event}")
    } else {
        format!(
            "type={message_type} event={event} transcript={}",
            bounded_flux_log(&transcript)
        )
    }
}

fn bounded_flux_log(text: &str) -> String {
    const MAX_LOG_CHARS: usize = 200;
    let mut bounded = text.chars().take(MAX_LOG_CHARS).collect::<String>();
    if text.chars().count() > MAX_LOG_CHARS {
        bounded.push_str("...");
    }
    bounded
}

fn normalized_transcript(transcript: Option<String>) -> Option<String> {
    transcript
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn pcm16_samples_to_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len().saturating_mul(2));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::config::schema::{DeepgramSttConfig, TranscriptionConfig};

    fn transcription_with_flux() -> TranscriptionConfig {
        let mut config = TranscriptionConfig {
            enabled: true,
            ..Default::default()
        };
        config.deepgram = Some(DeepgramSttConfig {
            api_key: Some("dg-key".into()),
            model: "nova-3".into(),
            language: Some("multi".into()),
            flux: Some(DeepgramFluxConfig {
                model: "flux-general-multi".into(),
                language_hints: vec!["ru".into(), "en".into()],
                eot_threshold: Some(0.7),
                eager_eot_threshold: None,
                eot_timeout_ms: Some(1200),
                keyterms: vec!["synapseclaw".into()],
            }),
        });
        config
    }

    #[test]
    fn flux_config_extracts_from_transcription() {
        let config =
            DeepgramFluxSessionConfig::from_transcription(Some(&transcription_with_flux())).unwrap();
        assert_eq!(config.model, "flux-general-multi");
        assert_eq!(config.language_hints, vec!["en".to_string(), "ru".to_string()]);
        assert_eq!(config.eot_timeout_ms, Some(1200));
        assert_eq!(config.keyterms, vec!["synapseclaw".to_string()]);
    }

    #[test]
    fn flux_status_reports_missing_config() {
        let status = DeepgramFluxSessionConfig::status_from_transcription(None);
        assert!(!status.ready);
        assert!(status.last_error.is_some());
    }

    #[test]
    fn stream_batcher_groups_into_transport_chunks() {
        let mut batcher = RealtimePcm16StreamBatcher::new(16_000, 1);
        let frame = vec![1i16; 320];
        let mut chunks = Vec::new();
        for _ in 0..4 {
            chunks.extend(batcher.push_frame(&frame));
        }
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1280);
        assert_eq!(batcher.channels(), 1);
    }

    #[test]
    fn parser_maps_end_of_turn_messages() {
        let event = parse_flux_server_message(
            r#"{"type":"TurnInfo","event":"EndOfTurn","turn_index":2,"transcript":" hello world ","languages":["en"]}"#,
        )
        .expect("event");
        assert_eq!(
            event,
            RealtimeTurnEvent::EndOfTurn {
                turn_index: 2,
                transcript: "hello world".into(),
                languages: vec!["en".into()],
            }
        );
    }
}
