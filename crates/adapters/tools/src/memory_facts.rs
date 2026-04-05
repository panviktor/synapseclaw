use synapse_domain::domain::dialogue_state::{DialogueSlot, FocusEntity};
use synapse_domain::domain::memory::MemoryCategory;
use synapse_domain::ports::agent_runtime::AgentToolFact;

fn category_name(category: &MemoryCategory) -> String {
    match category {
        MemoryCategory::Core => "core".to_string(),
        MemoryCategory::Daily => "daily".to_string(),
        MemoryCategory::Conversation => "conversation".to_string(),
        MemoryCategory::Entity => "entity".to_string(),
        MemoryCategory::Skill => "skill".to_string(),
        MemoryCategory::Reflection => "reflection".to_string(),
        MemoryCategory::Custom(name) => name.clone(),
    }
}

pub(crate) fn build_memory_entry_fact(
    tool_name: &str,
    action: &str,
    key: &str,
    category: Option<&MemoryCategory>,
) -> AgentToolFact {
    let mut slots = vec![
        DialogueSlot::observed("memory_action", action.to_string()),
        DialogueSlot::observed("memory_key", key.to_string()),
    ];
    let metadata = category.map(category_name);
    if let Some(category) = category {
        slots.push(DialogueSlot::observed(
            "memory_category",
            category_name(category),
        ));
    }

    AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: vec![FocusEntity {
            kind: "memory_entry".into(),
            name: key.to_string(),
            metadata,
        }],
        slots,
    }
}

pub(crate) fn build_core_block_fact(tool_name: &str, action: &str, label: &str) -> AgentToolFact {
    AgentToolFact {
        tool_name: tool_name.to_string(),
        focus_entities: vec![FocusEntity {
            kind: "core_memory_block".into(),
            name: label.to_string(),
            metadata: Some(action.to_string()),
        }],
        slots: vec![
            DialogueSlot::observed("memory_action", action.to_string()),
            DialogueSlot::observed("core_memory_label", label.to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_memory_entry_fact_includes_category_metadata() {
        let fact = build_memory_entry_fact(
            "memory_store",
            "store",
            "user_lang",
            Some(&MemoryCategory::Core),
        );

        assert_eq!(fact.focus_entities[0].kind, "memory_entry");
        assert_eq!(fact.focus_entities[0].name, "user_lang");
        assert_eq!(fact.focus_entities[0].metadata.as_deref(), Some("core"));
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "memory_category" && slot.value == "core")
        );
    }

    #[test]
    fn build_core_block_fact_marks_label_and_action() {
        let fact = build_core_block_fact("core_memory_update", "append", "user_knowledge");

        assert_eq!(fact.focus_entities[0].kind, "core_memory_block");
        assert_eq!(fact.focus_entities[0].name, "user_knowledge");
        assert_eq!(fact.focus_entities[0].metadata.as_deref(), Some("append"));
        assert!(
            fact.slots
                .iter()
                .any(|slot| slot.name == "core_memory_label" && slot.value == "user_knowledge")
        );
    }
}
