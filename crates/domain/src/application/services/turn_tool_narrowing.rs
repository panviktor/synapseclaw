//! Turn tool narrowing — typed runtime tool exposure policy.
//!
//! This layer narrows provider-visible tools from structured execution
//! guidance. It does not inspect user phrases and does not depend on adapter
//! internals.

use crate::application::services::execution_guidance::{ExecutionCapability, ExecutionGuidance};
use crate::application::services::turn_model_routing::infer_turn_capability_requirement;
use crate::domain::tool_repair::ToolRepairAction;
use crate::domain::turn_defaults::ResolvedTurnDefaults;
use crate::ports::tool::{ToolRuntimeRole, ToolSpec};

pub fn prepare_tool_specs_for_turn(
    tool_specs: Vec<ToolSpec>,
    guidance: Option<&ExecutionGuidance>,
    defaults: &ResolvedTurnDefaults,
    user_message: &str,
) -> Vec<ToolSpec> {
    let narrowed = narrow_tool_specs_for_turn(tool_specs, guidance, user_message);
    let specialized = specialize_tool_specs_for_turn(narrowed, guidance, defaults);
    suppress_recently_failed_tools(specialized, guidance)
}

pub fn should_force_implicit_target_for_tool(
    tool_name: &str,
    tool_specs: &[ToolSpec],
    guidance: Option<&ExecutionGuidance>,
    defaults: &ResolvedTurnDefaults,
) -> bool {
    implicit_delivery_lane(tool_specs, guidance, defaults)
        && tool_specs.iter().any(|spec| {
            spec.name == tool_name && spec.runtime_role == Some(ToolRuntimeRole::DirectDelivery)
        })
}

pub fn narrow_tool_specs_for_turn(
    tool_specs: Vec<ToolSpec>,
    guidance: Option<&ExecutionGuidance>,
    user_message: &str,
) -> Vec<ToolSpec> {
    if infer_turn_capability_requirement(user_message).is_some() {
        return Vec::new();
    }

    let Some(guidance) = guidance else {
        return tool_specs;
    };

    if guidance.prefer_answer_from_resolved_state {
        let filtered = tool_specs
            .iter()
            .filter(|spec| {
                matches!(
                    spec.runtime_role,
                    Some(
                        ToolRuntimeRole::ProfileMutation
                            | ToolRuntimeRole::MemoryMutation
                            | ToolRuntimeRole::RuntimeStateInspection
                            | ToolRuntimeRole::ExternalLookup
                    )
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        return if filtered.is_empty() {
            Vec::new()
        } else {
            filtered
        };
    }

    if guidance.direct_resolution_ready
        && guidance
            .preferred_capabilities
            .contains(&ExecutionCapability::Delivery)
    {
        let filtered = tool_specs
            .iter()
            .filter(|spec| matches!(spec.runtime_role, Some(ToolRuntimeRole::DirectDelivery)))
            .cloned()
            .collect::<Vec<_>>();
        return if filtered.is_empty() {
            tool_specs
        } else {
            filtered
        };
    }

    if guidance.direct_resolution_ready
        && guidance
            .preferred_capabilities
            .contains(&ExecutionCapability::ProfileDefaults)
    {
        let filtered = tool_specs
            .iter()
            .filter(|spec| {
                matches!(
                    spec.runtime_role,
                    Some(ToolRuntimeRole::ProfileMutation) | Some(ToolRuntimeRole::ExternalLookup)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        return if filtered.is_empty() {
            tool_specs
        } else {
            filtered
        };
    }

    tool_specs
}

fn specialize_tool_specs_for_turn(
    tool_specs: Vec<ToolSpec>,
    guidance: Option<&ExecutionGuidance>,
    defaults: &ResolvedTurnDefaults,
) -> Vec<ToolSpec> {
    if !implicit_delivery_lane(&tool_specs, guidance, defaults) {
        return tool_specs;
    }

    tool_specs
        .into_iter()
        .map(|spec| {
            if spec.runtime_role != Some(ToolRuntimeRole::DirectDelivery) {
                return spec;
            }

            let mut specialized = spec;
            specialized.description.push_str(
                " For this turn, the delivery target is already resolved by runtime. \
                 Provide only `content` and omit `target`.",
            );
            if let Some(properties) = specialized
                .parameters
                .get_mut("properties")
                .and_then(serde_json::Value::as_object_mut)
            {
                properties.remove("target");
                if let Some(content) = properties
                    .get_mut("content")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    content.insert(
                        "description".into(),
                        serde_json::Value::String(
                            "Message text to send to the already resolved delivery target".into(),
                        ),
                    );
                }
            }
            if let Some(required) = specialized
                .parameters
                .get_mut("required")
                .and_then(serde_json::Value::as_array_mut)
            {
                required.retain(|value| value.as_str() != Some("target"));
            }
            specialized
        })
        .collect()
}

fn delivery_mode_with_resolved_target(
    guidance: Option<&ExecutionGuidance>,
    defaults: &ResolvedTurnDefaults,
) -> bool {
    defaults.delivery_target.is_some()
        && guidance.is_some_and(|guidance| {
            guidance.direct_resolution_ready
                && guidance
                    .preferred_capabilities
                    .contains(&ExecutionCapability::Delivery)
        })
}

fn implicit_delivery_lane(
    tool_specs: &[ToolSpec],
    guidance: Option<&ExecutionGuidance>,
    defaults: &ResolvedTurnDefaults,
) -> bool {
    if delivery_mode_with_resolved_target(guidance, defaults) {
        return true;
    }

    defaults.delivery_target.is_some()
        && !tool_specs.is_empty()
        && tool_specs
            .iter()
            .all(|spec| spec.runtime_role == Some(ToolRuntimeRole::DirectDelivery))
}

fn suppress_recently_failed_tools(
    tool_specs: Vec<ToolSpec>,
    guidance: Option<&ExecutionGuidance>,
) -> Vec<ToolSpec> {
    let Some(guidance) = guidance else {
        return tool_specs;
    };
    if guidance.recent_failure_hints.is_empty() {
        return tool_specs;
    }

    let suppressible = guidance
        .recent_failure_hints
        .iter()
        .filter(|hint| should_suppress_recently_failed_tool(hint.suggested_action))
        .map(|hint| hint.tool_name.as_str())
        .collect::<Vec<_>>();

    if suppressible.is_empty() {
        return tool_specs;
    }

    tool_specs
        .iter()
        .filter(|spec| !should_drop_tool_spec(spec, &tool_specs, &suppressible))
        .cloned()
        .collect()
}

fn should_suppress_recently_failed_tool(action: ToolRepairAction) -> bool {
    matches!(
        action,
        ToolRepairAction::UseKnownTool
            | ToolRepairAction::AvoidDuplicateRetry
            | ToolRepairAction::SwitchRouteLane(_)
    )
}

fn should_drop_tool_spec(spec: &ToolSpec, all_specs: &[ToolSpec], suppressible: &[&str]) -> bool {
    if !suppressible.iter().any(|name| *name == spec.name) {
        return false;
    }

    let Some(role) = spec.runtime_role else {
        return false;
    };

    all_specs.iter().any(|candidate| {
        candidate.name != spec.name
            && candidate.runtime_role == Some(role)
            && !suppressible.iter().any(|name| *name == candidate.name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::execution_guidance::ExecutionFailureHint;
    use crate::domain::conversation_target::ConversationDeliveryTarget;
    use crate::domain::tool_repair::{ToolFailureKind, ToolRepairAction};
    use crate::domain::turn_defaults::{ResolvedDeliveryTarget, TurnDefaultSource};

    fn spec(name: &str, runtime_role: Option<ToolRuntimeRole>) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: format!("{name} desc"),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "target": {"type": "object"}
                },
                "required": ["content"]
            }),
            runtime_role,
        }
    }

    #[test]
    fn delivery_guidance_filters_archaeology_and_delegation_tools() {
        let specs = vec![
            spec("message_send", Some(ToolRuntimeRole::DirectDelivery)),
            spec("agents_send", Some(ToolRuntimeRole::DelegatedDelivery)),
            spec("agents_list", Some(ToolRuntimeRole::DelegatedDelivery)),
            spec("session_search", Some(ToolRuntimeRole::HistoricalLookup)),
            spec("file_read", Some(ToolRuntimeRole::WorkspaceDiscovery)),
            spec("memory_recall", Some(ToolRuntimeRole::MemoryMutation)),
            spec("web_search_tool", Some(ToolRuntimeRole::ExternalLookup)),
        ];
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };

        let filtered = narrow_tool_specs_for_turn(specs, Some(&guidance), "");
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["message_send"]);
    }

    #[test]
    fn profile_default_guidance_keeps_lookup_but_removes_history_archaeology() {
        let specs = vec![
            spec("user_profile", Some(ToolRuntimeRole::ProfileMutation)),
            spec("session_search", Some(ToolRuntimeRole::HistoricalLookup)),
            spec("file_read", Some(ToolRuntimeRole::WorkspaceDiscovery)),
            spec("memory_recall", Some(ToolRuntimeRole::MemoryMutation)),
            spec("web_search_tool", Some(ToolRuntimeRole::ExternalLookup)),
        ];
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::ProfileDefaults],
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };

        let filtered = narrow_tool_specs_for_turn(specs, Some(&guidance), "");
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["user_profile", "web_search_tool"]);
    }

    #[test]
    fn preserves_original_toolset_when_delivery_narrowing_would_be_empty() {
        let specs = vec![
            spec("file_read", Some(ToolRuntimeRole::WorkspaceDiscovery)),
            spec("memory_recall", Some(ToolRuntimeRole::MemoryMutation)),
        ];
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };

        let filtered = narrow_tool_specs_for_turn(specs.clone(), Some(&guidance), "");
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["file_read", "memory_recall"]);
    }

    #[test]
    fn answer_from_resolved_state_keeps_only_safe_runtime_tools() {
        let specs = vec![
            spec("core_memory_update", Some(ToolRuntimeRole::MemoryMutation)),
            spec("user_profile", Some(ToolRuntimeRole::ProfileMutation)),
            spec("memory_recall", Some(ToolRuntimeRole::HistoricalLookup)),
            spec("web_search_tool", Some(ToolRuntimeRole::ExternalLookup)),
            spec("file_read", Some(ToolRuntimeRole::WorkspaceDiscovery)),
        ];
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            prefer_answer_from_resolved_state: true,
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };

        let filtered = narrow_tool_specs_for_turn(specs, Some(&guidance), "");
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["core_memory_update", "user_profile", "web_search_tool",]
        );
    }

    #[test]
    fn multimodal_understanding_marker_exposes_no_tools() {
        let specs = vec![
            spec("memory_recall", Some(ToolRuntimeRole::MemoryMutation)),
            spec("content_search", Some(ToolRuntimeRole::WorkspaceDiscovery)),
            spec("glob_search", Some(ToolRuntimeRole::WorkspaceDiscovery)),
            spec("image_info", Some(ToolRuntimeRole::ExternalLookup)),
        ];

        let filtered = narrow_tool_specs_for_turn(
            specs,
            None,
            "[IMAGE:data:image/png;base64,abcd] Describe the image.",
        );

        assert!(filtered.is_empty());
    }

    #[test]
    fn media_generation_marker_exposes_no_tools() {
        let specs = vec![
            spec("memory_recall", Some(ToolRuntimeRole::MemoryMutation)),
            spec("content_search", Some(ToolRuntimeRole::WorkspaceDiscovery)),
            spec("web_search_tool", Some(ToolRuntimeRole::ExternalLookup)),
        ];

        let filtered = narrow_tool_specs_for_turn(
            specs,
            None,
            "[GENERATE:MUSIC] Create a short ambient loop.",
        );

        assert!(filtered.is_empty());
    }

    #[test]
    fn resolved_delivery_specializes_direct_delivery_schema_to_content_only() {
        let specs = vec![
            spec("message_send", Some(ToolRuntimeRole::DirectDelivery)),
            spec("file_read", Some(ToolRuntimeRole::WorkspaceDiscovery)),
        ];
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };
        let defaults = ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!ops:example.org".into(),
                    thread_ref: None,
                },
                source: TurnDefaultSource::ConfiguredChannel,
            }),
        };

        let prepared = prepare_tool_specs_for_turn(specs, Some(&guidance), &defaults, "");
        let message_send = prepared
            .iter()
            .find(|spec| spec.runtime_role == Some(ToolRuntimeRole::DirectDelivery))
            .expect("direct delivery spec");

        assert!(!message_send
            .parameters
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("properties")
            .contains_key("target"));
        assert!(message_send.description.contains("omit `target`"));
    }

    #[test]
    fn resolved_delivery_marks_direct_delivery_tool_for_implicit_target_execution() {
        let specs = vec![
            spec("message_send", Some(ToolRuntimeRole::DirectDelivery)),
            spec("file_read", Some(ToolRuntimeRole::WorkspaceDiscovery)),
        ];
        let guidance = ExecutionGuidance {
            direct_resolution_ready: true,
            preferred_capabilities: vec![ExecutionCapability::Delivery],
            recent_failure_hints: Vec::new(),
            ..ExecutionGuidance::default()
        };
        let defaults = ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!ops:example.org".into(),
                    thread_ref: None,
                },
                source: TurnDefaultSource::ConfiguredChannel,
            }),
        };

        assert!(should_force_implicit_target_for_tool(
            "message_send",
            &specs,
            Some(&guidance),
            &defaults,
        ));
        assert!(!should_force_implicit_target_for_tool(
            "file_read",
            &specs,
            Some(&guidance),
            &defaults,
        ));
    }

    #[test]
    fn exclusive_direct_delivery_lane_forces_implicit_target_even_without_guidance() {
        let specs = vec![spec("message_send", Some(ToolRuntimeRole::DirectDelivery))];
        let defaults = ResolvedTurnDefaults {
            delivery_target: Some(ResolvedDeliveryTarget {
                target: ConversationDeliveryTarget::Explicit {
                    channel: "matrix".into(),
                    recipient: "!ops:example.org".into(),
                    thread_ref: None,
                },
                source: TurnDefaultSource::ConfiguredChannel,
            }),
        };

        assert!(should_force_implicit_target_for_tool(
            "message_send",
            &specs,
            None,
            &defaults,
        ));

        let prepared = prepare_tool_specs_for_turn(specs, None, &defaults, "");
        let message_send = prepared.first().expect("message_send");
        assert!(!message_send
            .parameters
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("properties")
            .contains_key("target"));
    }

    #[test]
    fn suppresses_recently_failed_tool_when_same_role_alternative_exists() {
        let specs = vec![
            spec("image_info", Some(ToolRuntimeRole::ExternalLookup)),
            spec("vision_describe", Some(ToolRuntimeRole::ExternalLookup)),
        ];
        let guidance = ExecutionGuidance {
            recent_failure_hints: vec![ExecutionFailureHint {
                tool_name: "image_info".into(),
                failure_kind: ToolFailureKind::CapabilityMismatch,
                suggested_action: ToolRepairAction::SwitchRouteLane(
                    crate::config::schema::CapabilityLane::MultimodalUnderstanding,
                ),
            }],
            ..ExecutionGuidance::default()
        };

        let filtered = prepare_tool_specs_for_turn(
            specs,
            Some(&guidance),
            &ResolvedTurnDefaults::default(),
            "",
        );
        let names = filtered
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["vision_describe"]);
    }

    #[test]
    fn does_not_drop_only_available_tool_after_recent_failure() {
        let specs = vec![spec("image_info", Some(ToolRuntimeRole::ExternalLookup))];
        let guidance = ExecutionGuidance {
            recent_failure_hints: vec![ExecutionFailureHint {
                tool_name: "image_info".into(),
                failure_kind: ToolFailureKind::CapabilityMismatch,
                suggested_action: ToolRepairAction::SwitchRouteLane(
                    crate::config::schema::CapabilityLane::MultimodalUnderstanding,
                ),
            }],
            ..ExecutionGuidance::default()
        };

        let filtered = prepare_tool_specs_for_turn(
            specs,
            Some(&guidance),
            &ResolvedTurnDefaults::default(),
            "",
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "image_info");
    }
}
