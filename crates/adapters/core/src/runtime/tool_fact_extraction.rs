use serde_json::Value;
use synapse_domain::domain::dialogue_state::{DialogueSlot, FocusEntity};
use synapse_domain::domain::message::ChatMessage;
use synapse_domain::ports::agent_runtime::AgentToolFact;

const TOOL_CALL_START: &str = "<tool_call>";
const TOOL_CALL_END: &str = "</tool_call>";

pub(crate) fn build_tool_fact(tool_name: &str, arguments: &Value) -> AgentToolFact {
    let mut fact = AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: Vec::new(),
        slots: Vec::new(),
    };

    if let Value::Object(map) = arguments {
        for (raw_key, value) in map {
            collect_argument_fact(raw_key, value, &mut fact);
        }
    }

    fact.focus_entities = dedupe_focus_entities(fact.focus_entities);
    fact.slots = dedupe_slots(fact.slots);
    fact
}

pub(crate) fn extract_tool_facts_from_chat_history(
    history: &[ChatMessage],
    start_idx: usize,
) -> Vec<AgentToolFact> {
    let mut facts = Vec::new();

    for msg in history.iter().skip(start_idx) {
        if msg.role != "assistant" {
            continue;
        }

        for (tool_name, arguments) in parse_tool_calls_from_content(&msg.content) {
            facts.push(build_tool_fact(&tool_name, &arguments));
        }
    }

    facts
}

fn parse_tool_calls_from_content(content: &str) -> Vec<(String, Value)> {
    if let Some(calls) = parse_native_tool_calls(content) {
        return calls;
    }

    parse_tagged_tool_calls(content)
}

fn parse_native_tool_calls(content: &str) -> Option<Vec<(String, Value)>> {
    let value = serde_json::from_str::<Value>(content).ok()?;
    let calls = value.get("tool_calls")?.as_array()?;
    let mut parsed = Vec::new();

    for call in calls {
        let Some(name) = call.get("name").and_then(Value::as_str) else {
            continue;
        };
        let arguments = call
            .get("arguments")
            .map(parse_arguments_value)
            .unwrap_or(Value::Null);
        parsed.push((name.to_string(), arguments));
    }

    Some(parsed)
}

fn parse_tagged_tool_calls(content: &str) -> Vec<(String, Value)> {
    let mut calls = Vec::new();
    let mut rest = content;

    while let Some(start_idx) = rest.find(TOOL_CALL_START) {
        rest = &rest[start_idx + TOOL_CALL_START.len()..];
        let Some(end_idx) = rest.find(TOOL_CALL_END) else {
            break;
        };
        let payload = rest[..end_idx].trim();
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                let arguments = value
                    .get("arguments")
                    .map(parse_arguments_value)
                    .unwrap_or(Value::Null);
                calls.push((name.to_string(), arguments));
            }
        }
        rest = &rest[end_idx + TOOL_CALL_END.len()..];
    }

    calls
}

fn parse_arguments_value(value: &Value) -> Value {
    match value {
        Value::String(s) => {
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.clone()))
        }
        other => other.clone(),
    }
}

fn collect_argument_fact(key: &str, value: &Value, fact: &mut AgentToolFact) {
    let normalized_key = normalize_key(key);
    let Some(kind) = infer_entity_kind(&normalized_key) else {
        if let Some(slot_value) = flatten_slot_value(value) {
            fact.slots.push(DialogueSlot {
                name: normalized_key,
                value: slot_value,
            });
        }
        return;
    };

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

    if scalar_values.len() == 1 {
        fact.slots.push(DialogueSlot {
            name: normalized_key,
            value: scalar_values[0].clone(),
        });
    } else {
        fact.slots.push(DialogueSlot {
            name: normalized_key,
            value: scalar_values.join(", "),
        });
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
    fn extracts_facts_from_tagged_history_payload() {
        let history = vec![ChatMessage::assistant(
            "<tool_call>\n{\"name\":\"weather_lookup\",\"arguments\":{\"city\":\"Berlin\"}}\n</tool_call>",
        )];

        let facts = extract_tool_facts_from_chat_history(&history, 0);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].tool_name, "weather_lookup");
        assert_eq!(facts[0].focus_entities[0].name, "Berlin");
    }

    #[test]
    fn extracts_facts_from_native_history_payload() {
        let history = vec![ChatMessage::assistant(
            serde_json::json!({
                "content": "done",
                "tool_calls": [
                    {"name": "service_status", "arguments": "{\"service\":\"synapseclaw\"}"}
                ]
            })
            .to_string(),
        )];

        let facts = extract_tool_facts_from_chat_history(&history, 0);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].focus_entities[0].kind, "service");
        assert_eq!(facts[0].focus_entities[0].name, "synapseclaw");
    }
}
