use serde_json::Value;
use synapse_domain::domain::dialogue_state::{DialogueSlot, FocusEntity};
use synapse_domain::ports::agent_runtime::AgentToolFact;

pub(crate) fn build_tool_fact(tool_name: &str, arguments: &Value) -> AgentToolFact {
    let mut fact = AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: Vec::new(),
        slots: Vec::new(),
    };

    collect_argument_fact(None, arguments, &mut fact);

    fact.focus_entities = dedupe_focus_entities(fact.focus_entities);
    fact.slots = dedupe_slots(fact.slots);
    fact
}

fn collect_argument_fact(path: Option<&str>, value: &Value, fact: &mut AgentToolFact) {
    match value {
        Value::Object(map) => {
            for (raw_key, nested) in map {
                let normalized = normalize_key(raw_key);
                let next_path = match path {
                    Some(prefix) if !prefix.is_empty() => format!("{prefix}_{normalized}"),
                    _ => normalized,
                };
                collect_argument_fact(Some(&next_path), nested, fact);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_argument_fact(path, item, fact);
            }
        }
        _ => {
            let Some(path) = path else {
                return;
            };
            let normalized_key = normalize_key(path);
            let entity_key = normalized_key
                .rsplit('_')
                .find(|segment| !segment.is_empty())
                .unwrap_or(normalized_key.as_str());

            if let Some(kind) = infer_entity_kind(entity_key) {
                let scalar_values = flatten_entity_values(value);
                if scalar_values.is_empty() {
                    return;
                }

                for item in &scalar_values {
                    fact.focus_entities.push(FocusEntity {
                        kind: kind.to_string(),
                        name: item.clone(),
                        metadata: None,
                    });
                }

                fact.slots.push(DialogueSlot {
                    name: normalized_key,
                    value: if scalar_values.len() == 1 {
                        scalar_values[0].clone()
                    } else {
                        scalar_values.join(", ")
                    },
                });
                return;
            }

            if let Some(slot_value) = flatten_slot_value(value) {
                fact.slots.push(DialogueSlot {
                    name: normalized_key,
                    value: slot_value,
                });
            }
        }
    }
}

fn normalize_key(key: &str) -> String {
    let mut normalized = key.trim().to_ascii_lowercase().replace('-', "_");
    if let Some(base) = normalized.strip_suffix("_name") {
        normalized = base.to_string();
    }
    if let Some(base) = normalized.strip_suffix("_id") {
        normalized = base.to_string();
    }
    let trimmed = normalized.trim_end_matches(|ch: char| ch.is_ascii_digit());
    trimmed.trim_end_matches('_').to_string()
}

fn infer_entity_kind(key: &str) -> Option<&'static str> {
    match key {
        "city" | "cities" => Some("city"),
        "location" | "locations" | "place" | "places" => Some("location"),
        "service" | "services" => Some("service"),
        "environment" | "env" | "environments" => Some("environment"),
        "branch" | "branches" => Some("branch"),
        "file" | "files" => Some("file"),
        "path" | "paths" => Some("path"),
        "url" | "urls" => Some("url"),
        "room" | "rooms" => Some("room"),
        "channel" | "channels" => Some("channel"),
        "recipient" | "recipients" => Some("recipient"),
        "project" | "projects" => Some("project"),
        "repo" | "repository" => Some("repository"),
        _ => None,
    }
}

fn flatten_entity_values(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => vec![s.trim().to_string()],
        Value::Number(n) => vec![n.to_string()],
        Value::Bool(b) => vec![b.to_string()],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                Value::Bool(b) => Some(b.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn flatten_slot_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Array(items) => {
            let values = items
                .iter()
                .filter_map(|item| match item {
                    Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                    Value::Number(n) => Some(n.to_string()),
                    Value::Bool(b) => Some(b.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if values.is_empty() {
                None
            } else {
                Some(values.join(", "))
            }
        }
        _ => None,
    }
}

fn dedupe_focus_entities(values: Vec<FocusEntity>) -> Vec<FocusEntity> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.iter().any(|existing: &FocusEntity| {
            existing.kind == value.kind && existing.name == value.name
        }) {
            unique.push(value);
        }
    }
    unique
}

fn dedupe_slots(values: Vec<DialogueSlot>) -> Vec<DialogueSlot> {
    let mut unique = Vec::new();
    for value in values {
        if let Some(existing_idx) = unique
            .iter()
            .position(|existing: &DialogueSlot| existing.name == value.name)
        {
            unique[existing_idx] = value;
        } else {
            unique.push(value);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_focus_and_slots_from_structured_args() {
        let fact = build_tool_fact(
            "weather_lookup",
            &serde_json::json!({
                "cities": ["Berlin", "Tbilisi"],
                "timezone": "Europe/Berlin"
            }),
        );

        assert_eq!(fact.tool_name, "weather_lookup");
        assert_eq!(fact.focus_entities.len(), 2);
        assert_eq!(fact.focus_entities[0].kind, "city");
        assert_eq!(fact.focus_entities[0].name, "Berlin");
        assert_eq!(fact.focus_entities[1].name, "Tbilisi");
        assert!(fact.slots.iter().any(|slot| slot.name == "cities"));
        assert!(fact
            .slots
            .iter()
            .any(|slot| slot.name == "timezone" && slot.value == "Europe/Berlin"));
    }

    #[test]
    fn extracts_nested_target_fields_from_structured_args() {
        let fact = build_tool_fact(
            "message_send",
            &serde_json::json!({
                "content": "hello",
                "target": {
                    "channel": "matrix",
                    "recipient": "!room:example.com",
                    "thread_ref": "$thread"
                }
            }),
        );

        assert!(fact
            .focus_entities
            .iter()
            .any(|entity| entity.kind == "channel" && entity.name == "matrix"));
        assert!(fact
            .focus_entities
            .iter()
            .any(|entity| entity.kind == "recipient" && entity.name == "!room:example.com"));
        assert!(fact
            .slots
            .iter()
            .any(|slot| slot.name == "target_thread_ref" && slot.value == "$thread"));
    }
}
