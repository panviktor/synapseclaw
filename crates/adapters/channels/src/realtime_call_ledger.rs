use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use synapse_domain::ports::realtime_call::RealtimeCallSessionSnapshot;

const REALTIME_CALL_LEDGER_SCHEMA_VERSION: u32 = 1;
const REALTIME_CALL_LEDGER_FILENAME: &str = "realtime-call-sessions.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRealtimeCallLedger {
    #[serde(default = "realtime_call_ledger_schema_version")]
    schema_version: u32,
    #[serde(default)]
    sessions: Vec<RealtimeCallSessionSnapshot>,
}

impl Default for PersistedRealtimeCallLedger {
    fn default() -> Self {
        Self {
            schema_version: REALTIME_CALL_LEDGER_SCHEMA_VERSION,
            sessions: Vec::new(),
        }
    }
}

fn realtime_call_ledger_schema_version() -> u32 {
    REALTIME_CALL_LEDGER_SCHEMA_VERSION
}

fn realtime_call_ledger_path(synapseclaw_dir: &Path) -> PathBuf {
    synapseclaw_dir
        .join("state")
        .join(REALTIME_CALL_LEDGER_FILENAME)
}

fn load_ledger(path: &Path) -> Result<PersistedRealtimeCallLedger> {
    if !path.exists() {
        return Ok(PersistedRealtimeCallLedger::default());
    }

    let bytes = fs::read(path)
        .with_context(|| format!("failed to read realtime call ledger {}", path.display()))?;
    if bytes.is_empty() {
        return Ok(PersistedRealtimeCallLedger::default());
    }

    let ledger: PersistedRealtimeCallLedger = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse realtime call ledger {}", path.display()))?;
    if ledger.schema_version > REALTIME_CALL_LEDGER_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported realtime call ledger schema version {} (max supported: {})",
            ledger.schema_version,
            REALTIME_CALL_LEDGER_SCHEMA_VERSION
        );
    }
    Ok(ledger)
}

fn persist_ledger(path: &Path, ledger: &PersistedRealtimeCallLedger) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create realtime call ledger directory {}",
                parent.display()
            )
        })?;
    }

    let bytes =
        serde_json::to_vec_pretty(ledger).context("failed to serialize realtime call ledger")?;
    let tmp_path = path.with_file_name(format!(
        ".{REALTIME_CALL_LEDGER_FILENAME}.tmp-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::write(&tmp_path, &bytes).with_context(|| {
        format!(
            "failed to write temporary realtime call ledger {}",
            tmp_path.display()
        )
    })?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("failed to replace realtime call ledger {}", path.display()))?;
    Ok(())
}

pub fn load_persisted_transport_call_sessions(
    synapseclaw_dir: Option<&Path>,
    channel: &str,
) -> Result<Vec<RealtimeCallSessionSnapshot>> {
    let Some(synapseclaw_dir) = synapseclaw_dir else {
        return Ok(Vec::new());
    };
    let ledger = load_ledger(&realtime_call_ledger_path(synapseclaw_dir))?;
    Ok(ledger
        .sessions
        .into_iter()
        .filter(|session| session.channel == channel)
        .collect())
}

pub fn replace_persisted_transport_call_sessions(
    synapseclaw_dir: Option<&Path>,
    channel: &str,
    sessions: &[RealtimeCallSessionSnapshot],
) -> Result<()> {
    let Some(synapseclaw_dir) = synapseclaw_dir else {
        return Ok(());
    };
    let path = realtime_call_ledger_path(synapseclaw_dir);
    let mut ledger = load_ledger(&path)?;
    ledger.sessions.retain(|session| session.channel != channel);
    ledger.sessions.extend_from_slice(sessions);
    persist_ledger(&path, &ledger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::ports::realtime_call::{
        RealtimeCallDirection, RealtimeCallKind, RealtimeCallOrigin, RealtimeCallState,
    };

    fn snapshot(channel: &str, call_control_id: &str) -> RealtimeCallSessionSnapshot {
        RealtimeCallSessionSnapshot {
            channel: channel.into(),
            kind: RealtimeCallKind::Audio,
            direction: RealtimeCallDirection::Outbound,
            origin: RealtimeCallOrigin::cli_request(),
            objective: Some("Smoke".into()),
            call_control_id: call_control_id.into(),
            call_leg_id: None,
            call_session_id: None,
            state: RealtimeCallState::Ringing,
            created_at: "2026-04-21T10:00:00Z".into(),
            updated_at: "2026-04-21T10:00:00Z".into(),
            ended_at: None,
            end_reason: None,
            summary: None,
            decisions: Vec::new(),
            message_count: 0,
            interruption_count: 0,
            last_sequence: None,
        }
    }

    #[test]
    fn replace_transport_sessions_keeps_other_channels() {
        let dir = tempfile::tempdir().unwrap();
        replace_persisted_transport_call_sessions(
            Some(dir.path()),
            "matrix",
            &[snapshot("matrix", "mx-1")],
        )
        .unwrap();
        replace_persisted_transport_call_sessions(
            Some(dir.path()),
            "clawdtalk",
            &[snapshot("clawdtalk", "ct-1")],
        )
        .unwrap();
        replace_persisted_transport_call_sessions(
            Some(dir.path()),
            "matrix",
            &[snapshot("matrix", "mx-2")],
        )
        .unwrap();

        let matrix = load_persisted_transport_call_sessions(Some(dir.path()), "matrix").unwrap();
        let clawdtalk =
            load_persisted_transport_call_sessions(Some(dir.path()), "clawdtalk").unwrap();

        assert_eq!(matrix.len(), 1);
        assert_eq!(matrix[0].call_control_id, "mx-2");
        assert_eq!(clawdtalk.len(), 1);
        assert_eq!(clawdtalk[0].call_control_id, "ct-1");
    }
}
