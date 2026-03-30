//! Configuration schema — types re-exported from fork_config + IO operations.
//!
//! All struct/enum definitions now live in `synapse_core::config::schema`.
//! This module re-exports them and provides load/save/encrypt/decrypt.

pub use synapse_core::config::schema::*;

use anyhow::{Context, Result};
use directories::UserDirs;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use synapse_core::config::provider_aliases::{is_glm_alias, is_zai_alias};
use synapse_security::DomainMatcher;
#[cfg(unix)]
use tokio::fs::File;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

fn default_config_and_workspace_dirs() -> Result<(PathBuf, PathBuf)> {
    let config_dir = default_config_dir()?;
    Ok((config_dir.clone(), config_dir.join("workspace")))
}

const ACTIVE_WORKSPACE_STATE_FILE: &str = "active_workspace.toml";

#[derive(Debug, Serialize, Deserialize)]
struct ActiveWorkspaceState {
    config_dir: String,
}

fn default_config_dir() -> Result<PathBuf> {
    let home = UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .context("Could not find home directory")?;
    Ok(home.join(".synapseclaw"))
}

fn active_workspace_state_path(default_dir: &Path) -> PathBuf {
    default_dir.join(ACTIVE_WORKSPACE_STATE_FILE)
}

/// Returns `true` if `path` lives under the OS temp directory.
fn is_temp_directory(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    // Canonicalize when possible to handle symlinks (macOS /var → /private/var)
    let canon_temp = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canon_path.starts_with(&canon_temp)
}

async fn load_persisted_workspace_dirs(
    default_config_dir: &Path,
) -> Result<Option<(PathBuf, PathBuf)>> {
    let state_path = active_workspace_state_path(default_config_dir);
    if !state_path.exists() {
        return Ok(None);
    }

    let contents = match fs::read_to_string(&state_path).await {
        Ok(contents) => contents,
        Err(error) => {
            tracing::warn!(
                "Failed to read active workspace marker {}: {error}",
                state_path.display()
            );
            return Ok(None);
        }
    };

    let state: ActiveWorkspaceState = match toml::from_str(&contents) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(
                "Failed to parse active workspace marker {}: {error}",
                state_path.display()
            );
            return Ok(None);
        }
    };

    let raw_config_dir = state.config_dir.trim();
    if raw_config_dir.is_empty() {
        tracing::warn!(
            "Ignoring active workspace marker {} because config_dir is empty",
            state_path.display()
        );
        return Ok(None);
    }

    let expanded_dir = shellexpand::tilde(raw_config_dir);
    let parsed_dir = PathBuf::from(expanded_dir.as_ref());
    let config_dir = if parsed_dir.is_absolute() {
        parsed_dir
    } else {
        default_config_dir.join(parsed_dir)
    };

    // Safety: reject stale markers pointing at temp directories when the default
    // config dir is NOT itself under temp (i.e. real daemon, not a test with temp HOME).
    // Tests and transient runs can leave behind markers that hijack the daemon's config.
    if is_temp_directory(&config_dir) && !is_temp_directory(default_config_dir) {
        tracing::warn!(
            "Ignoring active workspace marker {} — points at temp directory {}; removing stale marker",
            state_path.display(),
            config_dir.display(),
        );
        let _ = fs::remove_file(&state_path).await;
        return Ok(None);
    }

    Ok(Some((config_dir.clone(), config_dir.join("workspace"))))
}

pub(crate) async fn persist_active_workspace_config_dir(config_dir: &Path) -> Result<()> {
    let default_config_dir = default_config_dir()?;
    let state_path = active_workspace_state_path(&default_config_dir);

    // Guard: never persist a temp-directory path as the active workspace.
    // This prevents transient test runs or one-off invocations from hijacking
    // the daemon's config resolution.
    #[cfg(not(test))]
    if is_temp_directory(config_dir) {
        tracing::warn!(
            path = %config_dir.display(),
            "Refusing to persist temp directory as active workspace marker"
        );
        return Ok(());
    }

    if config_dir == default_config_dir {
        if state_path.exists() {
            fs::remove_file(&state_path).await.with_context(|| {
                format!(
                    "Failed to clear active workspace marker: {}",
                    state_path.display()
                )
            })?;
        }
        return Ok(());
    }

    fs::create_dir_all(&default_config_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create default config directory: {}",
                default_config_dir.display()
            )
        })?;

    let state = ActiveWorkspaceState {
        config_dir: config_dir.to_string_lossy().into_owned(),
    };
    let serialized =
        toml::to_string_pretty(&state).context("Failed to serialize active workspace marker")?;

    let temp_path = default_config_dir.join(format!(
        ".{ACTIVE_WORKSPACE_STATE_FILE}.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    fs::write(&temp_path, serialized).await.with_context(|| {
        format!(
            "Failed to write temporary active workspace marker: {}",
            temp_path.display()
        )
    })?;

    if let Err(error) = fs::rename(&temp_path, &state_path).await {
        let _ = fs::remove_file(&temp_path).await;
        anyhow::bail!(
            "Failed to atomically persist active workspace marker {}: {error}",
            state_path.display()
        );
    }

    sync_directory(&default_config_dir).await?;
    Ok(())
}

pub(crate) fn resolve_config_dir_for_workspace(workspace_dir: &Path) -> (PathBuf, PathBuf) {
    let workspace_config_dir = workspace_dir.to_path_buf();
    if workspace_config_dir.join("config.toml").exists() {
        return (
            workspace_config_dir.clone(),
            workspace_config_dir.join("workspace"),
        );
    }

    let legacy_config_dir = workspace_dir
        .parent()
        .map(|parent| parent.join(".synapseclaw"));
    if let Some(legacy_dir) = legacy_config_dir {
        if legacy_dir.join("config.toml").exists() {
            return (legacy_dir, workspace_config_dir);
        }

        if workspace_dir
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("workspace"))
        {
            return (legacy_dir, workspace_config_dir);
        }
    }

    (
        workspace_config_dir.clone(),
        workspace_config_dir.join("workspace"),
    )
}

/// Resolve the current runtime config/workspace directories for onboarding flows.
///
/// This mirrors the same precedence used by `Config::load_or_init()`:
/// `SYNAPSECLAW_CONFIG_DIR` > `SYNAPSECLAW_WORKSPACE` > active workspace marker > defaults.
pub async fn resolve_runtime_dirs_for_onboarding() -> Result<(PathBuf, PathBuf)> {
    let (default_synapseclaw_dir, default_workspace_dir) = default_config_and_workspace_dirs()?;
    let (config_dir, workspace_dir, _) =
        resolve_runtime_config_dirs(&default_synapseclaw_dir, &default_workspace_dir).await?;
    Ok((config_dir, workspace_dir))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigResolutionSource {
    EnvConfigDir,
    EnvWorkspace,
    ActiveWorkspaceMarker,
    DefaultConfigDir,
}

impl ConfigResolutionSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::EnvConfigDir => "SYNAPSECLAW_CONFIG_DIR",
            Self::EnvWorkspace => "SYNAPSECLAW_WORKSPACE",
            Self::ActiveWorkspaceMarker => "active_workspace.toml",
            Self::DefaultConfigDir => "default",
        }
    }
}

async fn resolve_runtime_config_dirs(
    default_synapseclaw_dir: &Path,
    default_workspace_dir: &Path,
) -> Result<(PathBuf, PathBuf, ConfigResolutionSource)> {
    if let Ok(custom_config_dir) = std::env::var("SYNAPSECLAW_CONFIG_DIR") {
        let custom_config_dir = custom_config_dir.trim();
        if !custom_config_dir.is_empty() {
            let synapseclaw_dir = PathBuf::from(shellexpand::tilde(custom_config_dir).as_ref());
            return Ok((
                synapseclaw_dir.clone(),
                synapseclaw_dir.join("workspace"),
                ConfigResolutionSource::EnvConfigDir,
            ));
        }
    }

    if let Ok(custom_workspace) = std::env::var("SYNAPSECLAW_WORKSPACE") {
        if !custom_workspace.is_empty() {
            let expanded = shellexpand::tilde(&custom_workspace);
            let (synapseclaw_dir, workspace_dir) =
                resolve_config_dir_for_workspace(&PathBuf::from(expanded.as_ref()));
            return Ok((
                synapseclaw_dir,
                workspace_dir,
                ConfigResolutionSource::EnvWorkspace,
            ));
        }
    }

    if let Some((synapseclaw_dir, workspace_dir)) =
        load_persisted_workspace_dirs(default_synapseclaw_dir).await?
    {
        return Ok((
            synapseclaw_dir,
            workspace_dir,
            ConfigResolutionSource::ActiveWorkspaceMarker,
        ));
    }

    Ok((
        default_synapseclaw_dir.to_path_buf(),
        default_workspace_dir.to_path_buf(),
        ConfigResolutionSource::DefaultConfigDir,
    ))
}

fn decrypt_optional_secret(
    store: &synapse_security::SecretStore,
    value: &mut Option<String>,
    field_name: &str,
) -> Result<()> {
    if let Some(raw) = value.clone() {
        if synapse_security::SecretStore::is_encrypted(&raw) {
            *value = Some(
                store
                    .decrypt(&raw)
                    .with_context(|| format!("Failed to decrypt {field_name}"))?,
            );
        }
    }
    Ok(())
}

fn decrypt_secret(
    store: &synapse_security::SecretStore,
    value: &mut String,
    field_name: &str,
) -> Result<()> {
    if synapse_security::SecretStore::is_encrypted(value) {
        *value = store
            .decrypt(value)
            .with_context(|| format!("Failed to decrypt {field_name}"))?;
    }
    Ok(())
}

fn encrypt_optional_secret(
    store: &synapse_security::SecretStore,
    value: &mut Option<String>,
    field_name: &str,
) -> Result<()> {
    if let Some(raw) = value.clone() {
        if !synapse_security::SecretStore::is_encrypted(&raw) {
            *value = Some(
                store
                    .encrypt(&raw)
                    .with_context(|| format!("Failed to encrypt {field_name}"))?,
            );
        }
    }
    Ok(())
}

fn encrypt_secret(
    store: &synapse_security::SecretStore,
    value: &mut String,
    field_name: &str,
) -> Result<()> {
    if !synapse_security::SecretStore::is_encrypted(value) {
        *value = store
            .encrypt(value)
            .with_context(|| format!("Failed to encrypt {field_name}"))?;
    }
    Ok(())
}

fn config_dir_creation_error(path: &Path) -> String {
    format!(
        "Failed to create config directory: {}. If running as an OpenRC service, \
         ensure this path is writable by user 'synapseclaw'.",
        path.display()
    )
}

fn is_local_ollama_endpoint(api_url: Option<&str>) -> bool {
    let Some(raw) = api_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };

    reqwest::Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"))
}

fn has_ollama_cloud_credential(config_api_key: Option<&str>) -> bool {
    let config_key_present = config_api_key
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if config_key_present {
        return true;
    }

    ["OLLAMA_API_KEY", "SYNAPSECLAW_API_KEY", "API_KEY"]
        .iter()
        .any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        })
}

/// Parse the `SYNAPSECLAW_EXTRA_HEADERS` environment variable value.
///
/// Format: `Key:Value,Key2:Value2`
///
/// Entries without a colon or with an empty key are silently skipped.
/// Leading/trailing whitespace on both key and value is trimmed.
pub fn parse_extra_headers_env(raw: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some((key, value)) = entry.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() {
                tracing::warn!(
                    "Ignoring extra header with empty name in SYNAPSECLAW_EXTRA_HEADERS"
                );
                continue;
            }
            result.push((key.to_string(), value.to_string()));
        } else {
            tracing::warn!("Ignoring malformed extra header entry (missing ':'): {entry}");
        }
    }
    result
}

fn normalize_wire_api(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "responses" | "openai-responses" | "open-ai-responses" => Some("responses"),
        "chat_completions"
        | "chat-completions"
        | "chat"
        | "chatcompletions"
        | "openai-chat-completions"
        | "open-ai-chat-completions" => Some("chat_completions"),
        _ => None,
    }
}

fn read_codex_openai_api_key() -> Option<String> {
    let home = UserDirs::new()?.home_dir().to_path_buf();
    let auth_path = home.join(".codex").join("auth.json");
    let raw = std::fs::read_to_string(auth_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;

    parsed
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn lookup_model_provider_profile(
    config: &Config,
    provider_name: &str,
) -> Option<(String, ModelProviderConfig)> {
    let needle = provider_name.trim();
    if needle.is_empty() {
        return None;
    }

    config
        .model_providers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(needle))
        .map(|(name, profile)| (name.clone(), profile.clone()))
}

fn apply_named_model_provider_profile(config: &mut Config) {
    let Some(current_provider) = config.default_provider.clone() else {
        return;
    };

    let Some((profile_key, profile)) = lookup_model_provider_profile(config, &current_provider)
    else {
        return;
    };

    let base_url = profile
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    if config
        .api_url
        .as_deref()
        .map(str::trim)
        .is_none_or(|value| value.is_empty())
    {
        if let Some(base_url) = base_url.as_ref() {
            config.api_url = Some(base_url.clone());
        }
    }

    // Propagate api_path from the profile when not already set at top level.
    if config.api_path.is_none() {
        if let Some(ref path) = profile.api_path {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                config.api_path = Some(trimmed.to_string());
            }
        }
    }

    if profile.requires_openai_auth
        && config
            .api_key
            .as_deref()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
    {
        let codex_key = std::env::var("OPENAI_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(read_codex_openai_api_key);
        if let Some(codex_key) = codex_key {
            config.api_key = Some(codex_key);
        }
    }

    let normalized_wire_api = profile.wire_api.as_deref().and_then(normalize_wire_api);
    let profile_name = profile
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if normalized_wire_api == Some("responses") {
        config.default_provider = Some("openai-codex".to_string());
        return;
    }

    if let Some(profile_name) = profile_name {
        if !profile_name.eq_ignore_ascii_case(&profile_key) {
            config.default_provider = Some(profile_name.to_string());
            return;
        }
    }

    if let Some(base_url) = base_url {
        config.default_provider = Some(format!("custom:{base_url}"));
    }
}

async fn resolve_config_path_for_save(config: &Config) -> Result<PathBuf> {
    if config
        .config_path
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Ok(config.config_path.clone());
    }

    let (default_synapseclaw_dir, default_workspace_dir) = default_config_and_workspace_dirs()?;
    let (synapseclaw_dir, _workspace_dir, source) =
        resolve_runtime_config_dirs(&default_synapseclaw_dir, &default_workspace_dir).await?;
    let file_name = config
        .config_path
        .file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| std::ffi::OsStr::new("config.toml"));
    let resolved = synapseclaw_dir.join(file_name);
    tracing::warn!(
        path = %config.config_path.display(),
        resolved = %resolved.display(),
        source = source.as_str(),
        "Config path missing parent directory; resolving from runtime environment"
    );
    Ok(resolved)
}

#[async_trait::async_trait]
pub trait ConfigIO {
    async fn load_or_init() -> Result<Self>
    where
        Self: Sized;
    fn validate(&self) -> Result<()>;
    fn apply_env_overrides(&mut self);
    async fn save(&self) -> Result<()>;
}

#[async_trait::async_trait]
impl ConfigIO for Config {
    async fn load_or_init() -> Result<Self> {
        let (default_synapseclaw_dir, default_workspace_dir) = default_config_and_workspace_dirs()?;

        let (synapseclaw_dir, workspace_dir, resolution_source) =
            resolve_runtime_config_dirs(&default_synapseclaw_dir, &default_workspace_dir).await?;

        let config_path = synapseclaw_dir.join("config.toml");

        fs::create_dir_all(&synapseclaw_dir)
            .await
            .with_context(|| config_dir_creation_error(&synapseclaw_dir))?;
        fs::create_dir_all(&workspace_dir)
            .await
            .context("Failed to create workspace directory")?;

        if config_path.exists() {
            // Warn if config file is world-readable (may contain API keys)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&config_path).await {
                    if meta.permissions().mode() & 0o004 != 0 {
                        tracing::warn!(
                            "Config file {:?} is world-readable (mode {:o}). \
                             Consider restricting with: chmod 600 {:?}",
                            config_path,
                            meta.permissions().mode() & 0o777,
                            config_path,
                        );
                    }
                }
            }

            let contents = fs::read_to_string(&config_path)
                .await
                .context("Failed to read config file")?;

            // Track ignored/unknown config keys to warn users about silent misconfigurations
            // (e.g., using [providers.ollama] which doesn't exist instead of top-level api_url)
            let mut ignored_paths: Vec<String> = Vec::new();
            let mut config: Config = serde_ignored::deserialize(
                toml::de::Deserializer::parse(&contents).context("Failed to parse config file")?,
                |path| {
                    ignored_paths.push(path.to_string());
                },
            )
            .context("Failed to deserialize config file")?;

            // Warn about each unknown config key
            for path in ignored_paths {
                tracing::warn!(
                    "Unknown config key ignored: \"{}\". Check config.toml for typos or deprecated options.",
                    path
                );
            }
            // Set computed paths that are skipped during serialization
            config.config_path = config_path.clone();
            config.workspace_dir = workspace_dir;
            let store =
                synapse_security::SecretStore::new(&synapseclaw_dir, config.secrets.encrypt);
            decrypt_optional_secret(&store, &mut config.api_key, "config.api_key")?;
            decrypt_optional_secret(
                &store,
                &mut config.composio.api_key,
                "config.composio.api_key",
            )?;
            decrypt_optional_secret(
                &store,
                &mut config.microsoft365.client_secret,
                "config.microsoft365.client_secret",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.browser.computer_use.api_key,
                "config.browser.computer_use.api_key",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.web_search.brave_api_key,
                "config.web_search.brave_api_key",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.web_search.tavily_api_key,
                "config.web_search.tavily_api_key",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.storage.provider.config.db_url,
                "config.storage.provider.config.db_url",
            )?;

            for agent in config.agents.values_mut() {
                decrypt_optional_secret(&store, &mut agent.api_key, "config.agents.*.api_key")?;
            }

            // Decrypt TTS provider API keys
            if let Some(ref mut openai) = config.tts.openai {
                decrypt_optional_secret(&store, &mut openai.api_key, "config.tts.openai.api_key")?;
            }
            if let Some(ref mut elevenlabs) = config.tts.elevenlabs {
                decrypt_optional_secret(
                    &store,
                    &mut elevenlabs.api_key,
                    "config.tts.elevenlabs.api_key",
                )?;
            }
            if let Some(ref mut google) = config.tts.google {
                decrypt_optional_secret(&store, &mut google.api_key, "config.tts.google.api_key")?;
            }

            if let Some(ref mut matrix) = config.channels_config.matrix {
                decrypt_optional_secret(
                    &store,
                    &mut matrix.access_token,
                    "config.channels_config.matrix.access_token",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut matrix.password,
                    "config.channels_config.matrix.password",
                )?;
            }

            // Decrypt nested STT provider API keys
            decrypt_optional_secret(
                &store,
                &mut config.transcription.api_key,
                "config.transcription.api_key",
            )?;
            if let Some(ref mut openai) = config.transcription.openai {
                decrypt_optional_secret(
                    &store,
                    &mut openai.api_key,
                    "config.transcription.openai.api_key",
                )?;
            }
            if let Some(ref mut deepgram) = config.transcription.deepgram {
                decrypt_optional_secret(
                    &store,
                    &mut deepgram.api_key,
                    "config.transcription.deepgram.api_key",
                )?;
            }
            if let Some(ref mut assemblyai) = config.transcription.assemblyai {
                decrypt_optional_secret(
                    &store,
                    &mut assemblyai.api_key,
                    "config.transcription.assemblyai.api_key",
                )?;
            }
            if let Some(ref mut google) = config.transcription.google {
                decrypt_optional_secret(
                    &store,
                    &mut google.api_key,
                    "config.transcription.google.api_key",
                )?;
            }

            #[cfg(feature = "channel-nostr")]
            if let Some(ref mut ns) = config.channels_config.nostr {
                decrypt_secret(
                    &store,
                    &mut ns.private_key,
                    "config.channels_config.nostr.private_key",
                )?;
            }
            if let Some(ref mut fs) = config.channels_config.feishu {
                decrypt_secret(
                    &store,
                    &mut fs.app_secret,
                    "config.channels_config.feishu.app_secret",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut fs.encrypt_key,
                    "config.channels_config.feishu.encrypt_key",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut fs.verification_token,
                    "config.channels_config.feishu.verification_token",
                )?;
            }

            // Decrypt channel secrets
            if let Some(ref mut tg) = config.channels_config.telegram {
                decrypt_secret(
                    &store,
                    &mut tg.bot_token,
                    "config.channels_config.telegram.bot_token",
                )?;
            }
            if let Some(ref mut dc) = config.channels_config.discord {
                decrypt_secret(
                    &store,
                    &mut dc.bot_token,
                    "config.channels_config.discord.bot_token",
                )?;
            }
            if let Some(ref mut sl) = config.channels_config.slack {
                decrypt_secret(
                    &store,
                    &mut sl.bot_token,
                    "config.channels_config.slack.bot_token",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut sl.app_token,
                    "config.channels_config.slack.app_token",
                )?;
            }
            if let Some(ref mut mm) = config.channels_config.mattermost {
                decrypt_secret(
                    &store,
                    &mut mm.bot_token,
                    "config.channels_config.mattermost.bot_token",
                )?;
            }
            if let Some(ref mut wa) = config.channels_config.whatsapp {
                decrypt_optional_secret(
                    &store,
                    &mut wa.access_token,
                    "config.channels_config.whatsapp.access_token",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut wa.app_secret,
                    "config.channels_config.whatsapp.app_secret",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut wa.verify_token,
                    "config.channels_config.whatsapp.verify_token",
                )?;
            }
            if let Some(ref mut lq) = config.channels_config.linq {
                decrypt_secret(
                    &store,
                    &mut lq.api_token,
                    "config.channels_config.linq.api_token",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut lq.signing_secret,
                    "config.channels_config.linq.signing_secret",
                )?;
            }
            if let Some(ref mut wt) = config.channels_config.wati {
                decrypt_secret(
                    &store,
                    &mut wt.api_token,
                    "config.channels_config.wati.api_token",
                )?;
            }
            if let Some(ref mut nc) = config.channels_config.nextcloud_talk {
                decrypt_secret(
                    &store,
                    &mut nc.app_token,
                    "config.channels_config.nextcloud_talk.app_token",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut nc.webhook_secret,
                    "config.channels_config.nextcloud_talk.webhook_secret",
                )?;
            }
            if let Some(ref mut em) = config.channels_config.email {
                decrypt_secret(
                    &store,
                    &mut em.password,
                    "config.channels_config.email.password",
                )?;
            }
            if let Some(ref mut irc) = config.channels_config.irc {
                decrypt_optional_secret(
                    &store,
                    &mut irc.server_password,
                    "config.channels_config.irc.server_password",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut irc.nickserv_password,
                    "config.channels_config.irc.nickserv_password",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut irc.sasl_password,
                    "config.channels_config.irc.sasl_password",
                )?;
            }
            if let Some(ref mut lk) = config.channels_config.lark {
                decrypt_secret(
                    &store,
                    &mut lk.app_secret,
                    "config.channels_config.lark.app_secret",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut lk.encrypt_key,
                    "config.channels_config.lark.encrypt_key",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut lk.verification_token,
                    "config.channels_config.lark.verification_token",
                )?;
            }
            if let Some(ref mut fs) = config.channels_config.feishu {
                decrypt_secret(
                    &store,
                    &mut fs.app_secret,
                    "config.channels_config.feishu.app_secret",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut fs.encrypt_key,
                    "config.channels_config.feishu.encrypt_key",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut fs.verification_token,
                    "config.channels_config.feishu.verification_token",
                )?;
            }
            if let Some(ref mut dt) = config.channels_config.dingtalk {
                decrypt_secret(
                    &store,
                    &mut dt.client_secret,
                    "config.channels_config.dingtalk.client_secret",
                )?;
            }
            if let Some(ref mut wc) = config.channels_config.wecom {
                decrypt_secret(
                    &store,
                    &mut wc.webhook_key,
                    "config.channels_config.wecom.webhook_key",
                )?;
            }
            if let Some(ref mut qq) = config.channels_config.qq {
                decrypt_secret(
                    &store,
                    &mut qq.app_secret,
                    "config.channels_config.qq.app_secret",
                )?;
            }
            if let Some(ref mut wh) = config.channels_config.webhook {
                decrypt_optional_secret(
                    &store,
                    &mut wh.secret,
                    "config.channels_config.webhook.secret",
                )?;
            }
            if let Some(ref mut ct) = config.channels_config.clawdtalk {
                decrypt_secret(
                    &store,
                    &mut ct.api_key,
                    "config.channels_config.clawdtalk.api_key",
                )?;
                decrypt_optional_secret(
                    &store,
                    &mut ct.webhook_secret,
                    "config.channels_config.clawdtalk.webhook_secret",
                )?;
            }

            // Decrypt gateway paired tokens
            for token in &mut config.gateway.paired_tokens {
                decrypt_secret(&store, token, "config.gateway.paired_tokens[]")?;
            }

            // Decrypt IPC tokens (Phase 3.8)
            decrypt_optional_secret(
                &store,
                &mut config.agents_ipc.broker_token,
                "config.agents_ipc.broker_token",
            )?;
            decrypt_optional_secret(
                &store,
                &mut config.agents_ipc.proxy_token,
                "config.agents_ipc.proxy_token",
            )?;

            // Decrypt Nevis IAM secret
            decrypt_optional_secret(
                &store,
                &mut config.security.nevis.client_secret,
                "config.security.nevis.client_secret",
            )?;

            // Notion API key (top-level, not in ChannelsConfig)
            if !config.notion.api_key.is_empty() {
                decrypt_secret(&store, &mut config.notion.api_key, "config.notion.api_key")?;
            }

            config.apply_env_overrides();
            config.validate()?;
            tracing::info!(
                path = %config.config_path.display(),
                workspace = %config.workspace_dir.display(),
                source = resolution_source.as_str(),
                initialized = false,
                "Config loaded"
            );
            Ok(config)
        } else {
            let mut config = Config::default();
            config.config_path = config_path.clone();
            config.workspace_dir = workspace_dir;
            config.save().await?;

            // Restrict permissions on newly created config file (may contain API keys)
            #[cfg(unix)]
            {
                use std::{fs::Permissions, os::unix::fs::PermissionsExt};
                let _ = fs::set_permissions(&config_path, Permissions::from_mode(0o600)).await;
            }

            config.apply_env_overrides();
            config.validate()?;
            tracing::info!(
                path = %config.config_path.display(),
                workspace = %config.workspace_dir.display(),
                source = resolution_source.as_str(),
                initialized = true,
                "Config loaded"
            );
            Ok(config)
        }
    }

    /// Validate configuration values that would cause runtime failures.
    ///
    /// Called after TOML deserialization and env-override application to catch
    /// obviously invalid values early instead of failing at arbitrary runtime points.
    fn validate(&self) -> Result<()> {
        // Tunnel — OpenVPN
        if self.tunnel.provider.trim() == "openvpn" {
            let openvpn = self.tunnel.openvpn.as_ref().ok_or_else(|| {
                anyhow::anyhow!("tunnel.provider='openvpn' requires [tunnel.openvpn]")
            })?;

            if openvpn.config_file.trim().is_empty() {
                anyhow::bail!("tunnel.openvpn.config_file must not be empty");
            }
            if openvpn.connect_timeout_secs == 0 {
                anyhow::bail!("tunnel.openvpn.connect_timeout_secs must be greater than 0");
            }
        }

        // Gateway
        if self.gateway.host.trim().is_empty() {
            anyhow::bail!("gateway.host must not be empty");
        }

        // Autonomy
        if self.autonomy.max_actions_per_hour == 0 {
            anyhow::bail!("autonomy.max_actions_per_hour must be greater than 0");
        }
        for (i, env_name) in self.autonomy.shell_env_passthrough.iter().enumerate() {
            if !is_valid_env_var_name(env_name) {
                anyhow::bail!(
                    "autonomy.shell_env_passthrough[{i}] is invalid ({env_name}); expected [A-Za-z_][A-Za-z0-9_]*"
                );
            }
        }

        // Security OTP / estop
        if self.security.otp.token_ttl_secs == 0 {
            anyhow::bail!("security.otp.token_ttl_secs must be greater than 0");
        }
        if self.security.otp.cache_valid_secs == 0 {
            anyhow::bail!("security.otp.cache_valid_secs must be greater than 0");
        }
        if self.security.otp.cache_valid_secs < self.security.otp.token_ttl_secs {
            anyhow::bail!(
                "security.otp.cache_valid_secs must be greater than or equal to security.otp.token_ttl_secs"
            );
        }
        for (i, action) in self.security.otp.gated_actions.iter().enumerate() {
            let normalized = action.trim();
            if normalized.is_empty() {
                anyhow::bail!("security.otp.gated_actions[{i}] must not be empty");
            }
            if !normalized
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                anyhow::bail!(
                    "security.otp.gated_actions[{i}] contains invalid characters: {normalized}"
                );
            }
        }
        DomainMatcher::new(
            &self.security.otp.gated_domains,
            &self.security.otp.gated_domain_categories,
        )
        .with_context(|| {
            "Invalid security.otp.gated_domains or security.otp.gated_domain_categories"
        })?;
        if self.security.estop.state_file.trim().is_empty() {
            anyhow::bail!("security.estop.state_file must not be empty");
        }

        // Scheduler
        if self.scheduler.max_concurrent == 0 {
            anyhow::bail!("scheduler.max_concurrent must be greater than 0");
        }
        if self.scheduler.max_tasks == 0 {
            anyhow::bail!("scheduler.max_tasks must be greater than 0");
        }

        // Model routes
        for (i, route) in self.model_routes.iter().enumerate() {
            if route.hint.trim().is_empty() {
                anyhow::bail!("model_routes[{i}].hint must not be empty");
            }
            if route.provider.trim().is_empty() {
                anyhow::bail!("model_routes[{i}].provider must not be empty");
            }
            if route.model.trim().is_empty() {
                anyhow::bail!("model_routes[{i}].model must not be empty");
            }
        }

        // Embedding routes
        for (i, route) in self.embedding_routes.iter().enumerate() {
            if route.hint.trim().is_empty() {
                anyhow::bail!("embedding_routes[{i}].hint must not be empty");
            }
            if route.provider.trim().is_empty() {
                anyhow::bail!("embedding_routes[{i}].provider must not be empty");
            }
            if route.model.trim().is_empty() {
                anyhow::bail!("embedding_routes[{i}].model must not be empty");
            }
        }

        for (profile_key, profile) in &self.model_providers {
            let profile_name = profile_key.trim();
            if profile_name.is_empty() {
                anyhow::bail!("model_providers contains an empty profile name");
            }

            let has_name = profile
                .name
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty());
            let has_base_url = profile
                .base_url
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty());

            if !has_name && !has_base_url {
                anyhow::bail!(
                    "model_providers.{profile_name} must define at least one of `name` or `base_url`"
                );
            }

            if let Some(base_url) = profile.base_url.as_deref().map(str::trim) {
                if !base_url.is_empty() {
                    let parsed = reqwest::Url::parse(base_url).with_context(|| {
                        format!("model_providers.{profile_name}.base_url is not a valid URL")
                    })?;
                    if !matches!(parsed.scheme(), "http" | "https") {
                        anyhow::bail!(
                            "model_providers.{profile_name}.base_url must use http/https"
                        );
                    }
                }
            }

            if let Some(wire_api) = profile.wire_api.as_deref().map(str::trim) {
                if !wire_api.is_empty() && normalize_wire_api(wire_api).is_none() {
                    anyhow::bail!(
                        "model_providers.{profile_name}.wire_api must be one of: responses, chat_completions"
                    );
                }
            }
        }

        // Ollama cloud-routing safety checks
        if self
            .default_provider
            .as_deref()
            .is_some_and(|provider| provider.trim().eq_ignore_ascii_case("ollama"))
            && self
                .default_model
                .as_deref()
                .is_some_and(|model| model.trim().ends_with(":cloud"))
        {
            if is_local_ollama_endpoint(self.api_url.as_deref()) {
                anyhow::bail!(
                    "default_model uses ':cloud' with provider 'ollama', but api_url is local or unset. Set api_url to a remote Ollama endpoint (for example https://ollama.com)."
                );
            }

            if !has_ollama_cloud_credential(self.api_key.as_deref()) {
                anyhow::bail!(
                    "default_model uses ':cloud' with provider 'ollama', but no API key is configured. Set api_key or OLLAMA_API_KEY."
                );
            }
        }

        // Matrix: password-only login requires user_id
        if let Some(ref matrix) = self.channels_config.matrix {
            let has_access_token = matrix
                .access_token
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty());
            let has_password = matrix
                .password
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty());
            let has_user_id = matrix
                .user_id
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty());

            if has_password && !has_access_token && !has_user_id {
                anyhow::bail!(
                    "channels_config.matrix.user_id is required when password is set and access_token is omitted"
                );
            }
        }

        // Microsoft 365
        if self.microsoft365.enabled {
            let tenant = self
                .microsoft365
                .tenant_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if tenant.is_none() {
                anyhow::bail!(
                    "microsoft365.tenant_id must not be empty when microsoft365 is enabled"
                );
            }
            let client = self
                .microsoft365
                .client_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if client.is_none() {
                anyhow::bail!(
                    "microsoft365.client_id must not be empty when microsoft365 is enabled"
                );
            }
            let flow = self.microsoft365.auth_flow.trim();
            if flow != "client_credentials" && flow != "device_code" {
                anyhow::bail!(
                    "microsoft365.auth_flow must be 'client_credentials' or 'device_code'"
                );
            }
            if flow == "client_credentials"
                && self
                    .microsoft365
                    .client_secret
                    .as_deref()
                    .map_or(true, |s| s.trim().is_empty())
            {
                anyhow::bail!(
                    "microsoft365.client_secret must not be empty when auth_flow is 'client_credentials'"
                );
            }
        }

        // MCP
        if self.mcp.enabled {
            validate_mcp_config(&self.mcp)?;
        }

        // Knowledge graph
        if self.knowledge.enabled {
            if self.knowledge.max_nodes == 0 {
                anyhow::bail!("knowledge.max_nodes must be greater than 0");
            }
            if self.knowledge.db_path.trim().is_empty() {
                anyhow::bail!("knowledge.db_path must not be empty");
            }
        }

        // Google Workspace allowed_services validation
        let mut seen_gws_services = std::collections::HashSet::new();
        for (i, service) in self.google_workspace.allowed_services.iter().enumerate() {
            let normalized = service.trim();
            if normalized.is_empty() {
                anyhow::bail!("google_workspace.allowed_services[{i}] must not be empty");
            }
            if !normalized
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
            {
                anyhow::bail!(
                    "google_workspace.allowed_services[{i}] contains invalid characters: {normalized}"
                );
            }
            if !seen_gws_services.insert(normalized.to_string()) {
                anyhow::bail!(
                    "google_workspace.allowed_services contains duplicate entry: {normalized}"
                );
            }
        }

        // Project intelligence
        if self.project_intel.enabled {
            let lang = &self.project_intel.default_language;
            if !["en", "de", "fr", "it"].contains(&lang.as_str()) {
                anyhow::bail!(
                    "project_intel.default_language must be one of: en, de, fr, it (got '{lang}')"
                );
            }
            let sens = &self.project_intel.risk_sensitivity;
            if !["low", "medium", "high"].contains(&sens.as_str()) {
                anyhow::bail!(
                    "project_intel.risk_sensitivity must be one of: low, medium, high (got '{sens}')"
                );
            }
            if let Some(ref tpl_dir) = self.project_intel.templates_dir {
                let path = std::path::Path::new(tpl_dir);
                if !path.exists() {
                    anyhow::bail!("project_intel.templates_dir path does not exist: {tpl_dir}");
                }
            }
        }

        // Proxy (delegate to existing validation)
        self.proxy.validate()?;
        self.cloud_ops.validate()?;

        // Notion
        if self.notion.enabled {
            if self.notion.database_id.trim().is_empty() {
                anyhow::bail!("notion.database_id must not be empty when notion.enabled = true");
            }
            if self.notion.poll_interval_secs == 0 {
                anyhow::bail!("notion.poll_interval_secs must be greater than 0");
            }
            if self.notion.max_concurrent == 0 {
                anyhow::bail!("notion.max_concurrent must be greater than 0");
            }
            if self.notion.status_property.trim().is_empty() {
                anyhow::bail!("notion.status_property must not be empty");
            }
            if self.notion.input_property.trim().is_empty() {
                anyhow::bail!("notion.input_property must not be empty");
            }
            if self.notion.result_property.trim().is_empty() {
                anyhow::bail!("notion.result_property must not be empty");
            }
        }

        // Nevis IAM — delegate to NevisConfig::validate() for field-level checks
        if let Err(msg) = self.security.nevis.validate() {
            anyhow::bail!("security.nevis: {msg}");
        }

        // Transcription
        {
            let dp = self.transcription.default_provider.trim();
            match dp {
                "groq" | "openai" | "deepgram" | "assemblyai" | "google" => {}
                other => {
                    anyhow::bail!(
                        "transcription.default_provider must be one of: groq, openai, deepgram, assemblyai, google (got '{other}')"
                    );
                }
            }
        }

        // Transcription
        {
            let dp = self.transcription.default_provider.trim();
            match dp {
                "groq" | "openai" | "deepgram" | "assemblyai" | "google" => {}
                other => {
                    anyhow::bail!(
                        "transcription.default_provider must be one of: groq, openai, deepgram, assemblyai, google (got '{other}')"
                    );
                }
            }
        }

        Ok(())
    }

    /// Apply environment variable overrides to config
    fn apply_env_overrides(&mut self) {
        // API Key: SYNAPSECLAW_API_KEY or API_KEY (generic)
        if let Ok(key) = std::env::var("SYNAPSECLAW_API_KEY").or_else(|_| std::env::var("API_KEY"))
        {
            if !key.is_empty() {
                self.api_key = Some(key);
            }
        }
        // API Key: GLM_API_KEY overrides when provider is a GLM/Zhipu variant.
        if self.default_provider.as_deref().is_some_and(is_glm_alias) {
            if let Ok(key) = std::env::var("GLM_API_KEY") {
                if !key.is_empty() {
                    self.api_key = Some(key);
                }
            }
        }

        // API Key: ZAI_API_KEY overrides when provider is a Z.AI variant.
        if self.default_provider.as_deref().is_some_and(is_zai_alias) {
            if let Ok(key) = std::env::var("ZAI_API_KEY") {
                if !key.is_empty() {
                    self.api_key = Some(key);
                }
            }
        }

        // Provider override precedence:
        // 1) SYNAPSECLAW_PROVIDER always wins when set.
        // 2) SYNAPSECLAW_MODEL_PROVIDER/MODEL_PROVIDER (Codex app-server style).
        // 3) Legacy PROVIDER is honored only when config still uses default provider.
        if let Ok(provider) = std::env::var("SYNAPSECLAW_PROVIDER") {
            if !provider.is_empty() {
                self.default_provider = Some(provider);
            }
        } else if let Ok(provider) =
            std::env::var("SYNAPSECLAW_MODEL_PROVIDER").or_else(|_| std::env::var("MODEL_PROVIDER"))
        {
            if !provider.is_empty() {
                self.default_provider = Some(provider);
            }
        } else if let Ok(provider) = std::env::var("PROVIDER") {
            let should_apply_legacy_provider =
                self.default_provider.as_deref().map_or(true, |configured| {
                    configured.trim().eq_ignore_ascii_case("openrouter")
                });
            if should_apply_legacy_provider && !provider.is_empty() {
                self.default_provider = Some(provider);
            }
        }

        // Model: SYNAPSECLAW_MODEL or MODEL
        if let Ok(model) = std::env::var("SYNAPSECLAW_MODEL").or_else(|_| std::env::var("MODEL")) {
            if !model.is_empty() {
                self.default_model = Some(model);
            }
        }

        // Provider HTTP timeout: SYNAPSECLAW_PROVIDER_TIMEOUT_SECS
        if let Ok(timeout_secs) = std::env::var("SYNAPSECLAW_PROVIDER_TIMEOUT_SECS") {
            if let Ok(timeout_secs) = timeout_secs.parse::<u64>() {
                if timeout_secs > 0 {
                    self.provider_timeout_secs = timeout_secs;
                }
            }
        }

        // Extra provider headers: SYNAPSECLAW_EXTRA_HEADERS
        // Format: "Key:Value,Key2:Value2"
        // Env var headers override config file headers with the same name.
        if let Ok(raw) = std::env::var("SYNAPSECLAW_EXTRA_HEADERS") {
            for header in parse_extra_headers_env(&raw) {
                self.extra_headers.insert(header.0, header.1);
            }
        }

        // Apply named provider profile remapping (Codex app-server compatibility).
        apply_named_model_provider_profile(self);

        // Workspace directory: SYNAPSECLAW_WORKSPACE
        if let Ok(workspace) = std::env::var("SYNAPSECLAW_WORKSPACE") {
            if !workspace.is_empty() {
                let expanded = shellexpand::tilde(&workspace);
                let (_, workspace_dir) =
                    resolve_config_dir_for_workspace(&PathBuf::from(expanded.as_ref()));
                self.workspace_dir = workspace_dir;
            }
        }

        // Open-skills opt-in flag: SYNAPSECLAW_OPEN_SKILLS_ENABLED
        if let Ok(flag) = std::env::var("SYNAPSECLAW_OPEN_SKILLS_ENABLED") {
            if !flag.trim().is_empty() {
                match flag.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" | "on" => self.skills.open_skills_enabled = true,
                    "0" | "false" | "no" | "off" => self.skills.open_skills_enabled = false,
                    _ => tracing::warn!(
                        "Ignoring invalid SYNAPSECLAW_OPEN_SKILLS_ENABLED (valid: 1|0|true|false|yes|no|on|off)"
                    ),
                }
            }
        }

        // Open-skills directory override: SYNAPSECLAW_OPEN_SKILLS_DIR
        if let Ok(path) = std::env::var("SYNAPSECLAW_OPEN_SKILLS_DIR") {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                self.skills.open_skills_dir = Some(trimmed.to_string());
            }
        }

        // Skills prompt mode override: SYNAPSECLAW_SKILLS_PROMPT_MODE
        if let Ok(mode) = std::env::var("SYNAPSECLAW_SKILLS_PROMPT_MODE") {
            if !mode.trim().is_empty() {
                if let Some(parsed) = parse_skills_prompt_injection_mode(&mode) {
                    self.skills.prompt_injection_mode = parsed;
                } else {
                    tracing::warn!(
                        "Ignoring invalid SYNAPSECLAW_SKILLS_PROMPT_MODE (valid: full|compact)"
                    );
                }
            }
        }

        // Gateway port: SYNAPSECLAW_GATEWAY_PORT or PORT
        if let Ok(port_str) =
            std::env::var("SYNAPSECLAW_GATEWAY_PORT").or_else(|_| std::env::var("PORT"))
        {
            if let Ok(port) = port_str.parse::<u16>() {
                self.gateway.port = port;
            }
        }

        // Gateway host: SYNAPSECLAW_GATEWAY_HOST or HOST
        if let Ok(host) =
            std::env::var("SYNAPSECLAW_GATEWAY_HOST").or_else(|_| std::env::var("HOST"))
        {
            if !host.is_empty() {
                self.gateway.host = host;
            }
        }

        // Allow public bind: SYNAPSECLAW_ALLOW_PUBLIC_BIND
        if let Ok(val) = std::env::var("SYNAPSECLAW_ALLOW_PUBLIC_BIND") {
            self.gateway.allow_public_bind = val == "1" || val.eq_ignore_ascii_case("true");
        }

        // Temperature: SYNAPSECLAW_TEMPERATURE
        if let Ok(temp_str) = std::env::var("SYNAPSECLAW_TEMPERATURE") {
            match temp_str.parse::<f64>() {
                Ok(temp) if TEMPERATURE_RANGE.contains(&temp) => {
                    self.default_temperature = temp;
                }
                Ok(temp) => {
                    tracing::warn!(
                        "Ignoring SYNAPSECLAW_TEMPERATURE={temp}: \
                         value out of range (expected {}..={})",
                        TEMPERATURE_RANGE.start(),
                        TEMPERATURE_RANGE.end()
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        "Ignoring SYNAPSECLAW_TEMPERATURE={temp_str:?}: not a valid number"
                    );
                }
            }
        }

        // Reasoning override: SYNAPSECLAW_REASONING_ENABLED or REASONING_ENABLED
        if let Ok(flag) = std::env::var("SYNAPSECLAW_REASONING_ENABLED")
            .or_else(|_| std::env::var("REASONING_ENABLED"))
        {
            let normalized = flag.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "true" | "yes" | "on" => self.runtime.reasoning_enabled = Some(true),
                "0" | "false" | "no" | "off" => self.runtime.reasoning_enabled = Some(false),
                _ => {}
            }
        }

        if let Ok(raw) = std::env::var("SYNAPSECLAW_REASONING_EFFORT")
            .or_else(|_| std::env::var("REASONING_EFFORT"))
            .or_else(|_| std::env::var("SYNAPSECLAW_CODEX_REASONING_EFFORT"))
        {
            match normalize_reasoning_effort(&raw) {
                Ok(effort) => self.runtime.reasoning_effort = Some(effort),
                Err(message) => tracing::warn!("Ignoring reasoning effort env override: {message}"),
            }
        }

        // Web search enabled: SYNAPSECLAW_WEB_SEARCH_ENABLED or WEB_SEARCH_ENABLED
        if let Ok(enabled) = std::env::var("SYNAPSECLAW_WEB_SEARCH_ENABLED")
            .or_else(|_| std::env::var("WEB_SEARCH_ENABLED"))
        {
            self.web_search.enabled = enabled == "1" || enabled.eq_ignore_ascii_case("true");
        }

        // Web search provider: SYNAPSECLAW_WEB_SEARCH_PROVIDER or WEB_SEARCH_PROVIDER
        if let Ok(provider) = std::env::var("SYNAPSECLAW_WEB_SEARCH_PROVIDER")
            .or_else(|_| std::env::var("WEB_SEARCH_PROVIDER"))
        {
            let provider = provider.trim();
            if !provider.is_empty() {
                self.web_search.provider = provider.to_string();
            }
        }

        // Brave API key: SYNAPSECLAW_BRAVE_API_KEY or BRAVE_API_KEY
        if let Ok(api_key) =
            std::env::var("SYNAPSECLAW_BRAVE_API_KEY").or_else(|_| std::env::var("BRAVE_API_KEY"))
        {
            let api_key = api_key.trim();
            if !api_key.is_empty() {
                self.web_search.brave_api_key = Some(api_key.to_string());
            }
        }

        // Tavily API key: SYNAPSECLAW_TAVILY_API_KEY or TAVILY_API_KEY
        if let Ok(api_key) =
            std::env::var("SYNAPSECLAW_TAVILY_API_KEY").or_else(|_| std::env::var("TAVILY_API_KEY"))
        {
            let api_key = api_key.trim();
            if !api_key.is_empty() {
                self.web_search.tavily_api_key = Some(api_key.to_string());
            }
        }

        // Web search max results: SYNAPSECLAW_WEB_SEARCH_MAX_RESULTS or WEB_SEARCH_MAX_RESULTS
        if let Ok(max_results) = std::env::var("SYNAPSECLAW_WEB_SEARCH_MAX_RESULTS")
            .or_else(|_| std::env::var("WEB_SEARCH_MAX_RESULTS"))
        {
            if let Ok(max_results) = max_results.parse::<usize>() {
                if (1..=10).contains(&max_results) {
                    self.web_search.max_results = max_results;
                }
            }
        }

        // Web search timeout: SYNAPSECLAW_WEB_SEARCH_TIMEOUT_SECS or WEB_SEARCH_TIMEOUT_SECS
        if let Ok(timeout_secs) = std::env::var("SYNAPSECLAW_WEB_SEARCH_TIMEOUT_SECS")
            .or_else(|_| std::env::var("WEB_SEARCH_TIMEOUT_SECS"))
        {
            if let Ok(timeout_secs) = timeout_secs.parse::<u64>() {
                if timeout_secs > 0 {
                    self.web_search.timeout_secs = timeout_secs;
                }
            }
        }

        // Storage provider key (optional backend override): SYNAPSECLAW_STORAGE_PROVIDER
        if let Ok(provider) = std::env::var("SYNAPSECLAW_STORAGE_PROVIDER") {
            let provider = provider.trim();
            if !provider.is_empty() {
                self.storage.provider.config.provider = provider.to_string();
            }
        }

        // Storage connection URL (for remote backends): SYNAPSECLAW_STORAGE_DB_URL
        if let Ok(db_url) = std::env::var("SYNAPSECLAW_STORAGE_DB_URL") {
            let db_url = db_url.trim();
            if !db_url.is_empty() {
                self.storage.provider.config.db_url = Some(db_url.to_string());
            }
        }

        // Storage connect timeout: SYNAPSECLAW_STORAGE_CONNECT_TIMEOUT_SECS
        if let Ok(timeout_secs) = std::env::var("SYNAPSECLAW_STORAGE_CONNECT_TIMEOUT_SECS") {
            if let Ok(timeout_secs) = timeout_secs.parse::<u64>() {
                if timeout_secs > 0 {
                    self.storage.provider.config.connect_timeout_secs = Some(timeout_secs);
                }
            }
        }
        // Proxy enabled flag: SYNAPSECLAW_PROXY_ENABLED
        let explicit_proxy_enabled = std::env::var("SYNAPSECLAW_PROXY_ENABLED")
            .ok()
            .as_deref()
            .and_then(parse_proxy_enabled);
        if let Some(enabled) = explicit_proxy_enabled {
            self.proxy.enabled = enabled;
        }

        // Proxy URLs: SYNAPSECLAW_* wins, then generic *PROXY vars.
        let mut proxy_url_overridden = false;
        if let Ok(proxy_url) =
            std::env::var("SYNAPSECLAW_HTTP_PROXY").or_else(|_| std::env::var("HTTP_PROXY"))
        {
            self.proxy.http_proxy = normalize_proxy_url_option(Some(&proxy_url));
            proxy_url_overridden = true;
        }
        if let Ok(proxy_url) =
            std::env::var("SYNAPSECLAW_HTTPS_PROXY").or_else(|_| std::env::var("HTTPS_PROXY"))
        {
            self.proxy.https_proxy = normalize_proxy_url_option(Some(&proxy_url));
            proxy_url_overridden = true;
        }
        if let Ok(proxy_url) =
            std::env::var("SYNAPSECLAW_ALL_PROXY").or_else(|_| std::env::var("ALL_PROXY"))
        {
            self.proxy.all_proxy = normalize_proxy_url_option(Some(&proxy_url));
            proxy_url_overridden = true;
        }
        if let Ok(no_proxy) =
            std::env::var("SYNAPSECLAW_NO_PROXY").or_else(|_| std::env::var("NO_PROXY"))
        {
            self.proxy.no_proxy = normalize_no_proxy_list(vec![no_proxy]);
        }

        if explicit_proxy_enabled.is_none()
            && proxy_url_overridden
            && self.proxy.has_any_proxy_url()
        {
            self.proxy.enabled = true;
        }

        // Proxy scope and service selectors.
        if let Ok(scope_raw) = std::env::var("SYNAPSECLAW_PROXY_SCOPE") {
            if let Some(scope) = parse_proxy_scope(&scope_raw) {
                self.proxy.scope = scope;
            } else {
                tracing::warn!(
                    scope = %scope_raw,
                    "Ignoring invalid SYNAPSECLAW_PROXY_SCOPE (valid: environment|synapseclaw|services)"
                );
            }
        }

        if let Ok(services_raw) = std::env::var("SYNAPSECLAW_PROXY_SERVICES") {
            self.proxy.services = normalize_service_list(vec![services_raw]);
        }

        // ── Phase 3A: Ephemeral agent IPC bootstrap ────────────────
        // When launched as a subprocess by agents_spawn, the parent sets
        // SYNAPSECLAW_BROKER_TOKEN + SYNAPSECLAW_AGENT_ID + SYNAPSECLAW_SESSION_ID.
        // We override agents_ipc config so the child can use IPC tools
        // (agents_reply, state_set, etc.) without manual config.
        if let Ok(broker_token) = std::env::var("SYNAPSECLAW_BROKER_TOKEN") {
            if !broker_token.is_empty() {
                self.agents_ipc.enabled = true;
                self.agents_ipc.broker_token = Some(broker_token);

                if let Ok(broker_url) = std::env::var("SYNAPSECLAW_BROKER_URL") {
                    if !broker_url.is_empty() {
                        self.agents_ipc.broker_url = broker_url;
                    }
                }

                if let Ok(agent_id) = std::env::var("SYNAPSECLAW_AGENT_ID") {
                    if !agent_id.is_empty() {
                        self.agents_ipc.agent_id = Some(agent_id.clone());
                        tracing::info!(agent_id = agent_id, "IPC bootstrap: agent_id set from env");
                    }
                }

                tracing::info!("IPC bootstrap: enabled via SYNAPSECLAW_BROKER_TOKEN env var");
            }
        }

        // Phase 3A: Autonomy override for ephemeral agents.
        // Parent passes the execution boundary's autonomy level via env.
        // Child clamps its autonomy to at most this level (can only restrict,
        // never elevate).
        if let Ok(autonomy_str) = std::env::var("SYNAPSECLAW_AUTONOMY") {
            use synapse_security::AutonomyLevel;
            let target = match autonomy_str.trim().to_ascii_lowercase().as_str() {
                "read_only" | "readonly" => Some(AutonomyLevel::ReadOnly),
                "supervised" => Some(AutonomyLevel::Supervised),
                "full" => Some(AutonomyLevel::Full),
                _ => {
                    tracing::warn!(
                        "Ignoring invalid SYNAPSECLAW_AUTONOMY={autonomy_str:?} \
                         (valid: read_only|supervised|full)"
                    );
                    None
                }
            };
            if let Some(target) = target {
                // Clamp: only restrict, never elevate
                let current = &self.autonomy.level;
                let should_clamp = matches!(
                    (current, &target),
                    (
                        AutonomyLevel::Full,
                        AutonomyLevel::Supervised | AutonomyLevel::ReadOnly
                    ) | (AutonomyLevel::Supervised, AutonomyLevel::ReadOnly)
                );
                if should_clamp {
                    tracing::info!(
                        from = ?self.autonomy.level,
                        to = ?target,
                        "IPC bootstrap: clamping autonomy level"
                    );
                    self.autonomy.level = target;
                }
            }
        }

        if let Err(error) = self.proxy.validate() {
            tracing::warn!("Invalid proxy configuration ignored: {error}");
            self.proxy.enabled = false;
        }

        if self.proxy.enabled && self.proxy.scope == ProxyScope::Environment {
            self.proxy.apply_to_process_env();
        }

        set_runtime_proxy_config(self.proxy.clone());
    }

    async fn save(&self) -> Result<()> {
        // Encrypt secrets before serialization
        let mut config_to_save = self.clone();
        let config_path = resolve_config_path_for_save(self).await?;
        let synapseclaw_dir = config_path
            .parent()
            .context("Config path must have a parent directory")?;
        let store = synapse_security::SecretStore::new(synapseclaw_dir, self.secrets.encrypt);

        encrypt_optional_secret(&store, &mut config_to_save.api_key, "config.api_key")?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.composio.api_key,
            "config.composio.api_key",
        )?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.microsoft365.client_secret,
            "config.microsoft365.client_secret",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.browser.computer_use.api_key,
            "config.browser.computer_use.api_key",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.web_search.brave_api_key,
            "config.web_search.brave_api_key",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.web_search.tavily_api_key,
            "config.web_search.tavily_api_key",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.storage.provider.config.db_url,
            "config.storage.provider.config.db_url",
        )?;

        for agent in config_to_save.agents.values_mut() {
            encrypt_optional_secret(&store, &mut agent.api_key, "config.agents.*.api_key")?;
        }

        encrypt_optional_secret(
            &store,
            &mut config_to_save.agents_ipc.broker_token,
            "config.agents_ipc.broker_token",
        )?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.agents_ipc.proxy_token,
            "config.agents_ipc.proxy_token",
        )?;

        // Encrypt TTS provider API keys
        if let Some(ref mut openai) = config_to_save.tts.openai {
            encrypt_optional_secret(&store, &mut openai.api_key, "config.tts.openai.api_key")?;
        }
        if let Some(ref mut elevenlabs) = config_to_save.tts.elevenlabs {
            encrypt_optional_secret(
                &store,
                &mut elevenlabs.api_key,
                "config.tts.elevenlabs.api_key",
            )?;
        }
        if let Some(ref mut google) = config_to_save.tts.google {
            encrypt_optional_secret(&store, &mut google.api_key, "config.tts.google.api_key")?;
        }

        if let Some(ref mut matrix) = config_to_save.channels_config.matrix {
            encrypt_optional_secret(
                &store,
                &mut matrix.access_token,
                "config.channels_config.matrix.access_token",
            )?;
            encrypt_optional_secret(
                &store,
                &mut matrix.password,
                "config.channels_config.matrix.password",
            )?;
        }

        // Encrypt nested STT provider API keys
        encrypt_optional_secret(
            &store,
            &mut config_to_save.transcription.api_key,
            "config.transcription.api_key",
        )?;
        if let Some(ref mut openai) = config_to_save.transcription.openai {
            encrypt_optional_secret(
                &store,
                &mut openai.api_key,
                "config.transcription.openai.api_key",
            )?;
        }
        if let Some(ref mut deepgram) = config_to_save.transcription.deepgram {
            encrypt_optional_secret(
                &store,
                &mut deepgram.api_key,
                "config.transcription.deepgram.api_key",
            )?;
        }
        if let Some(ref mut assemblyai) = config_to_save.transcription.assemblyai {
            encrypt_optional_secret(
                &store,
                &mut assemblyai.api_key,
                "config.transcription.assemblyai.api_key",
            )?;
        }
        if let Some(ref mut google) = config_to_save.transcription.google {
            encrypt_optional_secret(
                &store,
                &mut google.api_key,
                "config.transcription.google.api_key",
            )?;
        }

        #[cfg(feature = "channel-nostr")]
        if let Some(ref mut ns) = config_to_save.channels_config.nostr {
            encrypt_secret(
                &store,
                &mut ns.private_key,
                "config.channels_config.nostr.private_key",
            )?;
        }
        if let Some(ref mut fs) = config_to_save.channels_config.feishu {
            encrypt_secret(
                &store,
                &mut fs.app_secret,
                "config.channels_config.feishu.app_secret",
            )?;
            encrypt_optional_secret(
                &store,
                &mut fs.encrypt_key,
                "config.channels_config.feishu.encrypt_key",
            )?;
            encrypt_optional_secret(
                &store,
                &mut fs.verification_token,
                "config.channels_config.feishu.verification_token",
            )?;
        }

        // Encrypt channel secrets
        if let Some(ref mut tg) = config_to_save.channels_config.telegram {
            encrypt_secret(
                &store,
                &mut tg.bot_token,
                "config.channels_config.telegram.bot_token",
            )?;
        }
        if let Some(ref mut dc) = config_to_save.channels_config.discord {
            encrypt_secret(
                &store,
                &mut dc.bot_token,
                "config.channels_config.discord.bot_token",
            )?;
        }
        if let Some(ref mut sl) = config_to_save.channels_config.slack {
            encrypt_secret(
                &store,
                &mut sl.bot_token,
                "config.channels_config.slack.bot_token",
            )?;
            encrypt_optional_secret(
                &store,
                &mut sl.app_token,
                "config.channels_config.slack.app_token",
            )?;
        }
        if let Some(ref mut mm) = config_to_save.channels_config.mattermost {
            encrypt_secret(
                &store,
                &mut mm.bot_token,
                "config.channels_config.mattermost.bot_token",
            )?;
        }
        if let Some(ref mut wa) = config_to_save.channels_config.whatsapp {
            encrypt_optional_secret(
                &store,
                &mut wa.access_token,
                "config.channels_config.whatsapp.access_token",
            )?;
            encrypt_optional_secret(
                &store,
                &mut wa.app_secret,
                "config.channels_config.whatsapp.app_secret",
            )?;
            encrypt_optional_secret(
                &store,
                &mut wa.verify_token,
                "config.channels_config.whatsapp.verify_token",
            )?;
        }
        if let Some(ref mut lq) = config_to_save.channels_config.linq {
            encrypt_secret(
                &store,
                &mut lq.api_token,
                "config.channels_config.linq.api_token",
            )?;
            encrypt_optional_secret(
                &store,
                &mut lq.signing_secret,
                "config.channels_config.linq.signing_secret",
            )?;
        }
        if let Some(ref mut wt) = config_to_save.channels_config.wati {
            encrypt_secret(
                &store,
                &mut wt.api_token,
                "config.channels_config.wati.api_token",
            )?;
        }
        if let Some(ref mut nc) = config_to_save.channels_config.nextcloud_talk {
            encrypt_secret(
                &store,
                &mut nc.app_token,
                "config.channels_config.nextcloud_talk.app_token",
            )?;
            encrypt_optional_secret(
                &store,
                &mut nc.webhook_secret,
                "config.channels_config.nextcloud_talk.webhook_secret",
            )?;
        }
        if let Some(ref mut em) = config_to_save.channels_config.email {
            encrypt_secret(
                &store,
                &mut em.password,
                "config.channels_config.email.password",
            )?;
        }
        if let Some(ref mut irc) = config_to_save.channels_config.irc {
            encrypt_optional_secret(
                &store,
                &mut irc.server_password,
                "config.channels_config.irc.server_password",
            )?;
            encrypt_optional_secret(
                &store,
                &mut irc.nickserv_password,
                "config.channels_config.irc.nickserv_password",
            )?;
            encrypt_optional_secret(
                &store,
                &mut irc.sasl_password,
                "config.channels_config.irc.sasl_password",
            )?;
        }
        if let Some(ref mut lk) = config_to_save.channels_config.lark {
            encrypt_secret(
                &store,
                &mut lk.app_secret,
                "config.channels_config.lark.app_secret",
            )?;
            encrypt_optional_secret(
                &store,
                &mut lk.encrypt_key,
                "config.channels_config.lark.encrypt_key",
            )?;
            encrypt_optional_secret(
                &store,
                &mut lk.verification_token,
                "config.channels_config.lark.verification_token",
            )?;
        }
        if let Some(ref mut fs) = config_to_save.channels_config.feishu {
            encrypt_secret(
                &store,
                &mut fs.app_secret,
                "config.channels_config.feishu.app_secret",
            )?;
            encrypt_optional_secret(
                &store,
                &mut fs.encrypt_key,
                "config.channels_config.feishu.encrypt_key",
            )?;
            encrypt_optional_secret(
                &store,
                &mut fs.verification_token,
                "config.channels_config.feishu.verification_token",
            )?;
        }
        if let Some(ref mut dt) = config_to_save.channels_config.dingtalk {
            encrypt_secret(
                &store,
                &mut dt.client_secret,
                "config.channels_config.dingtalk.client_secret",
            )?;
        }
        if let Some(ref mut wc) = config_to_save.channels_config.wecom {
            encrypt_secret(
                &store,
                &mut wc.webhook_key,
                "config.channels_config.wecom.webhook_key",
            )?;
        }
        if let Some(ref mut qq) = config_to_save.channels_config.qq {
            encrypt_secret(
                &store,
                &mut qq.app_secret,
                "config.channels_config.qq.app_secret",
            )?;
        }
        if let Some(ref mut wh) = config_to_save.channels_config.webhook {
            encrypt_optional_secret(
                &store,
                &mut wh.secret,
                "config.channels_config.webhook.secret",
            )?;
        }
        if let Some(ref mut ct) = config_to_save.channels_config.clawdtalk {
            encrypt_secret(
                &store,
                &mut ct.api_key,
                "config.channels_config.clawdtalk.api_key",
            )?;
            encrypt_optional_secret(
                &store,
                &mut ct.webhook_secret,
                "config.channels_config.clawdtalk.webhook_secret",
            )?;
        }

        // Encrypt gateway paired tokens
        for token in &mut config_to_save.gateway.paired_tokens {
            encrypt_secret(&store, token, "config.gateway.paired_tokens[]")?;
        }

        // Encrypt Nevis IAM secret
        encrypt_optional_secret(
            &store,
            &mut config_to_save.security.nevis.client_secret,
            "config.security.nevis.client_secret",
        )?;

        // Notion API key (top-level, not in ChannelsConfig)
        if !config_to_save.notion.api_key.is_empty() {
            encrypt_secret(
                &store,
                &mut config_to_save.notion.api_key,
                "config.notion.api_key",
            )?;
        }

        let toml_str =
            toml::to_string_pretty(&config_to_save).context("Failed to serialize config")?;

        let parent_dir = config_path
            .parent()
            .context("Config path must have a parent directory")?;

        fs::create_dir_all(parent_dir).await.with_context(|| {
            format!(
                "Failed to create config directory: {}",
                parent_dir.display()
            )
        })?;

        let file_name = config_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("config.toml");
        let temp_path = parent_dir.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));
        let backup_path = parent_dir.join(format!("{file_name}.bak"));

        let mut temp_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to create temporary config file: {}",
                    temp_path.display()
                )
            })?;
        temp_file
            .write_all(toml_str.as_bytes())
            .await
            .context("Failed to write temporary config contents")?;
        temp_file
            .sync_all()
            .await
            .context("Failed to fsync temporary config file")?;
        drop(temp_file);

        let had_existing_config = config_path.exists();
        if had_existing_config {
            fs::copy(&config_path, &backup_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create config backup before atomic replace: {}",
                        backup_path.display()
                    )
                })?;
        }

        if let Err(e) = fs::rename(&temp_path, &config_path).await {
            let _ = fs::remove_file(&temp_path).await;
            if had_existing_config && backup_path.exists() {
                fs::copy(&backup_path, &config_path)
                    .await
                    .context("Failed to restore config backup")?;
            }
            anyhow::bail!("Failed to atomically replace config file: {e}");
        }

        #[cfg(unix)]
        {
            use std::{fs::Permissions, os::unix::fs::PermissionsExt};
            if let Err(err) = fs::set_permissions(&config_path, Permissions::from_mode(0o600)).await
            {
                tracing::warn!(
                    "Failed to harden config permissions to 0600 at {}: {}",
                    config_path.display(),
                    err
                );
            }
        }

        sync_directory(parent_dir).await?;

        if had_existing_config {
            let _ = fs::remove_file(&backup_path).await;
        }

        Ok(())
    }
}

#[allow(clippy::unused_async)] // async needed on unix for tokio File I/O; no-op on other platforms
async fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let dir = File::open(path)
            .await
            .with_context(|| format!("Failed to open directory for fsync: {}", path.display()))?;
        dir.sync_all()
            .await
            .with_context(|| format!("Failed to fsync directory metadata: {}", path.display()))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use synapse_core::config::schema::{
        clear_runtime_proxy_client_cache, default_draft_update_interval_ms,
        default_session_backend, runtime_proxy_cache_key, runtime_proxy_client_cache,
        MCP_MAX_TOOL_TIMEOUT_SECS,
    };
    use synapse_core::domain::config::AutonomyLevel;
    #[cfg(unix)]
    use tempfile::TempDir;
    use tokio::sync::{Mutex, MutexGuard};
    use tokio::test;
    use tokio_stream::wrappers::ReadDirStream;
    use tokio_stream::StreamExt;

    // ── Defaults ─────────────────────────────────────────────

    #[test]
    async fn http_request_config_default_has_correct_values() {
        let cfg = HttpRequestConfig::default();
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_response_size, 1_000_000);
        assert!(!cfg.enabled);
        assert!(cfg.allowed_domains.is_empty());
    }

    #[test]
    async fn config_default_has_sane_values() {
        let c = Config::default();
        assert_eq!(c.default_provider.as_deref(), Some("openrouter"));
        assert!(c.default_model.as_deref().unwrap().contains("claude"));
        assert!((c.default_temperature - 0.7).abs() < f64::EPSILON);
        assert!(c.api_key.is_none());
        assert!(!c.skills.open_skills_enabled);
        assert_eq!(
            c.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Full
        );
        assert_eq!(c.provider_timeout_secs, 120);
        assert!(c.workspace_dir.to_string_lossy().contains("workspace"));
        assert!(c.config_path.to_string_lossy().contains("config.toml"));
    }

    #[test]
    async fn config_dir_creation_error_mentions_openrc_and_path() {
        let msg = config_dir_creation_error(Path::new("/etc/synapseclaw"));
        assert!(msg.contains("/etc/synapseclaw"));
        assert!(msg.contains("OpenRC"));
        assert!(msg.contains("synapseclaw"));
    }

    #[test]
    async fn config_schema_export_contains_expected_contract_shape() {
        let schema = schemars::schema_for!(Config);
        let schema_json = serde_json::to_value(&schema).expect("schema should serialize to json");

        assert_eq!(
            schema_json
                .get("$schema")
                .and_then(serde_json::Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );

        let properties = schema_json
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("schema should expose top-level properties");

        assert!(properties.contains_key("default_provider"));
        assert!(properties.contains_key("skills"));
        assert!(properties.contains_key("gateway"));
        assert!(properties.contains_key("channels_config"));
        assert!(!properties.contains_key("workspace_dir"));
        assert!(!properties.contains_key("config_path"));

        assert!(
            schema_json
                .get("$defs")
                .and_then(serde_json::Value::as_object)
                .is_some(),
            "schema should include reusable type definitions"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn save_sets_config_permissions_on_new_file() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        let workspace_dir = temp.path().join("workspace");

        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.workspace_dir = workspace_dir;

        config.save().await.expect("save config");

        let mode = std::fs::metadata(&config_path)
            .expect("config metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    async fn observability_config_default() {
        let o = ObservabilityConfig::default();
        assert_eq!(o.backend, "none");
        assert_eq!(o.runtime_trace_mode, "none");
        assert_eq!(o.runtime_trace_path, "state/runtime-trace.jsonl");
        assert_eq!(o.runtime_trace_max_entries, 200);
    }

    #[test]
    async fn autonomy_config_default() {
        let a = AutonomyConfig::default();
        assert_eq!(a.level, AutonomyLevel::Supervised);
        assert!(a.workspace_only);
        assert!(a.allowed_commands.contains(&"git".to_string()));
        assert!(a.allowed_commands.contains(&"cargo".to_string()));
        assert!(a.forbidden_paths.contains(&"/etc".to_string()));
        assert_eq!(a.max_actions_per_hour, 20);
        assert_eq!(a.max_cost_per_day_cents, 500);
        assert!(a.require_approval_for_medium_risk);
        assert!(a.block_high_risk_commands);
        assert!(a.shell_env_passthrough.is_empty());
    }

    #[test]
    async fn runtime_config_default() {
        let r = RuntimeConfig::default();
        assert_eq!(r.kind, "native");
        assert_eq!(r.docker.image, "alpine:3.20");
        assert_eq!(r.docker.network, "none");
        assert_eq!(r.docker.memory_limit_mb, Some(512));
        assert_eq!(r.docker.cpu_limit, Some(1.0));
        assert!(r.docker.read_only_rootfs);
        assert!(r.docker.mount_workspace);
    }

    #[test]
    async fn heartbeat_config_default() {
        let h = HeartbeatConfig::default();
        assert!(!h.enabled);
        assert_eq!(h.interval_minutes, 30);
        assert!(h.message.is_none());
        assert!(h.target.is_none());
        assert!(h.to.is_none());
    }

    #[test]
    async fn heartbeat_config_parses_delivery_aliases() {
        let raw = r#"
enabled = true
interval_minutes = 10
message = "Ping"
channel = "telegram"
recipient = "42"
"#;
        let parsed: HeartbeatConfig = toml::from_str(raw).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.interval_minutes, 10);
        assert_eq!(parsed.message.as_deref(), Some("Ping"));
        assert_eq!(parsed.target.as_deref(), Some("telegram"));
        assert_eq!(parsed.to.as_deref(), Some("42"));
    }

    #[test]
    async fn cron_config_default() {
        let c = CronConfig::default();
        assert!(c.enabled);
        assert_eq!(c.max_run_history, 50);
    }

    #[test]
    async fn cron_config_serde_roundtrip() {
        let c = CronConfig {
            enabled: false,
            max_run_history: 100,
        };
        let json = serde_json::to_string(&c).unwrap();
        let parsed: CronConfig = serde_json::from_str(&json).unwrap();
        assert!(!parsed.enabled);
        assert_eq!(parsed.max_run_history, 100);
    }

    #[test]
    async fn config_defaults_cron_when_section_missing() {
        let toml_str = r#"
workspace_dir = "/tmp/workspace"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;

        let parsed: Config = toml::from_str(toml_str).unwrap();
        assert!(parsed.cron.enabled);
        assert_eq!(parsed.cron.max_run_history, 50);
    }

    #[test]
    async fn memory_config_default_hygiene_settings() {
        let m = MemoryConfig::default();
        assert_eq!(m.backend, "sqlite");
        assert!(m.auto_save);
        assert!(m.hygiene_enabled);
        assert_eq!(m.archive_after_days, 7);
        assert_eq!(m.purge_after_days, 30);
        assert_eq!(m.conversation_retention_days, 30);
        assert!(m.sqlite_open_timeout_secs.is_none());
    }

    #[test]
    async fn storage_provider_config_defaults() {
        let storage = StorageConfig::default();
        assert!(storage.provider.config.provider.is_empty());
        assert!(storage.provider.config.db_url.is_none());
        assert_eq!(storage.provider.config.schema, "public");
        assert_eq!(storage.provider.config.table, "memories");
        assert!(storage.provider.config.connect_timeout_secs.is_none());
    }

    #[test]
    async fn channels_config_default() {
        let c = ChannelsConfig::default();
        assert!(c.cli);
        assert!(c.telegram.is_none());
        assert!(c.discord.is_none());
        assert!(!c.show_tool_calls);
    }

    // ── Serde round-trip ─────────────────────────────────────

    #[test]
    async fn config_toml_roundtrip() {
        let config = Config {
            workspace_dir: PathBuf::from("/tmp/test/workspace"),
            config_path: PathBuf::from("/tmp/test/config.toml"),
            api_key: Some("sk-test-key".into()),
            api_url: None,
            api_path: None,
            default_provider: Some("openrouter".into()),
            default_model: Some("gpt-4o".into()),
            summary_model: None,
            model_providers: HashMap::new(),
            default_temperature: 0.5,
            provider_timeout_secs: 120,
            extra_headers: HashMap::new(),
            observability: ObservabilityConfig {
                backend: "log".into(),
                ..ObservabilityConfig::default()
            },
            autonomy: AutonomyConfig {
                level: AutonomyLevel::Full,
                workspace_only: false,
                allowed_commands: vec!["docker".into()],
                forbidden_paths: vec!["/secret".into()],
                max_actions_per_hour: 50,
                max_cost_per_day_cents: 1000,
                require_approval_for_medium_risk: false,
                block_high_risk_commands: true,
                shell_env_passthrough: vec!["DATABASE_URL".into()],
                auto_approve: vec!["file_read".into()],
                always_ask: vec![],
                allowed_roots: vec![],
                non_cli_excluded_tools: vec![],
            },
            backup: BackupConfig::default(),
            data_retention: DataRetentionConfig::default(),
            cloud_ops: CloudOpsConfig::default(),
            conversational_ai: ConversationalAiConfig::default(),
            security: SecurityConfig::default(),
            security_ops: SecurityOpsConfig::default(),
            runtime: RuntimeConfig {
                kind: "docker".into(),
                ..RuntimeConfig::default()
            },
            reliability: ReliabilityConfig::default(),
            scheduler: SchedulerConfig::default(),
            skills: SkillsConfig::default(),
            model_routes: Vec::new(),
            embedding_routes: Vec::new(),
            query_classification: QueryClassificationConfig::default(),
            heartbeat: HeartbeatConfig {
                enabled: true,
                interval_minutes: 15,
                two_phase: true,
                message: Some("Check London time".into()),
                target: Some("telegram".into()),
                to: Some("123456".into()),
                ..HeartbeatConfig::default()
            },
            cron: CronConfig::default(),
            channels_config: ChannelsConfig {
                cli: true,
                telegram: Some(TelegramConfig {
                    bot_token: "123:ABC".into(),
                    allowed_users: vec!["user1".into()],
                    stream_mode: StreamMode::default(),
                    draft_update_interval_ms: default_draft_update_interval_ms(),
                    interrupt_on_new_message: false,
                    mention_only: false,
                }),
                discord: None,
                slack: None,
                mattermost: None,
                webhook: None,
                imessage: None,
                matrix: None,
                signal: None,
                whatsapp: None,
                linq: None,
                wati: None,
                nextcloud_talk: None,
                email: None,
                irc: None,
                lark: None,
                feishu: None,
                dingtalk: None,
                wecom: None,
                qq: None,
                twitter: None,
                mochat: None,
                #[cfg(feature = "channel-nostr")]
                nostr: None,
                clawdtalk: None,
                reddit: None,
                bluesky: None,
                message_timeout_secs: 300,
                ack_reactions: true,
                show_tool_calls: true,
                session_persistence: true,
                session_backend: default_session_backend(),
                session_ttl_hours: 0,
            },
            memory: MemoryConfig::default(),
            storage: StorageConfig::default(),
            tunnel: TunnelConfig::default(),
            gateway: GatewayConfig::default(),
            composio: ComposioConfig::default(),
            microsoft365: Microsoft365Config::default(),
            secrets: SecretsConfig::default(),
            browser: BrowserConfig::default(),
            browser_delegate: synapse_core::config::adapter_configs::BrowserDelegateConfig::default(
            ),
            http_request: HttpRequestConfig::default(),
            multimodal: MultimodalConfig::default(),
            web_fetch: WebFetchConfig::default(),
            web_search: WebSearchConfig::default(),
            project_intel: ProjectIntelConfig::default(),
            google_workspace: GoogleWorkspaceConfig::default(),
            proxy: ProxyConfig::default(),
            agent: AgentConfig::default(),
            identity: IdentityConfig::default(),
            cost: CostConfig::default(),

            agents: HashMap::new(),
            swarms: HashMap::new(),
            hooks: HooksConfig::default(),

            transcription: TranscriptionConfig::default(),
            tts: TtsConfig::default(),
            mcp: McpConfig::default(),
            agents_ipc: AgentsIpcConfig::default(),
            pipelines: PipelineEngineConfig::default(),
            nodes: NodesConfig::default(),
            workspace: WorkspaceConfig::default(),
            notion: NotionConfig::default(),
            node_transport: NodeTransportConfig::default(),
            knowledge: KnowledgeConfig::default(),
            linkedin: LinkedInConfig::default(),
            summary: SummaryConfig::default(),
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.api_key, config.api_key);
        assert_eq!(parsed.default_provider, config.default_provider);
        assert_eq!(parsed.default_model, config.default_model);
        assert!((parsed.default_temperature - config.default_temperature).abs() < f64::EPSILON);
        assert_eq!(parsed.observability.backend, "log");
        assert_eq!(parsed.observability.runtime_trace_mode, "none");
        assert_eq!(parsed.autonomy.level, AutonomyLevel::Full);
        assert!(!parsed.autonomy.workspace_only);
        assert_eq!(parsed.runtime.kind, "docker");
        assert!(parsed.heartbeat.enabled);
        assert_eq!(parsed.heartbeat.interval_minutes, 15);
        assert_eq!(
            parsed.heartbeat.message.as_deref(),
            Some("Check London time")
        );
        assert_eq!(parsed.heartbeat.target.as_deref(), Some("telegram"));
        assert_eq!(parsed.heartbeat.to.as_deref(), Some("123456"));
        assert!(parsed.channels_config.telegram.is_some());
        assert_eq!(
            parsed.channels_config.telegram.unwrap().bot_token,
            "123:ABC"
        );
    }

    #[test]
    async fn config_minimal_toml_uses_defaults() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(parsed.api_key.is_none());
        assert!(parsed.default_provider.is_none());
        assert_eq!(parsed.observability.backend, "none");
        assert_eq!(parsed.observability.runtime_trace_mode, "none");
        assert_eq!(parsed.autonomy.level, AutonomyLevel::Supervised);
        assert_eq!(parsed.runtime.kind, "native");
        assert!(!parsed.heartbeat.enabled);
        assert!(parsed.channels_config.cli);
        assert!(parsed.memory.hygiene_enabled);
        assert_eq!(parsed.memory.archive_after_days, 7);
        assert_eq!(parsed.memory.purge_after_days, 30);
        assert_eq!(parsed.memory.conversation_retention_days, 30);
        // provider_timeout_secs defaults to 120 when not specified
        assert_eq!(parsed.provider_timeout_secs, 120);
    }

    #[test]
    async fn provider_timeout_secs_parses_from_toml() {
        let raw = r#"
default_temperature = 0.7
provider_timeout_secs = 300
"#;
        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.provider_timeout_secs, 300);
    }

    #[test]
    async fn parse_extra_headers_env_basic() {
        let headers = parse_extra_headers_env("User-Agent:MyApp/1.0,X-Title:synapseclaw");
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers[0],
            ("User-Agent".to_string(), "MyApp/1.0".to_string())
        );
        assert_eq!(
            headers[1],
            ("X-Title".to_string(), "synapseclaw".to_string())
        );
    }

    #[test]
    async fn parse_extra_headers_env_with_url_value() {
        let headers =
            parse_extra_headers_env("HTTP-Referer:https://github.com/panviktor/synapseclaw");
        assert_eq!(headers.len(), 1);
        // Only splits on first colon, preserving URL colons in value
        assert_eq!(headers[0].0, "HTTP-Referer");
        assert_eq!(headers[0].1, "https://github.com/panviktor/synapseclaw");
    }

    #[test]
    async fn parse_extra_headers_env_empty_string() {
        let headers = parse_extra_headers_env("");
        assert!(headers.is_empty());
    }

    #[test]
    async fn parse_extra_headers_env_whitespace_trimming() {
        let headers = parse_extra_headers_env("  X-Title : synapseclaw , User-Agent : cli/1.0 ");
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers[0],
            ("X-Title".to_string(), "synapseclaw".to_string())
        );
        assert_eq!(
            headers[1],
            ("User-Agent".to_string(), "cli/1.0".to_string())
        );
    }

    #[test]
    async fn parse_extra_headers_env_skips_malformed() {
        let headers = parse_extra_headers_env("X-Valid:value,no-colon-here,Another:ok");
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0], ("X-Valid".to_string(), "value".to_string()));
        assert_eq!(headers[1], ("Another".to_string(), "ok".to_string()));
    }

    #[test]
    async fn parse_extra_headers_env_skips_empty_key() {
        let headers = parse_extra_headers_env(":value,X-Valid:ok");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], ("X-Valid".to_string(), "ok".to_string()));
    }

    #[test]
    async fn parse_extra_headers_env_allows_empty_value() {
        let headers = parse_extra_headers_env("X-Empty:");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], ("X-Empty".to_string(), String::new()));
    }

    #[test]
    async fn parse_extra_headers_env_trailing_comma() {
        let headers = parse_extra_headers_env("X-Title:synapseclaw,");
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers[0],
            ("X-Title".to_string(), "synapseclaw".to_string())
        );
    }

    #[test]
    async fn extra_headers_parses_from_toml() {
        let raw = r#"
default_temperature = 0.7

[extra_headers]
User-Agent = "MyApp/1.0"
X-Title = "synapseclaw"
"#;
        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.extra_headers.len(), 2);
        assert_eq!(parsed.extra_headers.get("User-Agent").unwrap(), "MyApp/1.0");
        assert_eq!(parsed.extra_headers.get("X-Title").unwrap(), "synapseclaw");
    }

    #[test]
    async fn extra_headers_defaults_to_empty() {
        let raw = r#"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(raw).unwrap();
        assert!(parsed.extra_headers.is_empty());
    }

    #[test]
    async fn storage_provider_dburl_alias_deserializes() {
        let raw = r#"
default_temperature = 0.7

[storage.provider.config]
provider = "postgres"
dbURL = "postgres://postgres:postgres@localhost:5432/synapseclaw"
schema = "public"
table = "memories"
connect_timeout_secs = 12
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.storage.provider.config.provider, "postgres");
        assert_eq!(
            parsed.storage.provider.config.db_url.as_deref(),
            Some("postgres://postgres:postgres@localhost:5432/synapseclaw")
        );
        assert_eq!(parsed.storage.provider.config.schema, "public");
        assert_eq!(parsed.storage.provider.config.table, "memories");
        assert_eq!(
            parsed.storage.provider.config.connect_timeout_secs,
            Some(12)
        );
    }

    #[test]
    async fn runtime_reasoning_enabled_deserializes() {
        let raw = r#"
default_temperature = 0.7

[runtime]
reasoning_enabled = false
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.runtime.reasoning_enabled, Some(false));
    }

    #[test]
    async fn runtime_reasoning_effort_deserializes() {
        let raw = r#"
default_temperature = 0.7

[runtime]
reasoning_effort = "HIGH"
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.runtime.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    async fn runtime_reasoning_effort_rejects_invalid_values() {
        let raw = r#"
default_temperature = 0.7

[runtime]
reasoning_effort = "turbo"
"#;

        let error = toml::from_str::<Config>(raw).expect_err("invalid value should fail");
        assert!(error.to_string().contains("reasoning_effort"));
    }

    #[test]
    async fn agent_config_defaults() {
        let cfg = AgentConfig::default();
        assert!(!cfg.compact_context);
        assert_eq!(cfg.max_tool_iterations, 10);
        assert_eq!(cfg.max_history_messages, 50);
        assert!(!cfg.parallel_tools);
        assert_eq!(cfg.tool_dispatcher, "auto");
    }

    #[test]
    async fn agent_config_deserializes() {
        let raw = r#"
default_temperature = 0.7
[agent]
compact_context = true
max_tool_iterations = 20
max_history_messages = 80
parallel_tools = true
tool_dispatcher = "xml"
"#;
        let parsed: Config = toml::from_str(raw).unwrap();
        assert!(parsed.agent.compact_context);
        assert_eq!(parsed.agent.max_tool_iterations, 20);
        assert_eq!(parsed.agent.max_history_messages, 80);
        assert!(parsed.agent.parallel_tools);
        assert_eq!(parsed.agent.tool_dispatcher, "xml");
    }

    #[tokio::test]
    async fn sync_directory_handles_existing_directory() {
        let dir = std::env::temp_dir().join(format!(
            "synapseclaw_test_sync_directory_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).await.unwrap();

        sync_directory(&dir).await.unwrap();

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn config_save_and_load_tmpdir() {
        let dir = std::env::temp_dir().join("synapseclaw_test_config");
        let _ = fs::remove_dir_all(&dir).await;
        fs::create_dir_all(&dir).await.unwrap();

        let config_path = dir.join("config.toml");
        let config = Config {
            workspace_dir: dir.join("workspace"),
            config_path: config_path.clone(),
            api_key: Some("sk-roundtrip".into()),
            api_url: None,
            api_path: None,
            default_provider: Some("openrouter".into()),
            default_model: Some("test-model".into()),
            summary_model: None,
            model_providers: HashMap::new(),
            default_temperature: 0.9,
            provider_timeout_secs: 120,
            extra_headers: HashMap::new(),
            observability: ObservabilityConfig::default(),
            autonomy: AutonomyConfig::default(),
            backup: BackupConfig::default(),
            data_retention: DataRetentionConfig::default(),
            cloud_ops: CloudOpsConfig::default(),
            conversational_ai: ConversationalAiConfig::default(),
            security: SecurityConfig::default(),
            security_ops: SecurityOpsConfig::default(),
            runtime: RuntimeConfig::default(),
            reliability: ReliabilityConfig::default(),
            scheduler: SchedulerConfig::default(),
            skills: SkillsConfig::default(),
            model_routes: Vec::new(),
            embedding_routes: Vec::new(),
            query_classification: QueryClassificationConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            cron: CronConfig::default(),
            channels_config: ChannelsConfig::default(),
            memory: MemoryConfig::default(),
            storage: StorageConfig::default(),
            tunnel: TunnelConfig::default(),
            gateway: GatewayConfig::default(),
            composio: ComposioConfig::default(),
            microsoft365: Microsoft365Config::default(),
            secrets: SecretsConfig::default(),
            browser: BrowserConfig::default(),
            browser_delegate: synapse_core::config::adapter_configs::BrowserDelegateConfig::default(
            ),
            http_request: HttpRequestConfig::default(),
            multimodal: MultimodalConfig::default(),
            web_fetch: WebFetchConfig::default(),
            web_search: WebSearchConfig::default(),
            project_intel: ProjectIntelConfig::default(),
            google_workspace: GoogleWorkspaceConfig::default(),
            proxy: ProxyConfig::default(),
            agent: AgentConfig::default(),
            identity: IdentityConfig::default(),
            cost: CostConfig::default(),

            agents: HashMap::new(),
            swarms: HashMap::new(),
            hooks: HooksConfig::default(),

            transcription: TranscriptionConfig::default(),
            tts: TtsConfig::default(),
            mcp: McpConfig::default(),
            agents_ipc: AgentsIpcConfig::default(),
            pipelines: PipelineEngineConfig::default(),
            nodes: NodesConfig::default(),
            workspace: WorkspaceConfig::default(),
            notion: NotionConfig::default(),
            node_transport: NodeTransportConfig::default(),
            knowledge: KnowledgeConfig::default(),
            linkedin: LinkedInConfig::default(),
            summary: SummaryConfig::default(),
        };

        config.save().await.unwrap();
        assert!(config_path.exists());

        let contents = tokio::fs::read_to_string(&config_path).await.unwrap();
        let loaded: Config = toml::from_str(&contents).unwrap();
        assert!(loaded
            .api_key
            .as_deref()
            .is_some_and(synapse_security::SecretStore::is_encrypted));
        let store = synapse_security::SecretStore::new(&dir, true);
        let decrypted = store.decrypt(loaded.api_key.as_deref().unwrap()).unwrap();
        assert_eq!(decrypted, "sk-roundtrip");
        assert_eq!(loaded.default_model.as_deref(), Some("test-model"));
        assert!((loaded.default_temperature - 0.9).abs() < f64::EPSILON);

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn config_save_encrypts_nested_credentials() {
        let dir = std::env::temp_dir().join(format!(
            "synapseclaw_test_nested_credentials_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).await.unwrap();

        let mut config = Config::default();
        config.workspace_dir = dir.join("workspace");
        config.config_path = dir.join("config.toml");
        config.api_key = Some("root-credential".into());
        config.composio.api_key = Some("composio-credential".into());
        config.browser.computer_use.api_key = Some("browser-credential".into());
        config.web_search.brave_api_key = Some("brave-credential".into());
        config.storage.provider.config.db_url = Some("postgres://user:pw@host/db".into());
        config.channels_config.feishu = Some(FeishuConfig {
            app_id: "cli_feishu_123".into(),
            app_secret: "feishu-secret".into(),
            encrypt_key: Some("feishu-encrypt".into()),
            verification_token: Some("feishu-verify".into()),
            allowed_users: vec!["*".into()],
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
        });

        config.agents.insert(
            "worker".into(),
            DelegateAgentConfig {
                provider: "openrouter".into(),
                model: "model-test".into(),
                system_prompt: None,
                api_key: Some("agent-credential".into()),
                temperature: None,
                max_depth: 3,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: 10,
            },
        );

        config.save().await.unwrap();

        let contents = tokio::fs::read_to_string(config.config_path.clone())
            .await
            .unwrap();
        let stored: Config = toml::from_str(&contents).unwrap();
        let store = synapse_security::SecretStore::new(&dir, true);

        let root_encrypted = stored.api_key.as_deref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(root_encrypted));
        assert_eq!(store.decrypt(root_encrypted).unwrap(), "root-credential");

        let composio_encrypted = stored.composio.api_key.as_deref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(
            composio_encrypted
        ));
        assert_eq!(
            store.decrypt(composio_encrypted).unwrap(),
            "composio-credential"
        );

        let browser_encrypted = stored.browser.computer_use.api_key.as_deref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(
            browser_encrypted
        ));
        assert_eq!(
            store.decrypt(browser_encrypted).unwrap(),
            "browser-credential"
        );

        let web_search_encrypted = stored.web_search.brave_api_key.as_deref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(
            web_search_encrypted
        ));
        assert_eq!(
            store.decrypt(web_search_encrypted).unwrap(),
            "brave-credential"
        );

        let worker = stored.agents.get("worker").unwrap();
        let worker_encrypted = worker.api_key.as_deref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(
            worker_encrypted
        ));
        assert_eq!(store.decrypt(worker_encrypted).unwrap(), "agent-credential");

        let storage_db_url = stored.storage.provider.config.db_url.as_deref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(storage_db_url));
        assert_eq!(
            store.decrypt(storage_db_url).unwrap(),
            "postgres://user:pw@host/db"
        );

        let feishu = stored.channels_config.feishu.as_ref().unwrap();
        assert!(synapse_security::SecretStore::is_encrypted(
            &feishu.app_secret
        ));
        assert_eq!(store.decrypt(&feishu.app_secret).unwrap(), "feishu-secret");
        assert!(feishu
            .encrypt_key
            .as_deref()
            .is_some_and(synapse_security::SecretStore::is_encrypted));
        assert_eq!(
            store
                .decrypt(feishu.encrypt_key.as_deref().unwrap())
                .unwrap(),
            "feishu-encrypt"
        );
        assert!(feishu
            .verification_token
            .as_deref()
            .is_some_and(synapse_security::SecretStore::is_encrypted));
        assert_eq!(
            store
                .decrypt(feishu.verification_token.as_deref().unwrap())
                .unwrap(),
            "feishu-verify"
        );

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn config_save_atomic_cleanup() {
        let dir =
            std::env::temp_dir().join(format!("synapseclaw_test_config_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).await.unwrap();

        let config_path = dir.join("config.toml");
        let mut config = Config::default();
        config.workspace_dir = dir.join("workspace");
        config.config_path = config_path.clone();
        config.default_model = Some("model-a".into());
        config.save().await.unwrap();
        assert!(config_path.exists());

        config.default_model = Some("model-b".into());
        config.save().await.unwrap();

        let contents = tokio::fs::read_to_string(&config_path).await.unwrap();
        assert!(contents.contains("model-b"));

        let names: Vec<String> = ReadDirStream::new(fs::read_dir(&dir).await.unwrap())
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect()
            .await;
        assert!(!names.iter().any(|name| name.contains(".tmp-")));
        assert!(!names.iter().any(|name| name.ends_with(".bak")));

        let _ = fs::remove_dir_all(&dir).await;
    }

    // ── Telegram / Discord config ────────────────────────────

    #[test]
    async fn telegram_config_serde() {
        let tc = TelegramConfig {
            bot_token: "123:XYZ".into(),
            allowed_users: vec!["alice".into(), "bob".into()],
            stream_mode: StreamMode::Partial,
            draft_update_interval_ms: 500,
            interrupt_on_new_message: true,
            mention_only: false,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let parsed: TelegramConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bot_token, "123:XYZ");
        assert_eq!(parsed.allowed_users.len(), 2);
        assert_eq!(parsed.stream_mode, StreamMode::Partial);
        assert_eq!(parsed.draft_update_interval_ms, 500);
        assert!(parsed.interrupt_on_new_message);
    }

    #[test]
    async fn telegram_config_defaults_stream_off() {
        let json = r#"{"bot_token":"tok","allowed_users":[]}"#;
        let parsed: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.stream_mode, StreamMode::Off);
        assert_eq!(parsed.draft_update_interval_ms, 1000);
        assert!(!parsed.interrupt_on_new_message);
    }

    #[test]
    async fn discord_config_serde() {
        let dc = DiscordConfig {
            bot_token: "discord-token".into(),
            guild_id: Some("12345".into()),
            allowed_users: vec![],
            listen_to_bots: false,
            mention_only: false,
        };
        let json = serde_json::to_string(&dc).unwrap();
        let parsed: DiscordConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bot_token, "discord-token");
        assert_eq!(parsed.guild_id.as_deref(), Some("12345"));
    }

    #[test]
    async fn discord_config_optional_guild() {
        let dc = DiscordConfig {
            bot_token: "tok".into(),
            guild_id: None,
            allowed_users: vec![],
            listen_to_bots: false,
            mention_only: false,
        };
        let json = serde_json::to_string(&dc).unwrap();
        let parsed: DiscordConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.guild_id.is_none());
    }

    // ── iMessage / Matrix config ────────────────────────────

    #[test]
    async fn imessage_config_serde() {
        let ic = IMessageConfig {
            allowed_contacts: vec!["+1234567890".into(), "user@icloud.com".into()],
        };
        let json = serde_json::to_string(&ic).unwrap();
        let parsed: IMessageConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allowed_contacts.len(), 2);
        assert_eq!(parsed.allowed_contacts[0], "+1234567890");
    }

    #[test]
    async fn imessage_config_empty_contacts() {
        let ic = IMessageConfig {
            allowed_contacts: vec![],
        };
        let json = serde_json::to_string(&ic).unwrap();
        let parsed: IMessageConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.allowed_contacts.is_empty());
    }

    #[test]
    async fn imessage_config_wildcard() {
        let ic = IMessageConfig {
            allowed_contacts: vec!["*".into()],
        };
        let toml_str = toml::to_string(&ic).unwrap();
        let parsed: IMessageConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.allowed_contacts, vec!["*"]);
    }

    #[test]
    async fn matrix_config_serde() {
        let mc = MatrixConfig {
            homeserver: "https://matrix.org".into(),
            access_token: Some("syt_token_abc".to_string()),
            user_id: Some("@bot:matrix.org".into()),
            device_id: Some("DEVICE123".into()),
            room_id: "!room123:matrix.org".into(),
            allowed_users: vec!["@user:matrix.org".into()],
            password: None,
            max_media_download_mb: None,
        };
        let json = serde_json::to_string(&mc).unwrap();
        let parsed: MatrixConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.homeserver, "https://matrix.org");
        assert_eq!(parsed.access_token.as_deref(), Some("syt_token_abc"));
        assert_eq!(parsed.user_id.as_deref(), Some("@bot:matrix.org"));
        assert_eq!(parsed.device_id.as_deref(), Some("DEVICE123"));
        assert_eq!(parsed.room_id, "!room123:matrix.org");
        assert_eq!(parsed.allowed_users.len(), 1);
    }

    #[test]
    async fn matrix_config_toml_roundtrip() {
        let mc = MatrixConfig {
            homeserver: "https://synapse.local:8448".into(),
            access_token: Some("tok".to_string()),
            user_id: None,
            device_id: None,
            room_id: "!abc:synapse.local".into(),
            allowed_users: vec!["@admin:synapse.local".into(), "*".into()],
            password: None,
            max_media_download_mb: None,
        };
        let toml_str = toml::to_string(&mc).unwrap();
        let parsed: MatrixConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.homeserver, "https://synapse.local:8448");
        assert_eq!(parsed.allowed_users.len(), 2);
    }

    #[test]
    async fn matrix_config_backward_compatible_without_session_hints() {
        let toml = r#"
homeserver = "https://matrix.org"
access_token = "tok"
room_id = "!ops:matrix.org"
allowed_users = ["@ops:matrix.org"]
"#;

        let parsed: MatrixConfig = toml::from_str(toml).unwrap();
        assert_eq!(parsed.homeserver, "https://matrix.org");
        assert!(parsed.user_id.is_none());
        assert!(parsed.device_id.is_none());
    }

    #[test]
    async fn signal_config_serde() {
        let sc = SignalConfig {
            http_url: "http://127.0.0.1:8686".into(),
            account: "+1234567890".into(),
            group_id: Some("group123".into()),
            allowed_from: vec!["+1111111111".into()],
            ignore_attachments: true,
            ignore_stories: false,
        };
        let json = serde_json::to_string(&sc).unwrap();
        let parsed: SignalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.http_url, "http://127.0.0.1:8686");
        assert_eq!(parsed.account, "+1234567890");
        assert_eq!(parsed.group_id.as_deref(), Some("group123"));
        assert_eq!(parsed.allowed_from.len(), 1);
        assert!(parsed.ignore_attachments);
        assert!(!parsed.ignore_stories);
    }

    #[test]
    async fn signal_config_toml_roundtrip() {
        let sc = SignalConfig {
            http_url: "http://localhost:8080".into(),
            account: "+9876543210".into(),
            group_id: None,
            allowed_from: vec!["*".into()],
            ignore_attachments: false,
            ignore_stories: true,
        };
        let toml_str = toml::to_string(&sc).unwrap();
        let parsed: SignalConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.http_url, "http://localhost:8080");
        assert_eq!(parsed.account, "+9876543210");
        assert!(parsed.group_id.is_none());
        assert!(parsed.ignore_stories);
    }

    #[test]
    async fn signal_config_defaults() {
        let json = r#"{"http_url":"http://127.0.0.1:8686","account":"+1234567890"}"#;
        let parsed: SignalConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.group_id.is_none());
        assert!(parsed.allowed_from.is_empty());
        assert!(!parsed.ignore_attachments);
        assert!(!parsed.ignore_stories);
    }

    #[test]
    async fn channels_config_with_imessage_and_matrix() {
        let c = ChannelsConfig {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: Some(IMessageConfig {
                allowed_contacts: vec!["+1".into()],
            }),
            matrix: Some(MatrixConfig {
                homeserver: "https://m.org".into(),
                access_token: Some("tok".to_string()),
                user_id: None,
                device_id: None,
                room_id: "!r:m".into(),
                allowed_users: vec!["@u:m".into()],
                password: None,
                max_media_download_mb: None,
            }),
            signal: None,
            whatsapp: None,
            linq: None,
            wati: None,
            nextcloud_talk: None,
            email: None,
            irc: None,
            lark: None,
            feishu: None,
            dingtalk: None,
            wecom: None,
            qq: None,
            twitter: None,
            mochat: None,
            nostr: None,
            clawdtalk: None,
            reddit: None,
            bluesky: None,
            message_timeout_secs: 300,
            ack_reactions: true,
            show_tool_calls: true,
            session_persistence: true,
            session_backend: default_session_backend(),
            session_ttl_hours: 0,
        };
        let toml_str = toml::to_string_pretty(&c).unwrap();
        let parsed: ChannelsConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.imessage.is_some());
        assert!(parsed.matrix.is_some());
        assert_eq!(parsed.imessage.unwrap().allowed_contacts, vec!["+1"]);
        assert_eq!(parsed.matrix.unwrap().homeserver, "https://m.org");
    }

    #[test]
    async fn channels_config_default_has_no_imessage_matrix() {
        let c = ChannelsConfig::default();
        assert!(c.imessage.is_none());
        assert!(c.matrix.is_none());
    }

    // ── Edge cases: serde(default) for allowed_users ─────────

    #[test]
    async fn discord_config_deserializes_without_allowed_users() {
        // Old configs won't have allowed_users — serde(default) should fill vec![]
        let json = r#"{"bot_token":"tok","guild_id":"123"}"#;
        let parsed: DiscordConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.allowed_users.is_empty());
    }

    #[test]
    async fn discord_config_deserializes_with_allowed_users() {
        let json = r#"{"bot_token":"tok","guild_id":"123","allowed_users":["111","222"]}"#;
        let parsed: DiscordConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.allowed_users, vec!["111", "222"]);
    }

    #[test]
    async fn slack_config_deserializes_without_allowed_users() {
        let json = r#"{"bot_token":"xoxb-tok"}"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.allowed_users.is_empty());
        assert!(!parsed.interrupt_on_new_message);
        assert!(!parsed.mention_only);
    }

    #[test]
    async fn slack_config_deserializes_with_allowed_users() {
        let json = r#"{"bot_token":"xoxb-tok","allowed_users":["U111"]}"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.allowed_users, vec!["U111"]);
        assert!(!parsed.interrupt_on_new_message);
        assert!(!parsed.mention_only);
    }

    #[test]
    async fn slack_config_deserializes_with_mention_only() {
        let json = r#"{"bot_token":"xoxb-tok","mention_only":true}"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.mention_only);
        assert!(!parsed.interrupt_on_new_message);
    }

    #[test]
    async fn slack_config_deserializes_interrupt_on_new_message() {
        let json = r#"{"bot_token":"xoxb-tok","interrupt_on_new_message":true}"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.interrupt_on_new_message);
        assert!(!parsed.mention_only);
    }

    #[test]
    async fn discord_config_toml_backward_compat() {
        let toml_str = r#"
bot_token = "tok"
guild_id = "123"
"#;
        let parsed: DiscordConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.allowed_users.is_empty());
        assert_eq!(parsed.bot_token, "tok");
    }

    #[test]
    async fn slack_config_toml_backward_compat() {
        let toml_str = r#"
bot_token = "xoxb-tok"
channel_id = "C123"
"#;
        let parsed: SlackConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.allowed_users.is_empty());
        assert!(!parsed.interrupt_on_new_message);
        assert!(!parsed.mention_only);
        assert_eq!(parsed.channel_id.as_deref(), Some("C123"));
    }

    #[test]
    async fn webhook_config_with_secret() {
        let json = r#"{"port":8080,"secret":"my-secret-key"}"#;
        let parsed: WebhookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.secret.as_deref(), Some("my-secret-key"));
    }

    #[test]
    async fn webhook_config_without_secret() {
        let json = r#"{"port":8080}"#;
        let parsed: WebhookConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.secret.is_none());
        assert_eq!(parsed.port, 8080);
    }

    // ── WhatsApp config ──────────────────────────────────────

    #[test]
    async fn whatsapp_config_serde() {
        let wc = WhatsAppConfig {
            access_token: Some("EAABx...".into()),
            phone_number_id: Some("123456789".into()),
            verify_token: Some("my-verify-token".into()),
            app_secret: None,
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+1234567890".into(), "+9876543210".into()],
        };
        let json = serde_json::to_string(&wc).unwrap();
        let parsed: WhatsAppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.access_token, Some("EAABx...".into()));
        assert_eq!(parsed.phone_number_id, Some("123456789".into()));
        assert_eq!(parsed.verify_token, Some("my-verify-token".into()));
        assert_eq!(parsed.allowed_numbers.len(), 2);
    }

    #[test]
    async fn whatsapp_config_toml_roundtrip() {
        let wc = WhatsAppConfig {
            access_token: Some("tok".into()),
            phone_number_id: Some("12345".into()),
            verify_token: Some("verify".into()),
            app_secret: Some("secret123".into()),
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+1".into()],
        };
        let toml_str = toml::to_string(&wc).unwrap();
        let parsed: WhatsAppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.phone_number_id, Some("12345".into()));
        assert_eq!(parsed.allowed_numbers, vec!["+1"]);
    }

    #[test]
    async fn whatsapp_config_deserializes_without_allowed_numbers() {
        let json = r#"{"access_token":"tok","phone_number_id":"123","verify_token":"ver"}"#;
        let parsed: WhatsAppConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.allowed_numbers.is_empty());
    }

    #[test]
    async fn whatsapp_config_wildcard_allowed() {
        let wc = WhatsAppConfig {
            access_token: Some("tok".into()),
            phone_number_id: Some("123".into()),
            verify_token: Some("ver".into()),
            app_secret: None,
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["*".into()],
        };
        let toml_str = toml::to_string(&wc).unwrap();
        let parsed: WhatsAppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.allowed_numbers, vec!["*"]);
    }

    #[test]
    async fn whatsapp_config_backend_type_cloud_precedence_when_ambiguous() {
        let wc = WhatsAppConfig {
            access_token: Some("tok".into()),
            phone_number_id: Some("123".into()),
            verify_token: Some("ver".into()),
            app_secret: None,
            session_path: Some("~/.synapseclaw/state/whatsapp-web/session.db".into()),
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+1".into()],
        };
        assert!(wc.is_ambiguous_config());
        assert_eq!(wc.backend_type(), "cloud");
    }

    #[test]
    async fn whatsapp_config_backend_type_web() {
        let wc = WhatsAppConfig {
            access_token: None,
            phone_number_id: None,
            verify_token: None,
            app_secret: None,
            session_path: Some("~/.synapseclaw/state/whatsapp-web/session.db".into()),
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec![],
        };
        assert!(!wc.is_ambiguous_config());
        assert_eq!(wc.backend_type(), "web");
    }

    #[test]
    async fn channels_config_with_whatsapp() {
        let c = ChannelsConfig {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: None,
            matrix: None,
            signal: None,
            whatsapp: Some(WhatsAppConfig {
                access_token: Some("tok".into()),
                phone_number_id: Some("123".into()),
                verify_token: Some("ver".into()),
                app_secret: None,
                session_path: None,
                pair_phone: None,
                pair_code: None,
                allowed_numbers: vec!["+1".into()],
            }),
            linq: None,
            wati: None,
            nextcloud_talk: None,
            email: None,
            irc: None,
            lark: None,
            feishu: None,
            dingtalk: None,
            wecom: None,
            qq: None,
            twitter: None,
            mochat: None,
            nostr: None,
            clawdtalk: None,
            reddit: None,
            bluesky: None,
            message_timeout_secs: 300,
            ack_reactions: true,
            show_tool_calls: true,
            session_persistence: true,
            session_backend: default_session_backend(),
            session_ttl_hours: 0,
        };
        let toml_str = toml::to_string_pretty(&c).unwrap();
        let parsed: ChannelsConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.whatsapp.is_some());
        let wa = parsed.whatsapp.unwrap();
        assert_eq!(wa.phone_number_id, Some("123".into()));
        assert_eq!(wa.allowed_numbers, vec!["+1"]);
    }

    #[test]
    async fn channels_config_default_has_no_whatsapp() {
        let c = ChannelsConfig::default();
        assert!(c.whatsapp.is_none());
    }

    #[test]
    async fn channels_config_default_has_no_nextcloud_talk() {
        let c = ChannelsConfig::default();
        assert!(c.nextcloud_talk.is_none());
    }

    // ══════════════════════════════════════════════════════════
    // SECURITY CHECKLIST TESTS — Gateway config
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn checklist_gateway_default_requires_pairing() {
        let g = GatewayConfig::default();
        assert!(g.require_pairing, "Pairing must be required by default");
    }

    #[test]
    async fn checklist_gateway_default_blocks_public_bind() {
        let g = GatewayConfig::default();
        assert!(
            !g.allow_public_bind,
            "Public bind must be blocked by default"
        );
    }

    #[test]
    async fn checklist_gateway_default_no_tokens() {
        let g = GatewayConfig::default();
        assert!(
            g.paired_tokens.is_empty(),
            "No pre-paired tokens by default"
        );
        assert_eq!(g.pair_rate_limit_per_minute, 10);
        assert_eq!(g.webhook_rate_limit_per_minute, 60);
        assert!(!g.trust_forwarded_headers);
        assert_eq!(g.rate_limit_max_keys, 10_000);
        assert_eq!(g.idempotency_ttl_secs, 300);
        assert_eq!(g.idempotency_max_keys, 10_000);
    }

    #[test]
    async fn checklist_gateway_cli_default_host_is_localhost() {
        // The CLI default for --host is 127.0.0.1 (checked in main.rs)
        // Here we verify the config default matches
        let c = Config::default();
        assert!(
            c.gateway.require_pairing,
            "Config default must require pairing"
        );
        assert!(
            !c.gateway.allow_public_bind,
            "Config default must block public bind"
        );
    }

    #[test]
    async fn checklist_gateway_serde_roundtrip() {
        let g = GatewayConfig {
            port: 42617,
            host: "127.0.0.1".into(),
            require_pairing: true,
            allow_public_bind: false,
            paired_tokens: vec!["zc_test_token".into()],
            pair_rate_limit_per_minute: 12,
            webhook_rate_limit_per_minute: 80,
            trust_forwarded_headers: true,
            rate_limit_max_keys: 2048,
            idempotency_ttl_secs: 600,
            idempotency_max_keys: 4096,
            token_metadata: HashMap::new(),
            ui_provisioning: UiProvisioningConfig::default(),
            admin_cidrs: vec!["100.64.0.0/10".into()],
        };
        let toml_str = toml::to_string(&g).unwrap();
        let parsed: GatewayConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.require_pairing);
        assert_eq!(parsed.admin_cidrs, vec!["100.64.0.0/10"]);
        assert!(!parsed.allow_public_bind);
        assert_eq!(parsed.paired_tokens, vec!["zc_test_token"]);
        assert_eq!(parsed.pair_rate_limit_per_minute, 12);
        assert_eq!(parsed.webhook_rate_limit_per_minute, 80);
        assert!(parsed.trust_forwarded_headers);
        assert_eq!(parsed.rate_limit_max_keys, 2048);
        assert_eq!(parsed.idempotency_ttl_secs, 600);
        assert_eq!(parsed.idempotency_max_keys, 4096);
    }

    #[test]
    async fn checklist_gateway_backward_compat_no_gateway_section() {
        // Old configs without [gateway] should get secure defaults
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(
            parsed.gateway.require_pairing,
            "Missing [gateway] must default to require_pairing=true"
        );
        assert!(
            !parsed.gateway.allow_public_bind,
            "Missing [gateway] must default to allow_public_bind=false"
        );
    }

    #[test]
    async fn checklist_autonomy_default_is_workspace_scoped() {
        let a = AutonomyConfig::default();
        assert!(a.workspace_only, "Default autonomy must be workspace_only");
        assert!(
            a.forbidden_paths.contains(&"/etc".to_string()),
            "Must block /etc"
        );
        assert!(
            a.forbidden_paths.contains(&"/proc".to_string()),
            "Must block /proc"
        );
        assert!(
            a.forbidden_paths.contains(&"~/.ssh".to_string()),
            "Must block ~/.ssh"
        );
    }

    // ══════════════════════════════════════════════════════════
    // COMPOSIO CONFIG TESTS
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn composio_config_default_disabled() {
        let c = ComposioConfig::default();
        assert!(!c.enabled, "Composio must be disabled by default");
        assert!(c.api_key.is_none(), "No API key by default");
        assert_eq!(c.entity_id, "default");
    }

    #[test]
    async fn composio_config_serde_roundtrip() {
        let c = ComposioConfig {
            enabled: true,
            api_key: Some("comp-key-123".into()),
            entity_id: "user42".into(),
        };
        let toml_str = toml::to_string(&c).unwrap();
        let parsed: ComposioConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.api_key.as_deref(), Some("comp-key-123"));
        assert_eq!(parsed.entity_id, "user42");
    }

    #[test]
    async fn composio_config_backward_compat_missing_section() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(
            !parsed.composio.enabled,
            "Missing [composio] must default to disabled"
        );
        assert!(parsed.composio.api_key.is_none());
    }

    #[test]
    async fn composio_config_partial_toml() {
        let toml_str = r"
enabled = true
";
        let parsed: ComposioConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.enabled);
        assert!(parsed.api_key.is_none());
        assert_eq!(parsed.entity_id, "default");
    }

    #[test]
    async fn composio_config_enable_alias_supported() {
        let toml_str = r"
enable = true
";
        let parsed: ComposioConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.enabled);
        assert!(parsed.api_key.is_none());
        assert_eq!(parsed.entity_id, "default");
    }

    // ══════════════════════════════════════════════════════════
    // SECRETS CONFIG TESTS
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn secrets_config_default_encrypts() {
        let s = SecretsConfig::default();
        assert!(s.encrypt, "Encryption must be enabled by default");
    }

    #[test]
    async fn secrets_config_serde_roundtrip() {
        let s = SecretsConfig { encrypt: false };
        let toml_str = toml::to_string(&s).unwrap();
        let parsed: SecretsConfig = toml::from_str(&toml_str).unwrap();
        assert!(!parsed.encrypt);
    }

    #[test]
    async fn secrets_config_backward_compat_missing_section() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(
            parsed.secrets.encrypt,
            "Missing [secrets] must default to encrypt=true"
        );
    }

    #[test]
    async fn config_default_has_composio_and_secrets() {
        let c = Config::default();
        assert!(!c.composio.enabled);
        assert!(c.composio.api_key.is_none());
        assert!(c.secrets.encrypt);
        assert!(!c.browser.enabled);
        assert!(c.browser.allowed_domains.is_empty());
    }

    #[test]
    async fn browser_config_default_disabled() {
        let b = BrowserConfig::default();
        assert!(!b.enabled);
        assert!(b.allowed_domains.is_empty());
        assert_eq!(b.backend, "agent_browser");
        assert!(b.native_headless);
        assert_eq!(b.native_webdriver_url, "http://127.0.0.1:9515");
        assert!(b.native_chrome_path.is_none());
        assert_eq!(b.computer_use.endpoint, "http://127.0.0.1:8787/v1/actions");
        assert_eq!(b.computer_use.timeout_ms, 15_000);
        assert!(!b.computer_use.allow_remote_endpoint);
        assert!(b.computer_use.window_allowlist.is_empty());
        assert!(b.computer_use.max_coordinate_x.is_none());
        assert!(b.computer_use.max_coordinate_y.is_none());
    }

    #[test]
    async fn browser_config_serde_roundtrip() {
        let b = BrowserConfig {
            enabled: true,
            allowed_domains: vec!["example.com".into(), "docs.example.com".into()],
            session_name: None,
            backend: "auto".into(),
            native_headless: false,
            native_webdriver_url: "http://localhost:4444".into(),
            native_chrome_path: Some("/usr/bin/chromium".into()),
            computer_use: BrowserComputerUseConfig {
                endpoint: "https://computer-use.example.com/v1/actions".into(),
                api_key: Some("test-token".into()),
                timeout_ms: 8_000,
                allow_remote_endpoint: true,
                window_allowlist: vec!["Chrome".into(), "Visual Studio Code".into()],
                max_coordinate_x: Some(3840),
                max_coordinate_y: Some(2160),
            },
        };
        let toml_str = toml::to_string(&b).unwrap();
        let parsed: BrowserConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.allowed_domains.len(), 2);
        assert_eq!(parsed.allowed_domains[0], "example.com");
        assert_eq!(parsed.backend, "auto");
        assert!(!parsed.native_headless);
        assert_eq!(parsed.native_webdriver_url, "http://localhost:4444");
        assert_eq!(
            parsed.native_chrome_path.as_deref(),
            Some("/usr/bin/chromium")
        );
        assert_eq!(
            parsed.computer_use.endpoint,
            "https://computer-use.example.com/v1/actions"
        );
        assert_eq!(parsed.computer_use.api_key.as_deref(), Some("test-token"));
        assert_eq!(parsed.computer_use.timeout_ms, 8_000);
        assert!(parsed.computer_use.allow_remote_endpoint);
        assert_eq!(parsed.computer_use.window_allowlist.len(), 2);
        assert_eq!(parsed.computer_use.max_coordinate_x, Some(3840));
        assert_eq!(parsed.computer_use.max_coordinate_y, Some(2160));
    }

    #[test]
    async fn browser_config_backward_compat_missing_section() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(!parsed.browser.enabled);
        assert!(parsed.browser.allowed_domains.is_empty());
    }

    // ── Environment variable overrides (Docker support) ─────────

    async fn env_override_lock() -> MutexGuard<'static, ()> {
        static ENV_OVERRIDE_TEST_LOCK: Mutex<()> = Mutex::const_new(());
        ENV_OVERRIDE_TEST_LOCK.lock().await
    }

    fn clear_proxy_env_test_vars() {
        for key in [
            "SYNAPSECLAW_PROXY_ENABLED",
            "SYNAPSECLAW_HTTP_PROXY",
            "SYNAPSECLAW_HTTPS_PROXY",
            "SYNAPSECLAW_ALL_PROXY",
            "SYNAPSECLAW_NO_PROXY",
            "SYNAPSECLAW_PROXY_SCOPE",
            "SYNAPSECLAW_PROXY_SERVICES",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    async fn env_override_api_key() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert!(config.api_key.is_none());

        std::env::set_var("SYNAPSECLAW_API_KEY", "sk-test-env-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("sk-test-env-key"));

        std::env::remove_var("SYNAPSECLAW_API_KEY");
    }

    #[test]
    async fn env_override_api_key_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("SYNAPSECLAW_API_KEY");
        std::env::set_var("API_KEY", "sk-fallback-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("sk-fallback-key"));

        std::env::remove_var("API_KEY");
    }

    #[test]
    async fn env_override_provider() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("SYNAPSECLAW_PROVIDER", "anthropic");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("anthropic"));

        std::env::remove_var("SYNAPSECLAW_PROVIDER");
    }

    #[test]
    async fn env_override_model_provider_alias() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("SYNAPSECLAW_PROVIDER");
        std::env::set_var("SYNAPSECLAW_MODEL_PROVIDER", "openai-codex");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("openai-codex"));

        std::env::remove_var("SYNAPSECLAW_MODEL_PROVIDER");
    }

    #[test]
    async fn toml_supports_model_provider_and_model_alias_fields() {
        let raw = r#"
default_temperature = 0.7
model_provider = "sub2api"
model = "gpt-5.3-codex"

[model_providers.sub2api]
name = "sub2api"
base_url = "https://api.tonsof.blue/v1"
wire_api = "responses"
requires_openai_auth = true
"#;

        let parsed: Config = toml::from_str(raw).expect("config should parse");
        assert_eq!(parsed.default_provider.as_deref(), Some("sub2api"));
        assert_eq!(parsed.default_model.as_deref(), Some("gpt-5.3-codex"));
        let profile = parsed
            .model_providers
            .get("sub2api")
            .expect("profile should exist");
        assert_eq!(profile.wire_api.as_deref(), Some("responses"));
        assert!(profile.requires_openai_auth);
    }

    #[test]
    async fn env_override_open_skills_enabled_and_dir() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert!(!config.skills.open_skills_enabled);
        assert!(config.skills.open_skills_dir.is_none());
        assert_eq!(
            config.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Full
        );

        std::env::set_var("SYNAPSECLAW_OPEN_SKILLS_ENABLED", "true");
        std::env::set_var("SYNAPSECLAW_OPEN_SKILLS_DIR", "/tmp/open-skills");
        std::env::set_var("SYNAPSECLAW_SKILLS_PROMPT_MODE", "compact");
        config.apply_env_overrides();

        assert!(config.skills.open_skills_enabled);
        assert_eq!(
            config.skills.open_skills_dir.as_deref(),
            Some("/tmp/open-skills")
        );
        assert_eq!(
            config.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Compact
        );

        std::env::remove_var("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        std::env::remove_var("SYNAPSECLAW_OPEN_SKILLS_DIR");
        std::env::remove_var("SYNAPSECLAW_SKILLS_PROMPT_MODE");
    }

    #[test]
    async fn env_override_open_skills_enabled_invalid_value_keeps_existing_value() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        config.skills.open_skills_enabled = true;
        config.skills.prompt_injection_mode = SkillsPromptInjectionMode::Compact;

        std::env::set_var("SYNAPSECLAW_OPEN_SKILLS_ENABLED", "maybe");
        std::env::set_var("SYNAPSECLAW_SKILLS_PROMPT_MODE", "invalid");
        config.apply_env_overrides();

        assert!(config.skills.open_skills_enabled);
        assert_eq!(
            config.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Compact
        );
        std::env::remove_var("SYNAPSECLAW_OPEN_SKILLS_ENABLED");
        std::env::remove_var("SYNAPSECLAW_SKILLS_PROMPT_MODE");
    }

    #[test]
    async fn env_override_provider_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("SYNAPSECLAW_PROVIDER");
        std::env::set_var("PROVIDER", "openai");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("openai"));

        std::env::remove_var("PROVIDER");
    }

    #[test]
    async fn env_override_provider_fallback_does_not_replace_non_default_provider() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("custom:https://proxy.example.com/v1".to_string()),
            ..Config::default()
        };

        std::env::remove_var("SYNAPSECLAW_PROVIDER");
        std::env::set_var("PROVIDER", "openrouter");
        config.apply_env_overrides();
        assert_eq!(
            config.default_provider.as_deref(),
            Some("custom:https://proxy.example.com/v1")
        );

        std::env::remove_var("PROVIDER");
    }

    #[test]
    async fn env_override_zero_claw_provider_overrides_non_default_provider() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("custom:https://proxy.example.com/v1".to_string()),
            ..Config::default()
        };

        std::env::set_var("SYNAPSECLAW_PROVIDER", "openrouter");
        std::env::set_var("PROVIDER", "anthropic");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("openrouter"));

        std::env::remove_var("SYNAPSECLAW_PROVIDER");
        std::env::remove_var("PROVIDER");
    }

    #[test]
    async fn env_override_glm_api_key_for_regional_aliases() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("glm-cn".to_string()),
            ..Config::default()
        };

        std::env::set_var("GLM_API_KEY", "glm-regional-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("glm-regional-key"));

        std::env::remove_var("GLM_API_KEY");
    }

    #[test]
    async fn env_override_zai_api_key_for_regional_aliases() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("zai-cn".to_string()),
            ..Config::default()
        };

        std::env::set_var("ZAI_API_KEY", "zai-regional-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("zai-regional-key"));

        std::env::remove_var("ZAI_API_KEY");
    }

    #[test]
    async fn env_override_model() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("SYNAPSECLAW_MODEL", "gpt-4o");
        config.apply_env_overrides();
        assert_eq!(config.default_model.as_deref(), Some("gpt-4o"));

        std::env::remove_var("SYNAPSECLAW_MODEL");
    }

    #[test]
    async fn model_provider_profile_maps_to_custom_endpoint() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("sub2api".to_string()),
            model_providers: HashMap::from([(
                "sub2api".to_string(),
                ModelProviderConfig {
                    name: Some("sub2api".to_string()),
                    base_url: Some("https://api.tonsof.blue/v1".to_string()),
                    wire_api: None,
                    requires_openai_auth: false,
                    azure_openai_resource: None,
                    azure_openai_deployment: None,
                    azure_openai_api_version: None,
                    api_path: None,
                },
            )]),
            ..Config::default()
        };

        config.apply_env_overrides();
        assert_eq!(
            config.default_provider.as_deref(),
            Some("custom:https://api.tonsof.blue/v1")
        );
        assert_eq!(
            config.api_url.as_deref(),
            Some("https://api.tonsof.blue/v1")
        );
    }

    #[test]
    async fn model_provider_profile_responses_uses_openai_codex_and_openai_key() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("sub2api".to_string()),
            model_providers: HashMap::from([(
                "sub2api".to_string(),
                ModelProviderConfig {
                    name: Some("sub2api".to_string()),
                    base_url: Some("https://api.tonsof.blue".to_string()),
                    wire_api: Some("responses".to_string()),
                    requires_openai_auth: true,
                    azure_openai_resource: None,
                    azure_openai_deployment: None,
                    azure_openai_api_version: None,
                    api_path: None,
                },
            )]),
            api_key: None,
            ..Config::default()
        };

        std::env::set_var("OPENAI_API_KEY", "sk-test-codex-key");
        config.apply_env_overrides();
        std::env::remove_var("OPENAI_API_KEY");

        assert_eq!(config.default_provider.as_deref(), Some("openai-codex"));
        assert_eq!(config.api_url.as_deref(), Some("https://api.tonsof.blue"));
        assert_eq!(config.api_key.as_deref(), Some("sk-test-codex-key"));
    }

    #[test]
    async fn save_repairs_bare_config_filename_using_runtime_resolution() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("workspace");
        let resolved_config_path = temp_home.join(".synapseclaw").join("config.toml");

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("SYNAPSECLAW_WORKSPACE", &workspace_dir);

        let mut config = Config::default();
        config.workspace_dir = workspace_dir;
        config.config_path = PathBuf::from("config.toml");
        config.default_temperature = 0.5;
        config.save().await.unwrap();

        assert!(resolved_config_path.exists());
        let saved = tokio::fs::read_to_string(&resolved_config_path)
            .await
            .unwrap();
        let parsed: Config = toml::from_str(&saved).unwrap();
        assert_eq!(parsed.default_temperature, 0.5);

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = tokio::fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn validate_ollama_cloud_model_requires_remote_api_url() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            default_provider: Some("ollama".to_string()),
            default_model: Some("glm-5:cloud".to_string()),
            api_url: None,
            api_key: Some("ollama-key".to_string()),
            ..Config::default()
        };

        let error = config.validate().expect_err("expected validation to fail");
        assert!(error.to_string().contains(
            "default_model uses ':cloud' with provider 'ollama', but api_url is local or unset"
        ));
    }

    #[test]
    async fn validate_ollama_cloud_model_accepts_remote_endpoint_and_env_key() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            default_provider: Some("ollama".to_string()),
            default_model: Some("glm-5:cloud".to_string()),
            api_url: Some("https://ollama.com/api".to_string()),
            api_key: None,
            ..Config::default()
        };

        std::env::set_var("OLLAMA_API_KEY", "ollama-env-key");
        let result = config.validate();
        std::env::remove_var("OLLAMA_API_KEY");

        assert!(result.is_ok(), "expected validation to pass: {result:?}");
    }

    #[test]
    async fn validate_rejects_unknown_model_provider_wire_api() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            default_provider: Some("sub2api".to_string()),
            model_providers: HashMap::from([(
                "sub2api".to_string(),
                ModelProviderConfig {
                    name: Some("sub2api".to_string()),
                    base_url: Some("https://api.tonsof.blue/v1".to_string()),
                    wire_api: Some("ws".to_string()),
                    requires_openai_auth: false,
                    azure_openai_resource: None,
                    azure_openai_deployment: None,
                    azure_openai_api_version: None,
                    api_path: None,
                },
            )]),
            ..Config::default()
        };

        let error = config.validate().expect_err("expected validation failure");
        assert!(error
            .to_string()
            .contains("wire_api must be one of: responses, chat_completions"));
    }

    #[test]
    async fn env_override_model_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("SYNAPSECLAW_MODEL");
        std::env::set_var("MODEL", "anthropic/claude-3.5-sonnet");
        config.apply_env_overrides();
        assert_eq!(
            config.default_model.as_deref(),
            Some("anthropic/claude-3.5-sonnet")
        );

        std::env::remove_var("MODEL");
    }

    #[test]
    async fn env_override_workspace() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("SYNAPSECLAW_WORKSPACE", "/custom/workspace");
        config.apply_env_overrides();
        assert_eq!(config.workspace_dir, PathBuf::from("/custom/workspace"));

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
    }

    #[test]
    async fn resolve_runtime_config_dirs_uses_env_workspace_first() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");
        let workspace_dir = default_config_dir.join("profile-a");

        std::env::set_var("SYNAPSECLAW_WORKSPACE", &workspace_dir);
        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::EnvWorkspace);
        assert_eq!(config_dir, workspace_dir);
        assert_eq!(resolved_workspace_dir, workspace_dir.join("workspace"));

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn resolve_runtime_config_dirs_uses_env_config_dir_first() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");
        let explicit_config_dir = default_config_dir.join("explicit-config");
        let marker_config_dir = default_config_dir.join("profiles").join("alpha");
        let state_path = default_config_dir.join(ACTIVE_WORKSPACE_STATE_FILE);

        fs::create_dir_all(&default_config_dir).await.unwrap();
        let state = ActiveWorkspaceState {
            config_dir: marker_config_dir.to_string_lossy().into_owned(),
        };
        fs::write(&state_path, toml::to_string(&state).unwrap())
            .await
            .unwrap();

        std::env::set_var("SYNAPSECLAW_CONFIG_DIR", &explicit_config_dir);
        std::env::remove_var("SYNAPSECLAW_WORKSPACE");

        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::EnvConfigDir);
        assert_eq!(config_dir, explicit_config_dir);
        assert_eq!(
            resolved_workspace_dir,
            explicit_config_dir.join("workspace")
        );

        std::env::remove_var("SYNAPSECLAW_CONFIG_DIR");
        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn resolve_runtime_config_dirs_uses_active_workspace_marker() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");
        let marker_config_dir = default_config_dir.join("profiles").join("alpha");
        let state_path = default_config_dir.join(ACTIVE_WORKSPACE_STATE_FILE);

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        fs::create_dir_all(&default_config_dir).await.unwrap();
        let state = ActiveWorkspaceState {
            config_dir: marker_config_dir.to_string_lossy().into_owned(),
        };
        fs::write(&state_path, toml::to_string(&state).unwrap())
            .await
            .unwrap();

        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::ActiveWorkspaceMarker);
        assert_eq!(config_dir, marker_config_dir);
        assert_eq!(resolved_workspace_dir, marker_config_dir.join("workspace"));

        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn resolve_runtime_config_dirs_falls_back_to_default_layout() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::DefaultConfigDir);
        assert_eq!(config_dir, default_config_dir);
        assert_eq!(resolved_workspace_dir, default_workspace_dir);

        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn load_or_init_workspace_override_uses_workspace_root_for_config() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("profile-a");

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("SYNAPSECLAW_WORKSPACE", &workspace_dir);

        let config = Box::pin(Config::load_or_init()).await.unwrap();

        assert_eq!(config.workspace_dir, workspace_dir.join("workspace"));
        assert_eq!(config.config_path, workspace_dir.join("config.toml"));
        assert!(workspace_dir.join("config.toml").exists());

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_workspace_suffix_uses_legacy_config_layout() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("workspace");
        let legacy_config_path = temp_home.join(".synapseclaw").join("config.toml");

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("SYNAPSECLAW_WORKSPACE", &workspace_dir);

        let config = Box::pin(Config::load_or_init()).await.unwrap();

        assert_eq!(config.workspace_dir, workspace_dir);
        assert_eq!(config.config_path, legacy_config_path);
        assert!(config.config_path.exists());

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_workspace_override_keeps_existing_legacy_config() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("custom-workspace");
        let legacy_config_dir = temp_home.join(".synapseclaw");
        let legacy_config_path = legacy_config_dir.join("config.toml");

        fs::create_dir_all(&legacy_config_dir).await.unwrap();
        fs::write(
            &legacy_config_path,
            r#"default_temperature = 0.7
default_model = "legacy-model"
"#,
        )
        .await
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("SYNAPSECLAW_WORKSPACE", &workspace_dir);

        let config = Box::pin(Config::load_or_init()).await.unwrap();

        assert_eq!(config.workspace_dir, workspace_dir);
        assert_eq!(config.config_path, legacy_config_path);
        assert_eq!(config.default_model.as_deref(), Some("legacy-model"));

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_decrypts_feishu_channel_secrets() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let config_dir = temp_home.join(".synapseclaw");
        let config_path = config_dir.join("config.toml");

        fs::create_dir_all(&config_dir).await.unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::remove_var("SYNAPSECLAW_WORKSPACE");

        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.workspace_dir = config_dir.join("workspace");
        config.secrets.encrypt = true;
        config.channels_config.feishu = Some(FeishuConfig {
            app_id: "cli_feishu_123".into(),
            app_secret: "feishu-secret".into(),
            encrypt_key: Some("feishu-encrypt".into()),
            verification_token: Some("feishu-verify".into()),
            allowed_users: vec!["*".into()],
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
        });
        config.save().await.unwrap();

        let loaded = Box::pin(Config::load_or_init()).await.unwrap();
        let feishu = loaded.channels_config.feishu.as_ref().unwrap();
        assert_eq!(feishu.app_secret, "feishu-secret");
        assert_eq!(feishu.encrypt_key.as_deref(), Some("feishu-encrypt"));
        assert_eq!(feishu.verification_token.as_deref(), Some("feishu-verify"));

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_uses_persisted_active_workspace_marker() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let custom_config_dir = temp_home.join("profiles").join("agent-alpha");

        fs::create_dir_all(&custom_config_dir).await.unwrap();
        fs::write(
            custom_config_dir.join("config.toml"),
            "default_temperature = 0.7\ndefault_model = \"persisted-profile\"\n",
        )
        .await
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::remove_var("SYNAPSECLAW_WORKSPACE");

        persist_active_workspace_config_dir(&custom_config_dir)
            .await
            .unwrap();

        let config = Box::pin(Config::load_or_init()).await.unwrap();

        assert_eq!(config.config_path, custom_config_dir.join("config.toml"));
        assert_eq!(config.workspace_dir, custom_config_dir.join("workspace"));
        assert_eq!(config.default_model.as_deref(), Some("persisted-profile"));

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_env_workspace_override_takes_priority_over_marker() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let marker_config_dir = temp_home.join("profiles").join("persisted-profile");
        let env_workspace_dir = temp_home.join("env-workspace");

        fs::create_dir_all(&marker_config_dir).await.unwrap();
        fs::write(
            marker_config_dir.join("config.toml"),
            "default_temperature = 0.7\ndefault_model = \"marker-model\"\n",
        )
        .await
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        persist_active_workspace_config_dir(&marker_config_dir)
            .await
            .unwrap();
        std::env::set_var("SYNAPSECLAW_WORKSPACE", &env_workspace_dir);

        let config = Box::pin(Config::load_or_init()).await.unwrap();

        assert_eq!(config.workspace_dir, env_workspace_dir.join("workspace"));
        assert_eq!(config.config_path, env_workspace_dir.join("config.toml"));

        std::env::remove_var("SYNAPSECLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn persist_active_workspace_marker_is_cleared_for_default_config_dir() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("synapseclaw_test_home_{}", uuid::Uuid::new_v4()));
        let default_config_dir = temp_home.join(".synapseclaw");
        let custom_config_dir = temp_home.join("profiles").join("custom-profile");
        let marker_path = default_config_dir.join(ACTIVE_WORKSPACE_STATE_FILE);

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);

        persist_active_workspace_config_dir(&custom_config_dir)
            .await
            .unwrap();
        assert!(marker_path.exists());

        persist_active_workspace_config_dir(&default_config_dir)
            .await
            .unwrap();
        assert!(!marker_path.exists());

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn env_override_empty_values_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        let original_provider = config.default_provider.clone();

        std::env::set_var("SYNAPSECLAW_PROVIDER", "");
        config.apply_env_overrides();
        assert_eq!(config.default_provider, original_provider);

        std::env::remove_var("SYNAPSECLAW_PROVIDER");
    }

    #[test]
    async fn env_override_gateway_port() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.gateway.port, 42617);

        std::env::set_var("SYNAPSECLAW_GATEWAY_PORT", "8080");
        config.apply_env_overrides();
        assert_eq!(config.gateway.port, 8080);

        std::env::remove_var("SYNAPSECLAW_GATEWAY_PORT");
    }

    #[test]
    async fn env_override_port_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("SYNAPSECLAW_GATEWAY_PORT");
        std::env::set_var("PORT", "9000");
        config.apply_env_overrides();
        assert_eq!(config.gateway.port, 9000);

        std::env::remove_var("PORT");
    }

    #[test]
    async fn env_override_gateway_host() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.gateway.host, "127.0.0.1");

        std::env::set_var("SYNAPSECLAW_GATEWAY_HOST", "0.0.0.0");
        config.apply_env_overrides();
        assert_eq!(config.gateway.host, "0.0.0.0");

        std::env::remove_var("SYNAPSECLAW_GATEWAY_HOST");
    }

    #[test]
    async fn env_override_host_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("SYNAPSECLAW_GATEWAY_HOST");
        std::env::set_var("HOST", "0.0.0.0");
        config.apply_env_overrides();
        assert_eq!(config.gateway.host, "0.0.0.0");

        std::env::remove_var("HOST");
    }

    #[test]
    async fn env_override_temperature() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("SYNAPSECLAW_TEMPERATURE", "0.5");
        config.apply_env_overrides();
        assert!((config.default_temperature - 0.5).abs() < f64::EPSILON);

        std::env::remove_var("SYNAPSECLAW_TEMPERATURE");
    }

    #[test]
    async fn env_override_temperature_out_of_range_ignored() {
        let _env_guard = env_override_lock().await;
        // Clean up any leftover env vars from other tests
        std::env::remove_var("SYNAPSECLAW_TEMPERATURE");

        let mut config = Config::default();
        let original_temp = config.default_temperature;

        // Temperature > 2.0 should be ignored
        std::env::set_var("SYNAPSECLAW_TEMPERATURE", "3.0");
        config.apply_env_overrides();
        assert!(
            (config.default_temperature - original_temp).abs() < f64::EPSILON,
            "Temperature 3.0 should be ignored (out of range)"
        );

        std::env::remove_var("SYNAPSECLAW_TEMPERATURE");
    }

    #[test]
    async fn env_override_reasoning_enabled() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.runtime.reasoning_enabled, None);

        std::env::set_var("SYNAPSECLAW_REASONING_ENABLED", "false");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_enabled, Some(false));

        std::env::set_var("SYNAPSECLAW_REASONING_ENABLED", "true");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_enabled, Some(true));

        std::env::remove_var("SYNAPSECLAW_REASONING_ENABLED");
    }

    #[test]
    async fn env_override_reasoning_invalid_value_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        config.runtime.reasoning_enabled = Some(false);

        std::env::set_var("SYNAPSECLAW_REASONING_ENABLED", "maybe");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_enabled, Some(false));

        std::env::remove_var("SYNAPSECLAW_REASONING_ENABLED");
    }

    #[test]
    async fn env_override_reasoning_effort() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.runtime.reasoning_effort, None);

        std::env::set_var("SYNAPSECLAW_REASONING_EFFORT", "HIGH");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_effort.as_deref(), Some("high"));

        std::env::remove_var("SYNAPSECLAW_REASONING_EFFORT");
    }

    #[test]
    async fn env_override_reasoning_effort_legacy_codex_env() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("SYNAPSECLAW_CODEX_REASONING_EFFORT", "minimal");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_effort.as_deref(), Some("minimal"));

        std::env::remove_var("SYNAPSECLAW_CODEX_REASONING_EFFORT");
    }

    #[test]
    async fn env_override_invalid_port_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        let original_port = config.gateway.port;

        std::env::set_var("PORT", "not_a_number");
        config.apply_env_overrides();
        assert_eq!(config.gateway.port, original_port);

        std::env::remove_var("PORT");
    }

    #[test]
    async fn env_override_web_search_config() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("WEB_SEARCH_ENABLED", "false");
        std::env::set_var("WEB_SEARCH_PROVIDER", "brave");
        std::env::set_var("WEB_SEARCH_MAX_RESULTS", "7");
        std::env::set_var("WEB_SEARCH_TIMEOUT_SECS", "20");
        std::env::set_var("BRAVE_API_KEY", "brave-test-key");

        config.apply_env_overrides();

        assert!(!config.web_search.enabled);
        assert_eq!(config.web_search.provider, "brave");
        assert_eq!(config.web_search.max_results, 7);
        assert_eq!(config.web_search.timeout_secs, 20);
        assert_eq!(
            config.web_search.brave_api_key.as_deref(),
            Some("brave-test-key")
        );

        std::env::remove_var("WEB_SEARCH_ENABLED");
        std::env::remove_var("WEB_SEARCH_PROVIDER");
        std::env::remove_var("WEB_SEARCH_MAX_RESULTS");
        std::env::remove_var("WEB_SEARCH_TIMEOUT_SECS");
        std::env::remove_var("BRAVE_API_KEY");
    }

    #[test]
    async fn env_override_web_search_invalid_values_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        let original_max_results = config.web_search.max_results;
        let original_timeout = config.web_search.timeout_secs;

        std::env::set_var("WEB_SEARCH_MAX_RESULTS", "99");
        std::env::set_var("WEB_SEARCH_TIMEOUT_SECS", "0");

        config.apply_env_overrides();

        assert_eq!(config.web_search.max_results, original_max_results);
        assert_eq!(config.web_search.timeout_secs, original_timeout);

        std::env::remove_var("WEB_SEARCH_MAX_RESULTS");
        std::env::remove_var("WEB_SEARCH_TIMEOUT_SECS");
    }

    #[test]
    async fn env_override_storage_provider_config() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("SYNAPSECLAW_STORAGE_PROVIDER", "postgres");
        std::env::set_var("SYNAPSECLAW_STORAGE_DB_URL", "postgres://example/db");
        std::env::set_var("SYNAPSECLAW_STORAGE_CONNECT_TIMEOUT_SECS", "15");

        config.apply_env_overrides();

        assert_eq!(config.storage.provider.config.provider, "postgres");
        assert_eq!(
            config.storage.provider.config.db_url.as_deref(),
            Some("postgres://example/db")
        );
        assert_eq!(
            config.storage.provider.config.connect_timeout_secs,
            Some(15)
        );

        std::env::remove_var("SYNAPSECLAW_STORAGE_PROVIDER");
        std::env::remove_var("SYNAPSECLAW_STORAGE_DB_URL");
        std::env::remove_var("SYNAPSECLAW_STORAGE_CONNECT_TIMEOUT_SECS");
    }

    #[test]
    async fn proxy_config_scope_services_requires_entries_when_enabled() {
        let proxy = ProxyConfig {
            enabled: true,
            http_proxy: Some("http://127.0.0.1:7890".into()),
            https_proxy: None,
            all_proxy: None,
            no_proxy: Vec::new(),
            scope: ProxyScope::Services,
            services: Vec::new(),
        };

        let error = proxy.validate().unwrap_err().to_string();
        assert!(error.contains("proxy.scope='services'"));
    }

    #[test]
    async fn env_override_proxy_scope_services() {
        let _env_guard = env_override_lock().await;
        clear_proxy_env_test_vars();

        let mut config = Config::default();
        std::env::set_var("SYNAPSECLAW_PROXY_ENABLED", "true");
        std::env::set_var("SYNAPSECLAW_HTTP_PROXY", "http://127.0.0.1:7890");
        std::env::set_var(
            "SYNAPSECLAW_PROXY_SERVICES",
            "provider.openai, tool.http_request",
        );
        std::env::set_var("SYNAPSECLAW_PROXY_SCOPE", "services");

        config.apply_env_overrides();

        assert!(config.proxy.enabled);
        assert_eq!(config.proxy.scope, ProxyScope::Services);
        assert_eq!(
            config.proxy.http_proxy.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert!(config.proxy.should_apply_to_service("provider.openai"));
        assert!(config.proxy.should_apply_to_service("tool.http_request"));
        assert!(!config.proxy.should_apply_to_service("provider.anthropic"));

        clear_proxy_env_test_vars();
    }

    #[test]
    async fn env_override_proxy_scope_environment_applies_process_env() {
        let _env_guard = env_override_lock().await;
        clear_proxy_env_test_vars();

        let mut config = Config::default();
        std::env::set_var("SYNAPSECLAW_PROXY_ENABLED", "true");
        std::env::set_var("SYNAPSECLAW_PROXY_SCOPE", "environment");
        std::env::set_var("SYNAPSECLAW_HTTP_PROXY", "http://127.0.0.1:7890");
        std::env::set_var("SYNAPSECLAW_HTTPS_PROXY", "http://127.0.0.1:7891");
        std::env::set_var("SYNAPSECLAW_NO_PROXY", "localhost,127.0.0.1");

        config.apply_env_overrides();

        assert_eq!(config.proxy.scope, ProxyScope::Environment);
        assert_eq!(
            std::env::var("HTTP_PROXY").ok().as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            std::env::var("HTTPS_PROXY").ok().as_deref(),
            Some("http://127.0.0.1:7891")
        );
        assert!(std::env::var("NO_PROXY")
            .ok()
            .is_some_and(|value| value.contains("localhost")));

        clear_proxy_env_test_vars();
    }

    fn runtime_proxy_cache_contains(cache_key: &str) -> bool {
        match runtime_proxy_client_cache().read() {
            Ok(guard) => guard.contains_key(cache_key),
            Err(poisoned) => poisoned.into_inner().contains_key(cache_key),
        }
    }

    #[test]
    async fn runtime_proxy_client_cache_reuses_default_profile_key() {
        let service_key = format!(
            "provider.cache_test.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        );
        let cache_key = runtime_proxy_cache_key(&service_key, None, None);

        clear_runtime_proxy_client_cache();
        assert!(!runtime_proxy_cache_contains(&cache_key));

        let _ = build_runtime_proxy_client(&service_key);
        assert!(runtime_proxy_cache_contains(&cache_key));

        let _ = build_runtime_proxy_client(&service_key);
        assert!(runtime_proxy_cache_contains(&cache_key));
    }

    #[test]
    async fn set_runtime_proxy_config_clears_runtime_proxy_client_cache() {
        let service_key = format!(
            "provider.cache_timeout_test.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        );
        let cache_key = runtime_proxy_cache_key(&service_key, Some(30), Some(5));

        clear_runtime_proxy_client_cache();
        let _ = build_runtime_proxy_client_with_timeouts(&service_key, 30, 5);
        assert!(runtime_proxy_cache_contains(&cache_key));

        set_runtime_proxy_config(ProxyConfig::default());
        assert!(!runtime_proxy_cache_contains(&cache_key));
    }

    #[test]
    async fn gateway_config_default_values() {
        let g = GatewayConfig::default();
        assert_eq!(g.port, 42617);
        assert_eq!(g.host, "127.0.0.1");
        assert!(g.require_pairing);
        assert!(!g.allow_public_bind);
        assert!(g.paired_tokens.is_empty());
        assert!(!g.trust_forwarded_headers);
        assert_eq!(g.rate_limit_max_keys, 10_000);
        assert_eq!(g.idempotency_max_keys, 10_000);
    }

    #[test]
    async fn lark_config_serde() {
        let lc = LarkConfig {
            app_id: "cli_123456".into(),
            app_secret: "secret_abc".into(),
            encrypt_key: Some("encrypt_key".into()),
            verification_token: Some("verify_token".into()),
            allowed_users: vec!["user_123".into(), "user_456".into()],
            mention_only: false,
            use_feishu: true,
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
        };
        let json = serde_json::to_string(&lc).unwrap();
        let parsed: LarkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_id, "cli_123456");
        assert_eq!(parsed.app_secret, "secret_abc");
        assert_eq!(parsed.encrypt_key.as_deref(), Some("encrypt_key"));
        assert_eq!(parsed.verification_token.as_deref(), Some("verify_token"));
        assert_eq!(parsed.allowed_users.len(), 2);
        assert!(parsed.use_feishu);
    }

    #[test]
    async fn lark_config_toml_roundtrip() {
        let lc = LarkConfig {
            app_id: "cli_123456".into(),
            app_secret: "secret_abc".into(),
            encrypt_key: Some("encrypt_key".into()),
            verification_token: Some("verify_token".into()),
            allowed_users: vec!["*".into()],
            mention_only: false,
            use_feishu: false,
            receive_mode: LarkReceiveMode::Webhook,
            port: Some(9898),
        };
        let toml_str = toml::to_string(&lc).unwrap();
        let parsed: LarkConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.app_id, "cli_123456");
        assert_eq!(parsed.app_secret, "secret_abc");
        assert!(!parsed.use_feishu);
    }

    #[test]
    async fn lark_config_deserializes_without_optional_fields() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret"}"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.encrypt_key.is_none());
        assert!(parsed.verification_token.is_none());
        assert!(parsed.allowed_users.is_empty());
        assert!(!parsed.mention_only);
        assert!(!parsed.use_feishu);
    }

    #[test]
    async fn lark_config_defaults_to_lark_endpoint() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret"}"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert!(
            !parsed.use_feishu,
            "use_feishu should default to false (Lark)"
        );
    }

    #[test]
    async fn lark_config_with_wildcard_allowed_users() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret","allowed_users":["*"]}"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.allowed_users, vec!["*"]);
    }

    #[test]
    async fn feishu_config_serde() {
        let fc = FeishuConfig {
            app_id: "cli_feishu_123".into(),
            app_secret: "secret_abc".into(),
            encrypt_key: Some("encrypt_key".into()),
            verification_token: Some("verify_token".into()),
            allowed_users: vec!["user_123".into(), "user_456".into()],
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
        };
        let json = serde_json::to_string(&fc).unwrap();
        let parsed: FeishuConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_id, "cli_feishu_123");
        assert_eq!(parsed.app_secret, "secret_abc");
        assert_eq!(parsed.encrypt_key.as_deref(), Some("encrypt_key"));
        assert_eq!(parsed.verification_token.as_deref(), Some("verify_token"));
        assert_eq!(parsed.allowed_users.len(), 2);
    }

    #[test]
    async fn feishu_config_toml_roundtrip() {
        let fc = FeishuConfig {
            app_id: "cli_feishu_123".into(),
            app_secret: "secret_abc".into(),
            encrypt_key: Some("encrypt_key".into()),
            verification_token: Some("verify_token".into()),
            allowed_users: vec!["*".into()],
            receive_mode: LarkReceiveMode::Webhook,
            port: Some(9898),
        };
        let toml_str = toml::to_string(&fc).unwrap();
        let parsed: FeishuConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.app_id, "cli_feishu_123");
        assert_eq!(parsed.app_secret, "secret_abc");
        assert_eq!(parsed.receive_mode, LarkReceiveMode::Webhook);
        assert_eq!(parsed.port, Some(9898));
    }

    #[test]
    async fn feishu_config_deserializes_without_optional_fields() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret"}"#;
        let parsed: FeishuConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.encrypt_key.is_none());
        assert!(parsed.verification_token.is_none());
        assert!(parsed.allowed_users.is_empty());
        assert_eq!(parsed.receive_mode, LarkReceiveMode::Websocket);
        assert!(parsed.port.is_none());
    }

    #[test]
    async fn nextcloud_talk_config_serde() {
        let nc = NextcloudTalkConfig {
            base_url: "https://cloud.example.com".into(),
            app_token: "app-token".into(),
            webhook_secret: Some("webhook-secret".into()),
            allowed_users: vec!["user_a".into(), "*".into()],
        };

        let json = serde_json::to_string(&nc).unwrap();
        let parsed: NextcloudTalkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.base_url, "https://cloud.example.com");
        assert_eq!(parsed.app_token, "app-token");
        assert_eq!(parsed.webhook_secret.as_deref(), Some("webhook-secret"));
        assert_eq!(parsed.allowed_users, vec!["user_a", "*"]);
    }

    #[test]
    async fn nextcloud_talk_config_defaults_optional_fields() {
        let json = r#"{"base_url":"https://cloud.example.com","app_token":"app-token"}"#;
        let parsed: NextcloudTalkConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.webhook_secret.is_none());
        assert!(parsed.allowed_users.is_empty());
    }

    // ── Config file permission hardening (Unix only) ───────────────

    #[cfg(unix)]
    #[test]
    async fn new_config_file_has_restricted_permissions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        // Create a config and save it
        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.save().await.unwrap();

        let meta = fs::metadata(&config_path).await.unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "New config file should be owner-only (0600), got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn save_restricts_existing_world_readable_config_to_owner_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.save().await.unwrap();

        // Simulate the regression state observed in issue #1345.
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loose_mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            loose_mode, 0o644,
            "test setup requires world-readable config"
        );

        config.default_temperature = 0.6;
        config.save().await.unwrap();

        let hardened_mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            hardened_mode, 0o600,
            "Saving config should restore owner-only permissions (0600)"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn world_readable_config_is_detectable() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        // Create a config file with intentionally loose permissions
        std::fs::write(&config_path, "# test config").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let meta = std::fs::metadata(&config_path).unwrap();
        let mode = meta.permissions().mode();
        assert!(
            mode & 0o004 != 0,
            "Test setup: file should be world-readable (mode {mode:o})"
        );
    }

    #[test]
    async fn transcription_config_defaults() {
        let tc = TranscriptionConfig::default();
        assert!(!tc.enabled);
        assert!(tc.api_url.contains("groq.com"));
        assert_eq!(tc.model, "whisper-large-v3-turbo");
        assert!(tc.language.is_none());
        assert_eq!(tc.max_duration_secs, 120);
    }

    #[test]
    async fn config_roundtrip_with_transcription() {
        let mut config = Config::default();
        config.transcription.enabled = true;
        config.transcription.language = Some("en".into());

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();

        assert!(parsed.transcription.enabled);
        assert_eq!(parsed.transcription.language.as_deref(), Some("en"));
        assert_eq!(parsed.transcription.model, "whisper-large-v3-turbo");
    }

    #[test]
    async fn config_without_transcription_uses_defaults() {
        let toml_str = r#"
            default_provider = "openrouter"
            default_model = "test-model"
            default_temperature = 0.7
        "#;
        let parsed: Config = toml::from_str(toml_str).unwrap();
        assert!(!parsed.transcription.enabled);
        assert_eq!(parsed.transcription.max_duration_secs, 120);
    }

    #[test]
    async fn security_defaults_are_backward_compatible() {
        let parsed: Config = toml::from_str(
            r#"
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.6"
default_temperature = 0.7
"#,
        )
        .unwrap();

        assert!(!parsed.security.otp.enabled);
        assert_eq!(parsed.security.otp.method, OtpMethod::Totp);
        assert!(!parsed.security.estop.enabled);
        assert!(parsed.security.estop.require_otp_to_resume);
    }

    #[test]
    async fn security_toml_parses_otp_and_estop_sections() {
        let parsed: Config = toml::from_str(
            r#"
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.6"
default_temperature = 0.7

[security.otp]
enabled = true
method = "totp"
token_ttl_secs = 30
cache_valid_secs = 120
gated_actions = ["shell", "browser_open"]
gated_domains = ["*.chase.com", "accounts.google.com"]
gated_domain_categories = ["banking"]

[security.estop]
enabled = true
state_file = "~/.synapseclaw/estop-state.json"
require_otp_to_resume = true
"#,
        )
        .unwrap();

        assert!(parsed.security.otp.enabled);
        assert!(parsed.security.estop.enabled);
        assert_eq!(parsed.security.otp.gated_actions.len(), 2);
        assert_eq!(parsed.security.otp.gated_domains.len(), 2);
        parsed.validate().unwrap();
    }

    #[test]
    async fn security_validation_rejects_invalid_domain_glob() {
        let mut config = Config::default();
        config.security.otp.gated_domains = vec!["bad domain.com".into()];

        let err = config.validate().expect_err("expected invalid domain glob");
        assert!(err.to_string().contains("gated_domains"));
    }

    #[tokio::test]
    async fn channel_secret_telegram_bot_token_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "synapseclaw_test_tg_bot_token_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).await.unwrap();

        let plaintext_token = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11";

        let mut config = Config::default();
        config.workspace_dir = dir.join("workspace");
        config.config_path = dir.join("config.toml");
        config.channels_config.telegram = Some(TelegramConfig {
            bot_token: plaintext_token.into(),
            allowed_users: vec!["user1".into()],
            stream_mode: StreamMode::default(),
            draft_update_interval_ms: default_draft_update_interval_ms(),
            interrupt_on_new_message: false,
            mention_only: false,
        });

        // Save (triggers encryption)
        config.save().await.unwrap();

        // Read raw TOML and verify plaintext token is NOT present
        let raw_toml = tokio::fs::read_to_string(&config.config_path)
            .await
            .unwrap();
        assert!(
            !raw_toml.contains(plaintext_token),
            "Saved TOML must not contain the plaintext bot_token"
        );

        // Parse stored TOML and verify the value is encrypted
        let stored: Config = toml::from_str(&raw_toml).unwrap();
        let stored_token = &stored.channels_config.telegram.as_ref().unwrap().bot_token;
        assert!(
            synapse_security::SecretStore::is_encrypted(stored_token),
            "Stored bot_token must be marked as encrypted"
        );

        // Decrypt and verify it matches the original plaintext
        let store = synapse_security::SecretStore::new(&dir, true);
        assert_eq!(store.decrypt(stored_token).unwrap(), plaintext_token);

        // Simulate a full load: deserialize then decrypt (mirrors load_or_init logic)
        let mut loaded: Config = toml::from_str(&raw_toml).unwrap();
        loaded.config_path = dir.join("config.toml");
        let load_store = synapse_security::SecretStore::new(&dir, loaded.secrets.encrypt);
        if let Some(ref mut tg) = loaded.channels_config.telegram {
            decrypt_secret(
                &load_store,
                &mut tg.bot_token,
                "config.channels_config.telegram.bot_token",
            )
            .unwrap();
        }
        assert_eq!(
            loaded.channels_config.telegram.as_ref().unwrap().bot_token,
            plaintext_token,
            "Loaded bot_token must match the original plaintext after decryption"
        );

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[test]
    async fn security_validation_rejects_unknown_domain_category() {
        let mut config = Config::default();
        config.security.otp.gated_domain_categories = vec!["not_real".into()];

        let err = config
            .validate()
            .expect_err("expected unknown domain category");
        assert!(err.to_string().contains("gated_domain_categories"));
    }

    #[test]
    async fn security_validation_rejects_zero_token_ttl() {
        let mut config = Config::default();
        config.security.otp.token_ttl_secs = 0;

        let err = config
            .validate()
            .expect_err("expected ttl validation failure");
        assert!(err.to_string().contains("token_ttl_secs"));
    }

    // ── MCP config validation ─────────────────────────────────────────────

    fn stdio_server(name: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Stdio,
            command: command.to_string(),
            ..Default::default()
        }
    }

    fn http_server(name: &str, url: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Http,
            url: Some(url.to_string()),
            ..Default::default()
        }
    }

    fn sse_server(name: &str, url: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Sse,
            url: Some(url.to_string()),
            ..Default::default()
        }
    }

    #[test]
    async fn validate_mcp_config_empty_servers_ok() {
        let cfg = McpConfig::default();
        assert!(validate_mcp_config(&cfg).is_ok());
    }

    #[test]
    async fn validate_mcp_config_valid_stdio_ok() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![stdio_server("fs", "/usr/bin/mcp-fs")],
            ..Default::default()
        };
        assert!(validate_mcp_config(&cfg).is_ok());
    }

    #[test]
    async fn validate_mcp_config_valid_http_ok() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![http_server("svc", "http://localhost:8080/mcp")],
            ..Default::default()
        };
        assert!(validate_mcp_config(&cfg).is_ok());
    }

    #[test]
    async fn validate_mcp_config_valid_sse_ok() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![sse_server("svc", "https://example.com/events")],
            ..Default::default()
        };
        assert!(validate_mcp_config(&cfg).is_ok());
    }

    #[test]
    async fn validate_mcp_config_rejects_empty_name() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![stdio_server("", "/usr/bin/tool")],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("empty name should fail");
        assert!(
            err.to_string().contains("name must not be empty"),
            "got: {err}"
        );
    }

    #[test]
    async fn validate_mcp_config_rejects_whitespace_name() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![stdio_server("   ", "/usr/bin/tool")],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("whitespace name should fail");
        assert!(
            err.to_string().contains("name must not be empty"),
            "got: {err}"
        );
    }

    #[test]
    async fn validate_mcp_config_rejects_duplicate_names() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![
                stdio_server("fs", "/usr/bin/mcp-a"),
                stdio_server("fs", "/usr/bin/mcp-b"),
            ],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("duplicate name should fail");
        assert!(err.to_string().contains("duplicate name"), "got: {err}");
    }

    #[test]
    async fn validate_mcp_config_rejects_zero_timeout() {
        let mut server = stdio_server("fs", "/usr/bin/mcp-fs");
        server.tool_timeout_secs = Some(0);
        let cfg = McpConfig {
            enabled: true,
            servers: vec![server],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("zero timeout should fail");
        assert!(err.to_string().contains("greater than 0"), "got: {err}");
    }

    #[test]
    async fn validate_mcp_config_rejects_timeout_exceeding_max() {
        let mut server = stdio_server("fs", "/usr/bin/mcp-fs");
        server.tool_timeout_secs = Some(MCP_MAX_TOOL_TIMEOUT_SECS + 1);
        let cfg = McpConfig {
            enabled: true,
            servers: vec![server],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("oversized timeout should fail");
        assert!(err.to_string().contains("exceeds max"), "got: {err}");
    }

    #[test]
    async fn validate_mcp_config_allows_max_timeout_exactly() {
        let mut server = stdio_server("fs", "/usr/bin/mcp-fs");
        server.tool_timeout_secs = Some(MCP_MAX_TOOL_TIMEOUT_SECS);
        let cfg = McpConfig {
            enabled: true,
            servers: vec![server],
            ..Default::default()
        };
        assert!(validate_mcp_config(&cfg).is_ok());
    }

    #[test]
    async fn validate_mcp_config_rejects_stdio_with_empty_command() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![stdio_server("fs", "")],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("empty command should fail");
        assert!(
            err.to_string().contains("requires non-empty command"),
            "got: {err}"
        );
    }

    #[test]
    async fn validate_mcp_config_rejects_http_without_url() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                name: "svc".to_string(),
                transport: McpTransport::Http,
                url: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("http without url should fail");
        assert!(err.to_string().contains("requires url"), "got: {err}");
    }

    #[test]
    async fn validate_mcp_config_rejects_sse_without_url() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                name: "svc".to_string(),
                transport: McpTransport::Sse,
                url: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("sse without url should fail");
        assert!(err.to_string().contains("requires url"), "got: {err}");
    }

    #[test]
    async fn validate_mcp_config_rejects_non_http_scheme() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![http_server("svc", "ftp://example.com/mcp")],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("non-http scheme should fail");
        assert!(err.to_string().contains("http/https"), "got: {err}");
    }

    #[test]
    async fn validate_mcp_config_rejects_invalid_url() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![http_server("svc", "not a url at all !!!")],
            ..Default::default()
        };
        let err = validate_mcp_config(&cfg).expect_err("invalid url should fail");
        assert!(err.to_string().contains("valid URL"), "got: {err}");
    }

    #[test]
    async fn mcp_config_default_disabled_with_empty_servers() {
        let cfg = McpConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.servers.is_empty());
    }

    #[test]
    async fn mcp_transport_serde_roundtrip_lowercase() {
        let cases = [
            (McpTransport::Stdio, "\"stdio\""),
            (McpTransport::Http, "\"http\""),
            (McpTransport::Sse, "\"sse\""),
        ];
        for (variant, expected_json) in &cases {
            let serialized = serde_json::to_string(variant).expect("serialize");
            assert_eq!(&serialized, expected_json, "variant: {variant:?}");
            let deserialized: McpTransport =
                serde_json::from_str(expected_json).expect("deserialize");
            assert_eq!(&deserialized, variant);
        }
    }

    #[test]
    async fn validate_matrix_password_requires_user_id() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            channels_config: ChannelsConfig {
                matrix: Some(MatrixConfig {
                    homeserver: "https://matrix.org".into(),
                    access_token: None,
                    user_id: None,
                    device_id: None,
                    room_id: "!room:matrix.org".into(),
                    allowed_users: vec!["@u:m".into()],
                    password: Some("secret".into()),
                    max_media_download_mb: None,
                }),
                ..ChannelsConfig::default()
            },
            ..Config::default()
        };
        let err = config.validate().expect_err("expected validation to fail");
        assert!(err
            .to_string()
            .contains("user_id is required when password is set"));
    }

    #[test]
    async fn validate_matrix_password_with_user_id_ok() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            channels_config: ChannelsConfig {
                matrix: Some(MatrixConfig {
                    homeserver: "https://matrix.org".into(),
                    access_token: None,
                    user_id: Some("@bot:matrix.org".into()),
                    device_id: None,
                    room_id: "!room:matrix.org".into(),
                    allowed_users: vec!["@u:m".into()],
                    password: Some("secret".into()),
                    max_media_download_mb: None,
                }),
                ..ChannelsConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    async fn validate_matrix_access_token_only_ok() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            channels_config: ChannelsConfig {
                matrix: Some(MatrixConfig {
                    homeserver: "https://matrix.org".into(),
                    access_token: Some("syt_tok".into()),
                    user_id: None,
                    device_id: None,
                    room_id: "!room:matrix.org".into(),
                    allowed_users: vec!["@u:m".into()],
                    password: None,
                    max_media_download_mb: None,
                }),
                ..ChannelsConfig::default()
            },
            ..Config::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    async fn swarm_strategy_roundtrip() {
        let cases = vec![
            (SwarmStrategy::Sequential, "\"sequential\""),
            (SwarmStrategy::Parallel, "\"parallel\""),
            (SwarmStrategy::Router, "\"router\""),
        ];
        for (variant, expected_json) in &cases {
            let serialized = serde_json::to_string(variant).expect("serialize");
            assert_eq!(&serialized, expected_json, "variant: {variant:?}");
            let deserialized: SwarmStrategy =
                serde_json::from_str(expected_json).expect("deserialize");
            assert_eq!(&deserialized, variant);
        }
    }

    #[test]
    async fn swarm_config_deserializes_with_defaults() {
        let toml_str = r#"
            agents = ["researcher", "writer"]
            strategy = "sequential"
        "#;
        let config: SwarmConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(config.agents, vec!["researcher", "writer"]);
        assert_eq!(config.strategy, SwarmStrategy::Sequential);
        assert!(config.router_prompt.is_none());
        assert!(config.description.is_none());
        assert_eq!(config.timeout_secs, 300);
    }

    #[test]
    async fn swarm_config_deserializes_full() {
        let toml_str = r#"
            agents = ["a", "b", "c"]
            strategy = "router"
            router_prompt = "Pick the best."
            description = "Multi-agent router"
            timeout_secs = 120
        "#;
        let config: SwarmConfig = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(config.agents.len(), 3);
        assert_eq!(config.strategy, SwarmStrategy::Router);
        assert_eq!(config.router_prompt.as_deref(), Some("Pick the best."));
        assert_eq!(config.description.as_deref(), Some("Multi-agent router"));
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    async fn config_with_swarms_section_deserializes() {
        let toml_str = r#"
            [agents.researcher]
            provider = "ollama"
            model = "llama3"

            [agents.writer]
            provider = "openrouter"
            model = "claude-sonnet"

            [swarms.pipeline]
            agents = ["researcher", "writer"]
            strategy = "sequential"
        "#;
        let config: Config = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(config.agents.len(), 2);
        assert_eq!(config.swarms.len(), 1);
        assert!(config.swarms.contains_key("pipeline"));
    }

    #[tokio::test]
    async fn nevis_client_secret_encrypt_decrypt_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "synapseclaw_test_nevis_secret_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).await.unwrap();

        let plaintext_secret = "nevis-test-client-secret-value";

        let mut config = Config::default();
        config.workspace_dir = dir.join("workspace");
        config.config_path = dir.join("config.toml");
        config.security.nevis.client_secret = Some(plaintext_secret.into());

        // Save (triggers encryption)
        config.save().await.unwrap();

        // Read raw TOML and verify plaintext secret is NOT present
        let raw_toml = tokio::fs::read_to_string(&config.config_path)
            .await
            .unwrap();
        assert!(
            !raw_toml.contains(plaintext_secret),
            "Saved TOML must not contain the plaintext client_secret"
        );

        // Parse stored TOML and verify the value is encrypted
        let stored: Config = toml::from_str(&raw_toml).unwrap();
        let stored_secret = stored.security.nevis.client_secret.as_ref().unwrap();
        assert!(
            synapse_security::SecretStore::is_encrypted(stored_secret),
            "Stored client_secret must be marked as encrypted"
        );

        // Decrypt and verify it matches the original plaintext
        let store = synapse_security::SecretStore::new(&dir, true);
        assert_eq!(store.decrypt(stored_secret).unwrap(), plaintext_secret);

        // Simulate a full load: deserialize then decrypt (mirrors load_or_init logic)
        let mut loaded: Config = toml::from_str(&raw_toml).unwrap();
        loaded.config_path = dir.join("config.toml");
        let load_store = synapse_security::SecretStore::new(&dir, loaded.secrets.encrypt);
        decrypt_optional_secret(
            &load_store,
            &mut loaded.security.nevis.client_secret,
            "config.security.nevis.client_secret",
        )
        .unwrap();
        assert_eq!(
            loaded.security.nevis.client_secret.as_deref().unwrap(),
            plaintext_secret,
            "Loaded client_secret must match the original plaintext after decryption"
        );

        let _ = fs::remove_dir_all(&dir).await;
    }

    // ══════════════════════════════════════════════════════════
    // Nevis config validation tests
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn nevis_config_validate_disabled_accepts_empty_fields() {
        let cfg = NevisConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    async fn nevis_config_validate_rejects_empty_instance_url() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: String::new(),
            client_id: "test-client".into(),
            ..NevisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("instance_url"));
    }

    #[test]
    async fn nevis_config_validate_rejects_empty_client_id() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: "https://nevis.example.com".into(),
            client_id: String::new(),
            ..NevisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("client_id"));
    }

    #[test]
    async fn nevis_config_validate_rejects_empty_realm() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: "https://nevis.example.com".into(),
            client_id: "test-client".into(),
            realm: String::new(),
            ..NevisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("realm"));
    }

    #[test]
    async fn nevis_config_validate_rejects_local_without_jwks() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: "https://nevis.example.com".into(),
            client_id: "test-client".into(),
            token_validation: "local".into(),
            jwks_url: None,
            ..NevisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("jwks_url"));
    }

    #[test]
    async fn nevis_config_validate_rejects_zero_session_timeout() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: "https://nevis.example.com".into(),
            client_id: "test-client".into(),
            token_validation: "remote".into(),
            session_timeout_secs: 0,
            ..NevisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("session_timeout_secs"));
    }

    #[test]
    async fn nevis_config_validate_accepts_valid_enabled_config() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: "https://nevis.example.com".into(),
            realm: "master".into(),
            client_id: "test-client".into(),
            token_validation: "remote".into(),
            session_timeout_secs: 3600,
            ..NevisConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    async fn nevis_config_validate_rejects_invalid_token_validation() {
        let cfg = NevisConfig {
            enabled: true,
            instance_url: "https://nevis.example.com".into(),
            realm: "master".into(),
            client_id: "test-client".into(),
            token_validation: "invalid_mode".into(),
            session_timeout_secs: 3600,
            ..NevisConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.contains("invalid value 'invalid_mode'"),
            "Expected invalid token_validation error, got: {err}"
        );
    }

    #[test]
    async fn nevis_config_debug_redacts_client_secret() {
        let cfg = NevisConfig {
            client_secret: Some("super-secret".into()),
            ..NevisConfig::default()
        };
        let debug_output = format!("{:?}", cfg);
        assert!(
            !debug_output.contains("super-secret"),
            "Debug output must not contain the raw client_secret"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output must show [REDACTED] for client_secret"
        );
    }
}
