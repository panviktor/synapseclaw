use synapse_domain::domain::dialogue_state::FocusEntity;
use synapse_domain::domain::memory::MemoryCategory;
use synapse_domain::domain::tool_fact::TypedToolFact;

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
    _action: &str,
    key: &str,
    category: Option<&MemoryCategory>,
) -> TypedToolFact {
    let metadata = category.map(category_name);

    TypedToolFact::focus(
        tool_name.to_string(),
        vec![FocusEntity {
            kind: "memory_entry".into(),
            name: key.to_string(),
            metadata,
        }],
        Vec::new(),
    )
}

pub(crate) fn build_core_block_fact(tool_name: &str, action: &str, label: &str) -> TypedToolFact {
    TypedToolFact::focus(
        tool_name.to_string(),
        vec![FocusEntity {
            kind: "core_memory_block".into(),
            name: label.to_string(),
            metadata: Some(action.to_string()),
        }],
        Vec::new(),
    )
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

        assert_eq!(fact.focus_entities()[0].kind, "memory_entry");
        assert_eq!(fact.focus_entities()[0].name, "user_lang");
        assert_eq!(fact.focus_entities()[0].metadata.as_deref(), Some("core"));
        assert!(fact.subjects().is_empty());
    }

    #[test]
    fn build_core_block_fact_marks_label_and_action() {
        let fact = build_core_block_fact("core_memory_update", "append", "user_knowledge");

        assert_eq!(fact.focus_entities()[0].kind, "core_memory_block");
        assert_eq!(fact.focus_entities()[0].name, "user_knowledge");
        assert_eq!(fact.focus_entities()[0].metadata.as_deref(), Some("append"));
        assert!(fact.subjects().is_empty());
    }
}
