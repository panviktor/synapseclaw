//! Runtime-command adapter contract.
//!
//! Web and channel adapters intentionally keep different transports and
//! lifecycles, but must not fork the common runtime-command decisions.

use std::sync::Arc;

use synapse_domain::application::services::assistant_output_presentation::{
    AssistantOutputPresenter, OutputDeliveryHints, PresentedOutput,
};
use synapse_domain::application::services::capability_doctor::CapabilityDoctorReport;
use synapse_domain::application::services::inbound_message_service::{
    CommandEffect, RuntimeSkillStatusView, RuntimeUserSkillCreateMetadata,
};
use synapse_domain::application::services::model_lane_resolution::ResolvedModelProfile;
use synapse_domain::application::services::route_switch_preflight::RouteSwitchPreflight;
use synapse_domain::application::services::runtime_command_presentation::{
    format_clear_session_response, format_common_command_effect, format_compact_session_response,
    format_provider_initialization_failure, format_switch_model_blocked,
    format_switch_model_failure, format_switch_model_success, format_switch_provider_success,
    format_unknown_provider, RuntimeCommandPresentationOptions,
};
use synapse_domain::config::schema::{CapabilityLane, Config};
use synapse_domain::domain::memory::SkillStatus;
use synapse_domain::ports::conversation_history::ConversationHistoryPort;
use synapse_domain::ports::memory::UnifiedMemoryPort;
use synapse_domain::ports::route_selection::RouteSelection;
use synapse_domain::ports::run_recipe_store::RunRecipeStorePort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeAdapterSurface {
    Web,
    Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeAdapterTransport {
    WebSocket,
    ChannelMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeAdapterLifecycle {
    LiveAgentSession,
    InboundSessionBackend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeDecisionOwner {
    Domain,
    AdapterCore,
    AdapterLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeDecisionOwnership {
    pub route_preflight: RuntimeDecisionOwner,
    pub context_budget: RuntimeDecisionOwner,
    pub route_diagnostics: RuntimeDecisionOwner,
    pub command_effects: RuntimeDecisionOwner,
    pub formatting_primitives: RuntimeDecisionOwner,
    pub provider_aliases: RuntimeDecisionOwner,
    pub provider_initialization: RuntimeDecisionOwner,
    pub transport_lifecycle: RuntimeDecisionOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeAdapterCapabilities {
    pub live_agent_session: bool,
    pub inbound_session_backend: bool,
    pub route_cache_stats: bool,
    pub route_mutation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeAdapterDescriptor {
    pub surface: RuntimeAdapterSurface,
    pub transport: RuntimeAdapterTransport,
    pub lifecycle: RuntimeAdapterLifecycle,
    pub decisions: RuntimeDecisionOwnership,
    pub capabilities: RuntimeAdapterCapabilities,
}

pub(crate) trait RuntimeAdapterContract {
    fn descriptor(&self) -> RuntimeAdapterDescriptor;

    fn presentation_options(&self, default_provider: &str) -> RuntimeCommandPresentationOptions {
        RuntimeCommandPresentationOptions::new(default_provider)
    }

    fn canonical_provider(&self, provider: &str) -> Option<String> {
        crate::runtime_routes::resolve_provider_alias(provider)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuntimeRouteMutationRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub lane: Option<CapabilityLane>,
    pub candidate_index: Option<usize>,
    pub target_context_window_tokens: Option<usize>,
}

impl RuntimeRouteMutationRequest {
    fn provider(provider: impl Into<String>) -> Self {
        Self {
            provider: Some(provider.into()),
            ..Self::default()
        }
    }

    fn model(
        provider: impl Into<String>,
        model: impl Into<String>,
        lane: Option<CapabilityLane>,
    ) -> Self {
        Self {
            provider: Some(provider.into()),
            model: Some(model.into()),
            lane,
            ..Self::default()
        }
    }

    fn with_candidate_index(mut self, candidate_index: Option<usize>) -> Self {
        self.candidate_index = candidate_index;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeProviderSwitchOutcome {
    pub provider: String,
    pub already_current: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum RuntimeModelSwitchOutcome {
    Applied {
        provider: String,
        lane: Option<CapabilityLane>,
        compacted: bool,
    },
    Blocked {
        provider: String,
        lane: Option<CapabilityLane>,
        compacted: bool,
        preflight: RouteSwitchPreflight,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeModelHelpSnapshot {
    pub route: RouteSelection,
    pub config: Config,
}

pub(crate) struct RuntimeSessionCompactionTarget {
    pub history: Arc<dyn ConversationHistoryPort>,
    pub conversation_key: String,
    pub target_profile: ResolvedModelProfile,
    pub memory: Option<Arc<dyn UnifiedMemoryPort>>,
    pub run_recipe_store: Option<Arc<dyn RunRecipeStorePort>>,
    pub agent_id: String,
    pub surface: crate::runtime_history_hygiene::RuntimeCompactionSurface,
}

#[async_trait::async_trait]
pub(crate) trait RuntimeCommandHost {
    fn current_provider(&self) -> String;

    async fn provider_help_route(&mut self) -> anyhow::Result<RouteSelection>;

    async fn model_help_snapshot(&mut self) -> anyhow::Result<RuntimeModelHelpSnapshot>;

    async fn capability_doctor_report(&mut self) -> anyhow::Result<CapabilityDoctorReport>;

    async fn skill_status_output(&mut self, view: RuntimeSkillStatusView)
        -> anyhow::Result<String>;

    async fn skill_tool_contracts_output(&mut self) -> anyhow::Result<String>;

    async fn skill_use_traces_output(&mut self) -> anyhow::Result<String>;

    async fn skill_health_output(&mut self, apply: bool) -> anyhow::Result<String>;

    async fn skill_patch_diff_output(&mut self, candidate: &str) -> anyhow::Result<String>;

    async fn apply_skill_patch_output(&mut self, candidate: &str) -> anyhow::Result<String>;

    async fn skill_patch_versions_output(
        &mut self,
        skill_ref: Option<&str>,
    ) -> anyhow::Result<String>;

    async fn create_user_skill_output(
        &mut self,
        name: &str,
        body: &str,
        metadata: &RuntimeUserSkillCreateMetadata,
    ) -> anyhow::Result<String>;

    async fn update_user_skill_output(
        &mut self,
        skill_ref: &str,
        body: &str,
        metadata: &RuntimeUserSkillCreateMetadata,
    ) -> anyhow::Result<String>;

    async fn rollback_skill_patch_output(&mut self, rollback_ref: &str) -> anyhow::Result<String>;

    async fn skill_auto_promotion_output(&mut self, apply: bool) -> anyhow::Result<String>;

    async fn skill_review_output(&mut self, apply: bool) -> anyhow::Result<String>;

    async fn update_skill_status_output(
        &mut self,
        skill_ref: &str,
        target_status: SkillStatus,
    ) -> anyhow::Result<String>;

    async fn switch_provider(
        &mut self,
        request: RuntimeRouteMutationRequest,
    ) -> anyhow::Result<RuntimeProviderSwitchOutcome>;

    async fn switch_model(
        &mut self,
        request: RuntimeRouteMutationRequest,
        compacted: bool,
    ) -> anyhow::Result<RuntimeModelSwitchOutcome>;

    async fn session_compaction_target(&mut self)
        -> anyhow::Result<RuntimeSessionCompactionTarget>;

    async fn clear_session(&mut self) -> anyhow::Result<()>;
}

pub(crate) async fn execute_runtime_command_output<C, H>(
    contract: &C,
    host: &mut H,
    effect: &CommandEffect,
    default_provider: &str,
    delivery_hints: OutputDeliveryHints,
) -> anyhow::Result<PresentedOutput>
where
    C: RuntimeAdapterContract,
    H: RuntimeCommandHost,
{
    let text = execute_runtime_command_effect(contract, host, effect, default_provider).await?;
    Ok(AssistantOutputPresenter::success(
        text,
        Vec::new(),
        String::new(),
        false,
        delivery_hints,
    ))
}

pub(crate) async fn execute_runtime_command_effect<C, H>(
    contract: &C,
    host: &mut H,
    effect: &CommandEffect,
    default_provider: &str,
) -> anyhow::Result<String>
where
    C: RuntimeAdapterContract,
    H: RuntimeCommandHost,
{
    let presentation_options = contract.presentation_options(default_provider);
    match effect {
        CommandEffect::ShowProviders => {
            let route = host.provider_help_route().await?;
            Ok(crate::runtime_routes::build_providers_help_response(&route))
        }
        CommandEffect::ShowModel => {
            let snapshot = host.model_help_snapshot().await?;
            Ok(crate::runtime_routes::build_models_help_response(
                &snapshot.route,
                &snapshot.config,
            ))
        }
        CommandEffect::ShowDoctor => {
            let report = host.capability_doctor_report().await?;
            Ok(crate::runtime_routes::build_capability_doctor_response(
                &report,
            ))
        }
        CommandEffect::ShowSkills { view } => host.skill_status_output(*view).await,
        CommandEffect::ShowSkillTools => host.skill_tool_contracts_output().await,
        CommandEffect::ShowSkillTraces => host.skill_use_traces_output().await,
        CommandEffect::ShowSkillHealth { apply } => host.skill_health_output(*apply).await,
        CommandEffect::ShowSkillDiff { candidate } => host.skill_patch_diff_output(candidate).await,
        CommandEffect::ApplySkillPatch { candidate } => {
            host.apply_skill_patch_output(candidate).await
        }
        CommandEffect::ShowSkillVersions { skill } => {
            host.skill_patch_versions_output(skill.as_deref()).await
        }
        CommandEffect::CreateUserSkill {
            name,
            body,
            metadata,
        } => host.create_user_skill_output(name, body, metadata).await,
        CommandEffect::UpdateUserSkill {
            skill,
            body,
            metadata,
        } => host.update_user_skill_output(skill, body, metadata).await,
        CommandEffect::RollbackSkillPatch { rollback } => {
            host.rollback_skill_patch_output(rollback).await
        }
        CommandEffect::AutoPromoteSkills { apply } => {
            host.skill_auto_promotion_output(*apply).await
        }
        CommandEffect::ReviewSkills { apply } => host.skill_review_output(*apply).await,
        CommandEffect::UpdateSkillStatus { skill, status } => {
            host.update_skill_status_output(skill, status.clone()).await
        }
        CommandEffect::SwitchProvider { provider } => match contract.canonical_provider(provider) {
            Some(provider_name) => match host
                .switch_provider(RuntimeRouteMutationRequest::provider(provider_name.clone()))
                .await
            {
                Ok(outcome) => Ok(format_switch_provider_success(
                    &outcome.provider,
                    &presentation_options,
                )),
                Err(error) => {
                    let safe_error = synapse_providers::sanitize_api_error(&error.to_string());
                    Ok(format_provider_initialization_failure(
                        &provider_name,
                        &safe_error,
                    ))
                }
            },
            None => Ok(format_unknown_provider(provider)),
        },
        CommandEffect::SwitchModel {
            model,
            inferred_provider,
            lane,
            candidate_index,
            compacted,
        } => {
            let provider = inferred_provider
                .clone()
                .unwrap_or_else(|| host.current_provider());
            if model.is_empty() {
                return Ok(format_switch_model_success(
                    model,
                    &provider,
                    *lane,
                    *compacted,
                    &presentation_options,
                ));
            }
            let request =
                RuntimeRouteMutationRequest::model(provider.clone(), model.clone(), *lane)
                    .with_candidate_index(*candidate_index);
            match host.switch_model(request, *compacted).await {
                Ok(RuntimeModelSwitchOutcome::Applied {
                    provider,
                    lane,
                    compacted,
                }) => Ok(format_switch_model_success(
                    model,
                    &provider,
                    lane,
                    compacted,
                    &presentation_options,
                )),
                Ok(RuntimeModelSwitchOutcome::Blocked {
                    provider,
                    lane,
                    compacted,
                    preflight,
                }) => Ok(format_switch_model_blocked(
                    model,
                    &provider,
                    lane,
                    &preflight,
                    compacted,
                    &presentation_options,
                )),
                Err(error) => {
                    let safe_error = synapse_providers::sanitize_api_error(&error.to_string());
                    Ok(format_switch_model_failure(model, &provider, &safe_error))
                }
            }
        }
        CommandEffect::SwitchModelBlocked { .. } => {
            Ok(format_common_command_effect(effect, &presentation_options)
                .expect("common command formatter should handle blocked model switches"))
        }
        CommandEffect::CompactSession { .. } => {
            let target = host.session_compaction_target().await?;
            let memory = target.memory.as_ref().map(|memory| memory.as_ref());
            let run_recipe_store = target.run_recipe_store.as_ref().map(|store| store.as_ref());
            let outcome = crate::runtime_history_hygiene::compact_history_for_runtime_command(
                target.history.as_ref(),
                &target.conversation_key,
                &target.target_profile,
                memory,
                run_recipe_store,
                &target.agent_id,
                target.surface,
            )
            .await;
            Ok(format_compact_session_response(outcome.compacted))
        }
        CommandEffect::ClearSession => {
            host.clear_session().await?;
            Ok(format_clear_session_response())
        }
    }
}

pub(crate) struct WebRuntimeAdapterContract;

pub(crate) struct ChannelRuntimeAdapterContract;

const COMMON_RUNTIME_DECISIONS: RuntimeDecisionOwnership = RuntimeDecisionOwnership {
    route_preflight: RuntimeDecisionOwner::Domain,
    context_budget: RuntimeDecisionOwner::Domain,
    route_diagnostics: RuntimeDecisionOwner::Domain,
    command_effects: RuntimeDecisionOwner::Domain,
    formatting_primitives: RuntimeDecisionOwner::Domain,
    provider_aliases: RuntimeDecisionOwner::AdapterCore,
    provider_initialization: RuntimeDecisionOwner::AdapterLifecycle,
    transport_lifecycle: RuntimeDecisionOwner::AdapterLifecycle,
};

impl RuntimeAdapterContract for WebRuntimeAdapterContract {
    fn descriptor(&self) -> RuntimeAdapterDescriptor {
        RuntimeAdapterDescriptor {
            surface: RuntimeAdapterSurface::Web,
            transport: RuntimeAdapterTransport::WebSocket,
            lifecycle: RuntimeAdapterLifecycle::LiveAgentSession,
            decisions: COMMON_RUNTIME_DECISIONS,
            capabilities: RuntimeAdapterCapabilities {
                live_agent_session: true,
                inbound_session_backend: false,
                route_cache_stats: true,
                route_mutation: true,
            },
        }
    }
}

impl RuntimeAdapterContract for ChannelRuntimeAdapterContract {
    fn descriptor(&self) -> RuntimeAdapterDescriptor {
        RuntimeAdapterDescriptor {
            surface: RuntimeAdapterSurface::Channel,
            transport: RuntimeAdapterTransport::ChannelMessage,
            lifecycle: RuntimeAdapterLifecycle::InboundSessionBackend,
            decisions: COMMON_RUNTIME_DECISIONS,
            capabilities: RuntimeAdapterCapabilities {
                live_agent_session: false,
                inbound_session_backend: true,
                route_cache_stats: true,
                route_mutation: true,
            },
        }
    }
}

pub(crate) fn runtime_adapter_descriptors() -> [RuntimeAdapterDescriptor; 2] {
    [
        WebRuntimeAdapterContract.descriptor(),
        ChannelRuntimeAdapterContract.descriptor(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use synapse_domain::application::services::capability_doctor::{
        CapabilityDoctorModelProfileSnapshot, CapabilityDoctorNode, CapabilityDoctorReadiness,
        CapabilityDoctorRouteSnapshot, CapabilityDoctorSeverity, CapabilityDoctorSubsystem,
        CapabilityDoctorSummary,
    };
    use synapse_domain::application::services::inbound_message_service::CommandEffect;
    use synapse_domain::application::services::route_switch_preflight::{
        RouteSwitchPreflight, RouteSwitchStatus,
    };
    use synapse_domain::application::services::runtime_command_presentation::{
        format_common_command_effect, RuntimeCommandPresentationOptions,
    };
    use synapse_domain::config::schema::CapabilityLane;

    const FORBIDDEN_ADAPTER_PRESENTATION_LITERALS: &[&str] = &[
        "Provider switched to",
        "Unknown provider",
        "Failed to initialize provider",
        "Model switched to",
        "Model switch to",
        "Conversation history cleared",
        "Target safe context budget",
        "Context preserved",
        "Context compacted before switching",
        "Compaction ran first",
    ];

    #[test]
    fn web_and_channel_share_common_runtime_decision_owners() {
        let [web, channel] = runtime_adapter_descriptors();

        assert_eq!(web.decisions, channel.decisions);
        assert_eq!(web.decisions.route_preflight, RuntimeDecisionOwner::Domain);
        assert_eq!(web.decisions.context_budget, RuntimeDecisionOwner::Domain);
        assert_eq!(
            web.decisions.route_diagnostics,
            RuntimeDecisionOwner::Domain
        );
        assert_eq!(web.decisions.command_effects, RuntimeDecisionOwner::Domain);
        assert_eq!(
            web.decisions.formatting_primitives,
            RuntimeDecisionOwner::Domain
        );
        assert_eq!(
            web.decisions.provider_aliases,
            RuntimeDecisionOwner::AdapterCore
        );
        assert_eq!(web.transport, RuntimeAdapterTransport::WebSocket);
        assert_eq!(channel.transport, RuntimeAdapterTransport::ChannelMessage);
        assert_ne!(web.lifecycle, channel.lifecycle);
    }

    #[test]
    fn web_and_channel_capability_differences_are_explicit() {
        let [web, channel] = runtime_adapter_descriptors();

        assert!(web.capabilities.live_agent_session);
        assert!(!channel.capabilities.live_agent_session);
        assert!(!web.capabilities.inbound_session_backend);
        assert!(channel.capabilities.inbound_session_backend);
        assert!(web.capabilities.route_cache_stats);
        assert!(channel.capabilities.route_cache_stats);
        assert!(web.capabilities.route_mutation);
        assert!(channel.capabilities.route_mutation);
    }

    #[test]
    fn web_and_channel_do_not_own_runtime_command_presentation_text() {
        assert_no_forbidden_adapter_literals("gateway/ws.rs", include_str!("gateway/ws.rs"));
        assert_no_forbidden_adapter_literals("channels/mod.rs", include_str!("channels/mod.rs"));
    }

    #[test]
    fn web_and_channel_runtime_commands_use_contract_hot_path() {
        assert_adapter_uses_contract(
            "gateway/ws.rs",
            include_str!("gateway/ws.rs"),
            "WebRuntimeAdapterContract",
        );
        assert_adapter_uses_contract(
            "channels/mod.rs",
            include_str!("channels/mod.rs"),
            "ChannelRuntimeAdapterContract",
        );
    }

    #[test]
    fn shared_formatter_covers_non_help_contract_outcomes() {
        let options = RuntimeCommandPresentationOptions::new("openrouter");
        let cases = [
            CommandEffect::SwitchProvider {
                provider: "openrouter".into(),
            },
            CommandEffect::SwitchModel {
                model: "vision-model".into(),
                inferred_provider: Some("openrouter".into()),
                lane: Some(CapabilityLane::MultimodalUnderstanding),
                candidate_index: None,
                compacted: false,
            },
            CommandEffect::SwitchModelBlocked {
                model: "tiny-model".into(),
                provider: "openrouter".into(),
                lane: Some(CapabilityLane::Reasoning),
                preflight: RouteSwitchPreflight {
                    estimated_context_tokens: 8_000,
                    target_context_window_tokens: Some(4_000),
                    safe_context_budget_tokens: Some(3_000),
                    reserved_output_headroom_tokens: Some(1_000),
                    recommended_compaction_threshold_tokens: Some(1_500),
                    recommended_condensation: None,
                    status: RouteSwitchStatus::TooLarge,
                },
                compacted: true,
            },
            CommandEffect::CompactSession { compacted: true },
            CommandEffect::ClearSession,
        ];

        for effect in cases {
            assert!(
                format_common_command_effect(&effect, &options).is_some(),
                "formatter must cover non-help runtime command effect: {effect:?}"
            );
        }
        assert!(format_common_command_effect(&CommandEffect::ShowModel, &options).is_none());
        assert!(format_common_command_effect(&CommandEffect::ShowProviders, &options).is_none());
    }

    fn assert_no_forbidden_adapter_literals(path: &str, source: &str) {
        for literal in FORBIDDEN_ADAPTER_PRESENTATION_LITERALS {
            assert!(
                !source.contains(literal),
                "{path} owns runtime-command presentation literal `{literal}`; use runtime_command_presentation instead"
            );
        }
    }

    fn assert_adapter_uses_contract(path: &str, source: &str, contract_type: &str) {
        assert!(
            source.contains(contract_type),
            "{path} must instantiate {contract_type} on the runtime-command hot path"
        );
        assert!(
            source.contains("execute_runtime_command_effect")
                || source.contains("execute_runtime_command_output"),
            "{path} must execute runtime commands through the shared adapter-core executor"
        );
    }

    #[derive(Default)]
    struct MockRuntimeCommandHost {
        current_provider: String,
        show_providers: usize,
        show_model: usize,
        show_doctor: usize,
        show_skill_tools: usize,
        show_skill_traces: usize,
        show_skill_health: usize,
        show_skill_diff: usize,
        apply_skill_patch: usize,
        show_skill_versions: usize,
        create_user_skill: usize,
        update_user_skill: usize,
        rollback_skill_patch: usize,
        auto_promote_skills: usize,
        switched_provider: Option<String>,
        switched_model: Option<String>,
        compact_target_requests: usize,
        cleared: bool,
    }

    fn test_route(provider: &str, model: &str) -> RouteSelection {
        RouteSelection {
            provider: provider.to_string(),
            model: model.to_string(),
            lane: None,
            candidate_index: None,
            last_admission: None,
            recent_admissions: Vec::new(),
            last_tool_repair: None,
            recent_tool_repairs: Vec::new(),
            context_cache: None,
            assumptions: Vec::new(),
            calibrations: Vec::new(),
            watchdog_alerts: Vec::new(),
            handoff_artifacts: Vec::new(),
            runtime_decision_traces: Vec::new(),
            usage_ledger: Default::default(),
        }
    }

    #[async_trait::async_trait]
    impl RuntimeCommandHost for MockRuntimeCommandHost {
        fn current_provider(&self) -> String {
            if self.current_provider.is_empty() {
                "openrouter".to_string()
            } else {
                self.current_provider.clone()
            }
        }

        async fn provider_help_route(&mut self) -> anyhow::Result<RouteSelection> {
            self.show_providers += 1;
            Ok(test_route("openrouter", "test-model"))
        }

        async fn model_help_snapshot(&mut self) -> anyhow::Result<RuntimeModelHelpSnapshot> {
            self.show_model += 1;
            Ok(RuntimeModelHelpSnapshot {
                route: test_route("openrouter", "test-model"),
                config: Config::default(),
            })
        }

        async fn capability_doctor_report(&mut self) -> anyhow::Result<CapabilityDoctorReport> {
            self.show_doctor += 1;
            Ok(test_capability_doctor_report())
        }

        async fn skill_status_output(
            &mut self,
            _view: RuntimeSkillStatusView,
        ) -> anyhow::Result<String> {
            Ok("Skills status: 0 total, 0 active, 0 review, 0 blocked".to_string())
        }

        async fn skill_tool_contracts_output(&mut self) -> anyhow::Result<String> {
            self.show_skill_tools += 1;
            Ok("Tool replay contracts\nAudit: ok".to_string())
        }

        async fn skill_use_traces_output(&mut self) -> anyhow::Result<String> {
            self.show_skill_traces += 1;
            Ok("Skill use traces for agent test-agent (0):".to_string())
        }

        async fn skill_health_output(&mut self, apply: bool) -> anyhow::Result<String> {
            self.show_skill_health += 1;
            Ok(format!("Skill health for agent test-agent: apply={apply}"))
        }

        async fn skill_patch_diff_output(&mut self, candidate: &str) -> anyhow::Result<String> {
            self.show_skill_diff += 1;
            Ok(format!("Skill patch diff for {candidate}"))
        }

        async fn apply_skill_patch_output(&mut self, candidate: &str) -> anyhow::Result<String> {
            self.apply_skill_patch += 1;
            Ok(format!("Applied skill patch {candidate}"))
        }

        async fn skill_patch_versions_output(
            &mut self,
            skill_ref: Option<&str>,
        ) -> anyhow::Result<String> {
            self.show_skill_versions += 1;
            Ok(format!(
                "Skill versions for {}",
                skill_ref.unwrap_or("all skills")
            ))
        }

        async fn create_user_skill_output(
            &mut self,
            name: &str,
            _body: &str,
            _metadata: &RuntimeUserSkillCreateMetadata,
        ) -> anyhow::Result<String> {
            self.create_user_skill += 1;
            Ok(format!("Created user-authored skill `{name}`"))
        }

        async fn update_user_skill_output(
            &mut self,
            skill_ref: &str,
            _body: &str,
            _metadata: &RuntimeUserSkillCreateMetadata,
        ) -> anyhow::Result<String> {
            self.update_user_skill += 1;
            Ok(format!("Updated skill `{skill_ref}`"))
        }

        async fn rollback_skill_patch_output(
            &mut self,
            rollback_ref: &str,
        ) -> anyhow::Result<String> {
            self.rollback_skill_patch += 1;
            Ok(format!("Rolled back skill patch {rollback_ref}"))
        }

        async fn skill_auto_promotion_output(&mut self, apply: bool) -> anyhow::Result<String> {
            self.auto_promote_skills += 1;
            Ok(format!("Skill patch auto-promotion apply={apply}"))
        }

        async fn skill_review_output(&mut self, apply: bool) -> anyhow::Result<String> {
            Ok(format!("Skill review apply={apply}"))
        }

        async fn update_skill_status_output(
            &mut self,
            skill_ref: &str,
            target_status: SkillStatus,
        ) -> anyhow::Result<String> {
            Ok(format!("Updated {skill_ref} to {target_status}"))
        }

        async fn switch_provider(
            &mut self,
            request: RuntimeRouteMutationRequest,
        ) -> anyhow::Result<RuntimeProviderSwitchOutcome> {
            let provider = request.provider.ok_or_else(|| {
                anyhow::anyhow!("provider route mutation request missing provider")
            })?;
            self.switched_provider = Some(provider.clone());
            Ok(RuntimeProviderSwitchOutcome {
                provider,
                already_current: false,
            })
        }

        async fn switch_model(
            &mut self,
            request: RuntimeRouteMutationRequest,
            compacted: bool,
        ) -> anyhow::Result<RuntimeModelSwitchOutcome> {
            let provider = request.provider.unwrap_or_else(|| self.current_provider());
            let model = request.model.unwrap_or_default();
            let lane = request.lane;
            self.switched_model = Some(model);
            Ok(RuntimeModelSwitchOutcome::Applied {
                provider,
                lane,
                compacted,
            })
        }

        async fn session_compaction_target(
            &mut self,
        ) -> anyhow::Result<RuntimeSessionCompactionTarget> {
            self.compact_target_requests += 1;
            Err(anyhow::anyhow!("mock compaction target unavailable"))
        }

        async fn clear_session(&mut self) -> anyhow::Result<()> {
            self.cleared = true;
            Ok(())
        }
    }

    fn test_capability_doctor_report() -> CapabilityDoctorReport {
        CapabilityDoctorReport {
            generated_at_unix: 100,
            route: CapabilityDoctorRouteSnapshot {
                provider: "openrouter".into(),
                model: "test-model".into(),
                lane: None,
                candidate_index: None,
            },
            model_profile: CapabilityDoctorModelProfileSnapshot {
                context_window_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                features: vec!["tool_calling".into()],
                context_window_source: "manual_config".into(),
                context_window_freshness: "explicit".into(),
                context_window_confidence: "high".into(),
                max_output_source: "manual_config".into(),
                max_output_freshness: "explicit".into(),
                max_output_confidence: "high".into(),
                features_source: "manual_config".into(),
                features_freshness: "explicit".into(),
                features_confidence: "high".into(),
                observed_at_unix: None,
            },
            summary: CapabilityDoctorSummary {
                ok: 1,
                warn: 0,
                error: 0,
            },
            nodes: vec![CapabilityDoctorNode {
                subsystem: CapabilityDoctorSubsystem::ProviderAdapter,
                subject: "openrouter".into(),
                readiness: CapabilityDoctorReadiness::Ready,
                severity: CapabilityDoctorSeverity::Ok,
                evidence: vec!["provider adapter is registered".into()],
                recommendation: None,
            }],
        }
    }

    #[tokio::test]
    async fn executor_canonicalizes_provider_before_host_mutation() {
        let contract = WebRuntimeAdapterContract;
        let mut host = MockRuntimeCommandHost::default();
        let response = execute_runtime_command_effect(
            &contract,
            &mut host,
            &CommandEffect::SwitchProvider {
                provider: "grok".to_string(),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(host.switched_provider.as_deref(), Some("xai"));
        assert!(response.contains("`xai`"));
    }

    #[tokio::test]
    async fn executor_rejects_unknown_provider_without_host_mutation() {
        let contract = ChannelRuntimeAdapterContract;
        let mut host = MockRuntimeCommandHost::default();
        let response = execute_runtime_command_effect(
            &contract,
            &mut host,
            &CommandEffect::SwitchProvider {
                provider: "missing-provider".to_string(),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(host.switched_provider, None);
        assert!(response.contains("Unknown provider"));
    }

    #[tokio::test]
    async fn executor_renders_show_providers_from_typed_snapshot() {
        let contract = WebRuntimeAdapterContract;
        let mut host = MockRuntimeCommandHost::default();
        let response = execute_runtime_command_effect(
            &contract,
            &mut host,
            &CommandEffect::ShowProviders,
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(host.show_providers, 1);
        assert_eq!(host.show_model, 0);
        assert!(response.contains("Current provider: `openrouter`"));
        assert!(response.contains("Current model: `test-model`"));
    }

    #[tokio::test]
    async fn executor_renders_show_model_from_typed_snapshot() {
        let contract = ChannelRuntimeAdapterContract;
        let mut host = MockRuntimeCommandHost::default();
        let response = execute_runtime_command_effect(
            &contract,
            &mut host,
            &CommandEffect::ShowModel,
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(host.show_providers, 0);
        assert_eq!(host.show_model, 1);
        assert!(response.contains("Current provider: `openrouter`"));
        assert!(response.contains("Current model: `test-model`"));
    }

    #[tokio::test]
    async fn executor_renders_show_doctor_from_shared_report() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ShowDoctor,
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ShowDoctor,
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.show_doctor, 1);
        assert_eq!(channel_host.show_doctor, 1);
        assert!(web_response.contains("Capability doctor"));
        assert!(web_response.contains("provider_adapter"));
    }

    #[tokio::test]
    async fn executor_renders_skill_tool_contracts_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ShowSkillTools,
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ShowSkillTools,
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.show_skill_tools, 1);
        assert_eq!(channel_host.show_skill_tools, 1);
        assert!(web_response.contains("Tool replay contracts"));
    }

    #[tokio::test]
    async fn executor_renders_skill_use_traces_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ShowSkillTraces,
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ShowSkillTraces,
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.show_skill_traces, 1);
        assert_eq!(channel_host.show_skill_traces, 1);
        assert!(web_response.contains("Skill use traces"));
    }

    #[tokio::test]
    async fn executor_renders_skill_health_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ShowSkillHealth { apply: false },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ShowSkillHealth { apply: false },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.show_skill_health, 1);
        assert_eq!(channel_host.show_skill_health, 1);
        assert!(web_response.contains("Skill health"));
    }

    #[tokio::test]
    async fn executor_renders_skill_patch_diff_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ShowSkillDiff {
                candidate: "patch-a".into(),
            },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ShowSkillDiff {
                candidate: "patch-a".into(),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.show_skill_diff, 1);
        assert_eq!(channel_host.show_skill_diff, 1);
        assert!(web_response.contains("Skill patch diff"));
    }

    #[tokio::test]
    async fn executor_applies_skill_patch_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ApplySkillPatch {
                candidate: "patch-a".into(),
            },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ApplySkillPatch {
                candidate: "patch-a".into(),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.apply_skill_patch, 1);
        assert_eq!(channel_host.apply_skill_patch, 1);
        assert!(web_response.contains("Applied skill patch"));
    }

    #[tokio::test]
    async fn executor_renders_skill_patch_versions_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::ShowSkillVersions {
                skill: Some("Skill A".into()),
            },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::ShowSkillVersions {
                skill: Some("Skill A".into()),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.show_skill_versions, 1);
        assert_eq!(channel_host.show_skill_versions, 1);
        assert!(web_response.contains("Skill versions"));
    }

    #[tokio::test]
    async fn executor_rolls_back_skill_patch_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::RollbackSkillPatch {
                rollback: "apply-a".into(),
            },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::RollbackSkillPatch {
                rollback: "apply-a".into(),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.rollback_skill_patch, 1);
        assert_eq!(channel_host.rollback_skill_patch, 1);
        assert!(web_response.contains("Rolled back skill patch"));
    }

    #[tokio::test]
    async fn executor_runs_skill_auto_promotion_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::AutoPromoteSkills { apply: false },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::AutoPromoteSkills { apply: false },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.auto_promote_skills, 1);
        assert_eq!(channel_host.auto_promote_skills, 1);
        assert!(web_response.contains("Skill patch auto-promotion"));
    }

    #[tokio::test]
    async fn executor_creates_user_skill_through_shared_host() {
        let mut web_host = MockRuntimeCommandHost::default();
        let web_response = execute_runtime_command_effect(
            &WebRuntimeAdapterContract,
            &mut web_host,
            &CommandEffect::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "Find the local checkout and compare tags.".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            },
            "openrouter",
        )
        .await
        .unwrap();
        let mut channel_host = MockRuntimeCommandHost::default();
        let channel_response = execute_runtime_command_effect(
            &ChannelRuntimeAdapterContract,
            &mut channel_host,
            &CommandEffect::CreateUserSkill {
                name: "Matrix release check".into(),
                body: "Find the local checkout and compare tags.".into(),
                metadata: RuntimeUserSkillCreateMetadata::default(),
            },
            "openrouter",
        )
        .await
        .unwrap();

        assert_eq!(web_response, channel_response);
        assert_eq!(web_host.create_user_skill, 1);
        assert_eq!(channel_host.create_user_skill, 1);
        assert!(web_response.contains("Created user-authored skill"));
    }

    #[tokio::test]
    async fn executor_dispatches_clear_session_through_host() {
        let contract = WebRuntimeAdapterContract;
        let mut host = MockRuntimeCommandHost::default();
        let response = execute_runtime_command_effect(
            &contract,
            &mut host,
            &CommandEffect::ClearSession,
            "openrouter",
        )
        .await
        .unwrap();

        assert!(host.cleared);
        assert!(response.contains("Conversation history cleared"));
    }
}
