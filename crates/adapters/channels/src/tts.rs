//! Multi-provider Text-to-Speech (TTS) subsystem.
//!
//! Supports OpenAI, ElevenLabs, Google Cloud, Edge, MiniMax, Mistral Voxtral, and xAI.
//! Provider selection is driven by [`TtsConfig`] in `config.toml`.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};

use synapse_domain::config::schema::{MiniMaxTtsConfig, MistralTtsConfig, TtsConfig, XaiTtsConfig};

/// Maximum text length before synthesis is rejected (default: 4096 chars).
const DEFAULT_MAX_TEXT_LENGTH: usize = 4096;

/// Default HTTP request timeout for TTS API calls.
const TTS_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// ── TtsProvider trait ────────────────────────────────────────────

/// Trait for pluggable TTS backends.
#[async_trait::async_trait]
pub trait TtsProvider: Send + Sync {
    /// Provider identifier (e.g. `"openai"`, `"elevenlabs"`).
    fn name(&self) -> &str;

    /// Synthesize `text` using the given `voice`, returning raw audio bytes.
    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>>;

    /// Voices supported by this provider.
    fn supported_voices(&self) -> Vec<String>;

    /// Audio output formats supported by this provider.
    fn supported_formats(&self) -> Vec<String>;
}

// ── OpenAI TTS ───────────────────────────────────────────────────

/// OpenAI TTS provider (`POST /v1/audio/speech`).
pub struct OpenAiTtsProvider {
    api_key: String,
    model: String,
    speed: f64,
    client: reqwest::Client,
}

impl OpenAiTtsProvider {
    /// Create a new OpenAI TTS provider from a lane-resolved config.
    pub fn new(config: &synapse_domain::config::schema::OpenAiTtsConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToOwned::to_owned)
            .context("Missing OpenAI TTS API key from resolved speech_synthesis lane")?;

        Ok(Self {
            api_key,
            model: config.model.clone(),
            speed: config.speed,
            client: reqwest::Client::builder()
                .timeout(TTS_HTTP_TIMEOUT)
                .build()
                .context("Failed to build HTTP client for OpenAI TTS")?,
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for OpenAiTtsProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": voice,
            "speed": self.speed,
            "response_format": "opus",
        });

        let resp = self
            .client
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to send OpenAI TTS request")?;

        let status = resp.status();
        if !status.is_success() {
            let error_body: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": "unknown"}));
            let msg = error_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            bail!("OpenAI TTS API error ({}): {}", status, msg);
        }

        let bytes = resp
            .bytes()
            .await
            .context("Failed to read OpenAI TTS response body")?;
        Ok(bytes.to_vec())
    }

    fn supported_voices(&self) -> Vec<String> {
        ["alloy", "echo", "fable", "onyx", "nova", "shimmer"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    fn supported_formats(&self) -> Vec<String> {
        ["mp3", "opus", "aac", "flac", "wav", "pcm"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

// ── ElevenLabs TTS ───────────────────────────────────────────────

/// ElevenLabs TTS provider (`POST /v1/text-to-speech/{voice_id}`).
pub struct ElevenLabsTtsProvider {
    api_key: String,
    model_id: String,
    stability: f64,
    similarity_boost: f64,
    client: reqwest::Client,
}

impl ElevenLabsTtsProvider {
    /// Create a new ElevenLabs TTS provider from a lane-resolved config.
    pub fn new(config: &synapse_domain::config::schema::ElevenLabsTtsConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToOwned::to_owned)
            .context("Missing ElevenLabs API key from resolved speech_synthesis lane")?;

        Ok(Self {
            api_key,
            model_id: config.model_id.clone(),
            stability: config.stability,
            similarity_boost: config.similarity_boost,
            client: reqwest::Client::builder()
                .timeout(TTS_HTTP_TIMEOUT)
                .build()
                .context("Failed to build HTTP client for ElevenLabs TTS")?,
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for ElevenLabsTtsProvider {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        if !voice
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("ElevenLabs voice ID contains invalid characters: {voice}");
        }
        let url = format!("https://api.elevenlabs.io/v1/text-to-speech/{voice}");
        let body = serde_json::json!({
            "text": text,
            "model_id": self.model_id,
            "voice_settings": {
                "stability": self.stability,
                "similarity_boost": self.similarity_boost,
            },
        });

        let resp = self
            .client
            .post(&url)
            .header("xi-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to send ElevenLabs TTS request")?;

        let status = resp.status();
        if !status.is_success() {
            let error_body: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": "unknown"}));
            let msg = error_body["detail"]["message"]
                .as_str()
                .or_else(|| error_body["detail"].as_str())
                .unwrap_or("unknown error");
            bail!("ElevenLabs TTS API error ({}): {}", status, msg);
        }

        let bytes = resp
            .bytes()
            .await
            .context("Failed to read ElevenLabs TTS response body")?;
        Ok(bytes.to_vec())
    }

    fn supported_voices(&self) -> Vec<String> {
        // ElevenLabs voices are user-specific; return empty (dynamic lookup).
        Vec::new()
    }

    fn supported_formats(&self) -> Vec<String> {
        ["mp3", "pcm", "ulaw"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

// ── Google Cloud TTS ─────────────────────────────────────────────

/// Google Cloud TTS provider (`POST /v1/text:synthesize`).
pub struct GoogleTtsProvider {
    api_key: String,
    language_code: String,
    client: reqwest::Client,
}

impl GoogleTtsProvider {
    /// Create a new Google Cloud TTS provider from a lane-resolved config.
    pub fn new(config: &synapse_domain::config::schema::GoogleTtsConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToOwned::to_owned)
            .context("Missing Google TTS API key from resolved speech_synthesis lane")?;

        Ok(Self {
            api_key,
            language_code: config.language_code.clone(),
            client: reqwest::Client::builder()
                .timeout(TTS_HTTP_TIMEOUT)
                .build()
                .context("Failed to build HTTP client for Google TTS")?,
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for GoogleTtsProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let url = "https://texttospeech.googleapis.com/v1/text:synthesize";
        let body = serde_json::json!({
            "input": { "text": text },
            "voice": {
                "languageCode": self.language_code,
                "name": voice,
            },
            "audioConfig": {
                "audioEncoding": "MP3",
            },
        });

        let resp = self
            .client
            .post(url)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to send Google TTS request")?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse Google TTS response")?;

        if !status.is_success() {
            let msg = resp_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            bail!("Google TTS API error ({}): {}", status, msg);
        }

        let audio_b64 = resp_body["audioContent"]
            .as_str()
            .context("Google TTS response missing 'audioContent' field")?;

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(audio_b64)
            .context("Failed to decode Google TTS base64 audio")?;
        Ok(bytes)
    }

    fn supported_voices(&self) -> Vec<String> {
        // Google voices vary by language; return common English defaults.
        [
            "en-US-Standard-A",
            "en-US-Standard-B",
            "en-US-Standard-C",
            "en-US-Standard-D",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }

    fn supported_formats(&self) -> Vec<String> {
        ["mp3", "wav", "ogg"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

// ── Edge TTS (subprocess) ────────────────────────────────────────

/// Edge TTS provider — free, uses the `edge-tts` CLI subprocess.
pub struct EdgeTtsProvider {
    binary_path: String,
}

impl EdgeTtsProvider {
    /// Allowed basenames for the Edge TTS binary.
    const ALLOWED_BINARIES: &[&str] = &["edge-tts", "edge-playback"];

    /// Create a new Edge TTS provider from config.
    ///
    /// `binary_path` must be a bare command name (no path separators) matching
    /// one of [`Self::ALLOWED_BINARIES`]. This prevents arbitrary executable
    /// paths like `/tmp/malicious/edge-tts` from passing the basename check.
    pub fn new(config: &synapse_domain::config::schema::EdgeTtsConfig) -> Result<Self> {
        let path = &config.binary_path;
        if path.contains('/') || path.contains('\\') {
            bail!(
                "Edge TTS binary_path must be a bare command name without path separators, got: {path}"
            );
        }
        if !Self::ALLOWED_BINARIES.contains(&path.as_str()) {
            bail!(
                "Edge TTS binary_path must be one of {:?}, got: {path}",
                Self::ALLOWED_BINARIES,
            );
        }
        Ok(Self {
            binary_path: config.binary_path.clone(),
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for EdgeTtsProvider {
    fn name(&self) -> &str {
        "edge"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let temp_dir = std::env::temp_dir();
        let output_file = temp_dir.join(format!("synapseclaw_tts_{}.mp3", uuid::Uuid::new_v4()));
        let output_path = output_file
            .to_str()
            .context("Failed to build temp file path for Edge TTS")?;

        let output = tokio::time::timeout(
            TTS_HTTP_TIMEOUT,
            tokio::process::Command::new(&self.binary_path)
                .arg("--text")
                .arg(text)
                .arg("--voice")
                .arg(voice)
                .arg("--write-media")
                .arg(output_path)
                .output(),
        )
        .await
        .context("Edge TTS subprocess timed out")?
        .context("Failed to spawn edge-tts subprocess")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up temp file on failure.
            let _ = tokio::fs::remove_file(&output_file).await;
            bail!("edge-tts failed (exit {}): {}", output.status, stderr);
        }

        let bytes = tokio::fs::read(&output_file)
            .await
            .context("Failed to read edge-tts output file")?;

        // Clean up temp file.
        let _ = tokio::fs::remove_file(&output_file).await;

        Ok(bytes)
    }

    fn supported_voices(&self) -> Vec<String> {
        // Edge TTS has many voices; return common defaults.
        [
            "en-US-AriaNeural",
            "en-US-GuyNeural",
            "en-US-JennyNeural",
            "en-GB-SoniaNeural",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["mp3".to_string()]
    }
}

// ── MiniMax TTS ─────────────────────────────────────────────────

/// MiniMax TTS provider (`POST /v1/t2a_v2`).
pub struct MiniMaxTtsProvider {
    api_key: String,
    base_url: String,
    model: String,
    voice_id: String,
    speed: f64,
    volume: f64,
    pitch: i32,
    sample_rate: u32,
    bitrate: u32,
    client: reqwest::Client,
}

impl MiniMaxTtsProvider {
    pub fn new(config: &MiniMaxTtsConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToOwned::to_owned)
            .context("Missing MiniMax TTS API key from resolved speech_synthesis lane")?;

        Ok(Self {
            api_key,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
            voice_id: config.voice_id.clone(),
            speed: config.speed,
            volume: config.volume,
            pitch: config.pitch,
            sample_rate: config.sample_rate,
            bitrate: config.bitrate,
            client: reqwest::Client::builder()
                .timeout(TTS_HTTP_TIMEOUT)
                .build()
                .context("Failed to build HTTP client for MiniMax TTS")?,
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for MiniMaxTtsProvider {
    fn name(&self) -> &str {
        "minimax"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let voice_id = if voice.is_empty() {
            self.voice_id.as_str()
        } else {
            voice
        };
        let body = serde_json::json!({
            "model": self.model,
            "text": text,
            "stream": false,
            "voice_setting": {
                "voice_id": voice_id,
                "speed": self.speed,
                "vol": self.volume,
                "pitch": self.pitch,
            },
            "audio_setting": {
                "sample_rate": self.sample_rate,
                "bitrate": self.bitrate,
                "format": "mp3",
                "channel": 1,
            },
        });

        let resp = self
            .client
            .post(&self.base_url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to send MiniMax TTS request")?;

        let status = resp.status();
        let value: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse MiniMax TTS response")?;

        if !status.is_success() {
            let msg = value["base_resp"]["status_msg"]
                .as_str()
                .or_else(|| value["error"]["message"].as_str())
                .unwrap_or("unknown error");
            bail!("MiniMax TTS API error ({}): {}", status, msg);
        }

        let code = value["base_resp"]["status_code"].as_i64().unwrap_or(0);
        if code != 0 {
            let msg = value["base_resp"]["status_msg"]
                .as_str()
                .unwrap_or("unknown error");
            bail!("MiniMax TTS API error (code {}): {}", code, msg);
        }

        let hex_audio = value["data"]["audio"]
            .as_str()
            .context("MiniMax TTS response missing data.audio")?;
        let bytes = hex::decode(hex_audio).context("Failed to decode MiniMax hex audio")?;
        Ok(bytes)
    }

    fn supported_voices(&self) -> Vec<String> {
        vec![self.voice_id.clone()]
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["mp3".to_string()]
    }
}

// ── Mistral Voxtral TTS ─────────────────────────────────────────

/// Mistral Voxtral TTS provider.
pub struct MistralTtsProvider {
    api_key: String,
    model: String,
    voice_id: String,
    response_format: String,
    client: reqwest::Client,
}

impl MistralTtsProvider {
    pub fn new(config: &MistralTtsConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToOwned::to_owned)
            .context("Missing Mistral TTS API key from resolved speech_synthesis lane")?;

        Ok(Self {
            api_key,
            model: config.model.clone(),
            voice_id: config.voice_id.clone(),
            response_format: config.response_format.clone(),
            client: reqwest::Client::builder()
                .timeout(TTS_HTTP_TIMEOUT)
                .build()
                .context("Failed to build HTTP client for Mistral TTS")?,
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for MistralTtsProvider {
    fn name(&self) -> &str {
        "mistral"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let voice_id = if voice.is_empty() {
            self.voice_id.as_str()
        } else {
            voice
        };
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice_id": voice_id,
            "response_format": self.response_format,
        });

        let resp = self
            .client
            .post("https://api.mistral.ai/v1/audio/speech")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to send Mistral TTS request")?;

        let status = resp.status();
        let value: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse Mistral TTS response")?;

        if !status.is_success() {
            let msg = value["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            bail!("Mistral TTS API error ({}): {}", status, msg);
        }

        let encoded = value["audio_data"]
            .as_str()
            .context("Mistral TTS response missing audio_data")?;
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            .context("Failed to decode Mistral base64 audio")?;
        Ok(bytes)
    }

    fn supported_voices(&self) -> Vec<String> {
        vec![self.voice_id.clone()]
    }

    fn supported_formats(&self) -> Vec<String> {
        ["mp3", "wav", "pcm", "flac", "opus"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

// ── xAI TTS ─────────────────────────────────────────────────────

/// xAI TTS provider (`POST /v1/tts`).
pub struct XaiTtsProvider {
    api_key: String,
    language: String,
    codec: String,
    sample_rate: u32,
    bitrate: u32,
    client: reqwest::Client,
}

impl XaiTtsProvider {
    pub fn new(config: &XaiTtsConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(ToOwned::to_owned)
            .context("Missing xAI TTS API key from resolved speech_synthesis lane")?;

        Ok(Self {
            api_key,
            language: config.language.clone(),
            codec: config.codec.clone(),
            sample_rate: config.sample_rate,
            bitrate: config.bitrate,
            client: reqwest::Client::builder()
                .timeout(TTS_HTTP_TIMEOUT)
                .build()
                .context("Failed to build HTTP client for xAI TTS")?,
        })
    }
}

#[async_trait::async_trait]
impl TtsProvider for XaiTtsProvider {
    fn name(&self) -> &str {
        "xai"
    }

    async fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let voice_id = if voice.is_empty() { "eve" } else { voice };
        let body = serde_json::json!({
            "text": text,
            "voice_id": voice_id,
            "language": self.language,
            "output_format": {
                "codec": self.codec,
                "sample_rate": self.sample_rate,
                "bit_rate": self.bitrate,
            },
        });

        let resp = self
            .client
            .post("https://api.x.ai/v1/tts")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to send xAI TTS request")?;

        let status = resp.status();
        if !status.is_success() {
            let value: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": "unknown"}));
            let msg = value["error"]["message"]
                .as_str()
                .or_else(|| value["error"].as_str())
                .unwrap_or("unknown error");
            bail!("xAI TTS API error ({}): {}", status, msg);
        }

        let bytes = resp
            .bytes()
            .await
            .context("Failed to read xAI TTS response body")?;
        Ok(bytes.to_vec())
    }

    fn supported_voices(&self) -> Vec<String> {
        ["eve", "ara", "rex", "sal", "leo"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }

    fn supported_formats(&self) -> Vec<String> {
        ["mp3", "wav", "pcm", "mulaw", "alaw"]
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    }
}

// ── TtsManager ───────────────────────────────────────────────────

/// Central manager for multi-provider TTS synthesis.
pub struct TtsManager {
    providers: HashMap<String, Box<dyn TtsProvider>>,
    default_provider: String,
    default_voice: String,
    max_text_length: usize,
}

impl TtsManager {
    /// Build a `TtsManager` from config, initializing all configured providers.
    pub fn new(config: &TtsConfig) -> Result<Self> {
        let mut providers: HashMap<String, Box<dyn TtsProvider>> = HashMap::new();

        if let Some(ref openai_cfg) = config.openai {
            match OpenAiTtsProvider::new(openai_cfg) {
                Ok(p) => {
                    providers.insert("openai".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping OpenAI TTS provider: {e}");
                }
            }
        }

        if let Some(ref elevenlabs_cfg) = config.elevenlabs {
            match ElevenLabsTtsProvider::new(elevenlabs_cfg) {
                Ok(p) => {
                    providers.insert("elevenlabs".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping ElevenLabs TTS provider: {e}");
                }
            }
        }

        if let Some(ref google_cfg) = config.google {
            match GoogleTtsProvider::new(google_cfg) {
                Ok(p) => {
                    providers.insert("google".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping Google TTS provider: {e}");
                }
            }
        }

        if let Some(ref edge_cfg) = config.edge {
            match EdgeTtsProvider::new(edge_cfg) {
                Ok(p) => {
                    providers.insert("edge".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping Edge TTS provider: {e}");
                }
            }
        }

        if let Some(ref minimax_cfg) = config.minimax {
            match MiniMaxTtsProvider::new(minimax_cfg) {
                Ok(p) => {
                    providers.insert("minimax".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping MiniMax TTS provider: {e}");
                }
            }
        }

        if let Some(ref mistral_cfg) = config.mistral {
            match MistralTtsProvider::new(mistral_cfg) {
                Ok(p) => {
                    providers.insert("mistral".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping Mistral TTS provider: {e}");
                }
            }
        }

        if let Some(ref xai_cfg) = config.xai {
            match XaiTtsProvider::new(xai_cfg) {
                Ok(p) => {
                    providers.insert("xai".to_string(), Box::new(p));
                }
                Err(e) => {
                    tracing::warn!("Skipping xAI TTS provider: {e}");
                }
            }
        }

        let max_text_length = if config.max_text_length == 0 {
            DEFAULT_MAX_TEXT_LENGTH
        } else {
            config.max_text_length
        };

        Ok(Self {
            providers,
            default_provider: config.default_provider.clone(),
            default_voice: config.default_voice.clone(),
            max_text_length,
        })
    }

    /// Synthesize text using the default provider and voice.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        self.synthesize_with_provider(text, &self.default_provider, &self.default_voice)
            .await
    }

    /// Synthesize text using a specific provider and voice.
    pub async fn synthesize_with_provider(
        &self,
        text: &str,
        provider: &str,
        voice: &str,
    ) -> Result<Vec<u8>> {
        if text.is_empty() {
            bail!("TTS text must not be empty");
        }
        let char_count = text.chars().count();
        if char_count > self.max_text_length {
            bail!(
                "TTS text too long ({} chars, max {})",
                char_count,
                self.max_text_length
            );
        }

        let tts = self.providers.get(provider).ok_or_else(|| {
            anyhow::anyhow!(
                "TTS provider '{}' not configured (available: {})",
                provider,
                self.available_providers().join(", ")
            )
        })?;

        tts.synthesize(text, voice).await
    }

    /// List names of all initialized providers.
    pub fn available_providers(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_tts_config() -> TtsConfig {
        TtsConfig::default()
    }

    #[test]
    fn tts_manager_creation_with_defaults() {
        let config = default_tts_config();
        let manager = TtsManager::new(&config).unwrap();
        // No providers configured by default, so list is empty.
        assert!(manager.available_providers().is_empty());
    }

    #[test]
    fn tts_manager_with_edge_provider() {
        let mut config = default_tts_config();
        config.default_provider = "edge".to_string();
        config.edge = Some(synapse_domain::config::schema::EdgeTtsConfig {
            binary_path: "edge-tts".into(),
        });

        let manager = TtsManager::new(&config).unwrap();
        assert_eq!(manager.available_providers(), vec!["edge"]);
    }

    #[tokio::test]
    async fn tts_rejects_empty_text() {
        let mut config = default_tts_config();
        config.default_provider = "edge".to_string();
        config.edge = Some(synapse_domain::config::schema::EdgeTtsConfig {
            binary_path: "edge-tts".into(),
        });

        let manager = TtsManager::new(&config).unwrap();
        let err = manager
            .synthesize_with_provider("", "edge", "en-US-AriaNeural")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("must not be empty"),
            "expected empty-text error, got: {err}"
        );
    }

    #[tokio::test]
    async fn tts_rejects_text_exceeding_max_length() {
        let mut config = default_tts_config();
        config.default_provider = "edge".to_string();
        config.max_text_length = 10;
        config.edge = Some(synapse_domain::config::schema::EdgeTtsConfig {
            binary_path: "edge-tts".into(),
        });

        let manager = TtsManager::new(&config).unwrap();
        let long_text = "a".repeat(11);
        let err = manager
            .synthesize_with_provider(&long_text, "edge", "en-US-AriaNeural")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("too long"),
            "expected too-long error, got: {err}"
        );
    }

    #[tokio::test]
    async fn tts_rejects_unknown_provider() {
        let config = default_tts_config();
        let manager = TtsManager::new(&config).unwrap();
        let err = manager
            .synthesize_with_provider("hello", "nonexistent", "voice")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not configured"),
            "expected not-configured error, got: {err}"
        );
    }

    #[test]
    fn tts_config_defaults() {
        let config = TtsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.default_provider, "openai");
        assert_eq!(config.default_voice, "alloy");
        assert_eq!(config.default_format, "mp3");
        assert_eq!(config.max_text_length, DEFAULT_MAX_TEXT_LENGTH);
        assert!(config.openai.is_none());
        assert!(config.elevenlabs.is_none());
        assert!(config.google.is_none());
        assert!(config.edge.is_none());
        assert!(config.minimax.is_none());
        assert!(config.mistral.is_none());
        assert!(config.xai.is_none());
    }

    #[test]
    fn tts_manager_max_text_length_zero_uses_default() {
        let mut config = default_tts_config();
        config.max_text_length = 0;
        let manager = TtsManager::new(&config).unwrap();
        assert_eq!(manager.max_text_length, DEFAULT_MAX_TEXT_LENGTH);
    }

    #[test]
    fn tts_manager_registers_voice_parity_providers() {
        let mut config = default_tts_config();
        config.minimax = Some(synapse_domain::config::schema::MiniMaxTtsConfig {
            api_key: Some("minimax-key".into()),
            base_url: "https://api.minimax.io/v1/t2a_v2".into(),
            model: "speech-2.8-hd".into(),
            voice_id: "English_Graceful_Lady".into(),
            speed: 1.0,
            volume: 1.0,
            pitch: 0,
            sample_rate: 32_000,
            bitrate: 128_000,
        });
        config.mistral = Some(synapse_domain::config::schema::MistralTtsConfig {
            api_key: Some("mistral-key".into()),
            model: "voxtral-mini-tts-2603".into(),
            voice_id: "voice-id".into(),
            response_format: "mp3".into(),
        });
        config.xai = Some(synapse_domain::config::schema::XaiTtsConfig {
            api_key: Some("xai-key".into()),
            language: "auto".into(),
            codec: "mp3".into(),
            sample_rate: 24_000,
            bitrate: 128_000,
        });

        let manager = TtsManager::new(&config).unwrap();
        assert_eq!(
            manager.available_providers(),
            vec!["minimax", "mistral", "xai"]
        );
    }
}
