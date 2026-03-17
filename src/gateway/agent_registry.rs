//! Agent registry for broker-centered multi-agent dashboard (Phase 3.8).
//!
//! Tracks registered agent daemons with their gateway URLs, live status,
//! and metadata. Separate from `NodeRegistry` which handles ephemeral
//! capability nodes — different trust model, lifecycle, and semantics.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Live status of a registered agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Online,
    Offline,
    Error,
}

/// A registered agent with live metadata.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub gateway_url: String,
    pub proxy_token: String,
    pub trust_level: Option<u8>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub status: AgentStatus,
    pub last_seen: i64,
    pub uptime_seconds: Option<u64>,
    pub channels: Vec<String>,
    /// Consecutive failed health polls.
    #[serde(skip)]
    pub missed_polls: u32,
}

/// Registry of agent daemons known to the broker.
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentInfo>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register or update an agent's gateway info.
    pub fn upsert(&self, agent_id: &str, gateway_url: &str, proxy_token: &str) {
        let now = chrono::Utc::now().timestamp();
        let mut agents = self.agents.write();
        let entry = agents
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentInfo {
                agent_id: agent_id.to_string(),
                gateway_url: gateway_url.to_string(),
                proxy_token: proxy_token.to_string(),
                trust_level: None,
                role: None,
                model: None,
                status: AgentStatus::Online,
                last_seen: now,
                uptime_seconds: None,
                channels: Vec::new(),
                missed_polls: 0,
            });
        entry.gateway_url = gateway_url.to_string();
        entry.proxy_token = proxy_token.to_string();
        entry.status = AgentStatus::Online;
        entry.last_seen = now;
        entry.missed_polls = 0;
    }

    /// Update agent metadata from a successful health/status poll.
    pub fn update_metadata(
        &self,
        agent_id: &str,
        model: Option<String>,
        uptime: Option<u64>,
        channels: Vec<String>,
    ) {
        let now = chrono::Utc::now().timestamp();
        let mut agents = self.agents.write();
        if let Some(info) = agents.get_mut(agent_id) {
            info.model = model;
            info.uptime_seconds = uptime;
            info.channels = channels;
            info.status = AgentStatus::Online;
            info.last_seen = now;
            info.missed_polls = 0;
        }
    }

    /// Record a failed health poll. After 3 consecutive failures → offline.
    pub fn record_poll_failure(&self, agent_id: &str) {
        let mut agents = self.agents.write();
        if let Some(info) = agents.get_mut(agent_id) {
            info.missed_polls += 1;
            if info.missed_polls >= 3 {
                info.status = AgentStatus::Offline;
            }
        }
    }

    /// Enrich with trust metadata from IPC TokenMetadata.
    pub fn set_trust_info(&self, agent_id: &str, trust_level: u8, role: &str) {
        let mut agents = self.agents.write();
        if let Some(info) = agents.get_mut(agent_id) {
            info.trust_level = Some(trust_level);
            info.role = Some(role.to_string());
        }
    }

    /// Get a specific agent's info.
    pub fn get(&self, agent_id: &str) -> Option<AgentInfo> {
        self.agents.read().get(agent_id).cloned()
    }

    /// List all registered agents.
    pub fn list(&self) -> Vec<AgentInfo> {
        self.agents.read().values().cloned().collect()
    }

    /// Remove an agent.
    pub fn remove(&self, agent_id: &str) {
        self.agents.write().remove(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_get() {
        let reg = AgentRegistry::new();
        reg.upsert("opus", "http://127.0.0.1:42618", "zc_proxy_test");
        let info = reg.get("opus").unwrap();
        assert_eq!(info.agent_id, "opus");
        assert_eq!(info.gateway_url, "http://127.0.0.1:42618");
        assert_eq!(info.status, AgentStatus::Online);
        assert_eq!(info.missed_polls, 0);
    }

    #[test]
    fn upsert_resets_status_and_missed_polls() {
        let reg = AgentRegistry::new();
        reg.upsert("opus", "http://127.0.0.1:42618", "zc_proxy_test");
        reg.record_poll_failure("opus");
        reg.record_poll_failure("opus");
        reg.record_poll_failure("opus");
        assert_eq!(reg.get("opus").unwrap().status, AgentStatus::Offline);

        // Re-register should reset
        reg.upsert("opus", "http://127.0.0.1:42618", "zc_proxy_new");
        let info = reg.get("opus").unwrap();
        assert_eq!(info.status, AgentStatus::Online);
        assert_eq!(info.missed_polls, 0);
        assert_eq!(info.proxy_token, "zc_proxy_new");
    }

    #[test]
    fn offline_after_three_failures() {
        let reg = AgentRegistry::new();
        reg.upsert("daily", "http://127.0.0.1:42619", "zc_proxy_d");
        assert_eq!(reg.get("daily").unwrap().status, AgentStatus::Online);

        reg.record_poll_failure("daily");
        assert_eq!(reg.get("daily").unwrap().status, AgentStatus::Online);
        reg.record_poll_failure("daily");
        assert_eq!(reg.get("daily").unwrap().status, AgentStatus::Online);
        reg.record_poll_failure("daily");
        assert_eq!(reg.get("daily").unwrap().status, AgentStatus::Offline);
    }

    #[test]
    fn update_metadata_resets_missed_polls() {
        let reg = AgentRegistry::new();
        reg.upsert("code", "http://127.0.0.1:42620", "zc_proxy_c");
        reg.record_poll_failure("code");
        reg.record_poll_failure("code");
        assert_eq!(reg.get("code").unwrap().missed_polls, 2);

        reg.update_metadata(
            "code",
            Some("claude-sonnet".into()),
            Some(3600),
            vec!["matrix".into()],
        );
        let info = reg.get("code").unwrap();
        assert_eq!(info.missed_polls, 0);
        assert_eq!(info.status, AgentStatus::Online);
        assert_eq!(info.model.as_deref(), Some("claude-sonnet"));
        assert_eq!(info.channels, vec!["matrix"]);
    }

    #[test]
    fn set_trust_info() {
        let reg = AgentRegistry::new();
        reg.upsert("opus", "http://127.0.0.1:42618", "t");
        reg.set_trust_info("opus", 1, "coordinator");
        let info = reg.get("opus").unwrap();
        assert_eq!(info.trust_level, Some(1));
        assert_eq!(info.role.as_deref(), Some("coordinator"));
    }

    #[test]
    fn list_and_remove() {
        let reg = AgentRegistry::new();
        reg.upsert("a", "http://a", "ta");
        reg.upsert("b", "http://b", "tb");
        assert_eq!(reg.list().len(), 2);

        reg.remove("a");
        assert_eq!(reg.list().len(), 1);
        assert!(reg.get("a").is_none());
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let reg = AgentRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }
}
