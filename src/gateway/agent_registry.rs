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
