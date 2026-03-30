//! ConfigIO implementation — load/save/encrypt/decrypt for Config.

use anyhow::{Context, Result};
use directories::UserDirs;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use synapse_core::config::provider_aliases::{is_glm_alias, is_zai_alias};
use synapse_core::config::schema::*;
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

// ConfigIO trait defined here (adapter-owned) to satisfy orphan rule.
#[async_trait::async_trait]
pub trait ConfigIO {
    async fn load_or_init() -> anyhow::Result<Self>
    where
        Self: Sized;
    fn validate(&self) -> anyhow::Result<()>;
    fn apply_env_overrides(&mut self);
    async fn save(&self) -> anyhow::Result<()>;
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
