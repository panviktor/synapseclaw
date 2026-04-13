//! Shared concrete agent-runtime assembly for web and channel ingress.
//!
//! This stays in adapter-core because it wires provider caches, tool registry,
//! hooks, approval, and observability into the domain `AgentRuntimePort`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::hooks::HookRunner;
use crate::runtime::agent_runtime_adapter::ChannelAgentRuntime;
use crate::runtime_routes::WorkspaceModelProfileCatalog;
use crate::tools::{ActivatedToolSet, Tool};
use synapse_domain::config::schema::{MultimodalConfig, ReliabilityConfig};
use synapse_infra::approval::ApprovalManager;
use synapse_observability::Observer;
use synapse_providers::{Provider, ProviderRuntimeOptions};

pub(crate) struct ChannelAgentRuntimeInput {
    pub provider: Arc<dyn Provider>,
    pub default_provider_name: String,
    pub default_api_key: Option<String>,
    pub default_api_url: Option<String>,
    pub provider_cache: Arc<Mutex<HashMap<String, Arc<dyn Provider>>>>,
    pub reliability: ReliabilityConfig,
    pub provider_runtime_options: ProviderRuntimeOptions,
    pub workspace_dir: PathBuf,
    pub tools_registry: Arc<Vec<Box<dyn Tool>>>,
    pub observer: Arc<dyn Observer>,
    pub approval_manager: Arc<ApprovalManager>,
    pub channel_name: String,
    pub multimodal: MultimodalConfig,
    pub excluded_tools: Arc<Vec<String>>,
    pub dedup_exempt_tools: Arc<Vec<String>>,
    pub hooks: Option<Arc<HookRunner>>,
    pub activated_tools: Option<Arc<Mutex<ActivatedToolSet>>>,
    pub message_timeout_secs: u64,
    pub max_tool_iterations: usize,
}

pub(crate) struct ChannelAgentRuntimeFactory;

impl ChannelAgentRuntimeFactory {
    pub(crate) fn build(input: ChannelAgentRuntimeInput) -> ChannelAgentRuntime {
        ChannelAgentRuntime {
            provider: input.provider,
            default_provider_name: input.default_provider_name.clone(),
            default_api_key: input.default_api_key,
            default_api_url: input.default_api_url.clone(),
            provider_cache: input.provider_cache,
            reliability: input.reliability,
            provider_runtime_options: input.provider_runtime_options,
            model_profile_catalog: Some(Arc::new(
                WorkspaceModelProfileCatalog::with_provider_endpoint(
                    input.workspace_dir,
                    Some(input.default_provider_name.as_str()),
                    input.default_api_url.as_deref(),
                ),
            )),
            tools_registry: input.tools_registry,
            observer: input.observer,
            approval_manager: input.approval_manager,
            channel_name: input.channel_name,
            multimodal: input.multimodal,
            excluded_tools: input.excluded_tools,
            dedup_exempt_tools: input.dedup_exempt_tools,
            hooks: input.hooks,
            activated_tools: input.activated_tools,
            message_timeout_secs: input.message_timeout_secs,
            max_tool_iterations: input.max_tool_iterations,
        }
    }
}
