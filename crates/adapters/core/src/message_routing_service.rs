//! Shared explicit message-routing service for web and channel ingress.
//!
//! This handles only configured deterministic routes. No route means local
//! inbound handling continues.

use std::collections::HashMap;
use std::sync::Arc;

use synapse_domain::application::services::assistant_output_presentation::{
    AssistantOutputPresenter, OutputDeliveryHints, PresentedOutput,
};
use synapse_domain::application::services::inbound_message_service;
use synapse_domain::application::services::pipeline_service::{
    run_pipeline, PipelineRunnerPorts, StartPipelineParams,
};
use synapse_domain::domain::channel::InboundEnvelope;
use synapse_domain::domain::pipeline_context::PipelineState;
use synapse_domain::domain::routing::RoutingInput;
use synapse_domain::ports::dead_letter::DeadLetterPort;
use synapse_domain::ports::message_router::MessageRouterPort;
use synapse_domain::ports::pipeline_executor::PipelineExecutorPort;
use synapse_domain::ports::pipeline_store::PipelineStorePort;
use synapse_domain::ports::run_store::{NoOpRunStore, RunStorePort};

#[derive(Clone, Default)]
pub(crate) struct MessageRoutingPorts {
    pub router: Option<Arc<dyn MessageRouterPort>>,
    pub pipeline_store: Option<Arc<dyn PipelineStorePort>>,
    pub pipeline_executor: Option<Arc<dyn PipelineExecutorPort>>,
    pub run_store: Option<Arc<dyn RunStorePort>>,
    pub dead_letter: Option<Arc<dyn DeadLetterPort>>,
}

pub(crate) async fn route_explicit_message(
    envelope: &InboundEnvelope,
    ports: MessageRoutingPorts,
    delivery_hints: OutputDeliveryHints,
) -> Option<PresentedOutput> {
    tracing::info!(
        has_router = ports.router.is_some(),
        has_store = ports.pipeline_store.is_some(),
        has_executor = ports.pipeline_executor.is_some(),
        content = %envelope.content,
        "explicit message routing check"
    );

    let (Some(router), Some(store), Some(executor)) = (
        ports.router.as_ref(),
        ports.pipeline_store.as_ref(),
        ports.pipeline_executor.as_ref(),
    ) else {
        return None;
    };

    let routing_input = RoutingInput {
        content: inbound_message_service::provider_facing_content(
            &envelope.content,
            &envelope.media_attachments,
        ),
        source_kind: format!("{:?}", envelope.source_kind),
        metadata: HashMap::new(),
    };
    let route_result = router.route(&routing_input).await?;
    tracing::info!(
        target = %route_result.target,
        pipeline = ?route_result.pipeline,
        matched = ?route_result.matched_rule,
        "explicit message routing result"
    );

    let pipeline_name = route_result.pipeline?;
    store.get(&pipeline_name).await.as_ref()?;

    let matched = route_result.matched_rule.as_deref().unwrap_or("unknown");
    tracing::info!(
        pipeline = %pipeline_name,
        matched_rule = %matched,
        content = %envelope.content,
        "message routed to pipeline"
    );

    let input = serde_json::json!({
        "message": routing_input.content,
        "source": envelope.source_adapter,
        "sender": envelope.actor_id,
    });
    let run_store: Arc<dyn RunStorePort> = ports
        .run_store
        .unwrap_or_else(|| Arc::new(NoOpRunStore) as Arc<dyn RunStorePort>);
    let pipeline_ports = PipelineRunnerPorts {
        pipeline_store: Arc::clone(store),
        executor: Arc::clone(executor),
        run_store,
        dead_letter: ports.dead_letter,
    };
    let params = StartPipelineParams {
        pipeline_name: pipeline_name.clone(),
        input,
        triggered_by: envelope.actor_id.clone(),
        depth: 0,
        parent_run_id: None,
    };
    let result = run_pipeline(&pipeline_ports, params).await;
    let reply = format_pipeline_result(&pipeline_name, &result);
    Some(AssistantOutputPresenter::success(
        reply,
        Vec::new(),
        String::new(),
        false,
        delivery_hints,
    ))
}

fn format_pipeline_result(
    pipeline_name: &str,
    result: &synapse_domain::application::services::pipeline_service::PipelineRunResult,
) -> String {
    match &result.state {
        PipelineState::Completed => result
            .data
            .as_object()
            .and_then(|obj| {
                obj.iter()
                    .rev()
                    .find(|(key, _)| *key != "_input")
                    .map(|(step, value)| {
                        let detail = value
                            .get("summary")
                            .and_then(|summary| summary.as_str())
                            .or_else(|| value.get("status").and_then(|status| status.as_str()))
                            .unwrap_or("done");
                        format!("Pipeline `{pipeline_name}` - {step}: {detail}")
                    })
            })
            .unwrap_or_else(|| format!("Pipeline `{pipeline_name}` completed.")),
        PipelineState::Failed => {
            let error = result.error.as_deref().unwrap_or("unknown error");
            format!("Pipeline `{pipeline_name}` failed: {error}")
        }
        _ => format!(
            "Pipeline `{pipeline_name}` ended in state: {:?}",
            result.state
        ),
    }
}
