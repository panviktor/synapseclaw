//! Shared inbound-runtime port assembly for web and channel transports.
//!
//! Transport adapters still own concrete side effects, but the runtime use-case
//! receives one unified port bundle.

use std::sync::Arc;

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use synapse_channels::inbound::{
    conversation_history_adapter, conversation_store_adapter, route_selection_adapter,
};
use synapse_domain::application::services::dialogue_state_service::DialogueStateStore;
use synapse_domain::application::use_cases::handle_inbound_message::InboundMessagePorts;
use synapse_domain::domain::message::ChatMessage;
use synapse_domain::ports::agent_runtime::AgentRuntimePort;
use synapse_domain::ports::channel_output::ChannelOutputPort;
use synapse_domain::ports::channel_registry::ChannelRegistryPort;
use synapse_domain::ports::conversation_context::ConversationContextPort;
use synapse_domain::ports::conversation_history::ConversationHistoryPort;
use synapse_domain::ports::conversation_store::ConversationStorePort;
use synapse_domain::ports::hooks::HooksPort;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::model_profile_catalog::ModelProfileCatalogPort;
use synapse_domain::ports::route_selection::RouteSelection;
use synapse_domain::ports::route_selection::RouteSelectionPort;
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;
use synapse_domain::ports::scoped_instruction_context::ScopedInstructionContextPort;
use synapse_domain::ports::session_summary::SessionSummaryPort;
use synapse_domain::ports::turn_defaults_context::TurnDefaultsContextPort;
use synapse_domain::ports::user_profile_store::UserProfileStorePort;

type ConversationHistoryMap = Arc<Mutex<HashMap<String, Vec<ChatMessage>>>>;
type RouteSelectionMap = Arc<Mutex<HashMap<String, RouteSelection>>>;
type ChannelSessionBackend = Arc<dyn crate::channels::session_backend::SessionBackend>;

pub(crate) struct InboundRuntimePortsInput {
    pub history: Arc<dyn ConversationHistoryPort>,
    pub routes: Arc<dyn RouteSelectionPort>,
    pub hooks: Arc<dyn HooksPort>,
    pub channel_output: Arc<dyn ChannelOutputPort>,
    pub agent_runtime: Arc<dyn AgentRuntimePort>,
    pub channel_registry: Arc<dyn ChannelRegistryPort>,
    pub session_summary: Option<Arc<dyn SessionSummaryPort>>,
    pub memory: Option<Arc<dyn UnifiedMemoryPort>>,
    pub event_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
    pub conversation_context: Option<Arc<dyn ConversationContextPort>>,
    pub model_profile_catalog: Option<Arc<dyn ModelProfileCatalogPort>>,
    pub turn_defaults_context: Option<Arc<dyn TurnDefaultsContextPort>>,
    pub scoped_instruction_context: Option<Arc<dyn ScopedInstructionContextPort>>,
    pub conversation_store: Option<Arc<dyn ConversationStorePort>>,
    pub dialogue_state_store: Option<Arc<DialogueStateStore>>,
    pub run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    pub user_profile_store: Option<Arc<dyn UserProfileStorePort>>,
}

pub(crate) struct InboundRuntimePortsFactory;

impl InboundRuntimePortsFactory {
    pub(crate) fn build(input: InboundRuntimePortsInput) -> InboundMessagePorts {
        InboundMessagePorts {
            history: input.history,
            routes: input.routes,
            hooks: input.hooks,
            channel_output: input.channel_output,
            agent_runtime: input.agent_runtime,
            channel_registry: input.channel_registry,
            session_summary: input.session_summary,
            memory: input.memory,
            event_tx: input.event_tx,
            conversation_context: input.conversation_context,
            model_profile_catalog: input.model_profile_catalog,
            turn_defaults_context: input.turn_defaults_context,
            scoped_instruction_context: input.scoped_instruction_context,
            conversation_store: input.conversation_store,
            dialogue_state_store: input.dialogue_state_store,
            run_recipe_store: input.run_recipe_store,
            user_profile_store: input.user_profile_store,
        }
    }
}

pub(crate) struct InboundRuntimeStoreFactory;

impl InboundRuntimeStoreFactory {
    pub(crate) fn history(
        map: ConversationHistoryMap,
        session_store: Option<ChannelSessionBackend>,
    ) -> Arc<dyn ConversationHistoryPort> {
        Arc::new(conversation_history_adapter::MutexMapConversationHistory::new(map, session_store))
    }

    pub(crate) fn routes(
        map: RouteSelectionMap,
        default_provider: impl Into<String>,
        default_model: impl Into<String>,
    ) -> Arc<dyn RouteSelectionPort> {
        Arc::new(route_selection_adapter::MutexMapRouteSelection::new(
            map,
            default_provider.into(),
            default_model.into(),
        ))
    }

    pub(crate) fn conversation_summary(
        conversation_store: Option<Arc<dyn ConversationStorePort>>,
    ) -> Option<Arc<dyn SessionSummaryPort>> {
        conversation_store.map(|store| {
            Arc::new(ConversationStoreSummaryAdapter {
                store,
                key_override: None,
            }) as Arc<dyn SessionSummaryPort>
        })
    }

    pub(crate) fn conversation_summary_for_key(
        conversation_store: Option<Arc<dyn ConversationStorePort>>,
        key: impl Into<String>,
    ) -> Option<Arc<dyn SessionSummaryPort>> {
        let key = key.into();
        conversation_store.map(|store| {
            Arc::new(ConversationStoreSummaryAdapter {
                store,
                key_override: Some(key),
            }) as Arc<dyn SessionSummaryPort>
        })
    }

    pub(crate) fn conversation_store(
        session_store: Option<ChannelSessionBackend>,
    ) -> Option<Arc<dyn ConversationStorePort>> {
        session_store.map(|store| {
            Arc::new(conversation_store_adapter::SessionBackendConversationStore::new(store))
                as Arc<dyn ConversationStorePort>
        })
    }

    pub(crate) fn composite_conversation_store(
        stores: Vec<Option<Arc<dyn ConversationStorePort>>>,
    ) -> Option<Arc<dyn ConversationStorePort>> {
        let mut stores: Vec<Arc<dyn ConversationStorePort>> =
            stores.into_iter().flatten().collect();
        if stores.is_empty() {
            return None;
        }
        if stores.len() == 1 {
            return stores.pop();
        }
        Some(Arc::new(CompositeConversationStore { stores }) as Arc<dyn ConversationStorePort>)
    }
}

struct CompositeConversationStore {
    stores: Vec<Arc<dyn ConversationStorePort>>,
}

impl CompositeConversationStore {
    async fn target_for_key(&self, key: &str) -> Arc<dyn ConversationStorePort> {
        for store in &self.stores {
            if store.get_session(key).await.is_some() {
                return Arc::clone(store);
            }
        }
        Arc::clone(&self.stores[0])
    }
}

#[async_trait::async_trait]
impl ConversationStorePort for CompositeConversationStore {
    async fn get_session(
        &self,
        key: &str,
    ) -> Option<synapse_domain::domain::conversation::ConversationSession> {
        for store in &self.stores {
            if let Some(session) = store.get_session(key).await {
                return Some(session);
            }
        }
        None
    }

    async fn list_sessions(
        &self,
        prefix: Option<&str>,
    ) -> Vec<synapse_domain::domain::conversation::ConversationSession> {
        let mut seen = std::collections::HashSet::new();
        let mut sessions = Vec::new();
        for store in &self.stores {
            for session in store.list_sessions(prefix).await {
                if seen.insert(session.key.clone()) {
                    sessions.push(session);
                }
            }
        }
        sessions
    }

    async fn upsert_session(
        &self,
        session: &synapse_domain::domain::conversation::ConversationSession,
    ) -> anyhow::Result<()> {
        self.target_for_key(&session.key)
            .await
            .upsert_session(session)
            .await
    }

    async fn delete_session(&self, key: &str) -> anyhow::Result<bool> {
        let mut deleted = false;
        for store in &self.stores {
            deleted |= store.delete_session(key).await?;
        }
        Ok(deleted)
    }

    async fn touch_session(&self, key: &str) -> anyhow::Result<()> {
        self.target_for_key(key).await.touch_session(key).await
    }

    async fn append_event(
        &self,
        session_key: &str,
        event: &synapse_domain::domain::conversation::ConversationEvent,
    ) -> anyhow::Result<()> {
        self.target_for_key(session_key)
            .await
            .append_event(session_key, event)
            .await
    }

    async fn get_events(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Vec<synapse_domain::domain::conversation::ConversationEvent> {
        self.target_for_key(session_key)
            .await
            .get_events(session_key, limit)
            .await
    }

    async fn clear_events(&self, session_key: &str) -> anyhow::Result<()> {
        self.target_for_key(session_key)
            .await
            .clear_events(session_key)
            .await
    }

    async fn update_label(&self, key: &str, label: &str) -> anyhow::Result<()> {
        self.target_for_key(key).await.update_label(key, label).await
    }

    async fn update_goal(&self, key: &str, goal: &str) -> anyhow::Result<()> {
        self.target_for_key(key).await.update_goal(key, goal).await
    }

    async fn increment_message_count(&self, key: &str) -> anyhow::Result<()> {
        self.target_for_key(key)
            .await
            .increment_message_count(key)
            .await
    }

    async fn add_token_usage(&self, key: &str, input: i64, output: i64) -> anyhow::Result<()> {
        self.target_for_key(key)
            .await
            .add_token_usage(key, input, output)
            .await
    }

    async fn get_summary(&self, key: &str) -> Option<String> {
        for store in &self.stores {
            if let Some(summary) = store.get_summary(key).await {
                return Some(summary);
            }
        }
        None
    }

    async fn set_summary(&self, key: &str, summary: &str) -> anyhow::Result<()> {
        self.target_for_key(key)
            .await
            .set_summary(key, summary)
            .await
    }
}

struct ConversationStoreSummaryAdapter {
    store: Arc<dyn ConversationStorePort>,
    key_override: Option<String>,
}

impl SessionSummaryPort for ConversationStoreSummaryAdapter {
    fn load_summary(&self, key: &str) -> Option<String> {
        let key = self.key_override.as_deref().unwrap_or(key).to_string();
        let store = Arc::clone(&self.store);
        block_on_summary_future(async move { store.get_summary(&key).await })
    }

    fn save_summary(&self, key: &str, summary: &str) {
        let key = self.key_override.as_deref().unwrap_or(key).to_string();
        let store = Arc::clone(&self.store);
        let summary = summary.to_string();
        let _ = block_on_summary_future(async move { store.set_summary(&key, &summary).await });
    }
}

fn block_on_summary_future<T>(future: impl Future<Output = T> + Send + 'static) -> T
where
    T: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            return tokio::task::block_in_place(|| handle.block_on(future));
        }
        return std::thread::spawn(move || run_summary_future(future))
            .join()
            .unwrap_or_else(|_| panic!("session summary worker panicked"));
    }

    run_summary_future(future)
}

fn run_summary_future<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build session summary runtime")
        .block_on(future)
}
