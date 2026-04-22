use crate::ports::realtime_call::RealtimeCallStartRequest;

const MAX_PROMPT_CHARS: usize = 480;
const MAX_OBJECTIVE_CHARS: usize = 240;
const MAX_CONTEXT_CHARS: usize = 480;
const MAX_AGENDA_ITEMS: usize = 6;
const MAX_AGENDA_ITEM_CHARS: usize = 160;
const DEFAULT_REALTIME_CALL_ANSWER_GREETING: &str = "Hello. I'm here.";

fn bounded_field(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut bounded = trimmed.chars().take(limit).collect::<String>();
    if trimmed.chars().count() > limit {
        bounded.push_str("...");
    }
    Some(bounded)
}

fn normalized_agenda_items(items: &[String]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| bounded_field(item, MAX_AGENDA_ITEM_CHARS))
        .take(MAX_AGENDA_ITEMS)
        .collect()
}

pub fn resolve_realtime_call_objective(request: &RealtimeCallStartRequest) -> Option<String> {
    request
        .objective
        .as_deref()
        .and_then(|value| bounded_field(value, MAX_OBJECTIVE_CHARS))
}

pub fn resolve_realtime_call_prompt(request: &RealtimeCallStartRequest) -> Option<String> {
    let explicit_prompt = request
        .prompt
        .as_deref()
        .and_then(|value| bounded_field(value, MAX_PROMPT_CHARS));
    let objective = resolve_realtime_call_objective(request);
    let context = request
        .context
        .as_deref()
        .and_then(|value| bounded_field(value, MAX_CONTEXT_CHARS));
    let agenda = normalized_agenda_items(&request.agenda);

    if objective.is_none() && context.is_none() && agenda.is_empty() {
        return explicit_prompt;
    }

    let mut sections = vec![
        "This is a live assistant voice call.".to_string(),
        "Keep turns short, speak naturally, ask at most one follow-up question at a time, and confirm any accepted action out loud.".to_string(),
    ];

    if let Some(objective) = objective {
        sections.push(format!("Primary task: {objective}"));
    }
    if let Some(context) = context {
        sections.push(format!("Call context: {context}"));
    }
    if !agenda.is_empty() {
        sections.push(format!("Suggested flow:\n- {}", agenda.join("\n- ")));
    }
    if let Some(explicit_prompt) = explicit_prompt {
        sections.push(format!(
            "Additional operator instructions: {explicit_prompt}"
        ));
    }

    Some(sections.join("\n\n"))
}

pub fn default_realtime_call_answer_greeting() -> &'static str {
    DEFAULT_REALTIME_CALL_ANSWER_GREETING
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::realtime_call::RealtimeCallOrigin;

    #[test]
    fn returns_explicit_prompt_when_no_structured_call_fields_exist() {
        let request = RealtimeCallStartRequest {
            to: "+15551234567".into(),
            prompt: Some("Say hello.".into()),
            origin: RealtimeCallOrigin::default(),
            objective: None,
            context: None,
            agenda: Vec::new(),
        };
        assert_eq!(
            resolve_realtime_call_prompt(&request).as_deref(),
            Some("Say hello.")
        );
    }

    #[test]
    fn builds_bounded_generic_call_prompt() {
        let request = RealtimeCallStartRequest {
            to: "+15551234567".into(),
            prompt: Some("Close by confirming the final answer out loud.".into()),
            origin: RealtimeCallOrigin::default(),
            objective: Some("Call the restaurant and reserve a table for two at 19:00.".into()),
            context: Some("Prefer a quiet place near Alexanderplatz. Budget is mid-range.".into()),
            agenda: vec![
                "Ask whether they have availability at 19:00.".into(),
                "If not, ask for the nearest available slot after 19:00.".into(),
                "Confirm the reservation details before ending the call.".into(),
            ],
        };

        let prompt = resolve_realtime_call_prompt(&request).expect("structured prompt");
        assert!(prompt.contains("live assistant voice call"));
        assert!(prompt.contains("Primary task: Call the restaurant"));
        assert!(prompt.contains("Call context: Prefer a quiet place"));
        assert!(prompt.contains("- Ask whether they have availability at 19:00."));
        assert!(prompt.contains("Additional operator instructions: Close by confirming"));
    }

    #[test]
    fn drops_empty_agenda_items_and_bounds_large_fields() {
        let request = RealtimeCallStartRequest {
            to: "+15551234567".into(),
            prompt: None,
            origin: RealtimeCallOrigin::default(),
            objective: Some("Call the store and ask whether the item is in stock.".into()),
            context: Some("x".repeat(MAX_CONTEXT_CHARS + 20)),
            agenda: vec![
                String::new(),
                "   ".into(),
                "Keep the first real item.".into(),
                "y".repeat(MAX_AGENDA_ITEM_CHARS + 20),
            ],
        };

        let prompt = resolve_realtime_call_prompt(&request).expect("structured prompt");
        assert!(prompt.contains("Keep the first real item."));
        assert!(prompt.contains("..."));
        assert!(!prompt.contains("\n- \n-"));
    }

    #[test]
    fn provides_short_default_answer_greeting() {
        assert_eq!(default_realtime_call_answer_greeting(), "Hello. I'm here.");
    }
}
