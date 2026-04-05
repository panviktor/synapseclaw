use serde_json::Value;
use synapse_domain::domain::dialogue_state::DialogueSlot;
use synapse_domain::ports::agent_runtime::AgentToolFact;

pub(crate) fn build_tool_fact(tool_name: &str, arguments: &Value) -> AgentToolFact {
    let mut fact = AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: Vec::new(),
        slots: Vec::new(),
    };

    collect_argument_fact(None, arguments, &mut fact);

    fact.slots = dedupe_slots(fact.slots);
    fact
}

pub(crate) fn fact_has_payload(fact: &AgentToolFact) -> bool {
    !fact.focus_entities.is_empty() || !fact.slots.is_empty()
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
            if let Some(slot_value) = flatten_slot_value(value) {
                fact.slots
                    .push(DialogueSlot::observed(normalized_key, slot_value));
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
        assert!(fact.focus_entities.is_empty());
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
            .slots
            .iter()
            .any(|slot| slot.name == "target_channel" && slot.value == "matrix"));
        assert!(fact
            .slots
            .iter()
            .any(|slot| slot.name == "target_recipient" && slot.value == "!room:example.com"));
        assert!(fact
            .slots
            .iter()
            .any(|slot| slot.name == "target_thread_ref" && slot.value == "$thread"));
    }
}
