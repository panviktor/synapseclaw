//! Audit logging for security events
//!
//! Each audit entry is chained via a Merkle hash: `entry_hash = SHA-256(prev_hash || canonical_json)`.
//! This makes the trail tamper-evident — modifying any entry invalidates all subsequent hashes.

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use fork_config::schema::AuditConfig;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Well-known seed for the genesis entry's `prev_hash`.
const GENESIS_PREV_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    CommandExecution,
    FileAccess,
    ConfigChange,
    AuthSuccess,
    AuthFailure,
    PolicyViolation,
    SecurityEvent,
    IpcSend,
    IpcBlocked,
    IpcRateLimited,
    IpcReceived,
    IpcStateChange,
    IpcAdminAction,
    IpcLeakDetected,
    ProvisioningArmed,
    ProvisioningDisarmed,
    ProvisioningAgentCreated,
    ProvisioningServiceInstalled,
    ProvisioningServiceStarted,
    ProvisioningServiceStopped,
    ProvisioningFailed,
}

/// Actor information (who performed the action)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub channel: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
}

/// Action information (what was done)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub command: Option<String>,
    pub risk_level: Option<String>,
    pub approved: bool,
    pub allowed: bool,
}

/// Execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

/// Security context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityContext {
    pub policy_violation: bool,
    pub rate_limit_remaining: Option<u32>,
    pub sandbox_backend: Option<String>,
}

/// Complete audit event with Merkle hash-chain fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_id: String,
    pub event_type: AuditEventType,
    pub actor: Option<Actor>,
    pub action: Option<Action>,
    pub result: Option<ExecutionResult>,
    pub security: SecurityContext,

    /// Monotonically increasing sequence number.
    #[serde(default)]
    pub sequence: u64,
    /// SHA-256 hash of the previous entry (genesis uses [`GENESIS_PREV_HASH`]).
    #[serde(default)]
    pub prev_hash: String,
    /// SHA-256 hash of (`prev_hash` || canonical JSON of this entry's content fields).
    #[serde(default)]
    pub entry_hash: String,

    /// HMAC-SHA256 chain value: HMAC(key, "{prev_hmac}|{event_json}").
    /// Present when HMAC audit chain is enabled (Phase 3B).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event
    pub fn new(event_type: AuditEventType) -> Self {
        Self {
            timestamp: Utc::now(),
            event_id: Uuid::new_v4().to_string(),
            event_type,
            actor: None,
            action: None,
            result: None,
            security: SecurityContext {
                policy_violation: false,
                rate_limit_remaining: None,
                sandbox_backend: None,
            },
            sequence: 0,
            prev_hash: String::new(),
            entry_hash: String::new(),
            hmac: None,
        }
    }

    /// Set the actor
    pub fn with_actor(
        mut self,
        channel: String,
        user_id: Option<String>,
        username: Option<String>,
    ) -> Self {
        self.actor = Some(Actor {
            channel,
            user_id,
            username,
        });
        self
    }

    /// Set the action
    pub fn with_action(
        mut self,
        command: String,
        risk_level: String,
        approved: bool,
        allowed: bool,
    ) -> Self {
        self.action = Some(Action {
            command: Some(command),
            risk_level: Some(risk_level),
            approved,
            allowed,
        });
        self
    }

    /// Set the result
    pub fn with_result(
        mut self,
        success: bool,
        exit_code: Option<i32>,
        duration_ms: u64,
        error: Option<String>,
    ) -> Self {
        self.result = Some(ExecutionResult {
            success,
            exit_code,
            duration_ms: Some(duration_ms),
            error,
        });
        self
    }

    /// Create an IPC audit event with agent context.
    pub fn ipc(
        event_type: AuditEventType,
        from_agent: &str,
        to_agent: Option<&str>,
        detail: &str,
    ) -> Self {
        let command = match to_agent {
            Some(to) => format!("ipc: from={from_agent} to={to} {detail}"),
            None => format!("ipc: from={from_agent} {detail}"),
        };
        Self::new(event_type)
            .with_actor("ipc".to_string(), Some(from_agent.to_string()), None)
            .with_action(command, "high".to_string(), false, true)
    }

    /// Set security context
    pub fn with_security(mut self, sandbox_backend: Option<String>) -> Self {
        self.security.sandbox_backend = sandbox_backend;
        self
    }
}

/// Compute the SHA-256 entry hash: `H(prev_hash || content_json)`.
///
/// `content_json` is the canonical JSON of the event *without* the chain fields
/// (`sequence`, `prev_hash`, `entry_hash`), so the hash covers only the payload.
fn compute_entry_hash(prev_hash: &str, event: &AuditEvent) -> String {
    // Build a canonical representation of the content fields only.
    let content = serde_json::json!({
        "timestamp": event.timestamp,
        "event_id": event.event_id,
        "event_type": event.event_type,
        "actor": event.actor,
        "action": event.action,
        "result": event.result,
        "security": event.security,
        "sequence": event.sequence,
    });
    let content_json = serde_json::to_string(&content).expect("serialize canonical content");

    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(content_json.as_bytes());
    hex::encode(hasher.finalize())
}

/// Internal chain state tracked across writes.
struct ChainState {
    prev_hash: String,
    sequence: u64,
}

/// Audit logger with Merkle hash-chain and optional HMAC-SHA256 for tamper detection.
pub struct AuditLogger {
    log_path: PathBuf,
    config: AuditConfig,
    buffer: Mutex<Vec<AuditEvent>>,
    chain: Mutex<ChainState>,
    /// HMAC key for chain computation (loaded from audit.key file).
    hmac_key: Option<Vec<u8>>,
    /// Previous HMAC in the chain (hex-encoded). Updated on each log().
    prev_hmac: Mutex<String>,
}

/// Structured command execution details for audit logging.
#[derive(Debug, Clone)]
pub struct CommandExecutionLog<'a> {
    pub channel: &'a str,
    pub command: &'a str,
    pub risk_level: &'a str,
    pub approved: bool,
    pub allowed: bool,
    pub success: bool,
    pub duration_ms: u64,
}

impl AuditLogger {
    /// Create a new audit logger.
    ///
    /// If the log file already exists, the chain state is recovered from the last
    /// entry so that new writes continue the existing hash chain.
    pub fn new(config: AuditConfig, synapseclaw_dir: PathBuf) -> Result<Self> {
        let log_path = synapseclaw_dir.join(&config.log_path);
        let chain_state = recover_chain_state(&log_path);
        let key_path = synapseclaw_dir.join("audit.key");

        let hmac_key = if config.sign_events {
            match load_or_generate_hmac_key(&key_path) {
                Ok(key) => {
                    tracing::info!("HMAC audit chain enabled");
                    Some(key)
                }
                Err(e) => {
                    tracing::warn!("HMAC audit chain disabled: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Read the last HMAC from the existing log file (if any) to continue the chain
        let prev_hmac = if hmac_key.is_some() {
            read_last_hmac(&log_path).unwrap_or_default()
        } else {
            String::new()
        };

        Ok(Self {
            log_path,
            config,
            buffer: Mutex::new(Vec::new()),
            chain: Mutex::new(chain_state),
            hmac_key,
            prev_hmac: Mutex::new(prev_hmac),
        })
    }

    /// Log an event
    pub fn log(&self, event: &AuditEvent) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check log size and rotate if needed
        self.rotate_if_needed()?;

        // Populate Merkle chain fields under the lock
        let mut chained = event.clone();
        {
            let mut state = self.chain.lock();
            chained.sequence = state.sequence;
            chained.prev_hash = state.prev_hash.clone();
            chained.entry_hash = compute_entry_hash(&state.prev_hash, &chained);
            state.prev_hash = chained.entry_hash.clone();
            state.sequence += 1;
        }

        // Compute HMAC chain if key is available
        if let Some(ref key) = self.hmac_key {
            chained.hmac = None;
            let event_json = serde_json::to_string(&chained)?;
            let prev = self.prev_hmac.lock().clone();
            let chain_input = format!("{prev}|{event_json}");
            let hmac_hex = compute_hmac_sha256(key, chain_input.as_bytes());
            chained.hmac = Some(hmac_hex.clone());
            *self.prev_hmac.lock() = hmac_hex;
        }

        // Serialize and write
        let line = serde_json::to_string(&chained)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;

        writeln!(file, "{}", line)?;
        file.sync_all()?;

        Ok(())
    }

    /// Log a command execution event.
    pub fn log_command_event(&self, entry: CommandExecutionLog<'_>) -> Result<()> {
        let event = AuditEvent::new(AuditEventType::CommandExecution)
            .with_actor(entry.channel.to_string(), None, None)
            .with_action(
                entry.command.to_string(),
                entry.risk_level.to_string(),
                entry.approved,
                entry.allowed,
            )
            .with_result(entry.success, None, entry.duration_ms, None);

        self.log(&event)
    }

    /// Backward-compatible helper to log a command execution event.
    #[allow(clippy::too_many_arguments)]
    pub fn log_command(
        &self,
        channel: &str,
        command: &str,
        risk_level: &str,
        approved: bool,
        allowed: bool,
        success: bool,
        duration_ms: u64,
    ) -> Result<()> {
        self.log_command_event(CommandExecutionLog {
            channel,
            command,
            risk_level,
            approved,
            allowed,
            success,
            duration_ms,
        })
    }

    /// Rotate log if it exceeds max size
    fn rotate_if_needed(&self) -> Result<()> {
        if let Ok(metadata) = std::fs::metadata(&self.log_path) {
            let current_size_mb = metadata.len() / (1024 * 1024);
            if current_size_mb >= u64::from(self.config.max_size_mb) {
                self.rotate()?;
            }
        }
        Ok(())
    }

    /// Rotate the log file
    fn rotate(&self) -> Result<()> {
        for i in (1..10).rev() {
            let old_name = format!("{}.{}.log", self.log_path.display(), i);
            let new_name = format!("{}.{}.log", self.log_path.display(), i + 1);
            let _ = std::fs::rename(&old_name, &new_name);
        }

        let rotated = format!("{}.1.log", self.log_path.display());
        std::fs::rename(&self.log_path, &rotated)?;
        Ok(())
    }
}

/// Recover chain state from an existing log file.
///
/// Returns the genesis state if the file does not exist or is empty.
fn recover_chain_state(log_path: &Path) -> ChainState {
    let file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(_) => {
            return ChainState {
                prev_hash: GENESIS_PREV_HASH.to_string(),
                sequence: 0,
            };
        }
    };

    let reader = BufReader::new(file);
    let mut last_entry: Option<AuditEvent> = None;
    for l in reader.lines().map_while(Result::ok) {
        if let Ok(entry) = serde_json::from_str::<AuditEvent>(&l) {
            last_entry = Some(entry);
        }
    }

    match last_entry {
        Some(entry) => ChainState {
            prev_hash: entry.entry_hash,
            sequence: entry.sequence + 1,
        },
        None => ChainState {
            prev_hash: GENESIS_PREV_HASH.to_string(),
            sequence: 0,
        },
    }
}

/// Verify the integrity of an audit log's Merkle hash chain.
///
/// Reads every entry from the log file and checks:
/// - Each `entry_hash` matches the recomputed `SHA-256(prev_hash || content)`.
/// - `prev_hash` links to the preceding entry (or the genesis seed for the first).
/// - Sequence numbers are contiguous starting from 0.
///
/// Returns `Ok(entry_count)` on success, or an error describing the first violation.
pub fn verify_chain(log_path: &Path) -> Result<u64> {
    let file = std::fs::File::open(log_path)?;
    let reader = BufReader::new(file);

    let mut expected_prev_hash = GENESIS_PREV_HASH.to_string();
    let mut expected_sequence: u64 = 0;

    for (line_idx, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEvent = serde_json::from_str(&line)?;

        // Check sequence continuity
        if entry.sequence != expected_sequence {
            bail!(
                "sequence gap at line {}: expected {}, got {}",
                line_idx + 1,
                expected_sequence,
                entry.sequence
            );
        }

        // Check prev_hash linkage
        if entry.prev_hash != expected_prev_hash {
            bail!(
                "prev_hash mismatch at line {} (sequence {}): expected {}, got {}",
                line_idx + 1,
                entry.sequence,
                expected_prev_hash,
                entry.prev_hash
            );
        }

        // Recompute and verify entry_hash
        let recomputed = compute_entry_hash(&entry.prev_hash, &entry);
        if entry.entry_hash != recomputed {
            bail!(
                "entry_hash mismatch at line {} (sequence {}): expected {}, got {}",
                line_idx + 1,
                entry.sequence,
                recomputed,
                entry.entry_hash
            );
        }

        expected_prev_hash = entry.entry_hash.clone();
        expected_sequence += 1;
    }

    Ok(expected_sequence)
}

// ── HMAC chain helpers ──────────────────────────────────────────

/// Compute HMAC-SHA256 and return hex-encoded result.
fn compute_hmac_sha256(key: &[u8], data: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256 as HmacSha2;

    type HmacSha256 = Hmac<HmacSha2>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

/// Load HMAC key from file, or generate and save a new one (32 random bytes).
fn load_or_generate_hmac_key(path: &std::path::Path) -> Result<Vec<u8>> {
    if path.exists() {
        let data = std::fs::read(path)?;
        if data.len() < 16 {
            anyhow::bail!("HMAC key file too short ({} bytes)", data.len());
        }
        Ok(data)
    } else {
        let key: [u8; 32] = rand::random();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, key)?;
        Ok(key.to_vec())
    }
}

/// Read the last HMAC value from an existing audit log file.
fn read_last_hmac(log_path: &std::path::Path) -> Result<String> {
    if !log_path.exists() {
        return Ok(String::new());
    }

    let file = std::fs::File::open(log_path)?;
    let reader = std::io::BufReader::new(file);
    let mut last_hmac = String::new();

    for line in reader.lines() {
        let line = line?;
        if let Ok(event) = serde_json::from_str::<AuditEvent>(&line) {
            if let Some(hmac) = event.hmac {
                last_hmac = hmac;
            }
        }
    }

    Ok(last_hmac)
}

/// Verify the HMAC chain in an audit log file.
///
/// Returns `Ok(count)` with the number of verified entries, or `Err` with
/// details about the first broken link.
pub fn verify_audit_chain(log_path: &std::path::Path, key_path: &std::path::Path) -> Result<usize> {
    let key = std::fs::read(key_path)
        .map_err(|e| anyhow::anyhow!("Failed to read HMAC key at {}: {e}", key_path.display()))?;

    let file = std::fs::File::open(log_path)
        .map_err(|e| anyhow::anyhow!("Failed to open audit log at {}: {e}", log_path.display()))?;
    let reader = std::io::BufReader::new(file);

    let mut prev_hmac = String::new();
    let mut verified = 0usize;

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let event: AuditEvent = serde_json::from_str(&line)
            .map_err(|e| anyhow::anyhow!("Line {}: invalid JSON: {e}", line_num + 1))?;

        let stored_hmac = match &event.hmac {
            Some(h) => h.clone(),
            None => continue, // Entry without HMAC — skip (pre-chain entries)
        };

        let mut event_for_hash = event;
        event_for_hash.hmac = None;
        let event_json = serde_json::to_string(&event_for_hash)?;
        let chain_input = format!("{prev_hmac}|{event_json}");
        let expected_hmac = compute_hmac_sha256(&key, chain_input.as_bytes());

        if stored_hmac != expected_hmac {
            anyhow::bail!(
                "HMAC chain broken at line {} (event_id={}): expected={}, stored={}",
                line_num + 1,
                event_for_hash.event_id,
                &expected_hmac[..16],
                &stored_hmac[..16],
            );
        }

        prev_hmac = stored_hmac;
        verified += 1;
    }

    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn audit_event_new_creates_unique_id() {
        let event1 = AuditEvent::new(AuditEventType::CommandExecution);
        let event2 = AuditEvent::new(AuditEventType::CommandExecution);
        assert_ne!(event1.event_id, event2.event_id);
    }

    #[test]
    fn audit_event_with_actor() {
        let event = AuditEvent::new(AuditEventType::CommandExecution).with_actor(
            "telegram".to_string(),
            Some("123".to_string()),
            Some("@synapseclaw_user".to_string()),
        );

        assert!(event.actor.is_some());
        let actor = event.actor.as_ref().unwrap();
        assert_eq!(actor.channel, "telegram");
        assert_eq!(actor.user_id, Some("123".to_string()));
        assert_eq!(actor.username, Some("@synapseclaw_user".to_string()));
    }

    #[test]
    fn audit_event_with_action() {
        let event = AuditEvent::new(AuditEventType::CommandExecution).with_action(
            "ls -la".to_string(),
            "low".to_string(),
            false,
            true,
        );

        assert!(event.action.is_some());
        let action = event.action.as_ref().unwrap();
        assert_eq!(action.command, Some("ls -la".to_string()));
        assert_eq!(action.risk_level, Some("low".to_string()));
    }

    #[test]
    fn audit_event_serializes_to_json() {
        let event = AuditEvent::new(AuditEventType::CommandExecution)
            .with_actor("telegram".to_string(), None, None)
            .with_action("ls".to_string(), "low".to_string(), false, true)
            .with_result(true, Some(0), 15, None);

        let json = serde_json::to_string(&event);
        assert!(json.is_ok());
        let json = json.expect("serialize");
        let parsed: AuditEvent = serde_json::from_str(json.as_str()).expect("parse");
        assert!(parsed.actor.is_some());
        assert!(parsed.action.is_some());
        assert!(parsed.result.is_some());
    }

    #[test]
    fn audit_logger_disabled_does_not_create_file() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: false,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;
        let event = AuditEvent::new(AuditEventType::CommandExecution);

        logger.log(&event)?;

        // File should not exist since logging is disabled
        assert!(!tmp.path().join("audit.log").exists());
        Ok(())
    }

    // ── §8.1 Log rotation tests ─────────────────────────────

    #[tokio::test]
    async fn audit_logger_writes_event_when_enabled() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 10,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;
        let event = AuditEvent::new(AuditEventType::CommandExecution)
            .with_actor("cli".to_string(), None, None)
            .with_action("ls".to_string(), "low".to_string(), false, true);

        logger.log(&event)?;

        let log_path = tmp.path().join("audit.log");
        assert!(log_path.exists(), "audit log file must be created");

        let content = tokio::fs::read_to_string(&log_path).await?;
        assert!(!content.is_empty(), "audit log must not be empty");

        let parsed: AuditEvent = serde_json::from_str(content.trim())?;
        assert!(parsed.action.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn audit_log_command_event_writes_structured_entry() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 10,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        logger.log_command_event(CommandExecutionLog {
            channel: "telegram",
            command: "echo test",
            risk_level: "low",
            approved: false,
            allowed: true,
            success: true,
            duration_ms: 42,
        })?;

        let log_path = tmp.path().join("audit.log");
        let content = tokio::fs::read_to_string(&log_path).await?;
        let parsed: AuditEvent = serde_json::from_str(content.trim())?;

        let action = parsed.action.unwrap();
        assert_eq!(action.command, Some("echo test".to_string()));
        assert_eq!(action.risk_level, Some("low".to_string()));
        assert!(action.allowed);

        let result = parsed.result.unwrap();
        assert!(result.success);
        assert_eq!(result.duration_ms, Some(42));
        Ok(())
    }

    #[test]
    fn audit_rotation_creates_numbered_backup() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 0, // Force rotation on first write
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        // Write initial content that triggers rotation
        let log_path = tmp.path().join("audit.log");
        std::fs::write(&log_path, "initial content\n")?;

        let event = AuditEvent::new(AuditEventType::CommandExecution);
        logger.log(&event)?;

        let rotated = format!("{}.1.log", log_path.display());
        assert!(
            std::path::Path::new(&rotated).exists(),
            "rotation must create .1.log backup"
        );
        Ok(())
    }

    // ── Merkle hash-chain tests ─────────────────────────────

    #[test]
    fn merkle_chain_genesis_uses_well_known_seed() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 10,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        let event = AuditEvent::new(AuditEventType::SecurityEvent);
        logger.log(&event)?;

        let log_path = tmp.path().join("audit.log");
        let content = std::fs::read_to_string(&log_path)?;
        let parsed: AuditEvent = serde_json::from_str(content.trim())?;

        assert_eq!(parsed.sequence, 0);
        assert_eq!(parsed.prev_hash, GENESIS_PREV_HASH);
        assert!(!parsed.entry_hash.is_empty());
        Ok(())
    }

    #[test]
    fn merkle_chain_multiple_entries_verify() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 10,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        // Write several events
        for i in 0..5 {
            let event = AuditEvent::new(AuditEventType::CommandExecution).with_action(
                format!("cmd-{}", i),
                "low".to_string(),
                false,
                true,
            );
            logger.log(&event)?;
        }

        let log_path = tmp.path().join("audit.log");
        let count = verify_chain(&log_path)?;
        assert_eq!(count, 5);
        Ok(())
    }

    #[test]
    fn merkle_chain_detects_tampered_entry() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 10,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        for i in 0..3 {
            let event = AuditEvent::new(AuditEventType::CommandExecution).with_action(
                format!("cmd-{}", i),
                "low".to_string(),
                false,
                true,
            );
            logger.log(&event)?;
        }

        // Tamper with the second entry (change the command text)
        let log_path = tmp.path().join("audit.log");
        let content = std::fs::read_to_string(&log_path)?;
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);

        let mut entry: serde_json::Value = serde_json::from_str(lines[1])?;
        entry["action"]["command"] = serde_json::Value::String("TAMPERED".to_string());
        let tampered_line = serde_json::to_string(&entry)?;

        let tampered_content = format!("{}\n{}\n{}\n", lines[0], tampered_line, lines[2]);
        std::fs::write(&log_path, tampered_content)?;

        // Verification must fail
        let result = verify_chain(&log_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("entry_hash mismatch"),
            "expected entry_hash mismatch, got: {}",
            err_msg
        );
        Ok(())
    }

    #[test]
    fn merkle_chain_detects_sequence_gap() -> Result<()> {
        let tmp = TempDir::new()?;
        let config = AuditConfig {
            enabled: true,
            max_size_mb: 10,
            ..Default::default()
        };
        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        for i in 0..3 {
            let event = AuditEvent::new(AuditEventType::CommandExecution).with_action(
                format!("cmd-{}", i),
                "low".to_string(),
                false,
                true,
            );
            logger.log(&event)?;
        }

        // Remove the second entry to create a sequence gap
        let log_path = tmp.path().join("audit.log");
        let content = std::fs::read_to_string(&log_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let gapped_content = format!("{}\n{}\n", lines[0], lines[2]);
        std::fs::write(&log_path, gapped_content)?;

        let result = verify_chain(&log_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("sequence gap"),
            "expected sequence gap, got: {}",
            err_msg
        );
        Ok(())
    }

    // ── HMAC chain tests ────────────────────────────────────────

    #[test]
    fn hmac_chain_write_and_verify() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let config = AuditConfig {
            enabled: true,
            log_path: "audit.log".into(),
            max_size_mb: 100,
            sign_events: true,
        };

        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;

        for _ in 0..3 {
            logger.log(&AuditEvent::new(AuditEventType::SecurityEvent))?;
        }

        let log_path = tmp.path().join("audit.log");
        let key_path = tmp.path().join("audit.key");
        let verified = verify_audit_chain(&log_path, &key_path)?;
        assert_eq!(verified, 3);
        Ok(())
    }

    #[test]
    fn hmac_chain_detects_tampered_entry() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let config = AuditConfig {
            enabled: true,
            log_path: "audit.log".into(),
            max_size_mb: 100,
            sign_events: true,
        };

        let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;
        logger.log(&AuditEvent::new(AuditEventType::SecurityEvent))?;
        logger.log(&AuditEvent::new(AuditEventType::CommandExecution))?;
        drop(logger);

        let log_path = tmp.path().join("audit.log");
        let tampered =
            std::fs::read_to_string(&log_path)?.replace("command_execution", "policy_violation");
        std::fs::write(&log_path, tampered)?;

        let key_path = tmp.path().join("audit.key");
        let result = verify_audit_chain(&log_path, &key_path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("chain broken"));
        Ok(())
    }

    #[test]
    fn hmac_chain_continues_after_restart() -> Result<()> {
        let tmp = TempDir::new().unwrap();
        let config = AuditConfig {
            enabled: true,
            log_path: "audit.log".into(),
            max_size_mb: 100,
            sign_events: true,
        };

        {
            let logger = AuditLogger::new(config.clone(), tmp.path().to_path_buf())?;
            logger.log(&AuditEvent::new(AuditEventType::SecurityEvent))?;
            logger.log(&AuditEvent::new(AuditEventType::IpcSend))?;
        }

        {
            let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;
            logger.log(&AuditEvent::new(AuditEventType::IpcBlocked))?;
        }

        let log_path = tmp.path().join("audit.log");
        let key_path = tmp.path().join("audit.key");
        let verified = verify_audit_chain(&log_path, &key_path)?;
        assert_eq!(verified, 3);
        Ok(())
    }

    // ── Merkle chain recovery test ─────────────────────────────

    #[test]
    fn merkle_chain_recovery_continues_after_restart() -> Result<()> {
        let tmp = TempDir::new()?;
        let log_path = tmp.path().join("audit.log");

        // First logger writes 2 entries
        {
            let config = AuditConfig {
                enabled: true,
                max_size_mb: 10,
                ..Default::default()
            };
            let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;
            for i in 0..2 {
                let event = AuditEvent::new(AuditEventType::CommandExecution).with_action(
                    format!("batch1-{}", i),
                    "low".to_string(),
                    false,
                    true,
                );
                logger.log(&event)?;
            }
        }

        // Second logger (simulating restart) continues the chain
        {
            let config = AuditConfig {
                enabled: true,
                max_size_mb: 10,
                ..Default::default()
            };
            let logger = AuditLogger::new(config, tmp.path().to_path_buf())?;
            for i in 0..2 {
                let event = AuditEvent::new(AuditEventType::CommandExecution).with_action(
                    format!("batch2-{}", i),
                    "low".to_string(),
                    false,
                    true,
                );
                logger.log(&event)?;
            }
        }

        // Full chain should verify (4 entries, sequences 0..3)
        let count = verify_chain(&log_path)?;
        assert_eq!(count, 4);
        Ok(())
    }
}
